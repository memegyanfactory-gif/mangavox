//! Tolerant JSON handling for LLM output (code fences, chatter around the object, sloppy types).

use serde_json::Value;

use crate::types::Bbox;

/// Find the first parseable JSON object in `text`. Handles ``` fences, prose before/after, and braces
/// inside strings.
pub fn extract_json(text: &str) -> Option<Value> {
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(rel) = text[from..].find('{') {
        let start = from + rel;
        if let Some(end) = matching_brace(bytes, start) {
            if let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(&text[start..=end]) {
                return Some(v);
            }
        }
        from = start + 1;
    }
    None
}

/// True when the first `{` never closes: the reply was cut off. `extract_json` alone would then
/// hand back the first whole inner object (one segment), which is not the answer.
pub fn is_cut_off(text: &str) -> bool {
    text.find('{').is_some_and(|start| matching_brace(text.as_bytes(), start).is_none())
}

/// `extract_json`, but a cut-off reply is not a whole answer.
pub fn extract_whole_json(text: &str) -> Option<Value> {
    if is_cut_off(text) {
        return None;
    }
    extract_json(text)
}

/// Rescue a reply that was cut off in the middle of its JSON (the model ran out of room, or the
/// connection dropped): close the open string, then the open arrays and objects, and drop a dangling
/// comma. When the tail is too ragged to close (a key without its value), it is cut back to the last
/// comma that leaves a whole value. Accepted only when it parses into a non-empty object.
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
    // Whole already (or broken some other way): nothing was cut off.
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
    // A sentence cut off in its middle must not be spoken as if it were finished: the cut-back is
    // tried first then, and the closed string only when there is nothing whole before it.
    if in_str {
        by_commas().or_else(whole)
    } else {
        whole().or_else(by_commas)
    }
}

/// Index of the `}` closing the `{` at `start`, aware of strings and escapes.
fn matching_brace(b: &[u8], start: usize) -> Option<usize> {
    let (mut depth, mut in_str, mut esc) = (0usize, false, false);
    for (i, &c) in b.iter().enumerate().skip(start) {
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
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// String value of `v[key]`; numbers are stringified, null / missing give "".
pub(crate) fn str_of(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// Like [`str_of`] but empty / "null" / "none" become `None`.
pub(crate) fn opt_str(v: &Value, key: &str) -> Option<String> {
    let s = str_of(v, key);
    (!s.is_empty() && !matches!(s.to_lowercase().as_str(), "null" | "none" | "unknown" | "n/a")).then_some(s)
}

pub(crate) fn str_list(v: &Value, key: &str) -> Vec<String> {
    match v.get(key) {
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        Some(Value::String(s)) => s.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        _ => vec![],
    }
}

pub(crate) fn num_of(v: &Value, key: &str) -> Option<f64> {
    match v.get(key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// `[x0,y0,x1,y1]` in 0..1 (0..100 / 0..1000 scales are detected and rescaled).
pub(crate) fn bbox_of(v: &Value, key: &str) -> Option<Bbox> {
    let a = v.get(key)?.as_array()?;
    if a.len() != 4 {
        return None;
    }
    let mut n = [0f32; 4];
    for (i, x) in a.iter().enumerate() {
        n[i] = match x {
            Value::Number(x) => x.as_f64()? as f32,
            Value::String(s) => s.trim().parse().ok()?,
            _ => return None,
        };
    }
    let max = n.iter().cloned().fold(0.0, f32::max);
    let scale = if max <= 1.5 {
        1.0
    } else if max <= 100.0 {
        100.0
    } else {
        1000.0
    };
    Bbox(n.map(|x| x / scale)).normalized()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_off_reply_is_not_mistaken_for_its_first_inner_object() {
        let cut = "{\"segments\":[{\"page\":1,\"panel\":1,\"text\":\"Ruko draws.\"},{\"page\":2,\"te";
        assert!(is_cut_off(cut));
        assert!(extract_whole_json(cut).is_none());
        assert!(extract_json(cut).is_some());
        assert!(salvage_json(cut).unwrap().get("segments").is_some());
        assert!(extract_whole_json("Here: {\"a\":1}").is_some());
    }

    #[test]
    fn a_cut_off_reply_is_closed_and_a_whole_one_is_left_alone() {
        // Cut between items: the open array and object are closed, the dangling comma dropped.
        let v = salvage_json("Sure ```json
{\"title\":\"T\",\"segments\":[{\"page\":1,\"text\":\"A.\"},").unwrap();
        assert_eq!(v["segments"].as_array().unwrap().len(), 1);
        assert_eq!(v["title"], "T");
        // Cut inside a sentence: the clipped item is not kept as if finished.
        let v = salvage_json("{\"segments\":[{\"page\":1,\"text\":\"A.\"},{\"page\":2,\"text\":\"Kai").unwrap();
        assert_eq!(v["segments"][1].get("text"), None);
        // Cut after a key: cut back to the last whole value.
        let v = salvage_json("{\"a\":1,\"b\":").unwrap();
        assert_eq!(v, serde_json::json!({"a": 1}));
        // Only a string to close.
        assert_eq!(salvage_json("{\"a\":\"x").unwrap()["a"], "x");
        // An escape cut in half, and a brace inside a string.
        assert_eq!(salvage_json("{\"a\":\"x } \\").unwrap()["a"], "x } ");
        // Nothing to save: no object, an empty one, or one that is whole already.
        assert!(salvage_json("no json").is_none());
        assert!(salvage_json("{").is_none());
        assert!(salvage_json("{\"a\":1}").is_none());
    }
}
