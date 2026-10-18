//! Spawns Python worker scripts the way `src/main/pipeline/python-bridge.ts` does:
//! `python <script> <args...>` with cwd set to the scripts directory, env plus
//! `PYTHONUNBUFFERED=1`, stdin ignored, and stdout read as JSON lines.

use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::Notify;

use crate::error::StudioError;
use crate::python::PythonInterpreter;

/// Bytes of stderr kept for error reports.
const STDERR_TAIL_BYTES: usize = 4096;
/// Grace period between SIGTERM and SIGKILL on Unix.
#[cfg(unix)]
const KILL_GRACE: Duration = Duration::from_millis(1500);

/// One JSON object printed by a worker on stdout.
///
/// Known fields are copied out; `raw` keeps the whole object for payloads such
/// as `page` results or `token_usage` that have no typed accessor.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkerMessage {
    /// The `type` field: `progress`, `stage_complete`, `done`, `error`, `info`, `page`, ...
    pub kind: String,
    pub stage: Option<String>,
    pub current: Option<u64>,
    pub total: Option<u64>,
    pub percent: Option<f64>,
    pub message: Option<String>,
    pub recoverable: bool,
    pub data: Option<Value>,
    pub outputs: Option<Value>,
    pub raw: Value,
}

impl WorkerMessage {
    pub fn from_value(raw: Value) -> Self {
        let text = |k: &str| raw.get(k).and_then(Value::as_str).map(str::to_string);
        let count = |k: &str| {
            raw.get(k)
                .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))
        };
        Self {
            kind: text("type").unwrap_or_default(),
            stage: text("stage"),
            current: count("current"),
            total: count("total"),
            percent: raw.get("percent").and_then(Value::as_f64),
            message: text("message"),
            recoverable: raw
                .get("recoverable")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            data: raw.get("data").cloned(),
            outputs: raw.get("outputs").cloned(),
            raw,
        }
    }

    pub fn is_done(&self) -> bool {
        self.kind == "done"
    }

    pub fn is_error(&self) -> bool {
        self.kind == "error"
    }
}

/// Result of one line of worker stdout.
#[derive(Debug, Clone, PartialEq)]
pub enum ParsedLine {
    Message(WorkerMessage),
    /// Non-JSON output (library logging, tracebacks). Forwarded to the log, not the protocol.
    Unparsed(String),
}

/// Splits a byte stream into JSON lines. Partial lines are buffered across
/// chunks (including UTF-8 sequences split between chunks); blank lines are skipped.
#[derive(Debug, Default)]
pub struct JsonLineParser {
    buf: Vec<u8>,
}

impl JsonLineParser {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<ParsedLine> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            if let Some(parsed) = parse_line(&line[..line.len() - 1]) {
                out.push(parsed);
            }
        }
        out
    }

    /// Flushes a final line that had no trailing newline, at end of stream.
    pub fn finish(&mut self) -> Option<ParsedLine> {
        let rest = std::mem::take(&mut self.buf);
        parse_line(&rest)
    }
}

fn parse_line(bytes: &[u8]) -> Option<ParsedLine> {
    let text = String::from_utf8_lossy(bytes);
    let line = text.trim();
    if line.is_empty() {
        return None;
    }
    match serde_json::from_str::<Value>(line) {
        Ok(value @ Value::Object(_)) => Some(ParsedLine::Message(WorkerMessage::from_value(value))),
        _ => Some(ParsedLine::Unparsed(line.to_string())),
    }
}

/// Cooperative cancellation shared between the caller and a running worker.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    inner: Arc<CancelInner>,
}

#[derive(Debug, Default)]
struct CancelInner {
    cancelled: AtomicBool,
    notify: Notify,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// Resolves once `cancel` has been called (immediately if it already was).
    pub async fn cancelled(&self) {
        loop {
            // Create the waiter before checking the flag so a cancel in between is not lost.
            let notified = self.inner.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

/// A single worker invocation.
#[derive(Debug, Clone)]
pub struct WorkerSpec {
    pub interpreter: PythonInterpreter,
    /// Absolute path of the script to run.
    pub script: PathBuf,
    /// Arguments placed after the script, usually the job JSON path.
    pub args: Vec<String>,
    /// Working directory (the scripts directory in the app).
    pub cwd: PathBuf,
    /// When true any `error` message aborts the run, even a recoverable one.
    /// The text workers (`vision_extract`, `text_enhance`, `scene_narrator`) use
    /// this; the pipeline worker only treats non-recoverable errors as fatal.
    pub strict_errors: bool,
}

/// Successful worker run.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkerReport {
    pub exit_code: Option<i32>,
    /// `outputs` of the `done` message, when one was received.
    pub outputs: Option<Value>,
}

/// Builds the command for `python <prefix> <script> <args...>` with the bridge's env and stdio.
pub fn build_command(
    interpreter: &PythonInterpreter,
    script: &Path,
    args: &[String],
    cwd: &Path,
) -> Command {
    let mut cmd = Command::new(&interpreter.program);
    cmd.args(&interpreter.prefix_args)
        .arg(script)
        .args(args)
        .current_dir(cwd)
        .env("PYTHONUNBUFFERED", "1")
        // Workers open job files and print paths without an explicit encoding;
        // on Windows the default would be the ANSI code page.
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        // No console window flashing up for every worker in the GUI app.
        cmd.creation_flags(crate::python::CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        // Own process group, so cancel can signal the whole tree.
        cmd.process_group(0);
    }
    cmd
}

fn map_spawn_error(
    interpreter: &PythonInterpreter,
    script: &Path,
    source: std::io::Error,
) -> StudioError {
    StudioError::Spawn {
        command: format!("{} {}", interpreter.command_line(), script.display()),
        source,
    }
}

/// Runs a worker, forwarding each protocol message to `on_message`, and resolves
/// when the worker reports `done`, fails with an error message, or exits.
///
/// Mirrors `_runPipelineProcess` in python-bridge.ts: the first `done` wins
/// (exit code is then ignored); a fatal `error` rejects; otherwise a zero exit
/// resolves and anything else is `StudioError::Exit` with the stderr tail.
/// Cancelling kills the whole process tree and returns `StudioError::Cancelled`.
pub async fn run_worker<F>(
    spec: &WorkerSpec,
    cancel: &CancelToken,
    on_message: F,
) -> Result<WorkerReport, StudioError>
where
    F: FnMut(&WorkerMessage) + Send,
{
    run_worker_logged(spec, cancel, on_message, log_worker_noise).await
}

/// [`run_worker`] that also forwards every non-protocol line (stderr, and stdout
/// lines that are not JSON objects) to `on_log` as it arrives.
///
/// A worker that reports an `error` and then exits without `done` fails with that
/// error, even when it was marked recoverable and the exit code is 0 (the pipeline
/// worker reports stage failures that way).
pub async fn run_worker_logged<F, L>(
    spec: &WorkerSpec,
    cancel: &CancelToken,
    mut on_message: F,
    mut on_log: L,
) -> Result<WorkerReport, StudioError>
where
    F: FnMut(&WorkerMessage) + Send,
    L: FnMut(&str) + Send,
{
    if cancel.is_cancelled() {
        return Err(StudioError::Cancelled);
    }
    let mut child = build_command(&spec.interpreter, &spec.script, &spec.args, &spec.cwd)
        .spawn()
        .map_err(|e| map_spawn_error(&spec.interpreter, &spec.script, e))?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| StudioError::Io(std::io::Error::other("stdout not piped")))?;
    let (log_tx, mut log_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let stderr_task = child
        .stderr
        .take()
        .map(|s| tokio::spawn(read_tail(s, Some(log_tx))));

    let mut parser = JsonLineParser::default();
    let mut buf = vec![0u8; 8192];
    let mut decided: Option<Result<Option<Value>, StudioError>> = None;
    // Last non-fatal error, reported if the worker exits without `done`.
    let mut soft_error: Option<StudioError> = None;

    'read: loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                kill_tree(&mut child).await;
                if let Some(task) = stderr_task { task.abort(); }
                return Err(StudioError::Cancelled);
            }
            Some(line) = log_rx.recv() => on_log(&line),
            read = stdout.read(&mut buf) => {
                let n = read?;
                let lines = if n == 0 {
                    parser.finish().into_iter().collect::<Vec<_>>()
                } else {
                    parser.push(&buf[..n])
                };
                for line in lines {
                    if let ParsedLine::Message(msg) = &line {
                        on_message(msg);
                        if let Some(outcome) = classify(msg, spec.strict_errors) {
                            decided = Some(outcome);
                            break 'read;
                        }
                        if msg.is_error() {
                            soft_error = Some(worker_error(msg));
                        }
                    } else if let ParsedLine::Unparsed(text) = &line {
                        on_log(text);
                    }
                }
                if n == 0 {
                    break 'read;
                }
            }
        }
    }

    let status = match wait_or_cancel(&mut child, cancel).await? {
        Some(status) => status,
        None => {
            if let Some(task) = stderr_task {
                task.abort();
            }
            return Err(StudioError::Cancelled);
        }
    };
    let stderr = match stderr_task {
        Some(task) => match tokio::time::timeout(Duration::from_secs(2), task).await {
            Ok(Ok(text)) => text,
            _ => String::new(),
        },
        None => String::new(),
    };
    while let Ok(line) = log_rx.try_recv() {
        on_log(&line);
    }

    match (decided, soft_error) {
        (Some(Ok(outputs)), _) => Ok(WorkerReport {
            exit_code: status.code(),
            outputs,
        }),
        (Some(Err(err)), _) => Err(err),
        (None, Some(err)) => Err(err),
        (None, None) if status.success() => Ok(WorkerReport {
            exit_code: status.code(),
            outputs: None,
        }),
        (None, None) => Err(StudioError::Exit {
            code: status.code(),
            stderr,
        }),
    }
}

/// Maps a protocol message to a terminal decision, if it is one.
fn classify(
    msg: &WorkerMessage,
    strict_errors: bool,
) -> Option<Result<Option<Value>, StudioError>> {
    if msg.is_done() {
        return Some(Ok(msg.outputs.clone()));
    }
    if msg.is_error() && (strict_errors || !msg.recoverable) {
        return Some(Err(worker_error(msg)));
    }
    None
}

fn worker_error(msg: &WorkerMessage) -> StudioError {
    StudioError::Worker {
        message: msg
            .message
            .clone()
            .unwrap_or_else(|| "Worker error".to_string()),
        stage: msg.stage.clone(),
    }
}

/// Non-protocol worker output. This crate has no logger, so it goes to stderr.
fn log_worker_noise(text: &str) {
    eprintln!("[python] {text}");
}

/// Waits for exit, or kills the tree and returns `None` if cancelled first.
async fn wait_or_cancel(
    child: &mut Child,
    cancel: &CancelToken,
) -> Result<Option<ExitStatus>, StudioError> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            kill_tree(child).await;
            Ok(None)
        }
        status = child.wait() => Ok(Some(status?)),
    }
}

/// Reads a stream to the end, sending each non-blank line to `lines` as it
/// arrives, and returns the last [`STDERR_TAIL_BYTES`] for error reports.
async fn read_tail<R: AsyncRead + Unpin>(
    mut reader: R,
    lines: Option<tokio::sync::mpsc::UnboundedSender<String>>,
) -> String {
    let mut tail = Vec::new();
    let mut pending = Vec::new();
    let mut buf = vec![0u8; 4096];
    let send = |bytes: &[u8]| {
        let text = String::from_utf8_lossy(bytes);
        let text = text.trim();
        if let (Some(tx), false) = (&lines, text.is_empty()) {
            let _ = tx.send(text.to_string());
        }
    };
    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        tail.extend_from_slice(&buf[..n]);
        if tail.len() > STDERR_TAIL_BYTES * 4 {
            tail.drain(..tail.len() - STDERR_TAIL_BYTES);
        }
        pending.extend_from_slice(&buf[..n]);
        while let Some(pos) = pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = pending.drain(..=pos).collect();
            send(&line);
        }
    }
    send(&pending);
    let start = tail.len().saturating_sub(STDERR_TAIL_BYTES);
    String::from_utf8_lossy(&tail[start..]).trim().to_string()
}

/// Kills the child and everything it started.
///
/// Windows: `taskkill /T /F /PID`. Unix: SIGTERM to the process group (set at
/// spawn), then SIGKILL after a short grace period. Always reaps the child.
pub async fn kill_tree(child: &mut Child) {
    let Some(pid) = child.id() else {
        let _ = child.wait().await;
        return;
    };
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .creation_flags(crate::python::CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        let _ = child.start_kill();
    }
    #[cfg(unix)]
    {
        let group = format!("-{pid}");
        let _ = Command::new("kill")
            .args(["-s", "TERM", "--", &group])
            .status()
            .await;
        if tokio::time::timeout(KILL_GRACE, child.wait())
            .await
            .is_err()
        {
            let _ = Command::new("kill")
                .args(["-s", "KILL", "--", &group])
                .status()
                .await;
            let _ = child.start_kill();
        }
    }
    let _ = child.wait().await;
}

/// Runs a script to completion and returns `(exit code, stdout, stderr)`.
/// Used for one-shot helpers (voice preview, Kokoro status) that print a JSON line.
pub async fn run_capture(
    interpreter: &PythonInterpreter,
    script: &Path,
    args: &[String],
    cwd: &Path,
) -> Result<(Option<i32>, String, String), StudioError> {
    let out = build_command(interpreter, script, args, cwd)
        .output()
        .await
        .map_err(|e| map_spawn_error(interpreter, script, e))?;
    Ok((
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    #[test]
    fn parser_splits_lines_across_chunks() {
        let mut p = JsonLineParser::default();
        assert!(p.push(b"{\"type\":\"progress\",\"cur").is_empty());
        let out = p.push(b"rent\":2,\"total\":4}\n{\"type\":\"done\"}\n");
        assert_eq!(out.len(), 2);
        match &out[0] {
            ParsedLine::Message(m) => {
                assert_eq!(m.kind, "progress");
                assert_eq!(m.current, Some(2));
                assert_eq!(m.total, Some(4));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(&out[1], ParsedLine::Message(m) if m.is_done()));
    }

    #[test]
    fn parser_keeps_utf8_split_across_chunks() {
        let line = "{\"type\":\"info\",\"message\":\"नमस्ते\"}\n".as_bytes();
        let split = line.iter().position(|&b| b == 0xE0).unwrap() + 1; // mid-codepoint
        let mut p = JsonLineParser::default();
        assert!(p.push(&line[..split]).is_empty());
        let out = p.push(&line[split..]);
        match &out[0] {
            ParsedLine::Message(m) => assert_eq!(m.message.as_deref(), Some("नमस्ते")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parser_skips_blank_lines_and_keeps_non_json() {
        let mut p = JsonLineParser::default();
        let out = p.push(b"\r\n   \nTraceback (most recent call last):\r\n[1,2]\n{\"type\":\"error\",\"message\":\"x\",\"recoverable\":true}\r\n");
        assert_eq!(out.len(), 3);
        assert_eq!(
            out[0],
            ParsedLine::Unparsed("Traceback (most recent call last):".into())
        );
        assert_eq!(out[1], ParsedLine::Unparsed("[1,2]".into()));
        match &out[2] {
            ParsedLine::Message(m) => {
                assert!(m.is_error());
                assert!(m.recoverable);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parser_finish_flushes_partial_tail() {
        let mut p = JsonLineParser::default();
        assert!(p
            .push(b"{\"type\":\"done\",\"outputs\":{\"a\":1}}")
            .is_empty());
        match p.finish() {
            Some(ParsedLine::Message(m)) => {
                assert_eq!(m.outputs, Some(serde_json::json!({"a": 1})))
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(p.finish(), None);
    }

    #[test]
    fn command_building_matches_bridge_contract() {
        let interp = PythonInterpreter {
            program: "python".into(),
            prefix_args: vec!["-3".into()],
            version: "3.12.0".into(),
        };
        let cmd = build_command(
            &interp,
            Path::new("/x/pipeline_worker.py"),
            &["/tmp/job.json".into()],
            Path::new("/x"),
        );
        let std_cmd = cmd.as_std();
        assert_eq!(std_cmd.get_program(), "python");
        let args: Vec<_> = std_cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["-3", "/x/pipeline_worker.py", "/tmp/job.json"]);
        assert_eq!(std_cmd.get_current_dir(), Some(Path::new("/x")));
        let env: Vec<_> = std_cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert!(env.contains(&("PYTHONUNBUFFERED".into(), Some("1".into()))));
    }

    #[test]
    fn classify_respects_strictness() {
        let recoverable = WorkerMessage::from_value(
            serde_json::json!({"type":"error","message":"m","recoverable":true}),
        );
        assert!(classify(&recoverable, false).is_none());
        assert!(classify(&recoverable, true).unwrap().is_err());
        let fatal = WorkerMessage::from_value(
            serde_json::json!({"type":"error","message":"m","stage":"ocr"}),
        );
        match classify(&fatal, false).unwrap() {
            Err(StudioError::Worker { message, stage }) => {
                assert_eq!(message, "m");
                assert_eq!(stage.as_deref(), Some("ocr"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn cancel_token_wakes_waiters_and_is_sticky() {
        let token = CancelToken::new();
        let waiter = tokio::spawn({
            let t = token.clone();
            async move { t.cancelled().await }
        });
        tokio::task::yield_now().await;
        token.cancel();
        tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .unwrap()
            .unwrap();
        // Already cancelled: returns immediately.
        tokio::time::timeout(Duration::from_secs(5), token.cancelled())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn run_worker_streams_and_reports_done() {
        let Some(interp) = testutil::interpreter() else {
            eprintln!("skipping: no Python 3 on PATH");
            return;
        };
        let dir = testutil::unique_dir("bridge-ok");
        let script = testutil::write_fake_worker(&dir);
        let job = dir.join("job.json");
        std::fs::write(&job, testutil::job_json("ok-7")).unwrap();
        let spec = WorkerSpec {
            interpreter: interp,
            script,
            args: vec![job.display().to_string()],
            cwd: dir.clone(),
            strict_errors: false,
        };
        let mut kinds = Vec::new();
        let report = run_worker(&spec, &CancelToken::new(), |m| kinds.push(m.kind.clone()))
            .await
            .unwrap();
        assert_eq!(report.outputs, Some(serde_json::json!({"id": "7"})));
        assert_eq!(kinds.first().map(String::as_str), Some("info"));
        assert_eq!(kinds.last().map(String::as_str), Some("done"));
        assert_eq!(kinds.iter().filter(|k| *k == "progress").count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn run_worker_maps_fatal_error_and_exit_stderr() {
        let Some(interp) = testutil::interpreter() else {
            eprintln!("skipping: no Python 3 on PATH");
            return;
        };
        let dir = testutil::unique_dir("bridge-err");
        let script = testutil::write_fake_worker(&dir);
        let job = dir.join("job.json");
        let spec = |strict| WorkerSpec {
            interpreter: interp.clone(),
            script: script.clone(),
            args: vec![job.display().to_string()],
            cwd: dir.clone(),
            strict_errors: strict,
        };

        std::fs::write(&job, testutil::job_json("fail-1")).unwrap();
        match run_worker(&spec(false), &CancelToken::new(), |_| {}).await {
            Err(StudioError::Worker { message, .. }) => assert_eq!(message, "boom 1"),
            other => panic!("unexpected {other:?}"),
        }

        std::fs::write(&job, testutil::job_json("quit-1")).unwrap();
        match run_worker(&spec(false), &CancelToken::new(), |_| {}).await {
            Err(StudioError::Exit { code, stderr }) => {
                assert_eq!(code, Some(4));
                assert!(
                    stderr.contains("fake worker noise"),
                    "stderr was {stderr:?}"
                );
            }
            other => panic!("unexpected {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn recoverable_error_without_done_fails_and_logs_stderr() {
        let Some(interp) = testutil::interpreter() else {
            eprintln!("skipping: no Python 3 on PATH");
            return;
        };
        let dir = testutil::unique_dir("bridge-soft");
        let script = testutil::write_fake_worker(&dir);
        let job = dir.join("job.json");
        std::fs::write(&job, testutil::job_json("soft-3")).unwrap();
        let spec = WorkerSpec {
            interpreter: interp,
            script,
            args: vec![job.display().to_string()],
            cwd: dir.clone(),
            strict_errors: false,
        };
        let mut logs = Vec::new();
        let result =
            run_worker_logged(&spec, &CancelToken::new(), |_| {}, |l| logs.push(l.to_string())).await;
        match result {
            Err(StudioError::Worker { message, stage }) => {
                assert_eq!(message, "soft 3");
                assert_eq!(stage.as_deref(), Some("fake"));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(logs.iter().any(|l| l == "soft stderr line"), "logs were {logs:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn run_worker_cancel_kills_child() {
        let Some(interp) = testutil::interpreter() else {
            eprintln!("skipping: no Python 3 on PATH");
            return;
        };
        let dir = testutil::unique_dir("bridge-cancel");
        let script = testutil::write_fake_worker(&dir);
        let job = dir.join("job.json");
        std::fs::write(&job, testutil::job_json("sleep-2")).unwrap();
        let spec = WorkerSpec {
            interpreter: interp,
            script,
            args: vec![job.display().to_string()],
            cwd: dir.clone(),
            strict_errors: false,
        };
        let cancel = CancelToken::new();
        let trigger = cancel.clone();
        let started = Arc::new(Notify::new());
        let signal = started.clone();
        let run = tokio::spawn(async move {
            run_worker(&spec, &cancel, move |m| {
                if m.kind == "progress" {
                    signal.notify_one();
                }
            })
            .await
        });
        tokio::time::timeout(Duration::from_secs(30), started.notified())
            .await
            .expect("worker started");
        trigger.cancel();
        let result = tokio::time::timeout(Duration::from_secs(30), run)
            .await
            .expect("cancel returns promptly")
            .unwrap();
        assert!(
            matches!(result, Err(StudioError::Cancelled)),
            "got {result:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
