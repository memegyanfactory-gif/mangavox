//! Suggestions in the chat: buttons the model puts under a reply (`suggest_replies`) and the
//! app's own proactive cards ("Ch. 12 failed at Audio: retry?"). Crates register a suggester;
//! the dock runs them on a background thread and shows what they return. Clicking one sends its
//! prompt as the user's message, so the agent still names what it does and asks where it should.
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    /// Stable per situation ("studio-failed-<chapter>"), so a dismissed card stays dismissed.
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// Sent as the user's message when clicked.
    pub prompt: String,
}

impl Suggestion {
    pub fn new(id: impl Into<String>, title: impl Into<String>, detail: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self { id: id.into(), title: title.into(), detail: detail.into(), prompt: prompt.into() }
    }
}

pub type Suggester = Arc<dyn Fn() -> Vec<Suggestion> + Send + Sync>;

static SUGGESTERS: RwLock<Vec<(&'static str, Suggester)>> = RwLock::new(Vec::new());

/// Adds (or replaces, by `name`) a source of proactive suggestions. It runs on a background
/// thread, so it may read files and the database, but must be quick.
pub fn register(name: &'static str, f: impl Fn() -> Vec<Suggestion> + Send + Sync + 'static) {
    let mut g = SUGGESTERS.write().unwrap_or_else(|e| e.into_inner());
    g.retain(|(n, _)| *n != name);
    g.push((name, Arc::new(f)));
}

/// Every registered suggester's cards, most urgent source first, without duplicates (blocking).
pub fn collect() -> Vec<Suggestion> {
    let all: Vec<Suggester> = SUGGESTERS.read().unwrap_or_else(|e| e.into_inner()).iter().map(|(_, f)| f.clone()).collect();
    let mut out: Vec<Suggestion> = vec![];
    for f in all {
        for s in f() {
            if !out.iter().any(|o| o.id == s.id) {
                out.push(s);
            }
        }
    }
    out
}

/// The `suggest_replies` argument: `[{label, prompt}]`, at most 4, empty ones dropped.
pub fn parse_replies(v: &Value) -> Vec<Suggestion> {
    let items: Vec<Value> = match v {
        Value::Array(a) => a.clone(),
        // Some models send the array as a JSON string.
        Value::String(s) => serde_json::from_str::<Vec<Value>>(s).unwrap_or_default(),
        _ => vec![],
    };
    items
        .iter()
        .filter_map(|it| {
            let label = it.get("label").and_then(Value::as_str).unwrap_or("").trim();
            let prompt = it.get("prompt").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty()).unwrap_or(label);
            (!label.is_empty() && !prompt.is_empty()).then(|| Suggestion::new(format!("reply-{label}"), truncate(label, 60), "", prompt))
        })
        .take(4)
        .collect()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max - 1).collect::<String>() + "…"
    }
}

/// Cards still to show: not dismissed, at most `max`.
pub fn visible(all: &[Suggestion], dismissed: &[String], max: usize) -> Vec<Suggestion> {
    all.iter().filter(|s| !dismissed.contains(&s.id)).take(max).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn replies_are_parsed_and_capped() {
        let v = json!([
            { "label": "Generate Ch. 12", "prompt": "Generate the recap for Ch. 12 of Solo" },
            { "label": "", "prompt": "x" },
            { "label": "Export" },
            { "label": "a", "prompt": "a" }, { "label": "b", "prompt": "b" }, { "label": "c", "prompt": "c" }
        ]);
        let r = parse_replies(&v);
        assert_eq!(r.len(), 4);
        assert_eq!(r[0].prompt, "Generate the recap for Ch. 12 of Solo");
        assert_eq!(r[1].title, "Export");
        assert_eq!(r[1].prompt, "Export");
        let s = parse_replies(&json!("[{\"label\":\"Go\",\"prompt\":\"Go now\"}]"));
        assert_eq!(s[0].prompt, "Go now");
        assert!(parse_replies(&json!(null)).is_empty());
    }

    #[test]
    fn registered_suggesters_are_merged_without_duplicates() {
        register("test-a", || vec![Suggestion::new("x", "X", "", "do x"), Suggestion::new("y", "Y", "", "do y")]);
        register("test-b", || vec![Suggestion::new("x", "X again", "", "do x")]);
        // Registering again under the same name replaces it.
        register("test-b", || vec![Suggestion::new("z", "Z", "", "do z")]);
        let all = collect();
        let ids: Vec<&str> = all.iter().map(|s| s.id.as_str()).filter(|i| ["x", "y", "z"].contains(i)).collect();
        assert_eq!(ids, ["x", "y", "z"]);
        let v = visible(&all, &["y".to_string()], 10);
        assert!(!v.iter().any(|s| s.id == "y"));
    }
}
