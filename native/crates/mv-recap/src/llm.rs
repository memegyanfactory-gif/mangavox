//! The one seam between this crate and the AI layer, so read/story logic can be tested with canned answers.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use futures::future::BoxFuture;
use mv_ai::{emit_noop, AiMessage, AiStepRequest, CancellationToken, Registry};
use serde_json::Value;

use crate::json::extract_json;
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
                }
            }
        })
    }
}

pub(crate) fn is_cancelled(e: &anyhow::Error) -> bool {
    e.to_string() == "cancelled"
}

pub(crate) fn request(m: &ModelChoice, messages: Vec<AiMessage>, images: Vec<PathBuf>, workspace: Option<PathBuf>) -> AiStepRequest {
    static N: AtomicU64 = AtomicU64::new(0);
    AiStepRequest {
        request_id: format!("recap-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)),
        provider_id: m.provider,
        model: m.model.clone(),
        messages,
        tools: None,
        effort: None,
        access: None,
        images,
        workspace,
        lean: true,
        mcp: None,
    }
}

/// Ask for JSON; on unparseable output retry once with a nudge. `Ok(Err(raw))` = still not JSON.
pub(crate) async fn ask_json(llm: &dyn Llm, req: AiStepRequest, cancel: &CancellationToken) -> Result<std::result::Result<Value, String>> {
    let raw = llm.step(req.clone(), cancel.clone()).await?;
    if let Some(v) = extract_json(&raw) {
        return Ok(Ok(v));
    }
    let mut retry = req;
    retry.request_id.push_str("-retry");
    retry.messages.push(AiMessage::assistant(raw.clone()));
    retry.messages.push(AiMessage::user("Your previous answer was not valid JSON. Reply with JSON only."));
    let raw2 = llm.step(retry, cancel.clone()).await?;
    let raw2 = if raw2.trim().is_empty() { raw } else { raw2 };
    // A provider talking about itself is a failure, never text to store as a reading.
    if let Some(e) = mv_ai::limit::provider_error_in(&raw2) {
        bail!(e);
    }
    Ok(extract_json(&raw2).ok_or(raw2))
}

pub(crate) fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}
