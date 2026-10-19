//! Chat threads + assistant preferences, persisted to `~/.mangavox/assistant/threads.json`.
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use mv_ai::{AiAccessMode, AiEffort, AiMessage, AiProviderId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

static COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Process-unique id (time + counter).
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}-{:x}-{:x}", now_ms(), COUNTER.fetch_add(1, Ordering::Relaxed))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepState {
    AwaitingApproval,
    Running,
    Done,
    Denied,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolStep {
    pub id: String,
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub args: Value,
    /// "read" | "act" | "destructive"
    pub risk: String,
    pub state: StepState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemStatus {
    Streaming,
    Done,
    Error,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatItem {
    pub id: String,
    pub role: ItemRole,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub activity: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<ToolStep>,
    #[serde(default = "done_status")]
    pub status: ItemStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// How long the model reasoned before it started answering ("Thought for 12s").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_ms: Option<u64>,
    pub created_at: u64,
}

fn done_status() -> ItemStatus {
    ItemStatus::Done
}

impl ChatItem {
    pub fn user(text: impl Into<String>) -> Self {
        Self { id: new_id("u"), role: ItemRole::User, text: text.into(), reasoning: None, activity: vec![], steps: vec![], status: ItemStatus::Done, error: None, model: None, reasoning_ms: None, created_at: now_ms() }
    }
    pub fn assistant(model: &str) -> Self {
        Self {
            id: new_id("a"),
            role: ItemRole::Assistant,
            text: String::new(),
            reasoning: None,
            activity: vec![],
            steps: vec![],
            status: ItemStatus::Streaming,
            error: None,
            model: Some(model.to_string()),
            reasoning_ms: None,
            created_at: now_ms(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: String,
    pub title: String,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub items: Vec<ChatItem>,
    /// What the model sees (system prompt excluded).
    #[serde(default)]
    pub history: Vec<AiMessage>,
}

impl Thread {
    pub fn new() -> Self {
        let now = now_ms();
        Self { id: new_id("t"), title: "New chat".into(), created_at: now, updated_at: now, items: vec![], history: vec![] }
    }
}

impl Default for Thread {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prefs {
    pub provider: AiProviderId,
    #[serde(default)]
    pub model: String,
    pub effort: AiEffort,
    pub access: AiAccessMode,
    /// `provider::model` keys.
    #[serde(default)]
    pub favourites: Vec<String>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self { provider: AiProviderId::Ollama, model: String::new(), effort: AiEffort::Medium, access: AiAccessMode::Ask, favourites: vec![] }
    }
}

pub fn model_key(p: AiProviderId, model: &str) -> String {
    format!("{}::{model}", p.as_str())
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Store {
    #[serde(default)]
    pub threads: Vec<Thread>,
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub prefs: Prefs,
}

pub fn store_path() -> PathBuf {
    mv_core::paths::root().join("assistant").join("threads.json")
}

impl Store {
    pub fn load() -> Self {
        Self::load_from(&store_path())
    }

    pub fn load_from(path: &std::path::Path) -> Self {
        let mut s: Store = std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        // A crash mid-stream must not leave a spinner behind.
        for t in &mut s.threads {
            for i in &mut t.items {
                if i.status == ItemStatus::Streaming {
                    i.status = ItemStatus::Stopped;
                }
            }
        }
        s
    }

    /// Fire-and-forget write on a helper thread; later snapshots always win.
    pub fn save(&self) {
        self.save_to(store_path());
    }

    pub fn save_to(&self, path: PathBuf) {
        static GEN: AtomicU64 = AtomicU64::new(0);
        static WRITTEN: Mutex<u64> = Mutex::new(0);
        let Ok(json) = serde_json::to_string(self) else { return };
        let generation = GEN.fetch_add(1, Ordering::SeqCst) + 1;
        std::thread::spawn(move || {
            let mut last = WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
            if generation < *last {
                return;
            }
            *last = generation;
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, &path).is_err() {
                let _ = std::fs::remove_file(&tmp);
            }
        });
    }

    pub fn thread(&self, id: &str) -> Option<&Thread> {
        self.threads.iter().find(|t| t.id == id)
    }

    pub fn thread_mut(&mut self, id: &str) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|t| t.id == id)
    }

    pub fn active_thread(&self) -> Option<&Thread> {
        self.active.as_deref().and_then(|id| self.thread(id))
    }

    pub fn new_thread(&mut self) -> String {
        // Reuse an empty chat instead of piling up blanks.
        if let Some(t) = self.threads.iter().find(|t| t.items.is_empty()) {
            let id = t.id.clone();
            self.active = Some(id.clone());
            return id;
        }
        let t = Thread::new();
        let id = t.id.clone();
        self.threads.insert(0, t);
        self.active = Some(id.clone());
        id
    }

    pub fn delete_thread(&mut self, id: &str) {
        let _ = self.take_thread(id);
    }

    /// Removes a thread and returns it with its position, so an Undo can put it back.
    pub fn take_thread(&mut self, id: &str) -> Option<(usize, Thread)> {
        let ix = self.threads.iter().position(|t| t.id == id)?;
        let t = self.threads.remove(ix);
        if self.active.as_deref() == Some(id) {
            self.active = self.threads.first().map(|t| t.id.clone());
        }
        Some((ix, t))
    }

    /// Puts back a thread from [`Store::take_thread`] (Undo), optionally making it active again.
    pub fn restore_thread(&mut self, ix: usize, thread: Thread, make_active: bool) {
        if self.thread(&thread.id).is_some() {
            return;
        }
        let id = thread.id.clone();
        self.threads.insert(ix.min(self.threads.len()), thread);
        if make_active {
            self.active = Some(id);
        }
    }

    pub fn rename_thread(&mut self, id: &str, title: &str) {
        let title = title.trim();
        if title.is_empty() {
            return;
        }
        if let Some(t) = self.thread_mut(id) {
            t.title = title.chars().take(80).collect();
        }
    }

    /// Threads whose title or text contains `q`, newest first.
    pub fn search(&self, q: &str) -> Vec<&Thread> {
        let q = q.trim().to_lowercase();
        let mut v: Vec<&Thread> = self
            .threads
            .iter()
            .filter(|t| q.is_empty() || t.title.to_lowercase().contains(&q) || t.items.iter().any(|i| i.text.to_lowercase().contains(&q)))
            .collect();
        v.sort_by_key(|t| std::cmp::Reverse(t.updated_at));
        v
    }

    pub fn toggle_favourite(&mut self, p: AiProviderId, model: &str) {
        let key = model_key(p, model);
        if let Some(i) = self.prefs.favourites.iter().position(|k| *k == key) {
            self.prefs.favourites.remove(i);
        } else {
            self.prefs.favourites.push(key);
        }
    }
}

pub fn title_from(text: &str) -> String {
    let clean = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.is_empty() {
        return "New chat".into();
    }
    if clean.chars().count() > 48 {
        format!("{}…", clean.chars().take(46).collect::<String>())
    } else {
        clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_truncates() {
        assert_eq!(title_from("  hello   world "), "hello world");
        assert_eq!(title_from(""), "New chat");
        assert!(title_from(&"x".repeat(100)).ends_with('…'));
    }

    #[test]
    fn thread_lifecycle_and_search() {
        let mut s = Store::default();
        let a = s.new_thread();
        // an empty thread is reused
        assert_eq!(s.new_thread(), a);
        s.thread_mut(&a).unwrap().items.push(ChatItem::user("find berserk"));
        s.rename_thread(&a, "  Manga help ");
        assert_eq!(s.thread(&a).unwrap().title, "Manga help");
        let b = s.new_thread();
        assert_ne!(a, b);
        assert_eq!(s.search("berserk").len(), 1);
        assert_eq!(s.search("").len(), 2);
        s.delete_thread(&b);
        assert_eq!(s.active.as_deref(), Some(a.as_str()));
    }

    #[test]
    fn deleted_threads_can_be_restored() {
        let mut s = Store::default();
        let a = s.new_thread();
        s.thread_mut(&a).unwrap().items.push(ChatItem::user("one"));
        let b = s.new_thread();
        s.thread_mut(&b).unwrap().items.push(ChatItem::user("two"));
        let (ix, t) = s.take_thread(&b).unwrap();
        assert_eq!(ix, 0);
        assert_eq!(s.active.as_deref(), Some(a.as_str()));
        s.restore_thread(ix, t.clone(), true);
        assert_eq!(s.threads[0].id, b);
        assert_eq!(s.active.as_deref(), Some(b.as_str()));
        // Restoring twice is a no-op.
        s.restore_thread(ix, t, false);
        assert_eq!(s.threads.len(), 2);
        assert!(s.take_thread("missing").is_none());
    }

    #[test]
    fn roundtrip_and_favourites() {
        let mut s = Store::default();
        s.toggle_favourite(AiProviderId::Openai, "gpt-5");
        assert_eq!(s.prefs.favourites, vec!["openai::gpt-5"]);
        let id = s.new_thread();
        s.thread_mut(&id).unwrap().history.push(AiMessage::user("hi"));
        let json = serde_json::to_string(&s).unwrap();
        let back: Store = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
        s.toggle_favourite(AiProviderId::Openai, "gpt-5");
        assert!(s.prefs.favourites.is_empty());
    }
}
