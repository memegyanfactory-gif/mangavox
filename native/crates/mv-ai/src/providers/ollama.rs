//! Ollama `/api/chat` (NDJSON streaming).

use super::{emit_reasoning, emit_text, non_empty, StepContext};
use crate::stream::{for_each_line, get_json, new_call_id, post_json, AiError, Result};
use crate::types::{AiMessage, AiModel, AiProviderId, AiRole, AiStepResult, AiToolCall, AiUsage, Emit};
use serde_json::{json, Map, Value};
use std::time::Duration;

pub fn to_ollama_messages(messages: &[AiMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|m| {
            if m.role == AiRole::Tool {
                let mut o = json!({ "role": "tool", "content": m.content });
                if let Some(n) = &m.tool_name {
                    o["tool_name"] = json!(n);
                }
                return o;
            }
            if m.role == AiRole::Assistant {
                if let Some(calls) = m.tool_calls.as_ref().filter(|c| !c.is_empty()) {
                    return json!({
                        "role": "assistant",
                        "content": m.content,
                        "tool_calls": calls.iter().map(|c| json!({ "function": { "name": c.name, "arguments": c.args } })).collect::<Vec<_>>(),
                    });
                }
            }
            json!({ "role": role_str(m.role), "content": m.content })
        })
        .collect()
}

pub(crate) fn role_str(r: AiRole) -> &'static str {
    match r {
        AiRole::System => "system",
        AiRole::User => "user",
        AiRole::Assistant => "assistant",
        AiRole::Tool => "tool",
    }
}

#[derive(Default)]
pub struct OllamaAcc {
    text: String,
    reasoning: String,
    tool_calls: Vec<AiToolCall>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

impl OllamaAcc {
    /// Feed one NDJSON line.
    pub fn feed_line(&mut self, line: &str, emit: &Emit) -> Result<()> {
        if line.trim().is_empty() {
            return Ok(());
        }
        let Ok(chunk) = serde_json::from_str::<Value>(line) else { return Ok(()) };
        if let Some(err) = chunk.get("error").filter(|e| !e.is_null()) {
            return Err(AiError::http(err.as_str().map(String::from).unwrap_or_else(|| err.to_string()), None));
        }
        let msg = chunk.get("message");
        if let Some(t) = msg.and_then(|m| m.get("thinking")).and_then(Value::as_str).filter(|s| !s.is_empty()) {
            self.reasoning.push_str(t);
            emit_reasoning(emit, t);
        }
        if let Some(c) = msg.and_then(|m| m.get("content")).and_then(Value::as_str).filter(|s| !s.is_empty()) {
            self.text.push_str(c);
            emit_text(emit, c);
        }
        for call in msg.and_then(|m| m.get("tool_calls")).and_then(Value::as_array).into_iter().flatten() {
            let f = call.get("function").cloned().unwrap_or(Value::Null);
            let mut args = f.get("arguments").cloned().unwrap_or_else(|| json!({}));
            if let Value::String(s) = &args {
                args = serde_json::from_str(s).unwrap_or_else(|_| json!({}));
            }
            let args = match args {
                Value::Object(m) => m,
                _ => Map::new(),
            };
            let name = f.get("name").and_then(Value::as_str).unwrap_or("undefined").to_string();
            self.tool_calls.push(AiToolCall { id: new_call_id(), name, args });
        }
        if chunk.get("done").and_then(Value::as_bool) == Some(true) {
            self.input_tokens = chunk.get("prompt_eval_count").and_then(Value::as_u64);
            self.output_tokens = chunk.get("eval_count").and_then(Value::as_u64);
        }
        Ok(())
    }

    pub fn finish(self) -> AiStepResult {
        AiStepResult {
            text: self.text,
            reasoning: non_empty(self.reasoning),
            tool_calls: self.tool_calls,
            usage: Some(AiUsage { input_tokens: self.input_tokens, output_tokens: self.output_tokens }),
            ..Default::default()
        }
    }
}

async fn run(ctx: &StepContext, with_tools: bool) -> Result<AiStepResult> {
    let req = &ctx.req;
    let mut body = json!({
        "model": req.model,
        "messages": to_ollama_messages(&req.messages),
        "stream": true,
    });
    if let Some(tools) = req.tools.as_ref().filter(|t| with_tools && !t.is_empty()) {
        body["tools"] = tools.iter().map(|t| json!({ "type": "function", "function": t })).collect();
    }
    let res = post_json(&ctx.client, &format!("{}/api/chat", ctx.base_url), &body, &[], &ctx.cancel).await?;
    let mut acc = OllamaAcc::default();
    for_each_line(res, &ctx.cancel, |line| acc.feed_line(line, &ctx.emit)).await?;
    Ok(acc.finish())
}

pub async fn ollama_step(ctx: &StepContext) -> Result<AiStepResult> {
    match run(ctx, true).await {
        Ok(r) => Ok(r),
        Err(err) => {
            // Many local models have no tool template; answer without tools instead of failing.
            let has_tools = ctx.req.tools.as_ref().is_some_and(|t| !t.is_empty());
            if has_tools && err.to_string().to_lowercase().contains("does not support tools") {
                ctx.status(format!("{} can't use app tools — answering without them", ctx.req.model));
                return run(ctx, false).await;
            }
            Err(err)
        }
    }
}

pub async fn ollama_models(client: &reqwest::Client, base_url: &str) -> Result<Vec<AiModel>> {
    let data: Value = get_json(client, &format!("{base_url}/api/tags"), &[], Duration::from_secs(4)).await?;
    Ok(data
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let name = m.get("name")?.as_str()?.to_string();
            let details = m.get("details");
            let tags: Vec<String> = ["parameter_size", "family"]
                .iter()
                .filter_map(|k| details.and_then(|d| d.get(*k)).and_then(Value::as_str))
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect();
            let mut model = AiModel::new(AiProviderId::Ollama, name.clone(), name);
            model.size_bytes = m.get("size").and_then(Value::as_u64);
            model.tags = Some(tags);
            Some(model)
        })
        .collect())
}

pub async fn ollama_version(client: &reqwest::Client, base_url: &str) -> Result<String> {
    let data: Value = get_json(client, &format!("{base_url}/api/version"), &[], Duration::from_millis(2500)).await?;
    Ok(data.get("version").and_then(Value::as_str).unwrap_or("").to_string())
}

#[cfg(test)]
mod tests {
    use super::super::testutil::recorder;
    use super::*;
    use crate::types::StreamEvent;

    const FIXTURE: &str = r#"{"model":"qwen3","created_at":"2026-01-01T00:00:00Z","message":{"role":"assistant","content":"","thinking":"Let me "},"done":false}
{"model":"qwen3","message":{"role":"assistant","content":"","thinking":"think."},"done":false}
{"model":"qwen3","message":{"role":"assistant","content":"Hel"},"done":false}
{"model":"qwen3","message":{"role":"assistant","content":"lo"},"done":false}
{"model":"qwen3","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"search_library","arguments":{"query":"one piece","limit":3}}}]},"done":false}
{"model":"qwen3","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":42,"eval_count":17}
"#;

    #[test]
    fn parses_ndjson_stream_with_thinking_and_tool_calls() {
        let (emit, log) = recorder();
        let mut acc = OllamaAcc::default();
        for line in FIXTURE.lines() {
            acc.feed_line(line, &emit).unwrap();
        }
        let r = acc.finish();
        assert_eq!(r.text, "Hello");
        assert_eq!(r.reasoning.as_deref(), Some("Let me think."));
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].name, "search_library");
        assert_eq!(r.tool_calls[0].args["query"], "one piece");
        let u = r.usage.unwrap();
        assert_eq!((u.input_tokens, u.output_tokens), (Some(42), Some(17)));
        let ev = log.lock().unwrap();
        assert_eq!(ev.len(), 4);
        assert_eq!(ev[0], StreamEvent::Reasoning { delta: "Let me ".into() });
        assert_eq!(ev[3], StreamEvent::Text { delta: "lo".into() });
    }

    #[test]
    fn string_arguments_are_parsed_and_errors_surface() {
        let (emit, _) = recorder();
        let mut acc = OllamaAcc::default();
        acc.feed_line(r#"{"message":{"tool_calls":[{"function":{"name":"x","arguments":"{\"a\":1}"}}]}}"#, &emit).unwrap();
        assert_eq!(acc.finish().tool_calls[0].args["a"], 1);
        let mut acc = OllamaAcc::default();
        let e = acc.feed_line(r#"{"error":"registry.ollama.ai/x does not support tools"}"#, &emit).unwrap_err();
        assert!(e.to_string().contains("does not support tools"));
    }

    #[test]
    fn messages_map_to_ollama_shape() {
        let mut call = AiMessage::assistant("");
        call.tool_calls = Some(vec![AiToolCall { id: "c1".into(), name: "f".into(), args: Map::new() }]);
        let mut tool = AiMessage::new(AiRole::Tool, "result");
        tool.tool_name = Some("f".into());
        let out = to_ollama_messages(&[AiMessage::user("hi"), call, tool]);
        assert_eq!(out[1]["tool_calls"][0]["function"]["name"], "f");
        assert_eq!(out[2]["tool_name"], "f");
        assert_eq!(out[0]["role"], "user");
    }
}
