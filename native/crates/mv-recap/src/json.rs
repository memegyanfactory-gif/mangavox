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
