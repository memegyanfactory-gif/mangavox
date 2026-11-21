//! Lenient JSON from model replies: a port of `mv-recap`'s private `close_brackets` and
//! `salvage_json` (a cut-off reply still yields what it finished). Strict extraction is
//! `mv_recap::extract_json` / `extract_whole_json`.
//!
//! Every LLM step parses its reply with [`parse_reply`], then reads the value into the model
//! types with `#[serde(default)]`, so a missing field is a default, never a failure.

use serde_json::Value;

/// Closes the brackets and strings a cut-off reply left open, then parses it.
///
/// For a reply whose JSON is complete except for its last closing brackets (some free models
/// stop after the innermost `}`). Nothing the model said is dropped. `None` when the reply stops
/// anywhere but right after a closing bracket (inside a string, after a key or a comma: really
/// cut off; [`salvage_json`] handles those), when it is whole already, or when brackets mismatch.
pub fn close_brackets(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let t = text[start..].trim_end();
    let t = t.strip_suffix("```").map(str::trim_end).unwrap_or(t);
    if !t.ends_with(['}', ']']) {
        return None;
    }
    let (mut stack, mut in_str, mut esc) = (Vec::<u8>::new(), false, false);
    for &c in t.as_bytes() {
        if in_str {
            match (esc, c) {
                (true, _) => esc = false,
                (_, b'\\') => esc = true,
                (_, b'"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'{' => stack.push(b'}'),
            b'[' => stack.push(b']'),
            b'}' | b']' => {
                if stack.pop() != Some(c) {
                    return None;
                }
            }
            _ => {}
        }
    }
    if in_str || stack.is_empty() {
        return None;
    }
    let closed = format!("{t}{}", stack.iter().rev().map(|&c| c as char).collect::<String>());
    serde_json::from_str::<Value>(&closed).ok().filter(Value::is_object)
}

/// Keeps every complete element of the reply's top-level arrays and drops the broken tail.
///
/// Rescues a reply cut off in the middle of its JSON (the model ran out of room, the connection
/// dropped): closes the open string, then the open arrays and objects, and drops a dangling
/// comma. When the tail is too ragged to close (a key without its value) it is cut back to the
/// last comma that leaves a whole value; a sentence cut in its middle is cut back too rather
/// than kept as if finished. Accepted only when it parses into a non-empty object; `None` for a
/// reply that is whole already.
pub fn salvage_json(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let t = &text[start..];
    let (mut stack, mut in_str, mut esc) = (Vec::<u8>::new(), false, false);
    // Where each comma outside a string sits, with what was open at that point.
    let mut commas: Vec<(usize, Vec<u8>)> = vec![];
    for (i, &c) in t.as_bytes().iter().enumerate() {
        if in_str {
            match (esc, c) {
                (true, _) => esc = false,
                (_, b'\\') => esc = true,
                (_, b'"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'{' => stack.push(b'}'),
            b'[' => stack.push(b']'),
            b'}' | b']' => {
                stack.pop();
            }
            b',' => commas.push((i, stack.clone())),
            _ => {}
        }
    }
    if stack.is_empty() {
        return None;
    }
    let mut head = t.to_string();
    if in_str {
        if esc {
            head.pop();
        }
        head.push('"');
    }
    let head = head.trim_end().trim_end_matches(',');
    let closed = |h: &str, open: &[u8]| format!("{h}{}", open.iter().rev().map(|&c| c as char).collect::<String>());
    let ok = |s: String| serde_json::from_str::<Value>(&s).ok().filter(|v| v.as_object().is_some_and(|o| !o.is_empty()));
    let whole = || ok(closed(head, &stack));
    let by_commas = || commas.iter().rev().take(8).find_map(|(at, open)| ok(closed(&t[..*at], open)));
    if in_str {
        by_commas().or_else(whole)
    } else {
        whole().or_else(by_commas)
    }
}

/// The reply's JSON by every means above: whole reply, first JSON block, closed brackets, salvage.
///
/// Only an object counts as an answer (every step asks for one). `None` when nothing in the
/// reply can be read; the caller then asks once more or fails with a readable message.
pub fn parse_reply(text: &str) -> Option<Value> {
    let trimmed = strip_fences(text.trim());
    if let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(trimmed) {
        return Some(v);
    }
    mv_recap::extract_whole_json(text).or_else(|| close_brackets(text)).or_else(|| salvage_json(text)).or_else(|| mv_recap::extract_json(text))
}

/// The inside of a single ```json fence around the whole reply.
fn strip_fences(t: &str) -> &str {
    let Some(rest) = t.strip_prefix("```") else {
        return t;
    };
    let rest = rest.trim_start_matches(|c: char| c.is_ascii_alphanumeric());
    rest.strip_suffix("```").unwrap_or(rest).trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn whole_and_fenced_replies() {
        assert_eq!(parse_reply("{\"a\":1}"), Some(json!({"a": 1})));
        assert_eq!(parse_reply("```json\n{\"a\":1}\n```"), Some(json!({"a": 1})));
        assert_eq!(parse_reply("Here you go: {\"a\":{\"b\":[1,2]}} hope it helps"), Some(json!({"a": {"b": [1, 2]}})));
        assert_eq!(parse_reply("no json at all"), None);
        assert_eq!(parse_reply("[1,2]"), None);
    }

    #[test]
    fn missing_closing_brackets_are_added() {
        let v = parse_reply(r#"{"scenes":[{"title":"a","beats":[{"lines":[{"spoken":"Hi"}]}]}"#).unwrap_or_default();
        assert_eq!(v["scenes"][0]["beats"][0]["lines"][0]["spoken"], "Hi");
        assert!(close_brackets(r#"{"a":[1}"#).is_none(), "mismatched");
        assert!(close_brackets(r#"{"a":1}"#).is_none(), "already whole");
    }

    #[test]
    fn a_cut_off_reply_keeps_what_was_finished() {
        let cut = "{\"scenes\":[{\"title\":\"A\"},{\"title\":\"B\"},{\"title\":\"C is cut";
        let v = parse_reply(cut).unwrap_or_default();
        let scenes = v["scenes"].as_array().cloned().unwrap_or_default();
        assert_eq!(scenes.len(), 2, "the clipped scene is not kept as if finished: {v}");
        assert_eq!(salvage_json("{\"a\":1,\"b\":"), Some(json!({"a": 1})));
        assert_eq!(salvage_json("{\"a\":\"x } \\"), Some(json!({"a": "x } "})));
        assert!(salvage_json("{").is_none());
        assert!(salvage_json("{\"a\":1}").is_none());
    }
}
