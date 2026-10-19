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

/// An image ready to embed in an API request.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageData {
    pub mime: &'static str,
    pub b64: String,
}

/// Images larger than this are skipped (with a Status event).
pub const MAX_IMAGE_BYTES: u64 = 4_500_000;

pub fn image_mime(path: &std::path::Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        _ => None,
    }
}

/// Read + base64 the request's images. Unreadable, unsupported or oversized files are skipped with a Status event.
pub fn load_images(paths: &[PathBuf], emit: &Emit) -> Vec<ImageData> {
    use base64::Engine;
    let name = |p: &std::path::Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut out = Vec::new();
    for p in paths {
        let Some(mime) = image_mime(p) else {
            emit(StreamEvent::Status { text: format!("Skipped {} (unsupported image type)", name(p)) });
            continue;
        };
        match std::fs::metadata(p) {
            Ok(m) if m.len() > MAX_IMAGE_BYTES => {
                emit(StreamEvent::Status { text: format!("Skipped {} (larger than 4.5 MB)", name(p)) });
                continue;
            }
            Err(e) => {
                emit(StreamEvent::Status { text: format!("Skipped {} ({e})", name(p)) });
                continue;
            }
            _ => {}
        }
        match std::fs::read(p) {
            Ok(bytes) => out.push(ImageData { mime, b64: base64::engine::general_purpose::STANDARD.encode(bytes) }),
            Err(e) => emit(StreamEvent::Status { text: format!("Skipped {} ({e})", name(p)) }),
        }
    }
    out
}

/// The last message object whose `role` is "user".
pub(crate) fn last_user_mut(msgs: &mut [serde_json::Value]) -> Option<&mut serde_json::Value> {
    msgs.iter_mut().rev().find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_images_and_skips_bad_ones() {
        let dir = tempfile::tempdir().unwrap();
        let ok = dir.path().join("a.JPG");
        std::fs::write(&ok, b"ABC").unwrap();
        let big = dir.path().join("big.png");
        std::fs::write(&big, vec![0u8; MAX_IMAGE_BYTES as usize + 1]).unwrap();
        let txt = dir.path().join("x.txt");
        std::fs::write(&txt, b"x").unwrap();
        let (emit, log) = testutil::recorder();
        let out = load_images(&[ok, big, txt, dir.path().join("missing.png")], &emit);
        assert_eq!(out, vec![ImageData { mime: "image/jpeg", b64: "QUJD".into() }]);
        assert_eq!(log.lock().unwrap().len(), 3);
    }
}
