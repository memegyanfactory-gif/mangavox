//! The assistant's agent loop (port of src/renderer/assistant/runtime.ts): one model step at a
//! time, app tools run between steps. Runs on the tokio runtime and reports to the UI through
//! an unbounded channel of [`AgentEvent`]s.
use std::sync::{Arc, Mutex};

use futures::channel::{mpsc::UnboundedSender, oneshot};
use futures::future::BoxFuture;
use futures::FutureExt;
use mv_ai::{emit_fn, AiAccessMode, AiEffort, AiMessage, AiProviderId, AiRole, AiStepRequest, AiStepResult, AiToolCall, AiToolSpec, CancellationToken, Emit, Registry, StreamEvent};
use mv_ui::Route;
use serde_json::{json, Value};

use crate::store::{new_id, StepState, ToolStep};
use crate::tools::{self, needs_approval, ToolOutput, ToolRunner};

/// Enough for a chain like search → add → download → generate → export with a check between.
pub const MAX_ROUNDS: usize = 16;

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
    /// The provider takes tool definitions. Without (the CLI agents), the tools are described in
    /// the prompt and called with ```mangavox blocks in the reply (see [`text_tool_prompt`]).
    pub native_tools: bool,
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
        lines.push("Use search and list tools before naming specific titles, chapters or ids — never invent them.".into());
        lines.push("Only act on the manga and chapters the user named or clearly agreed to. Never widen a request to other chapters (\"all\" only when the user said all). If it is unclear which chapters they mean, list them and ask.".into());
        lines.push("Studio work goes through the same queue as the Studio dashboard: list chapters with studio_library, then studio_generate for exactly those chapters (use redo_from only when the user wants a step made again). Check progress with studio_status.".into());
        lines.push("Deletes, overwrites and big downloads ask the user for confirmation in the chat; if they decline, do not retry.".into());
        lines.push("After acting, confirm briefly what happened. When there is an obvious next step, end with suggest_replies so the user can click it.".into());
    } else {
        lines.push("In this mode you cannot control the app; answer conversationally and suggest where in the app to go when useful.".into());
    }
    lines.push("Be concise and friendly. Use markdown (short lists, tables, code) only when it helps.".into());
    lines.join("\n")
}

/// The fence a model without native tool calling uses to call an app tool.
pub const TOOL_FENCE: &str = "```mangavox";

/// The tools as instructions, for providers that take no tool definitions (the CLI agents).
pub fn text_tool_prompt(specs: &[AiToolSpec]) -> String {
    let mut out = String::from(
        "App actions: you control MangaVox only through the actions below (not with your own shell, file or web tools). To run actions, put one block per action in your reply, exactly like this, and nothing after the blocks:\n\n```mangavox\n{\"tool\": \"studio_status\", \"args\": {}}\n```\n\nThe app runs them (asking the user where needed) and sends the results back as \"Tool result\" messages; then continue. Never claim an action happened before its result came back. Reply without blocks when you are done.\n\nActions (name(arguments): what it does):",
    );
    for t in specs {
        let props = t.parameters.get("properties").and_then(Value::as_object);
        let required: Vec<&str> = t.parameters.get("required").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let args: Vec<String> = props
            .map(|p| {
                p.iter()
                    .map(|(k, v)| {
                        let ty = v.get("enum").and_then(Value::as_array).map(|e| e.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("|")).unwrap_or_else(|| match v.get("items").and_then(|i| i.get("properties")).and_then(Value::as_object) {
                            // Lists of objects show their keys: suggestions: [{label, prompt}].
                            Some(item) => format!("[{{{}}}]", item.keys().cloned().collect::<Vec<_>>().join(", ")),
                            None => v.get("type").and_then(Value::as_str).unwrap_or("any").to_string(),
                        });
                        format!("{k}{}: {ty}", if required.contains(&k.as_str()) { "" } else { "?" })
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push_str(&format!("\n- {}({}): {}", t.name, args.join(", "), t.description));
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

/// Run the loop until the model stops calling tools, an error occurs, it is cancelled, or
/// [`MAX_ROUNDS`] is hit. Always ends by sending [`AgentEvent::Finished`].
pub async fn run_agent<R: StepRunner, T: ToolRunner>(runner: &R, tools: &T, p: AgentParams, tx: UnboundedSender<AgentEvent>, cancel: CancellationToken) {
    let mut history = p.history.clone();
    let outcome = run_loop(runner, tools, &p, &mut history, &tx, &cancel).await;
    let keep = history.len().saturating_sub(80);
    let _ = tx.unbounded_send(AgentEvent::Finished { outcome, history: history.split_off(keep) });
}

async fn run_loop<R: StepRunner, T: ToolRunner>(
    runner: &R,
    tools: &T,
    p: &AgentParams,
    history: &mut Vec<AiMessage>,
    tx: &UnboundedSender<AgentEvent>,
    cancel: &CancellationToken,
) -> Outcome {
    let specs = tools::tool_specs();
    let text_tools = p.tools_enabled && !p.native_tools;
    let system = if text_tools { format!("{}\n\n{}", p.system, text_tool_prompt(&specs)) } else { p.system.clone() };
    for round in 0..MAX_ROUNDS {
        if cancel.is_cancelled() {
            return Outcome::Stopped;
        }
        let _ = tx.unbounded_send(AgentEvent::Round(round));
        let delta_tx = tx.clone();
        let emit = emit_fn(move |ev| {
            let _ = delta_tx.unbounded_send(AgentEvent::Delta(ev));
        });
        let mut messages = vec![AiMessage::system(system.clone())];
        messages.extend(history.iter().cloned());
        let req = AiStepRequest {
            request_id: new_id("step"),
            provider_id: p.provider,
            model: p.model.clone(),
            messages,
            tools: (p.tools_enabled && p.native_tools).then(|| specs.clone()),
            effort: Some(p.effort),
            access: Some(*p.access.lock().unwrap_or_else(|e| e.into_inner())),
            images: if round == 0 { p.images.clone() } else { vec![] },
            workspace: None,
            lean: false,
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
            let Some(risk) = tools::risk_for(&call.name, &call.args) else {
                history.push(tool_message(call, json!({ "error": format!("Unknown tool {}", call.name) })));
                continue;
            };
            let step_id = new_id("s");
            let mode = *p.access.lock().unwrap_or_else(|e| e.into_inner());
            let approval = needs_approval(mode, risk);
            let args_value = Value::Object(call.args.clone());
            let _ = tx.unbounded_send(AgentEvent::StepAdded(ToolStep {
                id: step_id.clone(),
                name: call.name.clone(),
                label: tools::label(&call.name, &call.args),
                args: args_value,
                risk: risk.as_str().into(),
                state: if approval { StepState::AwaitingApproval } else { StepState::Running },
                result: None,
            }));
            let update = |state, result| {
                let _ = tx.unbounded_send(AgentEvent::StepUpdated { id: step_id.clone(), state, result });
            };

            let allowed = if approval {
                let (reply, rx) = oneshot::channel();
                let _ = tx.unbounded_send(AgentEvent::Approval { step_id: step_id.clone(), reply });
                tokio::select! {
                    ok = rx => ok.unwrap_or(false),
                    _ = cancel.cancelled() => false,
                }
            } else {
                true
            };
            if cancel.is_cancelled() {
                update(StepState::Denied, None);
                return Outcome::Stopped;
            }
            if !allowed {
                update(StepState::Denied, None);
                history.push(tool_message(call, json!({ "error": "The user declined this action. Do not retry it; ask what they want instead." })));
                continue;
            }
            update(StepState::Running, None);
            match tools.run(call.name.clone(), call.args.clone()).await {
                Ok(out) => {
                    let value = match out {
                        ToolOutput::Value(v) => v,
                        ToolOutput::Navigate(route, v) => {
                            let _ = tx.unbounded_send(AgentEvent::Navigate(route));
                            v
                        }
                        ToolOutput::Suggest(list, v) => {
                            let _ = tx.unbounded_send(AgentEvent::Suggest(list));
                            v
                        }
                    };
                    let content = tools::trim_result(value, 6000);
                    let text = content.to_string();
                    update(StepState::Done, Some(text.chars().take(600).collect()));
                    history.push(tool_message(call, content));
                }
                Err(msg) => {
                    update(StepState::Error, Some(msg.clone()));
                    history.push(tool_message(call, json!({ "error": msg })));
                }
            }
        }
    }
    // Hit the round cap while still calling tools.
    let _ = tx.unbounded_send(AgentEvent::Delta(StreamEvent::Text { delta: format!("\n\n_Stopped after {MAX_ROUNDS} rounds of actions._") }));
    Outcome::Done
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
        assert!(req.tools.as_ref().unwrap().len() >= 14);
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
        let rounds: Vec<_> = (0..20).map(|_| AiStepResult { tool_calls: vec![call("navigate")], ..Default::default() }).collect();
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
