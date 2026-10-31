//! `mangavox --mcp-bridge`: the app's tools as a stdio MCP server, for CLIs that load MCP servers
//! only from their own global settings (Grok, Antigravity; see `mv_ai::providers::cli_mcp`).
//!
//! It reads JSON-RPC lines on stdin and forwards each one to the MCP server of the turn that is
//! running now, found in [`crate::mcp::endpoint_file`] at every message, and writes the answers
//! to stdout. With no turn running it still starts cleanly: `initialize` is answered here, the
//! tool list is empty and a call gets one explanatory error. The CLI is told when the turn
//! changes (`notifications/tools/list_changed`), in case it keeps the bridge running.
use std::io::{BufRead, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mv_ai::McpEndpoint;
use serde_json::{json, Value};

use crate::mcp::{handle, read_endpoint, Shared};

/// The command-line flag that starts the bridge.
pub use mv_ai::providers::cli_mcp::BRIDGE_FLAG as FLAG;

/// The answer to a tool call while no turn is running.
pub const NO_TURN: &str = "MangaVox is not running an assistant reply right now. Its tools work only while MangaVox's assistant uses this agent.";

/// Runs the bridge until stdin closes. Returns the process exit code.
pub fn run() -> i32 {
    let path = crate::mcp::endpoint_file();
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let initialized = Arc::new(AtomicBool::new(false));
    {
        let (path, out, initialized) = (path.clone(), out.clone(), initialized.clone());
        std::thread::spawn(move || watch(&path, &out, &initialized));
    }
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if line.contains("\"initialize\"") || line.contains("notifications/initialized") {
            initialized.store(true, Ordering::SeqCst);
        }
        // One thread per message: a tool call can wait on the user's OK for a long time.
        let (path, out) = (path.clone(), out.clone());
        std::thread::spawn(move || {
            if let Some(reply) = answer(&line, read_endpoint(&path), post) {
                write_line(&out, &reply);
            }
        });
    }
    0
}

fn write_line(out: &Mutex<std::io::Stdout>, line: &str) {
    let mut o = out.lock().unwrap_or_else(|e| e.into_inner());
    let _ = writeln!(o, "{line}");
    let _ = o.flush();
}

/// Tells the client to list the tools again whenever the published turn changes.
fn watch(path: &Path, out: &Mutex<std::io::Stdout>, initialized: &AtomicBool) {
    let key = || read_endpoint(path).map(|e| e.token);
    let mut last = key();
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let now = key();
        if now != last {
            last = now;
            if initialized.load(Ordering::SeqCst) {
                write_line(out, &json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" }).to_string());
            }
        }
    }
}

/// The line owed back for one client line (`None` for a notification). `post` sends the line to
/// the turn's server: `Ok(Some(body))` is its answer, `Ok(None)` an accepted notification, and an
/// error (no server there any more) means the turn is gone.
pub fn answer(line: &str, endpoint: Option<McpEndpoint>, post: impl Fn(&McpEndpoint, &str) -> Result<Option<String>, String>) -> Option<String> {
    let msg: Value = match serde_json::from_str(line) {
        Ok(m) => m,
        Err(e) => return Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("Parse error: {e}") } }).to_string()),
    };
    let reply = match endpoint.map(|ep| post(&ep, line)) {
        Some(Ok(None)) => None,
        Some(Ok(Some(body))) => match serde_json::from_str::<Value>(&body) {
            Ok(v) => Some(v),
            Err(_) => idle(&msg),
        },
        None | Some(Err(_)) => idle(&msg),
    };
    let mut reply = reply?;
    // The tools change with the turn: say so, so the client listens for list_changed.
    if msg.get("method").and_then(Value::as_str) == Some("initialize") {
        if let Some(tools) = reply.pointer_mut("/result/capabilities/tools").and_then(Value::as_object_mut) {
            tools.insert("listChanged".into(), Value::Bool(true));
        }
    }
    Some(reply.to_string())
}

/// The answer with no turn running: no tools, and calls explain why.
fn idle(msg: &Value) -> Option<Value> {
    if msg.get("method").and_then(Value::as_str) == Some("tools/call") {
        let id = msg.get("id").filter(|i| !i.is_null())?.clone();
        return Some(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32603, "message": NO_TURN } }));
    }
    let (shared, rx) = Shared::new(vec![]);
    drop(rx);
    futures::executor::block_on(handle(&shared, msg.clone()))
}

/// One JSON-RPC POST to a turn's server on 127.0.0.1.
pub fn post(ep: &McpEndpoint, body: &str) -> Result<Option<String>, String> {
    let rest = ep.url.strip_prefix("http://").ok_or("not an http URL")?;
    let (host, path) = rest.split_once('/').map_or((rest, "/".to_string()), |(h, p)| (h, format!("/{p}")));
    let addr: SocketAddr = host.parse().map_err(|_| format!("bad address {host}"))?;
    if !addr.ip().is_loopback() {
        return Err("not a local address".into());
    }
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| e.to_string())?;
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        ep.token,
        body.len()
    );
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    // The server ends the connection once it sees no further request.
    let _ = s.shutdown(std::net::Shutdown::Write);
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    let raw = String::from_utf8_lossy(&raw);
    let (head, body) = raw.split_once("\r\n\r\n").ok_or("no HTTP response")?;
    match head.split_whitespace().nth(1) {
        Some("200") => Ok(Some(body.to_string())),
        Some("202") => Ok(None),
        status => Err(format!("HTTP {}", status.unwrap_or("?"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn ep() -> McpEndpoint {
        McpEndpoint { url: "http://127.0.0.1:50123/mcp".into(), token: "t".into() }
    }

    const INIT: &str = r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"grok","version":"1"}}}"#;
    const LIST: &str = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    const CALL: &str = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"navigate","arguments":{"page":"library"}}}"#;
    const NOTE: &str = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;

    #[test]
    fn lines_go_to_the_running_turn() {
        let sent = RefCell::new(vec![]);
        let fake = |e: &McpEndpoint, body: &str| {
            sent.borrow_mut().push((e.url.clone(), body.to_string()));
            let v: Value = serde_json::from_str(body).unwrap();
            Ok(match v["method"].as_str().unwrap() {
                "initialize" => Some(r#"{"jsonrpc":"2.0","id":0,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"mangavox"}}}"#.to_string()),
                "tools/list" => Some(r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"navigate"}]}}"#.to_string()),
                "tools/call" => Some(r#"{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"{\"ok\":true}"}],"isError":false}}"#.to_string()),
                _ => None,
            })
        };
        let init: Value = serde_json::from_str(&answer(INIT, Some(ep()), fake).unwrap()).unwrap();
        assert_eq!(init["result"]["capabilities"]["tools"]["listChanged"], true);
        assert!(answer(NOTE, Some(ep()), fake).is_none());
        let list: Value = serde_json::from_str(&answer(LIST, Some(ep()), fake).unwrap()).unwrap();
        assert_eq!(list["result"]["tools"][0]["name"], "navigate");
        let call: Value = serde_json::from_str(&answer(CALL, Some(ep()), fake).unwrap()).unwrap();
        assert_eq!(call["result"]["isError"], false);
        let sent = sent.borrow();
        assert_eq!(sent.len(), 4);
        assert!(sent.iter().all(|(url, _)| url == "http://127.0.0.1:50123/mcp"));
        assert_eq!(sent[3].1, CALL);
    }

    #[test]
    fn without_a_turn_the_cli_still_starts() {
        let never = |_: &McpEndpoint, _: &str| -> Result<Option<String>, String> { panic!("no turn to post to") };
        let init: Value = serde_json::from_str(&answer(INIT, None, never).unwrap()).unwrap();
        assert_eq!((init["id"].clone(), init["result"]["serverInfo"]["name"].clone()), (json!(0), json!("mangavox")));
        assert_eq!(init["result"]["capabilities"]["tools"]["listChanged"], true);
        assert!(answer(NOTE, None, never).is_none());
        let list: Value = serde_json::from_str(&answer(LIST, None, never).unwrap()).unwrap();
        assert_eq!(list["result"]["tools"], json!([]));
        let call: Value = serde_json::from_str(&answer(CALL, None, never).unwrap()).unwrap();
        assert_eq!((call["id"].clone(), call["error"]["message"].clone()), (json!(2), json!(NO_TURN)));
        let ping: Value = serde_json::from_str(&answer(r#"{"jsonrpc":"2.0","id":"p","method":"ping"}"#, None, never).unwrap()).unwrap();
        assert_eq!(ping["result"], json!({}));
        let bad: Value = serde_json::from_str(&answer("{nope", None, never).unwrap()).unwrap();
        assert_eq!(bad["error"]["code"], -32700);
        // A published turn whose server is gone (the app quit mid-turn) counts as none.
        let gone = |_: &McpEndpoint, _: &str| Err("connection refused".to_string());
        let call: Value = serde_json::from_str(&answer(CALL, Some(ep()), gone).unwrap()).unwrap();
        assert_eq!(call["error"]["message"], NO_TURN);
        let list: Value = serde_json::from_str(&answer(LIST, Some(ep()), gone).unwrap()).unwrap();
        assert_eq!(list["result"]["tools"], json!([]));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn posts_reach_a_real_turn_server_on_loopback() {
        let spec = mv_ai::AiToolSpec { name: "navigate".into(), description: "Go".into(), parameters: json!({ "type": "object", "properties": {} }) };
        let (server, _rx) = crate::mcp::McpServer::start(vec![spec]).await.unwrap();
        let ep = server.endpoint();
        let (e2, e3) = (ep.clone(), ep.clone());
        let list = tokio::task::spawn_blocking(move || answer(LIST, Some(e2), post)).await.unwrap().unwrap();
        assert!(list.contains("\"navigate\""), "{list}");
        assert!(server.connected());
        assert!(tokio::task::spawn_blocking(move || post(&e3, NOTE)).await.unwrap().unwrap().is_none());
        let wrong = McpEndpoint { token: "nope".into(), ..ep };
        assert_eq!(tokio::task::spawn_blocking(move || post(&wrong, LIST)).await.unwrap(), Err("HTTP 401".to_string()));
    }
}
