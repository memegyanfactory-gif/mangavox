//! Coding-agent CLIs (Claude Code, Codex, Gemini CLI, OpenCode, Grok, Antigravity) driven headless.
//! Prompts always go over stdin; argv only ever carries allow-listed flag values, because
//! Windows npm shims (`.cmd`) need a shell (`cmd /C`) to start.

use super::{emit_reasoning, emit_text, StepContext};
use crate::stream::{js_str, AiError, LineBuffer, Result};
use crate::transcript::build_transcript;
use crate::types::{AiAccessMode, AiProviderId, AiStepRequest, AiStepResult, AiUsage, Emit, StreamEvent};
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

pub fn cli_binary(id: AiProviderId) -> Option<&'static str> {
    match id {
        AiProviderId::ClaudeCode => Some("claude"),
        AiProviderId::Codex => Some("codex"),
        AiProviderId::GeminiCli => Some("gemini"),
        AiProviderId::Opencode => Some("opencode"),
        AiProviderId::GrokCli => Some("grok"),
        AiProviderId::Antigravity => Some("agy"),
        _ => None,
    }
}

fn safe_arg(a: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[\w.:/@~+=,-]+$").unwrap()).is_match(a)
}

/// Build the command. Args are allow-listed; on Windows the binary runs through `cmd` so npm `.cmd` shims work.
pub fn start_cli(bin: &str, args: &[String], workspace: &Path) -> Result<Command> {
    for a in args {
        if !safe_arg(a) {
            return Err(AiError::msg(format!("Refusing unsafe CLI argument: {a}")));
        }
    }
    let _ = std::fs::create_dir_all(workspace);
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.raw_arg(format!("/d /s /c \"{} {}\"", bin, args.join(" ")));
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new(bin);
        c.args(args);
        c
    };
    cmd.current_dir(workspace).env("NO_COLOR", "1").env("FORCE_COLOR", "0").kill_on_drop(true);
    Ok(cmd)
}

/// Kill a process and its children (taskkill /T /F on Windows, SIGTERM elsewhere).
pub async fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        let mut c = Command::new("taskkill");
        c.args(["/pid", &pid.to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null());
        c.creation_flags(0x0800_0000);
        let _ = c.status().await;
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("kill").args(["-TERM", &pid.to_string()]).stdout(Stdio::null()).stderr(Stdio::null()).status().await;
    }
}

/// Locate a binary on PATH. Returns the resolved path or None.
pub async fn find_binary(bin: &str) -> Option<String> {
    let mut c = Command::new(if cfg!(windows) { "where" } else { "which" });
    c.arg(bin).stdin(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    c.creation_flags(0x0800_0000);
    let out = tokio::time::timeout(Duration::from_secs(5), c.output()).await.ok()?.ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).lines().map(str::trim).find(|l| !l.is_empty()).map(String::from)
}

async fn run_capture(bin: &str, args: &[&str], workspace: &Path, timeout: Duration) -> Option<String> {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut cmd = start_cli(bin, &args, workspace).ok()?;
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let child = cmd.spawn().ok()?;
    let pid = child.id();
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(out)) => Some(String::from_utf8_lossy(&out.stdout).into_owned()),
        Ok(Err(_)) => None,
        Err(_) => {
            if let Some(p) = pid {
                kill_tree(p).await;
            }
            None
        }
    }
}

pub async fn cli_version(bin: &str, workspace: &Path) -> String {
    run_capture(bin, &["--version"], workspace, Duration::from_secs(10))
        .await
        .and_then(|o| o.lines().next().map(|l| l.trim().to_string()))
        .unwrap_or_default()
}

/// `opencode models` prints one provider/model per line.
pub async fn opencode_models(workspace: &Path) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^[\w.-]+/\S+$").unwrap());
    run_capture("opencode", &["models"], workspace, Duration::from_secs(20))
        .await
        .map(|o| o.lines().map(str::trim).filter(|l| re.is_match(l)).map(String::from).collect())
        .unwrap_or_default()
}

/// `grok models` prints `  * grok-4.7 (default)` / `  grok-x` lines under "Available models:".
pub fn parse_grok_models(out: &str) -> Vec<String> {
    let mut on = false;
    let mut v = Vec::new();
    for l in out.lines() {
        if l.trim_start().starts_with("Available models") {
            on = true;
        } else if on {
            let id = l.trim().trim_start_matches('*').trim().split_whitespace().next().unwrap_or("");
            if !id.is_empty() && safe_arg(id) {
                v.push(id.to_string());
            }
        }
    }
    v
}

pub async fn grok_models(workspace: &Path) -> Vec<String> {
    run_capture("grok", &["models"], workspace, Duration::from_secs(20)).await.map(|o| parse_grok_models(&o)).unwrap_or_default()
}

/// `agy models` prints `<id>\t<label>` lines after a "Fetching available models..." line.
pub fn parse_agy_models(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|l| {
            let (id, label) = l.split_once('\t')?;
            let id = id.trim();
            safe_arg(id).then(|| (id.to_string(), label.trim().to_string()))
        })
        .collect()
}

pub async fn agy_models(workspace: &Path) -> Vec<(String, String)> {
    run_capture("agy", &["models"], workspace, Duration::from_secs(20)).await.map(|o| parse_agy_models(&o)).unwrap_or_default()
}

/// What goes on stdin: the plain prompt, except Antigravity which wants one NDJSON `user` event.
pub fn stdin_payload(id: AiProviderId, prompt: &str) -> String {
    match id {
        AiProviderId::Antigravity => format!("{}\n", serde_json::json!({ "event": "user", "message": { "content": prompt } })),
        _ => prompt.to_string(),
    }
}

// ── Per-CLI command lines ────────────────────────────────────────────────────

fn has_model(req: &AiStepRequest) -> bool {
    !req.model.is_empty() && req.model != "default"
}

fn full(req: &AiStepRequest) -> bool {
    req.access == Some(AiAccessMode::Full)
}

pub fn cli_args(req: &AiStepRequest) -> Vec<String> {
    cli_args_with(req, &[])
}

/// Directory (inside the workspace) that attached images are copied to for the duration of a step.
pub const ATTACH_DIR: &str = ".attach";

/// Workspace-relative, forward-slash names (`.attach/<index>_<sanitized name>`) for the images, in order.
pub fn attachment_names(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let raw = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "image".into());
            let clean: String = raw.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' }).collect();
            format!("{ATTACH_DIR}/{i}_{clean}")
        })
        .collect()
}

/// Text appended to the stdin prompt so the CLI opens the attached files.
pub fn attachment_prompt(id: AiProviderId, rels: &[String]) -> String {
    if rels.is_empty() {
        return String::new();
    }
    match id {
        AiProviderId::GeminiCli => format!("

{}", rels.iter().map(|r| format!("@{r}")).collect::<Vec<_>>().join("
")),
        AiProviderId::Opencode => String::new(), // attached with -f
        _ => format!(
            "

Attached images - open every one with the {} before answering, in this order:
{}",
            if id == AiProviderId::ClaudeCode { "Read tool" } else { "image viewer" },
            rels.iter().map(|r| format!("- {r}")).collect::<Vec<_>>().join("
")
        ),
    }
}

/// Copy the images into `<workspace>/.attach`, returning their relative names. Failures become Status events.
fn stage_attachments(ctx: &StepContext) -> Vec<String> {
    let paths = &ctx.req.images;
    if paths.is_empty() {
        return Vec::new();
    }
    let dir = ctx.workspace.join(ATTACH_DIR);
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        ctx.status(format!("Could not attach images: {e}"));
        return Vec::new();
    }
    let mut ok = Vec::new();
    for (p, rel) in paths.iter().zip(attachment_names(paths)) {
        match std::fs::copy(p, ctx.workspace.join(&rel)) {
            Ok(_) => ok.push(rel),
            Err(e) => ctx.status(format!("Skipped {} ({e})", p.display())),
        }
    }
    ok
}

/// Like [`cli_args`] with the workspace-relative attachment names from [`attachment_names`].
pub fn cli_args_with(req: &AiStepRequest, rels: &[String]) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let mut add = |xs: &[&str]| a.extend(xs.iter().map(|s| s.to_string()));
    match req.provider_id {
        AiProviderId::ClaudeCode => {
            add(&["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages"]);
            if has_model(req) {
                add(&["--model", &req.model]);
            }
            if let Some(e) = req.effort {
                add(&["--effort", e.as_str()]);
            }
            add(&["--permission-mode", if full(req) { "acceptEdits" } else { "default" }]);
            if !rels.is_empty() {
                add(&["--allowedTools", "Read"]);
            }
        }
        AiProviderId::Codex => {
            add(&["exec"]);
            for r in rels {
                add(&["-i", r]);
            }
            add(&["--json", "--skip-git-repo-check", "--ephemeral", "--sandbox", if full(req) { "workspace-write" } else { "read-only" }]);
            if has_model(req) {
                add(&["-m", &req.model]);
            }
            if let Some(e) = req.effort {
                add(&["-c", &format!("model_reasoning_effort={}", e.as_str())]);
            }
            add(&["-"]);
        }
        AiProviderId::GeminiCli => {
            add(&["-o", "stream-json", "-p", "Answer."]);
            if has_model(req) {
                add(&["-m", &req.model]);
            }
            add(&["--approval-mode", if full(req) { "auto_edit" } else { "default" }]);
        }
        AiProviderId::Opencode => {
            add(&["run", "--format", "json"]);
            if has_model(req) {
                add(&["-m", &req.model]);
            }
            if let Some(e) = req.effort {
                add(&["--variant", e.as_str()]);
            }
            if full(req) {
                add(&["--auto"]);
            }
            for r in rels {
                add(&["-f", r]);
            }
        }
        AiProviderId::GrokCli => {
            add(&["--output-format", "streaming-json", "-p", "-"]);
            if has_model(req) {
                add(&["-m", &req.model]);
            }
            add(&["--permission-mode", if full(req) { "acceptEdits" } else { "default" }]);
        }
        AiProviderId::Antigravity => {
            add(&["--input-format", "stream-json", "--output-format", "stream-json"]);
            if has_model(req) {
                add(&["--model", &req.model]);
            }
            if let Some(e) = req.effort {
                add(&["--effort", e.as_str()]);
            }
            if full(req) {
                add(&["--mode", "accept-edits"]);
            }
        }
        _ => {}
    }
    a
}

// ── Event parsing ────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct CliAcc {
    pub text: String,
    pub reasoning: String,
    saw_delta: bool,
    pub error: Option<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    parts: HashMap<String, String>,
    /// Lines that were not understood as JSON events.
    pub plain: Vec<String>,
}

impl CliAcc {
    fn emit_text(&mut self, emit: &Emit, delta: &str) {
        if delta.is_empty() {
            return;
        }
        self.text.push_str(delta);
        emit_text(emit, delta);
    }

    /// Feed one stdout line from the given CLI.
    pub fn feed_line(&mut self, id: AiProviderId, line: &str, emit: &Emit) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        if line.starts_with('{') {
            if let Ok(ev) = serde_json::from_str::<Value>(line) {
                if self.on_event(id, &ev, emit) {
                    return;
                }
            }
        }
        self.plain.push(line.to_string());
    }

    /// Handle one parsed event; false if it was not understood.
    pub fn on_event(&mut self, id: AiProviderId, ev: &Value, emit: &Emit) -> bool {
        match id {
            AiProviderId::ClaudeCode => self.claude(ev, emit),
            AiProviderId::Codex => self.codex(ev, emit),
            AiProviderId::GeminiCli => self.gemini(ev, emit),
            AiProviderId::Opencode => self.opencode(ev, emit),
            AiProviderId::GrokCli => self.grok(ev, emit),
            AiProviderId::Antigravity => self.antigravity(ev, emit),
            _ => false,
        }
    }

    fn claude(&mut self, ev: &Value, emit: &Emit) -> bool {
        let ty = ev.get("type").and_then(Value::as_str).unwrap_or("");
        match ty {
            "stream_event" => {
                let d = ev.pointer("/event/delta");
                match d.and_then(|d| d.get("type")).and_then(Value::as_str) {
                    Some("text_delta") => {
                        self.saw_delta = true;
                        let t = js_str(d.and_then(|d| d.get("text")));
                        self.emit_text(emit, &t);
                    }
                    Some("thinking_delta") => {
                        let t = js_str(d.and_then(|d| d.get("thinking")));
                        self.reasoning.push_str(&t);
                        emit_reasoning(emit, &t);
                    }
                    _ => {}
                }
                true
            }
            "assistant" => {
                for b in ev.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") if !self.saw_delta => {
                            let t = js_str(b.get("text"));
                            self.emit_text(emit, &t);
                        }
                        Some("tool_use") => emit(StreamEvent::Activity { label: format!("Claude Code: {}", js_str(b.get("name"))), detail: None }),
                        _ => {}
                    }
                }
                self.saw_delta = false;
                true
            }
            "result" => {
                if ev.get("is_error").and_then(Value::as_bool) == Some(true) {
                    let r = js_str(ev.get("result"));
                    let sub = js_str(ev.get("subtype"));
                    self.error = Some(if !r.is_empty() { r } else if !sub.is_empty() { sub } else { "Claude Code failed".into() });
                }
                self.input_tokens = ev.pointer("/usage/input_tokens").and_then(Value::as_u64);
                self.output_tokens = ev.pointer("/usage/output_tokens").and_then(Value::as_u64);
                if self.text.is_empty() && self.error.is_none() {
                    if let Some(r) = ev.get("result").and_then(Value::as_str) {
                        self.emit_text(emit, r);
                    }
                }
                true
            }
            _ => ty == "system" || ty == "user",
        }
    }

    fn codex(&mut self, ev: &Value, emit: &Emit) -> bool {
        let ty = ev.get("type").and_then(Value::as_str);
        let item = ev.get("item").filter(|i| !i.is_null());
        let s = |v: Option<&Value>| v.and_then(Value::as_str).map(String::from);
        match (ty, item) {
            (Some("item.completed"), Some(item)) => {
                match item.get("type").and_then(Value::as_str) {
                    Some("agent_message") => {
                        let sep = if self.text.is_empty() { "" } else { "\n\n" };
                        let t = format!("{sep}{}", s(item.get("text")).unwrap_or_default());
                        self.emit_text(emit, &t);
                    }
                    Some("reasoning") => {
                        if let Some(t) = s(item.get("text")).filter(|t| !t.is_empty()) {
                            self.reasoning.push_str(&t);
                            emit_reasoning(emit, &t);
                        }
                    }
                    Some("error") => emit(StreamEvent::Status { text: js_str(item.get("message")) }),
                    _ => {}
                }
                true
            }
            (Some("item.started"), Some(item)) => {
                match item.get("type").and_then(Value::as_str) {
                    Some("command_execution") => emit(StreamEvent::Activity { label: "Codex ran a command".into(), detail: s(item.get("command")) }),
                    Some("web_search") => emit(StreamEvent::Activity { label: "Codex searched the web".into(), detail: s(item.get("query")) }),
                    Some("mcp_tool_call") => emit(StreamEvent::Activity {
                        label: format!("Codex: {}", s(item.get("tool")).unwrap_or_else(|| "tool".into())),
                        detail: None,
                    }),
                    _ => {}
                }
                true
            }
            (Some("turn.completed"), _) => {
                self.input_tokens = ev.pointer("/usage/input_tokens").and_then(Value::as_u64);
                self.output_tokens = ev.pointer("/usage/output_tokens").and_then(Value::as_u64);
                true
            }
            (Some("turn.failed"), _) => {
                self.error = Some(clean_codex_error(ev.pointer("/error/message")));
                true
            }
            (Some("error"), _) => {
                if self.error.is_none() {
                    self.error = Some(clean_codex_error(ev.get("message")));
                }
                true
            }
            (Some(_), _) => true,
            _ => false,
        }
    }

    fn gemini(&mut self, ev: &Value, emit: &Emit) -> bool {
        let ty = ev.get("type").and_then(Value::as_str);
        match ty {
            Some("message") if ev.get("role").and_then(Value::as_str) == Some("assistant") => {
                let t = js_str(ev.get("content"));
                self.emit_text(emit, &t);
                true
            }
            Some("tool_use") => {
                let name = ev.get("tool_name").and_then(Value::as_str).unwrap_or("tool");
                emit(StreamEvent::Activity { label: format!("Gemini: {name}"), detail: None });
                true
            }
            Some("error") => {
                let m = ev.get("message").and_then(Value::as_str).unwrap_or("warning");
                emit(StreamEvent::Status { text: m.to_string() });
                true
            }
            Some("result") => {
                if ev.get("status").and_then(Value::as_str) == Some("error") {
                    self.error = Some(ev.pointer("/error/message").and_then(Value::as_str).unwrap_or("Gemini CLI failed").to_string());
                }
                self.input_tokens = ev.pointer("/stats/input_tokens").and_then(Value::as_u64);
                self.output_tokens = ev.pointer("/stats/output_tokens").and_then(Value::as_u64);
                true
            }
            Some(_) => true,
            None => false,
        }
    }

    fn opencode(&mut self, ev: &Value, emit: &Emit) -> bool {
        let ty = ev.get("type").and_then(Value::as_str);
        let part = ev.get("part").cloned().unwrap_or(Value::Null);
        let part_text = part.get("text").and_then(Value::as_str);
        match ty {
            Some("text") if part_text.is_some() => {
                // A part can be re-sent as it grows; only emit the new tail.
                let next = part_text.unwrap();
                let key = js_str(part.get("id"));
                let prev = self.parts.insert(key, next.to_string()).unwrap_or_default();
                let tail = next.strip_prefix(prev.as_str()).unwrap_or(next).to_string();
                self.emit_text(emit, &tail);
                true
            }
            Some("reasoning") if part_text.is_some() => {
                let t = part_text.unwrap();
                self.reasoning.push_str(t);
                emit_reasoning(emit, t);
                true
            }
            Some("tool_use") => {
                let tool = part.get("tool").and_then(Value::as_str).unwrap_or("tool");
                emit(StreamEvent::Activity { label: format!("OpenCode: {tool}"), detail: None });
                true
            }
            Some("step_finish") => {
                self.input_tokens = part.pointer("/tokens/input").and_then(Value::as_u64);
                self.output_tokens = part.pointer("/tokens/output").and_then(Value::as_u64);
                true
            }
            Some("error") => {
                let e = ev.get("error");
                let msg = e
                    .and_then(|e| e.pointer("/data/message"))
                    .and_then(Value::as_str)
                    .or_else(|| e.and_then(|e| e.get("message")).and_then(Value::as_str))
                    .map(String::from)
                    .or_else(|| e.filter(|v| !v.is_null()).map(|v| js_str(Some(v))))
                    .unwrap_or_else(|| "OpenCode failed".into());
                self.error = Some(msg);
                true
            }
            Some(_) => true,
            None => false,
        }
    }

    /// Grok Build `--output-format streaming-json`: `text` / `thought` deltas in `data`, `tool_call`, `usage`, `end`, `error`.
    fn grok(&mut self, ev: &Value, emit: &Emit) -> bool {
        match ev.get("type").and_then(Value::as_str) {
            Some("text") => {
                let t = js_str(ev.get("data"));
                self.emit_text(emit, &t);
                true
            }
            Some("thought") => {
                let t = js_str(ev.get("data"));
                self.reasoning.push_str(&t);
                emit_reasoning(emit, &t);
                true
            }
            Some("tool_call") => {
                let name = ev.get("toolName").and_then(Value::as_str).unwrap_or("tool");
                emit(StreamEvent::Activity { label: format!("Grok: {name}"), detail: None });
                true
            }
            Some("usage") | Some("end") => {
                if let Some(u) = ev.get("usage") {
                    self.input_tokens = u.get("input_tokens").and_then(Value::as_u64);
                    self.output_tokens = u.get("output_tokens").and_then(Value::as_u64);
                }
                true
            }
            Some("error") => {
                self.error = Some(ev.get("message").and_then(Value::as_str).unwrap_or("Grok failed").to_string());
                true
            }
            Some(_) => true,
            None => false,
        }
    }

    /// Antigravity (`agy`) `--output-format stream-json`: `step_update` events carry `text_delta` (agent_response)
    /// and tool steps; a final `result` has status, full response and usage.
    fn antigravity(&mut self, ev: &Value, emit: &Emit) -> bool {
        match ev.get("event").and_then(Value::as_str) {
            Some("step_update") => {
                let su = ev.get("step_update").unwrap_or(&Value::Null);
                match su.get("step_type").and_then(Value::as_str) {
                    Some("agent_response") => {
                        if let Some(d) = su.get("text_delta").and_then(Value::as_str) {
                            self.emit_text(emit, d);
                        }
                    }
                    Some("tool") if su.get("state").and_then(Value::as_str) == Some("ACTIVE") => {
                        let name = su.get("tool_name").and_then(Value::as_str).unwrap_or("tool");
                        emit(StreamEvent::Activity { label: format!("Antigravity: {name}"), detail: None });
                    }
                    _ => {}
                }
                true
            }
            Some("result") => {
                let r = ev.get("result").unwrap_or(&Value::Null);
                self.input_tokens = r.pointer("/usage/input_tokens").and_then(Value::as_u64);
                self.output_tokens = r.pointer("/usage/output_tokens").and_then(Value::as_u64);
                if r.get("status").and_then(Value::as_str).is_some_and(|s| s != "SUCCESS") {
                    self.error = Some(r.get("error").and_then(Value::as_str).filter(|e| !e.is_empty()).unwrap_or("Antigravity failed").to_string());
                } else if self.text.trim().is_empty() {
                    if let Some(t) = r.get("response").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
                        self.emit_text(emit, t);
                    }
                }
                true
            }
            Some(_) => true,
            None => false,
        }
    }

    /// Final result once the process has exited. `code` is the exit code (None if killed by a signal).
    pub fn finish(mut self, bin: &str, code: Option<i32>, aborted: bool, stderr: &str, emit: &Emit) -> Result<AiStepResult> {
        if aborted {
            return Ok(AiStepResult { text: self.text, stopped: true, ..Default::default() });
        }
        if self.text.is_empty() && !self.plain.is_empty() && self.error.is_none() {
            let joined = self.plain.join("\n");
            self.emit_text(emit, &joined);
        }
        if let (Some(e), true) = (&self.error, self.text.is_empty()) {
            return Err(AiError::msg(e.clone()));
        }
        if code != Some(0) && self.text.is_empty() {
            let tail = last_meaningful_line(stderr);
            return Err(AiError::msg(if tail.is_empty() {
                format!("{bin} exited with code {}", code.map_or("null".into(), |c| c.to_string()))
            } else {
                tail
            }));
        }
        Ok(AiStepResult {
            text: self.text,
            reasoning: if self.reasoning.is_empty() { None } else { Some(self.reasoning) },
            usage: Some(AiUsage { input_tokens: self.input_tokens, output_tokens: self.output_tokens }),
            ..Default::default()
        })
    }
}

fn clean_codex_error(msg: Option<&Value>) -> String {
    let s = match msg {
        None | Some(Value::Null) => "Codex failed".to_string(),
        other => js_str(other),
    };
    serde_json::from_str::<Value>(&s)
        .ok()
        .and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(String::from))
        .unwrap_or(s)
}

pub fn last_meaningful_line(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines[lines.len().saturating_sub(3)..].join(" · ")
}

// ── Running ──────────────────────────────────────────────────────────────────

pub async fn cli_step(ctx: &StepContext) -> Result<AiStepResult> {
    let id = ctx.req.provider_id;
    let bin = cli_binary(id).ok_or_else(|| AiError::msg(format!("{} is not a CLI provider", id.as_str())))?;
    let rels = stage_attachments(ctx);
    let args = cli_args_with(&ctx.req, &rels);
    let mut prompt = build_transcript(&ctx.req.messages);
    prompt.push_str(&attachment_prompt(id, &rels));
    let res = run_cli(ctx, id, bin, &args, stdin_payload(id, &prompt)).await;
    if !ctx.req.images.is_empty() {
        let _ = std::fs::remove_dir_all(ctx.workspace.join(ATTACH_DIR));
    }
    res
}

async fn run_cli(ctx: &StepContext, id: AiProviderId, bin: &'static str, args: &[String], prompt: String) -> Result<AiStepResult> {
    let mut cmd = start_cli(bin, args, &ctx.workspace)?;
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| AiError::msg(format!("{bin} could not start: {e}")))?;
    let pid = child.id();

    if let Some(mut stdin) = child.stdin.take() {
        tokio::spawn(async move {
            // The process may exit before reading stdin; ignore broken pipes.
            let _ = stdin.write_all(prompt.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });
    }
    let mut stderr_pipe = child.stderr.take();
    let stderr_task = tokio::spawn(async move {
        let mut tail = String::new();
        if let Some(p) = stderr_pipe.as_mut() {
            let mut buf = [0u8; 4096];
            while let Ok(n) = p.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                tail.push_str(&String::from_utf8_lossy(&buf[..n]));
                if tail.len() > 8000 {
                    let mut cut = tail.len() - 4000;
                    while !tail.is_char_boundary(cut) {
                        cut += 1;
                    }
                    tail = tail[cut..].to_string();
                }
            }
        }
        tail
    });

    let mut acc = CliAcc::default();
    let mut lb = LineBuffer::new();
    let mut stdout = child.stdout.take().ok_or_else(|| AiError::msg("no stdout"))?;
    let mut buf = [0u8; 8192];
    let cancel: &CancellationToken = &ctx.cancel;
    let mut aborted = false;
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                aborted = true;
                if let Some(p) = pid { kill_tree(p).await; }
                let _ = child.start_kill();
                break;
            }
            n = stdout.read(&mut buf) => match n {
                Ok(0) | Err(_) => break,
                Ok(n) => for line in lb.push(&buf[..n]) { acc.feed_line(id, &line, &ctx.emit); },
            }
        }
    }
    if !aborted {
        if let Some(rest) = lb.finish() {
            acc.feed_line(id, &rest, &ctx.emit);
        }
    }
    let status = child.wait().await;
    let stderr = stderr_task.await.unwrap_or_default();
    let code = status.ok().and_then(|s| s.code());
    acc.finish(bin, code, aborted || cancel.is_cancelled(), &stderr, &ctx.emit)
}

#[cfg(test)]
mod tests {
    use super::super::testutil::recorder;
    use super::*;
    use crate::types::{AiEffort, AiMessage};

    fn feed_all(id: AiProviderId, fixture: &str) -> (CliAcc, Vec<StreamEvent>) {
        let (emit, log) = recorder();
        let mut acc = CliAcc::default();
        for line in fixture.lines() {
            acc.feed_line(id, line, &emit);
        }
        let ev = log.lock().unwrap().clone();
        (acc, ev)
    }

    const CLAUDE: &str = r#"{"type":"system","subtype":"init","session_id":"s1","tools":["Read"],"model":"claude-sonnet-5-5"}
{"type":"stream_event","event":{"type":"message_start","message":{"id":"m"}}}
{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}}
{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hello "}}}
{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"world"}}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hello world"},{"type":"tool_use","id":"t","name":"Read","input":{}}]}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":"x"}]}}
{"type":"result","subtype":"success","is_error":false,"result":"Hello world","usage":{"input_tokens":10,"output_tokens":4}}
"#;

    #[test]
    fn claude_code_stream_json() {
        let (acc, ev) = feed_all(AiProviderId::ClaudeCode, CLAUDE);
        assert_eq!(acc.text, "Hello world"); // not duplicated by the full assistant message / result
        assert_eq!(acc.reasoning, "hmm");
        assert!(acc.plain.is_empty());
        assert!(ev.contains(&StreamEvent::Activity { label: "Claude Code: Read".into(), detail: None }));
        let (emit, _) = recorder();
        let r = acc.finish("claude", Some(0), false, "", &emit).unwrap();
        assert_eq!(r.usage.unwrap().output_tokens, Some(4));
    }

    #[test]
    fn claude_code_without_partials_uses_the_assistant_message_and_errors_surface() {
        let (acc, _) = feed_all(
            AiProviderId::ClaudeCode,
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Hi there\"}]}}\n{\"type\":\"result\",\"is_error\":false,\"result\":\"Hi there\"}\n",
        );
        assert_eq!(acc.text, "Hi there");
        let (acc, _) = feed_all(AiProviderId::ClaudeCode, "{\"type\":\"result\",\"is_error\":true,\"result\":\"Invalid API key\"}\n");
        let (emit, _) = recorder();
        let e = acc.finish("claude", Some(1), false, "", &emit).unwrap_err();
        assert_eq!(e.to_string(), "Invalid API key");
    }

    const CODEX: &str = r#"{"type":"thread.started","thread_id":"th"}
{"type":"turn.started"}
{"type":"item.completed","item":{"id":"i0","type":"reasoning","text":"Thinking about it"}}
{"type":"item.started","item":{"id":"i1","type":"command_execution","command":"ls -la","status":"in_progress"}}
{"type":"item.completed","item":{"id":"i1","type":"command_execution","command":"ls -la","aggregated_output":"x","exit_code":0,"status":"completed"}}
{"type":"item.completed","item":{"id":"i2","type":"agent_message","text":"First part."}}
{"type":"item.completed","item":{"id":"i3","type":"agent_message","text":"Second part."}}
{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":20}}
"#;

    #[test]
    fn codex_json_lines() {
        let (acc, ev) = feed_all(AiProviderId::Codex, CODEX);
        assert_eq!(acc.text, "First part.\n\nSecond part.");
        assert_eq!(acc.reasoning, "Thinking about it");
        assert!(ev.contains(&StreamEvent::Activity { label: "Codex ran a command".into(), detail: Some("ls -la".into()) }));
        let (emit, _) = recorder();
        let r = acc.finish("codex", Some(0), false, "", &emit).unwrap();
        let u = r.usage.unwrap();
        assert_eq!((u.input_tokens, u.output_tokens), (Some(100), Some(20)));
    }

    #[test]
    fn codex_failure_message_is_unwrapped() {
        let (acc, _) = feed_all(
            AiProviderId::Codex,
            "{\"type\":\"error\",\"message\":\"{\\\"error\\\":{\\\"message\\\":\\\"model not found\\\"}}\"}\n{\"type\":\"turn.failed\",\"error\":{\"message\":\"{\\\"error\\\":{\\\"message\\\":\\\"model not found\\\"}}\"}}\n",
        );
        let (emit, _) = recorder();
        assert_eq!(acc.finish("codex", Some(1), false, "", &emit).unwrap_err().to_string(), "model not found");
    }

    const OPENCODE: &str = r#"{"type":"step_start","timestamp":1,"sessionID":"s","part":{"id":"p0","type":"step-start"}}
{"type":"reasoning","timestamp":2,"part":{"id":"p1","type":"reasoning","text":"pondering"}}
{"type":"text","timestamp":3,"part":{"id":"p2","type":"text","text":"Hel"}}
{"type":"text","timestamp":4,"part":{"id":"p2","type":"text","text":"Hello there"}}
{"type":"tool_use","timestamp":5,"part":{"id":"p3","type":"tool","tool":"bash","state":{"status":"completed"}}}
{"type":"step_finish","timestamp":6,"part":{"id":"p4","type":"step-finish","reason":"stop","tokens":{"input":7,"output":3,"reasoning":0}}}
"#;

    #[test]
    fn opencode_format_json_grows_parts_without_duplication() {
        let (acc, ev) = feed_all(AiProviderId::Opencode, OPENCODE);
        assert_eq!(acc.text, "Hello there");
        assert_eq!(acc.reasoning, "pondering");
        assert!(ev.contains(&StreamEvent::Activity { label: "OpenCode: bash".into(), detail: None }));
        let (emit, _) = recorder();
        let r = acc.finish("opencode", Some(0), false, "", &emit).unwrap();
        assert_eq!(r.usage.unwrap().input_tokens, Some(7));
    }

    #[test]
    fn opencode_error_and_plain_text_fallback() {
        let (acc, _) = feed_all(AiProviderId::Opencode, "{\"type\":\"error\",\"error\":{\"name\":\"X\",\"data\":{\"message\":\"no auth\"}}}\n");
        let (emit, _) = recorder();
        assert_eq!(acc.finish("opencode", Some(1), false, "", &emit).unwrap_err().to_string(), "no auth");
        // non-JSON output becomes the reply
        let (acc, _) = feed_all(AiProviderId::Opencode, "just words\nmore words\n");
        let (emit, log) = recorder();
        let r = acc.finish("opencode", Some(0), false, "", &emit).unwrap();
        assert_eq!(r.text, "just words\nmore words");
        assert_eq!(log.lock().unwrap().len(), 1);
    }

    #[test]
    fn gemini_cli_stream_json() {
        let fixture = r#"{"type":"init","session_id":"s","model":"gemini-2.5-pro"}
{"type":"message","role":"user","content":"Answer."}
{"type":"message","role":"assistant","content":"Hi ","delta":true}
{"type":"message","role":"assistant","content":"friend","delta":true}
{"type":"tool_use","tool_name":"read_file","tool_id":"1","parameters":{}}
{"type":"result","status":"success","stats":{"input_tokens":5,"output_tokens":2}}
"#;
        let (acc, ev) = feed_all(AiProviderId::GeminiCli, fixture);
        assert_eq!(acc.text, "Hi friend");
        assert!(ev.contains(&StreamEvent::Activity { label: "Gemini: read_file".into(), detail: None }));
    }

    // Captured from grok 1.0.62 (`grok --output-format streaming-json -p -`), long fields trimmed.
    const GROK: &str = r#"{"type":"available_commands","tools":["read_file"],"commands":["compact"]}
{"type":"thought","data":"The"}
{"type":"thought","data":" user wants OK."}
{"type":"tool_call","toolCallId":"c0","title":"list_dir","kind":"list","status":"pending","toolName":"list_dir","rawInput":{},"content":[],"locations":[]}
{"type":"tool_call_update","toolCallId":"c0","status":"completed","content":[],"rawOutput":null,"locations":[]}
{"type":"text","data":"O"}
{"type":"text","data":"K"}
{"type":"usage","usage":{"input_tokens":17321,"output_tokens":24,"reasoning_tokens":23},"signature":"x"}
{"type":"end","stopReason":"end_turn","sessionId":"s","usage":{"input_tokens":23577,"output_tokens":132},"num_turns":2}
"#;

    #[test]
    fn grok_streaming_json() {
        let (acc, ev) = feed_all(AiProviderId::GrokCli, GROK);
        assert_eq!(acc.text, "OK");
        assert_eq!(acc.reasoning, "The user wants OK.");
        assert!(acc.plain.is_empty());
        assert!(ev.contains(&StreamEvent::Activity { label: "Grok: list_dir".into(), detail: None }));
        let (emit, _) = recorder();
        let r = acc.finish("grok", Some(0), false, "", &emit).unwrap();
        let u = r.usage.unwrap();
        assert_eq!((u.input_tokens, u.output_tokens), (Some(23577), Some(132)));
        let (acc, _) = feed_all(AiProviderId::GrokCli, "{\"type\":\"error\",\"message\":\"unknown model id\"}\n");
        assert_eq!(acc.finish("grok", Some(1), false, "", &emit).unwrap_err().to_string(), "unknown model id");
    }

    // Captured from agy 1.4.2 (`agy --input-format stream-json --output-format stream-json`), trimmed.
    const AGY: &str = r#"{"event":"init","conversation_id":"c","init":{"cwd":"x","tools":["view_file"]}}
{"event":"step_update","step_update":{"conversation_id":"c","step_index":0,"state":"DONE","step_type":"user_input"}}
{"event":"step_update","step_update":{"conversation_id":"c","step_index":2,"state":"ACTIVE","step_type":"agent_response","text_delta":"OK"}}
{"event":"step_update","step_update":{"conversation_id":"c","step_index":3,"state":"ACTIVE","step_type":"tool","tool_name":"view_file","tool_info":{"name":"view_file"}}}
{"event":"step_update","step_update":{"conversation_id":"c","step_index":3,"state":"DONE","step_type":"tool","tool_name":"view_file"}}
{"event":"step_update","step_update":{"conversation_id":"c","step_index":2,"state":"DONE","step_type":"agent_response","text_delta":"\n","usage":{"input_tokens":2051}}}
{"event":"result","result":{"conversation_id":"c","status":"SUCCESS","response":"OK\n","num_turns":1,"usage":{"input_tokens":2051,"output_tokens":501,"thinking_tokens":500}}}
"#;

    #[test]
    fn antigravity_stream_json() {
        let (acc, ev) = feed_all(AiProviderId::Antigravity, AGY);
        assert_eq!(acc.text, "OK\n"); // deltas are not duplicated by the final `response`
        assert!(acc.plain.is_empty());
        assert_eq!(ev.iter().filter(|e| matches!(e, StreamEvent::Activity { .. })).count(), 1);
        let (emit, _) = recorder();
        let r = acc.finish("agy", Some(0), false, "", &emit).unwrap();
        assert_eq!(r.usage.unwrap().output_tokens, Some(501));
        // no deltas: the result's response is the reply; failures surface the error
        let (acc, _) = feed_all(AiProviderId::Antigravity, "{\"event\":\"result\",\"result\":{\"status\":\"SUCCESS\",\"response\":\"Hi\"}}\n");
        assert_eq!(acc.text, "Hi");
        let (acc, _) = feed_all(AiProviderId::Antigravity, "{\"event\":\"result\",\"result\":{\"status\":\"ERROR\",\"error\":\"stream input message is missing the event field\"}}\n");
        assert!(acc.finish("agy", Some(0), false, "", &emit).unwrap_err().to_string().contains("missing the event"));
    }

    #[test]
    fn grok_and_antigravity_commands_and_models() {
        let mut req = AiStepRequest {
            request_id: "r".into(),
            provider_id: AiProviderId::GrokCli,
            model: "grok-4.7".into(),
            messages: vec![AiMessage::user("x")],
            tools: None,
            effort: Some(AiEffort::Low),
            access: Some(AiAccessMode::Full),
            images: vec![],
            workspace: None,
        };
        let a = cli_args(&req);
        assert_eq!(&a[..4], ["--output-format", "streaming-json", "-p", "-"]);
        assert!(a.windows(2).any(|w| w == ["-m", "grok-4.7"]) && a.windows(2).any(|w| w == ["--permission-mode", "acceptEdits"]));
        assert!(a.iter().all(|x| safe_arg(x)));
        req.provider_id = AiProviderId::Antigravity;
        req.model = "gemini-3.8-flash-high".into();
        let a = cli_args(&req);
        assert!(a.windows(2).any(|w| w == ["--input-format", "stream-json"]) && a.windows(2).any(|w| w == ["--mode", "accept-edits"]));
        assert!(a.windows(2).any(|w| w == ["--model", "gemini-3.8-flash-high"]) && a.windows(2).any(|w| w == ["--effort", "low"]));
        assert!(a.iter().all(|x| safe_arg(x)));
        assert_eq!((cli_binary(AiProviderId::GrokCli), cli_binary(AiProviderId::Antigravity)), (Some("grok"), Some("agy")));
        assert_eq!(stdin_payload(AiProviderId::Codex, "a\nb"), "a\nb");
        let p = stdin_payload(AiProviderId::Antigravity, "a \"b\"\nc");
        assert!(p.ends_with('\n') && p.matches('\n').count() == 1);
        assert_eq!(serde_json::from_str::<Value>(&p).unwrap().pointer("/message/content").and_then(Value::as_str), Some("a \"b\"\nc"));
        assert_eq!(parse_grok_models("You are logged in.\n\nDefault model: grok-4.7\n\nAvailable models:\n  * grok-4.7 (default)\n  grok-5\n"), ["grok-4.7", "grok-5"]);
        let m = parse_agy_models("Fetching available models...\ngemini-3.8-flash-high\tGemini 3.8 Flash (High)\nclaude-sonnet-4-6\tClaude Sonnet 4.6 (Thinking)\n");
        assert_eq!(m[0], ("gemini-3.8-flash-high".to_string(), "Gemini 3.8 Flash (High)".to_string()));
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn failures_use_stderr_and_exit_code() {
        let (emit, _) = recorder();
        let e = CliAcc::default().finish("codex", Some(2), false, "warn\n\nboom happened\n", &emit).unwrap_err();
        assert_eq!(e.to_string(), "warn · boom happened");
        let e = CliAcc::default().finish("codex", Some(3), false, "", &emit).unwrap_err();
        assert_eq!(e.to_string(), "codex exited with code 3");
        let r = CliAcc::default().finish("codex", None, true, "", &emit).unwrap();
        assert!(r.stopped);
    }

    #[test]
    fn arguments_are_allow_listed() {
        for ok in ["--model", "claude-sonnet-5-5", "opencode/big-pickle", "model_reasoning_effort=high", "a@b~c+d,e:f"] {
            assert!(safe_arg(ok), "{ok}");
        }
        for bad in ["a b", "x;y", "$(id)", "a&b", "\"q\"", "a|b", "", "`x`"] {
            assert!(!safe_arg(bad), "{bad}");
        }
        let err = start_cli("claude", &["--model".into(), "x; rm -rf /".into()], Path::new(".")).err().unwrap();
        assert!(err.to_string().contains("unsafe"));
    }

    #[test]
    fn command_lines_per_cli() {
        let mut req = AiStepRequest {
            request_id: "r".into(),
            provider_id: AiProviderId::ClaudeCode,
            model: "opus".into(),
            messages: vec![AiMessage::user("x")],
            tools: None,
            effort: Some(AiEffort::High),
            access: Some(AiAccessMode::Full),
            images: vec![],
            workspace: None,
        };
        let a = cli_args(&req);
        assert!(a.windows(2).any(|w| w == ["--permission-mode", "acceptEdits"]));
        assert!(a.windows(2).any(|w| w == ["--effort", "high"]));
        req.provider_id = AiProviderId::Codex;
        req.model = "default".into();
        let a = cli_args(&req);
        assert_eq!(a.last().unwrap(), "-");
        assert!(!a.contains(&"-m".to_string()));
        assert!(a.windows(2).any(|w| w == ["--sandbox", "workspace-write"]));
        req.provider_id = AiProviderId::Opencode;
        req.model = "opencode/big-pickle".into();
        let a = cli_args(&req);
        assert!(a.contains(&"--auto".to_string()) && a.windows(2).any(|w| w == ["-m", "opencode/big-pickle"]));
        assert!(a.iter().all(|x| safe_arg(x)));
    }

    #[test]
    fn image_attachments_per_cli() {
        let rels = attachment_names(&[PathBuf::from("/x/My Page 003.jpg"), PathBuf::from(r"C:\a\face.png")]);
        assert_eq!(rels, [".attach/0_My_Page_003.jpg", ".attach/1_face.png"]);
        let mut req = AiStepRequest {
            request_id: "r".into(),
            provider_id: AiProviderId::ClaudeCode,
            model: "claude-haiku-5-5".into(),
            messages: vec![AiMessage::user("x")],
            tools: None,
            effort: None,
            access: None,
            images: vec![],
            workspace: None,
        };
        let none = cli_args_with(&req, &[]);
        assert!(!none.contains(&"--allowedTools".to_string()));
        let a = cli_args_with(&req, &rels);
        assert!(a.windows(2).any(|w| w == ["--allowedTools", "Read"]));
        assert!(a.iter().all(|x| safe_arg(x)));
        assert!(attachment_prompt(AiProviderId::ClaudeCode, &rels).contains("- .attach/0_My_Page_003.jpg"));
        req.provider_id = AiProviderId::Codex;
        let a = cli_args_with(&req, &rels);
        assert_eq!(a.last().unwrap(), "-");
        assert!(a.windows(2).any(|w| w == ["-i", ".attach/1_face.png"]));
        assert!(a.iter().all(|x| safe_arg(x)));
        req.provider_id = AiProviderId::GeminiCli;
        assert_eq!(cli_args_with(&req, &rels), cli_args(&req));
        assert_eq!(attachment_prompt(AiProviderId::GeminiCli, &rels), "

@.attach/0_My_Page_003.jpg
@.attach/1_face.png");
        req.provider_id = AiProviderId::Opencode;
        let a = cli_args_with(&req, &rels);
        assert!(a.windows(2).any(|w| w == ["-f", ".attach/0_My_Page_003.jpg"]));
        assert!(a.iter().all(|x| safe_arg(x)));
        assert_eq!(attachment_prompt(AiProviderId::Opencode, &rels), "");
        assert_eq!(attachment_prompt(AiProviderId::Codex, &[]), "");
    }

    #[test]
    fn claude_code_model_ids_are_safe() {
        for m in ["claude-haiku-5-5", "claude-sonnet-5-5", "claude-opus-5-5", "haiku", "sonnet", "opus"] {
            assert!(safe_arg(m), "{m}");
        }
    }

    /// Live: needs the `opencode` CLI; `opencode/big-pickle` is free.
    #[tokio::test]
    #[ignore]
    async fn live_opencode_big_pickle() {
        let dir = std::env::temp_dir().join("mv-ai-live-workspace");
        let (emit, log) = recorder();
        let ctx = StepContext {
            req: AiStepRequest {
                request_id: "live".into(),
                provider_id: AiProviderId::Opencode,
                model: "opencode/big-pickle".into(),
                messages: vec![AiMessage::user("Reply with exactly: ok")],
                tools: None,
                effort: None,
                access: None,
                images: vec![],
                workspace: None,
            },
            cancel: CancellationToken::new(),
            api_key: None,
            base_url: String::new(),
            emit,
            client: crate::stream::build_client(),
            workspace: dir,
        };
        let r = cli_step(&ctx).await.expect("opencode run");
        assert!(!r.text.trim().is_empty(), "empty reply");
        assert!(!log.lock().unwrap().is_empty());
    }

    /// Live: cancelling kills the whole process tree promptly.
    #[tokio::test]
    #[ignore]
    async fn live_opencode_cancel() {
        let dir = std::env::temp_dir().join("mv-ai-live-workspace");
        let (emit, _) = recorder();
        let cancel = CancellationToken::new();
        let ctx = StepContext {
            req: AiStepRequest {
                request_id: "live2".into(),
                provider_id: AiProviderId::Opencode,
                model: "opencode/big-pickle".into(),
                messages: vec![AiMessage::user("Write a 2000 word essay about the ocean.")],
                tools: None,
                effort: None,
                access: None,
                images: vec![],
                workspace: None,
            },
            cancel: cancel.clone(),
            api_key: None,
            base_url: String::new(),
            emit,
            client: crate::stream::build_client(),
            workspace: dir,
        };
        let c = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            c.cancel();
        });
        let r = cli_step(&ctx).await.expect("stopped, not an error");
        assert!(r.stopped);
    }
}
