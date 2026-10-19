//! Environment check for Studio: Python, the pipeline scripts, FFmpeg, installed
//! voice engines (with each voice's readiness, as `kokoro_tts.py` reports it) and a
//! text-extraction provider. Every gap comes with the exact command that fixes it.
//!
//! The last finished check is shared ([`latest`]): Studio and Settings → Audio both show
//! it, and both check again when [`mv_studio::voices::version`] moves (a pack downloaded or
//! removed, an engine installed, "Check again"), so neither shows a stale answer.
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use mv_studio::python::{config_path, configured_python, discover_python, quiet_command, PythonInterpreter, PythonLayout};
use mv_studio::tts::{self, VoiceInfo};
use mv_studio::voices;
use mv_studio::ProviderConfig;
use serde_json::Value;

/// Python modules probed with `importlib.util.find_spec` (nothing is imported).
const MODULES: &[&str] = &["kokoro_onnx", "soundfile", "numpy", "piper", "onnxruntime", "paddleocr", "manga_ocr", "PIL"];

/// What `kokoro_tts.py install-engine` pip-installs for each engine (`ENGINE_PACKAGES`).
pub const KOKORO_PACKAGES: &[&str] = &["kokoro-onnx>=0.4.0", "soundfile>=0.12.0", "numpy>=1.26"];
pub const PIPER_PACKAGES: &[&str] = &["piper-tts>=1.3.0", "onnxruntime>=1.19.0,<2"];

/// Ollama models that can read images (`python-bridge.ts`).
const VISION_KEYWORDS: &[&str] = &["llava", "vision", "bakllava", "moondream", "cogvlm", "minicpm-v", "llama3.2-vision", "gemma3"];
const VISION_PREFERENCE: &[&str] = &["minicpm-v", "llava-llama3", "llava", "gemma3", "moondream", "bakllava", "cogvlm", "llama3.2-vision"];

/// What a voice's own engine lacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Missing {
    /// The model files (the voice pack) are not downloaded.
    Model,
    /// The engine's Python packages are not installed (or do not load).
    Engine,
}

/// One voice as `kokoro_tts.py` reports it (`voices` in `status`, same as `voices-ready`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceStatus {
    /// It can be synthesized now, by its own engine or by a stand-in voice.
    pub ready: bool,
    /// Engine that runs (`none` when not ready).
    pub engine: String,
    /// The stand-in voice, when the voice's own engine cannot run.
    pub fallback_voice: Option<String>,
    pub fallback_engine: Option<String>,
    /// What the voice's own engine lacks (`None` when it runs natively).
    pub missing: Option<Missing>,
    /// What to get and where, when it is not ready.
    pub message: Option<String>,
}

impl VoiceStatus {
    fn from_json(v: &Value) -> Option<Self> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        Some(Self {
            ready: v.get("ready")?.as_bool()?,
            engine: s("engine").unwrap_or_else(|| "none".into()),
            fallback_voice: s("fallbackVoice"),
            fallback_engine: s("fallbackEngine"),
            missing: match v.get("missing").and_then(Value::as_str) {
                Some("model") => Some(Missing::Model),
                Some("engine") => Some(Missing::Engine),
                _ => None,
            },
            message: s("message"),
        })
    }
}

/// Installed voice engines as reported by `kokoro_tts.py status`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Engines {
    pub kokoro: bool,
    pub kokoro_models: bool,
    pub kokoro_deps: bool,
    pub piper_deps: bool,
    /// At least one Hindi / English Piper voice can speak (packages and that voice's files).
    pub piper_hi: bool,
    pub piper_en: bool,
    pub f5: bool,
    pub parler: bool,
    pub xtts: bool,
    /// Chatterbox (expressive narrator) runs: its own engine venv and its weights.
    pub chatterbox: bool,
    /// Chatterbox's engine (its own Python 3.11 + PyTorch venv) is installed and verified.
    pub chatterbox_deps: bool,
    /// Interpreter the voice scripts use (`activePython`).
    pub python: Option<String>,
    /// Per-voice truth from `kokoro_tts.py`. Empty when the script did not report it.
    pub voices: HashMap<String, VoiceStatus>,
    /// Piper voices whose files are downloaded; used only when `voices` is empty.
    pub piper_voices: Vec<String>,
}

impl Engines {
    pub fn any(&self) -> bool {
        self.kokoro || self.piper_hi || self.piper_en || self.f5 || self.parler || self.xtts || self.chatterbox
    }

    /// Ready engine names, best first.
    pub fn names(&self) -> Vec<&'static str> {
        let mut out = vec![];
        for (on, name) in [(self.chatterbox, "Chatterbox"), (self.f5, "F5-TTS"), (self.parler, "Parler-TTS"), (self.xtts, "XTTS"), (self.kokoro, "Kokoro")] {
            if on {
                out.push(name);
            }
        }
        if self.piper_hi || self.piper_en {
            out.push("Piper");
        }
        out
    }

    /// What `kokoro_tts.py` reported for one voice.
    pub fn voice_status(&self, id: &str) -> Option<&VoiceStatus> {
        self.voices.get(id)
    }

    /// Parses the JSON printed by `kokoro_tts.py status` (its `voices` map included).
    pub fn from_status(v: &Value) -> Self {
        let b = |p: &str| v.pointer(p).and_then(Value::as_bool).unwrap_or(false);
        let voices: HashMap<String, VoiceStatus> = v
            .get("voices")
            .and_then(Value::as_object)
            .map(|m| m.iter().filter_map(|(id, s)| Some((id.clone(), VoiceStatus::from_json(s)?))).collect())
            .unwrap_or_default();
        let piper_deps = b("/piperDepsReady") || b("/engines/piper/ready");
        let mut e = Self {
            kokoro: b("/engines/kokoro/ready"),
            kokoro_models: b("/kokoroReady"),
            kokoro_deps: b("/kokoroDepsReady"),
            piper_deps,
            piper_hi: false,
            piper_en: false,
            f5: b("/f5Ready"),
            parler: b("/parlerReady"),
            xtts: b("/xttsReady"),
            chatterbox: b("/chatterboxReady"),
            chatterbox_deps: b("/chatterboxDepsReady"),
            python: v.get("activePython").and_then(Value::as_str).map(str::to_string),
            piper_voices: voices.iter().filter(|(id, s)| tts::engine_hint(id) == "piper" && s.missing != Some(Missing::Model)).map(|(id, _)| id.clone()).collect(),
            voices,
        };
        e.piper_voices.sort();
        if e.voices.is_empty() {
            // An older script: only per-language flags (any voice of the language downloaded).
            e.piper_hi = piper_deps && b("/piperHiReady");
            e.piper_en = piper_deps && b("/piperEnReady");
        } else {
            e.refresh_piper_flags();
        }
        e
    }

    /// Recomputes `piper_hi` / `piper_en` from `piper_voices`.
    pub fn refresh_piper_flags(&mut self) {
        let any = |lang: &str| self.piper_deps && self.piper_voices.iter().any(|v| tts::voice_lang(v) == lang);
        (self.piper_hi, self.piper_en) = (any("hi"), any("en"));
    }

    /// The voice runs on its own engine (estimate from the engine flags).
    fn native(&self, id: &str) -> bool {
        match tts::engine_hint(id) {
            "kokoro" => self.kokoro,
            "piper" => self.piper_deps && self.piper_voices.iter().any(|v| v == id),
            "f5-tts" => self.f5,
            "parler-tts" => self.parler,
            "xtts" => self.xtts,
            "chatterbox" => self.chatterbox,
            _ => false,
        }
    }
}

/// How a voice will actually be synthesized right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceReady {
    Ready(&'static str),
    /// The voice's own engine is missing; the pipeline substitutes this one.
    Fallback(&'static str),
    Missing,
}

impl VoiceReady {
    pub fn usable(self) -> bool {
        !matches!(self, VoiceReady::Missing)
    }
}

/// How a voice is synthesized right now. `kokoro_tts.py`'s per-voice report is the truth
/// (the same `voice_plan` the pipeline uses); without one, it is estimated from the engine
/// flags with the same stand-ins ([`tts::fallback_candidates`]).
pub fn voice_ready(e: &Engines, v: &VoiceInfo) -> VoiceReady {
    match tts::engine_hint(v.id) {
        "say" if cfg!(target_os = "macos") => return VoiceReady::Ready("macOS"),
        "say" => return VoiceReady::Missing,
        _ => {}
    }
    if let Some(s) = e.voice_status(v.id) {
        return match (s.ready, &s.fallback_voice) {
            (false, _) => VoiceReady::Missing,
            (true, None) => VoiceReady::Ready(tts::engine_label(&s.engine)),
            (true, Some(_)) => VoiceReady::Fallback(tts::engine_label(s.fallback_engine.as_deref().unwrap_or(&s.engine))),
        };
    }
    if e.native(v.id) {
        return VoiceReady::Ready(tts::engine_label(tts::engine_hint(v.id)));
    }
    match tts::fallback_candidates(v.id).into_iter().find(|c| e.native(c)) {
        Some(c) => VoiceReady::Fallback(tts::engine_label(tts::engine_hint(c))),
        None => VoiceReady::Missing,
    }
}

/// The stand-in voice that speaks for `v` now, when its own engine cannot.
pub fn stand_in_voice(e: &Engines, v: &VoiceInfo) -> Option<String> {
    if let Some(s) = e.voice_status(v.id) {
        return s.fallback_voice.clone().filter(|_| s.ready);
    }
    if e.native(v.id) {
        return None;
    }
    tts::fallback_candidates(v.id).into_iter().find(|c| e.native(c)).map(str::to_string)
}

/// Voices offered on this platform (macOS system voices only on macOS).
pub fn voices(lang: &str) -> Vec<&'static VoiceInfo> {
    tts::voice_catalog()
        .iter()
        .filter(|v| v.lang == lang && (cfg!(target_os = "macos") || tts::engine_hint(v.id) != "say"))
        .collect()
}

/// Where page text comes from when a chapter has no script yet.
#[derive(Clone, Debug, PartialEq)]
pub enum OcrProvider {
    Gemini,
    Ollama { model: String },
    /// PaddleOCR / manga-ocr installed in the worker's Python.
    Local,
    Unavailable { ollama_running: bool },
}

impl OcrProvider {
    pub fn ready(&self) -> bool {
        !matches!(self, OcrProvider::Unavailable { .. })
    }
}

#[derive(Clone, Debug)]
pub struct Env {
    pub python: Result<PythonInterpreter, String>,
    pub scripts: Result<PythonLayout, String>,
    pub ffmpeg: bool,
    pub modules: HashMap<String, bool>,
    pub engines: Engines,
    pub tts_error: Option<String>,
    pub ocr: OcrProvider,
    /// Provider settings for jobs, with the Ollama vision model resolved.
    pub providers: ProviderConfig,
    /// `pipeline.ocr_engine` from config.json.
    pub ocr_engine: String,
}

impl Env {
    /// Python and the scripts are there, so jobs can be started at all.
    pub fn runnable(&self) -> bool {
        self.python.is_ok() && self.scripts.is_ok()
    }

    /// Command prefix for the worker interpreter, quoted when it has spaces.
    pub fn python_cmd(&self) -> String {
        match &self.python {
            Ok(p) => command_prefix(&p.program, &p.prefix_args),
            Err(_) => default_python().into(),
        }
    }

    /// Command prefix for the interpreter the voice scripts use.
    pub fn voice_python_cmd(&self) -> String {
        match &self.engines.python {
            Some(p) if !p.is_empty() => quote(p),
            _ => self.python_cmd(),
        }
    }

    pub fn scripts_dir(&self) -> Option<&Path> {
        self.scripts.as_ref().ok().map(|l| l.dir.as_path())
    }

    fn has(&self, module: &str) -> bool {
        self.modules.get(module).copied().unwrap_or(false)
    }

    /// pip command for the voice engine packages that are missing, if any.
    pub fn tts_packages_cmd(&self) -> Option<String> {
        let mut pkgs: Vec<&str> = vec![];
        if !(self.engines.kokoro_deps || self.has("kokoro_onnx") && self.has("soundfile")) {
            pkgs.extend(KOKORO_PACKAGES);
        }
        if !(self.engines.piper_deps || self.has("piper")) {
            pkgs.extend(PIPER_PACKAGES);
        }
        (!pkgs.is_empty()).then(|| pip_install(&self.voice_python_cmd(), &pkgs))
    }

    /// Command that downloads the Kokoro + Piper voice models, when none are present.
    pub fn voice_models_cmd(&self) -> Option<String> {
        let models = self.engines.kokoro_models || self.engines.piper_hi || self.engines.piper_en;
        match (models, self.scripts_dir()) {
            (false, Some(dir)) => Some(download_models_cmd(&self.voice_python_cmd(), dir)),
            _ => None,
        }
    }
}

/// `python` on Windows, `python3` elsewhere.
pub fn default_python() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

fn quote(s: &str) -> String {
    if s.contains(' ') && !s.starts_with('"') {
        format!("\"{s}\"")
    } else {
        s.to_string()
    }
}

fn command_prefix(program: &str, args: &[String]) -> String {
    std::iter::once(quote(program)).chain(args.iter().cloned()).collect::<Vec<_>>().join(" ")
}

/// `<python> -m pip install "<pkg>" ...` (each spec quoted so `>=`, `<` and `[` survive any shell).
pub fn pip_install(python: &str, packages: &[&str]) -> String {
    let specs: Vec<String> = packages.iter().map(|p| format!("\"{p}\"")).collect();
    format!("{python} -m pip install {}", specs.join(" "))
}

/// Downloads every Kokoro and Piper model file through the bundled script's own helper.
pub fn download_models_cmd(python: &str, scripts: &Path) -> String {
    format!(
        "{python} -c \"import sys; sys.path.insert(0, r'{}'); import kokoro_tts; kokoro_tts.download_all_voice_assets()\"",
        scripts.display()
    )
}

/// Local OCR packages from `python/requirements.txt`.
pub fn local_ocr_cmd(python: &str, scripts: &Path) -> String {
    format!("{python} -m pip install -r \"{}\"", scripts.join("requirements.txt").display())
}

pub fn python_install_cmd() -> &'static str {
    if cfg!(windows) {
        "winget install -e --id Python.Python.3.12"
    } else if cfg!(target_os = "macos") {
        "brew install python@3.12"
    } else {
        "sudo apt install python3 python3-pip"
    }
}

pub fn ffmpeg_install_cmd() -> &'static str {
    if cfg!(windows) {
        "winget install -e --id Gyan.FFmpeg"
    } else if cfg!(target_os = "macos") {
        "brew install ffmpeg"
    } else {
        "sudo apt install ffmpeg"
    }
}

pub const OLLAMA_PULL_CMD: &str = "ollama pull minicpm-v:8b";

/// The vision model to use: the configured one if installed, else the best installed
/// vision model (`autoDetectVisionModel`). `None` when no vision model is installed.
pub fn pick_vision_model(models: &[String], configured: &str) -> Option<String> {
    let base = configured.split(':').next().unwrap_or(configured);
    if !base.is_empty() && models.iter().any(|m| m.starts_with(base)) {
        return Some(configured.to_string());
    }
    let vision: Vec<&String> = models.iter().filter(|m| VISION_KEYWORDS.iter().any(|k| m.to_lowercase().contains(k))).collect();
    for pref in VISION_PREFERENCE {
        if let Some(m) = vision.iter().find(|m| m.to_lowercase().contains(pref)) {
            return Some((*m).clone());
        }
    }
    vision.first().map(|m| (*m).clone())
}

/// Installed Ollama models, or `None` when Ollama is not reachable (plain-HTTP URLs only).
fn ollama_models(url: &str) -> Option<Vec<String>> {
    let rest = url.trim().strip_prefix("http://")?;
    let host = rest.split('/').next().filter(|h| !h.is_empty())?;
    let addr = if host.contains(':') { host.to_string() } else { format!("{host}:80") };
    let sock = addr.to_socket_addrs().ok()?.next()?;
    let mut stream = TcpStream::connect_timeout(&sock, Duration::from_millis(800)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    // HTTP/1.0 keeps the response unchunked.
    write!(stream, "GET /api/tags HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n").ok()?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw).ok()?;
    let body = raw.split_once("\r\n\r\n")?.1;
    let v: Value = serde_json::from_str(body).ok()?;
    Some(v.get("models")?.as_array()?.iter().filter_map(|m| m.get("name")?.as_str().map(str::to_string)).collect())
}

fn probe_modules(py: &PythonInterpreter) -> HashMap<String, bool> {
    let list = MODULES.iter().map(|m| format!("'{m}'")).collect::<Vec<_>>().join(",");
    let code = format!("import importlib.util as u, json; print(json.dumps({{m: u.find_spec(m) is not None for m in [{list}]}}))");
    let out = quiet_command(&py.program).args(&py.prefix_args).arg("-c").arg(code).output();
    out.ok()
        .filter(|o| o.status.success())
        .and_then(|o| serde_json::from_slice::<HashMap<String, bool>>(String::from_utf8_lossy(&o.stdout).trim().as_bytes()).ok())
        .unwrap_or_default()
}

fn ffmpeg_available() -> bool {
    quiet_command("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn pipeline_setting(key: &str) -> Option<Value> {
    let text = std::fs::read_to_string(config_path()).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("pipeline")?.get(key).cloned()
}

/// Saved defaults from `config.json` -> `pipeline` (shared with the Electron app).
pub fn saved_voice() -> Option<String> {
    pipeline_setting("tts_voice").and_then(|v| v.as_str().map(str::to_string))
}

pub fn saved_speed() -> Option<f64> {
    pipeline_setting("tts_speed").and_then(|v| v.as_f64())
}

/// A finished check, shared by every view that shows voice readiness.
#[derive(Clone, Debug)]
pub struct Checked {
    /// Increases with every finished check.
    pub seq: u64,
    /// [`voices::version`] when the check started.
    pub version: u64,
    pub env: Env,
}

static LATEST: Mutex<Option<Checked>> = Mutex::new(None);
static SEQ: AtomicU64 = AtomicU64::new(0);
/// Versions of the checks running now.
static RUNNING: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// The newest finished check.
pub fn latest() -> Option<Checked> {
    LATEST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// A check that started at the current voices version is running.
pub fn checking_current() -> bool {
    let v = voices::version();
    RUNNING.lock().unwrap_or_else(|e| e.into_inner()).contains(&v)
}

/// Marks voice readiness out of date: Studio and Settings → Audio check again.
pub fn invalidate() {
    voices::invalidate();
}

/// Keeps a view's environment in step with [`latest`] and [`voices::version`].
#[derive(Clone, Debug, Default)]
pub struct EnvSync {
    seen_seq: u64,
    env_version: u64,
    tried_version: u64,
}

impl EnvSync {
    /// A check finished since this view last looked: the env to show now.
    pub fn take_newer(&mut self) -> Option<Env> {
        let c = latest()?;
        if c.seq <= self.seen_seq {
            return None;
        }
        self.seen_seq = c.seq;
        self.env_version = c.version;
        Some(c.env)
    }

    /// The shown env predates the last voice change and no check for it has started.
    pub fn should_check(&self, checking: bool) -> bool {
        let v = voices::version();
        self.env_version != v && self.tried_version != v && !checking && !checking_current()
    }

    /// Call when this view starts a check.
    pub fn started(&mut self) {
        self.tried_version = voices::version();
    }

    /// Something changed that this view has not started (or adopted) a check for yet.
    pub fn behind(&self) -> bool {
        let v = voices::version();
        self.tried_version != v && self.env_version != v
    }
}

struct Running(u64);

impl Drop for Running {
    fn drop(&mut self) {
        let mut r = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = r.iter().position(|v| *v == self.0) {
            r.remove(i);
        }
    }
}

/// Runs every check and shares the result ([`latest`]). Spawns a few short Python
/// processes; call from the background runtime.
pub async fn check() -> Env {
    let version = voices::version();
    RUNNING.lock().unwrap_or_else(|e| e.into_inner()).push(version);
    let _running = Running(version);
    let env = check_now().await;
    let mut latest = LATEST.lock().unwrap_or_else(|e| e.into_inner());
    if latest.as_ref().is_none_or(|c| c.version <= version) {
        let seq = SEQ.fetch_add(1, Ordering::SeqCst) + 1;
        *latest = Some(Checked { seq, version, env: env.clone() });
    }
    env
}

async fn check_now() -> Env {
    let base = tokio::task::spawn_blocking(|| {
        let python = discover_python(configured_python(&config_path()).as_deref()).map_err(|e| e.to_string());
        let scripts = PythonLayout::resolve(None).map_err(|e| e.to_string());
        let modules = python.as_ref().map(probe_modules).unwrap_or_default();
        (python, scripts, ffmpeg_available(), modules)
    })
    .await;
    let (python, scripts, ffmpeg, modules) = match base {
        Ok(b) => b,
        Err(e) => (Err(e.to_string()), Err("check failed".into()), false, HashMap::new()),
    };

    let (mut engines, tts_error) = match (&python, &scripts) {
        (Ok(py), Ok(layout)) => match tts::kokoro_status(py, layout).await {
            Ok(v) => (Engines::from_status(&v), None),
            Err(e) => (Engines::default(), Some(e.to_string())),
        },
        _ => (Engines::default(), None),
    };
    if engines.voices.is_empty() {
        // No per-voice report: which Piper voices are downloaded, from the files.
        engines.piper_voices = tts::voice_catalog()
            .iter()
            .filter(|v| tts::engine_hint(v.id) == "piper" && voices::pack_for_voice(v.id).is_some_and(voices::installed))
            .map(|v| v.id.to_string())
            .collect();
    }
    let has = |m: &str| modules.get(m).copied().unwrap_or(false);
    // `activePython` may differ from the worker interpreter; only trust our probe when it is the same one.
    let same_python = match (&python, &engines.python) {
        (Ok(py), Some(active)) => paths_match(&py.program, active),
        _ => engines.python.is_none(),
    };
    engines.kokoro_deps = engines.kokoro_deps || engines.kokoro || same_python && has("kokoro_onnx") && has("soundfile");
    engines.piper_deps = engines.piper_deps || same_python && has("piper");
    engines.refresh_piper_flags();

    let mut providers = ProviderConfig::load(&config_path());
    let ocr_engine = pipeline_setting("ocr_engine").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "vision".into());
    let url = providers.ollama_url.clone();
    let tags = tokio::task::spawn_blocking(move || ollama_models(&url)).await.ok().flatten();
    let ocr = if !providers.gemini_api_key.trim().is_empty() {
        OcrProvider::Gemini
    } else if let Some(model) = tags.as_deref().and_then(|t| pick_vision_model(t, &providers.ollama_vision_model)) {
        providers.ollama_vision_model = model.clone();
        OcrProvider::Ollama { model }
    } else if has("paddleocr") || has("manga_ocr") {
        OcrProvider::Local
    } else {
        OcrProvider::Unavailable { ollama_running: tags.is_some() }
    };

    Env { python, scripts, ffmpeg, modules, engines, tts_error, ocr, providers, ocr_engine }
}

/// True when `program` (a command name or path) and `path` name the same interpreter.
fn paths_match(program: &str, path: &str) -> bool {
    let p = PathBuf::from(program);
    if p.is_absolute() {
        return p == PathBuf::from(path);
    }
    // A bare command: resolve it the way the shell would.
    let lookup = if cfg!(windows) { "where" } else { "which" };
    quiet_command(lookup)
        .arg(program)
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).lines().next().map(|l| l.trim().eq_ignore_ascii_case(path.trim())))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn voice(id: &str) -> &'static VoiceInfo {
        tts::find_voice(id).unwrap()
    }

    /// `kokoro_tts.py status` on a PC with the Kokoro pack and engine, the Piper engine and
    /// only the Amy Piper pack (the per-voice part trimmed to the voices under test).
    fn amy_only_status() -> Value {
        json!({
            "engines": {"kokoro": {"ready": true}, "piper": {"ready": true}},
            "kokoroReady": true, "kokoroDepsReady": true, "piperDepsReady": true,
            "piperHiReady": false, "piperEnReady": true,
            "f5Ready": false, "parlerReady": false, "xttsReady": false,
            "activePython": "C:\\Py\\python.exe",
            "voices": {
                "en_piper_f": {"ready": true, "engine": "piper", "fallbackVoice": null, "fallbackEngine": null, "missing": null},
                "en_piper_m": {"ready": false, "engine": "none", "fallbackVoice": null, "fallbackEngine": null, "missing": "model",
                               "message": "Voice Lessac is not downloaded."},
                "en_piper_m2": {"ready": false, "engine": "none", "fallbackVoice": null, "fallbackEngine": null, "missing": "model"},
                "af_heart": {"ready": true, "engine": "kokoro", "fallbackVoice": null, "fallbackEngine": null, "missing": null},
                "f5_en_storyteller": {"ready": true, "engine": "kokoro", "fallbackVoice": "af_nova", "fallbackEngine": "kokoro", "missing": "engine"},
                "hi_piper_f": {"ready": false, "engine": "none", "fallbackVoice": null, "fallbackEngine": null, "missing": "model"}
            }
        })
    }

    #[test]
    fn status_json_maps_to_engines() {
        let e = Engines::from_status(&amy_only_status());
        assert!(e.kokoro && e.piper_deps && e.piper_en && !e.piper_hi && e.any());
        assert_eq!(e.names(), vec!["Kokoro", "Piper"]);
        assert_eq!(e.python.as_deref(), Some("C:\\Py\\python.exe"));
        assert_eq!(e.piper_voices, vec!["en_piper_f"]);
        let lessac = e.voice_status("en_piper_m").unwrap();
        assert_eq!((lessac.ready, lessac.missing), (false, Some(Missing::Model)));
        assert_eq!(lessac.message.as_deref(), Some("Voice Lessac is not downloaded."));
        assert!(!Engines::from_status(&json!({"installed": false})).any());
        // An older script without the per-voice map: the per-language flags still count.
        let old = Engines::from_status(&json!({"piperDepsReady": true, "piperEnReady": true}));
        assert!(old.piper_en && !old.piper_hi && old.voices.is_empty());
    }

    /// The reported bug: only the Amy pack is downloaded, so Amy is ready and Lessac/Ryan are not.
    #[test]
    fn piper_readiness_is_per_voice() {
        let e = Engines::from_status(&amy_only_status());
        assert_eq!(voice_ready(&e, voice("en_piper_f")), VoiceReady::Ready("Piper"));
        assert_eq!(voice_ready(&e, voice("en_piper_m")), VoiceReady::Missing);
        assert_eq!(voice_ready(&e, voice("en_piper_m2")), VoiceReady::Missing);
        assert_eq!(voice_ready(&e, voice("hi_piper_f")), VoiceReady::Missing);
        assert_eq!(voice_ready(&e, voice("af_heart")), VoiceReady::Ready("Kokoro"));
        assert_eq!(voice_ready(&e, voice("f5_en_storyteller")), VoiceReady::Fallback("Kokoro"));
        assert_eq!(stand_in_voice(&e, voice("f5_en_storyteller")).as_deref(), Some("af_nova"));
        assert_eq!(stand_in_voice(&e, voice("en_piper_f")), None);
    }

    /// Without a per-voice report the same rules are estimated from the engine flags.
    #[test]
    fn voice_readiness_follows_engine_fallbacks() {
        let none = Engines::default();
        assert_eq!(voice_ready(&none, voice("af_heart")), VoiceReady::Missing);

        let kokoro = Engines { kokoro: true, ..Default::default() };
        assert_eq!(voice_ready(&kokoro, voice("af_heart")), VoiceReady::Ready("Kokoro"));
        assert_eq!(voice_ready(&kokoro, voice("xtts_en_male")), VoiceReady::Fallback("Kokoro"));
        assert_eq!(voice_ready(&kokoro, voice("f5_hi_female")), VoiceReady::Fallback("Kokoro"));
        assert_eq!(stand_in_voice(&kokoro, voice("f5_hi_female")).as_deref(), Some("hf_alpha"));
        assert_eq!(voice_ready(&kokoro, voice("en_piper_m")), VoiceReady::Missing);

        let amy = Engines { piper_deps: true, piper_voices: vec!["en_piper_f".into()], ..Default::default() };
        assert_eq!(voice_ready(&amy, voice("en_piper_f")), VoiceReady::Ready("Piper"));
        assert_eq!(voice_ready(&amy, voice("en_piper_m2")), VoiceReady::Missing);
        assert_eq!(voice_ready(&amy, voice("am_adam")), VoiceReady::Fallback("Piper"));
        assert_eq!(voice_ready(&amy, voice("f5_en_storyteller")), VoiceReady::Fallback("Piper"));
        assert_eq!(voice_ready(&amy, voice("hi_piper_f")), VoiceReady::Missing);
        assert_eq!(voice_ready(&amy, voice("f5_hi_female")), VoiceReady::Missing);
        // Files without the engine are not a voice.
        let files_only = Engines { piper_voices: vec!["en_piper_f".into()], ..Default::default() };
        assert_eq!(voice_ready(&files_only, voice("en_piper_f")), VoiceReady::Missing);

        let f5 = Engines { f5: true, ..Default::default() };
        assert_eq!(voice_ready(&f5, voice("f5_en_male")), VoiceReady::Ready("F5-TTS"));

        // Chatterbox voices speak through Kokoro until Chatterbox is set up.
        assert_eq!(voice_ready(&kokoro, voice("cb_narrator_m")), VoiceReady::Fallback("Kokoro"));
        assert_eq!(stand_in_voice(&kokoro, voice("cb_narrator_m")).as_deref(), Some("am_michael"));
        let cb = Engines { chatterbox: true, kokoro: true, ..Default::default() };
        assert_eq!(voice_ready(&cb, voice("cb_narrator_f")), VoiceReady::Ready("Chatterbox"));
        assert_eq!(cb.names()[0], "Chatterbox");
        assert!(Engines::from_status(&json!({"chatterboxReady": true})).chatterbox);
    }

    #[test]
    fn env_sync_adopts_newer_checks_and_rechecks_after_changes() {
        let mut sync = EnvSync::default();
        let env = Env {
            python: Err("x".into()),
            scripts: Err("x".into()),
            ffmpeg: false,
            modules: HashMap::new(),
            engines: Engines::default(),
            tts_error: None,
            ocr: OcrProvider::Gemini,
            providers: ProviderConfig::default(),
            ocr_engine: "vision".into(),
        };
        let v = voices::version();
        *LATEST.lock().unwrap() = Some(Checked { seq: SEQ.fetch_add(1, Ordering::SeqCst) + 1, version: v, env });
        assert!(sync.take_newer().is_some());
        assert!(sync.take_newer().is_none(), "the same check is adopted once");
        invalidate();
        assert!(sync.behind() && sync.should_check(false));
        assert!(!sync.should_check(true));
        sync.started();
        assert!(!sync.behind() && !sync.should_check(false), "one check per change");
    }

    #[test]
    fn system_voices_only_on_macos() {
        let has_say = voices("en").iter().any(|v| v.id == "say-default");
        assert_eq!(has_say, cfg!(target_os = "macos"));
        assert!(voices("hi").iter().all(|v| v.lang == "hi"));
    }

    #[test]
    fn vision_model_selection_matches_bridge() {
        let m = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(pick_vision_model(&m(&["minicpm-v:8b", "llava:7b"]), "minicpm-v:8b").as_deref(), Some("minicpm-v:8b"));
        assert_eq!(pick_vision_model(&m(&["llama3:8b", "moondream:latest", "llava:13b"]), "minicpm-v:8b").as_deref(), Some("llava:13b"));
        assert_eq!(pick_vision_model(&m(&["llama3:8b", "qwen2:7b"]), "minicpm-v:8b"), None);
        assert_eq!(pick_vision_model(&[], "minicpm-v:8b"), None);
    }

    #[test]
    fn install_commands_are_exact() {
        assert_eq!(pip_install("python", &["a>=1", "b[x]"]), "python -m pip install \"a>=1\" \"b[x]\"");
        assert_eq!(command_prefix("C:\\Program Files\\Py\\python.exe", &[]), "\"C:\\Program Files\\Py\\python.exe\"");
        assert_eq!(command_prefix("py", &["-3".into()]), "py -3");
        let dl = download_models_cmd("python", Path::new("D:/app/python"));
        assert!(dl.starts_with("python -c \"import sys; sys.path.insert(0, r'D:/app/python'); import kokoro_tts;"), "{dl}");
        assert!(local_ocr_cmd("python", Path::new("D:/x")).ends_with("requirements.txt\""));
    }

    #[test]
    fn missing_packages_command_lists_only_missing_engines() {
        let mut env = Env {
            python: Err("x".into()),
            scripts: Err("x".into()),
            ffmpeg: false,
            modules: HashMap::new(),
            engines: Engines::default(),
            tts_error: None,
            ocr: OcrProvider::Gemini,
            providers: ProviderConfig::default(),
            ocr_engine: "vision".into(),
        };
        let cmd = env.tts_packages_cmd().unwrap();
        assert!(cmd.contains("kokoro-onnx") && cmd.contains("piper-tts"), "{cmd}");
        env.engines.piper_deps = true;
        let cmd = env.tts_packages_cmd().unwrap();
        assert!(cmd.contains("kokoro-onnx") && !cmd.contains("piper-tts"), "{cmd}");
        env.engines.kokoro_deps = true;
        assert_eq!(env.tts_packages_cmd(), None);
    }

    #[test]
    fn ollama_probe_handles_unreachable_and_https() {
        assert_eq!(ollama_models("https://example.com"), None);
        // Port 9 (discard) on loopback is closed on any normal machine.
        assert_eq!(ollama_models("http://127.0.0.1:9"), None);
    }
}
