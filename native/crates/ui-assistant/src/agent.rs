//! The assistant's agent loop (port of src/renderer/assistant/runtime.ts): one model step at a
//! time, app tools run between steps. Runs on the tokio runtime and reports to the UI through
//! an unbounded channel of [`AgentEvent`]s.
//!
//! A CLI agent that can load an MCP server runs the whole turn in one step instead: the app's
//! tools are served to it by [`crate::mcp`] and every call comes back here, to the same
//! [`Turn::call`] the native loop uses (same approvals, same step rows).
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use futures::channel::{mpsc::UnboundedSender, oneshot};
use futures::future::BoxFuture;
use futures::{FutureExt, StreamExt};
use mv_ai::{emit_fn, AiAccessMode, AiEffort, AiMessage, AiProviderId, AiRole, AiStepRequest, AiStepResult, AiToolCall, AiToolSpec, CancellationToken, Emit, Registry, StreamEvent};
use mv_ui::Route;
use serde_json::{json, Map, Value};

use crate::store::{new_id, StepState, ToolStep};
use crate::tools::{self, needs_approval, ToolOutput, ToolRunner, ToolScope};

/// Enough for a big edit (read the timeline, then a few dozen clip changes) to finish; the user
/// can stop it at any time.
pub const MAX_ROUNDS: usize = 40;
/// A tool result is cut to about this many characters (saying so, and how to get the rest).
pub const RESULT_MAX: usize = 8000;
/// Tool results of earlier turns are shortened in the history; the newest few stay whole.
const KEEP_FULL_RESULTS: usize = 4;
const SHORT_RESULT: usize = 300;
const SHORTENED: &str = "call the tool again for all of it]";

/// Runs one model step. Production uses the mv-ai [`Registry`]; tests use a fake.
pub trait StepRunner: Send + Sync + 'static {
    fn step(&self, req: AiStepRequest, emit: Emit, cancel: CancellationToken) -> BoxFuture<'static, AiStepResult>;
}

impl StepRunner for Arc<Registry> {
    fn step(&self, req: AiStepRequest, emit: Emit, cancel: CancellationToken) -> BoxFuture<'static, AiStepResult> {
        let r = self.clone();
        async move { r.run_step_cancellable(req, emit, cancel).await }.boxed()
    }
}

#[derive(Debug)]
pub enum Outcome {
    Done,
    Stopped,
    Error(String),
}

#[derive(Debug)]
pub enum AgentEvent {
    /// Streamed delta from the provider (text / reasoning / status / activity).
    Delta(StreamEvent),
    /// A new model round begins (0-based).
    Round(usize),
    StepAdded(ToolStep),
    StepUpdated { id: String, state: StepState, result: Option<String> },
    /// The UI must answer through `reply` (true = allow).
    Approval { step_id: String, reply: oneshot::Sender<bool> },
    Navigate(Route),
    /// Buttons to show under the reply (`suggest_replies`).
    Suggest(Vec<crate::suggest::Suggestion>),
    /// Always the last event; carries the model-side history to persist.
    Finished { outcome: Outcome, history: Vec<AiMessage> },
}

pub struct AgentParams {
    pub provider: AiProviderId,
    pub model: String,
    pub effort: AiEffort,
    /// Read before every tool call so the user can switch mode mid-run.
    pub access: Arc<Mutex<AiAccessMode>>,
    pub system: String,
    pub history: Vec<AiMessage>,
    pub tools_enabled: bool,
    /// The provider takes tool definitions. Without (the CLI agents), the tools reach it over MCP
    /// when it can load a server (`mcp`), else they are described in the prompt and called with
    /// ```mangavox blocks in the reply (see [`text_tool_prompt`]).
    pub native_tools: bool,
    /// The provider is a CLI agent that loads the app's MCP server for a run.
    pub mcp: bool,
    /// Where the user is: picks the tool groups sent with each step.
    pub page: Option<Route>,
    /// Images attached to the user's message; sent with the first round only.
    pub images: Vec<std::path::PathBuf>,
}

/// System prompt with live app context (port of `systemPrompt`).
pub fn system_prompt(tools_enabled: bool, page: Option<&str>, section: Option<&str>, library: Option<usize>, today: &str) -> String {
    let mut lines = vec![
        "You are the MangaVox assistant, built into the MangaVox desktop app: a manga reader and library with chapter downloads, anime / movie / series streaming, a discovery feed, and Studio, which turns downloaded manga chapters into narrated recap videos (Read → Transcript → Story → Audio → Music → Video), joins them into a full recap with a hook intro and watermark, and prepares a YouTube package.".to_string(),
        format!("Today is {today}."),
    ];
    if let Some(p) = page {
        lines.push(format!(
            "The user is on the \"{p}\" page in the {} section{}.",
            section.unwrap_or("manga"),
            library.map(|n| format!(", with {n} manga in their library")).unwrap_or_default()
        ));
    }
    if tools_enabled {
        lines.push("You can operate the app with tools. When the user asks you to do something in the app (open, find, play, switch, change a setting), do it with the tools rather than explaining how.".into());
        lines.push("You get the tools for the current page; when a request needs others (manga, anime / movies, Studio, the Video editor, YouTube), call load_tools first.".into());
        lines.push("Use search and list tools before naming specific titles, chapters or ids — never invent them.".into());
        lines.push("Only act on the manga and chapters the user named or clearly agreed to. Never widen a request to other chapters (\"all\" only when the user said all). If it is unclear which chapters they mean, list them and ask.".into());
        lines.push("Studio work goes through the same queue as the Studio dashboard: list chapters with studio_library, then studio_generate for exactly those chapters (use redo_from only when the user wants a step made again). Check progress with studio_status.".into());
        lines.push("Deletes, overwrites and big downloads ask the user for confirmation in the chat; if they decline, do not retry.".into());
        lines.push("After acting, confirm briefly what happened. When there is an obvious next step, end with suggest_replies so the user can click it.".into());
        lines.extend(crate::ext::prompt());
    } else {
        lines.push("In this mode you cannot control the app; answer conversationally and suggest where in the app to go when useful.".into());
    }
    lines.push("Be concise and friendly. Use markdown (short lists, tables, code) only when it helps.".into());
    lines.join("\n")
}

/// Added to the system prompt of a CLI agent that gets the tools over MCP.
pub const MCP_PROMPT: &str = "The app's tools are the tools of the \"mangavox\" MCP server (your client may show them as mcp__mangavox__<name>, mangavox__<name> or mangavox_<name>, or behind its own tool search). Operate the app only through them, never with shell, file or web tools. A tool that load_tools returns but your tool list does not show is called through run_tool.";

/// The fence a model without native tool calling uses to call an app tool.
pub const TOOL_FENCE: &str = "```mangavox";

/// The tools as instructions, for providers that take no tool definitions (the CLI agents).
pub fn text_tool_prompt(specs: &[AiToolSpec]) -> String {
    let mut out = String::from(
        "App actions: you control MangaVox only through the actions below (not with your own shell, file or web tools). To run actions, put one block per action in your reply, exactly like this, and nothing after the blocks:\n\n```mangavox\n{\"tool\": \"studio_status\", \"args\": {}}\n```\n\nThe app runs them (asking the user where needed) and sends the results back as \"Tool result\" messages; then continue. Never claim an action happened before its result came back. Reply without blocks when you are done.\n\nActions (name(arguments): what it does):",
    );
    for t in specs {
        out.push_str(&format!("\n- {}", tools::signature(t)));
    }
    out
}

/// The ```mangavox blocks of a reply as tool calls (`{"tool": name, "args": {...}}`, or a list
/// of those). Blocks that are not valid JSON are skipped.
pub fn parse_text_calls(text: &str) -> Vec<AiToolCall> {
    let mut calls = vec![];
    let mut rest = text;
    while let Some(start) = rest.find(TOOL_FENCE) {
        let body_start = start + TOOL_FENCE.len();
        let Some(len) = rest[body_start..].find("```") else { break };
        let body = rest[body_start..body_start + len].trim();
        rest = &rest[body_start + len + 3..];
        let items = match serde_json::from_str::<Value>(body) {
            Ok(Value::Array(a)) => a,
            Ok(v) => vec![v],
            Err(_) => continue,
        };
        for v in items {
            let name = v.get("tool").or_else(|| v.get("name")).and_then(Value::as_str).unwrap_or("").trim().to_string();
            if name.is_empty() {
                continue;
            }
            let args = v.get("args").or_else(|| v.get("arguments")).and_then(Value::as_object).cloned().unwrap_or_default();
            calls.push(AiToolCall { id: new_id("t"), name, args });
        }
    }
    calls
}

/// A reply without its ```mangavox blocks (also one still streaming), for display.
pub fn visible_text(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains(TOOL_FENCE) {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(TOOL_FENCE) {
        out.push_str(&rest[..start]);
        let after = start + TOOL_FENCE.len();
        match rest[after..].find("```") {
            Some(len) => rest = &rest[after + len + 3..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out.trim().to_string())
}

static STUDIO_CONTEXT: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Extra context (e.g. the Studio page/transcript) appended to the system prompt; `None` clears it.
pub fn set_studio_context(text: Option<String>) {
    *STUDIO_CONTEXT.lock().unwrap_or_else(|e| e.into_inner()) = text.filter(|t| !t.trim().is_empty());
}

pub fn studio_context() -> Option<String> {
    STUDIO_CONTEXT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// `YYYY-MM-DD` for today (UTC) without a date crate.
pub fn today_string() -> String {
    let days = (crate::store::now_ms() / 86_400_000) as i64;
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

fn tool_message(call: &AiToolCall, content: Value) -> AiMessage {
    let mut m = AiMessage::new(AiRole::Tool, content.to_string());
    m.tool_call_id = Some(call.id.clone());
    m.tool_name = Some(call.name.clone());
    m
}

/// Shortens the tool results of earlier turns, keeping the newest [`KEEP_FULL_RESULTS`] whole: a
/// model rarely needs an old list again, and can call the tool again when it does. Done once at
/// the start of a turn, so the history stays the same (and cacheable) through the turn's rounds.
pub fn shorten_old_results(history: &mut [AiMessage]) {
    let tools: Vec<usize> = history.iter().enumerate().filter(|(_, m)| m.role == AiRole::Tool).map(|(i, _)| i).collect();
    for &i in &tools[..tools.len().saturating_sub(KEEP_FULL_RESULTS)] {
        let m = &mut history[i];
        let n = m.content.chars().count();
        if n > SHORT_RESULT + 80 && !m.content.ends_with(SHORTENED) {
            let head: String = m.content.chars().take(SHORT_RESULT).collect();
            m.content = format!("{head}… [older result shortened from {n} characters; {SHORTENED}");
        }
    }
}

/// CLI agents whose MCP run never listed the tools this session: they use the text protocol.
static MCP_BROKEN: Mutex<Option<HashSet<AiProviderId>>> = Mutex::new(None);

fn mcp_broken(id: AiProviderId) -> bool {
    MCP_BROKEN.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|s| s.contains(&id))
}

fn mark_mcp_broken(id: AiProviderId) {
    MCP_BROKEN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashSet::new).insert(id);
}

/// Run the loop until the model stops calling tools, an error occurs, it is cancelled, or
/// [`MAX_ROUNDS`] is hit. Always ends by sending [`AgentEvent::Finished`].
pub async fn run_agent<R: StepRunner, T: ToolRunner>(runner: &R, tools: &T, p: AgentParams, tx: UnboundedSender<AgentEvent>, cancel: CancellationToken) {
    let mut history = p.history.clone();
    shorten_old_results(&mut history);
    let prompt = history.iter().rev().find(|m| m.role == AiRole::User).map(|m| m.content.clone()).unwrap_or_default();
    crate::ext::begin_turn(&prompt);
    let outcome = run_loop(runner, tools, &p, &mut history, &tx, &cancel).await;
    let keep = history.len().saturating_sub(80);
    let _ = tx.unbounded_send(AgentEvent::Finished { outcome, history: history.split_off(keep) });
}

/// What one tool call needs from the running turn.
struct Turn<'a, T> {
    tools: &'a T,
    p: &'a AgentParams,
    tx: &'a UnboundedSender<AgentEvent>,
    cancel: &'a CancellationToken,
}

impl<T: ToolRunner> Turn<'_, T> {
    /// Runs one call the same way for every provider: risk, approval (Ask mode asks; reads run
    /// directly), the tool, its step row, then the (trimmed) result for the model. `None` when the
    /// turn was cancelled meanwhile.
    async fn call(&self, scope: &mut ToolScope, call: &AiToolCall) -> Option<Value> {
        if call.name == tools::LOAD_TOOLS {
            return Some(scope.load(&call.args).unwrap_or_else(|e| json!({ "error": e })));
        }
        if call.name == tools::RUN_TOOL {
            let name = call.args.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
            let args = call.args.get("args").and_then(Value::as_object).cloned().unwrap_or_default();
            if name.is_empty() || name == tools::RUN_TOOL {
                return Some(json!({ "error": "Give the tool's name, and its args as an object" }));
            }
            return Box::pin(self.call(scope, &AiToolCall { id: call.id.clone(), name, args })).await;
        }
        let Some(risk) = tools::risk_for(&call.name, &call.args) else {
            return Some(json!({ "error": format!("Unknown tool {}", call.name) }));
        };
        scope.add_for(&call.name);
        let step_id = new_id("s");
        let mode = *self.p.access.lock().unwrap_or_else(|e| e.into_inner());
        let approval = needs_approval(mode, risk);
        let _ = self.tx.unbounded_send(AgentEvent::StepAdded(ToolStep {
            id: step_id.clone(),
            name: call.name.clone(),
            label: tools::label(&call.name, &call.args),
            args: Value::Object(call.args.clone()),
            risk: risk.as_str().into(),
            state: if approval { StepState::AwaitingApproval } else { StepState::Running },
            result: None,
        }));
        let update = |state, result| {
            let _ = self.tx.unbounded_send(AgentEvent::StepUpdated { id: step_id.clone(), state, result });
        };

        let allowed = if approval {
            let (reply, rx) = oneshot::channel();
            let _ = self.tx.unbounded_send(AgentEvent::Approval { step_id: step_id.clone(), reply });
            tokio::select! {
                ok = rx => ok.unwrap_or(false),
                _ = self.cancel.cancelled() => false,
            }
        } else {
            true
        };
        if self.cancel.is_cancelled() {
            update(StepState::Denied, None);
            return None;
        }
        if !allowed {
            update(StepState::Denied, None);
            return Some(json!({ "error": "The user declined this action. Do not retry it; ask what they want instead." }));
        }
        update(StepState::Running, None);
        Some(match self.tools.run(call.name.clone(), call.args.clone()).await {
            Ok(out) => {
                let value = match out {
                    ToolOutput::Value(v) => v,
                    ToolOutput::Navigate(route, v) => {
                        let _ = self.tx.unbounded_send(AgentEvent::Navigate(route));
                        v
                    }
                    ToolOutput::Suggest(list, v) => {
                        let _ = self.tx.unbounded_send(AgentEvent::Suggest(list));
                        v
                    }
                };
                let content = tools::trim_result(value, RESULT_MAX);
                update(StepState::Done, Some(content.to_string().chars().take(600).collect()));
                content
            }
            Err(msg) => {
                update(StepState::Error, Some(msg.clone()));
                json!({ "error": msg })
            }
        })
    }
}

async fn run_loop<R: StepRunner, T: ToolRunner>(
    runner: &R,
    tools: &T,
    p: &AgentParams,
    history: &mut Vec<AiMessage>,
    tx: &UnboundedSender<AgentEvent>,
    cancel: &CancellationToken,
) -> Outcome {
    let turn = Turn { tools, p, tx, cancel };
    if p.tools_enabled && p.mcp && !mcp_broken(p.provider) {
        if let Some(outcome) = run_mcp(runner, &turn, history).await {
            return outcome;
        }
    }
    let mut scope = ToolScope::new(p.page.as_ref(), history, false);
    let text_tools = p.tools_enabled && !p.native_tools;
    for round in 0..MAX_ROUNDS {
        if cancel.is_cancelled() {
            return Outcome::Stopped;
        }
        let _ = tx.unbounded_send(AgentEvent::Round(round));
        let delta_tx = tx.clone();
        let emit = emit_fn(move |ev| {
            let _ = delta_tx.unbounded_send(AgentEvent::Delta(ev));
        });
        // The scope grows when the model loads groups, so the list is made per round.
        let specs = scope.specs();
        let system = if text_tools { format!("{}\n\n{}", p.system, text_tool_prompt(&specs)) } else { p.system.clone() };
        let mut messages = vec![AiMessage::system(system)];
        messages.extend(history.iter().cloned());
        let req = AiStepRequest {
            request_id: new_id("step"),
            provider_id: p.provider,
            model: p.model.clone(),
            messages,
            tools: (p.tools_enabled && p.native_tools).then_some(specs),
            effort: Some(p.effort),
            access: Some(*p.access.lock().unwrap_or_else(|e| e.into_inner())),
            images: if round == 0 { p.images.clone() } else { vec![] },
            workspace: None,
            lean: false,
            mcp: None,
        };
        let result = runner.step(req, emit, cancel.clone()).await;
        if result.stopped || cancel.is_cancelled() {
            return Outcome::Stopped;
        }
        if let Some(e) = result.error {
            return Outcome::Error(e);
        }
        // The reply's text keeps its ```mangavox blocks: a CLI agent reads its own calls back
        // from the transcript next round.
        let calls = if text_tools { parse_text_calls(&result.text) } else { result.tool_calls.clone() };
        let mut assistant = AiMessage::assistant(result.text.clone());
        if !calls.is_empty() && !text_tools {
            assistant.tool_calls = Some(calls.clone());
        }
        assistant.raw = result.raw.clone();
        history.push(assistant);
        if calls.is_empty() {
            return Outcome::Done;
        }

        for call in &calls {
            match turn.call(&mut scope, call).await {
                Some(content) => history.push(tool_message(call, content)),
                None => return Outcome::Stopped,
            }
        }
        // Buttons were the last thing the reply needed: another round would only repeat its text.
        if calls.iter().all(|c| c.name == "suggest_replies") && history.last().is_some_and(|m| !m.content.contains("\"error\"")) {
            return Outcome::Done;
        }
    }
    // Hit the round cap while still calling tools.
    let _ = tx.unbounded_send(AgentEvent::Delta(StreamEvent::Text { delta: format!("\n\n_Stopped after {MAX_ROUNDS} rounds of actions._") }));
    Outcome::Done
}

/// The whole turn as one CLI run with the app's tools on an MCP server: the CLI's own loop calls
/// them, each call is run here by [`Turn::call`] while the step streams. `None` when the server
/// could not start (the caller falls back to the text protocol).
async fn run_mcp<R: StepRunner, T: ToolRunner>(runner: &R, turn: &Turn<'_, T>, history: &mut Vec<AiMessage>) -> Option<Outcome> {
    let (p, tx, cancel) = (turn.p, turn.tx, turn.cancel);
    let mut scope = ToolScope::new(p.page.as_ref(), history, true);
    let (server, mut calls) = match crate::mcp::McpServer::start(scope.specs()).await {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.unbounded_send(AgentEvent::Delta(StreamEvent::Status { text: format!("Could not serve the app's tools ({e}); using the text protocol") }));
            return None;
        }
    };
    // Grok and Antigravity reach the server through the app's stdio bridge, which finds it in a
    // file for as long as this turn runs. If that fails the CLI lists no tools and the check
    // below falls back to the text protocol.
    let _published = match mv_ai::providers::cli_mcp::bridged(p.provider).then(|| server.publish()) {
        Some(Err(e)) => {
            let _ = tx.unbounded_send(AgentEvent::Delta(StreamEvent::Status { text: format!("Could not publish the app's tools for this agent ({e})") }));
            None
        }
        other => other.and_then(Result::ok),
    };
    let _ = tx.unbounded_send(AgentEvent::Round(0));
    let delta_tx = tx.clone();
    let emit = emit_fn(move |ev| {
        let _ = delta_tx.unbounded_send(AgentEvent::Delta(ev));
    });
    let mut messages = vec![AiMessage::system(format!("{}\n\n{MCP_PROMPT}", p.system))];
    messages.extend(history.iter().cloned());
    let req = AiStepRequest {
        request_id: new_id("step"),
        provider_id: p.provider,
        model: p.model.clone(),
        messages,
        tools: None,
        effort: Some(p.effort),
        access: Some(*p.access.lock().unwrap_or_else(|e| e.into_inner())),
        images: p.images.clone(),
        workspace: None,
        lean: false,
        mcp: Some(server.endpoint()),
    };
    let step = runner.step(req, emit, cancel.clone());
    futures::pin_mut!(step);
    let mut done: Vec<(AiToolCall, Value)> = vec![];
    let result = loop {
        tokio::select! {
            r = &mut step => break r,
            Some(c) = calls.next() => {
                let call = AiToolCall { id: new_id("t"), name: c.name, args: c.args };
                // A cancelled turn keeps polling the step, which then stops the CLI's process tree.
                let content = turn.call(&mut scope, &call).await.unwrap_or_else(|| json!({ "error": "The user stopped this reply." }));
                let _ = c.reply.send(content.clone());
                server.set_tools(scope.specs());
                if call.name != tools::LOAD_TOOLS {
                    let call = if call.name == tools::RUN_TOOL { unwrap_run_tool(call) } else { call };
                    done.push((call, content));
                }
            }
        }
    };
    if result.stopped || cancel.is_cancelled() {
        return Some(Outcome::Stopped);
    }
    if let Some(e) = result.error {
        return Some(Outcome::Error(e));
    }
    if !server.connected() {
        // The CLI answered without ever listing the tools: its MCP set-up did not take. Later
        // replies use the text protocol.
        mark_mcp_broken(p.provider);
        let _ = tx.unbounded_send(AgentEvent::Delta(StreamEvent::Status { text: "This agent did not load the app's tools; the next replies describe them in the prompt instead".into() }));
    }
    // The calls and their results join the history as one tool round before the reply, so the
    // next turn (or another provider) sees what was done.
    if !done.is_empty() {
        let mut round = AiMessage::assistant("");
        round.tool_calls = Some(done.iter().map(|(c, _)| c.clone()).collect());
        history.push(round);
        history.extend(done.into_iter().map(|(c, v)| tool_message(&c, v)));
    }
    history.push(AiMessage::assistant(result.text));
    Some(Outcome::Done)
}

/// `run_tool {name, args}` as the call it stands for, for the history.
fn unwrap_run_tool(call: AiToolCall) -> AiToolCall {
    let name = call.args.get("name").and_then(Value::as_str).unwrap_or(tools::RUN_TOOL).to_string();
    let args: Map<String, Value> = call.args.get("args").and_then(Value::as_object).cloned().unwrap_or_default();
    AiToolCall { id: call.id, name, args }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::channel::mpsc::unbounded;
    use futures::StreamExt;
    use serde_json::Map;
    use std::collections::VecDeque;

    struct Fake(Mutex<VecDeque<AiStepResult>>, Mutex<Vec<AiStepRequest>>);
    impl Fake {
        fn new(r: Vec<AiStepResult>) -> Self {
            Fake(Mutex::new(r.into()), Mutex::new(vec![]))
        }
    }
    impl StepRunner for Fake {
        fn step(&self, req: AiStepRequest, emit: Emit, _c: CancellationToken) -> BoxFuture<'static, AiStepResult> {
            self.1.lock().unwrap().push(req);
            let r = self.0.lock().unwrap().pop_front().unwrap_or_default();
            async move {
                emit(StreamEvent::Text { delta: r.text.clone() });
                r
            }
            .boxed()
        }
    }

    struct FakeTools;
    impl ToolRunner for FakeTools {
        fn run(&self, name: String, _a: Map<String, Value>) -> BoxFuture<'static, tools::ToolResult> {
            async move {
                match name.as_str() {
                    "navigate" => Ok(ToolOutput::Navigate(Route::Library, json!({ "ok": true }))),
                    "search_anime" => Ok(ToolOutput::Value(json!([{ "title": "X" }]))),
                    _ => Err("boom".into()),
                }
            }
            .boxed()
        }
    }

    fn call(name: &str) -> AiToolCall {
        AiToolCall { id: new_id("c"), name: name.into(), args: Map::new() }
    }

    fn params(mode: AiAccessMode, tools_enabled: bool) -> AgentParams {
        AgentParams {
            provider: AiProviderId::Ollama,
            model: "m".into(),
            effort: AiEffort::Medium,
            access: Arc::new(Mutex::new(mode)),
            system: "sys".into(),
            history: vec![AiMessage::user("hi")],
            tools_enabled,
            native_tools: true,
            mcp: false,
            page: None,
            images: vec![],
        }
    }

    async fn collect(rx: futures::channel::mpsc::UnboundedReceiver<AgentEvent>, approve: Option<bool>) -> Vec<AgentEvent> {
        let mut rx = rx;
        let mut out = vec![];
        while let Some(ev) = rx.next().await {
            let fin = matches!(ev, AgentEvent::Finished { .. });
            match ev {
                AgentEvent::Approval { reply, .. } => {
                    let _ = reply.send(approve.unwrap_or(true));
                }
                other => out.push(other),
            }
            if fin {
                break;
            }
        }
        out
    }

    #[tokio::test]
    async fn plain_answer_finishes() {
        let fake = Fake::new(vec![AiStepResult { text: "hello".into(), ..Default::default() }]);
        let (tx, rx) = unbounded();
        run_agent(&fake, &FakeTools, params(AiAccessMode::Ask, true), tx, CancellationToken::new()).await;
        let evs = collect(rx, None).await;
        match evs.last().unwrap() {
            AgentEvent::Finished { outcome: Outcome::Done, history } => assert_eq!(history.last().unwrap().content, "hello"),
            other => panic!("{other:?}"),
        }
        let req = &fake.1.lock().unwrap()[0];
        assert_eq!(req.messages[0].role, AiRole::System);
        // The core tools and the manga page's, plus the way to load the rest.
        let names: Vec<&str> = req.tools.as_ref().unwrap().iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"navigate") && names.contains(&"list_library") && names.contains(&tools::LOAD_TOOLS), "{names:?}");
        assert!(!names.contains(&"search_anime") && !names.contains(&tools::RUN_TOOL));
    }

    #[tokio::test]
    async fn cli_providers_get_no_tools() {
        let fake = Fake::new(vec![AiStepResult::default()]);
        let (tx, _rx) = unbounded();
        run_agent(&fake, &FakeTools, params(AiAccessMode::Ask, false), tx, CancellationToken::new()).await;
        assert!(fake.1.lock().unwrap()[0].tools.is_none());
    }

    #[tokio::test]
    async fn cli_providers_call_tools_with_text_blocks() {
        let reply = "Opening it.
```mangavox
{\"tool\": \"navigate\", \"args\": {\"page\": \"library\"}}
```";
        let fake = Arc::new(Fake::new(vec![AiStepResult { text: reply.into(), ..Default::default() }, AiStepResult { text: "Done.".into(), ..Default::default() }]));
        let (tx, rx) = unbounded();
        let mut p = params(AiAccessMode::Full, true);
        p.native_tools = false;
        let f2 = fake.clone();
        let task = tokio::spawn(async move { run_agent(&*f2, &FakeTools, p, tx, CancellationToken::new()).await });
        let evs = collect(rx, Some(true)).await;
        task.await.unwrap();
        assert!(evs.iter().any(|e| matches!(e, AgentEvent::Navigate(Route::Library))));
        let reqs = fake.1.lock().unwrap();
        assert!(reqs[0].tools.is_none());
        assert!(reqs[0].messages[0].content.contains(TOOL_FENCE) && reqs[0].messages[0].content.contains("- navigate("));
        assert!(reqs[0].messages[0].content.contains("suggestions: [{label, prompt}]"));
        // The second round sees the result as a tool message.
        assert!(reqs[1].messages.iter().any(|m| m.role == AiRole::Tool && m.tool_name.as_deref() == Some("navigate")));
    }

    struct SuggestTools;
    impl ToolRunner for SuggestTools {
        fn run(&self, _n: String, _a: Map<String, Value>) -> BoxFuture<'static, tools::ToolResult> {
            async move { Ok(ToolOutput::Suggest(vec![crate::suggest::Suggestion::new("r", "Go", "", "Go")], json!({ "ok": true }))) }.boxed()
        }
    }

    #[tokio::test]
    async fn a_round_of_only_suggestions_ends_the_reply() {
        let fake = Fake::new(vec![AiStepResult { text: "Done.".into(), tool_calls: vec![call("suggest_replies")], ..Default::default() }, AiStepResult { text: "Done again.".into(), ..Default::default() }]);
        let (tx, rx) = unbounded();
        run_agent(&fake, &SuggestTools, params(AiAccessMode::Ask, true), tx, CancellationToken::new()).await;
        let evs = collect(rx, None).await;
        assert_eq!(fake.1.lock().unwrap().len(), 1, "no extra round after the buttons");
        assert!(evs.iter().any(|e| matches!(e, AgentEvent::Suggest(l) if l[0].title == "Go")));
    }

    #[test]
    fn text_blocks_parse_and_hide() {
        let t = "Sure.
```mangavox
{\"tool\": \"studio_status\"}
```
```mangavox
[{\"name\": \"a\", \"args\": {\"x\": 1}}, {\"tool\": \"\"}]
```
```mangavox
not json
```";
        let calls = parse_text_calls(t);
        assert_eq!(calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["studio_status", "a"]);
        assert_eq!(calls[1].args["x"], 1);
        assert_eq!(visible_text(t), "Sure.");
        assert_eq!(visible_text("Working…
```mangavox
{\"tool\": \"stu"), "Working…");
        assert_eq!(visible_text("plain"), "plain");
    }

    #[tokio::test]
    async fn approved_action_navigates_and_feeds_result_back() {
        let fake = Arc::new(Fake::new(vec![
            AiStepResult { tool_calls: vec![call("search_anime"), call("navigate")], ..Default::default() },
            AiStepResult { text: "done".into(), ..Default::default() },
        ]));
        let (tx, rx) = unbounded();
        let f2 = fake.clone();
        let task = tokio::spawn(async move { run_agent(&*f2, &FakeTools, params(AiAccessMode::Ask, true), tx, CancellationToken::new()).await });
        let evs = collect(rx, Some(true)).await;
        task.await.unwrap();
        assert!(evs.iter().any(|e| matches!(e, AgentEvent::Navigate(Route::Library))));
        match evs.last().unwrap() {
            AgentEvent::Finished { outcome: Outcome::Done, history } => {
                let tool_msgs: Vec<_> = history.iter().filter(|m| m.role == AiRole::Tool).collect();
                assert_eq!(tool_msgs.len(), 2);
                assert_eq!(history.last().unwrap().content, "done");
            }
            other => panic!("{other:?}"),
        }
        // second request carries the tool results
        assert_eq!(fake.1.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn denied_action_is_reported_to_model() {
        let fake = Arc::new(Fake::new(vec![
            AiStepResult { tool_calls: vec![call("navigate")], ..Default::default() },
            AiStepResult { text: "ok".into(), ..Default::default() },
        ]));
        let (tx, rx) = unbounded();
        let f2 = fake.clone();
        let task = tokio::spawn(async move { run_agent(&*f2, &FakeTools, params(AiAccessMode::Ask, true), tx, CancellationToken::new()).await });
        let evs = collect(rx, Some(false)).await;
        task.await.unwrap();
        assert!(!evs.iter().any(|e| matches!(e, AgentEvent::Navigate(_))));
        assert!(evs.iter().any(|e| matches!(e, AgentEvent::StepUpdated { state: StepState::Denied, .. })));
    }

    #[tokio::test]
    async fn full_mode_never_asks_and_caps_rounds() {
        let rounds: Vec<_> = (0..MAX_ROUNDS + 5).map(|_| AiStepResult { tool_calls: vec![call("navigate")], ..Default::default() }).collect();
        let fake = Fake::new(rounds);
        let (tx, rx) = unbounded();
        run_agent(&fake, &FakeTools, params(AiAccessMode::Full, true), tx, CancellationToken::new()).await;
        let evs = collect(rx, None).await;
        assert!(!evs.iter().any(|e| matches!(e, AgentEvent::StepAdded(s) if s.state == StepState::AwaitingApproval)));
        assert_eq!(fake.1.lock().unwrap().len(), MAX_ROUNDS);
        assert!(evs.iter().any(|e| matches!(e, AgentEvent::Delta(StreamEvent::Text { delta }) if delta.contains("Stopped after"))));
    }

    #[tokio::test]
    async fn errors_and_cancel() {
        let fake = Fake::new(vec![AiStepResult { error: Some("no key".into()), ..Default::default() }]);
        let (tx, rx) = unbounded();
        run_agent(&fake, &FakeTools, params(AiAccessMode::Ask, true), tx, CancellationToken::new()).await;
        let evs = collect(rx, None).await;
        assert!(matches!(evs.last().unwrap(), AgentEvent::Finished { outcome: Outcome::Error(e), .. } if e == "no key"));

        let fake = Fake::new(vec![]);
        let (tx, rx) = unbounded();
        let token = CancellationToken::new();
        token.cancel();
        run_agent(&fake, &FakeTools, params(AiAccessMode::Ask, true), tx, token).await;
        let evs = collect(rx, None).await;
        assert!(matches!(evs.last().unwrap(), AgentEvent::Finished { outcome: Outcome::Stopped, .. }));
    }

    #[tokio::test]
    async fn load_tools_adds_a_group_from_the_next_round() {
        let mut load = call(tools::LOAD_TOOLS);
        load.args.insert("groups".into(), json!(["media"]));
        let fake = Fake::new(vec![AiStepResult { tool_calls: vec![load], ..Default::default() }, AiStepResult { text: "ok".into(), ..Default::default() }]);
        let (tx, rx) = unbounded();
        run_agent(&fake, &FakeTools, params(AiAccessMode::Ask, true), tx, CancellationToken::new()).await;
        let evs = collect(rx, None).await;
        // Loading tools is not an app action: no step row, no approval.
        assert!(!evs.iter().any(|e| matches!(e, AgentEvent::StepAdded(_))));
        let reqs = fake.1.lock().unwrap();
        let names = |i: usize| reqs[i].tools.as_ref().unwrap().iter().map(|t| t.name.clone()).collect::<Vec<_>>();
        assert!(!names(0).contains(&"search_anime".to_string()));
        assert!(names(1).contains(&"search_anime".to_string()));
        let result = reqs[1].messages.iter().find(|m| m.role == AiRole::Tool).unwrap();
        assert!(result.content.contains("search_anime") && result.content.contains("\"loaded\":[\"media\"]"));
    }

    #[test]
    fn older_tool_results_are_shortened_newest_kept() {
        let mut h = vec![AiMessage::user("q")];
        for i in 0..6 {
            let mut m = AiMessage::new(AiRole::Tool, format!("{i}{}", "x".repeat(1000)));
            m.tool_name = Some("list_library".into());
            h.push(m);
        }
        h.push(AiMessage::assistant("y".repeat(1000)));
        shorten_old_results(&mut h);
        let lens: Vec<usize> = h[1..7].iter().map(|m| m.content.chars().count()).collect();
        assert!(lens[0] < 500 && lens[1] < 500, "{lens:?}");
        assert!(lens[2..].iter().all(|&n| n == 1001), "{lens:?}");
        assert!(h[1].content.starts_with('0') && h[1].content.contains("shortened from 1001"));
        assert_eq!(h[7].content.len(), 1000, "only tool results are shortened");
        // Done again, nothing changes.
        let again = h.clone();
        shorten_old_results(&mut h);
        assert_eq!(h, again);
    }

    // ── MCP turns ──────────────────────────────────────────────────────────────────

    /// A JSON-RPC request to the turn's MCP server, as a CLI would send it.
    async fn rpc(ep: &mv_ai::McpEndpoint, body: Value) -> Value {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let addr = ep.url.trim_start_matches("http://").split('/').next().unwrap().to_string();
        let mut s = tokio::net::TcpStream::connect(&addr).await.unwrap();
        let body = body.to_string();
        let req = format!("POST /mcp HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {}\r\nAccept: application/json, text/event-stream\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", ep.token, body.len());
        s.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = s.read(&mut chunk).await.unwrap();
            buf.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buf).into_owned();
            if let Some((head, rest)) = text.split_once("\r\n\r\n") {
                let len: usize = head.lines().find_map(|l| l.strip_prefix("Content-Length: ")).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
                if rest.len() >= len {
                    return if len == 0 { Value::Null } else { serde_json::from_str(rest).unwrap() };
                }
            }
            if n == 0 {
                panic!("connection closed: {text}");
            }
        }
    }

    /// A CLI agent with its own loop: lists the tools, calls the given ones over MCP, then answers.
    struct McpCli {
        calls: Vec<(&'static str, Value)>,
        list: bool,
        reqs: Mutex<Vec<AiStepRequest>>,
        results: Arc<Mutex<Vec<Value>>>,
    }
    impl StepRunner for McpCli {
        fn step(&self, req: AiStepRequest, _emit: Emit, _c: CancellationToken) -> BoxFuture<'static, AiStepResult> {
            let ep = req.mcp.clone();
            self.reqs.lock().unwrap().push(req);
            let (calls, list, results) = (self.calls.clone(), self.list, self.results.clone());
            async move {
                let Some(ep) = ep else { return AiStepResult { text: "text mode".into(), ..Default::default() } };
                if list {
                    rpc(&ep, json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": "2025-06-18" } })).await;
                    let listed = rpc(&ep, json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })).await;
                    results.lock().unwrap().push(listed);
                }
                for (i, (name, args)) in calls.into_iter().enumerate() {
                    let r = rpc(&ep, json!({ "jsonrpc": "2.0", "id": i + 2, "method": "tools/call", "params": { "name": name, "arguments": args } })).await;
                    results.lock().unwrap().push(r);
                }
                AiStepResult { text: "All done.".into(), ..Default::default() }
            }
            .boxed()
        }
    }

    fn mcp_params(provider: AiProviderId, mode: AiAccessMode) -> AgentParams {
        AgentParams { provider, native_tools: false, mcp: true, ..params(mode, true) }
    }

    #[tokio::test]
    async fn a_cli_turn_runs_once_and_calls_tools_over_mcp_with_the_same_approvals() {
        let cli = Arc::new(McpCli {
            calls: vec![("search_anime", json!({ "query": "x" })), ("navigate", json!({ "page": "library" })), (tools::RUN_TOOL, json!({ "name": "search_anime", "args": { "query": "y" } }))],
            list: true,
            reqs: Mutex::new(vec![]),
            results: Arc::new(Mutex::new(vec![])),
        });
        let (tx, rx) = unbounded();
        let c2 = cli.clone();
        let task = tokio::spawn(async move { run_agent(&*c2, &FakeTools, mcp_params(AiProviderId::Codex, AiAccessMode::Ask), tx, CancellationToken::new()).await });
        let evs = collect(rx, Some(true)).await;
        task.await.unwrap();
        // One process for the whole turn, with the endpoint and no tools in the request.
        let reqs = cli.reqs.lock().unwrap();
        assert_eq!(reqs.len(), 1);
        assert!(reqs[0].tools.is_none() && reqs[0].mcp.is_some());
        assert!(reqs[0].messages[0].content.contains(MCP_PROMPT) && !reqs[0].messages[0].content.contains(TOOL_FENCE));
        let results = cli.results.lock().unwrap();
        let listed: Vec<&str> = results[0]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(listed.contains(&"navigate") && listed.contains(&tools::RUN_TOOL) && listed.contains(&tools::LOAD_TOOLS));
        assert_eq!(results[1]["result"]["isError"], false);
        assert!(results[1]["result"]["content"][0]["text"].as_str().unwrap().contains("\"title\":\"X\""));
        // The read ran directly; the action asked first (Ask mode), then navigated.
        let added: Vec<&ToolStep> = evs.iter().filter_map(|e| if let AgentEvent::StepAdded(s) = e { Some(s) } else { None }).collect();
        assert_eq!(added.iter().map(|s| (s.name.as_str(), s.state)).collect::<Vec<_>>(), [("search_anime", StepState::Running), ("navigate", StepState::AwaitingApproval), ("search_anime", StepState::Running)]);
        assert!(evs.iter().any(|e| matches!(e, AgentEvent::Navigate(Route::Library))));
        match evs.last().unwrap() {
            AgentEvent::Finished { outcome: Outcome::Done, history } => {
                // user, the tool round, three results, the reply
                assert_eq!(history.len(), 6, "{history:?}");
                let names: Vec<&str> = history[1].tool_calls.as_ref().unwrap().iter().map(|c| c.name.as_str()).collect();
                assert_eq!(names, ["search_anime", "navigate", "search_anime"], "run_tool is kept as the call it stands for");
                assert_eq!(history[2].tool_call_id, history[1].tool_calls.as_ref().unwrap().first().map(|c| c.id.clone()));
                assert_eq!(history.last().unwrap().content, "All done.");
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn a_declined_mcp_call_comes_back_as_an_error() {
        let cli = Arc::new(McpCli { calls: vec![("navigate", json!({ "page": "library" }))], list: true, reqs: Mutex::new(vec![]), results: Arc::new(Mutex::new(vec![])) });
        let (tx, rx) = unbounded();
        let c2 = cli.clone();
        let task = tokio::spawn(async move { run_agent(&*c2, &FakeTools, mcp_params(AiProviderId::ClaudeCode, AiAccessMode::Ask), tx, CancellationToken::new()).await });
        let evs = collect(rx, Some(false)).await;
        task.await.unwrap();
        assert!(!evs.iter().any(|e| matches!(e, AgentEvent::Navigate(_))));
        let r = &cli.results.lock().unwrap()[1];
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("declined"));
    }

    #[tokio::test]
    async fn a_cli_that_never_lists_the_tools_falls_back_to_the_text_protocol() {
        let cli = Arc::new(McpCli { calls: vec![], list: false, reqs: Mutex::new(vec![]), results: Arc::new(Mutex::new(vec![])) });
        for _ in 0..2 {
            let (tx, rx) = unbounded();
            run_agent(&*cli, &FakeTools, mcp_params(AiProviderId::GeminiCli, AiAccessMode::Ask), tx, CancellationToken::new()).await;
            collect(rx, None).await;
        }
        let reqs = cli.reqs.lock().unwrap();
        assert!(reqs[0].mcp.is_some());
        assert!(reqs[1].mcp.is_none() && reqs[1].messages[0].content.contains(TOOL_FENCE));
    }

    /// Live: `MV_LIVE_CLI` = a signed-in CLI provider id (claude-code, codex, gemini-cli, opencode);
    /// `MV_LIVE_MODEL` optional. Runs one turn over MCP and the same turn on the text protocol,
    /// and prints the tokens each used.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn live_cli_turn_over_mcp_vs_text() {
        let id: AiProviderId = serde_json::from_value(json!(std::env::var("MV_LIVE_CLI").expect("MV_LIVE_CLI"))).unwrap();
        let model = std::env::var("MV_LIVE_MODEL").unwrap_or_else(|_| "default".into());
        let dir = std::env::temp_dir().join("mv-ai-live-mcp");
        let registry = Arc::new(Registry::new(dir.join("config.json"), dir.join("ws")));
        let mut used = vec![];
        for mcp in [true, false] {
            let before = mv_ai::usage::session_totals(id);
            let system = system_prompt(true, Some("library"), Some("manga"), Some(3), "2026-10-31");
            let p = AgentParams {
                provider: id,
                model: model.clone(),
                system,
                history: vec![AiMessage::user("Open the anime page with the app's navigate tool, then reply with exactly: done")],
                native_tools: false,
                mcp,
                ..params(AiAccessMode::Full, true)
            };
            let (tx, rx) = unbounded();
            let r2 = registry.clone();
            let task = tokio::spawn(async move { run_agent(&r2, &FakeTools, p, tx, CancellationToken::new()).await });
            let evs = collect(rx, Some(true)).await;
            task.await.unwrap();
            let navigated = evs.iter().any(|e| matches!(e, AgentEvent::Navigate(_)));
            let after = mv_ai::usage::session_totals(id);
            let text: String = evs.iter().filter_map(|e| if let AgentEvent::Delta(StreamEvent::Text { delta }) = e { Some(delta.as_str()) } else { None }).collect();
            eprintln!("{id:?} mcp={mcp}: navigated={navigated} steps={} in={} out={} reply={:?} last={:?}", after.2 - before.2, after.0 - before.0, after.1 - before.1, text.trim(), evs.last());
            used.push((navigated, after.0 - before.0));
        }
        assert!(used[0].0, "the MCP turn called the tool");
    }

    #[test]
    fn prompt_and_date() {
        let p = system_prompt(true, Some("library"), Some("manga"), Some(3), "2026-10-18");
        assert!(p.contains("\"library\" page") && p.contains("3 manga") && p.contains("operate the app"));
        assert!(system_prompt(false, None, None, None, "x").contains("cannot control the app"));
        let d = today_string();
        assert_eq!(d.len(), 10);
        assert!(d.starts_with("20"));
    }
}
