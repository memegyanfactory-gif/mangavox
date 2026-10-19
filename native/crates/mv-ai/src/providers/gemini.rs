//! Gemini `streamGenerateContent?alt=sse`.

use super::{emit_reasoning, emit_text, non_empty, ImageData, StepContext};
use crate::stream::{for_each_line, get_json, hdr, new_call_id, post_json, sse_data, AiError, Result};
use crate::types::{AiEffort, AiMessage, AiModel, AiProviderId, AiRole, AiStepRequest, AiStepResult, AiToolCall, AiUsage, Emit};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::sync::OnceLock;
use std::time::Duration;

/// Gemini's schema dialect rejects a few JSON-schema keywords.
pub fn clean_schema(schema: &Value) -> Value {
    match schema {
        Value::Array(a) => Value::Array(a.iter().map(clean_schema).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .filter(|(k, _)| !matches!(k.as_str(), "additionalProperties" | "$schema" | "default"))
                .map(|(k, v)| (k.clone(), clean_schema(v)))
                .collect::<Map<_, _>>(),
        ),
        other => other.clone(),
    }
}

pub struct GeminiContents {
    pub system: String,
    pub contents: Vec<Value>,
}

pub fn to_gemini(messages: &[AiMessage]) -> GeminiContents {
    let system = messages.iter().filter(|m| m.role == AiRole::System).map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n\n");
    let mut out: Vec<(&'static str, Vec<Value>)> = Vec::new();
    fn push(out: &mut Vec<(&'static str, Vec<Value>)>, role: &'static str, part: Value) {
        match out.last_mut() {
            Some((r, p)) if *r == role => p.push(part),
            _ => out.push((role, vec![part])),
        }
    }
    for m in messages {
        match m.role {
            AiRole::System => {}
            AiRole::Tool => push(
                &mut out,
                "user",
                json!({ "functionResponse": { "name": m.tool_name, "response": { "result": m.content } } }),
            ),
            AiRole::Assistant => {
                if !m.content.is_empty() {
                    push(&mut out, "model", json!({ "text": m.content }));
                }
                for c in m.tool_calls.iter().flatten() {
                    push(&mut out, "model", json!({ "functionCall": { "name": c.name, "args": c.args } }));
                }
            }
            AiRole::User => {
                if !m.content.is_empty() {
                    push(&mut out, "user", json!({ "text": m.content }));
                }
            }
        }
    }
    GeminiContents { system, contents: out.into_iter().map(|(role, parts)| json!({ "role": role, "parts": parts })).collect() }
}

fn thinking_budget(e: AiEffort) -> u32 {
    match e {
        AiEffort::Low => 1024,
        AiEffort::Medium => 4096,
        AiEffort::High => 12000,
    }
}

pub fn build_body(req: &AiStepRequest) -> Value {
    build_body_with(req, &[])
}

/// Like [`build_body`], attaching `images` (before the text) to the last user message.
pub fn build_body_with(req: &AiStepRequest, images: &[ImageData]) -> Value {
    static RE: OnceLock<Regex> = OnceLock::new();
    let GeminiContents { system, mut contents } = to_gemini(&req.messages);
    if let Some(Value::Array(p)) = super::last_user_mut(&mut contents).and_then(|m| m.get_mut("parts")) {
        let parts = images.iter().map(|i| json!({ "inline_data": { "mime_type": i.mime, "data": i.b64 } }));
        p.splice(0..0, parts);
    }
    let mut body = json!({ "contents": contents });
    if !system.is_empty() {
        body["systemInstruction"] = json!({ "parts": [{ "text": system }] });
    }
    if let Some(tools) = req.tools.as_ref().filter(|t| !t.is_empty()) {
        body["tools"] = json!([{
            "functionDeclarations": tools.iter().map(|t| json!({
                "name": t.name, "description": t.description, "parameters": clean_schema(&t.parameters),
            })).collect::<Vec<_>>()
        }]);
    }
    if let Some(e) = req.effort {
        if RE.get_or_init(|| Regex::new(r"2\.5|3").unwrap()).is_match(&req.model) {
            body["generationConfig"] = json!({ "thinkingConfig": { "thinkingBudget": thinking_budget(e), "includeThoughts": true } });
        }
    }
    body
}

/// Percent-encode like JS `encodeURIComponent`.
pub fn encode_uri_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[derive(Default)]
pub struct GeminiAcc {
    text: String,
    reasoning: String,
    tool_calls: Vec<AiToolCall>,
    usage: Option<AiUsage>,
}

impl GeminiAcc {
    /// Feed one parsed SSE `data:` payload.
    pub fn feed(&mut self, chunk: &Value, emit: &Emit) -> Result<()> {
        if let Some(err) = chunk.get("error").filter(|e| !e.is_null()) {
            return Err(AiError::msg(err.get("message").and_then(Value::as_str).unwrap_or("Gemini error")));
        }
        if let Some(u) = chunk.get("usageMetadata").filter(|u| !u.is_null()) {
            self.usage = Some(AiUsage {
                input_tokens: u.get("promptTokenCount").and_then(Value::as_u64),
                output_tokens: u.get("candidatesTokenCount").and_then(Value::as_u64),
            });
        }
        let parts = chunk.pointer("/candidates/0/content/parts").and_then(Value::as_array);
        for part in parts.into_iter().flatten() {
            if let Some(fc) = part.get("functionCall").filter(|f| !f.is_null()) {
                let args = match fc.get("args") {
                    Some(Value::Object(m)) => m.clone(),
                    _ => Map::new(),
                };
                let name = fc.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                self.tool_calls.push(AiToolCall { id: new_call_id(), name, args });
            } else if let Some(t) = part.get("text").and_then(Value::as_str) {
                if part.get("thought").and_then(Value::as_bool) == Some(true) {
                    self.reasoning.push_str(t);
                    emit_reasoning(emit, t);
                } else {
                    self.text.push_str(t);
                    emit_text(emit, t);
                }
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
        AiStepResult { text: self.text, reasoning: non_empty(self.reasoning), tool_calls: self.tool_calls, usage: self.usage, ..Default::default() }
    }
}

pub async fn gemini_step(ctx: &StepContext) -> Result<AiStepResult> {
    let body = build_body_with(&ctx.req, &super::load_images(&ctx.req.images, &ctx.emit));
    let url = format!("{}/models/{}:streamGenerateContent?alt=sse", ctx.base_url, encode_uri_component(&ctx.req.model));
    let res = post_json(&ctx.client, &url, &body, &[hdr("x-goog-api-key", ctx.api_key.clone().unwrap_or_default())], &ctx.cancel).await?;
    let mut acc = GeminiAcc::default();
    for_each_line(res, &ctx.cancel, |line| acc.feed_line(line, &ctx.emit)).await?;
    Ok(acc.finish())
}

pub async fn gemini_models(client: &reqwest::Client, base_url: &str, api_key: &str) -> Result<Vec<AiModel>> {
    static EXCLUDE: OnceLock<Regex> = OnceLock::new();
    let exclude = EXCLUDE.get_or_init(|| Regex::new(r"(?i)embedding|aqa|imagen|tts").unwrap());
    let data: Value = get_json(client, &format!("{base_url}/models?pageSize=200"), &[hdr("x-goog-api-key", api_key)], Duration::from_secs(8)).await?;
    Ok(data
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let name = m.get("name")?.as_str()?;
            let can_generate = m.get("supportedGenerationMethods").and_then(Value::as_array).is_some_and(|a| a.iter().any(|x| x == "generateContent"));
            if !can_generate || exclude.is_match(name) {
                return None;
            }
            let id = name.strip_prefix("models/").unwrap_or(name).to_string();
            let label = m.get("displayName").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(name).to_string();
            let mut model = AiModel::new(AiProviderId::Gemini, id, label);
            model.context = m.get("inputTokenLimit").and_then(Value::as_u64);
            Some(model)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::super::testutil::recorder;
    use super::*;
    use crate::types::{AiToolSpec, StreamEvent};

    const FIXTURE: &str = r#"data: {"candidates":[{"content":{"role":"model","parts":[{"text":"Considering the request. ","thought":true}]}}],"usageMetadata":{"promptTokenCount":30}}

data: {"candidates":[{"content":{"role":"model","parts":[{"text":"I'll look "}]}}]}

data: {"candidates":[{"content":{"role":"model","parts":[{"text":"it up."},{"functionCall":{"name":"search_library","args":{"query":"frieren"}}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":30,"candidatesTokenCount":12,"totalTokenCount":42}}

"#;

    #[test]
    fn parses_sse_with_thought_text_and_function_call() {
        let (emit, log) = recorder();
        let mut acc = GeminiAcc::default();
        for line in FIXTURE.lines() {
            acc.feed_line(line, &emit).unwrap();
        }
        let r = acc.finish();
        assert_eq!(r.text, "I'll look it up.");
        assert_eq!(r.reasoning.as_deref(), Some("Considering the request. "));
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].name, "search_library");
        assert_eq!(r.tool_calls[0].args["query"], "frieren");
        let u = r.usage.unwrap();
        assert_eq!((u.input_tokens, u.output_tokens), (Some(30), Some(12)));
        assert_eq!(log.lock().unwrap()[0], StreamEvent::Reasoning { delta: "Considering the request. ".into() });
    }

    #[test]
    fn schema_cleaning_and_request_body() {
        let schema = json!({"type":"object","additionalProperties":false,"$schema":"x","properties":{"a":{"type":"string","default":"z"}}});
        let cleaned = clean_schema(&schema);
        assert_eq!(cleaned, json!({"type":"object","properties":{"a":{"type":"string"}}}));
        let req = AiStepRequest {
            request_id: "r".into(),
            provider_id: AiProviderId::Gemini,
            model: "gemini-2.5-flash".into(),
            messages: vec![AiMessage::system("s"), AiMessage::user("u")],
            tools: Some(vec![AiToolSpec { name: "t".into(), description: "d".into(), parameters: schema }]),
            effort: Some(AiEffort::Medium),
            access: None,
            images: vec![],
            workspace: None,
        };
        let b = build_body(&req);
        assert_eq!(b["generationConfig"]["thinkingConfig"]["thinkingBudget"], 4096);
        assert_eq!(b["systemInstruction"]["parts"][0]["text"], "s");
        assert!(b["tools"][0]["functionDeclarations"][0]["parameters"].get("additionalProperties").is_none());
        assert_eq!(encode_uri_component("gemini 2.5/pro"), "gemini%202.5%2Fpro");
    }

    #[test]
    fn tool_results_become_function_responses() {
        let mut t = AiMessage::new(AiRole::Tool, "ok");
        t.tool_name = Some("f".into());
        let c = to_gemini(&[AiMessage::user("q"), t]);
        assert_eq!(c.contents.len(), 1); // same role merges
        assert_eq!(c.contents[0]["parts"][1]["functionResponse"]["response"]["result"], "ok");
    }

    #[test]
    fn images_become_inline_data_parts() {
        let req = AiStepRequest {
            request_id: "r".into(),
            provider_id: AiProviderId::Gemini,
            model: "gemini-2.5-flash".into(),
            messages: vec![AiMessage::user("u")],
            tools: None,
            effort: None,
            access: None,
            images: vec![],
            workspace: None,
        };
        let b = build_body_with(&req, &[ImageData { mime: "image/jpeg", b64: "QUJD".into() }]);
        assert_eq!(b["contents"][0]["parts"][0], json!({"inline_data":{"mime_type":"image/jpeg","data":"QUJD"}}));
        assert_eq!(b["contents"][0]["parts"][1]["text"], "u");
    }
}
