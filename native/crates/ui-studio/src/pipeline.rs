//! Generate: the whole Studio pipeline (Read → Transcript → Story → Audio → Music → Video) per
//! chapter, as one app-wide queue that keeps going when the Studio page is left and survives a
//! restart (`studio/pipeline_queue.json`).
//!
//! Every chapter runs its steps in order and skips the ones already done. A failed step marks
//! that chapter failed and the queue moves on to the next chapter. Chapters (and the whole queue)
//! can be paused, resumed, retried and cancelled; adding a chapter that is already waiting does
//! nothing (or widens what it will make). The page polls [`version`] and re-reads a
//! [`snapshot`], like `run` and `service`.
//!
//! The AI steps go through `run` (one AI run at a time, shown in Activity) and the audio / video
//! steps through `service` (the Python job queue), so manual runs and Generate never fight.
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use mv_ai::Registry;
use mv_recap::{ChapterRef, ModelChoice, Step};
use serde::{Deserialize, Serialize};

use crate::data::ChapterItem;
use crate::run;
use crate::service::{self, JobView, NarrationRequest};
use crate::setup::{self, Env};

const STEPS: usize = 6;
const WAIT: Duration = Duration::from_millis(400);

// ---------------------------------------------------------------- model

/// Where one step of one chapter stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    #[default]
    Pending,
    /// Not needed: already made before this run, or turned off (music).
    Skipped,
    Running,
    Done,
    Failed,
}

/// Where one chapter of the queue stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemState {
    Queued,
    Running,
    Paused,
    Done,
    Failed,
}

impl ItemState {
    /// Still to run (waiting, running or paused).
    pub fn active(self) -> bool {
        matches!(self, ItemState::Queued | ItemState::Running | ItemState::Paused)
    }
}

/// The chapter a queue item works on: enough to rebuild a [`ChapterItem`] after a restart.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QChapter {
    pub id: String,
    pub manga_id: String,
    pub manga_title: String,
    pub number: Option<f64>,
    pub title: Option<String>,
    pub pages_dir: PathBuf,
    pub output_dir: PathBuf,
}

impl QChapter {
    pub fn from_item(c: &ChapterItem) -> Self {
        Self {
            id: c.id.clone(),
            manga_id: c.manga_id.clone(),
            manga_title: c.manga_title.clone(),
            number: c.number,
            title: c.title.clone(),
            pages_dir: c.pages_dir.clone(),
            output_dir: c.output_dir.clone(),
        }
    }

    /// The chapter with its on-disk fields filled in (blocking).
    pub fn to_item(&self) -> ChapterItem {
        let mut c = ChapterItem {
            id: self.id.clone(),
            manga_id: self.manga_id.clone(),
            manga_title: self.manga_title.clone(),
            number: self.number,
            title: self.title.clone(),
            pages_dir: self.pages_dir.clone(),
            output_dir: self.output_dir.clone(),
            page_count: 0,
            has_text: false,
            video: None,
            audio_files: 0,
        };
        crate::data::scan(&mut c);
        c
    }

    pub fn label(&self) -> String {
        crate::data::chapter_label(self.number, self.title.as_deref())
    }

    pub fn chref(&self) -> ChapterRef {
        ChapterRef {
            chapter_id: self.id.clone(),
            manga_id: self.manga_id.clone(),
            manga_title: self.manga_title.clone(),
            label: self.label(),
            pages_dir: self.pages_dir.clone(),
            output_dir: self.output_dir.clone(),
        }
    }
}

/// "Ch. 12", "Ch. 3.5", or the title.
pub fn short_label(number: Option<f64>, title: Option<&str>) -> String {
    match number {
        Some(n) if n.fract() == 0.0 => format!("Ch. {}", n as i64),
        Some(n) => format!("Ch. {n}"),
        None => title.map(str::trim).filter(|t| !t.is_empty()).unwrap_or("Chapter").to_string(),
    }
}

/// One chapter in the Generate queue.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueueItem {
    pub id: u64,
    pub chapter: QChapter,
    /// Index of the last step to make (5 = Video, the whole pipeline).
    pub until: usize,
    /// A step to make again even when it is done ("Retry video").
    pub force: Option<usize>,
    /// `mv_recap::FORMAT_LANDSCAPE` or `FORMAT_REELS`.
    pub format: String,
    /// Burn subtitles into the video (`None` keeps the Video step's own setting).
    #[serde(default)]
    pub captions: Option<bool>,
    pub state: ItemState,
    pub steps: [StepState; STEPS],
    /// The step running now (or that failed).
    pub current: Option<usize>,
    /// What went wrong, as the step reported it (turned into a sentence by `issues`).
    pub error: Option<String>,
    /// Why it is paused, when it was not the user ("Stopped from Activity").
    #[serde(default)]
    pub note: Option<String>,
    /// The audio / video job of the running step.
    #[serde(skip)]
    pub job: Option<u64>,
}

impl QueueItem {
    pub fn until_step(&self) -> Step {
        Step::ALL[self.until.min(STEPS - 1)]
    }

    pub fn current_step(&self) -> Option<Step> {
        self.current.map(|i| Step::ALL[i.min(STEPS - 1)])
    }

    /// Steps finished or not needed, out of the steps this item makes.
    pub fn finished_steps(&self) -> usize {
        self.steps[..=self.until.min(STEPS - 1)].iter().filter(|s| matches!(s, StepState::Done | StepState::Skipped)).count()
    }

    /// 0..=1 over the steps this item makes.
    pub fn progress(&self) -> f32 {
        let n = self.until.min(STEPS - 1) + 1;
        if self.state == ItemState::Done {
            return 1.0;
        }
        self.finished_steps() as f32 / n as f32
    }
}

/// What [`Queue::add`] did with a chapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Added {
    New,
    /// Already waiting; it now also makes the later steps asked for.
    Widened,
    /// Already waiting for the same work.
    Already,
}

/// The queue itself (pure state; the statics below wrap it).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Queue {
    /// The whole queue is on hold; the running chapter went back to waiting.
    pub paused: bool,
    /// MangaVox closed while chapters were waiting: they wait for Resume.
    pub interrupted: bool,
    next_id: u64,
    pub items: Vec<QueueItem>,
}

impl Queue {
    pub fn add(&mut self, chapter: QChapter, until: Step, force: Option<Step>, format: &str, captions: Option<bool>) -> Added {
        let (until, force) = (until.index(), force.map(Step::index));
        if let Some(it) = self.items.iter_mut().find(|i| i.chapter.id == chapter.id && i.state.active()) {
            let mut widened = false;
            if until > it.until {
                it.until = until;
                widened = true;
            }
            // An earlier redo step covers a later one (it re-makes everything after it).
            if force.is_some_and(|f| it.force.is_none_or(|g| f < g)) && it.state != ItemState::Running {
                it.force = force;
                widened = true;
            }
            return if widened { Added::Widened } else { Added::Already };
        }
        // A finished or failed run of the chapter makes room for the new one.
        self.items.retain(|i| i.chapter.id != chapter.id);
        self.next_id += 1;
        self.items.push(QueueItem {
            id: self.next_id,
            chapter,
            until,
            force,
            format: format.to_string(),
            captions,
            state: ItemState::Queued,
            steps: [StepState::Pending; STEPS],
            current: None,
            error: None,
            note: None,
            job: None,
        });
        Added::New
    }

    pub fn get(&self, id: u64) -> Option<&QueueItem> {
        self.items.iter().find(|i| i.id == id)
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut QueueItem> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    /// The next waiting chapter, marked running (none while the queue is paused).
    pub fn claim_next(&mut self) -> Option<u64> {
        if self.paused {
            return None;
        }
        let it = self.items.iter_mut().find(|i| i.state == ItemState::Queued)?;
        it.state = ItemState::Running;
        it.note = None;
        Some(it.id)
    }

    /// Pauses one chapter. True when it was running (its step must be stopped).
    pub fn pause(&mut self, id: u64) -> bool {
        let Some(it) = self.get_mut(id) else { return false };
        let was = it.state == ItemState::Running;
        if matches!(it.state, ItemState::Queued | ItemState::Running) {
            it.state = ItemState::Paused;
            it.note = None;
        }
        was
    }

    /// Resumes a paused chapter, or retries a failed one from the step that failed.
    pub fn resume(&mut self, id: u64) {
        let Some(it) = self.get_mut(id) else { return };
        match it.state {
            ItemState::Paused => it.state = ItemState::Queued,
            ItemState::Failed => {
                it.state = ItemState::Queued;
                it.error = None;
                for s in it.steps.iter_mut().filter(|s| **s == StepState::Failed) {
                    *s = StepState::Pending;
                }
            }
            _ => {}
        }
        it.note = None;
    }

    /// Takes a chapter out of the queue. True when it was running.
    pub fn remove(&mut self, id: u64) -> bool {
        let was = self.get(id).is_some_and(|i| i.state == ItemState::Running);
        self.items.retain(|i| i.id != id);
        was
    }

    /// Holds the whole queue; the running chapter goes back to waiting (returned, to stop it).
    pub fn pause_all(&mut self) -> Option<u64> {
        self.paused = true;
        let it = self.items.iter_mut().find(|i| i.state == ItemState::Running)?;
        it.state = ItemState::Queued;
        Some(it.id)
    }

    pub fn resume_all(&mut self) {
        self.paused = false;
        self.interrupted = false;
        for it in self.items.iter_mut().filter(|i| i.state == ItemState::Paused) {
            it.state = ItemState::Queued;
            it.note = None;
        }
    }

    /// Drops every chapter still to run; returns the running one (to stop it).
    pub fn cancel_all(&mut self) -> Option<u64> {
        let running = self.items.iter().find(|i| i.state == ItemState::Running).map(|i| i.id);
        self.items.retain(|i| !i.state.active());
        self.paused = false;
        self.interrupted = false;
        running
    }

    pub fn clear_finished(&mut self) {
        self.items.retain(|i| i.state != ItemState::Done);
    }

    /// The item still to run (or the last one) for a chapter.
    pub fn for_chapter(&self, chapter_id: &str) -> Option<&QueueItem> {
        self.items.iter().rev().find(|i| i.chapter.id == chapter_id)
    }

    /// Chapters still to run.
    pub fn active(&self) -> usize {
        self.items.iter().filter(|i| i.state.active()).count()
    }

    /// After loading from disk: nothing is running any more. Waiting chapters wait for Resume
    /// (they never start by themselves when MangaVox opens).
    pub fn after_restart(&mut self) {
        for it in self.items.iter_mut() {
            if it.state == ItemState::Running {
                it.state = ItemState::Queued;
            }
            for s in it.steps.iter_mut().filter(|s| **s == StepState::Running) {
                *s = StepState::Pending;
            }
            it.job = None;
        }
        if self.items.iter().any(|i| i.state == ItemState::Queued) {
            self.paused = true;
            self.interrupted = true;
        }
    }

    /// The running step of `id` stopped before it finished. It goes back to "to do"; who stopped it
    /// decides the rest: a whole-queue pause leaves the chapter waiting, a chapter pause or a
    /// removal already said what it is, and anything else (Activity's Stop, a job's Cancel)
    /// pauses the chapter with a note, so it never silently restarts.
    pub fn stopped(&mut self, id: u64, step: Step) {
        let paused = self.paused;
        let Some(it) = self.get_mut(id) else { return };
        it.steps[step.index()] = StepState::Pending;
        it.job = None;
        if it.state != ItemState::Running {
            return;
        }
        if paused {
            it.state = ItemState::Queued;
        } else {
            it.state = ItemState::Paused;
            it.note = Some(format!("{} was stopped", step.label()));
        }
    }

    fn set_step(&mut self, id: u64, step: Step, st: StepState) {
        if let Some(it) = self.get_mut(id) {
            it.steps[step.index()] = st;
            if st == StepState::Running {
                it.current = Some(step.index());
            }
        }
    }
}

// ---------------------------------------------------------------- what a step needs

/// What the files say about one chapter, for deciding which steps to skip.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facts {
    pub status: mv_recap::PhaseStatus,
    /// The last narration finished (`audio/narration_params.json` is written at its end).
    pub narration_complete: bool,
    /// The clips were narrated with another voice than the one the chapter has now (the user
    /// picked a new narrator), so Audio and the steps after it are made again.
    pub voice_changed: bool,
    pub music_on: bool,
    pub music_fresh: bool,
}

/// The voice the chapter's clips were narrated with (`audio/narration_params.json`, written by
/// tts_bridge at the end of a narration), if any.
pub fn narrated_voice(output_dir: &std::path::Path) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(output_dir.join("audio").join("narration_params.json")).ok()?).ok()?;
    v.get("voice").and_then(serde_json::Value::as_str).map(str::to_string).filter(|s| !s.is_empty())
}

/// Clips exist and were narrated with a voice other than `wanted`.
pub fn voice_changed(audio_files: usize, narrated: Option<&str>, wanted: &str) -> bool {
    audio_files > 0 && !wanted.is_empty() && narrated.is_some_and(|n| n != wanted)
}

impl Facts {
    /// Reads the facts of a chapter (blocking).
    pub fn load(ch: &ChapterRef) -> Facts {
        let status = mv_recap::chapter_status(ch);
        let music = mv_score::load_settings(ch);
        let wanted = mv_studio::voices::voice_for_chapter(&ch.chapter_id);
        Facts {
            narration_complete: ch.output_dir.join("audio").join("narration_params.json").is_file(),
            voice_changed: voice_changed(status.audio_files, narrated_voice(&ch.output_dir).as_deref(), &wanted),
            music_on: music.enabled,
            music_fresh: music.enabled && mv_score::is_fresh(ch, &music),
            status,
        }
    }
}

/// Whether `step` has to run. `forced`: an earlier step just ran (its output is new), or the
/// user asked to make this step again. `Err` holds why it is skipped.
pub fn plan(step: Step, f: &Facts, format: &str, forced: bool) -> Result<(), &'static str> {
    let st = &f.status;
    let done = match step {
        Step::Read => st.total_pages > 0 && st.read_pages >= st.total_pages,
        // The read writes the transcript; it only runs on its own when it is missing.
        Step::Transcript => st.transcript || st.read_pages == 0,
        Step::Story => st.story,
        Step::Audio => st.audio_files > 0 && f.narration_complete && !f.voice_changed,
        Step::Music if !f.music_on => return Err("Music is off"),
        Step::Music => f.music_fresh,
        Step::Video => {
            let reels = format == mv_recap::FORMAT_REELS;
            st.video.is_some() && reels == !st.reels.is_empty()
        }
    };
    // The transcript follows the read, so it is never forced on its own.
    let forced = forced && step != Step::Transcript;
    if done && !forced {
        Err("Already done")
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------- app-wide queue

/// What the runner needs from the page: the AI registry and model, and the checked setup.
#[derive(Clone)]
pub struct Ctx {
    pub reg: Arc<Registry>,
    pub model: Option<(ModelChoice, String)>,
    pub env: Option<Env>,
    pub speed: f64,
    pub resolution: String,
    pub styles: Vec<String>,
}

static QUEUE: Mutex<Option<Queue>> = Mutex::new(None);
static CTX: Mutex<Option<Ctx>> = Mutex::new(None);
static VERSION: AtomicU64 = AtomicU64::new(0);
static RUNNER: AtomicBool = AtomicBool::new(false);

fn wake() -> &'static tokio::sync::Notify {
    static N: OnceLock<tokio::sync::Notify> = OnceLock::new();
    N.get_or_init(tokio::sync::Notify::new)
}

fn queue_path() -> PathBuf {
    mv_recap::studio_root().join("pipeline_queue.json")
}

fn load() -> Queue {
    let mut q: Queue = std::fs::read(queue_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    q.after_restart();
    q
}

fn save(q: &Queue) {
    let path = queue_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(q) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// Reads the queue.
fn read<T>(f: impl FnOnce(&Queue) -> T) -> T {
    let mut g = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(load))
}

/// Changes the queue, saves it and tells the page.
fn edit<T>(f: impl FnOnce(&mut Queue) -> T) -> T {
    let out = {
        let mut g = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        let q = g.get_or_insert_with(load);
        let out = f(q);
        save(q);
        out
    };
    VERSION.fetch_add(1, Ordering::SeqCst);
    out
}

/// Changes only what is not saved (the running job id).
fn touch(f: impl FnOnce(&mut Queue)) {
    {
        let mut g = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        f(g.get_or_insert_with(load));
    }
    VERSION.fetch_add(1, Ordering::SeqCst);
}

pub fn version() -> u64 {
    VERSION.load(Ordering::SeqCst)
}

pub fn snapshot() -> Queue {
    read(Queue::clone)
}

/// Hands the runner the registry, model and setup it uses for the next steps.
pub fn set_context(ctx: Ctx) {
    *CTX.lock().unwrap_or_else(|e| e.into_inner()) = Some(ctx);
}

/// The runner was handed a context (the Studio page was opened, or [`context_from_app`] ran).
pub fn has_context() -> bool {
    CTX.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// The context the Studio page hands over (see `StudioPage::sync_pipeline`), built without the
/// page for callers that queue work before Studio was opened (the phone companion): the
/// Assistant's registry and model, `env` (the last setup check) and the saved Studio prefs.
pub fn context_from_app(env: Option<Env>, cx: &mut gpui_kit::App) -> Ctx {
    let (provider, model) = ui_assistant::assistant_model(cx);
    let model = serde_json::from_value::<mv_ai::AiProviderId>(serde_json::Value::String(provider)).ok().map(|provider| (ModelChoice { provider, model }, ui_assistant::model_label(cx)));
    let prefs = mv_recap::Prefs::load();
    Ctx { reg: ui_assistant::registry(cx), model, env, speed: prefs.speed, resolution: prefs.resolution, styles: prefs.styles }
}

fn ctx() -> Option<Ctx> {
    CTX.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn ensure_runner() {
    if !RUNNER.swap(true, Ordering::SeqCst) {
        mv_ui::bg::runtime().spawn(runner());
    }
    wake().notify_one();
}

/// Adds chapters to the queue (each once; see [`Queue::add`]). Returns (new, widened, already).
pub fn enqueue(chapters: &[ChapterItem], until: Step, force: Option<Step>, format: &str, captions: Option<bool>, start: bool) -> (usize, usize, usize) {
    let counts = edit(|q| {
        let mut n = (0, 0, 0);
        for c in chapters {
            match q.add(QChapter::from_item(c), until, force, format, captions) {
                Added::New => n.0 += 1,
                Added::Widened => n.1 += 1,
                Added::Already => n.2 += 1,
            }
        }
        // Adding work to a queue that was held after a restart starts it.
        // Asking for work (Generate, a fix) starts a held queue; a pick that only joins a running
        // Generate does not undo the user's Pause.
        if n.0 + n.1 > 0 && (start || q.interrupted) {
            q.paused = false;
            q.interrupted = false;
        }
        n
    });
    ensure_runner();
    counts
}

/// Stops whatever the running step of `item` is doing (its AI run or Python job).
fn stop_step(item: &QueueItem) {
    let chapter = item.chapter.id.clone();
    if run::snapshot().is_some_and(|r| r.running() && r.chapter_id.as_deref() == Some(chapter.as_str())) {
        run::cancel();
    }
    if let Some(job) = item.job {
        service::cancel(job);
    }
}

fn stop_if_running(id: Option<u64>) {
    if let Some(it) = id.and_then(|id| read(|q| q.get(id).cloned())) {
        stop_step(&it);
    }
}

pub fn pause(id: u64) {
    let item = read(|q| q.get(id).cloned());
    if edit(|q| q.pause(id)) {
        if let Some(it) = item {
            stop_step(&it);
        }
    }
}

/// Resumes or retries one chapter. Asked for by the user, so it runs now even when the whole
/// queue was held.
pub fn resume(id: u64) {
    edit(|q| {
        q.resume(id);
        q.paused = false;
        q.interrupted = false;
    });
    ensure_runner();
}

/// Takes a chapter out of the queue, stopping it when it runs.
pub fn cancel(id: u64) {
    let item = read(|q| q.get(id).cloned());
    if edit(|q| q.remove(id)) {
        if let Some(it) = item {
            stop_step(&it);
        }
    }
}

pub fn pause_all() {
    let running = read(|q| q.items.iter().find(|i| i.state == ItemState::Running).cloned());
    edit(|q| q.pause_all());
    if let Some(it) = running {
        stop_step(&it);
    }
}

pub fn resume_all() {
    edit(Queue::resume_all);
    ensure_runner();
}

pub fn cancel_all() {
    let running = read(|q| q.items.iter().find(|i| i.state == ItemState::Running).map(|i| i.id));
    stop_if_running(running);
    edit(|q| q.cancel_all());
}

pub fn clear_finished() {
    edit(Queue::clear_finished);
}

/// Forgets a failed chapter (its issue goes away).
pub fn dismiss(id: u64) {
    edit(|q| {
        q.items.retain(|i| !(i.id == id && !i.state.active()));
    });
}

// ---------------------------------------------------------------- runner

async fn runner() {
    loop {
        match edit(Queue::claim_next) {
            Some(id) => run_item(id).await,
            None => wake().notified().await,
        }
    }
}

/// The item is still meant to run (not paused, removed, or held with the queue).
fn still_running(id: u64) -> bool {
    read(|q| !q.paused && q.get(id).is_some_and(|i| i.state == ItemState::Running))
}

enum Outcome {
    Done,
    Stopped,
    Failed(String),
}

impl From<run::Outcome> for Outcome {
    fn from(o: run::Outcome) -> Self {
        match o {
            run::Outcome::Done => Outcome::Done,
            run::Outcome::Cancelled => Outcome::Stopped,
            run::Outcome::Failed(e) => Outcome::Failed(e),
        }
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::task::spawn_blocking(f).await.ok()
}

async fn run_item(id: u64) {
    let Some(item) = read(|q| q.get(id).cloned()) else { return };
    let mut dirty = false;
    for &step in &Step::ALL[..=item.until.min(STEPS - 1)] {
        if !still_running(id) {
            return;
        }
        // Done in an earlier attempt of this item (a resume after a pause or a failure).
        if read(|q| q.get(id).map(|i| i.steps[step.index()])) == Some(StepState::Done) {
            continue;
        }
        let r = item.chapter.chref();
        let facts = blocking(move || Facts::load(&r)).await.unwrap_or_default();
        let forced = dirty || item.force == Some(step.index());
        if plan(step, &facts, &item.format, forced).is_err() {
            edit(|q| q.set_step(id, step, StepState::Skipped));
            continue;
        }
        edit(|q| q.set_step(id, step, StepState::Running));
        let out = run_step(id, &item, step, &facts, dirty, item.force == Some(step.index())).await;
        touch(|q| {
            if let Some(it) = q.get_mut(id) {
                it.job = None;
            }
        });
        match out {
            Outcome::Done => {
                // What a step made is new, so the steps built from it are made again. The
                // transcript only fills a gap (the story is written from the read).
                dirty |= step != Step::Transcript;
                edit(|q| q.set_step(id, step, StepState::Done));
            }
            Outcome::Stopped => {
                edit(|q| q.stopped(id, step));
                return;
            }
            Outcome::Failed(e) => {
                edit(|q| {
                    q.set_step(id, step, StepState::Failed);
                    if let Some(it) = q.get_mut(id) {
                        it.state = ItemState::Failed;
                        it.error = Some(e);
                    }
                });
                // Later chapters still run; the library shows what this one made.
                return;
            }
        }
    }
    edit(|q| {
        if let Some(it) = q.get_mut(id).filter(|i| i.state == ItemState::Running) {
            it.state = ItemState::Done;
            it.current = None;
            it.force = None;
        }
    });
}

/// Error texts that `issues` recognises (keep them in step).
pub const NO_MODEL: &str = "No AI model is picked";
pub const NO_SETUP: &str = "Python and the pipeline scripts are missing";
pub const NO_FFMPEG: &str = "FFmpeg is missing";
pub const NO_READ: &str = "Nothing has been read yet";
pub const NO_STORY: &str = "There is no story to narrate yet";
pub const NO_AUDIO: &str = "No audio files found: there is no narration to put in the video";
pub const VOICE_NOT_READY: &str = "The narrator voice is not ready";

/// `redo`: the user asked to make this step again (Redo from it), so it starts over instead of
/// only filling gaps.
async fn run_step(id: u64, item: &QueueItem, step: Step, facts: &Facts, dirty: bool, redo: bool) -> Outcome {
    let ch = item.chapter.chref();
    let ctx = ctx();
    let stop = move || !still_running(id);
    match step {
        Step::Read => {
            let Some((ctx, (model, label))) = ctx.and_then(|c| c.model.clone().map(|m| (c, m))) else { return Outcome::Failed(NO_MODEL.into()) };
            run::read_one(ctx.reg, model, label, ch, redo, stop).await.into()
        }
        Step::Transcript => {
            let r = ch.clone();
            match blocking(move || mv_recap::load_read(&r).map(|read| mv_recap::update_transcript(&r, &read))).await.flatten() {
                Some(Ok(_)) => Outcome::Done,
                Some(Err(e)) => Outcome::Failed(format!("Could not save the transcript: {e:#}")),
                None => Outcome::Failed(NO_READ.into()),
            }
        }
        Step::Story => {
            if facts.status.read_pages == 0 {
                return Outcome::Failed(NO_READ.into());
            }
            let Some((ctx, (model, label))) = ctx.and_then(|c| c.model.clone().map(|m| (c, m))) else { return Outcome::Failed(NO_MODEL.into()) };
            run::story_one(ctx.reg, model, label, ch, ctx.styles, stop).await.into()
        }
        Step::Audio => {
            if !facts.status.story {
                return Outcome::Failed(NO_STORY.into());
            }
            let Some(env) = ctx.as_ref().and_then(|c| c.env.clone()) else { return Outcome::Failed(NO_SETUP.into()) };
            let voice = mv_studio::voices::voice_for_chapter(&item.chapter.id);
            let ready = mv_studio::tts::find_voice(&voice).is_some_and(|v| setup::voice_ready(&env.engines, v).usable());
            if !ready {
                let name = mv_studio::tts::find_voice(&voice).map(|v| v.label.to_string()).unwrap_or(voice);
                return Outcome::Failed(format!("{VOICE_NOT_READY}: {name}"));
            }
            let c = ctx.expect("ctx");
            // A story written in this run, a new narrator or a redo replaces every clip; otherwise
            // only missing pages are voiced.
            let req = request(&c, voice, &item.format, &["tts"], dirty || redo || facts.voice_changed);
            job_step(id, item, &env, req, None, stop).await
        }
        Step::Music => {
            let reg_model = ctx.as_ref().map(|c| (c.reg.clone(), c.model.clone().map(|m| m.0)));
            music_step(ch, reg_model, redo).await
        }
        Step::Video => {
            let Some(env) = ctx.as_ref().and_then(|c| c.env.clone()) else { return Outcome::Failed(NO_SETUP.into()) };
            if !env.ffmpeg {
                return Outcome::Failed(NO_FFMPEG.into());
            }
            if facts.status.audio_files == 0 {
                return Outcome::Failed(NO_AUDIO.into());
            }
            let c = ctx.expect("ctx");
            let voice = mv_studio::voices::voice_for_chapter(&item.chapter.id);
            let req = request(&c, voice, &item.format, &["sync_map", "video"], false);
            // Subtitles: `service::build_job` puts the chapter's caption setting (which the Generate
            // questions saved, see `generate::apply_subtitles`) into `video_config["subtitles"]`.
            job_step(id, item, &env, req, None, stop).await
        }
    }
}

fn request(c: &Ctx, voice: String, format: &str, stages: &[&str], force_regen: bool) -> NarrationRequest {
    NarrationRequest {
        voice,
        speed: c.speed,
        video: stages.contains(&"video"),
        resolution: c.resolution.clone(),
        format: format.to_string(),
        stages: Some(stages.iter().map(|s| s.to_string()).collect()),
        force_regen,
        regen_pages: vec![],
        new_take: false,
    }
}

/// Queues one Python job for the chapter and waits for it.
async fn job_step(id: u64, item: &QueueItem, env: &Env, req: NarrationRequest, extra: Option<serde_json::Value>, stop: impl Fn() -> bool) -> Outcome {
    let chapter = item.chapter.to_item();
    // Another job of this chapter (started by hand) finishes first.
    let job = loop {
        if stop() {
            return Outcome::Stopped;
        }
        match service::enqueue_with(env, &req, std::slice::from_ref(&chapter), extra.as_ref()) {
            Ok(ids) if !ids.is_empty() => break ids[0],
            Ok(_) => tokio::time::sleep(WAIT).await,
            Err(e) => return Outcome::Failed(format!("{e:#}")),
        }
    };
    touch(|q| {
        if let Some(it) = q.get_mut(id) {
            it.job = Some(job);
        }
    });
    loop {
        tokio::time::sleep(WAIT).await;
        let Some(j) = service::job(job) else { return Outcome::Failed("The job went missing".into()) };
        match j.state {
            JobView::Done => return Outcome::Done,
            JobView::Failed(e) => return Outcome::Failed(e),
            JobView::Cancelled => return Outcome::Stopped,
            JobView::Queued | JobView::Running => {
                if stop() {
                    service::cancel(job);
                }
            }
        }
    }
}

/// Composes the chapter's score (AI spotting first when it is on and there is no cue sheet).
/// `new_take`: a redo picks different music (the next take) instead of rebuilding the same score.
async fn music_step(ch: ChapterRef, reg_model: Option<(Arc<Registry>, Option<ModelChoice>)>, new_take: bool) -> Outcome {
    let mut settings = mv_score::load_settings(&ch);
    if new_take {
        settings.take = settings.take.wrapping_add(1);
        let _ = mv_score::save_settings(&ch, &settings);
    }
    if settings.use_ai && settings.directions.is_empty() {
        if let Some((reg, Some(model))) = reg_model {
            // A failed spotting falls back to the rules, like the Music step.
            if let Ok(d) = mv_score::ai::spot(reg, model, ch.clone(), Default::default()).await {
                settings.directions = d;
                let _ = mv_score::save_settings(&ch, &settings);
            }
        }
    }
    match tokio::task::spawn_blocking(move || mv_score::build(&ch, &settings, true)).await {
        Ok(Ok(_)) => Outcome::Done,
        Ok(Err(e)) => Outcome::Failed(format!("{e:#}")),
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{plan, Added, Facts, ItemState, QChapter, Queue, StepState};
    use mv_recap::{PhaseStatus, Step};

    fn ch(id: &str) -> QChapter {
        QChapter { id: id.into(), manga_id: "m".into(), manga_title: "M".into(), number: Some(1.0), title: None, pages_dir: "/p".into(), output_dir: "/o".into() }
    }

    #[test]
    fn chapters_are_added_once() {
        let mut q = Queue::default();
        assert_eq!(q.add(ch("a"), Step::Audio, None, "landscape", None), Added::New);
        assert_eq!(q.add(ch("a"), Step::Audio, None, "landscape", None), Added::Already);
        assert_eq!(q.add(ch("a"), Step::Video, None, "landscape", None), Added::Widened, "asking for more widens it");
        assert_eq!(q.items.len(), 1);
        assert_eq!(q.items[0].until_step(), Step::Video);
        // A finished chapter is queued again as new work.
        q.items[0].state = ItemState::Done;
        assert_eq!(q.add(ch("a"), Step::Video, Some(Step::Video), "reels", None), Added::New);
        assert_eq!(q.items.len(), 1);
        assert_eq!(q.items[0].force, Some(Step::Video.index()));
        assert_eq!(q.items[0].format, "reels");
    }

    #[test]
    fn an_earlier_redo_replaces_a_later_one_on_a_waiting_chapter() {
        let mut q = Queue::default();
        q.add(ch("a"), Step::Video, Some(Step::Video), "landscape", None);
        assert_eq!(q.add(ch("a"), Step::Video, Some(Step::Audio), "landscape", None), Added::Widened);
        assert_eq!(q.items[0].force, Some(Step::Audio.index()));
        // A later redo is already covered (Audio re-makes Music and Video).
        assert_eq!(q.add(ch("a"), Step::Video, Some(Step::Music), "landscape", None), Added::Already);
        assert_eq!(q.items[0].force, Some(Step::Audio.index()));
    }

    #[test]
    fn pause_resume_and_retry() {
        let mut q = Queue::default();
        q.add(ch("a"), Step::Video, None, "landscape", None);
        q.add(ch("b"), Step::Video, None, "landscape", None);
        let a = q.claim_next().unwrap();
        assert!(q.pause(a), "a was running");
        assert_eq!(q.items[0].state, ItemState::Paused);
        let b = q.claim_next().unwrap();
        assert_ne!(a, b, "a paused chapter is skipped");
        q.items[1].state = ItemState::Failed;
        q.items[1].steps[3] = StepState::Failed;
        q.items[1].error = Some("x".into());
        q.resume(b);
        assert_eq!((q.items[1].state, q.items[1].steps[3], q.items[1].error.clone()), (ItemState::Queued, StepState::Pending, None));
        q.resume(a);
        assert_eq!(q.claim_next(), Some(a), "order is kept");
    }

    #[test]
    fn whole_queue_pause_and_cancel() {
        let mut q = Queue::default();
        q.add(ch("a"), Step::Video, None, "landscape", None);
        q.add(ch("b"), Step::Video, None, "landscape", None);
        let a = q.claim_next().unwrap();
        assert_eq!(q.pause_all(), Some(a));
        assert_eq!(q.items[0].state, ItemState::Queued, "the running chapter waits again");
        assert_eq!(q.claim_next(), None, "nothing starts while paused");
        q.resume_all();
        assert_eq!(q.claim_next(), Some(a));
        q.items[1].state = ItemState::Done;
        assert_eq!(q.cancel_all(), Some(a));
        assert_eq!(q.items.len(), 1, "finished chapters stay listed");
        q.clear_finished();
        assert!(q.items.is_empty());
    }

    #[test]
    fn a_stopped_step_lands_where_the_stop_came_from() {
        // Whole-queue Pause while a step runs: the step reports "stopped" afterwards, and the
        // chapter must stay waiting with the queue held (never restart, never look paused).
        let mut q = Queue::default();
        q.add(ch("a"), Step::Video, None, "landscape", None);
        q.add(ch("b"), Step::Video, None, "landscape", None);
        let a = q.claim_next().unwrap();
        q.set_step(a, Step::Audio, StepState::Running);
        q.pause_all();
        q.stopped(a, Step::Audio);
        assert!(q.paused);
        assert_eq!((q.items[0].state, q.items[0].steps[3], q.items[0].note.clone()), (ItemState::Queued, StepState::Pending, None));
        assert_eq!(q.claim_next(), None, "nothing starts while the queue is paused");
        // A pick that joins the queue keeps it paused; asking for work (Generate) starts it.
        assert_eq!(q.add(ch("c"), Step::Video, None, "landscape", None), Added::New);
        assert!(q.paused);
        q.resume_all();
        assert_eq!(q.claim_next(), Some(a), "the paused chapter goes first again");

        // A chapter pause: the chapter says paused, without a note.
        q.set_step(a, Step::Audio, StepState::Running);
        assert!(q.pause(a));
        q.stopped(a, Step::Audio);
        assert_eq!((q.items[0].state, q.items[0].note.clone()), (ItemState::Paused, None));

        // Stopped from elsewhere (Activity's Stop): paused with a note, the queue goes on.
        let b = q.claim_next().unwrap();
        q.stopped(b, Step::Read);
        assert_eq!(q.get(b).map(|i| (i.state, i.note.clone())), Some((ItemState::Paused, Some("Read was stopped".into()))));
        assert!(!q.paused);
        // A removed chapter stays removed.
        let c = q.claim_next().unwrap();
        q.remove(c);
        q.stopped(c, Step::Read);
        assert!(q.get(c).is_none());
    }

    #[test]
    fn a_new_narrator_makes_the_audio_again() {
        use super::voice_changed;
        assert!(voice_changed(10, Some("af_heart"), "cb_narrator_f"));
        assert!(!voice_changed(10, Some("af_heart"), "af_heart"));
        assert!(!voice_changed(0, Some("af_heart"), "cb_narrator_f"), "no clips: nothing to redo");
        assert!(!voice_changed(10, None, "cb_narrator_f"), "unknown narrator: keep the clips");
        let st = PhaseStatus { read_pages: 5, total_pages: 5, transcript: true, story: true, audio_files: 5, video: Some("/v.mp4".into()), ..Default::default() };
        let f = Facts { status: st, narration_complete: true, voice_changed: true, music_on: false, music_fresh: false };
        assert!(plan(Step::Audio, &f, "landscape", false).is_ok());
        assert!(plan(Step::Video, &f, "landscape", true).is_ok(), "the video follows the new audio");
        assert!(plan(Step::Audio, &Facts { voice_changed: false, ..f }, "landscape", false).is_err());
    }

    #[test]
    fn a_restart_holds_waiting_chapters() {
        let mut q = Queue::default();
        q.add(ch("a"), Step::Video, None, "landscape", None);
        q.claim_next();
        q.items[0].steps[0] = StepState::Running;
        let json = serde_json::to_string(&q).unwrap();
        let mut back: Queue = serde_json::from_str(&json).unwrap();
        back.after_restart();
        assert_eq!((back.items[0].state, back.items[0].steps[0]), (ItemState::Queued, StepState::Pending));
        assert!(back.paused && back.interrupted);
        assert_eq!(back.claim_next(), None);
        back.resume_all();
        assert!(back.claim_next().is_some());
    }

    #[test]
    fn steps_already_made_are_skipped() {
        let status = PhaseStatus { read_pages: 10, total_pages: 10, transcript: true, story: true, audio_files: 10, ..Default::default() };
        let f = Facts { status, narration_complete: true, voice_changed: false, music_on: true, music_fresh: false };
        assert!(plan(Step::Read, &f, "landscape", false).is_err());
        assert!(plan(Step::Story, &f, "landscape", false).is_err());
        assert!(plan(Step::Audio, &f, "landscape", false).is_err());
        assert!(plan(Step::Music, &f, "landscape", false).is_ok(), "stale score");
        assert!(plan(Step::Video, &f, "landscape", false).is_ok(), "no video yet");
        assert!(plan(Step::Story, &f, "landscape", true).is_ok(), "forced after an earlier step ran");
        assert!(plan(Step::Transcript, &f, "landscape", true).is_err(), "the read wrote the transcript");
        let off = Facts { music_on: false, ..f.clone() };
        assert_eq!(plan(Step::Music, &off, "landscape", true), Err("Music is off"));
        // A half-read chapter, and narration that stopped part-way, run again.
        let part = Facts { status: PhaseStatus { read_pages: 4, total_pages: 10, audio_files: 3, ..Default::default() }, ..Default::default() };
        assert!(plan(Step::Read, &part, "landscape", false).is_ok());
        assert!(plan(Step::Audio, &part, "landscape", false).is_ok());
        // A landscape video does not count as the reels asked for.
        let vid = Facts { status: PhaseStatus { video: Some("/v.mp4".into()), ..Default::default() }, ..Default::default() };
        assert!(plan(Step::Video, &vid, "landscape", false).is_err());
        assert!(plan(Step::Video, &vid, mv_recap::FORMAT_REELS, false).is_ok());
    }
}
