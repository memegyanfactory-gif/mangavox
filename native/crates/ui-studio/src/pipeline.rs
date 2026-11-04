//! Generate: the whole Studio pipeline (Read → Transcript → Story → Audio → Music → Video) per
//! chapter, as one app-wide queue that keeps going when the Studio page is left and survives a
//! restart (`studio/pipeline_queue.json`).
//!
//! Every chapter runs its steps in order and skips the ones already done. The AI steps, the
//! audio / music steps and the video render run in three lanes: the next chapter is read while
//! one is voiced, and a chapter lands on the Video editor's timeline as soon as its narration is
//! timed (Audio writes the sync map), while its video renders in the background without holding
//! up the narration of the next. A failed step marks that chapter failed and the queue moves on
//! to the next chapter. Chapters (and the whole queue)
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

    /// Only the video is asked for again (its narration is made): see [`Queue::claim_render`].
    pub fn renders_only(&self) -> bool {
        self.until == RENDER_STEP && self.force == Some(RENDER_STEP)
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

    /// The next waiting chapter, marked running (none while the queue is paused). Render
    /// requests are left to [`Queue::claim_render`].
    pub fn claim_next(&mut self) -> Option<u64> {
        self.claim(|i| !i.renders_only())
    }

    /// The next waiting render request (Export's "Render them first", a re-edit), marked running:
    /// it goes straight to the render lane instead of waiting behind other chapters' AI and voice.
    pub fn claim_render(&mut self) -> Option<u64> {
        self.claim(QueueItem::renders_only)
    }

    fn claim(&mut self, which: impl Fn(&QueueItem) -> bool) -> Option<u64> {
        if self.paused {
            return None;
        }
        let it = self.items.iter_mut().find(|i| i.state == ItemState::Queued && which(i))?;
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

    /// Holds the whole queue; the running chapters (one per lane) go back to waiting (returned, to stop them).
    pub fn pause_all(&mut self) -> Vec<u64> {
        self.paused = true;
        let mut ids = vec![];
        for it in self.items.iter_mut().filter(|i| i.state == ItemState::Running) {
            it.state = ItemState::Queued;
            ids.push(it.id);
        }
        ids
    }

    pub fn resume_all(&mut self) {
        self.paused = false;
        self.interrupted = false;
        for it in self.items.iter_mut().filter(|i| i.state == ItemState::Paused) {
            it.state = ItemState::Queued;
            it.note = None;
        }
    }

    /// Drops every chapter still to run; returns the running ones (to stop them).
    pub fn cancel_all(&mut self) -> Vec<u64> {
        let running = self.items.iter().filter(|i| i.state == ItemState::Running).map(|i| i.id).collect();
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

    /// The running step of `id` was stopped to start again (another AI was picked): the chapter
    /// waits again at its place in the queue, so it is the next one the AI lane takes.
    pub fn restart(&mut self, id: u64, step: Step) {
        let Some(it) = self.get_mut(id) else { return };
        it.steps[step.index()] = StepState::Pending;
        it.job = None;
        if it.state == ItemState::Running {
            it.state = ItemState::Queued;
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
/// The AI step was stopped to start again with another AI (see [`set_context`]).
static RESTART_AI: AtomicBool = AtomicBool::new(false);

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
    let model = |c: &Option<Ctx>| c.as_ref().and_then(|c| c.model.as_ref().map(|(m, _)| (m.provider, m.model.clone())));
    let switched = {
        let mut g = CTX.lock().unwrap_or_else(|e| e.into_inner());
        let new = Some(ctx);
        let switched = g.is_some() && model(&g) != model(&new);
        *g = new;
        switched
    };
    // Another AI was picked while a Generate step waits for a usage limit: that step starts
    // again with the new AI (a read carries on from the pages already read).
    let waiting = run::snapshot().filter(|r| r.running() && r.waiting.is_some()).and_then(|r| r.chapter_id);
    if switched && waiting.is_some_and(|ch| read(|q| q.items.iter().any(|i| i.state == ItemState::Running && i.chapter.id == ch))) {
        RESTART_AI.store(true, Ordering::SeqCst);
        run::cancel();
    }
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
    render_wake().notify_one();
}

/// Wakes the render lane for a new render request ([`Queue::claim_render`]).
fn render_wake() -> &'static tokio::sync::Notify {
    static N: OnceLock<tokio::sync::Notify> = OnceLock::new();
    N.get_or_init(tokio::sync::Notify::new)
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
    mv_recap::retry_now();
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
    let running: Vec<QueueItem> = read(|q| q.items.iter().filter(|i| i.state == ItemState::Running).cloned().collect());
    edit(|q| q.pause_all());
    for it in &running {
        stop_step(it);
    }
}

/// Resumes the queue; an AI step waiting for a usage limit tries again now.
pub fn resume_all() {
    edit(Queue::resume_all);
    mv_recap::retry_now();
    ensure_runner();
}

pub fn cancel_all() {
    let running: Vec<QueueItem> = read(|q| q.items.iter().filter(|i| i.state == ItemState::Running).cloned().collect());
    for it in &running {
        stop_step(it);
    }
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

/// Read, Transcript and Story (the AI steps) run in one lane; Audio and Music in a second, and the
/// Video render in a third, so the AI reads and writes the next chapter while this one is voiced,
/// and voicing never waits for a render. A request for the video alone goes straight to the render
/// lane ([`Queue::claim_render`]).
const AI_STEPS: usize = 3;
/// Where the render lane starts (the Video step).
const RENDER_STEP: usize = 5;
/// How often a media step waiting for an export looks again.
const EXPORT_WAIT: Duration = Duration::from_secs(2);

async fn runner() {
    // The AI lane reads every waiting chapter ahead; the media lane voices them in order and the
    // render lane renders them in order. While the AI waits for a usage limit, the other lanes
    // carry on with what is read.
    let (to_media, mut media) = tokio::sync::mpsc::unbounded_channel::<(u64, bool)>();
    let (to_render, mut render) = tokio::sync::mpsc::unbounded_channel::<(u64, bool)>();
    let ai = async move {
        loop {
            let Some(id) = edit(Queue::claim_next) else {
                wake().notified().await;
                continue;
            };
            let Some(dirty) = run_steps(id, 0..AI_STEPS, false).await else { continue };
            hand_on(id, dirty, AI_STEPS, &to_media);
        }
    };
    let media = async move {
        while let Some((id, dirty)) = media.recv().await {
            if let Some(dirty) = run_steps(id, AI_STEPS..RENDER_STEP, dirty).await {
                hand_on(id, dirty, RENDER_STEP, &to_render);
                render_wake().notify_one();
            }
        }
    };
    let render = async move {
        loop {
            // A render request comes first (someone waits on it) and skips the other lanes; the
            // voice steps still run here if its narration turned out not to be made.
            let (id, dirty, from) = if let Some(id) = edit(Queue::claim_render) {
                (id, false, AI_STEPS)
            } else if let Ok((id, dirty)) = render.try_recv() {
                (id, dirty, RENDER_STEP)
            } else {
                render_wake().notified().await;
                continue;
            };
            if run_steps(id, from..STEPS, dirty).await.is_some() {
                finish(id);
                // The export's watermark goes on now, beside the next render, so Export later only
                // joins the chapters.
                if let Some(ch) = read(|q| q.get(id).map(|i| i.chapter.clone())) {
                    tokio::task::spawn_blocking(move || crate::export::prepare_segment(&ch));
                }
            }
        }
    };
    tokio::join!(ai, media, render);
}

/// The steps of `id` before `next` are made: it goes on to the lane that starts at step `next`
/// when it makes that step, else it is finished.
fn hand_on(id: u64, dirty: bool, next: usize, lane: &tokio::sync::mpsc::UnboundedSender<(u64, bool)>) {
    if read(|q| q.get(id).is_some_and(|i| i.until >= next)) {
        touch(|q| {
            if let Some(it) = q.get_mut(id) {
                it.current = None;
            }
        });
        let _ = lane.send((id, dirty));
    } else {
        finish(id);
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

/// Runs the steps of `id` in `steps` (up to the last one it makes). `dirty`: an earlier step made
/// something new. Returns the new `dirty` when they all finished, `None` when the chapter stopped,
/// failed or is no longer meant to run.
async fn run_steps(id: u64, steps: std::ops::Range<usize>, mut dirty: bool) -> Option<bool> {
    let item = read(|q| q.get(id).cloned())?;
    for &step in Step::ALL[steps].iter().filter(|s| s.index() <= item.until) {
        if !still_running(id) {
            return None;
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
        // A running export has the PC first: voicing and rendering start once it is done.
        while step.index() >= AI_STEPS && crate::export::exporting() {
            if !still_running(id) {
                return None;
            }
            tokio::time::sleep(EXPORT_WAIT).await;
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
            Outcome::Stopped if step.index() < AI_STEPS && RESTART_AI.swap(false, Ordering::SeqCst) => {
                edit(|q| q.restart(id, step));
                return None;
            }
            Outcome::Stopped => {
                edit(|q| q.stopped(id, step));
                return None;
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
                return None;
            }
        }
    }
    Some(dirty)
}

/// Every step of `id` is made.
fn finish(id: u64) {
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
            // only missing pages are voiced. The sync map is timed right after, so the chapter is
            // on the Video editor's timeline before its video is rendered.
            let req = request(&c, voice, &item.format, &["tts", "sync_map"], dirty || redo || facts.voice_changed);
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

// ---------------------------------------------------------------- activity (read-only)

/// Studio's AI run (Read, Story, a page rewrite: `run`, private to this crate) as plain data,
/// for activity views outside the Studio page (the phone companion's Activity).
#[derive(Clone, Debug, PartialEq)]
pub struct AiActivity {
    /// "read" or "story".
    pub kind: &'static str,
    pub chapter_id: Option<String>,
    /// 1-based position of the chapter in the batch, and the batch size.
    pub chapter_pos: (usize, usize),
    pub pages_done: usize,
    pub pages_total: usize,
    /// 0..=1 across the batch.
    pub progress: f32,
    /// "Chapter 1 · reading pages 5–7 · 4/38 done".
    pub status: String,
    /// Waiting for an AI usage limit: why, and when it carries on (unix seconds).
    pub waiting: Option<(String, i64)>,
    /// "running", "done", "failed" or "cancelled".
    pub state: &'static str,
    pub error: Option<String>,
}

/// The current (or last) AI run, if any ran this session.
pub fn ai_activity() -> Option<AiActivity> {
    run::snapshot().map(|r| {
        let (state, error) = match &r.state {
            run::RunState::Running => ("running", None),
            run::RunState::Done => ("done", None),
            run::RunState::Failed(e) => ("failed", Some(e.clone())),
            run::RunState::Cancelled => ("cancelled", None),
        };
        AiActivity {
            kind: if r.kind == run::RunKind::Story { "story" } else { "read" },
            chapter_id: r.chapter_id.clone(),
            chapter_pos: r.chapter_pos,
            pages_done: r.done,
            pages_total: r.total,
            progress: r.progress(),
            status: r.status_line(),
            waiting: r.waiting.clone(),
            state,
            error,
        }
    })
}

/// A Studio export or join in flight (the Studio page's or the assistant's): its manga, 0..=1
/// and what it is doing.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportActivity {
    /// Which export (for [`cancel_export`]).
    pub id: u64,
    pub manga_id: String,
    pub manga_title: String,
    pub progress: f32,
    pub label: String,
    /// Waiting for the export before it to finish (not started yet).
    pub waiting: bool,
}

/// One tracked export: its id, manga (id and title), progress and cancel flag. Weak, so an
/// entry goes when its job and workers are gone.
struct Tracked {
    id: u64,
    manga_id: String,
    manga_title: String,
    state: std::sync::Weak<Mutex<(f32, String)>>,
    cancel: std::sync::Weak<AtomicBool>,
}

/// Exports in flight.
static EXPORTS: Mutex<Vec<Tracked>> = Mutex::new(Vec::new());
static EXPORT_IDS: AtomicU64 = AtomicU64::new(1);
/// Exports asked for while another runs, in the order they start: (id, manga id, manga title).
static WAITING: Mutex<Vec<(u64, String, String)>> = Mutex::new(Vec::new());

/// An export of `manga_id` waits its turn (shown as activity until it starts or is cancelled).
pub(crate) fn wait_export(manga_id: &str, manga_title: &str) -> u64 {
    let id = EXPORT_IDS.fetch_add(1, Ordering::Relaxed);
    WAITING.lock().unwrap_or_else(|e| e.into_inner()).push((id, manga_id.to_string(), manga_title.to_string()));
    id
}

/// The waiting export `id` is up: true to start it, false when it was cancelled meanwhile.
pub(crate) fn start_waiting(id: u64) -> bool {
    let mut g = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    let had = g.iter().any(|w| w.0 == id);
    g.retain(|w| w.0 != id);
    had
}

/// Called by `export::Job::new`.
pub(crate) fn track_export(manga_id: &str, manga_title: &str, state: &Arc<Mutex<(f32, String)>>, cancel: &Arc<AtomicBool>) {
    let mut g = EXPORTS.lock().unwrap_or_else(|e| e.into_inner());
    g.retain(|t| t.state.strong_count() > 0);
    let id = EXPORT_IDS.fetch_add(1, Ordering::Relaxed);
    g.push(Tracked { id, manga_id: manga_id.to_string(), manga_title: manga_title.to_string(), state: Arc::downgrade(state), cancel: Arc::downgrade(cancel) });
}

/// Exports and joins running now, then the ones waiting their turn.
pub fn exports() -> Vec<ExportActivity> {
    let mut g = EXPORTS.lock().unwrap_or_else(|e| e.into_inner());
    g.retain(|t| t.state.strong_count() > 0);
    let mut out: Vec<ExportActivity> = g
        .iter()
        .filter_map(|t| {
            let (progress, label) = t.state.upgrade()?.lock().ok()?.clone();
            Some(ExportActivity { id: t.id, manga_id: t.manga_id.clone(), manga_title: t.manga_title.clone(), progress: progress.clamp(0., 1.), label, waiting: false })
        })
        .collect();
    drop(g);
    out.extend(WAITING.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(id, m, t)| ExportActivity {
        id: *id,
        manga_id: m.clone(),
        manga_title: t.clone(),
        progress: 0.,
        label: "Queued: starts when the export before it is done".into(),
        waiting: true,
    }));
    out
}

/// Cancels the export `id` (from [`exports`]); it stops at its next step and says so.
pub fn cancel_export(id: u64) {
    // A waiting one simply never starts.
    WAITING.lock().unwrap_or_else(|e| e.into_inner()).retain(|w| w.0 != id);
    let g = EXPORTS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = g.iter().find(|t| t.id == id).and_then(|t| t.cancel.upgrade()) {
        c.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod waiting_tests {
    use super::*;

    #[test]
    fn a_waiting_export_shows_until_it_starts_or_is_cancelled() {
        let a = wait_export("wait-a", "A");
        let b = wait_export("wait-b", "B");
        let shown = |id: u64| exports().iter().any(|e| e.id == id && e.waiting);
        assert!(shown(a) && shown(b));
        cancel_export(b);
        assert!(!shown(b) && !start_waiting(b), "a cancelled one never starts");
        assert!(start_waiting(a) && !shown(a), "starting takes it off the waiting list");
        assert!(!start_waiting(a));
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
    fn render_requests_skip_the_chapters_waiting_for_the_ai() {
        let mut q = Queue::default();
        q.add(ch("a"), Step::Video, None, "landscape", None);
        q.add(ch("b"), Step::Video, Some(Step::Video), "landscape", None);
        assert_eq!(q.claim_render(), Some(q.items[1].id), "the render request goes straight to the render lane");
        assert_eq!(q.claim_next(), Some(q.items[0].id));
        assert_eq!(q.claim_next(), None, "the AI lane never takes a render request");
        q.add(ch("c"), Step::Video, Some(Step::Video), "landscape", None);
        q.add(ch("c"), Step::Video, Some(Step::Audio), "landscape", None);
        assert_eq!(q.claim_render(), None, "a redo of the voice is no longer render-only");
        q.paused = true;
        q.items[1].state = ItemState::Queued;
        assert_eq!(q.claim_render(), None, "nothing starts while paused");
    }

    #[test]
    fn whole_queue_pause_and_cancel() {
        let mut q = Queue::default();
        q.add(ch("a"), Step::Video, None, "landscape", None);
        q.add(ch("b"), Step::Video, None, "landscape", None);
        // The AI lane runs b while the media lane still works on a: both stop.
        let a = q.claim_next().unwrap();
        let b = q.claim_next().unwrap();
        assert_eq!(q.pause_all(), vec![a, b]);
        assert!(q.items.iter().all(|i| i.state == ItemState::Queued), "the running chapters wait again");
        assert_eq!(q.claim_next(), None, "nothing starts while paused");
        q.resume_all();
        assert_eq!(q.claim_next(), Some(a), "order is kept");
        q.items[1].state = ItemState::Done;
        assert_eq!(q.cancel_all(), vec![a]);
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
    fn switching_ai_during_a_limit_restarts_the_chapter_in_place() {
        let mut q = Queue::default();
        q.add(ch("a"), Step::Video, None, "landscape", None);
        q.add(ch("b"), Step::Video, None, "landscape", None);
        let a = q.claim_next().unwrap();
        q.set_step(a, Step::Read, StepState::Running);
        q.restart(a, Step::Read);
        assert_eq!((q.items[0].state, q.items[0].steps[0], q.items[0].note.clone()), (ItemState::Queued, StepState::Pending, None));
        assert!(!q.paused);
        assert_eq!(q.claim_next(), Some(a), "it goes again before the chapters after it");
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
