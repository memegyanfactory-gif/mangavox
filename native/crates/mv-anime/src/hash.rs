//! Canonical JSON and SHA-256 hashes for the staleness channels (design 5.5): a picture is
//! outdated when the request the engine would send now hashes differently from the one that
//! made the selected take. Key order never matters (keys are sorted recursively).
//!
//! The channels are kept apart on purpose: the picture channel hashes only what is sent to the
//! picture model ([`request_hash`]), the sound channel only what makes a line's voice
//! ([`sound_hash`]), and the timing channel ([`value_hash`] of the cut's inputs, built by the cut
//! step) only what moves cuts. Changing a voice therefore never outdates a picture.

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::model::{Emotion, GenRequest};

/// Compact JSON with object keys sorted recursively (no reliance on serde_json's key order).
pub fn canonical(v: &Value) -> String {
    let mut out = String::new();
    write_canonical(v, &mut out);
    out
}

fn write_canonical(v: &Value, out: &mut String) {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                if let Some(x) = map.get(k) {
                    write_canonical(x, out);
                }
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(x, out);
            }
            out.push(']');
        }
        // Scalars: serde_json's compact form (strings escaped, numbers as written).
        other => out.push_str(&other.to_string()),
    }
}

/// Lower-case hex SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Lower-case hex SHA-256 of a file's content (a reference picture's or a voice clip's hash).
pub fn file_hash(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path).with_context(|| format!("Could not open {} to check whether it changed.", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).with_context(|| format!("Could not read {} to check whether it changed.", path.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// `sha256_hex(canonical(v))`.
pub fn value_hash(v: &Value) -> String {
    sha256_hex(canonical(v).as_bytes())
}

/// The picture channel: hash of exactly what is sent (final prompt + every reference's content
/// hash + model, aspect, resolution).
pub fn request_hash(req: &GenRequest) -> String {
    // A plain struct of strings: serializing it cannot fail; Null would still hash stably.
    value_hash(&serde_json::to_value(req).unwrap_or(Value::Null))
}

/// The sound channel of a line: spoken text, voice, reference clip hash, speed, emotion, engine.
pub fn sound_hash(spoken: &str, voice_id: &str, reference_hash: Option<&str>, speed: f32, emotion: &Emotion, engine: &str) -> String {
    let emotion = serde_json::to_value(emotion).unwrap_or(Value::Null);
    value_hash(&json!({
        "spoken": spoken,
        "voice_id": voice_id,
        "reference": reference_hash,
        // Rounded so a speed read back from JSON hashes like the one that was written.
        "speed": (speed as f64 * 1000.0).round() / 1000.0,
        "emotion": emotion,
        "engine": engine,
    }))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 15) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_sha256() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn canonical_sorts_nested_keys_and_escapes() {
        let v = json!({"b": [ {"z": 1, "a": "x\"y"} ], "a": null});
        assert_eq!(canonical(&v), r#"{"a":null,"b":[{"a":"x\"y","z":1}]}"#);
    }
}
