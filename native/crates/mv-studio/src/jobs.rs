//! Pipeline job payloads (the JSON written to each worker's job file) and a
//! sequential in-memory job queue. Payload field names match the objects built in
//! `pipeline.ipc.ts`, `text.ipc.ts` and `python-bridge.ts`.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::{broadcast, Notify};

use crate::bridge::{run_worker_logged, CancelToken, WorkerMessage, WorkerSpec};
use crate::error::StudioError;
use crate::python::{configured_python, discover_python, PythonInterpreter, PythonLayout};

/// Stages run when a pipeline job names none (`python-bridge.ts`).
pub const DEFAULT_STAGES: &[&str] = &["ocr", "tts", "sync_map", "video"];

/// Ollama / Gemini / g0i settings read from `config.json`, with the TS defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub ollama_url: String,
    pub ollama_vision_model: String,
    pub ollama_vision_temperature: f64,
    pub gemini_api_key: String,
    pub g0i_api_key: String,
    pub g0i_model: String,
    pub g0i_family: String,
    pub g0i_base_url: String,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            ollama_url: "http://127.0.0.1:11434".into(),
            ollama_vision_model: "minicpm-v:8b".into(),
            ollama_vision_temperature: 0.3,
            gemini_api_key: String::new(),
            g0i_api_key: String::new(),
            g0i_model: "gemini-2.5-flash".into(),
            g0i_family: "gemini".into(),
            g0i_base_url: "https://g0i.ai/v1".into(),
        }
    }
}

impl ProviderConfig {
    /// Reads `ollama`, `gemini` and `g0i` sections of `config.json`; missing file or keys keep defaults.
    pub fn load(config: &Path) -> Self {
        let mut out = Self::default();
        let Some(all) = std::fs::read_to_string(config)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        else {
            return out;
        };
        let str_of = |v: Option<&Value>, default: &str| {
            v.and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| default.to_string())
        };
        if let Some(o) = all.get("ollama") {
            out.ollama_url = str_of(o.get("url"), &out.ollama_url);
            out.ollama_vision_model = str_of(o.get("vision_model"), &out.ollama_vision_model);
            if let Some(t) = o.get("vision_temperature").and_then(Value::as_f64) {
                out.ollama_vision_temperature = t;
            }
        }
        out.gemini_api_key = str_of(all.pointer("/gemini/api_key"), "");
        if let Some(g) = all.get("g0i") {
            out.g0i_api_key = str_of(g.get("api_key"), &out.g0i_api_key);
            out.g0i_model = str_of(g.get("model"), &out.g0i_model);
            out.g0i_family = str_of(g.get("family"), &out.g0i_family);
            out.g0i_base_url = str_of(g.get("base_url"), &out.g0i_base_url);
        }
        out
    }
}

/// `pipeline_worker.py` job: OCR, TTS, sync map and video stages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineJob {
    pub pages_dir: PathBuf,
    pub output_dir: PathBuf,
    pub stages: Vec<String>,
    pub tts_config: Value,
    pub video_config: Value,
    pub ocr_engine: String,
    pub ollama_url: String,
    pub ollama_vision_model: String,
    pub ollama_vision_temperature: f64,
    pub gemini_api_key: String,
    pub g0i_api_key: String,
    pub g0i_model: String,
    pub g0i_family: String,
    pub g0i_base_url: String,
}

impl PipelineJob {
    /// Builds a job with the defaults `python-bridge.ts` applies.
    pub fn new(
        pages_dir: impl Into<PathBuf>,
        output_dir: impl Into<PathBuf>,
        stages: Option<Vec<String>>,
        providers: &ProviderConfig,
    ) -> Self {
        Self {
            pages_dir: pages_dir.into(),
            output_dir: output_dir.into(),
            stages: stages
                .unwrap_or_else(|| DEFAULT_STAGES.iter().map(|s| s.to_string()).collect()),
            tts_config: Value::Object(Map::new()),
            video_config: Value::Object(Map::new()),
            ocr_engine: "vision".into(),
            ollama_url: providers.ollama_url.clone(),
            ollama_vision_model: providers.ollama_vision_model.clone(),
            ollama_vision_temperature: providers.ollama_vision_temperature,
            gemini_api_key: providers.gemini_api_key.clone(),
            g0i_api_key: providers.g0i_api_key.clone(),
            g0i_model: providers.g0i_model.clone(),
            g0i_family: providers.g0i_family.clone(),
            g0i_base_url: providers.g0i_base_url.clone(),
        }
    }

    /// Job for `TEXT_SEND_TO_TTS`: TTS and sync map, plus video unless `auto_continue_to_video` is off.
    pub fn send_to_tts(
        pages_dir: impl Into<PathBuf>,
        output_dir: impl Into<PathBuf>,
        tts_config: Value,
        video_config: Value,
        auto_video: bool,
        providers: &ProviderConfig,
    ) -> Self {
        let mut stages = vec!["tts".to_string(), "sync_map".to_string()];
        if auto_video {
            stages.push("video".to_string());
        }
        let mut job = Self::new(pages_dir, output_dir, Some(stages), providers);
        job.tts_config = tts_config;
        job.video_config = video_config;
        job
    }
}

/// `vision_extract.py` job: OCR / text extraction from page images.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextExtractJob {
    pub pages_dir: PathBuf,
    pub output_dir: PathBuf,
    pub ollama_url: String,
    pub model: String,
    pub temperature: f64,
    pub gemini_api_key: String,
    pub gemini_model: String,
    /// `auto` (default), `ollama`, `gemini` or `paddleocr`.
    pub engine: String,
    pub paddle_lang: String,
}

impl TextExtractJob {
    pub fn new(
        pages_dir: impl Into<PathBuf>,
        output_dir: impl Into<PathBuf>,
        providers: &ProviderConfig,
        engine: Option<&str>,
        paddle_lang: Option<&str>,
    ) -> Self {
        Self {
            pages_dir: pages_dir.into(),
            output_dir: output_dir.into(),
            ollama_url: providers.ollama_url.clone(),
            model: providers.ollama_vision_model.clone(),
            temperature: providers.ollama_vision_temperature,
            gemini_api_key: providers.gemini_api_key.clone(),
            gemini_model: String::new(),
            engine: engine
                .map(|e| e.to_lowercase())
                .unwrap_or_else(|| "auto".into()),
            paddle_lang: paddle_lang.unwrap_or("en").to_string(),
        }
    }
}

/// One cast member fed to the recap prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CastMember {
    pub name: String,
    pub description: String,
}

/// `text_enhance.py` job in a per-page mode (`detailed`, ...).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnhanceJob {
    /// `{ page, text }` segments (text may carry extra keys; they pass through).
    pub segments: Vec<Value>,
    pub output_dir: PathBuf,
    pub ollama_url: String,
    pub model: String,
    pub fallback_models: Vec<String>,
    pub mode: String,
    pub temperature: f64,
    pub manga_title: String,
    pub chapter_number: Option<i64>,
    pub context_pages: u32,
    pub cast: Vec<CastMember>,
}

/// `text_enhance.py` combine pass: merges `enhanced_detailed.json` into one script.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CombineJob {
    pub output_dir: PathBuf,
    pub ollama_url: String,
    pub model: String,
    pub fallback_models: Vec<String>,
    /// Always `combine`.
    pub mode: String,
    pub temperature: f64,
    pub max_combine_chars: u32,
}

/// `scene_narrator.py` job. The fields are the ones `TEXT_NARRATE_START` builds
/// (pages, memory, provider keys, batch size, page range, ...); they are kept as
/// a map because the narrator reads them by name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NarrateJob {
    pub output_dir: PathBuf,
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

/// One job the queue can run.
#[derive(Debug, Clone, PartialEq)]
pub enum JobPayload {
    Pipeline(PipelineJob),
    TextExtract(TextExtractJob),
    TextEnhance(EnhanceJob),
    TextCombine(CombineJob),
    Narrate(NarrateJob),
}

impl JobPayload {
    /// Name used in logs and snapshots.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Pipeline(_) => "pipeline",
            Self::TextExtract(_) => "text-extract",
            Self::TextEnhance(_) => "text-enhance",
            Self::TextCombine(_) => "text-combine",
            Self::Narrate(_) => "narrate",
        }
    }

    /// Worker script in the scripts directory.
    pub fn script(&self) -> &'static str {
        match self {
            Self::Pipeline(_) => "pipeline_worker.py",
            Self::TextExtract(_) => "vision_extract.py",
            Self::TextEnhance(_) | Self::TextCombine(_) => "text_enhance.py",
            Self::Narrate(_) => "scene_narrator.py",
        }
    }

    /// Text workers treat any `error` as fatal; the pipeline only non-recoverable ones.
    pub fn strict_errors(&self) -> bool {
        !matches!(self, Self::Pipeline(_))
    }

    /// Directory the job file and worker output go to.
    pub fn output_dir(&self) -> &Path {
        match self {
            Self::Pipeline(j) => &j.output_dir,
            Self::TextExtract(j) => &j.output_dir,
            Self::TextEnhance(j) => &j.output_dir,
            Self::TextCombine(j) => &j.output_dir,
            Self::Narrate(j) => &j.output_dir,
        }
    }

    /// Serialized job JSON.
    pub fn to_job_json(&self) -> Result<String, serde_json::Error> {
        match self {
            Self::Pipeline(j) => serde_json::to_string_pretty(j),
            Self::TextExtract(j) => serde_json::to_string_pretty(j),
            Self::TextEnhance(j) => serde_json::to_string_pretty(j),
            Self::TextCombine(j) => serde_json::to_string_pretty(j),
            Self::Narrate(j) => serde_json::to_string_pretty(j),
        }
    }

    /// Writes the job file the way the TS bridge does: `job.json` for pipeline
    /// jobs, `job_<millis>.json` for the text workers. Creates the output directory.
    pub fn write_job_file(&self) -> Result<PathBuf, StudioError> {
        let dir = self.output_dir();
        std::fs::create_dir_all(dir)?;
        let name = match self {
            Self::Pipeline(_) => "job.json".to_string(),
            _ => {
                let millis = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0);
                format!("job_{millis}.json")
            }
        };
        let path = dir.join(name);
        std::fs::write(&path, self.to_job_json()?)?;
        Ok(path)
    }
}

/// Lifecycle of a queued job.
#[derive(Debug, Clone, PartialEq)]
pub enum JobState {
    Queued,
    Running,
    Completed { outputs: Option<Value> },
    Failed { error: String },
    Cancelled,
}

impl JobState {
    pub fn is_finished(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. } | Self::Failed { .. } | Self::Cancelled
        )
    }
}

/// Read-only view of one job.
#[derive(Debug, Clone, PartialEq)]
pub struct JobSnapshot {
    pub id: u64,
    /// Caller key, usually the chapter id (used by `cancel_key`).
    pub key: String,
    pub kind: &'static str,
    pub state: JobState,
    pub last_message: Option<WorkerMessage>,
}

/// Queue notifications.
#[derive(Debug, Clone)]
pub enum JobEvent {
    Started {
        id: u64,
        key: String,
    },
    /// Every protocol message from the running worker.
    Message {
        id: u64,
        key: String,
        message: WorkerMessage,
    },
    /// A non-protocol output line from the running worker (stderr, library logging).
    Log {
        id: u64,
        key: String,
        line: String,
    },
    Finished {
        id: u64,
        key: String,
        state: JobState,
    },
}

#[derive(Default)]
struct QueueState {
    jobs: Vec<JobSnapshot>,
    pending: VecDeque<u64>,
    payloads: HashMap<u64, JobPayload>,
    running: Option<(u64, CancelToken)>,
    next_id: u64,
}

struct Shared {
    state: Mutex<QueueState>,
    wake: Notify,
    events: broadcast::Sender<JobEvent>,
    interpreter: PythonInterpreter,
    layout: PythonLayout,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn emit(&self, event: JobEvent) {
        // No subscribers is fine.
        let _ = self.events.send(event);
    }
}

/// Runs jobs one at a time, in enqueue order. Jobs stay in memory for the
/// lifetime of the queue (the TS app keeps history in SQLite; that is not ported).
///
/// `JobQueue::new` spawns the worker loop on the current Tokio runtime.
#[derive(Clone)]
pub struct JobQueue {
    shared: Arc<Shared>,
}

impl JobQueue {
    pub fn new(interpreter: PythonInterpreter, layout: PythonLayout) -> Self {
        let (events, _) = broadcast::channel(256);
        let shared = Arc::new(Shared {
            state: Mutex::new(QueueState::default()),
            wake: Notify::new(),
            events,
            interpreter,
            layout,
        });
        tokio::spawn(run_loop(shared.clone()));
        Self { shared }
    }

    /// Discovers Python (honouring `pipeline.python_path` in `config.json`) and
    /// resolves the scripts directory, then builds a queue.
    pub fn discover(config: &Path, scripts_dir: Option<&Path>) -> Result<Self, StudioError> {
        let interpreter = discover_python(configured_python(config).as_deref())?;
        let layout = PythonLayout::resolve(scripts_dir)?;
        Ok(Self::new(interpreter, layout))
    }

    pub fn interpreter(&self) -> &PythonInterpreter {
        &self.shared.interpreter
    }

    pub fn layout(&self) -> &PythonLayout {
        &self.shared.layout
    }

    /// Adds a job to the end of the queue and returns its id.
    pub fn enqueue(&self, key: impl Into<String>, payload: JobPayload) -> u64 {
        let id = {
            let mut st = self.shared.lock();
            st.next_id += 1;
            let id = st.next_id;
            st.jobs.push(JobSnapshot {
                id,
                key: key.into(),
                kind: payload.kind(),
                state: JobState::Queued,
                last_message: None,
            });
            st.pending.push_back(id);
            st.payloads.insert(id, payload);
            id
        };
        self.shared.wake.notify_one();
        id
    }

    /// Cancels a queued job (it never starts) or the running one (its process tree is killed).
    /// Returns false when the job is unknown or already finished.
    pub fn cancel(&self, id: u64) -> bool {
        let mut st = self.shared.lock();
        if let Some(pos) = st.pending.iter().position(|&p| p == id) {
            st.pending.remove(pos);
            st.payloads.remove(&id);
            let event = finish(&mut st, id, JobState::Cancelled);
            drop(st);
            if let Some(event) = event {
                self.shared.emit(event);
            }
            return true;
        }
        if let Some((running, token)) = &st.running {
            if *running == id {
                token.cancel();
                return true;
            }
        }
        false
    }

    /// Cancels every unfinished job with `key`. Returns how many were signalled.
    pub fn cancel_key(&self, key: &str) -> usize {
        let ids: Vec<u64> = {
            let st = self.shared.lock();
            st.jobs
                .iter()
                .filter(|j| j.key == key && !j.state.is_finished())
                .map(|j| j.id)
                .collect()
        };
        ids.into_iter().filter(|&id| self.cancel(id)).count()
    }

    pub fn snapshot(&self) -> Vec<JobSnapshot> {
        self.shared.lock().jobs.clone()
    }

    pub fn job(&self, id: u64) -> Option<JobSnapshot> {
        self.shared.lock().jobs.iter().find(|j| j.id == id).cloned()
    }

    /// Receives `JobEvent`s from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<JobEvent> {
        self.shared.events.subscribe()
    }
}

/// Records a job's final state and returns the event to emit.
fn finish(st: &mut QueueState, id: u64, state: JobState) -> Option<JobEvent> {
    let job = st.jobs.iter_mut().find(|j| j.id == id)?;
    job.state = state.clone();
    Some(JobEvent::Finished {
        id,
        key: job.key.clone(),
        state,
    })
}

async fn run_loop(shared: Arc<Shared>) {
    loop {
        let next = {
            let mut st = shared.lock();
            match st.pending.pop_front() {
                Some(id) => {
                    let payload = st.payloads.remove(&id);
                    let token = CancelToken::new();
                    st.running = Some((id, token.clone()));
                    if let Some(job) = st.jobs.iter_mut().find(|j| j.id == id) {
                        job.state = JobState::Running;
                    }
                    let key = st
                        .jobs
                        .iter()
                        .find(|j| j.id == id)
                        .map(|j| j.key.clone())
                        .unwrap_or_default();
                    payload.map(|p| (id, key, p, token))
                }
                None => None,
            }
        };
        let Some((id, key, payload, token)) = next else {
            shared.wake.notified().await;
            continue;
        };

        shared.emit(JobEvent::Started {
            id,
            key: key.clone(),
        });
        let state = match execute(&shared, id, &key, &payload, &token).await {
            Ok(outputs) => JobState::Completed { outputs },
            Err(StudioError::Cancelled) => JobState::Cancelled,
            Err(err) => JobState::Failed {
                error: err.to_string(),
            },
        };

        let event = {
            let mut st = shared.lock();
            st.running = None;
            finish(&mut st, id, state)
        };
        if let Some(event) = event {
            shared.emit(event);
        }
    }
}

async fn execute(
    shared: &Shared,
    id: u64,
    key: &str,
    payload: &JobPayload,
    token: &CancelToken,
) -> Result<Option<Value>, StudioError> {
    let job_path = payload.write_job_file()?;
    let spec = WorkerSpec {
        interpreter: shared.interpreter.clone(),
        script: shared.layout.script(payload.script())?,
        args: vec![job_path.display().to_string()],
        cwd: shared.layout.dir.clone(),
        strict_errors: payload.strict_errors(),
    };
    let report = run_worker_logged(
        &spec,
        token,
        |message| {
            {
                let mut st = shared.lock();
                if let Some(job) = st.jobs.iter_mut().find(|j| j.id == id) {
                    job.last_message = Some(message.clone());
                }
            }
            shared.emit(JobEvent::Message {
                id,
                key: key.to_string(),
                message: message.clone(),
            });
        },
        |line| {
            shared.emit(JobEvent::Log {
                id,
                key: key.to_string(),
                line: line.to_string(),
            });
        },
    )
    .await?;
    Ok(report.outputs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;
    use std::time::Duration;

    fn providers() -> ProviderConfig {
        ProviderConfig::default()
    }

    #[test]
    fn pipeline_job_serializes_like_python_bridge() {
        let mut job = PipelineJob::new("/lib/m/c/pages", "/lib/m/c/pipeline", None, &providers());
        job.tts_config = serde_json::json!({"engine": "kokoro", "voice": "af_heart", "speed": 1.0});
        let v = serde_json::to_value(&job).unwrap();
        assert_eq!(
            v["stages"],
            serde_json::json!(["ocr", "tts", "sync_map", "video"])
        );
        assert_eq!(v["ocr_engine"], "vision");
        assert_eq!(v["ollama_vision_model"], "minicpm-v:8b");
        assert_eq!(v["g0i_base_url"], "https://g0i.ai/v1");
        assert_eq!(v["tts_config"]["voice"], "af_heart");
        assert!(v.get("pages_dir").is_some() && v.get("output_dir").is_some());
    }

    #[test]
    fn send_to_tts_stages_follow_auto_video_flag() {
        let with = PipelineJob::send_to_tts("p", "o", Value::Null, Value::Null, true, &providers());
        assert_eq!(with.stages, vec!["tts", "sync_map", "video"]);
        let without =
            PipelineJob::send_to_tts("p", "o", Value::Null, Value::Null, false, &providers());
        assert_eq!(without.stages, vec!["tts", "sync_map"]);
    }

    #[test]
    fn enhance_and_combine_payload_keys() {
        let enhance = EnhanceJob {
            segments: vec![serde_json::json!({"page": 0, "text": "hi"})],
            output_dir: "o".into(),
            ollama_url: "u".into(),
            model: "m".into(),
            fallback_models: vec!["f".into()],
            mode: "detailed".into(),
            temperature: 0.7,
            manga_title: "T".into(),
            chapter_number: None,
            context_pages: 2,
            cast: vec![CastMember {
                name: "A".into(),
                description: "d".into(),
            }],
        };
        let v = serde_json::to_value(&enhance).unwrap();
        assert!(v["chapter_number"].is_null());
        assert_eq!(v["cast"][0]["name"], "A");
        let combine = CombineJob {
            output_dir: "o".into(),
            ollama_url: "u".into(),
            model: "m".into(),
            fallback_models: vec![],
            mode: "combine".into(),
            temperature: 0.6,
            max_combine_chars: 16000,
        };
        let v = serde_json::to_value(&combine).unwrap();
        assert_eq!(v["max_combine_chars"], 16000);
    }

    #[test]
    fn narrate_job_flattens_extra_fields() {
        let mut fields = Map::new();
        fields.insert("provider".into(), Value::from("gemini"));
        fields.insert("batch_size".into(), Value::from(12));
        let job = NarrateJob {
            output_dir: "o".into(),
            fields,
        };
        let v = serde_json::to_value(&job).unwrap();
        assert_eq!(v["output_dir"], "o");
        assert_eq!(v["provider"], "gemini");
        assert_eq!(v["batch_size"], 12);
        let back: NarrateJob = serde_json::from_value(v).unwrap();
        assert_eq!(back, job);
    }

    #[test]
    fn job_file_names_and_strictness() {
        let dir = testutil::unique_dir("jobfile");
        let pipeline = JobPayload::Pipeline(PipelineJob::new(
            dir.join("pages"),
            dir.join("out"),
            None,
            &providers(),
        ));
        assert_eq!(
            pipeline.write_job_file().unwrap(),
            dir.join("out").join("job.json")
        );
        assert!(!pipeline.strict_errors());
        let extract = JobPayload::TextExtract(TextExtractJob::new(
            dir.join("pages"),
            dir.join("text"),
            &providers(),
            Some("PaddleOCR"),
            None,
        ));
        let path = extract.write_job_file().unwrap();
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("job_"));
        assert!(extract.strict_errors());
        let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(v["engine"], "paddleocr");
        assert_eq!(v["paddle_lang"], "en");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn provider_config_reads_sections_with_defaults() {
        let dir = testutil::unique_dir("providers");
        let cfg = dir.join("config.json");
        std::fs::write(&cfg, r#"{"ollama":{"vision_model":"llava:13b"},"gemini":{"api_key":"k"},"g0i":{"api_key":"g"}}"#).unwrap();
        let p = ProviderConfig::load(&cfg);
        assert_eq!(p.ollama_vision_model, "llava:13b");
        assert_eq!(p.ollama_url, "http://127.0.0.1:11434");
        assert_eq!(p.gemini_api_key, "k");
        assert_eq!(p.g0i_api_key, "g");
        assert_eq!(p.g0i_family, "gemini");
        assert_eq!(
            ProviderConfig::load(&dir.join("none.json")),
            ProviderConfig::default()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    async fn next_finished(rx: &mut broadcast::Receiver<JobEvent>) -> (u64, JobState) {
        loop {
            match tokio::time::timeout(Duration::from_secs(60), rx.recv())
                .await
                .expect("queue event in time")
                .unwrap()
            {
                JobEvent::Finished { id, state, .. } => return (id, state),
                _ => {}
            }
        }
    }

    fn fake_queue(tag: &str) -> Option<(JobQueue, PathBuf)> {
        let interp = testutil::interpreter()?;
        let dir = testutil::unique_dir(tag);
        let scripts = dir.join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        testutil::write_fake_worker_as(&scripts, "pipeline_worker.py");
        let queue = JobQueue::new(interp, PythonLayout::new(&scripts));
        Some((queue, dir))
    }

    fn fake_payload(dir: &Path, mode: &str, id: u64) -> JobPayload {
        // The fake worker reads its mode and id from the basename of pages_dir.
        JobPayload::Pipeline(PipelineJob::new(
            dir.join(format!("{mode}-{id}")),
            dir.join(format!("out-{id}")),
            None,
            &providers(),
        ))
    }

    #[tokio::test]
    async fn queue_runs_jobs_sequentially_in_order() {
        let Some((queue, dir)) = fake_queue("queue-order") else {
            eprintln!("skipping: no Python 3 on PATH");
            return;
        };
        let mut rx = queue.subscribe();
        let ids: Vec<u64> = (1..=3)
            .map(|i| queue.enqueue("ch", fake_payload(&dir, "ok", i)))
            .collect();

        let mut started = Vec::new();
        let mut in_flight = 0i32;
        let mut max_in_flight = 0i32;
        let mut finished = Vec::new();
        while finished.len() < ids.len() {
            match tokio::time::timeout(Duration::from_secs(60), rx.recv())
                .await
                .expect("event in time")
                .unwrap()
            {
                JobEvent::Started { id, .. } => {
                    started.push(id);
                    in_flight += 1;
                    max_in_flight = max_in_flight.max(in_flight);
                }
                JobEvent::Finished { id, state, .. } => {
                    in_flight -= 1;
                    assert!(
                        matches!(state, JobState::Completed { .. }),
                        "job {id}: {state:?}"
                    );
                    finished.push(id);
                }
                JobEvent::Message { .. } | JobEvent::Log { .. } => {}
            }
        }
        assert_eq!(started, ids);
        assert_eq!(finished, ids);
        assert_eq!(max_in_flight, 1);
        assert!(matches!(
            queue.job(ids[0]).unwrap().state,
            JobState::Completed { outputs: Some(_) }
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn cancel_running_job_then_next_job_runs() {
        let Some((queue, dir)) = fake_queue("queue-cancel") else {
            eprintln!("skipping: no Python 3 on PATH");
            return;
        };
        let mut rx = queue.subscribe();
        let slow = queue.enqueue("ch-1", fake_payload(&dir, "sleep", 1));
        let fast = queue.enqueue("ch-2", fake_payload(&dir, "ok", 2));

        loop {
            if let JobEvent::Message { id, message, .. } =
                tokio::time::timeout(Duration::from_secs(60), rx.recv())
                    .await
                    .unwrap()
                    .unwrap()
            {
                if id == slow && message.kind == "progress" {
                    break;
                }
            }
        }
        assert!(queue.cancel(slow));
        assert_eq!(next_finished(&mut rx).await, (slow, JobState::Cancelled));
        assert_eq!(next_finished(&mut rx).await.0, fast);
        assert!(matches!(
            queue.job(fast).unwrap().state,
            JobState::Completed { .. }
        ));
        assert!(
            !queue.cancel(slow),
            "finished jobs cannot be cancelled again"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn cancel_queued_job_never_starts() {
        let Some((queue, dir)) = fake_queue("queue-cancel-queued") else {
            eprintln!("skipping: no Python 3 on PATH");
            return;
        };
        let mut rx = queue.subscribe();
        let blocker = queue.enqueue("ch-1", fake_payload(&dir, "sleep", 1));
        let queued = queue.enqueue("ch-2", fake_payload(&dir, "ok", 2));
        let last = queue.enqueue("ch-3", fake_payload(&dir, "ok", 3));
        // Wait until the blocker is running before cancelling the queued job.
        loop {
            if let JobEvent::Started { id, .. } =
                tokio::time::timeout(Duration::from_secs(60), rx.recv())
                    .await
                    .unwrap()
                    .unwrap()
            {
                if id == blocker {
                    break;
                }
            }
        }
        assert_eq!(queue.cancel_key("ch-2"), 1);
        assert_eq!(queue.job(queued).unwrap().state, JobState::Cancelled);
        assert!(queue.cancel(blocker));

        let mut seen = Vec::new();
        for _ in 0..3 {
            let (id, state) = next_finished(&mut rx).await;
            seen.push((id, state));
            if id == last {
                break;
            }
        }
        assert!(seen.contains(&(blocker, JobState::Cancelled)), "{seen:?}");
        assert!(seen.contains(&(queued, JobState::Cancelled)), "{seen:?}");
        assert!(
            seen.iter()
                .any(|(id, s)| *id == last && matches!(s, JobState::Completed { .. })),
            "{seen:?}"
        );
        assert!(
            queue.job(queued).unwrap().last_message.is_none(),
            "cancelled-while-queued job must not run"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
