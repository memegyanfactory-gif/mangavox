//! Errors, HTTP helpers and NDJSON / SSE line readers shared by the provider adapters.

use futures_util::StreamExt;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub enum AiError {
    Http { message: String, status: Option<u16> },
    Cancelled,
    Other(String),
}

impl AiError {
    pub fn msg(s: impl Into<String>) -> Self {
        AiError::Other(s.into())
    }
    pub fn http(message: impl Into<String>, status: Option<u16>) -> Self {
        AiError::Http { message: message.into(), status }
    }
    pub fn is_cancelled(&self) -> bool {
        matches!(self, AiError::Cancelled)
    }
}

impl std::fmt::Display for AiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AiError::Http { message, .. } => f.write_str(message),
            AiError::Cancelled => f.write_str("cancelled"),
            AiError::Other(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for AiError {}

impl From<reqwest::Error> for AiError {
    fn from(e: reqwest::Error) -> Self {
        let e = e.without_url();
        // Mirrors Node's "fetch failed" so connection problems read the same everywhere.
        AiError::Other(format!("fetch failed: {e}"))
    }
}

impl From<std::io::Error> for AiError {
    fn from(e: std::io::Error) -> Self {
        AiError::Other(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, AiError>;

/// Build the shared HTTP client.
pub fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("MangaVox")
        .connect_timeout(Duration::from_secs(15))
        .build()
        .expect("reqwest client")
}

fn json_error_message(j: &Value) -> Option<String> {
    let pick = j
        .get("error")
        .and_then(|e| e.get("message"))
        .filter(|v| !v.is_null())
        .or_else(|| j.get("error").filter(|v| !v.is_null()))
        .or_else(|| j.get("message").filter(|v| !v.is_null()))
        .or_else(|| j.get("detail").filter(|v| !v.is_null()))?;
    match pick {
        Value::String(s) if s.is_empty() => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

/// Turn a non-2xx response into a readable message.
pub async fn read_error(res: reqwest::Response) -> String {
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    if let Ok(j) = serde_json::from_str::<Value>(&text) {
        if let Some(msg) = json_error_message(&j) {
            return format!("{}: {}", status.as_u16(), msg);
        }
    }
    let reason = status.canonical_reason().unwrap_or("");
    let snippet: String = text.chars().take(300).collect();
    let mut out = format!("{} {}", status.as_u16(), reason);
    if !snippet.is_empty() {
        out.push_str(": ");
        out.push_str(&snippet);
    }
    out
}

async fn send_cancellable(req: reqwest::RequestBuilder, cancel: &CancellationToken) -> Result<reqwest::Response> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(AiError::Cancelled),
        r = req.send() => Ok(r?),
    }
}

/// POST JSON and return the response, turning non-2xx into an error.
pub async fn post_json(
    client: &reqwest::Client,
    url: &str,
    body: &Value,
    headers: &[(String, String)],
    cancel: &CancellationToken,
) -> Result<reqwest::Response> {
    let mut rb = client.post(url).header("content-type", "application/json").body(body.to_string());
    for (k, v) in headers {
        rb = rb.header(k.as_str(), v.as_str());
    }
    let res = send_cancellable(rb, cancel).await?;
    if !res.status().is_success() {
        let status = res.status().as_u16();
        return Err(AiError::http(read_error(res).await, Some(status)));
    }
    Ok(res)
}

pub async fn get_json<T: DeserializeOwned>(client: &reqwest::Client, url: &str, headers: &[(String, String)], timeout: Duration) -> Result<T> {
    let mut rb = client.get(url).timeout(timeout);
    for (k, v) in headers {
        rb = rb.header(k.as_str(), v.as_str());
    }
    let res = rb.send().await?;
    if !res.status().is_success() {
        let status = res.status().as_u16();
        return Err(AiError::http(read_error(res).await, Some(status)));
    }
    res.json::<T>().await.map_err(AiError::from)
}

/// Incremental newline splitter that is safe across chunk boundaries (including split UTF-8).
#[derive(Default)]
pub struct LineBuffer {
    buf: Vec<u8>,
}

impl LineBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed bytes; returns every completed line (without `\n` / `\r\n`).
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=nl).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            out.push(String::from_utf8_lossy(&line).into_owned());
        }
        out
    }

    /// The unterminated tail, if it has any non-blank content.
    pub fn finish(&mut self) -> Option<String> {
        let s = String::from_utf8_lossy(&self.buf).into_owned();
        self.buf.clear();
        if s.trim().is_empty() {
            None
        } else {
            Some(s)
        }
    }
}

/// Call `f` for each line of a streamed body (NDJSON / SSE). Stops with `Cancelled` on cancel.
pub async fn for_each_line(
    res: reqwest::Response,
    cancel: &CancellationToken,
    mut f: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    let mut stream = res.bytes_stream();
    let mut lb = LineBuffer::new();
    loop {
        let next = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(AiError::Cancelled),
            n = stream.next() => n,
        };
        match next {
            None => break,
            Some(Err(e)) => return Err(e.into()),
            Some(Ok(chunk)) => {
                for line in lb.push(&chunk) {
                    f(&line)?;
                }
            }
        }
    }
    if let Some(rest) = lb.finish() {
        f(&rest)?;
    }
    Ok(())
}

/// Parsed JSON payload of an SSE `data:` line (skips `[DONE]`, keep-alives and non-JSON).
pub fn sse_data(line: &str) -> Option<Value> {
    let data = line.strip_prefix("data:")?.trim();
    if data.is_empty() || data == "[DONE]" {
        return None;
    }
    serde_json::from_str(data).ok()
}

/// Parse a whole SSE document (used by tests and recorded fixtures).
pub fn parse_sse_str(text: &str) -> Vec<Value> {
    text.lines().filter_map(sse_data).collect()
}

pub fn safe_json_parse(text: &str) -> serde_json::Map<String, Value> {
    if text.is_empty() {
        return Default::default();
    }
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(m)) => m,
        _ => Default::default(),
    }
}

static CALL_SEQ: AtomicU64 = AtomicU64::new(0);

fn base36(mut n: u128) -> String {
    if n == 0 {
        return "0".into();
    }
    let mut digits = Vec::new();
    while n > 0 {
        digits.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(n % 36) as usize]);
        n /= 36;
    }
    digits.reverse();
    String::from_utf8(digits).unwrap()
}

pub fn now_millis() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

pub fn new_call_id() -> String {
    let seq = (CALL_SEQ.fetch_add(1, Ordering::Relaxed) + 1) % 1_000_000;
    format!("call_{}_{}", base36(now_millis()), seq)
}

/// Header list helper.
pub fn hdr(k: &str, v: impl Into<String>) -> (String, String) {
    (k.to_string(), v.into())
}

/// JS-style `String(v)` for the JSON scalars we meet in CLI events.
pub fn js_str(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(o) => o.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_buffer_handles_split_chunks_crlf_and_utf8() {
        let mut lb = LineBuffer::new();
        let bytes = "héllo\r\nwörld\nrest".as_bytes();
        let mut lines = lb.push(&bytes[..2]); // splits inside the 'é'
        lines.extend(lb.push(&bytes[2..9]));
        lines.extend(lb.push(&bytes[9..]));
        assert_eq!(lines, vec!["héllo", "wörld"]);
        assert_eq!(lb.finish().as_deref(), Some("rest"));
        assert_eq!(lb.finish(), None);
    }

    #[test]
    fn sse_skips_done_and_garbage() {
        let v = parse_sse_str("event: x\ndata: {\"a\":1}\n\ndata: [DONE]\ndata: not json\n: keepalive\ndata:{\"b\":2}\n");
        assert_eq!(v.len(), 2);
        assert_eq!(v[1]["b"], 2);
    }

    #[test]
    fn error_message_extraction() {
        let j: Value = serde_json::from_str(r#"{"error":{"message":"bad key"}}"#).unwrap();
        assert_eq!(json_error_message(&j).as_deref(), Some("bad key"));
        let j: Value = serde_json::from_str(r#"{"error":"nope"}"#).unwrap();
        assert_eq!(json_error_message(&j).as_deref(), Some("nope"));
        let j: Value = serde_json::from_str(r#"{"detail":"x"}"#).unwrap();
        assert_eq!(json_error_message(&j).as_deref(), Some("x"));
    }

    #[test]
    fn call_ids_are_unique() {
        assert_ne!(new_call_id(), new_call_id());
    }
}
