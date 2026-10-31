//! The app's tools as an MCP server for the CLI agents (Claude Code, Codex, Gemini CLI, OpenCode,
//! Grok, Antigravity), so they call MangaVox tools from their own agent loop: one CLI process per
//! chat turn instead of one per round plus a text protocol.
//!
//! Streamable HTTP (JSON responses only) on 127.0.0.1 with a random port and a random bearer
//! token, one server per turn. It lists the turn's tools and hands every call to the agent loop
//! ([`McpCall`]), which runs it exactly like a native tool call: same risk, same approval in Ask
//! mode, same step row in the chat.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use futures::channel::{mpsc, oneshot};
use mv_ai::{AiToolSpec, CancellationToken, McpEndpoint};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// Answered when a client asks for no particular version.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// Request bodies above this are refused.
const MAX_BODY: usize = 4 << 20;

/// A `tools/call` for the agent loop; `reply` takes the result (an `{"error": ...}` object marks a failure).
pub struct McpCall {
    pub name: String,
    pub args: Map<String, Value>,
    pub reply: oneshot::Sender<Value>,
}

/// What the JSON-RPC side shares with the server's owner.
pub(crate) struct Shared {
    tools: Mutex<Vec<AiToolSpec>>,
    calls: mpsc::UnboundedSender<McpCall>,
    listed: AtomicBool,
}

impl Shared {
    pub(crate) fn new(tools: Vec<AiToolSpec>) -> (Self, mpsc::UnboundedReceiver<McpCall>) {
        let (calls, rx) = mpsc::unbounded();
        (Shared { tools: Mutex::new(tools), calls, listed: AtomicBool::new(false) }, rx)
    }
}

/// A running server; dropping it stops it.
pub struct McpServer {
    endpoint: McpEndpoint,
    shared: Arc<Shared>,
    stop: CancellationToken,
}

impl McpServer {
    /// Starts a server for `tools` on a free loopback port. Calls arrive on the returned receiver.
    pub async fn start(tools: Vec<AiToolSpec>) -> std::io::Result<(McpServer, mpsc::UnboundedReceiver<McpCall>)> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        let mut key = [0u8; 24];
        getrandom::fill(&mut key).map_err(|e| std::io::Error::other(e.to_string()))?;
        let token: String = key.iter().map(|b| format!("{b:02x}")).collect();
        let (shared, rx) = Shared::new(tools);
        let shared = Arc::new(shared);
        let stop = CancellationToken::new();
        let (s, t, st) = (shared.clone(), token.clone(), stop.clone());
        tokio::spawn(async move {
            loop {
                let conn = tokio::select! {
                    _ = st.cancelled() => break,
                    c = listener.accept() => c,
                };
                let Ok((stream, peer)) = conn else { continue };
                if !peer.ip().is_loopback() {
                    continue;
                }
                let (s, t, st) = (s.clone(), t.clone(), st.clone());
                tokio::spawn(async move {
                    tokio::select! {
                        _ = st.cancelled() => {}
                        _ = serve_conn(stream, &s, &t) => {}
                    }
                });
            }
        });
        Ok((McpServer { endpoint: McpEndpoint { url: format!("http://127.0.0.1:{port}/mcp"), token }, shared, stop }, rx))
    }

    pub fn endpoint(&self) -> McpEndpoint {
        self.endpoint.clone()
    }

    /// Replaces the listed tools (after `load_tools`), for a client that lists them again.
    pub fn set_tools(&self, tools: Vec<AiToolSpec>) {
        *self.shared.tools.lock().unwrap_or_else(|e| e.into_inner()) = tools;
    }

    /// A client listed the tools: the CLI really loaded the server.
    pub fn connected(&self) -> bool {
        self.shared.listed.load(Ordering::SeqCst)
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

// ── JSON-RPC ─────────────────────────────────────────────────────────────────────────

/// Handles one JSON-RPC message (or a batch). `None` when nothing is owed back (notifications).
pub(crate) async fn handle(shared: &Shared, msg: Value) -> Option<Value> {
    if let Value::Array(batch) = msg {
        let mut out = vec![];
        for m in batch {
            out.extend(Box::pin(handle(shared, m)).await);
        }
        return (!out.is_empty()).then_some(Value::Array(out));
    }
    let method = msg.get("method").and_then(Value::as_str)?;
    // No id: a notification (`notifications/initialized`, `notifications/cancelled`).
    let id = msg.get("id").filter(|i| !i.is_null())?.clone();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => {
            // Echo the client's version: the methods served here are the same in every one.
            let version = params.get("protocolVersion").and_then(Value::as_str).filter(|v| v.len() == 10).unwrap_or(PROTOCOL_VERSION);
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "mangavox", "title": "MangaVox", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Tools that operate the MangaVox app. The app asks the user before actions where needed; if a result says the user declined, do not retry.",
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => {
            shared.listed.store(true, Ordering::SeqCst);
            let tools = shared.tools.lock().unwrap_or_else(|e| e.into_inner()).clone();
            Ok(json!({ "tools": tools.iter().map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": t.parameters })).collect::<Vec<_>>() }))
        }
        "tools/call" => call(shared, &params).await,
        "resources/list" => Ok(json!({ "resources": [] })),
        "resources/templates/list" => Ok(json!({ "resourceTemplates": [] })),
        "prompts/list" => Ok(json!({ "prompts": [] })),
        other => Err((-32601, format!("Method not found: {other}"))),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    })
}

async fn call(shared: &Shared, params: &Value) -> Result<Value, (i64, String)> {
    let name = params.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).ok_or((-32602, "params.name is required".to_string()))?;
    let args = match params.get("arguments") {
        Some(Value::Object(a)) => a.clone(),
        None | Some(Value::Null) => Map::new(),
        Some(_) => return Err((-32602, "params.arguments must be an object".into())),
    };
    let (reply, rx) = oneshot::channel();
    shared.calls.unbounded_send(McpCall { name: name.to_string(), args, reply }).map_err(|_| (-32603, "This chat turn has ended".to_string()))?;
    let value = rx.await.map_err(|_| (-32603, "This chat turn has ended".to_string()))?;
    let is_error = value.get("error").is_some();
    Ok(json!({ "content": [{ "type": "text", "text": value.to_string() }], "isError": is_error }))
}

// ── HTTP ─────────────────────────────────────────────────────────────────────────────

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// Serves requests on one connection (keep-alive) until it closes or misbehaves.
async fn serve_conn(stream: TcpStream, shared: &Shared, token: &str) {
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    while let Some(req) = read_request(&mut read).await {
        let (status, body) = respond(shared, token, req).await;
        if write.write_all(&http_response(status, body.as_deref())).await.is_err() {
            break;
        }
    }
}

/// Status line and optional JSON body for one request.
async fn respond(shared: &Shared, token: &str, req: Request) -> (&'static str, Option<String>) {
    // Browsers name an Origin: only a page on this machine may talk to the server (DNS rebinding).
    let local = |v: &str| {
        let host = v.trim_start_matches("http://").trim_start_matches("https://");
        host.starts_with("127.0.0.1") || host.starts_with("localhost") || host.starts_with("[::1]")
    };
    if req.header("origin").is_some_and(|o| !local(o)) || req.header("host").is_some_and(|h| !local(h)) {
        return ("403 Forbidden", None);
    }
    if !req.header("authorization").and_then(|a| a.strip_prefix("Bearer ")).is_some_and(|t| same(t.trim(), token)) {
        return ("401 Unauthorized", None);
    }
    if req.path.split('?').next() != Some("/mcp") {
        return ("404 Not Found", None);
    }
    if req.method != "POST" {
        // No server-to-client stream (GET) and no sessions to end (DELETE).
        return ("405 Method Not Allowed", None);
    }
    let msg = match serde_json::from_slice::<Value>(&req.body) {
        Ok(m) => m,
        Err(e) => return ("400 Bad Request", Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("Parse error: {e}") } }).to_string())),
    };
    match handle(shared, msg).await {
        Some(v) => ("200 OK", Some(v.to_string())),
        None => ("202 Accepted", None),
    }
}

/// Compares the whole token without stopping at the first difference.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

fn http_response(status: &str, body: Option<&str>) -> Vec<u8> {
    let body = body.unwrap_or("");
    let ty = if body.is_empty() { "" } else { "Content-Type: application/json\r\n" };
    format!("HTTP/1.1 {status}\r\n{ty}Content-Length: {}\r\nCache-Control: no-store\r\n\r\n{body}", body.len()).into_bytes()
}

async fn read_request<R: tokio::io::AsyncBufRead + Unpin>(r: &mut R) -> Option<Request> {
    let mut line = String::new();
    // Skip blank lines between keep-alive requests.
    while line.trim().is_empty() {
        line.clear();
        if r.read_line(&mut line).await.ok()? == 0 {
            return None;
        }
    }
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next()?.to_string(), parts.next()?.to_string());
    let mut headers = vec![];
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).await.ok()? == 0 || headers.len() > 100 {
            return None;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        let (k, v) = h.split_once(':')?;
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    let get = |n: &str| headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.clone());
    let mut body = vec![];
    if get("transfer-encoding").is_some_and(|t| t.eq_ignore_ascii_case("chunked")) {
        loop {
            let mut size = String::new();
            r.read_line(&mut size).await.ok()?;
            let n = usize::from_str_radix(size.trim().split(';').next()?, 16).ok()?;
            if n == 0 {
                let mut end = String::new();
                r.read_line(&mut end).await.ok()?;
                break;
            }
            if body.len() + n > MAX_BODY {
                return None;
            }
            let start = body.len();
            body.resize(start + n, 0);
            r.read_exact(&mut body[start..]).await.ok()?;
            let mut crlf = [0u8; 2];
            r.read_exact(&mut crlf).await.ok()?;
        }
    } else {
        let n: usize = get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
        if n > MAX_BODY {
            return None;
        }
        body.resize(n, 0);
        r.read_exact(&mut body).await.ok()?;
    }
    Some(Request { method, path, headers, body })
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    fn spec(name: &str) -> AiToolSpec {
        AiToolSpec { name: name.into(), description: format!("{name} tool"), parameters: json!({ "type": "object", "properties": {}, "required": [] }) }
    }

    #[tokio::test]
    async fn initialize_list_and_notifications() {
        let (shared, _rx) = Shared::new(vec![spec("navigate")]);
        let init = handle(&shared, json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } } })).await.unwrap();
        assert_eq!(init["id"], 1);
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert!(init["result"]["capabilities"]["tools"].is_object());
        assert_eq!(init["result"]["serverInfo"]["name"], "mangavox");
        assert!(handle(&shared, json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await.is_none());
        assert!(!shared.listed.load(Ordering::SeqCst));
        let list = handle(&shared, json!({ "jsonrpc": "2.0", "id": "a", "method": "tools/list" })).await.unwrap();
        assert_eq!(list["id"], "a");
        assert_eq!(list["result"]["tools"][0]["name"], "navigate");
        assert_eq!(list["result"]["tools"][0]["inputSchema"]["type"], "object");
        assert!(shared.listed.load(Ordering::SeqCst));
        let unknown = handle(&shared, json!({ "jsonrpc": "2.0", "id": 2, "method": "nope" })).await.unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
        // A batch answers its requests only.
        let batch = handle(&shared, json!([{ "jsonrpc": "2.0", "id": 3, "method": "ping" }, { "jsonrpc": "2.0", "method": "notifications/cancelled" }])).await.unwrap();
        assert_eq!(batch.as_array().unwrap().len(), 1);
        assert_eq!(batch[0]["result"], json!({}));
    }

    #[tokio::test]
    async fn calls_go_to_the_agent_loop_and_errors_are_flagged() {
        let (shared, mut rx) = Shared::new(vec![]);
        let loop_side = tokio::spawn(async move {
            let c = rx.next().await.unwrap();
            assert_eq!((c.name.as_str(), &c.args["page"]), ("navigate", &json!("library")));
            c.reply.send(json!({ "ok": true })).unwrap();
            let c = rx.next().await.unwrap();
            c.reply.send(json!({ "error": "The user declined this action." })).unwrap();
        });
        let ok = handle(&shared, json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "navigate", "arguments": { "page": "library" } } })).await.unwrap();
        assert_eq!(ok["result"]["isError"], false);
        assert_eq!(serde_json::from_str::<Value>(ok["result"]["content"][0]["text"].as_str().unwrap()).unwrap(), json!({ "ok": true }));
        let denied = handle(&shared, json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "studio_generate" } })).await.unwrap();
        assert_eq!(denied["result"]["isError"], true);
        loop_side.await.unwrap();
        let bad = handle(&shared, json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "arguments": {} } })).await.unwrap();
        assert_eq!(bad["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn a_finished_turn_answers_calls_with_an_error() {
        let (shared, rx) = Shared::new(vec![]);
        drop(rx);
        let r = handle(&shared, json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "x" } })).await.unwrap();
        assert_eq!(r["error"]["code"], -32603);
    }

    /// One raw HTTP exchange with the server.
    async fn post(url: &str, auth: Option<&str>, extra: &str, body: &str) -> String {
        let addr = url.trim_start_matches("http://").split('/').next().unwrap().to_string();
        let mut s = TcpStream::connect(&addr).await.unwrap();
        let auth = auth.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
        let req = format!("POST /mcp HTTP/1.1\r\nHost: {addr}\r\n{auth}{extra}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        s.write_all(req.as_bytes()).await.unwrap();
        let mut buf = vec![0u8; 65536];
        let n = s.read(&mut buf).await.unwrap();
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }

    #[tokio::test]
    async fn http_needs_the_token_and_a_local_origin() {
        let (server, _rx) = McpServer::start(vec![spec("navigate")]).await.unwrap();
        let ep = server.endpoint();
        assert!(ep.url.starts_with("http://127.0.0.1:") && ep.url.ends_with("/mcp"));
        assert_eq!(ep.token.len(), 48);
        let list = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        assert!(post(&ep.url, None, "", list).await.starts_with("HTTP/1.1 401"));
        assert!(post(&ep.url, Some("wrong"), "", list).await.starts_with("HTTP/1.1 401"));
        assert!(post(&ep.url, Some(&ep.token), "Origin: https://evil.example\r\n", list).await.starts_with("HTTP/1.1 403"));
        assert!(!server.connected());
        let ok = post(&ep.url, Some(&ep.token), "Origin: http://localhost\r\n", list).await;
        assert!(ok.starts_with("HTTP/1.1 200") && ok.contains("application/json") && ok.contains("\"navigate\""), "{ok}");
        assert!(server.connected());
        let note = post(&ep.url, Some(&ep.token), "", r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).await;
        assert!(note.starts_with("HTTP/1.1 202"));
        assert!(post(&ep.url, Some(&ep.token), "", "{not json").await.starts_with("HTTP/1.1 400"));
        // Tools loaded later show in the next list.
        server.set_tools(vec![spec("video_timeline")]);
        assert!(post(&ep.url, Some(&ep.token), "", list).await.contains("video_timeline"));
        // Stopped with the turn.
        let url = ep.url.clone();
        drop(server);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let addr = url.trim_start_matches("http://").split('/').next().unwrap().to_string();
        assert!(TcpStream::connect(&addr).await.is_err());
    }

    #[tokio::test]
    async fn chunked_bodies_and_keep_alive() {
        let (shared, _rx) = Shared::new(vec![]);
        let raw = "POST /mcp HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n{\"id\"\r\n1e\r\n:1,\"jsonrpc\":\"2.0\",\"method\":\"p\r\n4\r\ning\"\r\n1\r\n}\r\n0\r\n\r\nGET /mcp HTTP/1.1\r\n\r\n";
        let mut r = BufReader::new(raw.as_bytes());
        let first = read_request(&mut r).await.unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&first.body).unwrap()["method"], "ping");
        let second = read_request(&mut r).await.unwrap();
        assert_eq!((second.method.as_str(), second.body.len()), ("GET", 0));
        assert!(read_request(&mut r).await.is_none());
        let (status, _) = respond(&shared, "t", Request { method: "GET".into(), path: "/mcp".into(), headers: vec![("Authorization".into(), "Bearer t".into())], body: vec![] }).await;
        assert_eq!(status, "405 Method Not Allowed");
    }
}
