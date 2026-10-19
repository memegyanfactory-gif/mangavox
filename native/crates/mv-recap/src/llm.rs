//! The one seam between this crate and the AI layer, so read/story logic can be tested with canned answers.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use futures::future::BoxFuture;
use mv_ai::{emit_noop, AiMessage, AiStepRequest, CancellationToken, Registry};
use serde_json::Value;

use crate::json::extract_json;
use crate::types::ModelChoice;

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
    Ok(extract_json(&raw2).ok_or(if raw2.trim().is_empty() { raw } else { raw2 }))
}

pub(crate) fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}
