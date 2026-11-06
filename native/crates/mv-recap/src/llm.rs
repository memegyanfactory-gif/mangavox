//! The one seam between this crate and the AI layer, so read/story logic can be tested with canned answers.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use futures::future::BoxFuture;
use mv_ai::{emit_noop, AiEffort, AiMessage, AiStepRequest, CancellationToken, Registry};
use serde_json::Value;

use crate::json::{extract_whole_json, salvage_json};
use crate::types::{ModelChoice, Progress, ProgressFn};

pub(crate) trait Llm: Send + Sync {
    /// One model call; returns the assistant text. Errors on provider failure or cancellation.
    fn step<'a>(&'a self, req: AiStepRequest, cancel: CancellationToken) -> BoxFuture<'a, Result<String>>;
}

impl Llm for Arc<Registry> {
    fn step<'a>(&'a self, req: AiStepRequest, cancel: CancellationToken) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let r = self.run_step_cancellable(req, emit_noop(), cancel).await;
            if r.stopped {
                bail!("cancelled");
            }
            match r.error {
                Some(e) => Err(anyhow!(e)),
                None => Ok(r.text),
            }
        })
    }
}

/// The registry for long runs (Read, Story): when the provider hits a usage / rate limit, the run
/// says so ([`Progress::Waiting`]), sleeps until the limit resets and sends the same request again,
/// so nothing is skipped or stored half-done. Cancelling stops the wait.
pub(crate) struct Patient {
    reg: Arc<Registry>,
    progress: ProgressFn,
}

impl Patient {
    pub(crate) fn new(reg: Arc<Registry>, progress: ProgressFn) -> Self {
        Patient { reg, progress }
    }
}

impl Llm for Patient {
    fn step<'a>(&'a self, req: AiStepRequest, cancel: CancellationToken) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let mut attempt = 0;
            loop {
                let r = self.reg.run_step_cancellable(req.clone(), emit_noop(), cancel.clone()).await;
                if r.stopped {
                    bail!("cancelled");
                }
                let Some(limit) = r.limit else {
                    if attempt > 0 {
                        (self.progress)(Progress::Resumed);
                    }
                    return match r.error {
                        Some(e) => Err(anyhow!(e)),
                        None => Ok(r.text),
                    };
                };
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
                let wait = limit.wait_secs(attempt, now);
                (self.progress)(Progress::Waiting { message: r.error.unwrap_or(limit.message), until: now + wait as i64 });
                attempt += 1;
                tokio::select! {
                    _ = cancel.cancelled() => bail!("cancelled"),
                    _ = tokio::time::sleep(std::time::Duration::from_secs(wait)) => {}
                    _ = retry_signal().notified() => {}
                }
            }
        })
    }
}

fn retry_signal() -> &'static tokio::sync::Notify {
    static N: std::sync::OnceLock<tokio::sync::Notify> = std::sync::OnceLock::new();
    N.get_or_init(tokio::sync::Notify::new)
}

/// A run waiting for a usage limit to reset sends its request again now (the user asked to carry on).
pub fn retry_now() {
    retry_signal().notify_waiters();
}

pub(crate) fn is_cancelled(e: &anyhow::Error) -> bool {
    e.to_string() == "cancelled"
}

pub(crate) fn request(m: &ModelChoice, messages: Vec<AiMessage>, images: Vec<PathBuf>, workspace: Option<PathBuf>) -> AiStepRequest {
    static N: AtomicU64 = AtomicU64::new(0);
    AiStepRequest {
        request_id: format!("recap-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)),
        provider_id: m.provider,
        // The id stays as chosen; mv-ai matches Antigravity's model id to the effort (and drops the effort where it is refused).
        model: m.model.clone(),
        messages,
        tools: None,
        // Low effort: each call thinks briefly before it answers. A page read went from 22 s to 14 s on Opus.
        effort: Some(AiEffort::Low),
        access: None,
        images,
        workspace,
        lean: true,
        mcp: None,
    }
}

/// How long to wait before a failed model call is sent again (the 1st and 2nd retry).
fn backoff(retry: usize) -> std::time::Duration {
    if cfg!(test) {
        return std::time::Duration::from_millis(1);
    }
    std::time::Duration::from_secs(if retry == 0 { 4 } else { 12 })
}

/// One model call that survives a hiccup: a CLI that exits, a timeout, an overloaded server, an
/// empty answer or a provider message in place of an answer is sent again twice, after a short wait
/// that cancelling cuts short. Only the last failure is returned.
async fn call_with_retries(llm: &dyn Llm, req: &AiStepRequest, cancel: &CancellationToken) -> Result<String> {
    let mut retry = 0;
    loop {
        let failure = match llm.step(req.clone(), cancel.clone()).await {
            Ok(text) if text.trim().is_empty() => anyhow!("the AI sent an empty answer"),
            // A provider talking about itself is a failure, never text to store as a reading.
            Ok(text) => match mv_ai::limit::provider_error_in(&text) {
                Some(e) => anyhow!(e),
                None => return Ok(text),
            },
            Err(e) if is_cancelled(&e) => return Err(e),
            Err(e) => e,
        };
        if retry >= 2 || cancel.is_cancelled() {
            return Err(failure);
        }
        tokio::select! {
            _ = cancel.cancelled() => bail!("cancelled"),
            _ = tokio::time::sleep(backoff(retry)) => {}
        }
        retry += 1;
    }
}

/// Ask for JSON; on unparseable output retry once with a nudge. `Ok(Err(raw))` = still not JSON.
/// A reply that was cut off is not used here: a half page reading would silently drop panels.
pub(crate) async fn ask_json(llm: &dyn Llm, req: AiStepRequest, cancel: &CancellationToken) -> Result<std::result::Result<Value, String>> {
    ask_json_with(llm, req, cancel, false).await
}

/// [`ask_json`] for the story: an answer that was cut off keeps what it said (the story then asks
/// for the pages it is missing) instead of failing the part.
pub(crate) async fn ask_story_json(llm: &dyn Llm, req: AiStepRequest, cancel: &CancellationToken) -> Result<std::result::Result<Value, String>> {
    ask_json_with(llm, req, cancel, true).await
}

async fn ask_json_with(llm: &dyn Llm, req: AiStepRequest, cancel: &CancellationToken, salvage: bool) -> Result<std::result::Result<Value, String>> {
    let raw = call_with_retries(llm, &req, cancel).await?;
    if let Some(v) = extract_whole_json(&raw) {
        return Ok(Ok(v));
    }
    let mut retry = req;
    retry.request_id.push_str("-retry");
    retry.messages.push(AiMessage::assistant(raw.clone()));
    retry.messages.push(AiMessage::user("Your previous answer was not valid JSON. Reply with JSON only."));
    // A nudge that cannot be answered leaves the first reply to be judged.
    let raw2 = match call_with_retries(llm, &retry, cancel).await {
        Ok(t) => t,
        Err(e) if is_cancelled(&e) => return Err(e),
        Err(_) => raw.clone(),
    };
    if let Some(v) = extract_whole_json(&raw2) {
        return Ok(Ok(v));
    }
    // Cut off mid-way: what was said is kept, the caller asks for what is missing.
    if salvage {
        if let Some(v) = salvage_json(&raw2).or_else(|| salvage_json(&raw)) {
            return Ok(Ok(v));
        }
    }
    Ok(Err(raw2))
}

pub(crate) fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}
