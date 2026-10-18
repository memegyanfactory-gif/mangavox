//! Environment check for Studio: Python, the pipeline scripts, FFmpeg, installed
//! voice engines and a text-extraction provider. Every gap comes with the exact
//! command that fixes it; nothing is installed from here.
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

use mv_studio::python::{config_path, configured_python, discover_python, quiet_command, PythonInterpreter, PythonLayout};
use mv_studio::tts::{self, VoiceInfo};
use mv_studio::ProviderConfig;
use serde_json::Value;

/// Python modules probed with `importlib.util.find_spec` (nothing is imported).
const MODULES: &[&str] = &["kokoro_onnx", "soundfile", "numpy", "piper", "onnxruntime", "paddleocr", "manga_ocr", "PIL"];

/// What `kokoro_tts.py install` pip-installs for each engine.
pub const KOKORO_PACKAGES: &[&str] = &["kokoro-onnx>=0.4.0", "soundfile>=0.12.0", "numpy>=2.0.0", "misaki[en]>=0.9.0"];
pub const PIPER_PACKAGES: &[&str] = &["piper-tts>=1.3.0", "onnxruntime>=1.19.0,<2"];

/// Ollama models that can read images (`python-bridge.ts`).
const VISION_KEYWORDS: &[&str] = &["llava", "vision", "bakllava", "moondream", "cogvlm", "minicpm-v", "llama3.2-vision", "gemma3"];
const VISION_PREFERENCE: &[&str] = &["minicpm-v", "llava-llama3", "llava", "gemma3", "moondream", "bakllava", "cogvlm", "llama3.2-vision"];

/// Installed voice engines as reported by `kokoro_tts.py status`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Engines {
    pub kokoro: bool,
    pub kokoro_models: bool,
    pub kokoro_deps: bool,
    pub piper_deps: bool,
    pub piper_hi: bool,
    pub piper_en: bool,
    pub f5: bool,
    pub parler: bool,
    pub xtts: bool,
    /// Interpreter the voice scripts use (`activePython`).
    pub python: Option<String>,
}

impl Engines {
    pub fn any(&self) -> bool {
        self.kokoro || self.piper_hi || self.piper_en || self.f5 || self.parler || self.xtts
    }

    fn piper(&self, lang: &str) -> bool {
        if lang == "hi" {
            self.piper_hi
        } else {
            self.piper_en
        }
    }

    /// Ready engine names, best first.
    pub fn names(&self) -> Vec<&'static str> {
        let mut out = vec![];
        for (on, name) in [(self.f5, "F5-TTS"), (self.parler, "Parler-TTS"), (self.xtts, "XTTS"), (self.kokoro, "Kokoro")] {
            if on {
                out.push(name);
            }
        }
        if self.piper_hi || self.piper_en {
            out.push("Piper");
        }
        out
    }

    /// Parses the JSON printed by `kokoro_tts.py status`.
    pub fn from_status(v: &Value) -> Self {
        let b = |p: &str| v.pointer(p).and_then(Value::as_bool).unwrap_or(false);
        let piper_engine = b("/engines/piper/ready");
        Self {
            kokoro: b("/engines/kokoro/ready"),
            kokoro_models: b("/kokoroReady"),
            kokoro_deps: false,
            piper_deps: piper_engine,
            piper_hi: piper_engine && b("/piperHiReady"),
            piper_en: piper_engine && b("/piperEnReady"),
            f5: b("/f5Ready"),
            parler: b("/parlerReady"),
            xtts: b("/xttsReady"),
            python: v.get("activePython").and_then(Value::as_str).map(str::to_string),
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

/// Mirrors `resolve_engine_for_voice` / `voice_ready` in `kokoro_tts.py`.
pub fn voice_ready(e: &Engines, v: &VoiceInfo) -> VoiceReady {
    let lighter = || {
        if e.kokoro {
            VoiceReady::Fallback("Kokoro")
        } else if e.piper(v.lang) {
            VoiceReady::Fallback("Piper")
        } else {
            VoiceReady::Missing
        }
    };
    match tts::engine_hint(v.id) {
        "say" if cfg!(target_os = "macos") => VoiceReady::Ready("macOS"),
        "say" => VoiceReady::Missing,
        "f5-tts" if e.f5 => VoiceReady::Ready("F5-TTS"),
        "parler-tts" if e.parler => VoiceReady::Ready("Parler-TTS"),
        "xtts" if e.xtts => VoiceReady::Ready("XTTS"),
        "f5-tts" | "parler-tts" | "xtts" => lighter(),
        "piper" if e.piper(v.lang) => VoiceReady::Ready("Piper"),
        "piper" => VoiceReady::Missing,
        _ if e.kokoro => VoiceReady::Ready("Kokoro"),
        _ if e.piper(v.lang) => VoiceReady::Fallback("Piper"),
        _ => VoiceReady::Missing,
    }
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

/// Runs every check. Spawns a few short Python processes; call from the background runtime.
pub async fn check() -> Env {
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
    let has = |m: &str| modules.get(m).copied().unwrap_or(false);
    // `activePython` may differ from the worker interpreter; only trust our probe when it is the same one.
    let same_python = match (&python, &engines.python) {
        (Ok(py), Some(active)) => paths_match(&py.program, active),
        _ => engines.python.is_none(),
    };
    engines.kokoro_deps = engines.kokoro || same_python && has("kokoro_onnx") && has("soundfile");
    engines.piper_deps = engines.piper_deps || same_python && has("piper");

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

    #[test]
    fn status_json_maps_to_engines() {
        let v = json!({
            "engines": {"kokoro": {"ready": true}, "piper": {"ready": true}},
            "kokoroReady": true, "piperHiReady": false, "piperEnReady": true,
            "f5Ready": false, "parlerReady": false, "xttsReady": false,
            "activePython": "C:\\Py\\python.exe"
        });
        let e = Engines::from_status(&v);
        assert!(e.kokoro && e.piper_en && !e.piper_hi && e.any());
        assert_eq!(e.names(), vec!["Kokoro", "Piper"]);
        assert_eq!(e.python.as_deref(), Some("C:\\Py\\python.exe"));
        assert!(!Engines::from_status(&json!({"installed": false})).any());
    }

    #[test]
    fn voice_readiness_follows_engine_fallbacks() {
        let none = Engines::default();
        assert_eq!(voice_ready(&none, voice("af_heart")), VoiceReady::Missing);

        let kokoro = Engines { kokoro: true, ..Default::default() };
        assert_eq!(voice_ready(&kokoro, voice("af_heart")), VoiceReady::Ready("Kokoro"));
        assert_eq!(voice_ready(&kokoro, voice("xtts_en_male")), VoiceReady::Fallback("Kokoro"));
        assert_eq!(voice_ready(&kokoro, voice("f5_hi_female")), VoiceReady::Fallback("Kokoro"));
        assert_eq!(voice_ready(&kokoro, voice("en_piper_m")), VoiceReady::Missing);

        let piper_en = Engines { piper_en: true, ..Default::default() };
        assert_eq!(voice_ready(&piper_en, voice("en_piper_m")), VoiceReady::Ready("Piper"));
        assert_eq!(voice_ready(&piper_en, voice("am_adam")), VoiceReady::Fallback("Piper"));
        assert_eq!(voice_ready(&piper_en, voice("hi_piper_f")), VoiceReady::Missing);

        let f5 = Engines { f5: true, ..Default::default() };
        assert_eq!(voice_ready(&f5, voice("f5_en_male")), VoiceReady::Ready("F5-TTS"));
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
