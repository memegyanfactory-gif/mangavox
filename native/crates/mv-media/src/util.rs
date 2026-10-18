//! Helpers that mirror the JavaScript semantics the TypeScript services rely on.

use serde_json::Value;

/// Equivalent of `encodeURIComponent`: leaves `A-Z a-z 0-9 - _ . ! ~ * ' ( )` untouched.
pub fn encode_uri_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// `s || undefined` for optional strings: empty strings become `None`.
pub fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.is_empty())
}

/// `obj[key]` as a non-empty string (`obj.key || undefined`).
pub fn str_field(v: &Value, key: &str) -> Option<String> {
    non_empty(v.get(key).and_then(Value::as_str).map(str::to_owned))
}

/// `String(v)` for strings and numbers; `None` for null, bool, arrays and objects.
pub fn value_to_string(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// `Number(v)` for numbers and numeric strings; `None` when not a finite number.
pub fn value_to_f64(v: Option<&Value>) -> Option<f64> {
    let n = match v? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    n.is_finite().then_some(n)
}

/// `s.match(/\d{4}/)`: the first run of four ASCII digits.
pub fn first_four_digits(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    if bytes.len() < 4 {
        return None;
    }
    (0..=bytes.len() - 4)
        .find(|&i| bytes[i..i + 4].iter().all(u8::is_ascii_digit))
        .map(|i| &s[i..i + 4])
}

/// `parseInt(s, 10)`: the leading integer of `s`, or `None` when there is none.
pub fn parse_leading_int(s: &str) -> Option<i64> {
    let t = s.trim_start();
    let mut end = 0;
    for (i, c) in t.char_indices() {
        let ok = c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+'));
        if !ok {
            break;
        }
        end = i + c.len_utf8();
    }
    t[..end].parse::<i64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn encodes_like_encode_uri_component() {
        assert_eq!(encode_uri_component("a b&c/é"), "a%20b%26c%2F%C3%A9");
        assert_eq!(encode_uri_component("it's (ok)!*~"), "it's%20(ok)!*~");
    }

    #[test]
    fn four_digit_year() {
        assert_eq!(first_four_digits("2008-2013"), Some("2008"));
        assert_eq!(first_four_digits("abc 12345"), Some("1234"));
        assert_eq!(first_four_digits("198"), None);
    }

    #[test]
    fn parse_int_like_js() {
        assert_eq!(parse_leading_int("2010-2015"), Some(2010));
        assert_eq!(parse_leading_int("  -5x"), Some(-5));
        assert_eq!(parse_leading_int("x"), None);
    }

    #[test]
    fn number_and_string_coercions() {
        assert_eq!(value_to_string(Some(&json!(2010))), Some("2010".into()));
        assert_eq!(value_to_string(Some(&json!(null))), None);
        assert_eq!(value_to_f64(Some(&json!("8.8"))), Some(8.8));
        assert_eq!(value_to_f64(Some(&json!("abc"))), None);
        assert_eq!(str_field(&json!({"a": ""}), "a"), None);
    }
}
