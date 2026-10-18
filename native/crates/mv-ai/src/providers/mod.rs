//! Provider adapters. Each exposes a `*_step(ctx)` plus a stream accumulator that can be
//! fed recorded events offline (see the tests in each module).

pub mod anthropic;
pub mod cli;
pub mod gemini;
pub mod ollama;
pub mod openai;

use crate::types::{AiStepRequest, Emit, StreamEvent};
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

pub struct StepContext {
    pub req: AiStepRequest,
    pub cancel: CancellationToken,
    pub api_key: Option<String>,
    pub base_url: String,
    pub emit: Emit,
    pub client: reqwest::Client,
    /// Working directory for CLI agents.
    pub workspace: PathBuf,
}

impl StepContext {
    pub(crate) fn status(&self, text: impl Into<String>) {
        (self.emit)(StreamEvent::Status { text: text.into() });
    }
}

pub(crate) fn emit_text(emit: &Emit, delta: &str) {
    emit(StreamEvent::Text { delta: delta.to_string() });
}

pub(crate) fn emit_reasoning(emit: &Emit, delta: &str) {
    emit(StreamEvent::Reasoning { delta: delta.to_string() });
}

pub(crate) fn non_empty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use crate::types::{Emit, StreamEvent};
    use std::sync::{Arc, Mutex};

    /// An emitter that records every event.
    pub fn recorder() -> (Emit, Arc<Mutex<Vec<StreamEvent>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let l = log.clone();
        (Arc::new(move |e| l.lock().unwrap().push(e)), log)
    }
}
