//! The one seam to the AI layer, so the planning logic can be tested with canned answers.
//! Requests go to whichever provider and model the user picked; usage limits are waited out.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use futures::future::BoxFuture;
use mv_ai::{emit_noop, AiEffort, AiMessage, AiStepRequest, CancellationToken, Registry};
use mv_recap::{extract_json, ModelChoice};
use serde_json::Value;

pub(crate) trait Llm: Send + Sync {
    fn step<'a>(&'a self, req: AiStepRequest, cancel: CancellationToken) -> BoxFuture<'a, Result<String>>;
}

/// The registry, waiting out usage limits (`on_wait` tells the user) and asking again.
pub(crate) struct Patient {
    pub reg: Arc<Registry>,
    pub on_wait: Arc<dyn Fn(String) + Send + Sync>,
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
                    return match r.error {
                        Some(e) => Err(anyhow!(e)),
                        None => Ok(r.text),
                    };
                };
                let now = crate::util::now();
                let wait = limit.wait_secs(attempt, now);
                (self.on_wait)(format!("{} - waiting {} min, then asking again", r.error.unwrap_or(limit.message), wait.div_ceil(60)));
                attempt += 1;
                tokio::select! {
                    _ = cancel.cancelled() => bail!("cancelled"),
                    _ = tokio::time::sleep(std::time::Duration::from_secs(wait)) => {}
                }
            }
        })
    }
}

pub(crate) fn request(m: &ModelChoice, system: &str, user: String, images: Vec<PathBuf>) -> AiStepRequest {
    static N: AtomicU64 = AtomicU64::new(0);
    AiStepRequest {
        request_id: format!("mv3d-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)),
        provider_id: m.provider,
        model: m.model.clone(),
        messages: vec![AiMessage::system(system.to_string()), AiMessage::user(user)],
        tools: None,
        effort: Some(AiEffort::Low),
        access: None,
        images,
        workspace: None,
        lean: true,
        mcp: None,
    }
}

/// Asks for JSON; on an unreadable answer asks once more.
pub(crate) async fn ask_json(llm: &dyn Llm, req: AiStepRequest, cancel: &CancellationToken) -> Result<Value> {
    let raw = llm.step(req.clone(), cancel.clone()).await?;
    if let Some(v) = extract_json(&raw) {
        return Ok(v);
    }
    let mut retry = req;
    retry.request_id.push_str("-retry");
    retry.messages.push(AiMessage::assistant(raw.clone()));
    retry.messages.push(AiMessage::user("Your previous answer was not valid JSON. Reply with the JSON object only."));
    let raw2 = llm.step(retry, cancel.clone()).await?;
    if let Some(e) = mv_ai::limit::provider_error_in(&raw2) {
        bail!(e);
    }
    extract_json(&raw2).ok_or_else(|| anyhow!("the AI did not answer with JSON: {}", crate::util::truncate(&raw2, 300)))
}

/// Asks for a Python script; returns the code inside the first ```python fence (or the whole answer).
pub(crate) async fn ask_code(llm: &dyn Llm, req: AiStepRequest, cancel: &CancellationToken) -> Result<String> {
    let raw = llm.step(req, cancel.clone()).await?;
    if let Some(e) = mv_ai::limit::provider_error_in(&raw) {
        bail!(e);
    }
    let code = extract_code(&raw);
    if code.trim().is_empty() {
        bail!("the AI answered without a script");
    }
    Ok(code)
}

pub(crate) fn extract_code(raw: &str) -> String {
    for fence in ["```python", "```py", "```"] {
        if let Some(start) = raw.find(fence) {
            let rest = &raw[start + fence.len()..];
            let rest = rest.strip_prefix('\n').or_else(|| rest.strip_prefix("\r\n")).unwrap_or(rest);
            let end = rest.find("```").unwrap_or(rest.len());
            return rest[..end].trim_end().to_string();
        }
    }
    raw.trim().to_string()
}

/// Canned answers for tests: the n-th call gets the n-th answer (the last one repeats).
#[cfg(test)]
pub(crate) struct Canned(pub std::sync::Mutex<Vec<String>>, pub std::sync::Mutex<Vec<AiStepRequest>>);

#[cfg(test)]
impl Llm for Canned {
    fn step<'a>(&'a self, req: AiStepRequest, _cancel: CancellationToken) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            self.1.lock().unwrap().push(req);
            let mut v = self.0.lock().unwrap();
            Ok(if v.len() > 1 { v.remove(0) } else { v.first().cloned().unwrap_or_default() })
        })
    }
}
