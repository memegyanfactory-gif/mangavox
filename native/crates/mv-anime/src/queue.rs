//! The engine's own persisted job queue (design 5.7), modelled on Studio's `pipeline.rs`:
//! `anime/queue.json` journal, one lane per kind of work (one job at a time per lane; Llm up to
//! `settings.llm_parallel`), a version counter pages poll, and a held queue after a restart
//! (Running -> Queued, `interrupted`, waits for Resume). Transient failures retry at most twice;
//! never a picture job (the Web lane: a retry would press Generate again on the user's account),
//! and never on sign-in, a human check, a site warning, a closed window or a paid label (those
//! pause the Web lane instead).
//!
//! The journal is written (atomically) on every change of state; progress lines are written at
//! most every two seconds, by a background writer from a copy taken under the queue's lock, so
//! no caller ever waits for the disk (a newer copy is never overwritten by an older one; call
//! [`flush_journal`] when the app closes). Only the newest [`KEEP_FINISHED`]
//! finished jobs are kept. A job id is never reused, and a job runs only from `Queued`, so a
//! finished job never runs again and an interrupted one runs again only after Resume.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use futures::future::BoxFuture;
use futures::FutureExt as _;
use mv_ai::CancellationToken;
use serde::{Deserialize, Serialize};

use crate::backend::{transient_text, ImageBackend, ImageError, Py};
use crate::llm::Ai;
use crate::model::Target;
use crate::not_built;
use crate::store::Store;

/// Automatic retries of a transient failure (timeout, site "busy").
pub const AUTO_RETRIES: u8 = 2;
/// Wait before a transient failure is run again.
const RETRY_DELAY: Duration = Duration::from_secs(5);
/// Least time between two journal writes for progress alone.
const PROGRESS_SAVE: Duration = Duration::from_secs(2);
/// How often the runner looks for work when nothing wakes it.
const TICK: Duration = Duration::from_millis(500);
/// Finished (done or failed) jobs kept in the list and the journal; older ones are dropped.
pub const KEEP_FINISHED: usize = 200;

/// Where a job runs; each lane runs one job at a time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    #[default]
    Llm,
    /// The Magnific website (one picture at a time app-wide).
    Web,
    Voice,
    /// ffmpeg and other CPU work.
    Render,
}

impl Lane {
    /// Every lane, in the order the runner fills them.
    pub const ALL: [Lane; 4] = [Lane::Llm, Lane::Web, Lane::Voice, Lane::Render];
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Where a job is in its life.
pub enum JobState {
    #[default]
    Queued,
    Running,
    Paused,
    Done,
    Failed,
}

/// One job of the journal.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Job {
    pub id: u64,
    /// Series id.
    pub project: String,
    pub kind: JobKind,
    pub lane: Lane,
    pub state: JobState,
    pub label: String,
    pub progress: f32,
    /// Last status text.
    pub line: String,
    pub error: Option<String>,
    pub tries: u8,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub hidden: bool,
}

/// What a job does.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobKind {
    Story { episode: String },
    /// "Adapt this scene again" (Story step): one scene of the episode.
    AdaptScene { episode: String, scene: String },
    Bible { episode: String },
    PlanScene { episode: String, scene: String },
    Frame { episode: String, shot: String, variants: u8 },
    Sheet { target: Target, variants: u8 },
    Crops { character: String },
    Voice { episode: String, line: String },
    Audition { character: String, voice: String },
    Animatic { episode: String },
    /// A kind written by a newer MangaVox; fails with a clear message.
    #[default]
    #[serde(other)]
    Unknown,
}

impl JobKind {
    /// The lane this kind runs in.
    pub fn lane(&self) -> Lane {
        match self {
            JobKind::Story { .. } | JobKind::AdaptScene { .. } | JobKind::Bible { .. } | JobKind::PlanScene { .. } | JobKind::Unknown => Lane::Llm,
            JobKind::Frame { .. } | JobKind::Sheet { .. } => Lane::Web,
            JobKind::Voice { .. } | JobKind::Audition { .. } => Lane::Voice,
            JobKind::Crops { .. } | JobKind::Animatic { .. } => Lane::Render,
        }
    }

    /// Variant name, the key of the ETA table in `settings.json`.
    pub fn name(&self) -> &'static str {
        match self {
            JobKind::Story { .. } => "story",
            JobKind::AdaptScene { .. } => "adapt_scene",
            JobKind::Bible { .. } => "bible",
            JobKind::PlanScene { .. } => "plan_scene",
            JobKind::Frame { .. } => "frame",
            JobKind::Sheet { .. } => "sheet",
            JobKind::Crops { .. } => "crops",
            JobKind::Voice { .. } => "voice",
            JobKind::Audition { .. } => "audition",
            JobKind::Animatic { .. } => "animatic",
            JobKind::Unknown => "unknown",
        }
    }

    /// What the job does, in plain words for the user ("Planning the shots").
    pub fn title(&self) -> &'static str {
        match self {
            JobKind::Story { .. } => "Writing the story",
            JobKind::AdaptScene { .. } => "Adapting the scene",
            JobKind::Bible { .. } => "Building the series bible",
            JobKind::PlanScene { .. } => "Planning the shots",
            JobKind::Frame { .. } => "Drawing a frame",
            JobKind::Sheet { .. } => "Drawing a reference sheet",
            JobKind::Crops { .. } => "Cutting out character pictures",
            JobKind::Voice { .. } => "Voicing lines",
            JobKind::Audition { .. } => "Auditioning a voice",
            JobKind::Animatic { .. } => "Making the animatic",
            JobKind::Unknown => "A job from a newer MangaVox",
        }
    }
}

/// `anime/queue.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Queue {
    pub paused: bool,
    /// MangaVox closed while jobs ran; held until Resume.
    pub interrupted: bool,
    pub next_id: u64,
    pub jobs: Vec<Job>,
    /// Lanes paused on their own (the Web lane after a sign-in or paid-label stop).
    pub paused_lanes: Vec<Lane>,
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn warn(msg: &str) {
    mv_core::logging::global().warn("anime", msg);
}

fn info(msg: &str) {
    mv_core::logging::global().info("anime", msg);
}

impl Queue {
    /// The journal as the app finds it at start: a job that was running when MangaVox closed is
    /// queued again (never lost), and the queue is held (`interrupted`) while any job waits,
    /// until the user presses Resume. Missing file: an empty queue. A corrupt file is set aside
    /// as `queue.json.corrupt-<unix_ms>` (logged) and the queue starts empty.
    pub fn load(path: &Path) -> Queue {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Queue { next_id: 1, ..Queue::default() },
            Err(e) => {
                warn(&format!("Could not read the job queue {}: {e}", path.display()));
                return Queue { next_id: 1, ..Queue::default() };
            }
        };
        let mut q: Queue = match serde_json::from_slice(&bytes) {
            Ok(q) => q,
            Err(e) => {
                let aside = PathBuf::from(format!("{}.corrupt-{}", path.display(), now_ms()));
                let moved = std::fs::rename(path, &aside).is_ok();
                warn(&format!(
                    "The job queue {} could not be read ({e}); starting with an empty queue{}.",
                    path.display(),
                    if moved { format!(" (the old file is kept as {})", aside.display()) } else { String::new() }
                ));
                return Queue { next_id: 1, ..Queue::default() };
            }
        };
        for job in &mut q.jobs {
            if job.state == JobState::Running {
                job.state = JobState::Queued;
                job.progress = 0.;
                job.started_at = None;
                job.line = "Stopped when MangaVox closed; runs again after Resume.".into();
            }
        }
        if q.jobs.iter().any(|j| j.state == JobState::Queued) {
            q.interrupted = true;
        }
        let top = q.jobs.iter().map(|j| j.id).max().unwrap_or(0);
        q.next_id = q.next_id.max(top + 1).max(1);
        q.paused_lanes.dedup();
        q
    }

    /// Atomic write: temp file, `sync_all`, rename over the journal.
    pub fn save(&self, path: &Path) -> Result<()> {
        write_journal(path, &serde_json::to_vec_pretty(self)?)
    }

    /// Adds a job and answers its id. An identical request (same project and kind) is not added
    /// twice: a queued copy answers for it; a paused copy is queued again (the user asked for it
    /// once more); a running copy gets one follow-up copy queued behind it, because the work it
    /// runs was read when it started (a picture job excepted: a second press while it draws is
    /// the same job, never a second Generate). Never reuses an id.
    pub fn add(&mut self, kind: JobKind, project: &str, label: String) -> u64 {
        self.add_changed(kind, project, label).0
    }

    /// [`Queue::add`], and whether the queue changed.
    fn add_changed(&mut self, kind: JobKind, project: &str, label: String) -> (u64, bool) {
        let same = |j: &Job| j.project == project && j.kind == kind;
        if let Some(j) = self.jobs.iter().find(|j| j.state == JobState::Queued && same(j)) {
            return (j.id, false);
        }
        if let Some(j) = self.jobs.iter_mut().find(|j| j.state == JobState::Paused && same(j)) {
            j.state = JobState::Queued;
            j.progress = 0.;
            j.line = String::new();
            return (j.id, true);
        }
        if let Some(j) = self.jobs.iter().find(|j| j.state == JobState::Running && same(j)) {
            if kind.lane() == Lane::Web {
                return (j.id, false);
            }
        }
        let id = self.next_id.max(self.jobs.iter().map(|j| j.id + 1).max().unwrap_or(1)).max(1);
        self.next_id = id + 1;
        let lane = kind.lane();
        self.jobs.push(Job { id, project: project.to_string(), kind, lane, label, created_at: now_secs(), ..Job::default() });
        (id, true)
    }

    /// Drops the oldest finished (done or failed) jobs beyond the newest `keep`; answers how
    /// many went. Waiting and running jobs are never touched.
    pub fn prune_finished(&mut self, keep: usize) -> usize {
        let mut finished: Vec<(i64, u64)> =
            self.jobs.iter().filter(|j| matches!(j.state, JobState::Done | JobState::Failed)).map(|j| (j.finished_at.unwrap_or(j.created_at), j.id)).collect();
        if finished.len() <= keep {
            return 0;
        }
        finished.sort_unstable_by(|a, b| b.cmp(a));
        let gone: HashSet<u64> = finished[keep..].iter().map(|f| f.1).collect();
        self.jobs.retain(|j| !gone.contains(&j.id));
        gone.len()
    }

    /// The job with this id.
    pub fn job(&self, id: u64) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == id)
    }

    fn job_mut(&mut self, id: u64) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    /// Nothing starts: paused by the user, or held after a restart.
    pub fn held(&self) -> bool {
        self.paused || self.interrupted
    }

    /// This lane waits for the user (see [`pause_lane`]).
    pub fn lane_paused(&self, lane: Lane) -> bool {
        self.paused_lanes.contains(&lane)
    }

    /// Removes the jobs that have not started (queued or paused), of one project or of all;
    /// returns their ids. Running and finished jobs stay.
    pub fn drop_waiting(&mut self, project: Option<&str>) -> Vec<u64> {
        let gone: Vec<u64> = self.jobs.iter().filter(|j| matches!(j.state, JobState::Queued | JobState::Paused) && project.is_none_or(|p| j.project == p)).map(|j| j.id).collect();
        self.jobs.retain(|j| !gone.contains(&j.id));
        self.settle_hold();
        gone
    }

    /// Removes every unfinished job of a project (queued, paused, running or failed); returns
    /// their ids. Done jobs stay in the list until "Clear finished".
    pub fn drop_project(&mut self, project: &str) -> Vec<u64> {
        let gone: Vec<u64> = self.jobs.iter().filter(|j| j.project == project && j.state != JobState::Done).map(|j| j.id).collect();
        self.jobs.retain(|j| !gone.contains(&j.id));
        self.settle_hold();
        gone
    }

    /// The restart hold only stands while something still waits to run.
    fn settle_hold(&mut self) {
        if self.interrupted && !self.jobs.iter().any(|j| j.state == JobState::Queued) {
            self.interrupted = false;
        }
    }
}

/// Atomic write of a serialized journal: temp file, `sync_all`, rename (a few tries: Windows
/// refuses the replace while a virus scanner has the file open for a moment).
fn write_journal(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write as _;
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().ok_or_else(|| anyhow!("The job queue has no folder to live in."))?;
    std::fs::create_dir_all(dir).map_err(|e| anyhow!("Could not create the folder {}: {e}", dir.display()))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "queue.json".into());
    let tmp = dir.join(format!("{name}.tmp-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        let mut last = Ok(());
        for attempt in 0..4u64 {
            last = std::fs::rename(&tmp, path);
            if last.is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(30 * (attempt + 1)));
        }
        last
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        anyhow!("Could not save the job queue to {}: {e}", path.display())
    })
}

// ---------------------------------------------------------------- the process-wide queue

struct Active {
    lane: Lane,
    project: String,
    cancel: CancellationToken,
}

/// A copy of the queue made under its lock (in memory only), for the journal writer.
struct PendingSave {
    seq: u64,
    path: PathBuf,
    q: Queue,
}

/// The journal writer: one background thread writes the newest copy handed to it, so neither
/// the screens (`enqueue`, `snapshot`) nor the runner ever wait for the disk. Copies that pile
/// up while it writes collapse into the newest; an older copy never lands over a newer one.
struct Writer {
    slot: Mutex<WriterState>,
    cv: Condvar,
    /// The thread could not be started: copies are written by the caller instead.
    inline: bool,
}

#[derive(Default)]
struct WriterState {
    pending: Option<PendingSave>,
    /// Newest copy handed over / newest copy on disk. A copy that failed to write does not count
    /// as written: [`flush_journal`] answers `false` until a later try lands it (or a newer one).
    latest: u64,
    written: u64,
}

/// First wait before a failed journal write is tried again; it doubles up to [`RETRY_MAX`].
const RETRY_FIRST: Duration = Duration::from_millis(250);
const RETRY_MAX: Duration = Duration::from_secs(5);

/// What the writer does once a copy was tried: on success it counts as written; on failure it
/// is put back to be tried again unless a newer copy is already waiting (that one replaces it).
/// Answers whether the copy landed.
fn after_write(st: &mut WriterState, p: PendingSave, ok: bool) -> bool {
    if ok {
        st.written = st.written.max(p.seq);
        return true;
    }
    if st.pending.as_ref().is_none_or(|newer| newer.seq < p.seq) && p.seq > st.written {
        st.pending = Some(p);
    }
    false
}

fn writer() -> &'static Writer {
    static W: OnceLock<Writer> = OnceLock::new();
    W.get_or_init(|| {
        let started = std::thread::Builder::new().name("mv-anime-queue-journal".into()).spawn(|| {
            // Failed writes in a row (the drive asleep or gone, the file held by another program).
            let mut failures: u32 = 0;
            loop {
                let w = writer();
                let p = {
                    let mut st = w.slot.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        if let Some(p) = st.pending.take() {
                            break p;
                        }
                        st = w.cv.wait(st).unwrap_or_else(|e| e.into_inner());
                    }
                };
                let result = write_copy(&p);
                let ok = result.is_ok();
                {
                    let mut st = w.slot.lock().unwrap_or_else(|e| e.into_inner());
                    after_write(&mut st, p, ok);
                    w.cv.notify_all();
                }
                // Logged with no lock held.
                match result {
                    Ok(()) if failures > 0 => {
                        info(&format!("the job queue was saved after {failures} failed tries"));
                        failures = 0;
                    }
                    Ok(()) => {}
                    Err(e) => {
                        failures = failures.saturating_add(1);
                        // The first failure of a run, then now and then (a gone drive would fill the log).
                        if failures == 1 || failures % 60 == 0 {
                            warn(&format!("{e:#} (trying again; {failures} failed tries so far)"));
                        }
                        let wait = RETRY_FIRST.saturating_mul(1u32 << failures.min(5).saturating_sub(1)).min(RETRY_MAX);
                        std::thread::sleep(wait);
                    }
                }
            }
        });
        Writer { slot: Mutex::new(WriterState::default()), cv: Condvar::new(), inline: started.is_err() }
    })
}

fn write_copy(p: &PendingSave) -> Result<()> {
    serde_json::to_vec_pretty(&p.q).map_err(anyhow::Error::from).and_then(|bytes| write_journal(&p.path, &bytes))
}

/// Hands a journal copy to the writer (never waits for the disk).
fn write_pending(p: Option<PendingSave>) {
    let Some(p) = p else { return };
    let w = writer();
    let mut st = w.slot.lock().unwrap_or_else(|e| e.into_inner());
    if p.seq <= st.latest {
        return;
    }
    st.latest = p.seq;
    if w.inline {
        drop(st);
        // No writer thread: the callers write themselves, one copy at a time, and a copy older
        // than one already on disk is skipped, so an older copy never lands over a newer one.
        static INLINE: Mutex<()> = Mutex::new(());
        let _one = INLINE.lock().unwrap_or_else(|e| e.into_inner());
        if w.slot.lock().unwrap_or_else(|e| e.into_inner()).written >= p.seq {
            return;
        }
        let result = write_copy(&p);
        let ok = result.is_ok();
        {
            let mut st = w.slot.lock().unwrap_or_else(|e| e.into_inner());
            if ok {
                st.written = st.written.max(p.seq);
            }
            w.cv.notify_all();
        }
        if let Err(e) = result {
            warn(&format!("{e:#}"));
        }
        return;
    }
    st.pending = Some(p);
    w.cv.notify_all();
}

/// Waits (blocking, off the UI thread) until the journal on disk holds the queue's newest state,
/// or `wait` is up; answers whether it does. Call it when MangaVox closes, so the last change is
/// never lost (the writer otherwise lands each change within moments).
pub fn flush_journal(wait: Duration) -> bool {
    let w = writer();
    let until = Instant::now() + wait;
    let mut st = w.slot.lock().unwrap_or_else(|e| e.into_inner());
    while st.written < st.latest {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        st = w.cv.wait_timeout(st, left).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
    }
    true
}

struct State {
    q: Queue,
    path: PathBuf,
    /// Jobs whose task is running (a paused or cancelled job's task may still be winding down).
    active: HashMap<u64, Active>,
    /// Transient failures wait until then before they run again.
    retry_after: HashMap<u64, Instant>,
    last_save: Instant,
    /// Numbers the journal copies, so an older one never overwrites a newer one.
    save_seq: u64,
    llm_parallel: usize,
    /// Log lines made while the lock is held; [`change`] writes them once it has let go (the log
    /// appends to a file, and the UI thread takes this lock).
    logs: Vec<(bool, String)>,
}

static VERSION: AtomicU64 = AtomicU64::new(0);

fn state() -> MutexGuard<'static, State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    let m = S.get_or_init(|| {
        let path = crate::store::root().join("queue.json");
        Mutex::new(State {
            q: Queue::load(&path),
            path,
            active: HashMap::new(),
            retry_after: HashMap::new(),
            last_save: Instant::now(),
            save_seq: 0,
            llm_parallel: 1,
            logs: Vec::new(),
        })
    });
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Loads the queue file once (it reads the data drive). Call it from a background thread at
/// start-up, so the first screen draw that shows the queue never waits for the disk.
pub fn warm() {
    drop(state());
}

/// Writes log lines collected under the lock (call with no lock held).
fn write_logs(logs: Vec<(bool, String)>) {
    for (is_warn, line) in logs {
        if is_warn {
            warn(&line);
        } else {
            info(&line);
        }
    }
}

fn wake() -> &'static tokio::sync::Notify {
    static N: OnceLock<tokio::sync::Notify> = OnceLock::new();
    N.get_or_init(tokio::sync::Notify::new)
}

impl State {
    /// The journal as it is now (old finished jobs dropped first), to write once the lock is
    /// let go ([`write_pending`]).
    fn pending_save(&mut self) -> Option<PendingSave> {
        self.last_save = Instant::now();
        self.q.prune_finished(KEEP_FINISHED);
        self.save_seq += 1;
        Some(PendingSave { seq: self.save_seq, path: self.path.clone(), q: self.q.clone() })
    }

    /// Jobs of `project` whose task is still running or winding down.
    fn busy(&self, project: &str) -> bool {
        self.active.values().any(|a| a.project == project)
    }
}

/// Applies a change; when it changed something: journal written (after the lock is let go),
/// version bumped, runner woken.
fn change<R>(f: impl FnOnce(&mut State) -> (bool, R)) -> R {
    let mut s = state();
    let (changed, r) = f(&mut s);
    let logs = std::mem::take(&mut s.logs);
    let pending = if changed { s.pending_save() } else { None };
    drop(s);
    write_logs(logs);
    if changed {
        write_pending(pending);
        VERSION.fetch_add(1, Ordering::SeqCst);
        wake().notify_one();
    }
    r
}

/// Adds a job (see [`Queue::add`] for identical requests); returns its id.
pub fn enqueue(kind: JobKind, project: &str, label: String) -> u64 {
    change(|s| {
        let name = kind.name();
        let (id, changed) = s.q.add_changed(kind, project, label);
        if changed {
            s.retry_after.remove(&id);
            s.logs.push((false, format!("job {id} queued ({name}, project {project})")));
        }
        (changed, id)
    })
}

/// Holds one job: a queued job waits, a running job is stopped (it starts over on Resume).
pub fn pause(id: u64) {
    change(|s| {
        let running = s.active.get(&id).map(|a| a.cancel.clone());
        let Some(job) = s.q.job_mut(id) else { return (false, ()) };
        match job.state {
            JobState::Queued | JobState::Running => {
                // The user's pause is not a try: the run it stops gives its try back.
                if job.state == JobState::Running {
                    job.tries = job.tries.saturating_sub(1);
                }
                job.state = JobState::Paused;
                job.line = "Paused".into();
                if let Some(token) = running {
                    token.cancel();
                }
                (true, ())
            }
            _ => (false, ()),
        }
    })
}

/// A paused job waits for its turn again.
pub fn resume(id: u64) {
    change(|s| match s.q.job_mut(id) {
        Some(job) if job.state == JobState::Paused => {
            job.state = JobState::Queued;
            job.progress = 0.;
            job.line = String::new();
            (true, ())
        }
        _ => (false, ()),
    })
}

/// A failed (or paused) job runs again from the start, with its automatic retries renewed.
pub fn retry(id: u64) {
    change(|s| {
        s.retry_after.remove(&id);
        match s.q.job_mut(id) {
            Some(job) if matches!(job.state, JobState::Failed | JobState::Paused) => {
                job.state = JobState::Queued;
                job.error = None;
                job.tries = 0;
                job.progress = 0.;
                job.line = String::new();
                job.started_at = None;
                job.finished_at = None;
                job.hidden = false;
                (true, ())
            }
            _ => (false, ()),
        }
    })
}

/// Removes a job; a running one is stopped first.
pub fn cancel(id: u64) {
    change(|s| {
        if let Some(a) = s.active.get(&id) {
            a.cancel.cancel();
        }
        s.retry_after.remove(&id);
        let before = s.q.jobs.len();
        s.q.jobs.retain(|j| j.id != id);
        let changed = s.q.jobs.len() != before;
        if changed {
            s.logs.push((false, format!("job {id} cancelled")));
            s.q.settle_hold();
        }
        (changed, ())
    })
}

/// Removes every job that has not started yet (queued or paused), of one project or of all;
/// returns how many went. Running jobs carry on.
pub fn cancel_waiting(project: Option<&str>) -> usize {
    change(|s| {
        let gone = s.q.drop_waiting(project);
        for id in &gone {
            s.retry_after.remove(id);
        }
        if !gone.is_empty() {
            s.logs.push((false, format!("{} waiting job(s) cancelled", gone.len())));
        }
        (!gone.is_empty(), gone.len())
    })
}

/// Removes every unfinished job of a project, stopping a running one (the project left the
/// list); returns how many went.
pub fn cancel_project(project: &str) -> usize {
    change(|s| {
        // Every task of the project, also a paused one still winding down.
        for a in s.active.values().filter(|a| a.project == project) {
            a.cancel.cancel();
        }
        let gone = s.q.drop_project(project);
        for id in &gone {
            s.retry_after.remove(id);
        }
        if !gone.is_empty() {
            s.logs.push((false, format!("{} job(s) of project {project} cancelled", gone.len())));
        }
        (!gone.is_empty(), gone.len())
    })
}

/// [`cancel_project`], then waits (blocking: call it off the UI thread) until no task of the
/// project is still winding down, or `wait` is up; answers whether all of them stopped. Call it
/// before the project's folder is moved or deleted. Meanwhile, and until the last task ends,
/// nothing a task still writes can make the project's folder again once it is gone.
pub fn cancel_project_wait(project: &str, wait: Duration) -> bool {
    let dir = Store::open(project).dir;
    crate::store::set_closing(&dir, true);
    cancel_project(project);
    let until = Instant::now() + wait;
    loop {
        if !state().busy(project) {
            crate::store::set_closing(&dir, false);
            return true;
        }
        if Instant::now() >= until {
            warn(&format!("project {project}: a job was still stopping after {:.0} s", wait.as_secs_f32()));
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Starts nothing new (running jobs finish).
pub fn pause_all() {
    change(|s| (!std::mem::replace(&mut s.q.paused, true), ()))
}

/// Lets the queue run again, also after a restart (clears `interrupted`).
pub fn resume_all() {
    change(|s| {
        let changed = s.q.paused || s.q.interrupted;
        s.q.paused = false;
        s.q.interrupted = false;
        (changed, ())
    })
}

/// Pauses one lane only (the Web lane after a sign-in, human-check or paid-label stop).
pub fn pause_lane(lane: Lane) {
    change(|s| {
        if s.q.paused_lanes.contains(&lane) {
            return (false, ());
        }
        s.q.paused_lanes.push(lane);
        (true, ())
    })
}

/// Lets a lane paused by [`pause_lane`] run again.
pub fn resume_lane(lane: Lane) {
    change(|s| {
        let before = s.q.paused_lanes.len();
        s.q.paused_lanes.retain(|l| *l != lane);
        (s.q.paused_lanes.len() != before, ())
    })
}

/// Removes finished (done) jobs from the list.
pub fn clear_finished() {
    change(|s| {
        let before = s.q.jobs.len();
        s.q.jobs.retain(|j| j.state != JobState::Done);
        (s.q.jobs.len() != before, ())
    })
}

/// Hides (or shows again) every failed job in the list.
pub fn set_hidden_failed(hide: bool) {
    change(|s| {
        let mut changed = false;
        for j in s.q.jobs.iter_mut().filter(|j| j.state == JobState::Failed && j.hidden != hide) {
            j.hidden = hide;
            changed = true;
        }
        (changed, ())
    })
}

/// Bumped on every change of the queue.
pub fn version() -> u64 {
    VERSION.load(Ordering::SeqCst)
}

/// A copy of the whole queue as it is now (for pages to draw).
pub fn snapshot() -> Queue {
    state().q.clone()
}

/// Learned length of a job kind in seconds: the mean of its last finished runs (`ETA_KEEP` of
/// them, kept in `settings.json`); `None` before the first one finished.
pub fn eta(kind: &JobKind) -> Option<f32> {
    let settings = crate::settings::load();
    let runs: Vec<f32> = settings.eta.get(kind.name())?.iter().copied().filter(|s| s.is_finite() && *s >= 0.).collect();
    (!runs.is_empty()).then(|| runs.iter().sum::<f32>() / runs.len() as f32)
}

/// Seconds a job still needs, from [`eta`] and its progress (`None` when unknown or finished).
pub fn remaining_secs(job: &Job) -> Option<f32> {
    match job.state {
        JobState::Done | JobState::Failed => None,
        _ => eta(&job.kind).map(|e| e * (1. - job.progress.clamp(0., 1.))),
    }
}

fn record_eta(kind: &JobKind, secs: f32) {
    if !secs.is_finite() || kind == &JobKind::Unknown {
        return;
    }
    let mut settings = crate::settings::load();
    let runs = settings.eta.entry(kind.name().to_string()).or_default();
    runs.push(secs);
    let extra = runs.len().saturating_sub(crate::settings::ETA_KEEP);
    runs.drain(..extra);
    if let Err(e) = crate::settings::save(&settings) {
        warn(&format!("Could not save the job time estimates: {e:#}"));
    }
}

// ---------------------------------------------------------------- the runner

/// Which model tier a step asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tier {
    /// Story, bible, shots, revise: the project's planner (High effort).
    Planner,
    /// JSON repair, short rewrites: the project's helper (Low effort).
    Helper,
}

/// What the UI hands the runner once (`start_runner`).
pub struct RunCtx {
    /// The AI for a tier; `None` when no provider is set up.
    pub ai: Box<dyn Fn(Tier) -> Option<Ai> + Send + Sync>,
    pub image: Arc<dyn ImageBackend>,
    pub python: Box<dyn Fn() -> Result<Py> + Send + Sync>,
}

/// Everything one running job gets: the step functions' first argument.
pub struct JobCtx<'a> {
    pub job: &'a Job,
    pub store: &'a Store,
    pub run: &'a RunCtx,
    /// (fraction 0..1, status line).
    pub progress: &'a (dyn Fn(f32, &str) + Send + Sync),
    pub cancel: &'a CancellationToken,
}

/// What runs a job: [`execute`] in the app; tests hand [`start_runner_with`] their own.
pub type Exec = Arc<dyn for<'a> Fn(&'a JobCtx<'a>) -> BoxFuture<'a, Result<()>> + Send + Sync>;

/// Starts the runner on `rt` (idempotent).
pub fn start_runner(rt: &tokio::runtime::Handle, ctx: Arc<RunCtx>) {
    start_runner_with(rt, ctx, Arc::new(run_step));
}

fn run_step<'a>(cx: &'a JobCtx<'a>) -> BoxFuture<'a, Result<()>> {
    Box::pin(execute(cx))
}

/// [`start_runner`] with another way to run a job (tests). Only the first call in a process
/// starts a runner.
pub fn start_runner_with(rt: &tokio::runtime::Handle, ctx: Arc<RunCtx>, exec: Exec) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let parallel = crate::settings::load().llm_parallel.clamp(1, crate::settings::LLM_PARALLEL_MAX) as usize;
    state().llm_parallel = parallel;
    let handle = rt.clone();
    rt.spawn(async move {
        loop {
            for (job, token) in take_startable() {
                let (ctx, exec) = (ctx.clone(), exec.clone());
                handle.spawn(run_one(job, token, ctx, exec));
            }
            tokio::select! {
                _ = wake().notified() => {}
                _ = tokio::time::sleep(TICK) => {}
            }
        }
    });
}

/// Marks the jobs that may start now as running and hands them out.
fn take_startable() -> Vec<(Job, CancellationToken)> {
    change(|s| {
        let mut out = Vec::new();
        if s.q.held() {
            return (false, out);
        }
        let now = Instant::now();
        s.retry_after.retain(|_, t| *t > now);
        for lane in Lane::ALL {
            if s.q.lane_paused(lane) {
                continue;
            }
            let cap = if lane == Lane::Llm { s.llm_parallel.max(1) } else { 1 };
            let mut busy = s.active.values().filter(|a| a.lane == lane).count();
            while busy < cap {
                let (active, waiting) = (&s.active, &s.retry_after);
                let Some(job) = s.q.jobs.iter_mut().find(|j| j.state == JobState::Queued && j.lane == lane && !active.contains_key(&j.id) && !waiting.contains_key(&j.id)) else {
                    break;
                };
                job.state = JobState::Running;
                job.started_at = Some(now_secs());
                job.finished_at = None;
                job.tries = job.tries.saturating_add(1);
                job.progress = 0.;
                job.error = None;
                job.line = "Starting…".into();
                s.logs.push((false, format!("job {} started: {} ({}, project {}, try {})", job.id, job.label, job.kind.name(), job.project, job.tries)));
                let token = CancellationToken::new();
                out.push((job.clone(), token.clone()));
                s.active.insert(job.id, Active { lane, project: job.project.clone(), cancel: token });
                busy += 1;
            }
        }
        (!out.is_empty(), out)
    })
}

/// A progress report of a running job (the journal is written at most every two seconds).
fn report(id: u64, fraction: f32, line: &str) {
    let mut s = state();
    let Some(job) = s.q.job_mut(id).filter(|j| j.state == JobState::Running) else { return };
    if fraction.is_finite() {
        job.progress = fraction.clamp(0., 1.);
    }
    if !line.is_empty() {
        job.line = line.to_string();
    }
    let pending = if s.last_save.elapsed() >= PROGRESS_SAVE { s.pending_save() } else { None };
    drop(s);
    write_pending(pending);
    VERSION.fetch_add(1, Ordering::SeqCst);
}

async fn run_one(job: Job, token: CancellationToken, ctx: Arc<RunCtx>, exec: Exec) {
    let id = job.id;
    let store = Store::open(&job.project);
    let progress = move |fraction: f32, line: &str| report(id, fraction, line);
    let started = Instant::now();
    let result = {
        let cx = JobCtx { job: &job, store: &store, run: &ctx, progress: &progress, cancel: &token };
        match std::panic::AssertUnwindSafe(exec(&cx)).catch_unwind().await {
            Ok(r) => r,
            Err(_) => Err(anyhow!("{} stopped unexpectedly. Try again; if it happens again, please report it.", job.kind.title())),
        }
    };
    finish(id, &job.kind, &job.project, result, started.elapsed(), &token);
}

/// What became of a failed job.
#[derive(Debug, PartialEq, Eq)]
enum After {
    Retry,
    /// Failed, and the Web lane waits for the user.
    StopLane,
    Fail,
}

fn after_failure(job: &Job, e: &anyhow::Error) -> After {
    let image = e.chain().find_map(|c| c.downcast_ref::<ImageError>());
    if image.is_some_and(ImageError::stops_lane) {
        return After::StopLane;
    }
    // A picture job is never run again by itself, whatever went wrong: a retry would press
    // Generate again on the user's account. It fails once with its message; the user decides.
    if job.lane == Lane::Web || job.kind.lane() == Lane::Web {
        return After::Fail;
    }
    let transient = match image {
        Some(i) => i.is_transient(),
        // Any message of the chain: a step's `.context()` must not hide a real "HTTP 503".
        None => e.chain().any(|c| transient_text(&c.to_string())),
    };
    if transient && job.tries <= AUTO_RETRIES {
        After::Retry
    } else {
        After::Fail
    }
}

fn finish(id: u64, kind: &JobKind, project: &str, result: Result<()>, took: Duration, token: &CancellationToken) {
    let mut done_ok = false;
    change(|s| {
        s.active.remove(&id);
        if !s.busy(project) {
            // The last task of a project being removed has ended: nothing more can write there.
            crate::store::set_closing(&Store::open(project).dir, false);
        }
        let mut stop_lane = None;
        // Its work was saved before the user's Pause landed: done, never run again on Resume.
        let saved_anyway = result.is_ok();
        let changed = match s.q.job_mut(id) {
            // Cancelled (removed) meanwhile: the user's choice stands.
            None => true,
            // Paused (or paused and resumed) meanwhile: an error leaves the user's choice.
            Some(job) if job.state != JobState::Running && !(saved_anyway && matches!(job.state, JobState::Paused | JobState::Queued)) => true,
            Some(job) => {
                match result {
                    Ok(()) => {
                        job.state = JobState::Done;
                        job.progress = 1.;
                        job.line = "Done".into();
                        job.error = None;
                        job.finished_at = Some(now_secs());
                        done_ok = true;
                        s.logs.push((false, format!("job {id} done in {:.1} s: {}", took.as_secs_f32(), job.label)));
                    }
                    Err(_) if token.is_cancelled() => {
                        // Stopped by the user, not failed: no try used up.
                        job.tries = job.tries.saturating_sub(1);
                        job.state = JobState::Paused;
                        job.line = "Stopped".into();
                        s.logs.push((false, format!("job {id} stopped: {}", job.label)));
                    }
                    Err(e) => {
                        let message = format!("{e:#}");
                        match after_failure(job, &e) {
                            After::Retry => {
                                job.state = JobState::Queued;
                                job.line = format!("Trying again ({} of {AUTO_RETRIES}): {message}", job.tries.min(AUTO_RETRIES));
                                s.retry_after.insert(id, Instant::now() + RETRY_DELAY);
                                s.logs.push((true, format!("job {id} failed, trying again: {message}")));
                            }
                            after => {
                                s.logs.push((true, format!("job {id} failed: {} ({message})", job.label)));
                                job.state = JobState::Failed;
                                job.error = Some(message.clone());
                                job.line = message;
                                job.finished_at = Some(now_secs());
                                if matches!(after, After::StopLane) {
                                    stop_lane = Some(job.lane);
                                }
                            }
                        }
                    }
                }
                true
            }
        };
        if let Some(lane) = stop_lane {
            if !s.q.paused_lanes.contains(&lane) {
                s.q.paused_lanes.push(lane);
            }
        }
        (changed, ())
    });
    if done_ok {
        record_eta(kind, took.as_secs_f32());
    }
}

/// Runs one job by calling its step's function. A step compiled out of this build fails with
/// "This step is not built yet".
pub async fn execute(cx: &JobCtx<'_>) -> Result<()> {
    match &cx.job.kind {
        #[cfg(feature = "story")]
        JobKind::Story { episode } => crate::story::run_job(cx, episode).await,
        #[cfg(feature = "story")]
        JobKind::AdaptScene { episode, scene } => crate::story::adapt_scene(cx, episode, scene).await,
        #[cfg(feature = "bible")]
        JobKind::Bible { episode } => crate::bible::run_job(cx, episode).await,
        #[cfg(feature = "bible")]
        JobKind::Crops { character } => crate::crops::run_job(cx, character).await,
        #[cfg(feature = "bible")]
        JobKind::Sheet { target, variants } => crate::sheets::run_job(cx, target, *variants).await,
        #[cfg(feature = "shots")]
        JobKind::PlanScene { episode, scene } => crate::director::run_job(cx, episode, scene).await,
        #[cfg(feature = "frames")]
        JobKind::Frame { episode, shot, variants } => crate::frames::run_job(cx, episode, shot, *variants).await,
        #[cfg(feature = "sound")]
        JobKind::Voice { episode, line } => crate::voices::run_job(cx, episode, line).await,
        #[cfg(feature = "sound")]
        JobKind::Audition { character, voice } => crate::voices::run_audition(cx, character, voice).await,
        #[cfg(feature = "cut")]
        JobKind::Animatic { episode } => crate::animatic::run_job(cx, episode).await,
        JobKind::Unknown => Err(anyhow!("This job was made by a newer MangaVox. Update MangaVox to run it, or remove it from the queue.")),
        #[allow(unreachable_patterns)]
        _ => Err(not_built()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(kind: JobKind, tries: u8) -> Job {
        Job { id: 1, project: "p".into(), lane: kind.lane(), kind, tries, ..Job::default() }
    }

    fn frame() -> JobKind {
        JobKind::Frame { episode: "ep01".into(), shot: "sh0001".into(), variants: 1 }
    }

    fn story() -> JobKind {
        JobKind::Story { episode: "ep01".into() }
    }

    fn copy(seq: u64) -> PendingSave {
        PendingSave { seq, path: PathBuf::from("queue.json"), q: Queue::default() }
    }

    #[test]
    fn a_failed_journal_write_is_tried_again_and_does_not_count_as_written() {
        let mut st = WriterState { latest: 3, written: 2, ..Default::default() };
        // Failed: put back to be tried again; not written.
        assert!(!after_write(&mut st, copy(3), false));
        assert_eq!(st.written, 2);
        assert_eq!(st.pending.as_ref().map(|p| p.seq), Some(3));
        // The retry lands it.
        let p = st.pending.take().expect("pending");
        assert!(after_write(&mut st, p, true));
        assert_eq!((st.written, st.pending.is_none()), (3, true));
        // A failed copy never replaces a newer one waiting.
        st.latest = 5;
        st.pending = Some(copy(5));
        assert!(!after_write(&mut st, copy(4), false));
        assert_eq!(st.pending.as_ref().map(|p| p.seq), Some(5));
        assert_eq!(st.written, 3);
    }

    #[test]
    fn a_picture_job_is_never_retried_by_itself() {
        for e in [
            anyhow!("The site is busy right now"),
            anyhow!("timed out waiting for the page"),
            anyhow::Error::new(ImageError::Site("Our servers are temporarily overloaded, try again later".into())),
            anyhow::Error::new(ImageError::Other("HTTP 503 Service Unavailable".into())),
        ] {
            assert_eq!(after_failure(&job(frame(), 1), &e), After::Fail, "{e}");
            assert_eq!(after_failure(&job(JobKind::Sheet { target: Target::StyleFrame, variants: 1 }, 1), &e), After::Fail, "{e}");
        }
        // Warnings, a closed window and a sign-in stop the lane; none is ever a hiccup.
        for e in [ImageError::Warning("Too many requests, please try again later".into()), ImageError::Closed(String::new()), ImageError::NeedSignIn] {
            assert!(!e.is_transient());
            assert_eq!(after_failure(&job(frame(), 1), &anyhow::Error::new(e)), After::StopLane);
        }
    }

    #[test]
    fn only_real_hiccups_of_other_jobs_are_retried() {
        let busy = anyhow!("The AI is overloaded");
        assert_eq!(after_failure(&job(story(), 1), &busy), After::Retry);
        assert_eq!(after_failure(&job(story(), AUTO_RETRIES + 1), &busy), After::Fail, "at most twice");
        // The AI's own 10-minute timeout would only hang again.
        assert_eq!(after_failure(&job(story(), 1), &anyhow!("The AI did not answer within 10 minutes. Try again, or pick another model.")), After::Fail);
        // A number in a sentence is not a status code.
        let cut = anyhow!("Could not make the video of cut 503 of 600.");
        assert_eq!(after_failure(&job(JobKind::Animatic { episode: "ep01".into() }, 1), &cut), After::Fail);
        // A real one wrapped in a step's context is found.
        let wrapped = anyhow!("HTTP 503 Service Unavailable").context("Could not plan the scene.");
        assert_eq!(after_failure(&job(story(), 1), &wrapped), After::Retry);
    }

    #[test]
    fn plain_titles_for_every_kind() {
        assert_eq!(JobKind::PlanScene { episode: "e".into(), scene: "s".into() }.title(), "Planning the shots");
        assert_eq!(JobKind::AdaptScene { episode: "e".into(), scene: "s".into() }.title(), "Adapting the scene");
        assert!(!JobKind::Unknown.title().contains('_'));
    }
}
