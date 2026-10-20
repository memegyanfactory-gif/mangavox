//! App-wide narration service: one `mv_studio::JobQueue` that outlives the Studio
//! page, plus per-job progress and logs collected from the queue's events. The
//! page polls [`version`] and re-reads a [`snapshot`] when it changes.
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mv_studio::{JobEvent, JobPayload, JobQueue, JobState, PipelineJob, PythonInterpreter, PythonLayout};
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::data::ChapterItem;
use crate::setup::{Env, OcrProvider};

const MAX_LOG_LINES: usize = 400;

#[derive(Clone, Debug, PartialEq)]
pub enum JobView {
    Queued,
    Running,
    Done,
    Failed(String),
    Cancelled,
}

impl JobView {
    pub fn finished(&self) -> bool {
        matches!(self, JobView::Done | JobView::Failed(_) | JobView::Cancelled)
    }
}

#[derive(Clone, Debug)]
pub struct JobInfo {
    pub id: u64,
    pub chapter_id: String,
    pub manga_title: String,
    pub chapter_label: String,
    pub output_dir: PathBuf,
    pub voice: String,
    pub stages: Vec<String>,
    pub state: JobView,
    /// Current pipeline stage and its percent (0-100).
    pub stage: Option<String>,
    pub stage_percent: f64,
    /// One-line status for the job row.
    pub status: String,
    pub logs: VecDeque<String>,
    /// Who narrates, as the voice step reported it (its own engine or a stand-in voice).
    pub narrator: Option<Narrator>,
    /// Pages this job voices again (a per-page Regenerate / New take); empty for whole chapters.
    pub regen_pages: Vec<usize>,
}

/// The engine and voice that actually narrate a chapter (`tts_bridge._announce_engine`).
#[derive(Clone, Debug, PartialEq)]
pub struct Narrator {
    /// `chatterbox`, `kokoro`, `piper`, ...
    pub engine: String,
    /// The voice the user picked.
    pub voice: String,
    /// The voice that speaks (differs when a stand-in fills in).
    pub synth_voice: String,
    /// A stand-in speaks for the picked voice (its engine is missing or would not start).
    pub stand_in: bool,
}

impl Narrator {
    /// From an `info` message that names the engine (`None` for other info lines).
    pub fn from_message(raw: &serde_json::Value) -> Option<Narrator> {
        let s = |k: &str| raw.get(k).and_then(serde_json::Value::as_str).map(str::to_string);
        let engine = s("engine").filter(|e| !e.is_empty())?;
        let voice = s("voice").unwrap_or_default();
        let synth_voice = s("synth_voice").unwrap_or_else(|| voice.clone());
        let stand_in = raw.get("stand_in").and_then(serde_json::Value::as_bool).unwrap_or(false);
        Some(Narrator { engine, voice, synth_voice, stand_in })
    }

    /// "Chatterbox", "Kokoro".
    pub fn engine_label(&self) -> &'static str {
        mv_studio::tts::engine_label(&self.engine)
    }
}

impl JobInfo {
    /// Overall progress 0..=1 across the job's stages.
    pub fn progress(&self) -> f64 {
        if self.state == JobView::Done {
            return 1.0;
        }
        overall_progress(&self.stages, self.stage.as_deref(), self.stage_percent)
    }

    pub fn video_path(&self) -> Option<PathBuf> {
        let p = self.output_dir.join(crate::data::VIDEO_FILE);
        (self.state == JobView::Done && p.is_file()).then_some(p)
    }

    fn log(&mut self, line: impl Into<String>) {
        if self.logs.len() >= MAX_LOG_LINES {
            self.logs.pop_front();
        }
        self.logs.push_back(line.into());
    }
}

/// Equal weight per stage; `text_cleanup` counts as the end of `ocr`.
pub fn overall_progress(stages: &[String], stage: Option<&str>, percent: f64) -> f64 {
    if stages.is_empty() {
        return 0.0;
    }
    let (stage, percent) = match stage {
        Some("text_cleanup") => (Some("ocr"), 100.0),
        s => (s, percent),
    };
    let Some(idx) = stage.and_then(|s| stages.iter().position(|x| x == s)) else { return 0.0 };
    ((idx as f64 + (percent / 100.0).clamp(0.0, 1.0)) / stages.len() as f64).clamp(0.0, 1.0)
}

/// Friendly stage names for the UI.
pub fn stage_label(stage: &str) -> &str {
    match stage {
        "ocr" => "Reading page text",
        "text_cleanup" => "Cleaning up text",
        "tts" => "Narrating",
        "sync_map" => "Syncing audio to pages",
        "video" => "Rendering video",
        "init" | "fatal" => "Starting",
        other => other,
    }
}

/// Options picked on the page for one batch of chapters.
#[derive(Clone, Debug)]
pub struct NarrationRequest {
    pub voice: String,
    pub speed: f64,
    pub video: bool,
    pub resolution: String,
    /// `mv_recap::FORMAT_LANDSCAPE` or `FORMAT_REELS`.
    pub format: String,
    /// Explicit pipeline stages (Studio's Audio / Video phases); `None` = full run.
    pub stages: Option<Vec<String>>,
    /// Re-synthesize pages whose audio already exists.
    pub force_regen: bool,
    /// Voice only these pages again (the rest are reused): a per-page Regenerate / New take.
    pub regen_pages: Vec<usize>,
    /// The regen pages move to their next take (a new performance). tts_bridge records the
    /// take in `audio/takes.json` only once the new clip replaced the old one.
    pub new_take: bool,
}

/// The pipeline job for one chapter. Text extraction is skipped when the chapter
/// already has a script or OCR result (the worker then reuses it).
pub fn build_job(req: &NarrationRequest, env: &Env, ch: &ChapterItem) -> PipelineJob {
    let stages = req.stages.clone().unwrap_or_else(|| {
        let mut stages = vec![];
        if !ch.has_text {
            stages.push("ocr".to_string());
        }
        stages.extend(["tts".to_string(), "sync_map".to_string()]);
        if req.video {
            stages.push("video".to_string());
        }
        stages
    });
    let mut job = PipelineJob::new(&ch.pages_dir, &ch.output_dir, Some(stages), &env.providers);
    job.tts_config = json!({
        "engine": mv_studio::tts::engine_hint(&req.voice),
        "voice": req.voice,
        "speed": req.speed,
        "force_regen": req.force_regen,
    });
    if !req.regen_pages.is_empty() {
        job.tts_config["regen_pages"] = json!(req.regen_pages);
        if req.new_take {
            job.tts_config["new_take"] = json!(true);
        }
    }
    job.video_config = json!({
        "resolution": req.resolution,
        "format": req.format,
        // Asking for the video stage means "render it now", not "reuse the last file".
        "force_rerender": true,
        "transition": "fade",
        "transition_duration": 0.5,
        "effect": "ken_burns",
    });
    // The chapter's own score (Music step), when it is on and matches the narration.
    let r = crate::page::chref(ch);
    let music = mv_score::load_settings(&r);
    if req.video && music.enabled && mv_score::is_fresh(&r, &music) {
        job.video_config["score"] = json!(mv_score::score_wav(&r).to_string_lossy());
    }
    // The chapter's caption style (its own, else the Studio default; see `video`).
    if req.video {
        job.video_config["subtitles"] = crate::video::subtitle_config_for(ch);
    }
    job.ocr_engine = env.ocr_engine.clone();
    if matches!(env.ocr, OcrProvider::Local | OcrProvider::Unavailable { .. }) {
        // Without an Ollama model the worker falls through to the local OCR engines.
        job.ollama_url.clear();
        job.ollama_vision_model.clear();
    }
    job
}

struct Inner {
    queue: JobQueue,
    jobs: Mutex<Vec<JobInfo>>,
}

static SERVICE: Mutex<Option<Arc<Inner>>> = Mutex::new(None);
static VERSION: AtomicU64 = AtomicU64::new(0);

fn bump() {
    VERSION.fetch_add(1, Ordering::SeqCst);
}

/// Changes whenever any job changes.
pub fn version() -> u64 {
    VERSION.load(Ordering::SeqCst)
}

fn current() -> Option<Arc<Inner>> {
    SERVICE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn lock_jobs(inner: &Inner) -> std::sync::MutexGuard<'_, Vec<JobInfo>> {
    inner.jobs.lock().unwrap_or_else(|e| e.into_inner())
}

/// The queue for `python` + `layout`, created on first use. It is rebuilt when the
/// interpreter or scripts changed (e.g. after installing Python) and nothing is running.
fn ensure(python: &PythonInterpreter, layout: &PythonLayout) -> Arc<Inner> {
    let mut slot = SERVICE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(inner) = slot.as_ref() {
        let same = inner.queue.interpreter() == python && inner.queue.layout() == layout;
        let busy = lock_jobs(inner).iter().any(|j| !j.state.finished());
        if same || busy {
            return inner.clone();
        }
    }
    let rt = mv_ui::bg::runtime();
    let queue = {
        let _guard = rt.enter();
        JobQueue::new(python.clone(), layout.clone())
    };
    let history = slot.as_ref().map(|i| lock_jobs(i).clone()).unwrap_or_default();
    let inner = Arc::new(Inner { queue, jobs: Mutex::new(history) });
    rt.spawn(pump(inner.clone()));
    *slot = Some(inner.clone());
    inner
}

async fn pump(inner: Arc<Inner>) {
    let mut rx = inner.queue.subscribe();
    loop {
        match rx.recv().await {
            Ok(ev) => {
                let stage_done = apply(&inner, ev);
                if let Some((chapter, stage)) = stage_done {
                    let _ = tokio::task::spawn_blocking(move || crate::data::mark_stage_complete(&chapter, &stage)).await;
                }
                bump();
            }
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => break,
        }
    }
}

/// Folds one queue event into the job list. Returns `(chapter, stage)` when a stage completed.
fn apply(inner: &Inner, ev: JobEvent) -> Option<(String, String)> {
    let mut jobs = lock_jobs(inner);
    let id = match &ev {
        JobEvent::Started { id, .. } | JobEvent::Message { id, .. } | JobEvent::Log { id, .. } | JobEvent::Finished { id, .. } => *id,
    };
    let job = jobs.iter_mut().find(|j| j.id == id)?;
    apply_to(job, ev)
}

fn apply_to(job: &mut JobInfo, ev: JobEvent) -> Option<(String, String)> {
    match ev {
        JobEvent::Started { .. } => {
            job.state = JobView::Running;
            job.status = "Starting…".into();
            job.log("Started");
        }
        JobEvent::Log { line, .. } => job.log(line),
        JobEvent::Message { message: m, .. } => match m.kind.as_str() {
            "progress" => {
                let stage = m.stage.clone().unwrap_or_default();
                if job.stage.as_deref() != Some(stage.as_str()) {
                    job.log(format!("{}…", stage_label(&stage)));
                }
                job.stage_percent = m.percent.unwrap_or_else(|| match (m.current, m.total) {
                    (Some(c), Some(t)) if t > 0 => c as f64 * 100.0 / t as f64,
                    _ => 0.0,
                });
                job.status = match (m.current, m.total) {
                    (Some(c), Some(t)) => format!("{} · {c}/{t}", stage_label(&stage)),
                    _ => stage_label(&stage).to_string(),
                };
                job.stage = Some(stage);
            }
            "info" => {
                if let Some(n) = Narrator::from_message(&m.raw) {
                    // The voice step's own line (with the voices) wins over the worker's guess.
                    if job.narrator.as_ref().is_none_or(|old| old.voice.is_empty() || !n.voice.is_empty()) {
                        job.narrator = Some(n);
                    }
                }
                if let Some(msg) = m.message {
                    job.status = msg.clone();
                    job.log(msg);
                }
            }
            "stage_complete" => {
                let stage = m.stage.unwrap_or_default();
                job.log(format!("Finished: {}", stage_label(&stage)));
                job.stage = Some(stage.clone());
                job.stage_percent = 100.0;
                return Some((job.chapter_id.clone(), stage));
            }
            "error" => {
                let msg = m.message.unwrap_or_else(|| "Worker error".into());
                match m.stage {
                    Some(s) => job.log(format!("Error in {}: {msg}", stage_label(&s))),
                    None => job.log(format!("Error: {msg}")),
                }
            }
            "done" => job.log("Pipeline finished"),
            _ => {}
        },
        JobEvent::Finished { state, .. } => {
            job.state = match state {
                JobState::Completed { .. } => JobView::Done,
                JobState::Failed { error } => JobView::Failed(error),
                JobState::Cancelled => JobView::Cancelled,
                JobState::Queued | JobState::Running => job.state.clone(),
            };
            job.status = match &job.state {
                JobView::Done => "Finished".into(),
                JobView::Failed(e) => e.clone(),
                JobView::Cancelled => "Cancelled".into(),
                _ => job.status.clone(),
            };
            job.log(job.status.clone());
        }
    }
    None
}

/// Queues one pipeline job per chapter, skipping chapters that already have an
/// unfinished job. Returns how many were queued.
pub fn enqueue(env: &Env, req: &NarrationRequest, chapters: &[ChapterItem]) -> anyhow::Result<usize> {
    enqueue_with(env, req, chapters, None).map(|ids| ids.len())
}

/// [`enqueue`] that returns the new jobs' ids, with `video_extra` merged into each job's video
/// settings (Generate's answers, e.g. `{"captions": false}`).
pub fn enqueue_with(env: &Env, req: &NarrationRequest, chapters: &[ChapterItem], video_extra: Option<&serde_json::Value>) -> anyhow::Result<Vec<u64>> {
    let (Ok(python), Ok(layout)) = (&env.python, &env.scripts) else {
        anyhow::bail!("Python and the pipeline scripts are required");
    };
    let inner = ensure(python, layout);
    let mut added = vec![];
    for ch in chapters {
        let busy = lock_jobs(&inner).iter().any(|j| j.chapter_id == ch.id && !j.state.finished());
        if busy {
            continue;
        }
        let mut job = build_job(req, env, ch);
        if let (Some(serde_json::Value::Object(extra)), Some(cfg)) = (video_extra, job.video_config.as_object_mut()) {
            for (k, v) in extra {
                cfg.insert(k.clone(), v.clone());
            }
        }
        let stages = job.stages.clone();
        // Hold the lock across enqueue so the pump cannot see events before the record exists.
        let mut jobs = lock_jobs(&inner);
        let id = inner.queue.enqueue(ch.id.clone(), JobPayload::Pipeline(job));
        jobs.push(JobInfo {
            id,
            chapter_id: ch.id.clone(),
            manga_title: ch.manga_title.clone(),
            chapter_label: ch.label(),
            output_dir: ch.output_dir.clone(),
            voice: req.voice.clone(),
            stages,
            state: JobView::Queued,
            stage: None,
            stage_percent: 0.0,
            status: "Waiting in queue".into(),
            logs: VecDeque::new(),
            narrator: None,
            regen_pages: req.regen_pages.clone(),
        });
        added.push(id);
    }
    bump();
    Ok(added)
}

/// One job by id.
pub fn job(id: u64) -> Option<JobInfo> {
    current().and_then(|i| lock_jobs(&i).iter().find(|j| j.id == id).cloned())
}

pub fn cancel(id: u64) -> bool {
    current().map(|i| i.queue.cancel(id)).unwrap_or(false)
}

/// Drops finished jobs from the list.
pub fn clear_finished() {
    if let Some(inner) = current() {
        lock_jobs(&inner).retain(|j| !j.state.finished());
        bump();
    }
}

/// All jobs, newest first.
pub fn snapshot() -> Vec<JobInfo> {
    let mut out = current().map(|i| lock_jobs(&i).clone()).unwrap_or_default();
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mv_studio::{ProviderConfig, WorkerMessage};
    use std::collections::HashMap;

    fn env(ocr: OcrProvider) -> Env {
        Env {
            python: Err("x".into()),
            scripts: Err("x".into()),
            ffmpeg: true,
            modules: HashMap::new(),
            engines: Default::default(),
            tts_error: None,
            ocr,
            providers: ProviderConfig::default(),
            ocr_engine: "vision".into(),
        }
    }

    fn chapter(has_text: bool) -> ChapterItem {
        ChapterItem {
            id: "c1".into(),
            manga_id: "m".into(),
            manga_title: "M".into(),
            number: Some(1.0),
            title: None,
            pages_dir: "/lib/m/c1/pages".into(),
            output_dir: "/lib/m/c1/pipeline".into(),
            page_count: 10,
            has_text,
            video: None,
            audio_files: 0,
        }
    }

    fn req(video: bool) -> NarrationRequest {
        NarrationRequest {
            voice: "af_heart".into(),
            speed: 1.1,
            video,
            resolution: "720p".into(),
            format: mv_recap::FORMAT_REELS.into(),
            stages: None,
            force_regen: true,
            regen_pages: vec![],
            new_take: false,
        }
    }

    #[test]
    fn job_stages_and_configs() {
        let job = build_job(&req(true), &env(OcrProvider::Gemini), &chapter(false));
        assert_eq!(job.stages, vec!["ocr", "tts", "sync_map", "video"]);
        assert_eq!(job.tts_config["engine"], "kokoro");
        assert_eq!(job.tts_config["voice"], "af_heart");
        assert_eq!(job.tts_config["speed"], 1.1);
        assert_eq!(job.video_config["resolution"], "720p");
        assert_eq!(job.video_config["format"], "reels");
        assert!(job.video_config["subtitles"].is_object(), "a render carries the chapter's caption setting");
        assert_eq!(job.output_dir, PathBuf::from("/lib/m/c1/pipeline"));
        assert!(!job.ollama_url.is_empty());

        let job = build_job(&req(false), &env(OcrProvider::Local), &chapter(true));
        assert_eq!(job.stages, vec!["tts", "sync_map"]);
        assert!(job.ollama_url.is_empty() && job.ollama_vision_model.is_empty());
        assert!(job.tts_config.get("regen_pages").is_none(), "whole chapters by default");

        // A per-page Regenerate voices only that page again.
        let one = NarrationRequest { regen_pages: vec![4], force_regen: false, stages: Some(vec!["tts".into()]), ..req(false) };
        let job = build_job(&one, &env(OcrProvider::Gemini), &chapter(true));
        assert_eq!(job.tts_config["regen_pages"], json!([4]));
        assert_eq!(job.tts_config["force_regen"], false);
        assert!(job.tts_config.get("new_take").is_none(), "Regenerate keeps the page's take");
        assert_eq!(job.stages, vec!["tts"]);
        // New take: the page's next take, recorded by tts_bridge once the clip is in place.
        let take = NarrationRequest { new_take: true, ..one };
        assert_eq!(build_job(&take, &env(OcrProvider::Gemini), &chapter(true)).tts_config["new_take"], true);
    }

    #[test]
    fn progress_spans_stages() {
        let stages: Vec<String> = ["ocr", "tts", "sync_map", "video"].iter().map(|s| s.to_string()).collect();
        assert_eq!(overall_progress(&stages, None, 0.0), 0.0);
        assert_eq!(overall_progress(&stages, Some("ocr"), 50.0), 0.125);
        assert_eq!(overall_progress(&stages, Some("text_cleanup"), 0.0), 0.25);
        assert_eq!(overall_progress(&stages, Some("video"), 100.0), 1.0);
        assert_eq!(overall_progress(&stages, Some("unknown"), 40.0), 0.0);
        assert_eq!(overall_progress(&[], Some("tts"), 40.0), 0.0);
    }

    fn msg(v: serde_json::Value) -> JobEvent {
        JobEvent::Message { id: 1, key: "c1".into(), message: WorkerMessage::from_value(v) }
    }

    #[test]
    fn events_update_job_view() {
        let mut job = JobInfo {
            id: 1,
            chapter_id: "c1".into(),
            manga_title: "M".into(),
            chapter_label: "Chapter 1".into(),
            output_dir: "/o".into(),
            voice: "af_heart".into(),
            stages: vec!["tts".into(), "sync_map".into()],
            state: JobView::Queued,
            stage: None,
            stage_percent: 0.0,
            status: String::new(),
            logs: VecDeque::new(),
            narrator: None,
            regen_pages: vec![],
        };
        apply_to(&mut job, JobEvent::Started { id: 1, key: "c1".into() });
        assert_eq!(job.state, JobView::Running);
        apply_to(&mut job, msg(json!({"type":"progress","stage":"tts","current":2,"total":4,"percent":50.0})));
        apply_to(&mut job, msg(json!({"type":"progress","stage":"tts","current":3,"total":4,"percent":75.0})));
        assert_eq!(job.status, "Narrating · 3/4");
        // The worker names the engine first, then the voice step says who really narrates.
        apply_to(&mut job, msg(json!({"type":"info","stage":"tts","message":"TTS engine: chatterbox","engine":"chatterbox"})));
        assert_eq!(job.narrator.as_ref().map(|n| (n.engine.as_str(), n.stand_in)), Some(("chatterbox", false)));
        apply_to(&mut job, msg(json!({"type":"info","stage":"tts","engine":"kokoro","voice":"cb_narrator_f","synth_voice":"af_heart","stand_in":true,
            "message":"Chatterbox could not run (CUDA); narrating with Kokoro (af_heart) instead."})));
        let n = job.narrator.clone().unwrap();
        assert_eq!((n.engine_label(), n.synth_voice.as_str(), n.stand_in), ("Kokoro", "af_heart", true));
        assert!(job.status.starts_with("Chatterbox could not run"));
        apply_to(&mut job, msg(json!({"type":"info","message":"Loaded 3 existing audio file(s) from disk"})));
        assert_eq!(job.narrator, Some(n), "other info lines keep the narrator");
        assert_eq!(job.progress(), 0.375);
        assert_eq!(job.logs.iter().filter(|l| l.starts_with("Narrating")).count(), 1, "progress logs once per stage");
        let done = apply_to(&mut job, msg(json!({"type":"stage_complete","stage":"tts"})));
        assert_eq!(done, Some(("c1".into(), "tts".into())));
        apply_to(&mut job, JobEvent::Log { id: 1, key: "c1".into(), line: "[ocr] noise".into() });
        apply_to(&mut job, JobEvent::Finished { id: 1, key: "c1".into(), state: JobState::Failed { error: "No script text".into() } });
        assert_eq!(job.state, JobView::Failed("No script text".into()));
        assert_eq!(job.status, "No script text");
        assert!(job.logs.contains(&"[ocr] noise".to_string()));
    }

    #[test]
    fn logs_are_capped() {
        let mut job = JobInfo {
            id: 1,
            chapter_id: "c".into(),
            manga_title: String::new(),
            chapter_label: String::new(),
            output_dir: "/o".into(),
            voice: String::new(),
            stages: vec![],
            state: JobView::Running,
            stage: None,
            stage_percent: 0.0,
            status: String::new(),
            logs: VecDeque::new(),
            narrator: None,
            regen_pages: vec![],
        };
        for i in 0..(MAX_LOG_LINES + 50) {
            job.log(i.to_string());
        }
        assert_eq!(job.logs.len(), MAX_LOG_LINES);
        assert_eq!(job.logs.front().map(String::as_str), Some("50"));
    }
}
