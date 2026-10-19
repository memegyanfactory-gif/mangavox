//! OpenAI-compatible `/chat/completions` (OpenAI, OpenRouter, Groq, DeepSeek, Mistral, xAI,
//! LM Studio, llama.cpp, custom endpoints) over SSE.

use super::{emit_reasoning, emit_text, non_empty, ImageData, StepContext};
use crate::stream::{for_each_line, get_json, hdr, new_call_id, post_json, safe_json_parse, sse_data, AiError, Result};
use crate::types::{AiMessage, AiModel, AiProviderId, AiRole, AiStepResult, AiToolCall, AiUsage, Emit};
use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Duration;

pub fn to_openai_messages(messages: &[AiMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|m| {
            if m.role == AiRole::Tool {
                let mut o = json!({ "role": "tool", "content": m.content });
                if let Some(id) = &m.tool_call_id {
                    o["tool_call_id"] = json!(id);
                }
                return o;
            }
            if m.role == AiRole::Assistant {
                if let Some(calls) = m.tool_calls.as_ref().filter(|c| !c.is_empty()) {
                    return json!({
                        "role": "assistant",
                        "content": if m.content.is_empty() { Value::Null } else { json!(m.content) },
                        "tool_calls": calls.iter().map(|c| json!({
                            "id": c.id,
                            "type": "function",
                            "function": { "name": c.name, "arguments": Value::Object(c.args.clone()).to_string() },
                        })).collect::<Vec<_>>(),
                    });
                }
            }
            json!({ "role": super::ollama::role_str(m.role), "content": m.content })
        })
        .collect()
}

fn auth_headers(api_key: Option<&str>, provider: AiProviderId) -> Vec<(String, String)> {
    let mut h = Vec::new();
    if let Some(k) = api_key.filter(|k| !k.is_empty()) {
        h.push(hdr("authorization", format!("Bearer {k}")));
    }
    if provider == AiProviderId::Openrouter {
        h.push(hdr("HTTP-Referer", "https://mangavox.app"));
        h.push(hdr("X-Title", "MangaVox"));
    }
    h
}

/// Reasoning models on OpenAI take `reasoning_effort`; other hosts ignore or reject it.
fn effort_field(ctx: &StepContext) -> Option<(&'static str, Value)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?i)^(o\d|gpt-5)").unwrap());
    let req = &ctx.req;
    let effort = req.effort?;
    match req.provider_id {
        AiProviderId::Openai if re.is_match(&req.model) => Some(("reasoning_effort", json!(effort.as_str()))),
        AiProviderId::Openrouter => Some(("reasoning", json!({ "effort": effort.as_str() }))),
        _ => None,
    }
}

#[derive(Default)]
struct Partial {
    id: String,
    name: String,
    args: String,
}

#[derive(Default)]
pub struct OpenAiAcc {
    text: String,
    reasoning: String,
    /// Insertion-ordered (index, partial call).
    partial: Vec<(u64, Partial)>,
    usage: Option<AiUsage>,
}

impl OpenAiAcc {
    /// Feed one parsed SSE `data:` payload.
    pub fn feed(&mut self, chunk: &Value, emit: &Emit) -> Result<()> {
        if let Some(err) = chunk.get("error").filter(|e| !e.is_null()) {
            let msg = err.get("message").and_then(Value::as_str).map(String::from).unwrap_or_else(|| err.to_string());
            return Err(AiError::msg(msg));
        }
        if let Some(u) = chunk.get("usage").filter(|u| !u.is_null()) {
            self.usage = Some(AiUsage {
                input_tokens: u.get("prompt_tokens").and_then(Value::as_u64),
                output_tokens: u.get("completion_tokens").and_then(Value::as_u64),
            });
        }
        let Some(delta) = chunk.get("choices").and_then(|c| c.get(0)).and_then(|c| c.get("delta")) else { return Ok(()) };
        let think = delta.get("reasoning_content").filter(|v| !v.is_null()).or_else(|| delta.get("reasoning"));
        if let Some(t) = think.and_then(Value::as_str).filter(|s| !s.is_empty()) {
            self.reasoning.push_str(t);
            emit_reasoning(emit, t);
        }
        if let Some(c) = delta.get("content").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            self.text.push_str(c);
            emit_text(emit, c);
        }
        for tc in delta.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
            let idx = tc.get("index").and_then(Value::as_u64).unwrap_or(0);
            let pos = match self.partial.iter().position(|(i, _)| *i == idx) {
                Some(p) => p,
                None => {
                    self.partial.push((idx, Partial::default()));
                    self.partial.len() - 1
                }
            };
            let cur = &mut self.partial[pos].1;
            if let Some(id) = tc.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                cur.id = id.to_string();
            }
            if let Some(n) = tc.pointer("/function/name").and_then(Value::as_str) {
                cur.name.push_str(n);
            }
            if let Some(a) = tc.pointer("/function/arguments").and_then(Value::as_str) {
                cur.args.push_str(a);
            }
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
        let tool_calls = self
            .partial
            .into_iter()
            .map(|(_, c)| c)
            .filter(|c| !c.name.is_empty())
            .map(|c| AiToolCall {
                id: if c.id.is_empty() { new_call_id() } else { c.id },
                name: c.name,
                args: safe_json_parse(&c.args),
            })
            .collect();
        AiStepResult { text: self.text, reasoning: non_empty(self.reasoning), tool_calls, usage: self.usage, ..Default::default() }
    }
}

/// Turn the last user message's content into `[image_url..., text]` parts.
pub fn attach_images(messages: &mut [Value], images: &[ImageData]) {
    if images.is_empty() {
        return;
    }
    let Some(m) = super::last_user_mut(messages) else { return };
    let text = m.get("content").and_then(Value::as_str).unwrap_or("").to_string();
    let mut parts: Vec<Value> =
        images.iter().map(|i| json!({ "type": "image_url", "image_url": { "url": format!("data:{};base64,{}", i.mime, i.b64) } })).collect();
    parts.push(json!({ "type": "text", "text": text }));
    m["content"] = Value::Array(parts);
}

async fn run(ctx: &StepContext, with_tools: bool) -> Result<AiStepResult> {
    let req = &ctx.req;
    let mut messages = to_openai_messages(&req.messages);
    attach_images(&mut messages, &super::load_images(&req.images, &ctx.emit));
    let mut body = json!({
        "model": req.model,
        "messages": messages,
        "stream": true,
    });
    if let Some((k, v)) = effort_field(ctx) {
        body[k] = v;
    }
    if matches!(req.provider_id, AiProviderId::Openai | AiProviderId::Openrouter) {
        body["stream_options"] = json!({ "include_usage": true });
    }
    if let Some(tools) = req.tools.as_ref().filter(|t| with_tools && !t.is_empty()) {
        body["tools"] = tools.iter().map(|t| json!({ "type": "function", "function": t })).collect();
    }
    let headers = auth_headers(ctx.api_key.as_deref(), req.provider_id);
    let res = post_json(&ctx.client, &format!("{}/chat/completions", ctx.base_url), &body, &headers, &ctx.cancel).await?;
    let mut acc = OpenAiAcc::default();
    for_each_line(res, &ctx.cancel, |line| acc.feed_line(line, &ctx.emit)).await?;
    Ok(acc.finish())
}

fn tool_unsupported(msg: &str) -> bool {
    static A: OnceLock<Regex> = OnceLock::new();
    static B: OnceLock<Regex> = OnceLock::new();
    A.get_or_init(|| Regex::new(r"(?i)tool|function").unwrap()).is_match(msg)
        && B.get_or_init(|| Regex::new(r"(?i)support|not allowed|unsupported|invalid").unwrap()).is_match(msg)
}

pub async fn openai_compat_step(ctx: &StepContext) -> Result<AiStepResult> {
    match run(ctx, true).await {
        Ok(r) => Ok(r),
        Err(err) => {
            let has_tools = ctx.req.tools.as_ref().is_some_and(|t| !t.is_empty());
            if has_tools && !err.is_cancelled() && tool_unsupported(&err.to_string()) {
                ctx.status(format!("{} can't use app tools — answering without them", ctx.req.model));
                return run(ctx, false).await;
            }
            Err(err)
        }
    }
}

fn chat_model_exclude(id: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(embed|whisper|tts|dall-e|image|moderation|audio|transcribe|realtime|search-preview|davinci|babbage)").unwrap()
    })
    .is_match(id)
}

pub async fn openai_compat_models(client: &reqwest::Client, provider: AiProviderId, base_url: &str, api_key: Option<&str>) -> Result<Vec<AiModel>> {
    let data: Value = get_json(client, &format!("{base_url}/models"), &auth_headers(api_key, provider), Duration::from_secs(8)).await?;
    Ok(data
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?.to_string();
            if chat_model_exclude(&id) {
                return None;
            }
            let label = m.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(&id).to_string();
            let mut model = AiModel::new(provider, id, label);
            model.context = m.get("context_length").and_then(Value::as_u64);
            Some(model)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::super::testutil::recorder;
    use super::*;
    use crate::types::StreamEvent;

    const FIXTURE: &str = r#"data: {"id":"c1","choices":[{"index":0,"delta":{"role":"assistant","content":""}}]}

data: {"id":"c1","choices":[{"index":0,"delta":{"reasoning_content":"Hmm. "}}]}

data: {"id":"c1","choices":[{"index":0,"delta":{"content":"Let me "}}]}

data: {"id":"c1","choices":[{"index":0,"delta":{"content":"check."}}]}

data: {"id":"c1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_abc","type":"function","function":{"name":"search_","arguments":""}}]}}]}

data: {"id":"c1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"name":"library","arguments":"{\"que"}}]}}]}

data: {"id":"c1","choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"function":{"name":"noop","arguments":"{}"}}]}}]}

data: {"id":"c1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"ry\":\"naruto\"}"}}]}}]}

data: {"id":"c1","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}

data: {"id":"c1","choices":[],"usage":{"prompt_tokens":120,"completion_tokens":33}}

data: [DONE]
"#;

    #[test]
    fn parses_sse_with_tool_call_deltas() {
        let (emit, log) = recorder();
        let mut acc = OpenAiAcc::default();
        for line in FIXTURE.lines() {
            acc.feed_line(line, &emit).unwrap();
        }
        let r = acc.finish();
        assert_eq!(r.text, "Let me check.");
        assert_eq!(r.reasoning.as_deref(), Some("Hmm. "));
        assert_eq!(r.tool_calls.len(), 2);
        assert_eq!(r.tool_calls[0].id, "call_abc");
        assert_eq!(r.tool_calls[0].name, "search_library");
        assert_eq!(r.tool_calls[0].args["query"], "naruto");
        assert_eq!(r.tool_calls[1].name, "noop");
        assert!(r.tool_calls[1].id.starts_with("call_"));
        let u = r.usage.unwrap();
        assert_eq!((u.input_tokens, u.output_tokens), (Some(120), Some(33)));
        assert_eq!(log.lock().unwrap()[0], StreamEvent::Reasoning { delta: "Hmm. ".into() });
    }

    #[test]
    fn stream_error_chunk_fails() {
        let (emit, _) = recorder();
        let mut acc = OpenAiAcc::default();
        let e = acc.feed_line(r#"data: {"error":{"message":"rate limited"}}"#, &emit).unwrap_err();
        assert_eq!(e.to_string(), "rate limited");
    }

    #[test]
    fn tool_fallback_detection() {
        assert!(tool_unsupported("400: registry model does not support tools"));
        assert!(tool_unsupported("400: Invalid function schema"));
        assert!(!tool_unsupported("401: bad key"));
    }

    #[test]
    fn assistant_tool_calls_serialize_arguments_as_string() {
        let mut m = AiMessage::assistant("");
        let mut args = serde_json::Map::new();
        args.insert("q".into(), json!("x"));
        m.tool_calls = Some(vec![AiToolCall { id: "c".into(), name: "f".into(), args }]);
        let out = to_openai_messages(&[m]);
        assert_eq!(out[0]["content"], Value::Null);
        assert_eq!(out[0]["tool_calls"][0]["function"]["arguments"], "{\"q\":\"x\"}");
    }

    #[test]
    fn model_filter_excludes_non_chat() {
        assert!(chat_model_exclude("text-embedding-3-small"));
        assert!(!chat_model_exclude("gpt-5"));
    }

    #[test]
    fn images_become_content_parts_on_last_user_message() {
        let mut m = to_openai_messages(&[AiMessage::user("a"), AiMessage::user("b")]);
        attach_images(&mut m, &[ImageData { mime: "image/webp", b64: "QUJD".into() }]);
        assert_eq!(m[0]["content"], "a");
        assert_eq!(m[1]["content"][0], json!({"type":"image_url","image_url":{"url":"data:image/webp;base64,QUJD"}}));
        assert_eq!(m[1]["content"][1], json!({"type":"text","text":"b"}));
    }
}
