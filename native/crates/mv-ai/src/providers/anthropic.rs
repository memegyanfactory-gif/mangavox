//! Claude Messages API over plain HTTP.
//!
//! Behaviours that must not change: adaptive thinking (`thinking: {type:"adaptive",
//! display:"summarized"}` + `output_config.effort`, never `budget_tokens`) only for current models;
//! raw assistant blocks are replayed unchanged; tools are sent with `eager_input_streaming`;
//! tool calls are ignored when the response stopped on `max_tokens` or `refusal`.

use super::{emit_reasoning, emit_text, non_empty, ImageData, StepContext};
use crate::stream::{for_each_line, get_json, hdr, post_json, safe_json_parse, sse_data, AiError, Result};
use crate::types::{AiMessage, AiModel, AiProviderId, AiRole, AiStepRequest, AiStepResult, AiToolCall, AiUsage, Emit};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Duration;

const VERSION: &str = "2023-06-01";

/// Models on adaptive thinking + `output_config.effort` (budget_tokens is rejected there).
pub fn is_adaptive_model(model: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"claude-(fable|mythos)-|claude-(opus|sonnet|haiku)-(5|[6-9])|claude-opus-4-[6-9]|claude-sonnet-4-6").unwrap())
        .is_match(model)
}

pub struct AnthropicMessages {
    pub system: String,
    pub messages: Vec<Value>,
}

pub fn to_anthropic(messages: &[AiMessage]) -> AnthropicMessages {
    let system = messages.iter().filter(|m| m.role == AiRole::System).map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n\n");
    let mut out: Vec<(&'static str, Vec<Value>)> = Vec::new();
    fn push(out: &mut Vec<(&'static str, Vec<Value>)>, role: &'static str, block: Value) {
        match out.last_mut() {
            Some((r, c)) if *r == role => c.push(block),
            _ => out.push((role, vec![block])),
        }
    }
    for m in messages {
        match m.role {
            AiRole::System => {}
            AiRole::Tool => push(
                &mut out,
                "user",
                json!({ "type": "tool_result", "tool_use_id": m.tool_call_id, "content": m.content }),
            ),
            AiRole::Assistant => {
                // Replay the original blocks (thinking + signature, text, tool_use) unchanged when we have them.
                if let Some(Value::Array(blocks)) = &m.raw {
                    for b in blocks {
                        push(&mut out, "assistant", b.clone());
                    }
                    continue;
                }
                if !m.content.is_empty() {
                    push(&mut out, "assistant", json!({ "type": "text", "text": m.content }));
                }
                for c in m.tool_calls.iter().flatten() {
                    push(&mut out, "assistant", json!({ "type": "tool_use", "id": c.id, "name": c.name, "input": c.args }));
                }
            }
            AiRole::User => {
                if !m.content.is_empty() {
                    push(&mut out, "user", json!({ "type": "text", "text": m.content }));
                }
            }
        }
    }
    AnthropicMessages {
        system,
        messages: out.into_iter().map(|(role, content)| json!({ "role": role, "content": content })).collect(),
    }
}

pub fn build_body(req: &AiStepRequest) -> Value {
    build_body_with(req, &[])
}

/// Like [`build_body`], attaching `images` (before the text) to the last user message.
pub fn build_body_with(req: &AiStepRequest, images: &[ImageData]) -> Value {
    let AnthropicMessages { system, mut messages } = to_anthropic(&req.messages);
    if let Some(Value::Array(c)) = super::last_user_mut(&mut messages).and_then(|m| m.get_mut("content")) {
        let blocks = images.iter().map(|i| json!({ "type": "image", "source": { "type": "base64", "media_type": i.mime, "data": i.b64 } }));
        c.splice(0..0, blocks);
    }
    let mut body = json!({
        "model": req.model,
        "max_tokens": 32000,
        "stream": true,
        "messages": messages,
    });
    // Prompt caching: the tool list, the system prompt and the conversation so far are cache
    // breakpoints, so the rounds of one turn (and the next turn within minutes) re-read them at a
    // tenth of the price instead of paying for them again.
    if !system.is_empty() {
        body["system"] = json!([{ "type": "text", "text": system, "cache_control": { "type": "ephemeral" } }]);
    }
    if let Some(last) = messages_last_block(&mut body) {
        last["cache_control"] = json!({ "type": "ephemeral" });
    }
    if is_adaptive_model(&req.model) {
        body["thinking"] = json!({ "type": "adaptive", "display": "summarized" });
        if let Some(e) = req.effort {
            body["output_config"] = json!({ "effort": e.as_str() });
        }
    }
    if let Some(tools) = req.tools.as_ref().filter(|t| !t.is_empty()) {
        body["tools"] = tools
            .iter()
            .map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.parameters, "eager_input_streaming": true }))
            .collect();
        if let Some(last) = body["tools"].as_array_mut().and_then(|t| t.last_mut()) {
            last["cache_control"] = json!({ "type": "ephemeral" });
        }
    }
    body
}

/// The last content block of the last message when that message is the user's (a text or a
/// tool result; assistant thinking blocks cannot carry a breakpoint).
fn messages_last_block(body: &mut Value) -> Option<&mut Value> {
    let last = body["messages"].as_array_mut()?.last_mut()?;
    if last["role"] != "user" {
        return None;
    }
    last["content"].as_array_mut()?.last_mut()
}

#[derive(Default)]
pub struct AnthropicAcc {
    text: String,
    reasoning: String,
    /// content block by index, plus its accumulated partial tool-input JSON.
    blocks: BTreeMap<u64, (Map<String, Value>, String)>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    stop_reason: Option<String>,
}

fn append(block: &mut Map<String, Value>, key: &str, add: &str) {
    let cur = block.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    block.insert(key.to_string(), Value::String(cur + add));
}

impl AnthropicAcc {
    /// Feed one parsed SSE `data:` payload.
    pub fn feed(&mut self, ev: &Value, emit: &Emit) -> Result<()> {
        let idx = ev.get("index").and_then(Value::as_u64).unwrap_or(0);
        match ev.get("type").and_then(Value::as_str).unwrap_or("") {
            // With prompt caching `input_tokens` is only the uncached part: cache writes and reads
            // are input too (counted once each, as Claude Code's own totals are).
            "message_start" => {
                self.input_tokens = ev.pointer("/message/usage/input_tokens").and_then(Value::as_u64).map(|n| {
                    let u = |k: &str| ev.pointer(&format!("/message/usage/{k}")).and_then(Value::as_u64).unwrap_or(0);
                    n + u("cache_creation_input_tokens") + u("cache_read_input_tokens")
                })
            }
            "content_block_start" => {
                let block = ev.get("content_block").and_then(Value::as_object).cloned().unwrap_or_default();
                self.blocks.insert(idx, (block, String::new()));
            }
            "content_block_delta" => {
                let d = ev.get("delta").cloned().unwrap_or(Value::Null);
                let b = self.blocks.get_mut(&idx);
                let s = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                match d.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" => {
                        let t = s("text");
                        self.text.push_str(&t);
                        if let Some((b, _)) = b {
                            append(b, "text", &t);
                        }
                        emit_text(emit, &t);
                    }
                    "thinking_delta" => {
                        let t = s("thinking");
                        self.reasoning.push_str(&t);
                        if let Some((b, _)) = b {
                            append(b, "thinking", &t);
                        }
                        emit_reasoning(emit, &t);
                    }
                    "signature_delta" => {
                        if let Some((b, _)) = b {
                            append(b, "signature", &s("signature"));
                        }
                    }
                    "input_json_delta" => {
                        if let Some((_, j)) = b {
                            j.push_str(&s("partial_json"));
                        }
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(o) = ev.pointer("/usage/output_tokens").and_then(Value::as_u64) {
                    self.output_tokens = Some(o);
                }
                if let Some(s) = ev.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop_reason = Some(s.to_string());
                }
            }
            "error" => {
                let msg = ev.pointer("/error/message").and_then(Value::as_str).unwrap_or("Anthropic stream error");
                return Err(AiError::msg(msg));
            }
            _ => {}
        }
        Ok(())
    }

    pub fn feed_line(&mut self, line: &str, emit: &Emit) -> Result<()> {
        match sse_data(line) {
            Some(v) => self.feed(&v, emit),
            None => Ok(()),
        }
    }

    pub fn finish(self) -> AiStepResult {
        let usage = Some(AiUsage { input_tokens: self.input_tokens, output_tokens: self.output_tokens });
        let mut raw: Vec<Value> = Vec::new();
        let mut tool_calls: Vec<AiToolCall> = Vec::new();
        for (_, (mut block, json_text)) in self.blocks {
            if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                let input = safe_json_parse(&json_text);
                block.insert("input".into(), Value::Object(input.clone()));
                tool_calls.push(AiToolCall {
                    id: block.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                    name: block.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                    args: input,
                });
            }
            raw.push(Value::Object(block));
        }
        // A truncated or refused response must not run half-formed tool calls.
        if self.stop_reason.as_deref() == Some("refusal") {
            let text = if self.text.is_empty() { "Claude declined this request.".to_string() } else { self.text };
            return AiStepResult { text, usage, ..Default::default() };
        }
        let calls = if self.stop_reason.as_deref() == Some("max_tokens") { Vec::new() } else { tool_calls };
        AiStepResult { text: self.text, reasoning: non_empty(self.reasoning), tool_calls: calls, raw: Some(Value::Array(raw)), usage, ..Default::default() }
    }
}

pub async fn anthropic_step(ctx: &StepContext) -> Result<AiStepResult> {
    let body = build_body_with(&ctx.req, &super::load_images(&ctx.req.images, &ctx.emit));
    let headers = [hdr("x-api-key", ctx.api_key.clone().unwrap_or_default()), hdr("anthropic-version", VERSION)];
    let res = post_json(&ctx.client, &format!("{}/v1/messages", ctx.base_url), &body, &headers, &ctx.cancel).await?;
    let mut acc = AnthropicAcc::default();
    for_each_line(res, &ctx.cancel, |line| acc.feed_line(line, &ctx.emit)).await?;
    Ok(acc.finish())
}

pub async fn anthropic_models(client: &reqwest::Client, base_url: &str, api_key: &str) -> Result<Vec<AiModel>> {
    let data: Value = get_json(
        client,
        &format!("{base_url}/v1/models?limit=100"),
        &[hdr("x-api-key", api_key), hdr("anthropic-version", VERSION)],
        Duration::from_secs(8),
    )
    .await?;
    Ok(data
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?.to_string();
            let label = m.get("display_name").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(&id).to_string();
            let mut model = AiModel::new(AiProviderId::Anthropic, id, label);
            model.context = m.get("max_input_tokens").and_then(Value::as_u64);
            Some(model)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::super::testutil::recorder;
    use super::*;
    use crate::types::{AiEffort, AiToolSpec, StreamEvent};

    const FIXTURE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[],"usage":{"input_tokens":200,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"The user wants "}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"a search."}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"EqQB"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"Y2xhdWRl"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Searching"}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"..."}}

event: content_block_start
data: {"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_01","name":"search_library","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"query\": \"ber"}}

event: content_block_delta
data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"serk\"}"}}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":55}}

event: message_stop
data: {"type":"message_stop"}
"#;

    fn run(fixture: &str) -> (AiStepResult, Vec<StreamEvent>) {
        let (emit, log) = recorder();
        let mut acc = AnthropicAcc::default();
        for line in fixture.lines() {
            acc.feed_line(line, &emit).unwrap();
        }
        let r = acc.finish();
        let ev = log.lock().unwrap().clone();
        (r, ev)
    }

    #[test]
    fn parses_thinking_signature_and_tool_use() {
        let (r, ev) = run(FIXTURE);
        assert_eq!(r.text, "Searching...");
        assert_eq!(r.reasoning.as_deref(), Some("The user wants a search."));
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].id, "toolu_01");
        assert_eq!(r.tool_calls[0].args["query"], "berserk");
        let u = r.usage.unwrap();
        assert_eq!((u.input_tokens, u.output_tokens), (Some(200), Some(55)));
        assert_eq!(ev.len(), 4);
        // raw blocks keep thinking + signature so they can be replayed unchanged
        let raw = r.raw.unwrap();
        assert_eq!(raw[0]["type"], "thinking");
        assert_eq!(raw[0]["thinking"], "The user wants a search.");
        assert_eq!(raw[0]["signature"], "EqQBY2xhdWRl");
        assert_eq!(raw[1]["text"], "Searching...");
        assert_eq!(raw[2]["input"]["query"], "berserk");
        assert!(raw[2].get("_json").is_none());
    }

    #[test]
    fn max_tokens_and_refusal_ignore_tool_calls() {
        let (r, _) = run(&FIXTURE.replace("\"tool_use\"},\"usage\"", "\"max_tokens\"},\"usage\""));
        assert!(r.tool_calls.is_empty());
        assert_eq!(r.text, "Searching...");
        assert!(r.raw.is_some());
        let (r, _) = run(&FIXTURE.replace("\"tool_use\"},\"usage\"", "\"refusal\"},\"usage\""));
        assert!(r.tool_calls.is_empty());
        assert!(r.raw.is_none());
        let (r, _) = run("data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"refusal\"}}");
        assert_eq!(r.text, "Claude declined this request.");
    }

    #[test]
    fn stream_error_event_fails() {
        let (emit, _) = recorder();
        let mut acc = AnthropicAcc::default();
        let e = acc.feed_line(r#"data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#, &emit).unwrap_err();
        assert_eq!(e.to_string(), "Overloaded");
    }

    fn req(model: &str) -> AiStepRequest {
        AiStepRequest {
            request_id: "r".into(),
            provider_id: AiProviderId::Anthropic,
            model: model.into(),
            messages: vec![AiMessage::system("sys"), AiMessage::user("hi")],
            tools: Some(vec![AiToolSpec { name: "t".into(), description: "d".into(), parameters: json!({"type":"object"}) }]),
            effort: Some(AiEffort::High),
            access: None,
            images: vec![],
            workspace: None,
            lean: false,
            mcp: None,
        }
    }

    #[test]
    fn adaptive_thinking_only_for_current_models_and_never_budget_tokens() {
        let b = build_body(&req("claude-sonnet-5-5"));
        assert_eq!(b["thinking"], json!({"type":"adaptive","display":"summarized"}));
        assert_eq!(b["output_config"], json!({"effort":"high"}));
        assert_eq!(b["system"][0]["text"], "sys");
        assert_eq!(b["tools"][0]["eager_input_streaming"], true);
        assert!(!b.to_string().contains("budget_tokens"));
        for m in ["claude-opus-4-7", "claude-fable-5-1", "claude-haiku-5-5", "claude-sonnet-4-6"] {
            assert!(is_adaptive_model(m), "{m}");
        }
        for m in ["claude-3-5-sonnet-20241022", "claude-opus-4-1", "claude-sonnet-4-5", "claude-haiku-4-5"] {
            let b = build_body(&req(m));
            assert!(b.get("thinking").is_none() && b.get("output_config").is_none(), "{m}");
        }
    }

    #[test]
    fn raw_assistant_blocks_are_replayed_unchanged_and_roles_merge() {
        let raw = json!([
            {"type":"thinking","thinking":"hm","signature":"SIG"},
            {"type":"tool_use","id":"t1","name":"f","input":{"a":1}}
        ]);
        let mut a = AiMessage::assistant("ignored because raw wins");
        a.raw = Some(raw.clone());
        let mut tool = AiMessage::new(AiRole::Tool, "42");
        tool.tool_call_id = Some("t1".into());
        let out = to_anthropic(&[AiMessage::user("q"), a, tool, AiMessage::user("more")]);
        assert_eq!(out.messages.len(), 3);
        assert_eq!(out.messages[1]["content"], raw);
        // tool_result and the following user text share one user turn
        assert_eq!(out.messages[2]["role"], "user");
        assert_eq!(out.messages[2]["content"][0]["type"], "tool_result");
        assert_eq!(out.messages[2]["content"][1]["text"], "more");
    }

    #[test]
    fn tools_system_and_conversation_are_cache_breakpoints() {
        let mut r = req("claude-sonnet-5-5");
        r.tools.as_mut().unwrap().push(AiToolSpec { name: "u".into(), description: "d".into(), parameters: json!({"type":"object"}) });
        let b = build_body(&r);
        let eph = json!({ "type": "ephemeral" });
        assert!(b["tools"][0].get("cache_control").is_none());
        assert_eq!(b["tools"][1]["cache_control"], eph);
        assert_eq!(b["system"][0]["cache_control"], eph);
        assert_eq!(b["messages"][0]["content"][0]["cache_control"], eph);
        // Never more than the API's four breakpoints.
        assert!(b.to_string().matches("ephemeral").count() <= 4);
        // A trailing assistant turn (prefill) gets none.
        r.messages.push(AiMessage::assistant("pre"));
        assert!(build_body(&r)["messages"][1]["content"][0].get("cache_control").is_none());
    }

    #[test]
    fn cached_input_counts_once() {
        let (emit, _) = recorder();
        let mut acc = AnthropicAcc::default();
        acc.feed_line(r#"data: {"type":"message_start","message":{"usage":{"input_tokens":10,"cache_creation_input_tokens":300,"cache_read_input_tokens":2000,"output_tokens":1}}}"#, &emit).unwrap();
        assert_eq!(acc.finish().usage.unwrap().input_tokens, Some(2310));
    }

    #[test]
    fn images_attach_to_last_user_message_before_text() {
        let b = build_body_with(&req("claude-sonnet-5-5"), &[ImageData { mime: "image/png", b64: "QUJD".into() }]);
        let c = &b["messages"][0]["content"];
        assert_eq!(c[0], json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"QUJD"}}));
        assert_eq!(c[1]["type"], "text");
        assert_eq!(build_body(&req("claude-sonnet-5-5"))["messages"][0]["content"].as_array().unwrap().len(), 1);
    }
}
