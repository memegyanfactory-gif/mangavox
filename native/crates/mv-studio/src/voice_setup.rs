//! One-click voice setup: everything a voice pack needs to speak, as one job. The steps
//! are the Python runtime (only when this PC has none), the pack's engine, the pack's
//! model files, then a check that its voices load. A job can also install an engine on
//! its own. Progress lives in a global registry like the pack downloads in
//! [`crate::voices`], so Settings → Audio and Studio's Setup sheet show the same job.
//!
//! Each step that changes what is installed calls [`voices::invalidate`], so every view
//! that shows voice readiness checks again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use crate::bridge::CancelToken;
use crate::error::StudioError;
use crate::python::{self, PythonInterpreter, PythonLayout};
use crate::tts;
use crate::voices::{self, DownloadState, VoicePack, PACKS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Runtime,
    Engine,
    Model,
    Verify,
}

impl Step {
    pub const COUNT: u8 = 4;

    pub fn number(self) -> u8 {
        match self {
            Step::Runtime => 1,
            Step::Engine => 2,
            Step::Model => 3,
            Step::Verify => 4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Step::Runtime => "Preparing Python",
            Step::Engine => "Installing the engine",
            Step::Model => "Downloading the voices",
            Step::Verify => "Checking the voices",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SetupProgress {
    pub step: Step,
    /// Latest detail from the step (an installer line, the download size).
    pub detail: String,
    /// Download progress during [`Step::Model`].
    pub download: Option<voices::Progress>,
}

impl SetupProgress {
    /// "Step 2 of 4 · Installing the engine: <detail>".
    pub fn summary(&self) -> String {
        let head = format!("Step {} of {} · {}", self.step.number(), Step::COUNT, self.step.label());
        if self.detail.is_empty() {
            head
        } else {
            format!("{head}: {}", self.detail)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SetupState {
    Running(SetupProgress),
    Failed(String),
}

/// What a job sets up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A voice pack: its engine and its model files.
    Pack(&'static VoicePack),
    /// An engine on its own (`kokoro`, `piper` or `chatterbox`).
    Engine(&'static str),
}

impl Target {
    pub fn pack(id: &str) -> Option<Self> {
        voices::find_pack(id).map(Target::Pack)
    }

    pub fn engine(name: &str) -> Self {
        Target::Engine(match name {
            "kokoro" => "kokoro",
            "chatterbox" => "chatterbox",
            _ => "piper",
        })
    }

    /// Registry key: the pack id, or `engine:<name>`.
    pub fn key(self) -> &'static str {
        match self {
            Target::Pack(p) => p.id,
            Target::Engine("kokoro") => "engine:kokoro",
            Target::Engine("chatterbox") => "engine:chatterbox",
            Target::Engine(_) => "engine:piper",
        }
    }

    /// `kokoro_tts.py install-engine` name.
    pub fn engine_name(self) -> &'static str {
        match self {
            Target::Pack(p) => voices::engine_of(p),
            Target::Engine(e) => e,
        }
    }
}

struct Job {
    id: u64,
    target: Target,
    state: SetupState,
    cancel: CancelToken,
}

static JOBS: Mutex<Option<HashMap<&'static str, Job>>> = Mutex::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// Engine installs run one at a time: two pip runs in one environment would collide.
static ENGINE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn with_jobs<T>(f: impl FnOnce(&mut HashMap<&'static str, Job>) -> T) -> T {
    let mut g = JOBS.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

/// State of the job for `key` (a pack id or `engine:<name>`, see [`Target::key`]). During
/// the model step it carries the live download progress.
pub fn state(key: &str) -> Option<SetupState> {
    let (target, mut state) = with_jobs(|m| m.get(key).map(|j| (j.target, j.state.clone())))?;
    if let (Target::Pack(pack), SetupState::Running(p)) = (target, &mut state) {
        if p.step == Step::Model {
            if let Some(DownloadState::Running(d)) = voices::download_state(pack.id) {
                p.download = Some(d);
            }
        }
    }
    Some(state)
}

pub fn any_running() -> bool {
    with_jobs(|m| m.values().any(|j| matches!(j.state, SetupState::Running(_))))
}

/// Progress of a running job that is preparing Python or installing `engine`, for that
/// engine's row.
pub fn engine_busy(engine: &str) -> Option<SetupProgress> {
    with_jobs(|m| {
        m.values().find_map(|j| match &j.state {
            SetupState::Running(p) if j.target.engine_name() == engine && matches!(p.step, Step::Runtime | Step::Engine) => Some(p.clone()),
            _ => None,
        })
    })
}

/// Stops a job (and its download). Files already in place stay.
pub fn cancel(key: &str) {
    let job = with_jobs(|m| m.remove(key));
    if let Some(j) = job {
        j.cancel.cancel();
        if let Target::Pack(p) = j.target {
            if matches!(&j.state, SetupState::Running(s) if s.step == Step::Model) {
                voices::cancel_download(p.id);
            }
        }
    }
}

/// Forgets a failed job so its card shows the normal action again.
pub fn clear_error(key: &str) {
    with_jobs(|m| {
        if matches!(m.get(key), Some(j) if matches!(j.state, SetupState::Failed(_))) {
            m.remove(key);
        }
    });
}

/// The pack "Set up voices" sets up: one already downloaded (its engine is what is
/// missing), else the recommended one.
pub fn suggested_pack() -> &'static VoicePack {
    PACKS.iter().find(|p| voices::installed(p)).or_else(|| PACKS.iter().find(|p| p.recommended)).unwrap_or(&PACKS[0])
}

/// Rough size of an engine's Python packages, for button captions.
pub fn engine_size_hint(engine: &str) -> &'static str {
    match engine {
        "kokoro" => "about 60 MB",
        // Its own Python 3.11 and PyTorch with CUDA (about 2.5 GB to download).
        "chatterbox" => "about 5 GB",
        _ => "about 40 MB",
    }
}

/// `kokoro_tts.py status` flag that says an engine's packages load.
fn deps_flag(engine: &str) -> &'static str {
    match engine {
        "kokoro" => "kokoroDepsReady",
        "chatterbox" => "chatterboxDepsReady",
        _ => "piperDepsReady",
    }
}

fn set(key: &'static str, id: u64, step: Step, detail: impl Into<String>) {
    let detail = detail.into();
    with_jobs(|m| {
        if let Some(j) = m.get_mut(key).filter(|j| j.id == id) {
            j.state = SetupState::Running(SetupProgress { step, detail, download: None });
        }
    });
}

/// Sets up `target`, publishing progress for [`state`]. Returns at once when the same job
/// is already running. Run it on the tokio runtime.
pub async fn run(target: Target) -> Result<(), StudioError> {
    let key = target.key();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let cancel = CancelToken::new();
    let fresh = with_jobs(|m| {
        if matches!(m.get(key), Some(j) if matches!(j.state, SetupState::Running(_))) {
            return false;
        }
        let state = SetupState::Running(SetupProgress { step: Step::Runtime, detail: "Looking for Python…".into(), download: None });
        m.insert(key, Job { id, target, state, cancel: cancel.clone() });
        true
    });
    if !fresh {
        return Ok(());
    }
    let res = steps(key, id, target, &cancel).await;
    with_jobs(|m| {
        if m.get(key).is_some_and(|j| j.id == id) {
            match &res {
                Ok(()) | Err(StudioError::Cancelled) => {
                    m.remove(key);
                }
                Err(e) => {
                    if let Some(j) = m.get_mut(key) {
                        j.state = SetupState::Failed(e.to_string());
                    }
                }
            }
        }
    });
    voices::invalidate();
    res
}

async fn steps(key: &'static str, id: u64, target: Target, cancel: &CancelToken) -> Result<(), StudioError> {
    let engine = target.engine_name();
    let label = tts::engine_label(engine);

    // 1. Python. kokoro_tts.py builds the voice environment from it.
    let (py, layout) = locate().await?;
    let py = match py {
        Ok(p) => p,
        Err(_) => {
            set(key, id, Step::Runtime, "Installing a private Python 3.12 with uv…");
            install_runtime(cancel).await?;
            blocking(python::voice_python).await?.map_err(|_| StudioError::PythonNotFound(no_python_message()))?
        }
    };
    if cancel.is_cancelled() {
        return Err(StudioError::Cancelled);
    }

    // 2. The engine.
    set(key, id, Step::Engine, "Waiting for another install to finish…");
    let guard = tokio::select! {
        g = ENGINE_LOCK.lock() => g,
        _ = cancel.cancelled() => return Err(StudioError::Cancelled),
    };
    set(key, id, Step::Engine, format!("Installing {label}…"));
    let installed = tts::install_engine(&py, &layout, engine, cancel, |line| set(key, id, Step::Engine, line)).await;
    drop(guard);
    installed.map_err(|e| match e {
        StudioError::Cancelled => StudioError::Cancelled,
        e => StudioError::Preview(format!("The {label} engine could not be installed: {e}")),
    })?;
    voices::invalidate();

    // 3. The model files.
    if let Target::Pack(pack) = target {
        if !voices::installed(pack) {
            set(key, id, Step::Model, format!("{} ({})", pack.name, format_mb(pack.size())));
            voices::clear_error(pack.id);
            download(pack, cancel).await?;
        }
    }

    // 4. The voices load.
    set(key, id, Step::Verify, "Checking that the voices load…");
    verify(&py, &layout, target).await
}

fn format_mb(bytes: u64) -> String {
    format!("{:.0} MB", bytes as f64 / (1024.0 * 1024.0))
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, StudioError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| StudioError::Preview(format!("setup task failed: {e}")))
}

/// The voice interpreter (or why there is none) and the scripts folder.
async fn locate() -> Result<(Result<PythonInterpreter, StudioError>, PythonLayout), StudioError> {
    let (py, layout) = blocking(|| (python::voice_python(), PythonLayout::resolve(None))).await?;
    Ok((py, layout?))
}

async fn download(pack: &'static VoicePack, cancel: &CancelToken) -> Result<(), StudioError> {
    let res = tokio::select! {
        r = voices::run_download(pack.id) => r,
        _ = cancel.cancelled() => {
            voices::cancel_download(pack.id);
            return Err(StudioError::Cancelled);
        }
    };
    res?;
    // `run_download` returns at once when the pack was already downloading: wait for that one.
    while matches!(voices::download_state(pack.id), Some(DownloadState::Running(_))) {
        tokio::select! {
            _ = cancel.cancelled() => return Err(StudioError::Cancelled),
            _ = tokio::time::sleep(Duration::from_millis(400)) => {}
        }
    }
    if voices::installed(pack) {
        return Ok(());
    }
    match voices::download_state(pack.id) {
        Some(DownloadState::Failed(e)) => Err(StudioError::Download(e)),
        // Paused from the pack card.
        _ => Err(StudioError::Cancelled),
    }
}

/// Every voice of the pack must load with its own engine; an engine alone must import.
async fn verify(py: &PythonInterpreter, layout: &PythonLayout, target: Target) -> Result<(), StudioError> {
    match target {
        Target::Pack(pack) => {
            let map = tts::kokoro_voices_ready(py, layout).await?;
            check_pack_voices(pack, &map)
        }
        Target::Engine(engine) => {
            let status = tts::kokoro_status(py, layout).await?;
            if status.get(deps_flag(engine)).and_then(serde_json::Value::as_bool).unwrap_or(false) {
                Ok(())
            } else {
                let python = status.get("activePython").and_then(serde_json::Value::as_str).unwrap_or("the voice environment");
                Err(StudioError::Preview(format!("{} installed but does not load in {python}.", tts::engine_label(engine))))
            }
        }
    }
}

/// Checks the `voices-ready` map for every voice of `pack`.
pub fn check_pack_voices(pack: &VoicePack, map: &serde_json::Value) -> Result<(), StudioError> {
    for v in pack.voice_ids() {
        let s = map.get(v);
        let native = s.is_some_and(|s| s.get("ready").and_then(serde_json::Value::as_bool) == Some(true) && s.get("fallbackVoice").is_none_or(serde_json::Value::is_null));
        if !native {
            let why = s.and_then(|s| s.get("message")).and_then(serde_json::Value::as_str).map(str::to_string).unwrap_or_else(|| {
                let label = tts::find_voice(v).map(|i| i.label).unwrap_or(v);
                format!("{label} still does not load after setup")
            });
            return Err(StudioError::Preview(why));
        }
    }
    Ok(())
}

fn no_python_message() -> String {
    let fix = if cfg!(windows) {
        "winget install -e --id Python.Python.3.12"
    } else if cfg!(target_os = "macos") {
        "brew install python@3.12"
    } else {
        "sudo apt install python3 python3-venv"
    };
    format!("Voices need Python 3.10 or newer. Install it ({fix}), then press Set up again.")
}

/// `uv` from the data folder (`bin/`), else from PATH.
pub fn uv_binary(root: &Path) -> Option<PathBuf> {
    let exe = if cfg!(windows) { "uv.exe" } else { "uv" };
    let local = root.join("bin").join(exe);
    if local.is_file() {
        return Some(local);
    }
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(exe)).find(|p| p.is_file())
}

/// A private Python 3.12 (`<root>/uv-python`) and the voice environment built from it, with uv.
async fn install_runtime(cancel: &CancelToken) -> Result<(), StudioError> {
    let root = python::mangavox_home();
    let Some(uv) = uv_binary(&root) else {
        return Err(StudioError::PythonNotFound(no_python_message()));
    };
    run_tool(&uv, &["python", "install", "3.12"], &root, cancel).await?;
    let venv = root.join("tts-venv").to_string_lossy().into_owned();
    run_tool(&uv, &["venv", "--seed", "--python", "3.12", &venv], &root, cancel).await
}

async fn run_tool(program: &Path, args: &[&str], root: &Path, cancel: &CancelToken) -> Result<(), StudioError> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .env("UV_PYTHON_INSTALL_DIR", root.join("uv-python"))
        .env("UV_CACHE_DIR", root.join("uv-cache"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(python::CREATE_NO_WINDOW);
    let command = format!("{} {}", program.display(), args.join(" "));
    let child = cmd.spawn().map_err(|source| StudioError::Spawn { command: command.clone(), source })?;
    let out = tokio::select! {
        o = child.wait_with_output() => o?,
        _ = cancel.cancelled() => return Err(StudioError::Cancelled),
    };
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let tail: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).rev().take(3).collect();
    Err(StudioError::Worker { message: format!("`{command}` failed: {}", tail.into_iter().rev().collect::<Vec<_>>().join(" ")), stage: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn targets_have_stable_keys() {
        assert_eq!(Target::pack("piper-en-amy").map(Target::key), Some("piper-en-amy"));
        assert_eq!(Target::pack("kokoro-lite").map(Target::engine_name), Some("kokoro"));
        assert_eq!(Target::engine("kokoro").key(), "engine:kokoro");
        assert_eq!(Target::engine("piper").engine_name(), "piper");
        assert_eq!(Target::engine("chatterbox").key(), "engine:chatterbox");
        assert_eq!(Target::pack("chatterbox").map(Target::engine_name), Some("chatterbox"));
        assert_eq!(deps_flag("chatterbox"), "chatterboxDepsReady");
        assert!(Target::pack("nope").is_none());
    }

    #[test]
    fn progress_summary_numbers_the_steps() {
        let p = SetupProgress { step: Step::Engine, detail: "pip".into(), download: None };
        assert_eq!(p.summary(), "Step 2 of 4 · Installing the engine: pip");
    }

    #[test]
    fn chatterbox_pack_needs_its_own_engine_not_a_kokoro_stand_in() {
        let cb = voices::find_pack("chatterbox").unwrap();
        let ok = json!({
            "cb_narrator_f": {"ready": true, "engine": "chatterbox", "fallbackVoice": null},
            "cb_narrator_m": {"ready": true, "engine": "chatterbox", "fallbackVoice": null},
            "cb_storyteller_m": {"ready": true, "engine": "chatterbox", "fallbackVoice": null}
        });
        assert!(check_pack_voices(cb, &ok).is_ok());
        let stand_in = json!({
            "cb_narrator_f": {"ready": true, "engine": "chatterbox", "fallbackVoice": null},
            "cb_narrator_m": {"ready": true, "engine": "chatterbox", "fallbackVoice": null},
            "cb_storyteller_m": {"ready": true, "engine": "kokoro", "fallbackVoice": "am_adam"}
        });
        assert!(check_pack_voices(cb, &stand_in).is_err());
    }

    #[test]
    fn verify_needs_every_voice_of_the_pack_on_its_own_engine() {
        let amy = voices::find_pack("piper-en-amy").unwrap();
        let ok = json!({"en_piper_f": {"ready": true, "engine": "piper", "fallbackVoice": null}});
        assert!(check_pack_voices(amy, &ok).is_ok());
        let missing = json!({"en_piper_f": {"ready": false, "engine": "none", "missing": "engine", "message": "Voice Amy is downloaded but the Piper engine is not installed."}});
        let err = check_pack_voices(amy, &missing).unwrap_err().to_string();
        assert!(err.contains("Piper engine"), "{err}");
        let stand_in = json!({"en_piper_f": {"ready": true, "engine": "kokoro", "fallbackVoice": "af_heart"}});
        assert!(check_pack_voices(amy, &stand_in).is_err());
        assert!(check_pack_voices(amy, &json!({})).is_err());
    }

    #[test]
    fn registry_tracks_failures_and_clears_them() {
        let key = "engine:piper";
        with_jobs(|m| {
            m.insert(key, Job { id: 0, target: Target::engine("piper"), state: SetupState::Failed("x".into()), cancel: CancelToken::new() });
        });
        assert_eq!(state(key), Some(SetupState::Failed("x".into())));
        assert!(engine_busy("piper").is_none());
        clear_error(key);
        assert_eq!(state(key), None);
    }

    /// Delete models (Settings → Storage) takes everything voice setup installed: the packs,
    /// both voice environments, and the Pythons and package cache uv downloaded for them,
    /// without following the junctions uv makes inside `uv-python`.
    #[test]
    fn delete_models_removes_engines_and_uv_dirs() {
        let root = std::env::temp_dir().join(format!("mv-voice-setup-wipe-{}", std::process::id()));
        let outside = std::env::temp_dir().join(format!("mv-voice-setup-wipe-outside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
        let gone = [
            "models/chatterbox/t3_cfg.safetensors",
            "models/kokoro/voices-v1.0.bin",
            "voice-samples/af_heart_kokoro_1.0_v3.wav",
            "tts-venv/Scripts/python.exe",
            "engines/chatterbox/Scripts/python.exe",
            "engines/chatterbox/mangavox-engine.json",
            "uv-python/cpython-3.11.16-windows-x86_64-none/python.exe",
            "uv-cache/wheels-v6/torch.whl",
        ];
        let kept = ["config.json", "studio/prefs.json", "bin/uv.exe", "library/manga/m/c/1.jpg"];
        for f in gone.iter().chain(&kept) {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"x").unwrap();
        }
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("keep.txt"), b"x").unwrap();
        #[cfg(windows)]
        {
            // uv links `cpython-3.11-...` to the full version's folder with a junction.
            let link = root.join("uv-python").join("cpython-3.11-windows-x86_64-none");
            let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(&outside).output().map(|o| o.status.success()).unwrap_or(false);
            assert!(made, "mklink /J");
        }
        mv_core::paths::wipe(&root, mv_core::paths::Part::Models).unwrap();
        for d in ["models", "voice-samples", "tts-venv", "engines", "uv-python", "uv-cache"] {
            assert!(!root.join(d).exists(), "{d} is left");
        }
        for f in kept {
            assert!(root.join(f).is_file(), "{f} was removed");
        }
        assert!(outside.join("keep.txt").is_file(), "the junction was followed");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn uv_is_found_in_the_data_folder_first() {
        let root = std::env::temp_dir().join(format!("mv-voice-setup-uv-{}", std::process::id()));
        let exe = root.join("bin").join(if cfg!(windows) { "uv.exe" } else { "uv" });
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"").unwrap();
        assert_eq!(uv_binary(&root), Some(exe));
        let _ = std::fs::remove_dir_all(&root);
    }
}
