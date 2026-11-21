//! One JSON call to the user's AI (design 3.4, 6): a port of `mv-recap`'s private `llm.rs`.
//! Each call is stopped after 600 s, retried twice (after 4 s and 12 s) on an empty or failed
//! reply, nudged once to "reply with valid JSON only", and waits out a usage limit (woken early
//! when another account signs in, or by [`retry_now`]). No Auto-switch bail.
//!
//! Test seam (design 11.1): an [`Ai`] made by [`Ai::fake`] sends every call to its [`Ask`]
//! instead of the registry, so tests feed canned replies ([`Canned`]): good JSON, a cut-off
//! reply, junk, a provider error or a usage limit. A fake answers at once: retries and limit
//! waits are a few milliseconds.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use futures::future::BoxFuture;
use mv_ai::{emit_noop, AiEffort, AiLimit, AiMessage, AiProviderId, AiStepRequest, AiStepResult, CancellationToken, Registry};
use mv_recap::ModelChoice;
use serde_json::Value;

/// Longest one model call may take.
const STEP_TIMEOUT: Duration = Duration::from_secs(600);
/// How long a timed-out call is given to stop after its cancel before it is left behind.
const CANCEL_GRACE: Duration = Duration::from_secs(15);
const TIMED_OUT: &str = "The AI did not answer within 10 minutes. Try again, or pick another model in the engine settings.";
const CANCELLED: &str = "cancelled";
/// Longest wait for a limit of a CLI whose sign-in cannot be watched.
const UNTRACKED_WAIT: u64 = 1800;
/// What the user reads when the reply was not JSON even after the nudge.
pub const UNREADABLE: &str = "The AI's answer could not be read. Try again or pick another model.";

/// One model call: the seam between the engine and the AI layer. The real one is the app's
/// registry; tests use [`Canned`]. The result is the registry's: `text`, or `error` (with
/// `limit` when it is a usage limit), or `stopped` when cancelled.
pub trait Ask: Send + Sync {
    /// Sends one request and answers the model's result (never an `Err`: failures are in the result).
    fn ask<'a>(&'a self, req: AiStepRequest, cancel: CancellationToken) -> BoxFuture<'a, AiStepResult>;
}

impl Ask for Registry {
    fn ask<'a>(&'a self, req: AiStepRequest, cancel: CancellationToken) -> BoxFuture<'a, AiStepResult> {
        Box::pin(self.run_step_cancellable(req, emit_noop(), cancel))
    }
}

/// Canned replies for tests, answered in order; once they run out every call answers with an
/// error. Records every request it was sent.
#[derive(Default)]
pub struct Canned {
    replies: Mutex<VecDeque<AiStepResult>>,
    seen: Mutex<Vec<AiStepRequest>>,
}

impl Canned {
    /// Replies that are plain text, in order.
    pub fn new<S: Into<String>>(texts: impl IntoIterator<Item = S>) -> Canned {
        Canned::from_results(texts.into_iter().map(|t| AiStepResult { text: t.into(), ..Default::default() }))
    }

    /// Full results (errors, limits), in order.
    pub fn from_results(results: impl IntoIterator<Item = AiStepResult>) -> Canned {
        Canned { replies: Mutex::new(results.into_iter().collect()), seen: Mutex::new(Vec::new()) }
    }

    /// A usage-limit reply (`retry_at` in unix seconds, if the provider said).
    pub fn limit(message: &str, retry_at: Option<i64>) -> AiStepResult {
        AiStepResult {
            error: Some(message.to_string()),
            limit: Some(AiLimit { message: message.to_string(), retry_at }),
            ..Default::default()
        }
    }

    /// A provider error reply.
    pub fn error(message: &str) -> AiStepResult {
        AiStepResult { error: Some(message.to_string()), ..Default::default() }
    }

    /// Every request sent so far.
    pub fn seen(&self) -> Vec<AiStepRequest> {
        self.seen.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl Ask for Canned {
    fn ask<'a>(&'a self, req: AiStepRequest, _cancel: CancellationToken) -> BoxFuture<'a, AiStepResult> {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(req);
        }
        let next = self.replies.lock().ok().and_then(|mut r| r.pop_front());
        Box::pin(async move { next.unwrap_or_else(|| Canned::error("no more canned replies")) })
    }
}

/// The AI a job talks to: the app's registry and the chosen provider + model. Tests build one
/// with [`Ai::fake`]: its model id names a registered [`Ask`] that answers instead of the
/// registry.
#[derive(Clone)]
pub struct Ai {
    pub reg: Arc<Registry>,
    pub model: ModelChoice,
}

/// Model ids of fakes start with this; the rest keys [`fakes`].
const FAKE_PREFIX: &str = "mv-anime-fake-";

/// Every fake made in this process (tests only; a few per test run).
fn fakes() -> &'static Mutex<std::collections::HashMap<String, Arc<dyn Ask>>> {
    static F: std::sync::OnceLock<Mutex<std::collections::HashMap<String, Arc<dyn Ask>>>> = std::sync::OnceLock::new();
    F.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// A JSON reply that could not be read even after the nudge. Its `Display` is the message for
/// the user; `raw` is the last reply, for the step to save to `work/ai/<job>.txt`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BadReply {
    pub raw: String,
}

impl std::fmt::Display for BadReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(UNREADABLE)
    }
}

impl std::error::Error for BadReply {}

/// Whether an error from [`Ai::ask_json`] is the user's cancel.
pub fn is_cancelled(e: &anyhow::Error) -> bool {
    e.to_string() == CANCELLED
}

fn retry_signal() -> &'static tokio::sync::Notify {
    static N: std::sync::OnceLock<tokio::sync::Notify> = std::sync::OnceLock::new();
    N.get_or_init(tokio::sync::Notify::new)
}

/// A call waiting for a usage limit to reset sends its request again now (the user asked to
/// carry on).
pub fn retry_now() {
    retry_signal().notify_waiters();
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The reply says the model cannot see the attached pictures.
fn says_blind(text: &str) -> bool {
    let t = text.to_lowercase();
    let cannot = ["can't see", "cannot see", "can not see", "unable to see", "can't view", "cannot view", "unable to view", "can't process", "cannot process", "don't have the ability to see", "do not have the ability to see", "not able to see"];
    let what = ["image", "picture", "photo", "attachment"];
    t.len() < 2000 && cannot.iter().any(|c| t.contains(c)) && what.iter().any(|w| t.contains(w))
}

/// A reply that is not JSON but holds an answer to mend (it opens a JSON value and says
/// something), unlike an empty reply or a refusal, which has to be asked again in full.
fn repairable(raw: &str) -> bool {
    let t = raw.trim();
    t.len() >= 200 && t.contains(['{', '['])
}

impl Ai {
    /// The registry answers for `model`.
    pub fn new(reg: Arc<Registry>, model: ModelChoice) -> Ai {
        Ai { reg, model }
    }

    /// An AI that only `ask` answers (tests). The registry beside it is never called. Its model
    /// id holds a key (`mv-anime-fake-<n>`) that must stay in the id; text may go before it
    /// (e.g. `"gpt-oss-120b/" + id` to look like a text-only model).
    pub fn fake(ask: Arc<dyn Ask>) -> Ai {
        static N: AtomicU64 = AtomicU64::new(0);
        let key = format!("{FAKE_PREFIX}{}", N.fetch_add(1, Ordering::Relaxed));
        if let Ok(mut f) = fakes().lock() {
            f.insert(key.clone(), ask);
        }
        let tmp = std::env::temp_dir().join("mv-anime-fake-ai");
        let reg = Arc::new(Registry::new(tmp.join("config.json"), tmp));
        Ai { reg, model: ModelChoice { provider: AiProviderId::Anthropic, model: key } }
    }

    /// The fake answering for this AI, if it is one.
    fn fake_ask(&self) -> Option<Arc<dyn Ask>> {
        let at = self.model.model.find(FAKE_PREFIX)?;
        fakes().lock().ok()?.get(&self.model.model[at..]).cloned()
    }

    fn is_fake(&self) -> bool {
        self.model.model.contains(FAKE_PREFIX)
    }

    /// [`Ai::fake`] answering `texts` in order.
    pub fn canned<S: Into<String>>(texts: impl IntoIterator<Item = S>) -> Ai {
        Ai::fake(Arc::new(Canned::new(texts)))
    }

    /// Whether the model can be sent pictures, as far as is known without asking it
    /// (Antigravity's GPT-OSS is text only; the others take images or say so in their reply).
    pub fn can_see(&self) -> bool {
        !(self.model.provider == AiProviderId::Antigravity && self.model.model.starts_with("gpt-oss"))
    }

    fn label(&self) -> String {
        if self.model.model.is_empty() {
            self.model.provider.as_str().to_string()
        } else {
            self.model.model.clone()
        }
    }

    fn request(&self, system: &str, user: String, images: Vec<PathBuf>, effort: AiEffort) -> AiStepRequest {
        static N: AtomicU64 = AtomicU64::new(0);
        let workspace = if !self.is_fake() {
            let ws = crate::store::root().join("work").join("ai");
            let _ = std::fs::create_dir_all(&ws);
            Some(ws)
        } else {
            None
        };
        AiStepRequest {
            request_id: format!("anime-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)),
            provider_id: self.model.provider,
            model: self.model.model.clone(),
            messages: vec![AiMessage::system(system), AiMessage::user(user)],
            tools: None,
            effort: Some(effort),
            access: None,
            images,
            workspace,
            lean: true,
            mcp: None,
        }
    }

    /// How long to wait before a failed call is sent again (1st and 2nd retry).
    fn backoff(&self, retry: usize) -> Duration {
        if self.is_fake() {
            return Duration::from_millis(2);
        }
        Duration::from_secs(if retry == 0 { 4 } else { 12 })
    }

    /// One call, stopped after [`STEP_TIMEOUT`]. `None`: it timed out.
    async fn timed(&self, req: AiStepRequest, cancel: &CancellationToken) -> Option<AiStepResult> {
        let token = cancel.child_token();
        let fake = self.fake_ask();
        let run: BoxFuture<'_, AiStepResult> = match &fake {
            Some(ask) => ask.ask(req, token.clone()),
            None if self.is_fake() => Box::pin(async { Canned::error("This test AI is no longer registered.") }),
            None => Ask::ask(&*self.reg, req, token.clone()),
        };
        tokio::pin!(run);
        tokio::select! {
            r = &mut run => Some(r),
            _ = tokio::time::sleep(STEP_TIMEOUT) => {
                token.cancel();
                // A provider that ignores the cancel is not waited for long.
                tokio::select! {
                    _ = &mut run => {}
                    _ = tokio::time::sleep(CANCEL_GRACE) => {}
                }
                None
            }
        }
    }

    /// One call that waits out usage limits: the run says so, sleeps until the limit resets
    /// (or another account signs in, or [`retry_now`]), then sends the same request again.
    async fn patient(&self, req: &AiStepRequest, progress: &(dyn Fn(String) + Send + Sync), cancel: &CancellationToken) -> Result<String> {
        let mut attempt = 0u32;
        loop {
            let real = !self.is_fake();
            let account = if real { mv_ai::account::fingerprint(req.provider_id) } else { None };
            let Some(r) = self.timed(req.clone(), cancel).await else { return Err(anyhow!(TIMED_OUT)) };
            if r.stopped || cancel.is_cancelled() {
                return Err(anyhow!(CANCELLED));
            }
            let Some(limit) = r.limit else {
                if attempt > 0 {
                    progress("The AI answers again; carrying on.".into());
                }
                return match r.error {
                    Some(e) => Err(anyhow!(e)),
                    None => Ok(r.text),
                };
            };
            let now = now_secs();
            let mut wait = limit.wait_secs(attempt, now);
            if real && !mv_ai::account::tracked(req.provider_id) && mv_ai::provider_def(req.provider_id).kind == mv_ai::AiProviderKind::Cli {
                wait = wait.min(UNTRACKED_WAIT);
            }
            let message = r.error.unwrap_or(limit.message);
            let minutes = wait.div_ceil(60).max(1);
            progress(format!("{} hit a usage limit ({}). Waiting about {minutes} min, then asking again; nothing is skipped.", self.label(), message.trim()));
            attempt += 1;
            let sleep = if real { Duration::from_secs(wait) } else { Duration::from_millis(5) };
            if real {
                tokio::select! {
                    _ = cancel.cancelled() => return Err(anyhow!(CANCELLED)),
                    _ = tokio::time::sleep(sleep) => {}
                    _ = retry_signal().notified() => {}
                    _ = mv_ai::account::changed(req.provider_id, account) => {
                        progress("Another account is signed in to the AI tool: trying again now.".into());
                    }
                }
            } else {
                tokio::select! {
                    _ = cancel.cancelled() => return Err(anyhow!(CANCELLED)),
                    _ = tokio::time::sleep(sleep) => {}
                }
            }
        }
    }

    /// One call that survives a hiccup: an empty answer, a provider message in place of an
    /// answer or a failed call is sent again twice, after 4 s and 12 s. A timeout is not
    /// retried (it would hang again). Only the last failure is returned.
    async fn with_retries(&self, req: &AiStepRequest, progress: &(dyn Fn(String) + Send + Sync), cancel: &CancellationToken) -> Result<String> {
        let mut retry = 0;
        loop {
            let failure = match self.patient(req, progress, cancel).await {
                Ok(text) if text.trim().is_empty() => anyhow!("The AI sent an empty answer."),
                Ok(text) => match mv_ai::limit::provider_error_in(&text) {
                    Some(e) => anyhow!(e),
                    None => return Ok(text),
                },
                Err(e) if is_cancelled(&e) => return Err(e),
                Err(e) if e.to_string() == TIMED_OUT => return Err(e),
                Err(e) => e,
            };
            if retry >= 2 || cancel.is_cancelled() {
                return Err(failure);
            }
            progress(format!("The AI did not answer properly ({failure}); trying again ({} of 2)…", retry + 1));
            tokio::select! {
                _ = cancel.cancelled() => return Err(anyhow!(CANCELLED)),
                _ = tokio::time::sleep(self.backoff(retry)) => {}
            }
            retry += 1;
        }
    }

    /// One system + user turn (with `images` on the user message unless the model is text
    /// only); returns the reply's JSON (lenient parse, see `json`). `progress` gets status lines.
    ///
    /// A reply that is not JSON is answered once with "reply with valid JSON only" (without the
    /// pictures when only its JSON is broken). A reply saying the model cannot see the pictures
    /// is asked again without them. Still not JSON: a [`BadReply`] error ("The AI's answer could
    /// not be read…") carrying the raw reply. Cancelled: an error for which [`is_cancelled`] holds.
    pub async fn ask_json(
        &self,
        system: &str,
        user: String,
        images: Vec<PathBuf>,
        effort: AiEffort,
        progress: &(dyn Fn(String) + Send + Sync),
        cancel: &CancellationToken,
    ) -> Result<Value> {
        let mut images = images;
        if !images.is_empty() && !self.can_see() {
            progress(format!("{} can't see pictures, so it gets the text only.", self.label()));
            images.clear();
        }
        let mut req = self.request(system, user, images, effort);
        let mut raw = self.with_retries(&req, progress, cancel).await?;
        if let Some(v) = crate::json::parse_reply(&raw) {
            return Ok(v);
        }
        if !req.images.is_empty() && says_blind(&raw) {
            progress(format!("{} can't see pictures; asking again with the text only.", self.label()));
            req.images.clear();
            req.request_id.push_str("-text");
            raw = self.with_retries(&req, progress, cancel).await?;
            if let Some(v) = crate::json::parse_reply(&raw) {
                return Ok(v);
            }
        }
        let mut retry = req;
        retry.request_id.push_str("-retry");
        retry.messages.push(AiMessage::assistant(raw.clone()));
        if repairable(&raw) {
            // Only the JSON is broken: mending it needs no second look at the pictures.
            retry.images.clear();
            retry.messages.push(AiMessage::user("Your previous answer was not valid JSON. Send the same answer again as valid JSON only, changing nothing else."));
        } else {
            retry.messages.push(AiMessage::user("Your previous answer was not valid JSON. Reply with JSON only."));
        }
        progress("The AI's answer was not valid JSON; asking once more.".into());
        let raw2 = match self.with_retries(&retry, progress, cancel).await {
            Ok(t) => t,
            Err(e) if is_cancelled(&e) => return Err(e),
            Err(_) => raw.clone(),
        };
        if let Some(v) = crate::json::parse_reply(&raw2) {
            return Ok(v);
        }
        Err(anyhow::Error::new(BadReply { raw: raw2 }))
    }
}
