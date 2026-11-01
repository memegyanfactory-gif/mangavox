//! Tool calls written as text, for models without native tool calling (some local models, CLI
//! agents whose MCP set-up did not take). The model is asked for ```mangavox blocks; small models
//! drift from that, so this parser also takes the shapes they actually produce:
//!
//! * ```mangavox fences (an unterminated fence at the end counts too),
//! * `<tool_call>…</tool_call>` / `<function_call>…</function_call>` tags (Hermes, Qwen),
//! * `[TOOL_CALLS] […]` (Mistral),
//! * and, only when the reply is nothing but the call (whitespace aside), ```json / ```tool
//!   fences or a bare `{"tool": …}` / `{"name", "arguments"}` object: example JSON inside an
//!   explanation is never run,
//!
//! and repairs the usual JSON mistakes (single quotes, unquoted keys, trailing commas, Python
//! literals, smart quotes, missing closing braces) before giving up. A block that still is not a
//! call comes back in [`Parsed::invalid`] with the reason, so the agent can ask for a retry.
use mv_ai::AiToolCall;
use serde_json::{Map, Value};

use crate::store::new_id;

/// The fence the prompt asks for.
pub const FENCE: &str = "```mangavox";
const TAGS: [(&str, &str); 3] = [("<tool_call>", "</tool_call>"), ("<function_call>", "</function_call>"), ("<tools>", "</tools>")];
const GENERIC_FENCES: [&str; 5] = ["```json", "```tool_call", "```tool", "```JSON", "```javascript"];

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Parsed {
    pub calls: Vec<AiToolCall>,
    /// Blocks meant as calls that could not be read, each with why (for the retry message).
    pub invalid: Vec<String>,
}

/// The tool calls in a reply (see the module docs for what is accepted).
pub fn parse(text: &str) -> Parsed {
    let mut out = Parsed::default();
    let mut blocks: Vec<String> = fenced(text, FENCE);
    for (open, close) in TAGS {
        blocks.extend(tagged(text, open, close));
    }
    if let Some(i) = text.find("[TOOL_CALLS]") {
        let rest = text[i + "[TOOL_CALLS]".len()..].trim();
        blocks.push(balanced(rest).unwrap_or(rest).to_string());
    }
    // Other fences and bare objects only when the reply has none of the above and is nothing
    // but calls (an example in an explanation is not one).
    if blocks.is_empty() {
        blocks = sole_fences(text).unwrap_or_default();
    }
    if blocks.is_empty() {
        if let Some(b) = bare_object(text) {
            blocks.push(b.to_string());
        }
    }
    for b in blocks {
        match read_block(&b) {
            Ok(calls) if !calls.is_empty() => out.calls.extend(calls),
            Ok(_) => out.invalid.push(format!("`{}` has no tool name (use {{\"tool\": \"name\", \"args\": {{...}}}})", preview(&b))),
            Err(e) => out.invalid.push(format!("`{}` is not valid JSON ({e})", preview(&b))),
        }
    }
    out
}

/// The message sent back when a reply's calls could not be read (capped retries in the agent).
pub fn retry_message(invalid: &[String]) -> String {
    format!(
        "Your tool call could not be read: {}. Reply again with exactly one block per call, valid JSON with double quotes:\n\n{FENCE}\n{{\"tool\": \"tool_name\", \"args\": {{\"key\": \"value\"}}}}\n```",
        invalid.join("; ")
    )
}

/// A reply without its call blocks (also one still streaming), for display.
pub fn visible(text: &str) -> std::borrow::Cow<'_, str> {
    let has_tag = TAGS.iter().any(|(o, _)| text.contains(o)) || text.contains("[TOOL_CALLS]");
    if !text.contains(FENCE) && !has_tag {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut s = strip(text, FENCE, "```");
    for (o, c) in TAGS {
        s = strip(&s, o, c);
    }
    if let Some(i) = s.find("[TOOL_CALLS]") {
        s.truncate(i);
    }
    std::borrow::Cow::Owned(s.trim().to_string())
}

fn strip(text: &str, open: &str, close: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        out.push_str(&rest[..start]);
        let after = start + open.len();
        match rest[after..].find(close) {
            Some(len) => rest = &rest[after + len + close.len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

fn preview(b: &str) -> String {
    let one: String = b.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > 80 {
        one.chars().take(80).collect::<String>() + "…"
    } else {
        one
    }
}

/// Bodies of `open … ```` fences; an unterminated last one runs to the end.
fn fenced(text: &str, open: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        let after = start + open.len();
        // ```json must not match ```jsonc etc.: the fence word ends at a newline or space.
        if rest[after..].chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_') {
            rest = &rest[after..];
            continue;
        }
        match rest[after..].find("```") {
            Some(len) => {
                out.push(rest[after..after + len].trim().to_string());
                rest = &rest[after + len + 3..];
            }
            None => {
                out.push(rest[after..].trim().to_string());
                break;
            }
        }
    }
    out.retain(|b| !b.is_empty());
    out
}

fn tagged(text: &str, open: &str, close: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        let after = start + open.len();
        match rest[after..].find(close) {
            Some(len) => {
                out.push(rest[after..after + len].trim().to_string());
                rest = &rest[after + len + close.len()..];
            }
            None => {
                out.push(rest[after..].trim().to_string());
                break;
            }
        }
    }
    out.retain(|b| !b.is_empty());
    out
}

fn looks_like_call(b: &str) -> bool {
    let low = b.to_lowercase();
    (low.contains("\"tool\"") || low.contains("'tool'") || low.contains("tool:")) || ((low.contains("\"name\"") || low.contains("'name'")) && (low.contains("arguments") || low.contains("\"args\"") || low.contains("parameters")))
}

/// The bodies of a reply made only of ```json / ```tool… fences holding calls (whitespace
/// between them; an unterminated last one counts). `None` when anything else is in the reply.
fn sole_fences(text: &str) -> Option<Vec<String>> {
    let mut out = vec![];
    let mut rest = text.trim();
    while !rest.is_empty() {
        let open = GENERIC_FENCES.iter().find(|f| rest.starts_with(**f) && !rest[f.len()..].chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_'))?;
        let after = &rest[open.len()..];
        let (body, next) = match after.find("```") {
            Some(len) => (&after[..len], &after[len + 3..]),
            None => (after, ""),
        };
        let body = body.trim();
        if !looks_like_call(body) {
            return None;
        }
        out.push(body.to_string());
        rest = next.trim();
    }
    (!out.is_empty()).then_some(out)
}

/// A `{"tool": …}` / `{"name", "arguments"}` object that is the whole reply (no fence; an
/// unclosed one at the end counts, the reply may have been cut).
fn bare_object(text: &str) -> Option<&str> {
    let t = text.trim();
    if !t.starts_with('{') {
        return None;
    }
    let cand = match balanced(t) {
        Some(b) if b.len() == t.len() => b,
        Some(_) => return None,
        None => t,
    };
    looks_like_call(cand).then_some(cand)
}

/// The leading balanced `{…}` or `[…]` of `s` (string-aware), if it closes.
fn balanced(s: &str) -> Option<&str> {
    let s = s.trim_start();
    let first = s.chars().next()?;
    if first != '{' && first != '[' {
        return None;
    }
    let mut depth = 0i32;
    let mut in_str: Option<char> = None;
    let mut esc = false;
    for (i, c) in s.char_indices() {
        if let Some(q) = in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == q {
                in_str = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => in_str = Some(c),
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[..i + c.len_utf8()]);
                }
            }
            _ => {}
        }
    }
    None
}

/// The calls in one block. `Err` when it is not JSON even after [`repair`].
fn read_block(b: &str) -> Result<Vec<AiToolCall>, String> {
    let b = b.trim().trim_start_matches("json").trim();
    let v = match serde_json::from_str::<Value>(b) {
        Ok(v) => v,
        Err(first) => serde_json::from_str::<Value>(&repair(b)).map_err(|_| first.to_string())?,
    };
    let mut out = vec![];
    collect(&v, &mut out);
    Ok(out)
}

fn collect(v: &Value, out: &mut Vec<AiToolCall>) {
    match v {
        Value::Array(items) => items.iter().for_each(|i| collect(i, out)),
        Value::Object(o) => {
            if let Some(list) = o.get("tool_calls").or_else(|| o.get("calls")).and_then(Value::as_array) {
                list.iter().for_each(|i| collect(i, out));
                return;
            }
            if let Some(f) = o.get("function").and_then(Value::as_object) {
                // OpenAI style: {"type": "function", "function": {"name", "arguments": "{…}"}}.
                return collect(&Value::Object(f.clone()), out);
            }
            let name = ["tool", "name", "tool_name", "action"].iter().find_map(|k| o.get(*k).and_then(Value::as_str)).map(clean_name).unwrap_or_default();
            if name.is_empty() {
                return;
            }
            let args = ["args", "arguments", "parameters", "input", "params"].iter().find_map(|k| o.get(*k)).map(args_of).unwrap_or_else(|| {
                // {"tool": "navigate", "page": "library"}: the other keys are the arguments.
                o.iter().filter(|(k, _)| !matches!(k.as_str(), "tool" | "name" | "tool_name" | "action" | "id" | "type")).map(|(k, v)| (k.clone(), v.clone())).collect()
            });
            out.push(AiToolCall { id: new_id("t"), name, args });
        }
        _ => {}
    }
}

/// Arguments given as an object, or as a JSON string holding one (OpenAI's `arguments`).
fn args_of(v: &Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m.clone(),
        Value::String(s) => serde_json::from_str::<Value>(s).or_else(|_| serde_json::from_str::<Value>(&repair(s))).ok().and_then(|v| v.as_object().cloned()).unwrap_or_default(),
        _ => Map::new(),
    }
}

/// `mangavox.navigate`, `mcp__mangavox__navigate`, `functions.navigate` → `navigate`.
pub fn clean_name(raw: &str) -> String {
    let mut n = raw.trim().trim_matches('`').trim();
    for p in ["mcp__mangavox__", "mangavox__", "mangavox.", "mangavox_", "functions.", "tools.", "app."] {
        if let Some(r) = n.strip_prefix(p) {
            n = r;
        }
    }
    n.trim_end_matches("()").to_string()
}

/// Fixes the JSON mistakes models make: single-quoted strings, unquoted keys, trailing commas,
/// `True`/`False`/`None`, smart quotes, raw newlines in strings and unclosed brackets.
pub fn repair(s: &str) -> String {
    let s = s.replace(['\u{201c}', '\u{201d}'], "\"").replace(['\u{2018}', '\u{2019}'], "'");
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 8);
    let mut stack: Vec<char> = vec![];
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' | '\'' => {
                // A string: copy it as a double-quoted one.
                let q = c;
                out.push('"');
                i += 1;
                while i < chars.len() {
                    let d = chars[i];
                    if d == '\\' && i + 1 < chars.len() {
                        // \' is not a JSON escape.
                        if chars[i + 1] == '\'' {
                            out.push('\'');
                        } else {
                            out.push(d);
                            out.push(chars[i + 1]);
                        }
                        i += 2;
                        continue;
                    }
                    if d == q {
                        break;
                    }
                    match d {
                        '"' => out.push_str("\\\""),
                        '\n' => out.push_str("\\n"),
                        '\r' => {}
                        '\t' => out.push_str("\\t"),
                        _ => out.push(d),
                    }
                    i += 1;
                }
                out.push('"');
                i += 1;
                continue;
            }
            '{' | '[' => {
                stack.push(if c == '{' { '}' } else { ']' });
                out.push(c);
            }
            '}' | ']' => {
                trim_trailing_comma(&mut out);
                if stack.last() == Some(&c) {
                    stack.pop();
                }
                out.push(c);
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '-') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                let next = chars[i..].iter().find(|c| !c.is_whitespace()).copied();
                if out.ends_with(|c: char| c.is_ascii_digit() || c == '.') {
                    // The exponent of a number (1e5).
                    out.push_str(&word);
                } else if next == Some(':') {
                    out.push('"');
                    out.push_str(&word);
                    out.push('"');
                } else {
                    out.push_str(match word.as_str() {
                        "True" | "true" => "true",
                        "False" | "false" => "false",
                        "None" | "null" | "undefined" | "nil" => "null",
                        _ => {
                            // A bare word as a value: make it a string.
                            out.push('"');
                            out.push_str(&word);
                            out.push('"');
                            continue;
                        }
                    });
                }
                continue;
            }
            _ => out.push(c),
        }
        i += 1;
    }
    trim_trailing_comma(&mut out);
    while let Some(c) = stack.pop() {
        out.push(c);
    }
    out
}

fn trim_trailing_comma(out: &mut String) {
    let t = out.trim_end().len();
    if out[..t].ends_with(',') {
        out.truncate(t - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(p: &Parsed) -> Vec<&str> {
        p.calls.iter().map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn the_asked_for_fence_parses() {
        let p = parse("Sure.\n```mangavox\n{\"tool\": \"studio_status\", \"args\": {}}\n```\n```mangavox\n[{\"name\": \"a\", \"args\": {\"x\": 1}}, {\"tool\": \"b\"}]\n```");
        assert_eq!(names(&p), ["studio_status", "a", "b"]);
        assert_eq!(p.calls[1].args["x"], 1);
        assert!(p.invalid.is_empty());
    }

    #[test]
    fn common_mistakes_are_repaired() {
        // Single quotes, unquoted keys, trailing commas, Python literals, no closing brace.
        let p = parse("```mangavox\n{tool: 'navigate', args: {page: 'library', force: True,},\n```");
        assert_eq!(names(&p), ["navigate"]);
        assert_eq!(p.calls[0].args["page"], "library");
        assert_eq!(p.calls[0].args["force"], true);
        // Smart quotes and an apostrophe inside a double-quoted string.
        let p = parse("```mangavox\n{\u{201c}tool\u{201d}: \u{201c}manga_search\u{201d}, \u{201c}args\u{201d}: {\"query\": \"Frieren's journey\"}}\n```");
        assert_eq!(p.calls[0].args["query"], "Frieren's journey");
        // Arguments next to the name instead of under args.
        let p = parse("```mangavox\n{\"tool\": \"navigate\", \"page\": \"anime\"}\n```");
        assert_eq!(p.calls[0].args["page"], "anime");
        // Prefixed names.
        assert_eq!(names(&parse("```mangavox\n{\"tool\": \"mcp__mangavox__go_back\"}\n```")), ["go_back"]);
    }

    #[test]
    fn other_shapes_small_models_use() {
        // Hermes / Qwen tags with OpenAI-style string arguments.
        let p = parse("<tool_call>\n{\"name\": \"search_anime\", \"arguments\": \"{\\\"query\\\": \\\"naruto\\\"}\"}\n</tool_call>");
        assert_eq!(names(&p), ["search_anime"]);
        assert_eq!(p.calls[0].args["query"], "naruto");
        // OpenAI function objects in a json fence.
        let p = parse("```json\n{\"type\": \"function\", \"function\": {\"name\": \"navigate\", \"arguments\": {\"page\": \"studio\"}}}\n```");
        assert_eq!(names(&p), ["navigate"]);
        // Mistral.
        assert_eq!(names(&parse("[TOOL_CALLS] [{\"name\": \"go_back\", \"arguments\": {}}]")), ["go_back"]);
        // A bare object with no fence at all, when it is the whole reply.
        assert_eq!(names(&parse("  {\"tool\": \"navigate\", \"args\": {\"page\": \"library\"}}\n")), ["navigate"]);
        assert_eq!(names(&parse("{\"name\": \"go_back\", \"arguments\": {}}")), ["go_back"]);
        // Two json fences and nothing else: both run.
        assert_eq!(names(&parse("```json\n{\"tool\": \"a\"}\n```\n\n```tool\n{\"tool\": \"b\"}\n```")), ["a", "b"]);
        // An unterminated fence (the reply was cut) still counts.
        assert_eq!(names(&parse("```mangavox\n{\"tool\": \"go_back\"}")), ["go_back"]);
    }

    #[test]
    fn ordinary_json_is_not_a_call() {
        assert!(parse("Here is the data:\n```json\n{\"title\": \"Solo\", \"chapters\": 3}\n```").calls.is_empty());
        assert!(parse("plain text").calls.is_empty());
        assert!(parse("plain text").invalid.is_empty());
    }

    #[test]
    fn example_json_in_an_explanation_is_not_run() {
        // A bare object inside prose.
        let p = parse("I'll open it. {\"tool\": \"navigate\", \"args\": {\"page\": \"library\"}}");
        assert!(p.calls.is_empty() && p.invalid.is_empty(), "{p:?}");
        let p = parse("To delete a chapter the call looks like {\"name\": \"delete_chapter\", \"arguments\": {\"id\": 3}} — want me to?");
        assert!(p.calls.is_empty(), "{p:?}");
        // A json fence with an explanation around it.
        let p = parse("Here is how a call looks:\n```json\n{\"tool\": \"studio_thumbnail_make\", \"args\": {\"manga\": \"Solo\"}}\n```\nShall I run it?");
        assert!(p.calls.is_empty(), "{p:?}");
        let p = parse("```json\n{\"tool\": \"go_back\"}\n```\nThat is the call.");
        assert!(p.calls.is_empty(), "{p:?}");
        // The explicit protocol still works inside prose.
        assert_eq!(names(&parse("Opening it.\n```mangavox\n{\"tool\": \"go_back\"}\n```\nDone.")), ["go_back"]);
    }

    #[test]
    fn unreadable_blocks_are_reported_for_a_retry() {
        let p = parse("```mangavox\nnavigate to library please\n```");
        assert!(p.calls.is_empty());
        assert_eq!(p.invalid.len(), 1, "{p:?}");
        let msg = retry_message(&p.invalid);
        assert!(msg.contains("could not be read") && msg.contains(FENCE));
        let p = parse("```mangavox\n{\"args\": {}}\n```");
        assert!(p.invalid[0].contains("no tool name"));
    }

    #[test]
    fn blocks_are_hidden_from_the_reply() {
        assert_eq!(visible("Sure.\n```mangavox\n{\"tool\": \"a\"}\n```"), "Sure.");
        assert_eq!(visible("Working…\n```mangavox\n{\"tool\": \"stu"), "Working…");
        assert_eq!(visible("Ok <tool_call>{\"name\":\"a\"}</tool_call>"), "Ok");
        assert_eq!(visible("plain"), "plain");
    }

    #[test]
    fn repair_keeps_valid_json_valid() {
        let j = r#"{"a": "x, y", "b": [1, 2], "c": {"d": null}}"#;
        assert_eq!(serde_json::from_str::<Value>(&repair(j)).unwrap(), serde_json::from_str::<Value>(j).unwrap());
    }
}
