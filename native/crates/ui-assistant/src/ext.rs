//! Tools other crates add to the assistant. A page crate that depends on this one (Studio's Video
//! editor) cannot be called from here, so it registers a [`ToolProvider`] instead: its tools then
//! appear in the tool table, run through the same approval policy and step rows as the built-in
//! ones, and its prompt lines join the system prompt.
use std::sync::{Arc, RwLock};

use futures::future::BoxFuture;
use serde_json::{Map, Value};

use crate::tools::{AppTool, Phrases, ToolResult};

pub trait ToolProvider: Send + Sync + 'static {
    /// The provider's tools (names should share a prefix, e.g. `video_`).
    fn tools(&self) -> Vec<AppTool>;
    /// How a call is worded in the chat ("Change the captions"), when it is this provider's.
    fn phrases(&self, name: &str, args: &Map<String, Value>) -> Option<Phrases>;
    /// Runs a call, when it is this provider's.
    fn run(&self, name: &str, args: Map<String, Value>) -> Option<BoxFuture<'static, ToolResult>>;
    /// Lines added to the system prompt while tools are on.
    fn prompt(&self) -> Option<String> {
        None
    }
}

static PROVIDERS: RwLock<Vec<(&'static str, Arc<dyn ToolProvider>)>> = RwLock::new(Vec::new());
/// The chat turn running now: a number that changes every turn, and the user's message.
static TURN: std::sync::Mutex<Option<(u64, String)>> = std::sync::Mutex::new(None);
static TURNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The chat turn whose tools are running (so a provider can fold one turn's edits into one undo
/// step, labelled with what was asked). Set when a turn starts; the last turn stays until the next.
pub fn current_turn() -> Option<(u64, String)> {
    TURN.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub(crate) fn begin_turn(prompt: &str) {
    let n = TURNS.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    *TURN.lock().unwrap_or_else(|e| e.into_inner()) = Some((n, prompt.to_string()));
}

/// Adds (or replaces) the provider registered as `id`.
pub fn register(id: &'static str, provider: Arc<dyn ToolProvider>) {
    let mut g = PROVIDERS.write().unwrap_or_else(|e| e.into_inner());
    g.retain(|(k, _)| *k != id);
    g.push((id, provider));
}

pub(crate) fn providers() -> Vec<Arc<dyn ToolProvider>> {
    PROVIDERS.read().unwrap_or_else(|e| e.into_inner()).iter().map(|(_, p)| p.clone()).collect()
}

pub(crate) fn tools() -> Vec<AppTool> {
    providers().iter().flat_map(|p| p.tools()).collect()
}

pub(crate) fn phrases(name: &str, args: &Map<String, Value>) -> Option<Phrases> {
    providers().iter().find_map(|p| p.phrases(name, args))
}

pub(crate) fn run(name: &str, args: &Map<String, Value>) -> Option<BoxFuture<'static, ToolResult>> {
    providers().iter().find_map(|p| p.run(name, args.clone()))
}

pub(crate) fn prompt() -> Vec<String> {
    providers().iter().filter_map(|p| p.prompt()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{Risk, ToolOutput};
    use futures::FutureExt;
    use serde_json::json;

    struct Echo;
    impl ToolProvider for Echo {
        fn tools(&self) -> Vec<AppTool> {
            vec![AppTool { name: "echo_test", description: "Echo".into(), parameters: json!({"type": "object", "properties": {}, "required": []}), risk: Risk::Read }]
        }
        fn phrases(&self, name: &str, _: &Map<String, Value>) -> Option<Phrases> {
            (name == "echo_test").then(|| Phrases { ask: "Echo".into(), doing: "Echoing".into(), done: "Echoed".into() })
        }
        fn run(&self, name: &str, args: Map<String, Value>) -> Option<BoxFuture<'static, ToolResult>> {
            (name == "echo_test").then(|| async move { Ok(ToolOutput::Value(Value::Object(args))) }.boxed())
        }
        fn prompt(&self) -> Option<String> {
            Some("Echo tools exist.".into())
        }
    }

    #[tokio::test]
    async fn registered_tools_join_the_table_and_run() {
        register("echo", Arc::new(Echo));
        register("echo", Arc::new(Echo));
        assert_eq!(crate::tools::tool_specs().iter().filter(|s| s.name == "echo_test").count(), 1);
        assert_eq!(crate::tools::risk_of("echo_test"), Some(Risk::Read));
        assert_eq!(crate::tools::phrases("echo_test", &Map::new()).done, "Echoed");
        let mut a = Map::new();
        a.insert("x".into(), json!(1));
        assert_eq!(crate::tools::execute("echo_test", &a).await.unwrap(), ToolOutput::Value(json!({"x": 1})));
        assert!(crate::agent::system_prompt(true, None, None, None, "x").contains("Echo tools exist."));
    }
}
