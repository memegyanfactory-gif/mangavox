//! The optional **layered motion pack**: what lets the renderer animate panels in 2.5D (the
//! Look's "Layered motion" option). It is its own Python with PyTorch (CUDA) and three model
//! checkpoints (SAM2.1 small, Depth Anything V2 small, LaMa), about 3.5 GB to download and
//! 5 GB on disk, all in `<data>/models/vfx/` (so Settings → Storage → Delete models takes it).
//!
//! `python/layered_motion.py install` does the work (venv with uv, PyTorch from the PyTorch
//! wheel index, pinned Hugging Face revisions checked by size and SHA-256, then a load check on
//! the GPU) and writes `mangavox-vfx.json` last; the pack counts as installed only with that
//! marker. One install job at a time; its progress lives here so every view shows the same job.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde_json::Value;

use crate::bridge::{run_worker_logged, CancelToken, WorkerSpec};
use crate::error::StudioError;
use crate::python::{self, PythonLayout};

/// About what Install downloads: PyTorch with CUDA (~3 GB) and the three models (~490 MB).
pub const DOWNLOAD_BYTES: u64 = 3_000_000_000 + WEIGHT_BYTES;
/// About what the pack takes on disk once installed.
pub const DISK_BYTES: u64 = 4_600_000_000 + WEIGHT_BYTES;
const WEIGHT_BYTES: u64 = 184_305_280 + 99_173_660 + 205_803_670;
/// The install marker `layered_motion.py install` writes once everything loaded.
pub const MARKER: &str = "mangavox-vfx.json";

/// `<root>/models/vfx`.
pub fn pack_dir_in(root: &Path) -> PathBuf {
    root.join("models").join("vfx")
}

/// The pack folder in the app's data folder.
pub fn pack_dir() -> PathBuf {
    pack_dir_in(&python::mangavox_home())
}

/// The pack's interpreter inside `dir` (`venv/Scripts/python.exe` on Windows).
pub fn venv_python_in(dir: &Path) -> PathBuf {
    let venv = dir.join("venv");
    if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python3")
    }
}

/// The install marker of the pack in `dir`, when it says the install finished.
pub fn marker_in(dir: &Path) -> Option<Value> {
    let v: Value = serde_json::from_slice(&std::fs::read(dir.join(MARKER)).ok()?).ok()?;
    (v.get("ok").and_then(Value::as_bool) == Some(true)).then_some(v)
}

/// The pack's Python when the pack in `root`'s data folder is installed (interpreter + marker).
pub fn python_in(root: &Path) -> Option<PathBuf> {
    let dir = pack_dir_in(root);
    let py = venv_python_in(&dir);
    (py.is_file() && marker_in(&dir).is_some()).then_some(py)
}

/// The installed pack's Python (what the renderer gets as `video_config["vfx_python"]`).
pub fn python() -> Option<PathBuf> {
    python_in(&python::mangavox_home())
}

pub fn installed() -> bool {
    python().is_some()
}

/// The graphics card the pack was installed for (from its marker).
pub fn gpu() -> Option<String> {
    marker_in(&pack_dir())?.get("gpu").and_then(Value::as_str).map(str::to_string)
}

/// [`installed`] and [`gpu`] for views that ask on every draw: read from disk once per
/// install / remove ([`changes`]), not every frame.
pub fn cached() -> (bool, Option<String>) {
    static CACHE: Mutex<Option<(u64, bool, Option<String>)>> = Mutex::new(None);
    let now = changes();
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    match g.as_ref() {
        Some((seen, on, gpu)) if *seen == now => (*on, gpu.clone()),
        _ => {
            let (on, gpu) = (installed(), gpu());
            *g = Some((now, on, gpu.clone()));
            (on, gpu)
        }
    }
}

/// [`installed`], cached like [`cached`].
pub fn installed_cached() -> bool {
    cached().0
}

fn dir_size(path: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(path) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// Size of the pack on disk, measured once in the background (about 30 000 files): `None`
/// until known. [`changed`] measures again.
pub fn size_on_disk() -> Option<u64> {
    let mut g = SIZE.lock().unwrap_or_else(|e| e.into_inner());
    match *g {
        Size::Known(v, seen) if seen == CHANGES.load(Ordering::SeqCst) => Some(v),
        Size::Measuring => None,
        _ => {
            *g = Size::Measuring;
            let seen = CHANGES.load(Ordering::SeqCst);
            std::thread::spawn(move || {
                let v = dir_size(&pack_dir());
                *SIZE.lock().unwrap_or_else(|e| e.into_inner()) = Size::Known(v, seen);
            });
            None
        }
    }
}

enum Size {
    Unknown,
    Measuring,
    Known(u64, u64),
}

static SIZE: Mutex<Size> = Mutex::new(Size::Unknown);
/// Bumped when the pack is installed or removed, so views read [`installed`] again.
static CHANGES: AtomicU64 = AtomicU64::new(0);

pub fn changes() -> u64 {
    CHANGES.load(Ordering::SeqCst)
}

fn changed() {
    CHANGES.fetch_add(1, Ordering::SeqCst);
}

/// The install job's state.
#[derive(Clone, Debug, PartialEq)]
pub enum PackState {
    Running(PackProgress),
    Removing,
    /// Install failed (the message).
    Failed(String),
    /// Remove failed (the message): the pack may be partly deleted.
    RemoveFailed(String),
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct PackProgress {
    /// 1-based step of `total` (Python, PyTorch, libraries, models, load check).
    pub step: u64,
    pub total: u64,
    pub message: String,
    /// Within the step (a download), 0..100.
    pub percent: Option<f64>,
}

impl PackProgress {
    /// Overall 0..1: the steps done plus the share of the current one.
    pub fn fraction(&self) -> f32 {
        let total = self.total.max(1) as f32;
        let within = self.percent.map(|p| (p / 100.) as f32).unwrap_or(0.);
        ((self.step.saturating_sub(1) as f32 + within) / total).clamp(0., 1.)
    }

    /// "Step 2 of 5 · <message>".
    pub fn summary(&self) -> String {
        if self.total == 0 {
            return self.message.clone();
        }
        format!("Step {} of {} · {}", self.step.max(1), self.total, self.message)
    }
}

struct Job {
    id: u64,
    state: PackState,
    cancel: CancelToken,
    /// Stop was asked: the job stays (busy) until its process tree is gone and it finished.
    stopping: bool,
}

static JOB: Mutex<Option<Job>> = Mutex::new(None);
static NEXT: AtomicU64 = AtomicU64::new(1);

pub fn state() -> Option<PackState> {
    JOB.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|j| j.state.clone())
}

pub fn busy() -> bool {
    matches!(state(), Some(PackState::Running(_) | PackState::Removing))
}

/// Stops the install (files already in place stay; Install resumes the downloads). The job
/// shows "Stopping…" and stays busy until the installer's process tree is gone, so a new
/// Install cannot start a second installer beside it.
pub fn cancel() {
    let mut g = JOB.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(j) = g.as_mut().filter(|j| matches!(j.state, PackState::Running(_))) {
        j.stopping = true;
        if let PackState::Running(p) = &mut j.state {
            p.message = "Stopping…".into();
            p.percent = None;
        }
        j.cancel.cancel();
    }
}

/// Forgets a failed job so the normal Install shows again.
pub fn clear_error() {
    let mut g = JOB.lock().unwrap_or_else(|e| e.into_inner());
    if matches!(g.as_ref(), Some(j) if matches!(j.state, PackState::Failed(_) | PackState::RemoveFailed(_))) {
        *g = None;
    }
}

fn start(state: PackState) -> Option<(u64, CancelToken)> {
    let mut g = JOB.lock().unwrap_or_else(|e| e.into_inner());
    if matches!(g.as_ref(), Some(j) if matches!(j.state, PackState::Running(_) | PackState::Removing)) {
        return None;
    }
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let cancel = CancelToken::new();
    *g = Some(Job { id, state, cancel: cancel.clone(), stopping: false });
    Some((id, cancel))
}

fn set(id: u64, state: PackState) {
    if let Some(j) = JOB.lock().unwrap_or_else(|e| e.into_inner()).as_mut().filter(|j| j.id == id && !j.stopping) {
        j.state = state;
    }
}

/// Ends job `id`; true when it had been stopped.
fn finish(id: u64, res: &Result<(), StudioError>) -> bool {
    let mut g = JOB.lock().unwrap_or_else(|e| e.into_inner());
    let mut stopped = false;
    if let Some(j) = g.as_mut().filter(|j| j.id == id) {
        stopped = j.stopping;
        match res {
            Ok(()) | Err(StudioError::Cancelled) => *g = None,
            // A stopped install that ended with an error (killed mid-step) is a stop too.
            Err(_) if j.stopping => *g = None,
            Err(e) => {
                j.state = if matches!(j.state, PackState::Removing) { PackState::RemoveFailed(e.to_string()) } else { PackState::Failed(e.to_string()) };
            }
        }
    }
    changed();
    stopped
}

/// Installs the pack, publishing progress for [`state`]. Returns at once when a job runs
/// already. Run it on the tokio runtime.
pub async fn install() -> Result<(), StudioError> {
    let Some((id, cancel)) = start(PackState::Running(PackProgress { step: 0, total: 5, message: "Starting…".into(), percent: None })) else {
        return Ok(());
    };
    let res = run_install(id, &cancel).await;
    if finish(id, &res) && res.is_err() {
        return Err(StudioError::Cancelled);
    }
    res
}

async fn run_install(id: u64, cancel: &CancelToken) -> Result<(), StudioError> {
    let (py, layout) = tokio::task::spawn_blocking(|| (python::voice_python(), PythonLayout::resolve(None)))
        .await
        .map_err(|e| StudioError::Preview(format!("setup task failed: {e}")))?;
    let spec = WorkerSpec { interpreter: py?, script: layout?.script("layered_motion.py")?, args: vec!["install".into()], cwd: python::mangavox_home(), strict_errors: true };
    let mut last = PackProgress { step: 0, total: 5, message: String::new(), percent: None };
    run_worker_logged(
        &spec,
        cancel,
        |m| {
            if m.kind != "progress" {
                return;
            }
            if let (Some(c), Some(t)) = (m.current, m.total) {
                if c != last.step {
                    last.percent = None;
                }
                last.step = c;
                last.total = t.max(1);
            }
            if let Some(p) = m.percent {
                last.percent = Some(p);
            }
            if let Some(text) = m.message.as_deref() {
                last.message = text.to_string();
            }
            set(id, PackState::Running(last.clone()));
        },
        |_| {},
    )
    .await
    .map(|_| ())?;
    if installed() {
        Ok(())
    } else {
        Err(StudioError::Preview("The layered motion pack did not finish installing.".into()))
    }
}

/// Deletes `<data>/models/vfx`.
pub async fn remove() -> Result<(), StudioError> {
    let Some((id, _)) = start(PackState::Removing) else {
        return Ok(());
    };
    let dir = pack_dir();
    let res = tokio::task::spawn_blocking(move || remove_in(&dir))
        .await
        .map_err(|e| StudioError::Preview(format!("remove task failed: {e}")))
        .and_then(|r| r.map_err(StudioError::from));
    let _ = finish(id, &res);
    res
}

/// Removes the pack folder `dir` (a missing folder is fine).
pub fn remove_in(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_needs_the_interpreter_and_the_marker() {
        let root = std::env::temp_dir().join(format!("mv-vfx-pack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        assert!(python_in(&root).is_none());
        let dir = pack_dir_in(&root);
        let py = venv_python_in(&dir);
        std::fs::create_dir_all(py.parent().unwrap()).unwrap();
        std::fs::write(&py, b"").unwrap();
        assert!(python_in(&root).is_none(), "an interpreter alone is a half-finished install");
        std::fs::write(dir.join(MARKER), br#"{"ok": false}"#).unwrap();
        assert!(python_in(&root).is_none());
        std::fs::write(dir.join(MARKER), br#"{"ok": true, "gpu": "RTX"}"#).unwrap();
        assert_eq!(python_in(&root), Some(py));
        remove_in(&dir).unwrap();
        assert!(!dir.exists());
        remove_in(&dir).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stop_keeps_the_job_until_it_finished_and_remove_failures_are_their_own() {
        // One test owns the global job slot (the tests of this module run in parallel).
        let (id, token) = start(PackState::Running(PackProgress { step: 2, total: 5, message: "x".into(), percent: Some(10.) })).unwrap();
        cancel();
        assert!(token.is_cancelled());
        assert!(busy(), "still busy until the installer's tree is gone");
        assert!(start(PackState::Removing).is_none(), "no second job while stopping");
        set(id, PackState::Running(PackProgress { step: 3, total: 5, message: "late progress".into(), percent: None }));
        assert!(matches!(state(), Some(PackState::Running(p)) if p.message == "Stopping…"));
        assert!(finish(id, &Err(StudioError::Preview("killed".into()))));
        assert_eq!(state(), None, "a stopped install is not reported as failed");

        let (id, _) = start(PackState::Removing).unwrap();
        assert!(!finish(id, &Err(StudioError::Preview("in use".into()))));
        assert!(matches!(state(), Some(PackState::RemoveFailed(_))));
        assert!(!busy());
        clear_error();
        assert_eq!(state(), None);
    }

    #[test]
    fn progress_spans_the_steps() {
        let p = |step, percent| PackProgress { step, total: 5, message: "x".into(), percent };
        assert_eq!(p(1, None).fraction(), 0.);
        assert!((p(4, Some(50.)).fraction() - 0.7).abs() < 1e-6);
        assert_eq!(p(2, None).summary(), "Step 2 of 5 · x");
    }

    #[test]
    fn the_pack_lives_with_the_models() {
        assert!(pack_dir_in(Path::new("D")).ends_with(Path::new("models").join("vfx")));
        assert!(DOWNLOAD_BYTES < DISK_BYTES);
    }
}
