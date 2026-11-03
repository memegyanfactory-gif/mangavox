//! Small shared helpers: atomic JSON files, slugs, time, tokens.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};

pub(crate) fn write_json<T: Serialize>(path: &Path, v: &T) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(v)?).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

pub(crate) fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// `"Ganghyeok's Gym #2"` -> `"ganghyeok-s-gym-2"` (at most 40 characters).
pub(crate) fn slug(s: &str) -> String {
    let s: String = s.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    let s: String = s.chars().take(40).collect();
    let s = s.trim_end_matches('-').to_string();
    if s.is_empty() {
        "asset".into()
    } else {
        s
    }
}

const STOP: [&str; 14] = ["the", "a", "an", "of", "and", "s", "with", "in", "on", "at", "to", "for", "his", "her"];

/// Lower-case word tokens without filler words, for name matching.
pub(crate) fn tokens(s: &str) -> Vec<String> {
    let mut v: Vec<String> = s
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !STOP.contains(w))
        .map(|w| w.strip_suffix('s').filter(|b| b.len() > 2).unwrap_or(w).to_string())
        .collect();
    v.dedup();
    v
}

pub(crate) fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}
