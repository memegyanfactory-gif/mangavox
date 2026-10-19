//! The assistant dock: header, chat-history drawer, message list, approvals and composer.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::channel::{mpsc, oneshot};
use futures::StreamExt;
use gpui_kit::component::input::{InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ai::{provider_def, AiAccessMode, AiEffort, AiMessage, AiProviderId, AiProviderState, AiRole, CancellationToken, StreamEvent};
use mv_ui::theme::{duration, easing, icon as icon_px, radius, ramp};
use mv_ui::toast::{self, Toast};
use mv_ui::{
    btn, copy_button, disclosure, empty_view, glyphs, icon_button, icon_tag, inline_alert, kbd, list_row, menu_button, motion, ph, ph_icon, section_label, shimmer_text, size, spinner, tag, tip, tip_kbd, AlertKind, ButtonKind, Interactive as _, MenuItem, Ph,
    RampExt as _, Route, Tokens, Tone, Weight, MONO_FONT,
};

use crate::agent::{self, AgentEvent, AgentParams, Outcome};
use crate::picker::{pretty_id, ModelPicker};
use crate::shared::{shared, Shared};
use crate::store::{now_ms, title_from, ChatItem, ItemRole, ItemStatus, StepState, ToolStep};
use crate::tools::{self, AppTools};
use crate::ui::{ago_ms, date_group, field, fmt_secs, not_ready_title, provider_logo, search_prefix, set_value, text_state, value_of};

const PLACEHOLDER: &str = "Ask about your library, find anime, or attach a panel…";
const DRAWER_W: f32 = 288.;
const IMAGE_EXTS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "gif"];

struct Run {
    thread: String,
    cancel: CancellationToken,
    access: Arc<Mutex<AiAccessMode>>,
    approvals: HashMap<String, oneshot::Sender<bool>>,
    /// When the first reasoning delta of this reply arrived (for "Thought for 12s").
    thinking_since: Option<u64>,
    _task: Task<()>,
}

struct Rename {
    id: String,
    input: Entity<InputState>,
    _sub: Subscription,
}

pub struct Dock {
    shared: Entity<Shared>,
    /// Keyboard scope of the whole dock (Esc, Ctrl+N, Enter on an approval).
    focus: FocusHandle,
    input: Entity<TextareaState>,
    search: Entity<InputState>,
    picker: Entity<ModelPicker>,
    meter: AnyView,
    attachments: Vec<PathBuf>,
    rename: Option<Rename>,
    drawer: bool,
    scroll: ScrollHandle,
    /// Sections the user opened or closed (thought, activity, step results), by key. Unset keys use
    /// their default, so a choice is never undone when a reply finishes.
    toggled: HashMap<String, bool>,
    /// Follow new output. Off once the user scrolls up; back on at the bottom or via "Jump to latest".
    follow: bool,
    /// The last wheel move went down. Following resumes at the bottom only after one, so a small
    /// touchpad scroll up (a few px, still "at the bottom") isn't undone on the next frame.
    wheel_down: bool,
    /// "Always allow in this chat": access per thread for this session.
    thread_access: HashMap<String, AiAccessMode>,
    run: Option<Run>,
    _subs: Vec<Subscription>,
}

pub fn create(window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| Dock::new(window, cx)).into()
}

impl Dock {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shared = shared(cx);
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 8).submit_on_enter(true).placeholder(PLACEHOLDER));
        let search = text_state("Search chats", window, cx);
        let picker = crate::picker::create_entity(true, window, cx);
        let meter = crate::usage_meter::create(window, cx);
        let mut subs = vec![];
        subs.push(cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::PressEnter { shift: false, .. } => this.send_from_composer(window, cx),
            // Focus changes the composer border and the shortcut hints; typing enables Send.
            InputEvent::Focus | InputEvent::Blur | InputEvent::Change => cx.notify(),
            _ => {}
        }));
        subs.push(cx.subscribe_in(&search, window, |_, _, ev: &InputEvent, _, cx| {
            if let InputEvent::Change = ev {
                cx.notify();
            }
        }));
        subs.push(cx.observe(&shared, |this, _, cx| {
            this.shared.update(cx, |s, cx| s.autopick(cx));
            cx.notify();
        }));
        shared.update(cx, |s, cx| {
            if s.store.active.is_none() {
                s.store.active = s.store.threads.first().map(|t| t.id.clone());
            }
            s.refresh_providers(false, cx);
        });
        Self {
            shared,
            focus: cx.focus_handle(),
            input,
            search,
            picker,
            meter,
            attachments: vec![],
            rename: None,
            drawer: false,
            scroll: ScrollHandle::new(),
            toggled: HashMap::new(),
            follow: true,
            wheel_down: false,
            thread_access: HashMap::new(),
            run: None,
            _subs: subs,
        }
    }

    // ── Sending ──

    fn model_missing(&self, cx: &App) -> bool {
        self.shared.read(cx).store.prefs.model.is_empty()
    }

    /// No model yet: open the picker (or the setup card) instead of failing the message.
    fn ask_for_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ready = self.shared.read(cx).providers.iter().find(|p| p.state == AiProviderState::Ready).map(|p| p.id);
        match ready {
            Some(p) => self.open_picker(p, window, cx),
            None => toast::show(cx, Toast::info("Connect a model first: download one or add an API key.")),
        }
    }

    /// Opens the model picker from the dock (setup card, ready-provider chips, Send with no model).
    /// The composer takes the keyboard first, so the picker hands it back there after a choice and
    /// you can type the first message straight away (kit buttons don't move focus on their own).
    fn open_picker(&mut self, provider: AiProviderId, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| s.focus(window, cx));
        self.picker.update(cx, |p, cx| p.open_at(Some(provider), window, cx));
    }

    /// The running reply belongs to the chat on screen. Chats can be switched while a reply
    /// streams, and the other chat's composer must not stop (or pretend to send) that reply.
    fn running_here(&self, cx: &App) -> bool {
        self.run.as_ref().is_some_and(|r| self.shared.read(cx).store.active.as_deref() == Some(r.thread.as_str()))
    }

    /// "Wait for the reply in “Title” to finish", naming the chat that is busy.
    fn busy_reason(&self, cx: &App) -> String {
        let title = self.run.as_ref().and_then(|r| self.shared.read(cx).store.thread(&r.thread).map(|t| t.title.clone())).filter(|t| !t.trim().is_empty());
        match title {
            Some(t) => format!("Wait for the reply in “{t}” to finish"),
            None => "Wait for the reply in the other chat to finish".into(),
        }
    }

    /// Why New chat / Regenerate wait: the reply on screen, or the one in another chat (named).
    fn wait_reason(&self, cx: &App) -> String {
        if self.running_here(cx) {
            "Wait for the reply to finish".into()
        } else {
            self.busy_reason(cx)
        }
    }

    fn send_from_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        if self.run.is_some() {
            if !self.running_here(cx) {
                // One reply at a time: say why Enter did nothing instead of dropping it silently.
                if !text.trim().is_empty() || !self.attachments.is_empty() {
                    let why = self.busy_reason(cx);
                    toast::show(cx, Toast::info(why));
                }
                return;
            }
            // Enter on an empty composer answers the waiting approval (never one hidden behind
            // the open chat drawer).
            if text.trim().is_empty() && !self.drawer {
                if let Some(id) = self.pending_approval(cx) {
                    self.decide(&id, true, cx);
                }
            }
            return;
        }
        if text.trim().is_empty() && self.attachments.is_empty() {
            return;
        }
        if self.model_missing(cx) {
            self.ask_for_model(window, cx);
            return;
        }
        if let Some(reason) = self.send_blocked(cx) {
            toast::show(cx, Toast::warning(reason));
            return;
        }
        self.input.update(cx, |s, cx| s.set_value("", window, cx));
        let text = if text.trim().is_empty() { "Look at the attached image(s).".to_string() } else { text };
        self.send(text, window, cx);
    }

    /// Why Send can't work right now: the provider has no key, isn't installed or is off.
    fn send_blocked(&self, cx: &App) -> Option<String> {
        let s = self.shared.read(cx);
        let st = s.status(s.store.prefs.provider)?;
        matches!(st.state, AiProviderState::NeedsKey | AiProviderState::NotInstalled | AiProviderState::Disabled).then(|| format!("{} — fix it in Settings first", not_ready_title(&st.label, st.state)))
    }

    fn effective_access(&self, cx: &App) -> AiAccessMode {
        let s = self.shared.read(cx);
        s.store.active.as_ref().and_then(|id| self.thread_access.get(id)).copied().unwrap_or(s.store.prefs.access)
    }

    fn send(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = text.trim().to_string();
        if self.run.is_some() && !prompt.is_empty() && !self.running_here(cx) {
            // A suggestion clicked in another chat: say why nothing happened.
            let why = self.busy_reason(cx);
            toast::show(cx, Toast::info(why));
        }
        if prompt.is_empty() || self.run.is_some() {
            return;
        }
        if self.model_missing(cx) {
            self.ask_for_model(window, cx);
            return;
        }
        let images = std::mem::take(&mut self.attachments);
        let (thread_id, item_id, prefs, history) = self.shared.update(cx, |s, cx| {
            let id = match s.store.active.clone().filter(|id| s.store.thread(id).is_some()) {
                Some(id) => id,
                None => s.store.new_thread(),
            };
            let prefs = s.store.prefs.clone();
            let reply = ChatItem::assistant(&prefs.model);
            let reply_id = reply.id.clone();
            let t = s.store.thread_mut(&id).expect("thread");
            if t.items.is_empty() {
                t.title = title_from(&prompt);
            }
            t.items.push(ChatItem::user(prompt.clone()));
            t.items.push(reply);
            t.updated_at = now_ms();
            let mut history = t.history.clone();
            history.push(AiMessage::user(prompt.clone()));
            s.save();
            cx.notify();
            (id, reply_id, prefs, history)
        });
        self.follow = true;
        self.scroll.scroll_to_bottom();
        // Sent from a suggestion or Retry (kit buttons don't take focus): put the keyboard in the
        // composer so Esc can stop the reply and a follow-up can be typed straight away.
        if !self.focus.contains_focused(window, cx) {
            self.input.update(cx, |s, cx| s.focus(window, cx));
        }

        let def = provider_def(prefs.provider);
        let access_mode = self.thread_access.get(&thread_id).copied().unwrap_or(prefs.access);
        let access = Arc::new(Mutex::new(access_mode));
        let cancel = CancellationToken::new();
        let (tx, mut rx) = mpsc::unbounded::<AgentEvent>();
        let registry = self.shared.read(cx).registry.clone();
        let params = AgentParams { provider: prefs.provider, model: prefs.model.clone(), effort: prefs.effort, access: access.clone(), system: String::new(), history, tools_enabled: def.supports_tools, images };
        let token = cancel.clone();
        mv_ui::bg::runtime().spawn(async move {
            let mut params = params;
            let (page, section) = tools::page_context().map(|r| (tools::page_name(&r), tools::section_name(&r))).unzip();
            let library = tokio::task::spawn_blocking(|| mv_core::library::open_readonly().and_then(|c| mv_core::library::list_manga(&c)).map(|m| m.len()).ok()).await.ok().flatten();
            params.system = agent::system_prompt(params.tools_enabled, page, section, library, &agent::today_string());
            if let Some(extra) = agent::studio_context() {
                params.system.push_str("\n\n");
                params.system.push_str(&extra);
            }
            agent::run_agent(&registry, &AppTools, params, tx, token).await;
        });

        let (tid, iid) = (thread_id.clone(), item_id.clone());
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Some(first) = rx.next().await {
                let mut batch = vec![first];
                while let Ok(ev) = rx.try_recv() {
                    batch.push(ev);
                }
                let done = batch.iter().any(|e| matches!(e, AgentEvent::Finished { .. }));
                if this.update_in(cx, |this, window, cx| this.apply(&tid, &iid, batch, window, cx)).is_err() || done {
                    break;
                }
                // Batch deltas: at most ~30 UI updates per second.
                cx.background_executor().timer(Duration::from_millis(33)).await;
            }
        });
        self.run = Some(Run { thread: thread_id, cancel, access, approvals: HashMap::new(), thinking_since: None, _task: task });
        cx.notify();
    }

    fn finish_item(&mut self, thread_id: &str, item_id: &str, status: ItemStatus, error: Option<String>, history: Option<Vec<AiMessage>>, cx: &mut Context<Self>) {
        let (tid, iid) = (thread_id.to_string(), item_id.to_string());
        self.shared.update(cx, |s, cx| {
            if let Some(t) = s.store.thread_mut(&tid) {
                t.updated_at = now_ms();
                if let Some(h) = history {
                    t.history = h;
                }
                if let Some(i) = t.items.iter_mut().find(|i| i.id == iid) {
                    i.status = status;
                    i.error = error;
                    for st in &mut i.steps {
                        if matches!(st.state, StepState::Running | StepState::AwaitingApproval) {
                            st.state = StepState::Denied;
                        }
                    }
                }
            }
            s.save();
            cx.notify();
        });
    }

    fn apply(&mut self, thread_id: &str, item_id: &str, batch: Vec<AgentEvent>, window: &mut Window, cx: &mut Context<Self>) {
        let mut finished: Option<(Outcome, Vec<AiMessage>)> = None;
        let mut navigations = vec![];
        let mut approvals = vec![];
        let mut since = self.run.as_ref().and_then(|r| r.thinking_since);
        let (tid, iid) = (thread_id.to_string(), item_id.to_string());
        self.shared.update(cx, |s, cx| {
            let Some(item) = s.store.thread_mut(&tid).and_then(|t| t.items.iter_mut().find(|i| i.id == iid)) else { return };
            for ev in batch {
                match ev {
                    AgentEvent::Delta(StreamEvent::Text { delta }) => {
                        // The answer started: the thinking time is known.
                        if let (Some(t0), None, true) = (since, item.reasoning_ms, item.reasoning.is_some()) {
                            item.reasoning_ms = Some(now_ms().saturating_sub(t0));
                        }
                        item.text.push_str(&delta)
                    }
                    AgentEvent::Delta(StreamEvent::Reasoning { delta }) => {
                        since.get_or_insert_with(now_ms);
                        item.reasoning.get_or_insert_with(String::new).push_str(&delta)
                    }
                    AgentEvent::Delta(StreamEvent::Status { text }) => push_activity(item, text),
                    AgentEvent::Delta(StreamEvent::Activity { label, detail }) => push_activity(item, detail.map(|d| format!("{label} · {d}")).unwrap_or(label)),
                    AgentEvent::Round(n) => {
                        if n > 0 && !item.text.is_empty() && !item.text.ends_with('\n') {
                            item.text.push_str("\n\n");
                        }
                    }
                    AgentEvent::StepAdded(step) => item.steps.push(step),
                    AgentEvent::StepUpdated { id, state, result } => {
                        if let Some(st) = item.steps.iter_mut().find(|s| s.id == id) {
                            st.state = state;
                            if result.is_some() {
                                st.result = result;
                            }
                        }
                    }
                    AgentEvent::Approval { step_id, reply } => approvals.push((step_id, reply)),
                    AgentEvent::Navigate(route) => navigations.push(route),
                    AgentEvent::Finished { outcome, history } => {
                        if let (Some(t0), None, true) = (since, item.reasoning_ms, item.reasoning.is_some()) {
                            item.reasoning_ms = Some(now_ms().saturating_sub(t0));
                        }
                        finished = Some((outcome, history))
                    }
                }
            }
            cx.notify();
        });
        let asked = !approvals.is_empty();
        if let Some(run) = self.run.as_mut() {
            run.thinking_since = since;
            for (id, reply) in approvals {
                run.approvals.insert(id, reply);
            }
        }
        // An approval needs the keyboard: when nothing has focus, give it to the dock so Enter and
        // Esc answer it (a click on a kit button does not move focus on its own). Never take it
        // from a field elsewhere in the app: keys typed there (an Enter in a page search) would
        // otherwise land on the card and approve an action the user hasn't read.
        if asked && window.focused(cx).is_none() {
            window.focus(&self.focus, cx);
        }
        // The reply belongs to a chat you switched away from: its card isn't on screen, and the
        // reply would wait for an answer nobody can see.
        if asked && !self.running_here(cx) {
            let title = self.shared.read(cx).store.thread(thread_id).map(|t| t.title.clone()).unwrap_or_default();
            let (tid, weak) = (thread_id.to_string(), cx.weak_entity());
            toast::show(
                cx,
                Toast::warning(format!("The assistant needs your OK in “{title}”")).icon(ph::SHIELD_WARNING).action("Show", move |w, cx| {
                    let _ = weak.update(cx, |this, cx| this.select_thread(tid.clone(), w, cx));
                }),
            );
        }
        for route in navigations {
            if route != Route::Back {
                tools::set_page_context(&route);
            }
            mv_ui::navigate(route, window, cx);
        }
        if let Some((outcome, history)) = finished {
            let (status, error) = match outcome {
                Outcome::Done => (ItemStatus::Done, None),
                Outcome::Stopped => (ItemStatus::Stopped, None),
                Outcome::Error(e) => (ItemStatus::Error, Some(e)),
            };
            self.finish_item(thread_id, item_id, status, error, Some(history), cx);
            self.run = None;
        }
        if self.follow {
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = self.run.as_mut() {
            run.cancel.cancel();
            run.approvals.clear();
        }
        cx.notify();
    }

    /// The approval the running reply is waiting on, if it is in the chat on screen (Enter and Esc
    /// never answer a card the user can't see).
    fn pending_approval(&self, cx: &App) -> Option<String> {
        let run = self.run.as_ref()?;
        let s = self.shared.read(cx);
        if s.store.active.as_deref() != Some(run.thread.as_str()) {
            return None;
        }
        let t = s.store.thread(&run.thread)?;
        t.items.iter().rev().flat_map(|i| i.steps.iter()).find(|st| st.state == StepState::AwaitingApproval && run.approvals.contains_key(&st.id)).map(|st| st.id.clone())
    }

    fn decide(&mut self, step_id: &str, ok: bool, cx: &mut Context<Self>) {
        if let Some(run) = self.run.as_mut() {
            if let Some(reply) = run.approvals.remove(step_id) {
                let _ = reply.send(ok);
            }
        }
        cx.notify();
    }

    /// "Always allow in this chat": safe actions stop asking for the rest of this chat (risky ones
    /// still ask), starting with the one on screen.
    fn always_allow(&mut self, step_id: &str, cx: &mut Context<Self>) {
        if let Some(run) = &self.run {
            self.thread_access.insert(run.thread.clone(), AiAccessMode::Auto);
            *run.access.lock().unwrap_or_else(|e| e.into_inner()) = AiAccessMode::Auto;
        }
        self.decide(step_id, true, cx);
        toast::show(cx, Toast::info("Actions in this chat now run without asking. Risky ones still ask."));
    }

    fn retry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.run.is_some() {
            return;
        }
        let text = self.shared.update(cx, |s, cx| {
            let id = s.store.active.clone()?;
            let t = s.store.thread_mut(&id)?;
            let cut = t.items.iter().rposition(|i| i.role == ItemRole::User)?;
            let text = t.items[cut].text.clone();
            t.items.truncate(cut);
            if let Some(h) = t.history.iter().rposition(|m| m.role == AiRole::User) {
                t.history.truncate(h);
            }
            cx.notify();
            Some(text)
        });
        if let Some(text) = text {
            self.send(text, window, cx);
        }
    }

    /// Edit the last message: its text goes back into the composer and the exchange is removed,
    /// so sending runs it again (Claude / ChatGPT "Edit"). The reply would otherwise be lost if
    /// you change your mind, so a toast offers Undo until something new is sent.
    fn edit_last(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.run.is_some() {
            return;
        }
        let removed = self.shared.update(cx, |s, cx| {
            let id = s.store.active.clone()?;
            let t = s.store.thread_mut(&id)?;
            let cut = t.items.iter().rposition(|i| i.role == ItemRole::User)?;
            let text = t.items[cut].text.clone();
            let items = t.items.split_off(cut);
            let history = t.history.iter().rposition(|m| m.role == AiRole::User).map(|h| t.history.split_off(h)).unwrap_or_default();
            s.save();
            cx.notify();
            Some((id, cut, text, items, history))
        });
        let Some((tid, cut, text, items, history)) = removed else { return };
        self.input.update(cx, |s, cx| {
            s.set_value(text, window, cx);
            s.focus(window, cx);
        });
        let weak = cx.weak_entity();
        toast::show(
            cx,
            Toast::info("Editing your last message").icon(glyphs::EDIT).action("Undo", move |w, cx| {
                let restored = shared(cx).update(cx, |s, cx| {
                    // Only while nothing was sent in that chat since.
                    let t = s.store.thread_mut(&tid).filter(|t| t.items.len() == cut)?;
                    t.items.extend(items.iter().cloned());
                    t.history.extend(history.iter().cloned());
                    s.save();
                    cx.notify();
                    Some(())
                });
                if restored.is_some() {
                    let _ = weak.update(cx, |this, cx| this.input.update(cx, |s, cx| s.set_value("", w, cx)));
                }
            }),
        );
    }

    fn new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.run.is_some() {
            return;
        }
        self.shared.update(cx, |s, cx| {
            s.store.new_thread();
            s.save();
            cx.notify();
        });
        self.drawer = false;
        self.commit_rename(cx);
        self.follow = true;
        self.input.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    /// Switching chats is allowed while a reply streams: it keeps writing to its own thread (the
    /// drawer marks that chat with a spinner) instead of the click doing nothing.
    fn select_thread(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        // Clicking another chat finishes a rename in progress (as a field losing focus would).
        self.commit_rename(cx);
        self.shared.update(cx, |s, cx| {
            s.store.active = Some(id);
            s.save();
            cx.notify();
        });
        self.drawer = false;
        self.follow = true;
        self.scroll.scroll_to_bottom();
        self.input.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    fn toggle_drawer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.drawer {
            self.close_drawer(window, cx);
            return;
        }
        self.drawer = true;
        // So Esc closes it even when it was opened with the mouse.
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Closes the drawer (Close button, a click on the scrim, Esc) and hands the keyboard back to
    /// the composer. A rename in progress is kept, as a field losing focus would keep it.
    fn close_drawer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_rename(cx);
        self.drawer = false;
        self.input.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    /// Saves the rename field's text, if one is open (Esc is the only way to cancel it).
    fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.rename.take() else { return };
        let name = value_of(&r.input, cx);
        self.shared.update(cx, |s, cx| {
            s.store.rename_thread(&r.id, &name);
            s.save();
            cx.notify();
        });
    }

    fn start_rename(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        // Renaming a second chat keeps what was typed for the first.
        self.commit_rename(cx);
        let current = self.shared.read(cx).store.thread(&id).map(|t| t.title.clone()).unwrap_or_default();
        let input = text_state("Chat name", window, cx);
        set_value(&input, &current, window, cx);
        let rid = id.clone();
        let sub = cx.subscribe_in(&input, window, move |this, input, ev: &InputEvent, window, cx| {
            let enter = matches!(ev, InputEvent::PressEnter { .. });
            if !enter && !matches!(ev, InputEvent::Blur) {
                return;
            }
            // Esc cancelled it already (the field blurs as it goes away).
            if this.rename.as_ref().map(|r| r.id != rid).unwrap_or(true) {
                return;
            }
            let name = value_of(input, cx);
            let rid = rid.clone();
            this.shared.update(cx, |s, cx| {
                s.store.rename_thread(&rid, &name);
                s.save();
                cx.notify();
            });
            this.rename = None;
            // Enter removes the field that had the keyboard: keep it in the drawer so Esc still
            // closes it (on Blur the user already put focus somewhere else).
            if enter {
                window.focus(&this.focus, cx);
            }
            cx.notify();
        });
        input.update(cx, |s, cx| s.focus(window, cx));
        self.rename = Some(Rename { id, input, _sub: sub });
        cx.notify();
    }

    /// Deletes at once and offers Undo (a chat is cheap to restore, so no confirmation).
    fn delete_thread(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.run.as_ref().is_some_and(|r| r.thread == id) {
            return;
        }
        let removed = self.shared.update(cx, |s, cx| {
            let was_active = s.store.active.as_deref() == Some(id);
            let r = s.store.take_thread(id);
            s.save();
            cx.notify();
            r.map(|(ix, t)| (ix, t, was_active))
        });
        let Some((ix, thread, was_active)) = removed else { return };
        let title = thread.title.clone();
        toast::show(
            cx,
            Toast::success(format!("Deleted “{title}”")).icon(glyphs::DELETE).action("Undo", move |_, cx| {
                let t = thread.clone();
                shared(cx).update(cx, |s, cx| {
                    s.store.restore_thread(ix, t, was_active);
                    s.save();
                    cx.notify();
                });
            }),
        );
        cx.notify();
    }

    fn set_pref(&mut self, f: impl FnOnce(&mut crate::store::Prefs), cx: &mut Context<Self>) {
        self.shared.update(cx, |s, cx| {
            f(&mut s.store.prefs);
            s.save();
            cx.notify();
        });
        // A running turn picks up access changes immediately: the mode of ITS chat, which may not
        // be the one on screen (an effort change in another chat must not undo "Always allow").
        if let Some(run) = &self.run {
            let access = self.thread_access.get(&run.thread).copied().unwrap_or(self.shared.read(cx).store.prefs.access);
            *run.access.lock().unwrap_or_else(|e| e.into_inner()) = access;
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        let plain = !(m.control || m.alt || m.shift || m.platform);
        match ks.key.as_str() {
            // The topmost surface first: a rename, then the drawer over the chat, then the
            // approval on screen, then the running reply.
            "escape" if plain => {
                if self.rename.is_some() {
                    self.rename = None;
                    // Keep the keyboard in the drawer so a second Esc closes it.
                    window.focus(&self.focus, cx);
                } else if self.drawer {
                    self.close_drawer(window, cx);
                } else if let Some(id) = self.pending_approval(cx) {
                    self.decide(&id, false, cx);
                } else if self.running_here(cx) {
                    // Only the reply on screen; one in a chat you switched away from keeps going.
                    self.stop(cx);
                } else {
                    return;
                }
                cx.stop_propagation();
                cx.notify();
            }
            "enter" if plain && !self.drawer && self.focus.is_focused(window) => {
                if let Some(id) = self.pending_approval(cx) {
                    self.decide(&id, true, cx);
                    cx.stop_propagation();
                }
            }
            "n" if m.control && !m.shift && !m.alt => {
                self.new_chat(window, cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    // ── Header and drawer ──

    fn header(&self, title: String, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self.run.is_some();
        let wait = self.wait_reason(cx);
        let long = title.chars().count() > 34;
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .h(px(44.))
            .px(px(8.))
            .flex_none()
            .border_b_1()
            .border_color(Tokens::border())
            .child(icon_button("dock-chats", ph::SIDEBAR_SIMPLE, "Chat history").small().selected(self.drawer).on_click(cx.listener(|this, _, window, cx| this.toggle_drawer(window, cx))))
            .child(
                div()
                    .id("dock-title")
                    .flex_1()
                    .min_w_0()
                    .px(px(4.))
                    .truncate()
                    .text_size(px(size::MD))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(Tokens::text_1())
                    .when(long, |d| d.tooltip(tip(title.clone())))
                    .child(title),
            )
            .child(self.meter.clone())
            .child(
                icon_button("dock-new", ph::NOTE_PENCIL, "New chat")
                    .small()
                    .kbd("ctrl-n")
                    .when(running, |b| b.disabled(wait.clone()))
                    .on_click(cx.listener(|this, _, w, cx| this.new_chat(w, cx))),
            )
    }

    fn drawer_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let q = value_of(&self.search, cx);
        let (rows, active): (Vec<(String, String, u64)>, Option<String>) = {
            let s = self.shared.read(cx);
            (s.store.search(&q).into_iter().map(|t| (t.id.clone(), t.title.clone(), t.updated_at)).collect(), s.store.active.clone())
        };
        let busy_thread = self.run.as_ref().map(|r| r.thread.clone());
        let running = self.run.is_some();
        let wait = self.wait_reason(cx);
        let mut list = div().id("dock-thread-list").flex().flex_col().gap(px(1.)).flex_1().min_h_0().overflow_y_scroll().px(px(6.)).pb(px(10.));
        if rows.is_empty() {
            list = list.child(if q.is_empty() {
                empty_view(ph::CHATS_CIRCLE, "No chats yet", "Your conversations with the assistant show up here.")
                    .compact()
                    .action(btn("drawer-start", "Start a chat", ButtonKind::Primary).small().icon(ph::NOTE_PENCIL).on_click(cx.listener(|this, _, w, cx| this.new_chat(w, cx))))
            } else {
                empty_view(glyphs::SEARCH, "No matching chats", format!("Nothing matches “{q}”.")).compact()
            });
        }
        let mut last_group = "";
        for (i, (id, title, updated)) in rows.into_iter().enumerate() {
            let group = date_group(updated);
            if group != last_group {
                list = list.child(section_label(group).px(px(8.)).pt(px(if i == 0 { 4. } else { 12. })));
                last_group = group;
            }
            let is_active = active.as_deref() == Some(id.as_str());
            let busy = busy_thread.as_deref() == Some(id.as_str());
            list = list.child(self.thread_row(i, id, title, updated, is_active, busy, cx));
        }
        let panel = div()
            .id("dock-drawer")
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .w(px(DRAWER_W))
            .flex()
            .flex_col()
            .bg(Tokens::sidebar())
            .border_r_1()
            .border_color(Tokens::border())
            .shadow_lg()
            .occlude()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(4.))
                    .h(px(44.))
                    .pl(px(14.))
                    .pr(px(8.))
                    .child(div().flex_1().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Chats"))
                    .child(icon_button("drawer-new", ph::NOTE_PENCIL, "New chat").small().kbd("ctrl-n").when(running, |b| b.disabled(wait.clone())).on_click(cx.listener(|this, _, w, cx| this.new_chat(w, cx))))
                    .child(icon_button("drawer-close", glyphs::CLOSE, "Close").small().kbd("escape").on_click(cx.listener(|this, _, w, cx| this.toggle_drawer(w, cx)))),
            )
            .child(div().flex_none().px(px(8.)).pb(px(8.)).child(field(&self.search).small().cleanable(true).prefix(search_prefix())))
            .child(list);
        // Slide in from the left (Material 3 enter easing); static under reduced motion.
        let panel = if motion::reduced() {
            panel.into_any_element()
        } else {
            panel.with_animation("dock-drawer-in", Animation::new(duration::BASE).with_easing(easing::enter), |d, t| d.left(px(-DRAWER_W * (1. - t)))).into_any_element()
        };
        let scrim = div()
            .id("dock-scrim")
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .bg(Tokens::overlay())
            .occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                this.close_drawer(window, cx);
                // The dock root would otherwise take focus from the composer on this press.
                window.prevent_default();
            }));
        div().absolute().top_0().bottom_0().left_0().right_0().child(motion::appear("dock-scrim-in", scrim)).child(panel)
    }

    #[allow(clippy::too_many_arguments)]
    fn thread_row(&self, i: usize, id: String, title: String, updated: u64, is_active: bool, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        if let Some(r) = self.rename.as_ref().filter(|r| r.id == id) {
            return div().flex_none().px(px(2.)).py(px(2.)).child(field(&r.input).small()).into_any_element();
        }
        // Its running reply is waiting for an approval: the row says so rather than spinning.
        let waiting = busy && self.run.as_ref().is_some_and(|r| !r.approvals.is_empty());
        let (sel, ren, del) = (id.clone(), id.clone(), id);
        // The hover actions sit on a fade of the row's own colour, so they never print over the title.
        let solid = Tokens::sidebar().blend(if is_active { Tokens::active_hover() } else { Tokens::hover() });
        let fade = linear_gradient(90., linear_color_stop(solid.alpha(0.), 0.), linear_color_stop(solid, 0.3));
        list_row(("thread", i), is_active)
            .group("thread-row")
            .relative()
            .flex_none()
            .h(px(34.))
            .min_h(px(34.))
            .py(px(0.))
            .gap(px(8.))
            .text_size(px(size::SM))
            .on_click(cx.listener(move |this, _, window, cx| this.select_thread(sel.clone(), window, cx)))
            .child(div().flex_1().min_w_0().truncate().child(title))
            .child(div().flex_none().text_size(px(size::XS)).text_color(Tokens::text_3()).group_hover("thread-row", |s| s.invisible()).map(|d| {
                if waiting {
                    d.child(ph_icon(ph::SHIELD_WARNING, Weight::Fill, Tokens::warning(), icon_px::XS))
                } else if busy {
                    d.child(spinner(("thread-busy", i), Tokens::accent(), icon_px::XS))
                } else {
                    d.child(ago_ms(updated))
                }
            }))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right(px(2.))
                    .flex()
                    .items_center()
                    .pl(px(18.))
                    .rounded_r(px(radius::MD))
                    .bg(fade)
                    .invisible()
                    .group_hover("thread-row", |s| s.visible())
                    .child(icon_button(("ren", i), glyphs::EDIT, "Rename").small().w(px(26.)).h(px(26.)).on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.start_rename(ren.clone(), window, cx);
                    })))
                    .child(
                        icon_button(("del", i), glyphs::DELETE, "Delete chat")
                            .small()
                            .danger()
                            .w(px(26.))
                            .h(px(26.))
                            .when(busy, |b| b.disabled("Stop the reply first"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.delete_thread(&del, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    // ── Messages ──

    fn is_open(&self, key: &str, default: bool) -> bool {
        self.toggled.get(key).copied().unwrap_or(default)
    }

    fn render_item(&self, item: &ChatItem, ix: usize, last: bool, last_user: bool, cx: &mut Context<Self>) -> AnyElement {
        match item.role {
            ItemRole::User => {
                let editable = last_user && self.run.is_none();
                div()
                    .id(("user-msg", ix))
                    .group("user-msg")
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(px(2.))
                    .child(
                        div()
                            .max_w(relative(0.88))
                            .px(px(12.))
                            .py(px(8.))
                            .rounded(px(radius::XL))
                            .bg(Tokens::surface_2())
                            .text_size(px(size::MD))
                            .line_height(px(20.))
                            .text_color(Tokens::text_1())
                            .child(item.text.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .invisible()
                            .group_hover("user-msg", |s| s.visible())
                            .child(copy_button(("copy-user", ix), item.text.clone()).small())
                            .when(editable, |d| d.child(icon_button(("edit-user", ix), glyphs::EDIT, "Edit message").small().on_click(cx.listener(|this, _, w, cx| this.edit_last(w, cx))))),
                    )
                    .into_any_element()
            }
            ItemRole::Assistant => self.render_assistant(item, ix, last, cx),
        }
    }

    /// A row that opens and closes a section (thinking, activity). Its icons light up with the text.
    fn section_toggle(&self, key: String, icon: Ph, title: SharedString, open: bool, live: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let k = key.clone();
        div()
            .id(SharedString::from(format!("hdr-{key}")))
            .group("sec-hdr")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(24.))
            .px(px(6.))
            .ml(px(-6.))
            .rounded(px(radius::SM))
            .text_size(px(size::SM))
            .text_color(Tokens::text_3())
            .role(Role::Button)
            .aria_expanded(open)
            .hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
            .pressable()
            .child(ph_icon(icon, Weight::Bold, Tokens::text_3(), icon_px::SM).group_hover("sec-hdr", |s| s.text_color(Tokens::text_1())))
            .child(if live { shimmer_text(title).into_any_element() } else { div().child(title).into_any_element() })
            .child(ph_icon(if open { ph::CARET_DOWN } else { ph::CARET_RIGHT }, Weight::Bold, Tokens::text_3(), icon_px::XS).group_hover("sec-hdr", |s| s.text_color(Tokens::text_1())))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggled.insert(k.clone(), !open);
                cx.notify();
            }))
    }

    fn render_assistant(&self, item: &ChatItem, ix: usize, last: bool, cx: &mut Context<Self>) -> AnyElement {
        let streaming = item.status == ItemStatus::Streaming;
        let idle = self.run.is_none();
        let mut col = div().id(("asst-msg", ix)).group("asst-msg").flex().flex_col().gap(px(8.)).w_full();

        if let Some(reasoning) = item.reasoning.as_ref().filter(|r| !r.trim().is_empty()) {
            let key = format!("thought:{}", item.id);
            let open = self.is_open(&key, false);
            let thinking = streaming && item.text.is_empty() && item.steps.is_empty();
            let title: SharedString = if thinking {
                "Thinking…".into()
            } else if let Some(ms) = item.reasoning_ms {
                format!("Thought for {}", fmt_secs(ms)).into()
            } else {
                "Thought process".into()
            };
            col = col.child(self.section_toggle(key, ph::BRAIN, title, open, thinking, cx));
            if open {
                col = col.child(div().ml(px(6.)).pl(px(10.)).border_l_1().border_color(Tokens::border()).text_size(px(size::SM)).line_height(px(19.)).text_color(Tokens::text_3()).child(reasoning.trim().to_string()));
            }
        }

        if !item.activity.is_empty() {
            let key = format!("activity:{}", item.id);
            // Open while the reply streams, closed after, unless the user chose otherwise.
            let open = self.is_open(&key, streaming);
            col = col.child(self.section_toggle(key, ph::PULSE, format!("Activity · {}", item.activity.len()).into(), open, false, cx));
            if open {
                col = col.child(
                    div().flex().flex_col().gap(px(3.)).ml(px(6.)).pl(px(10.)).border_l_1().border_color(Tokens::border()).children(
                        item.activity.iter().rev().take(8).rev().map(|a| {
                            div().flex().gap(px(6.)).items_center().text_size(px(size::XS)).text_color(Tokens::text_3()).child(ph_icon(ph::TERMINAL_WINDOW, Weight::Bold, Tokens::text_3(), icon_px::XS)).child(div().truncate().child(a.clone()))
                        }),
                    ),
                );
            }
        }

        if !item.steps.is_empty() {
            let mut rows = div().flex().flex_col().gap(px(1.));
            for st in &item.steps {
                let key = format!("step:{}", st.id);
                let open = self.is_open(&key, st.state == StepState::Error);
                rows = rows.child(step_row(st, key, open, cx));
            }
            col = col.child(rows);
            let waiting = self.run.as_ref().map(|r| &r.approvals);
            for st in item.steps.iter().filter(|s| s.state == StepState::AwaitingApproval && waiting.is_some_and(|w| w.contains_key(&s.id))) {
                col = col.child(self.approval_card(st, cx));
            }
        }

        if !item.text.trim().is_empty() {
            col = col.child(div().text_size(px(size::MD)).text_color(Tokens::text_1()).child(TextView::markdown(SharedString::from(format!("md-{}", item.id)), item.text.clone()).selectable(true)));
        } else if streaming && item.reasoning.is_none() && item.steps.is_empty() && item.activity.is_empty() {
            col = col.child(div().flex().items_center().gap(px(8.)).h(px(24.)).child(ph_icon(glyphs::ASSISTANT, Weight::Fill, Tokens::accent(), icon_px::SM)).child(shimmer_text("Thinking…").text_size(px(size::MD))));
        }

        match item.status {
            ItemStatus::Done if !item.text.trim().is_empty() => {
                let model = item.model.as_deref().filter(|m| !m.is_empty()).map(|m| pretty_id(AiProviderId::Custom, m));
                let wait = self.wait_reason(cx);
                col = col.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .ml(px(-6.))
                        // Always there on the latest answer; on hover for older ones.
                        .when(!last, |d| d.invisible().group_hover("asst-msg", |s| s.visible()))
                        .child(copy_button(("copy-reply", ix), item.text.clone()).small())
                        .when(last, |d| {
                            d.child(icon_button(("regen", ix), glyphs::RETRY, "Regenerate").small().when(!idle, |b| b.disabled(wait)).on_click(cx.listener(|this, _, w, cx| this.retry(w, cx))))
                        })
                        .when_some(model, |d, m| d.child(div().pl(px(6.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(m))),
                );
            }
            ItemStatus::Error => col = col.child(self.error_block(item, ix, last && idle, cx)),
            ItemStatus::Stopped => {
                col = col.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(size::SM))
                        .text_color(Tokens::text_3())
                        .child(ph_icon(ph::STOP_CIRCLE, Weight::Bold, Tokens::text_3(), icon_px::SM))
                        .child("Stopped")
                        .when(last && idle, |d| d.child(btn(("continue", ix), "Continue", ButtonKind::Ghost).small().icon(ph::PLAY).on_click(cx.listener(|this, _, w, cx| this.send("Continue".into(), w, cx))))),
                );
            }
            _ => {}
        }
        col.into_any_element()
    }

    /// Plain-language error with a way forward; the provider's raw message sits behind Details.
    fn error_block(&self, item: &ChatItem, ix: usize, can_retry: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let raw = item.error.clone().unwrap_or_else(|| "Something went wrong.".into());
        let label = provider_def(self.shared.read(cx).store.prefs.provider).label;
        let (title, hint) = friendly_error(label, &raw);
        // An unrecognised error already shows its text as the hint; Details only when it was cut.
        let show_raw = if title == "Something went wrong" { raw.trim().chars().count() > 160 } else { true };
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(inline_alert(AlertKind::Error, title, hint))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(6.))
                    .when(can_retry, |d| d.child(btn(("retry", ix), "Retry", ButtonKind::Secondary).small().icon(glyphs::RETRY).on_click(cx.listener(|this, _, w, cx| this.retry(w, cx)))))
                    .when(show_raw, |d| {
                        d.child(div().flex_1().min_w_0().child(disclosure(("err-details", ix), "Details").content(div().text_size(px(size::XS)).font_family(MONO_FONT).text_color(Tokens::text_3()).child(truncate_chars(&raw, 1200)))))
                    }),
            )
    }

    fn approval_card(&self, st: &ToolStep, cx: &mut Context<Self>) -> impl IntoElement {
        let (allow_id, deny_id, always_id) = (st.id.clone(), st.id.clone(), st.id.clone());
        let args = st.args.as_object().cloned().unwrap_or_default();
        let p = tools::phrases(&st.name, &args);
        let destructive = st.risk == "destructive";
        // Known tools say everything in the title; show raw arguments only for unknown ones.
        let raw_args = if tools::risk_of(&st.name).is_none() && !args.is_empty() {
            args.iter().map(|(k, v)| format!("{k}: {}", v.as_str().map(String::from).unwrap_or_else(|| v.to_string()))).collect::<Vec<_>>().join("\n")
        } else {
            String::new()
        };
        let (glyph, fg, edge, soft) = if destructive { (ph::WARNING, Tokens::danger(), Tokens::danger_border(), Tokens::danger_soft()) } else { (ph::SHIELD_WARNING, Tokens::warning(), Tokens::warning_border(), Tokens::warning_soft()) };
        let badge = if destructive { icon_tag(ph::WARNING, "Can't be undone", Tone::Danger) } else { icon_tag(ph::PENCIL_SIMPLE, "Changes the app", Tone::Warning) };
        let sid = st.id.clone();
        div()
            .id(SharedString::from(format!("approval-{sid}")))
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(12.))
            .rounded(px(radius::LG))
            .border_1()
            .border_color(edge)
            .bg(soft)
            .role(Role::Alert)
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .child(div().pt(px(2.)).child(ph_icon(glyph, Weight::Fill, fg, icon_px::MD)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .flex_1()
                            .min_w_0()
                            .child(div().text_size(px(size::MD)).line_height(px(20.)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("{}?", p.ask)))
                            .child(div().flex().child(badge)),
                    ),
            )
            .when(!raw_args.is_empty(), |d| d.child(div().px(px(10.)).py(px(6.)).rounded(px(radius::SM)).bg(Tokens::well()).font_family(MONO_FONT).text_size(px(size::XS)).text_color(Tokens::text_2()).child(raw_args)))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        btn(SharedString::from(format!("allow-{sid}")), "Allow", if destructive { ButtonKind::Danger } else { ButtonKind::Primary })
                            .small()
                            .icon(ph::CHECK)
                            .kbd("enter")
                            .on_click(cx.listener(move |this, _, _, cx| this.decide(&allow_id, true, cx))),
                    )
                    .child(btn(SharedString::from(format!("deny-{sid}")), "Deny", ButtonKind::Secondary).small().icon(glyphs::CLOSE).kbd("escape").on_click(cx.listener(move |this, _, _, cx| this.decide(&deny_id, false, cx))))
                    .when(!destructive, |d| {
                        d.child(
                            btn(SharedString::from(format!("always-{sid}")), "Always allow in this chat", ButtonKind::Ghost)
                                .small()
                                .tooltip("Safe actions run without asking for the rest of this chat")
                                .on_click(cx.listener(move |this, _, _, cx| this.always_allow(&always_id, cx))),
                        )
                    }),
            )
    }

    fn empty_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let (model_missing, loaded, ready) = {
            let s = self.shared.read(cx);
            (s.store.prefs.model.is_empty(), s.providers_loaded, s.providers.iter().filter(|p| p.state == AiProviderState::Ready).map(|p| (p.id, p.label.clone())).collect::<Vec<_>>())
        };
        // Centred with auto margins, not justify_center: content taller than a short dock (the
        // setup card plus provider chips) then scrolls from its top instead of being cut off there.
        let body = div().flex().flex_col().items_center().gap(px(14.)).w_full().my_auto().py(px(16.));
        let shell = |body: Div| div().id("dock-empty").flex().flex_col().size_full().px(px(20.)).overflow_y_scroll().child(body).into_any_element();
        if model_missing && !loaded {
            return shell(body.child(spinner("dock-detect", Tokens::text_3(), icon_px::LG)).child(shimmer_text("Looking for AI on this PC…").text_size(px(size::MD))));
        }
        if model_missing {
            // First run: a path to a working model instead of a message that fails.
            let to = |tab: &'static str| move |_: &ClickEvent, w: &mut Window, cx: &mut App| mv_ui::navigate(Route::Settings { tab: Some(tab.into()) }, w, cx);
            let setup = match ready.first().map(|r| r.0) {
                Some(first) => empty_view(ph::PLUGS_CONNECTED, "Choose a model to start chatting", "These are ready on this PC. Pick a model, or add an API key for a cloud one.")
                    .action(btn("setup-choose", "Choose a model", ButtonKind::Primary).icon(glyphs::MODEL).on_click(cx.listener(move |this, _, w, cx| this.open_picker(first, w, cx))))
                    .secondary(btn("setup-key", "Add an API key", ButtonKind::Secondary).icon(ph::KEY).on_click(to("providers"))),
                None => empty_view(ph::PLUGS_CONNECTED, "Connect a model to start chatting", "Run a model on this PC, or add an API key for a cloud model.")
                    .action(btn("setup-local", "Download a local model", ButtonKind::Primary).icon(ph::CLOUD_ARROW_DOWN).on_click(to("models")))
                    .secondary(btn("setup-key", "Add an API key", ButtonKind::Secondary).icon(ph::KEY).on_click(to("providers"))),
            };
            return shell(
                body.child(setup)
                .when(!ready.is_empty(), |d| {
                    d.child(div().flex().flex_wrap().justify_center().gap(px(6.)).children(ready.into_iter().enumerate().map(|(i, (id, label))| {
                        div()
                            .id(("ready-prov", i))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .h(px(28.))
                            .px(px(10.))
                            .rounded(px(radius::PILL))
                            .border_1()
                            .border_color(Tokens::border())
                            .text_size(px(size::SM))
                            .text_color(Tokens::text_2())
                            .hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()).border_color(Tokens::border_strong()))
                            .pressable()
                            .tooltip(tip(format!("Choose a {label} model")))
                            .child(provider_logo(id, icon_px::SM))
                            .child(label)
                            .on_click(cx.listener(move |this, _, w, cx| this.open_picker(id, w, cx)))
                    })))
                }),
            );
        }
        let page = tools::page_context();
        let items = suggestions(page.as_ref().map(tools::page_name));
        shell(
            body.child(div().flex().items_center().justify_center().size(px(52.)).rounded_full().bg(Tokens::accent_soft()).child(ph_icon(glyphs::ASSISTANT, Weight::Duotone, Tokens::accent(), icon_px::XL)))
            .child(div().ramp(ramp::SUBTITLE).text_color(Tokens::text_1()).child("How can I help?"))
            .child(div().max_w(px(320.)).text_center().text_size(px(size::MD)).line_height(px(20.)).text_color(Tokens::text_3()).child("Ask about your library, find something to watch, or let me open pages and change settings for you."))
            .child(div().flex().flex_col().gap(px(6.)).w_full().pt(px(6.)).children(items.into_iter().enumerate().map(|(i, (glyph, text))| {
                div()
                    .id(("suggest", i))
                    .group("suggest")
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(px(radius::LG))
                    .border_1()
                    .border_color(Tokens::border())
                    .text_size(px(size::SM))
                    .text_color(Tokens::text_2())
                    .role(Role::Button)
                    .hover(|s| s.bg(Tokens::hover()).border_color(Tokens::border_strong()).text_color(Tokens::text_1()))
                    .pressable()
                    .child(ph_icon(glyph, Weight::Regular, Tokens::accent(), icon_px::MD))
                    .child(div().flex_1().min_w_0().child(text))
                    .child(ph_icon(ph::ARROW_RIGHT, Weight::Bold, Tokens::text_3(), icon_px::XS).invisible().group_hover("suggest", |s| s.visible()))
                    .on_click(cx.listener(move |this, _, w, cx| this.send(text.to_string(), w, cx)))
            }))),
        )
    }

    // ── Composer ──

    fn attach(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Attach images".into()) });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update(cx, |this, cx| this.add_paths(paths, cx));
            }
        })
        .detach();
    }

    /// Adds image files to the message; says so when something else was dropped or picked.
    fn add_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let mut skipped = 0;
        for p in paths {
            if !is_image(&p) {
                skipped += 1;
            } else if !self.attachments.contains(&p) {
                self.attachments.push(p);
            }
        }
        if skipped > 0 {
            toast::show(cx, Toast::warning(format!("Skipped {skipped} file{}: only PNG, JPG, WEBP and GIF images can be attached", if skipped == 1 { "" } else { "s" })));
        }
        cx.notify();
    }

    fn composer(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (prefs, status) = {
            let s = self.shared.read(cx);
            (s.store.prefs.clone(), s.status(s.store.prefs.provider).cloned())
        };
        let access = self.effective_access(cx);
        // Stop belongs to the chat whose reply is running; another chat only waits for it.
        let running = self.running_here(cx);
        let busy_elsewhere = self.run.is_some() && !running;
        let empty = self.input.read(cx).value().trim().is_empty();
        let focused = self.input.read(cx).focus_handle(cx).is_focused(window);
        // Esc stops the reply only while the keyboard is in the dock, so the hint says so only then.
        let dock_focused = self.focus.contains_focused(window, cx);
        let weak = cx.weak_entity();

        // Effort: hidden for providers that ignore it.
        let effort = effort_supported(prefs.provider).then(|| {
            let items: Vec<MenuItem> = std::iter::once(MenuItem::heading("Reasoning effort"))
                .chain([(AiEffort::Low, "Low · fastest replies", ph::LIGHTNING), (AiEffort::Medium, "Medium · balanced", ph::GAUGE), (AiEffort::High, "High · thinks longest", ph::BRAIN)].into_iter().map(|(e, label, icon)| {
                    let w = weak.clone();
                    MenuItem::new(label, move |_, cx| {
                        let _ = w.update(cx, |this, cx| this.set_pref(|p| p.effort = e, cx));
                    })
                    .icon(icon)
                    .checked(prefs.effort == e)
                }))
                .collect();
            menu_button("chip-effort", ph::BRAIN, "Reasoning effort", items).label(effort_label(prefs.effort)).small().min_width(220.).anchor(Anchor::BottomLeft)
        });
        let access_items: Vec<MenuItem> = std::iter::once(MenuItem::heading("Before the assistant acts"))
            .chain(
                [(AiAccessMode::Ask, "Ask: approve every action", ph::LOCK_SIMPLE), (AiAccessMode::Auto, "Auto: only risky actions ask", ph::SHIELD_CHECK), (AiAccessMode::Full, "Full: nothing asks", ph::SHIELD_SLASH)].into_iter().map(|(m, label, icon)| {
                    let w = weak.clone();
                    MenuItem::new(label, move |_, cx| {
                        let _ = w.update(cx, |this, cx| {
                            // Picking a mode replaces this chat's "Always allow" too.
                            if let Some(id) = this.shared.read(cx).store.active.clone() {
                                this.thread_access.remove(&id);
                            }
                            this.set_pref(|p| p.access = m, cx);
                        });
                    })
                    .icon(icon)
                    .checked(access == m)
                }),
            )
            .collect();
        let (access_label, access_icon) = match access {
            AiAccessMode::Ask => ("Ask", ph::LOCK_SIMPLE),
            AiAccessMode::Auto => ("Auto", ph::SHIELD_CHECK),
            AiAccessMode::Full => ("Full access", ph::SHIELD_SLASH),
        };
        // Full access is flagged in warning colour so "nothing asks" is never invisible.
        let access_chip = menu_button("chip-access", access_icon, "Who approves actions", access_items).label(access_label).small().min_width(250.).anchor(Anchor::BottomLeft).when(access == AiAccessMode::Full, |b| b.text_color(Tokens::warning()));

        let attach = icon_button("dock-attach", ph::PAPERCLIP, "Attach images").small().on_click(cx.listener(|this, _, _, cx| this.attach(cx)));

        let blocked = if busy_elsewhere { Some(self.busy_reason(cx)) } else { self.send_blocked(cx) };
        let can_send = (!empty || !self.attachments.is_empty()) && blocked.is_none();
        let send = div()
            .id("dock-send")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(28.))
            .rounded_full()
            .role(Role::Button)
            .map(|d| {
                if running {
                    d.bg(Tokens::accent()).hover(|s| s.bg(Tokens::accent_hover())).pressable().active(|s| s.bg(Tokens::accent_pressed())).tooltip(tip_kbd("Stop", "escape")).aria_label("Stop")
                } else if can_send {
                    d.bg(Tokens::accent()).hover(|s| s.bg(Tokens::accent_hover())).pressable().active(|s| s.bg(Tokens::accent_pressed())).tooltip(tip_kbd("Send", "enter")).aria_label("Send")
                } else {
                    d.bg(Tokens::active()).cursor_not_allowed().tooltip(tip(blocked.clone().unwrap_or_else(|| "Type a message to send".into()))).aria_label("Send")
                }
            })
            .child(if running { ph_icon(ph::STOP, Weight::Fill, Tokens::on_accent(), icon_px::XS) } else { ph_icon(ph::ARROW_UP, Weight::Bold, if can_send { Tokens::on_accent() } else { Tokens::text_disabled() }, icon_px::SM) })
            .when(running || can_send, |d| d.on_click(cx.listener(move |this, _, w, cx| if this.running_here(cx) { this.stop(cx) } else { this.send_from_composer(w, cx) })));

        // A provider that can't answer gets a fix next to the box, not a failed message later.
        let alert = status.filter(|s| s.state != AiProviderState::Ready && !prefs.model.is_empty()).map(|s| {
            let recheck = matches!(s.state, AiProviderState::Offline | AiProviderState::Error);
            let body = s.detail.clone().unwrap_or_default();
            let a = inline_alert(AlertKind::Warning, not_ready_title(&s.label, s.state), body);
            if recheck {
                a.action("Re-check", cx.listener(|this, _, _, cx| this.shared.update(cx, |s, cx| s.refresh_providers(true, cx))))
            } else {
                a.action("Fix in Settings", |_, w, cx| mv_ui::navigate(Route::Settings { tab: Some("providers".into()) }, w, cx))
            }
        });

        let thumbs = self.attachments.iter().enumerate().map(|(i, p)| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("image").to_string();
            div()
                .id(("att", i))
                .relative()
                .flex_none()
                .size(px(52.))
                .rounded(px(radius::MD))
                .overflow_hidden()
                .border_1()
                .border_color(Tokens::border())
                .bg(Tokens::surface_2())
                .tooltip(tip(name))
                .child(img(p.clone()).size_full().object_fit(ObjectFit::Cover).with_fallback(|| div().size_full().flex().items_center().justify_center().child(ph_icon(glyphs::IMAGE, Weight::Duotone, Tokens::text_3(), icon_px::LG)).into_any_element()))
                .child(
                    div()
                        .id(("att-x", i))
                        .absolute()
                        .top(px(3.))
                        .right(px(3.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(20.))
                        .rounded_full()
                        .bg(Tokens::scrim())
                        .role(Role::Button)
                        .aria_label("Remove image")
                        .hover(|s| s.bg(Tokens::overlay()))
                        .pressable()
                        .tooltip(tip("Remove"))
                        .child(ph_icon(ph::X, Weight::Bold, Tokens::text_1(), icon_px::XS))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if i < this.attachments.len() {
                                this.attachments.remove(i);
                            }
                            cx.notify();
                        })),
                )
        });

        let paste_target = weak.clone();
        let textarea = Textarea::new(&self.input).appearance(false).bordered(false).on_paste(move |item, _, cx| {
            let paths = pasted_images(item);
            if paths.is_empty() {
                return false;
            }
            let _ = paste_target.update(cx, |this, cx| this.add_paths(paths, cx));
            true
        });

        let hints = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_end()
            .gap(px(5.))
            .h(px(22.))
            .px(px(4.))
            .text_size(px(size::XS))
            .text_color(Tokens::text_3())
            .map(|d| if running { d.child(kbd("escape")).child("to stop") } else { d.child(kbd("enter")).child("to send").child(div().w(px(6.))).child(kbd("shift-enter")).child("for a new line") })
            // Reserved so the list doesn't jump when the box gains focus.
            .when(!(focused || (running && dock_focused)), |d| d.invisible());

        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(6.))
            .px(px(12.))
            .pt(px(6.))
            .pb(px(4.))
            .children(alert)
            .child(
                div()
                    .id("composer")
                    .flex()
                    .flex_col()
                    .rounded(px(radius::XL))
                    .border_1()
                    .border_color(if focused { Tokens::accent_border() } else { Tokens::border() })
                    .bg(Tokens::surface())
                    .drag_over::<ExternalPaths>(|s, _, _, _| s.border_color(Tokens::accent()).bg(Tokens::accent_soft()))
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.add_paths(paths.paths().to_vec(), cx)))
                    // A click anywhere on the box (padding, toolbar gaps) puts the caret in the text.
                    .on_click(cx.listener(|this, _, window, cx| {
                        if !this.input.read(cx).focus_handle(cx).is_focused(window) {
                            this.input.update(cx, |s, cx| s.focus(window, cx));
                        }
                    }))
                    .when(!self.attachments.is_empty(), |d| d.child(div().flex().flex_wrap().gap(px(8.)).px(px(10.)).pt(px(10.)).children(thumbs)))
                    .child(div().px(px(8.)).pt(px(8.)).child(textarea))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .px(px(6.))
                            .pb(px(6.))
                            .pt(px(4.))
                            .child(self.picker.clone())
                            .children(effort)
                            .child(access_chip)
                            .child(div().flex_1())
                            .child(attach)
                            .child(div().w(px(4.)))
                            .child(send),
                    ),
            )
            .child(hints)
    }
}

fn push_activity(item: &mut ChatItem, line: String) {
    item.activity.push(line);
    if item.activity.len() > 12 {
        item.activity.remove(0);
    }
}

fn is_image(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str())).unwrap_or(false)
}

/// Images on the clipboard: copied files, or a screenshot saved under `~/.mangavox/assistant/attachments`.
fn pasted_images(item: &ClipboardItem) -> Vec<PathBuf> {
    let mut out = vec![];
    for e in item.entries() {
        match e {
            ClipboardEntry::ExternalPaths(ps) => out.extend(ps.paths().iter().filter(|p| is_image(p)).cloned()),
            ClipboardEntry::Image(image) => {
                let ext = match image.format {
                    ImageFormat::Png => "png",
                    ImageFormat::Jpeg => "jpg",
                    ImageFormat::Webp => "webp",
                    ImageFormat::Gif => "gif",
                    _ => continue,
                };
                let dir = mv_core::paths::root().join("assistant").join("attachments");
                let path = dir.join(format!("paste-{}-{:x}.{ext}", now_ms(), image.id));
                if std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&path, &image.bytes).is_ok() {
                    out.push(path);
                }
            }
            _ => {}
        }
    }
    out
}

/// Providers whose requests carry the effort setting (others ignore it, so the chip hides).
fn effort_supported(p: AiProviderId) -> bool {
    matches!(p, AiProviderId::Anthropic | AiProviderId::ClaudeCode | AiProviderId::Codex | AiProviderId::Opencode | AiProviderId::Antigravity | AiProviderId::Gemini | AiProviderId::Openai | AiProviderId::Openrouter)
}

fn effort_label(e: AiEffort) -> &'static str {
    match e {
        AiEffort::Low => "Low",
        AiEffort::Medium => "Medium",
        AiEffort::High => "High",
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        s.chars().take(max).collect::<String>() + "…"
    } else {
        s.to_string()
    }
}

/// Plain-language title and next step for a provider error (NN/g error-message guidelines). The
/// raw text stays available under Details.
pub fn friendly_error(provider: &str, raw: &str) -> (String, String) {
    let e = raw.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| e.contains(w));
    if has(&["connection refused", "error sending request", "tcp connect", "dns error", "failed to connect", "could not connect", "connect error", "os error 10061"]) {
        (format!("Couldn't reach {provider}"), format!("Is {provider} running? Check Settings → AI providers, then retry."))
    } else if has(&["timed out", "timeout"]) {
        (format!("{provider} took too long to answer"), "Retry, or pick a smaller or faster model.".into())
    } else if has(&["401", "403", "unauthorized", "invalid api key", "invalid x-api-key", "incorrect api key", "authentication", "permission denied"]) {
        (format!("{provider} didn't accept the API key"), "Add a valid key in Settings → AI providers.".into())
    } else if has(&["429", "rate limit", "rate_limit", "quota", "too many requests", "overloaded"]) {
        (format!("{provider} is busy or rate-limiting"), "Wait a moment, then retry.".into())
    } else if has(&["does not support tools", "tool use is not supported", "tools are not supported"]) {
        ("This model can't use app tools".into(), "Pick a model with tool support, or ask without actions.".into())
    } else if has(&["context length", "context_length", "too many tokens", "maximum context", "prompt is too long"]) {
        ("This chat is too long for the model".into(), "Start a new chat, or pick a model with a larger context.".into())
    } else if has(&["model not found", "no such model", "unknown model", "404"]) {
        ("Model not found".into(), "Pick another model below the message box.".into())
    } else {
        ("Something went wrong".into(), truncate_chars(raw.trim(), 160))
    }
}

fn tool_glyph(name: &str) -> Ph {
    match name {
        "search_anime" | "search_movies" => glyphs::SEARCH,
        "navigate" | "go_back" | "set_category" => ph::COMPASS,
        "open_anime" => glyphs::ANIME,
        "open_movie" => glyphs::MOVIES,
        "open_manga" => glyphs::MANGA,
        "play_title" => glyphs::PLAY,
        "open_settings" => glyphs::SETTINGS,
        "list_library" => glyphs::LIBRARY,
        "get_watch_history" => glyphs::HISTORY,
        "list_themes" | "set_theme" => ph::PALETTE,
        "get_app_state" => ph::EYE,
        _ => ph::WRENCH,
    }
}

fn lower_first(s: &str) -> String {
    let mut cs = s.chars();
    cs.next().map(|f| f.to_lowercase().collect::<String>() + cs.as_str()).unwrap_or_default()
}

/// One tool call: its icon (or a spinner while it runs), a label in the right tense, and its
/// result behind a caret when there is one.
fn step_row(st: &ToolStep, key: String, open: bool, cx: &mut Context<Dock>) -> Stateful<Div> {
    let args = st.args.as_object().cloned().unwrap_or_default();
    let p = tools::phrases(&st.name, &args);
    let label = match st.state {
        StepState::Running => format!("{}…", p.doing),
        StepState::AwaitingApproval => format!("{} · waiting for your OK", p.ask),
        StepState::Done => p.done,
        StepState::Denied => p.ask,
        StepState::Error => format!("Couldn't {}", lower_first(&p.ask)),
    };
    let lead: AnyElement = match st.state {
        StepState::Running => spinner(SharedString::from(format!("step-spin-{}", st.id)), Tokens::accent(), icon_px::XS).into_any_element(),
        StepState::AwaitingApproval => ph_icon(ph::SHIELD_WARNING, Weight::Fill, Tokens::warning(), icon_px::XS).into_any_element(),
        StepState::Error => ph_icon(ph::WARNING_CIRCLE, Weight::Fill, Tokens::danger(), icon_px::XS).into_any_element(),
        StepState::Denied => ph_icon(ph::PROHIBIT, Weight::Bold, Tokens::text_3(), icon_px::XS).into_any_element(),
        StepState::Done => ph_icon(tool_glyph(&st.name), Weight::Bold, Tokens::text_3(), icon_px::XS).into_any_element(),
    };
    let result = st.result.clone().filter(|r| !r.trim().is_empty());
    let has_result = result.is_some();
    let k = key.clone();
    div()
        .id(SharedString::from(format!("step-{key}")))
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .id("step-head")
                .group("step-head")
                .flex()
                .items_center()
                .gap(px(7.))
                .h(px(24.))
                .px(px(6.))
                .ml(px(-6.))
                .rounded(px(radius::SM))
                .text_size(px(size::SM))
                .text_color(if st.state == StepState::Error { Tokens::danger() } else { Tokens::text_3() })
                // Clickable only when there is a result to show (no false affordance).
                .when(has_result, |d| {
                    d.role(Role::Button).aria_expanded(open).hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1())).pressable().on_click(cx.listener(move |this, _, _, cx| {
                        this.toggled.insert(k.clone(), !open);
                        cx.notify();
                    }))
                })
                .child(lead)
                .child(div().flex_1().min_w_0().truncate().child(label))
                .when(st.state == StepState::Denied, |d| d.child(tag("Declined", Tone::Neutral)))
                .when(st.state == StepState::Done, |d| d.child(ph_icon(ph::CHECK_CIRCLE, Weight::Fill, Tokens::success(), icon_px::XS)))
                .when(has_result, |d| d.child(ph_icon(if open { ph::CARET_DOWN } else { ph::CARET_RIGHT }, Weight::Bold, Tokens::text_3(), icon_px::XS).group_hover("step-head", |s| s.text_color(Tokens::text_1())))),
        )
        .when_some(result.filter(|_| open), |d, r| {
            d.child(
                div()
                    .ml(px(6.))
                    .pl(px(12.))
                    .border_l_1()
                    .border_color(Tokens::border())
                    .font_family(MONO_FONT)
                    .text_size(px(size::XS))
                    .text_color(if st.state == StepState::Error { Tokens::danger() } else { Tokens::text_3() })
                    .child(truncate_chars(&r, 800)),
            )
        })
}

/// Starter prompts that fit the page the user is on, each with a glyph for what it does.
pub fn suggestions(page: Option<&str>) -> Vec<(Ph, &'static str)> {
    match page {
        Some("library") | Some("browse") | Some("history") | Some("manga-detail") | Some("reader") => {
            vec![(glyphs::LIBRARY, "What manga have I not finished?"), (glyphs::HISTORY, "Open my reading history"), (glyphs::BROWSE, "Take me to Browse")]
        }
        Some("anime") | Some("anime-detail") => vec![(glyphs::SEARCH, "Search for a popular action anime"), (glyphs::HISTORY, "What did I watch recently?"), (glyphs::DISCOVER, "Open the Discover page")],
        Some("movies") | Some("movie-detail") => vec![(glyphs::SEARCH, "Find a good sci-fi movie"), (glyphs::HISTORY, "What did I watch recently?"), (glyphs::FAVORITES, "Open my favourites")],
        Some("discover") | Some("favorites") | Some("media-history") => vec![(glyphs::HISTORY, "What did I watch recently?"), (glyphs::SEARCH, "Find something like Death Note"), (glyphs::MOVIES, "Switch to movies")],
        Some("settings") => vec![(glyphs::SETTINGS, "Open the AI providers settings"), (glyphs::DOWNLOAD, "Where can I download local models?"), (glyphs::BACK, "Take me back")],
        _ => vec![(glyphs::ASSISTANT, "What can you do in this app?"), (glyphs::LIBRARY, "Show me my library"), (glyphs::HISTORY, "What did I watch recently?")],
    }
}

impl Render for Dock {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, items) = {
            let s = self.shared.read(cx);
            match s.store.active_thread() {
                Some(t) => (t.title.clone(), t.items.clone()),
                None => ("Assistant".to_string(), vec![]),
            }
        };
        // Back at the bottom after scrolling down (or nothing left to scroll): follow new output again.
        // Right at the bottom counts too, however it got there (scrollbar drag, keyboard), so the
        // "Jump to latest" button never sits over a list that is already at its end.
        if !self.follow {
            let (off, max) = (self.scroll.offset().y, self.scroll.max_offset().y);
            if max <= px(1.) || -off >= max - px(1.) || (self.wheel_down && -off >= max - px(24.)) {
                self.follow = true;
            }
        }
        let n = items.len();
        let last_user = items.iter().rposition(|i| i.role == ItemRole::User);
        let list = if items.is_empty() {
            div().flex_1().min_h_0().child(self.empty_view(cx)).into_any_element()
        } else {
            let mut col = div()
                .id("dock-messages")
                .flex()
                .flex_col()
                .gap(px(18.))
                .px(px(14.))
                .py(px(14.))
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| {
                    // Scrolling up to reread stops the auto-follow (stick-to-bottom).
                    let dy = ev.delta.pixel_delta(px(20.)).y;
                    if dy > px(0.) {
                        this.follow = false;
                        this.wheel_down = false;
                    } else if dy < px(0.) {
                        this.wheel_down = true;
                    }
                    cx.notify();
                }));
            for (i, item) in items.iter().enumerate() {
                col = col.child(self.render_item(item, i, i + 1 == n, Some(i) == last_user, cx));
            }
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(col)
                .when(!self.follow, |d| {
                    d.child(
                        div().absolute().bottom(px(12.)).left_0().right_0().flex().justify_center().child(
                            icon_button("dock-jump", ph::ARROW_DOWN, "Jump to latest")
                                .small()
                                .rounded_full()
                                .bg(Tokens::surface_3())
                                .border_1()
                                .border_color(Tokens::border_strong())
                                .shadow_md()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.follow = true;
                                    this.scroll.scroll_to_bottom();
                                    cx.notify();
                                })),
                        ),
                    )
                })
                .into_any_element()
        };
        let drawer = self.drawer.then(|| self.drawer_view(cx));
        div()
            .id("assistant-dock")
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(Tokens::base())
            .track_focus(&self.focus)
            .key_context("AssistantDock")
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)))
            .child(self.header(title, cx))
            .child(list)
            .child(self.composer(window, cx))
            .children(drawer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn errors_read_as_plain_language() {
        assert_eq!(friendly_error("Ollama", "error sending request for url (http://127.0.0.1:11434/api/chat): tcp connect error").0, "Couldn't reach Ollama");
        assert_eq!(friendly_error("OpenAI", "HTTP 401: Incorrect API key provided").0, "OpenAI didn't accept the API key");
        assert_eq!(friendly_error("Groq", "429 Too Many Requests").0, "Groq is busy or rate-limiting");
        assert_eq!(friendly_error("Ollama", "registry.ollama.ai/x does not support tools").0, "This model can't use app tools");
        let (t, hint) = friendly_error("X", "weird failure");
        assert_eq!(t, "Something went wrong");
        assert_eq!(hint, "weird failure");
    }

    #[::core::prelude::v1::test]
    fn attachments_accept_images_only() {
        assert!(is_image(Path::new("a/b.PNG")));
        assert!(is_image(Path::new("c.webp")));
        assert!(!is_image(Path::new("notes.txt")));
        assert!(!is_image(Path::new("noext")));
    }

    #[::core::prelude::v1::test]
    fn effort_chip_only_where_it_does_something() {
        assert!(effort_supported(AiProviderId::Anthropic));
        assert!(!effort_supported(AiProviderId::Ollama));
        assert_eq!(effort_label(AiEffort::High), "High");
    }

    #[::core::prelude::v1::test]
    fn every_page_has_suggestions() {
        for page in [None, Some("library"), Some("anime"), Some("movies"), Some("discover"), Some("settings")] {
            assert_eq!(suggestions(page).len(), 3);
        }
    }
}
