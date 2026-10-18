//! Helpers for tests that drive a fake Python worker written to a temp directory.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::python::{discover_python, PythonInterpreter};

/// The fake worker. `pages_dir` basename `<mode>-<id>` selects behaviour:
/// `ok` emits progress and done, `sleep` blocks until killed, `soft` logs to stderr,
/// emits a recoverable error and exits 0, `fail` emits a fatal error, anything
/// else writes to stderr and exits 4.
const FAKE_WORKER: &str = r#"import json, os, sys, time
job = json.load(open(sys.argv[1], encoding="utf-8"))
mode, _, ident = os.path.basename(job["pages_dir"]).partition("-")

def emit(obj):
    print(json.dumps(obj), flush=True)

emit({"type": "info", "message": "start " + ident})
if mode == "ok":
    for i in range(1, 3):
        emit({"type": "progress", "stage": "fake", "current": i, "total": 2, "percent": i * 50.0})
    emit({"type": "done", "outputs": {"id": ident}})
elif mode == "sleep":
    emit({"type": "progress", "stage": "fake", "current": 1, "total": 2, "percent": 50.0})
    time.sleep(120)
    emit({"type": "done", "outputs": {}})
elif mode == "soft":
    print("soft stderr line", file=sys.stderr, flush=True)
    emit({"type": "error", "stage": "fake", "message": "soft " + ident, "recoverable": True})
elif mode == "fail":
    emit({"type": "error", "stage": "fake", "message": "boom " + ident, "recoverable": False})
    sys.exit(1)
else:
    print("fake worker noise", file=sys.stderr, flush=True)
    sys.exit(4)
"#;

/// Discovered interpreter, or `None` so the test can skip.
pub fn interpreter() -> Option<PythonInterpreter> {
    discover_python(None).ok()
}

/// A fresh, empty directory under the system temp dir.
pub fn unique_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("mv-studio-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Job JSON whose `pages_dir` basename selects the fake worker's behaviour.
pub fn job_json(pages_dir_name: &str) -> String {
    serde_json::json!({ "pages_dir": pages_dir_name, "output_dir": "." }).to_string()
}

/// Writes `fake_worker.py` into `dir`.
pub fn write_fake_worker(dir: &Path) -> PathBuf {
    write_fake_worker_as(dir, "fake_worker.py")
}

/// Writes the fake worker under another name (e.g. `pipeline_worker.py` for queue tests).
pub fn write_fake_worker_as(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, FAKE_WORKER).expect("write fake worker");
    path
}
