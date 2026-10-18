//! The assistant dock: header, thread drawer, message list, approvals and composer.
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::channel::{mpsc, oneshot};
use futures::StreamExt;
use gpui_kit::component::input::{InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ai::{provider_def, AiAccessMode, AiEffort, AiMessage, AiModel, AiProviderId, AiProviderSettings, AiProviderState, AiRole, CancellationToken, StreamEvent};
use mv_ui::{icon, icon_button, size, Route, Tokens};

use crate::agent::{self, AgentEvent, AgentParams, Outcome};
use crate::shared::{shared, Shared};
use crate::store::{model_key, now_ms, title_from, ChatItem, ItemRole, ItemStatus, StepState, ToolStep};
use crate::tools::{self, AppTools};
use crate::ui::{ago_ms, field, menu_row, monogram, muted, popup_surface, set_value, text_state, value_of};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Menu {
    Effort,
    Access,
    Model,
}

struct Run {
    cancel: CancellationToken,
    access: Arc<Mutex<AiAccessMode>>,
    approvals: HashMap<String, oneshot::Sender<bool>>,
    _task: Task<()>,
}

struct Rename {
    id: String,
    input: Entity<InputState>,
}

pub struct Dock {
    shared: Entity<Shared>,
    input: Entity<TextareaState>,
    search: Entity<InputState>,
    picker_search: Entity<InputState>,
    custom_model: Entity<InputState>,
    rename: Option<Rename>,
    confirm_delete: Option<String>,
    drawer: bool,
    menu: Option<Menu>,
    /// `None` shows favourites across all providers.
    picker_provider: Option<AiProviderId>,
    scroll: ScrollHandle,
    expanded: HashSet<String>,
    run: Option<Run>,
    _subs: Vec<Subscription>,
}

pub fn create(window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| Dock::new(window, cx)).into()
}

impl Dock {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shared = shared(cx);
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 6).submit_on_enter(true).placeholder("Ask anything, or tell me what to do…"));
        let search = text_state("Search chats", window, cx);
        let picker_search = text_state("Search models", window, cx);
        let custom_model = text_state("Custom model id", window, cx);
        let mut subs = vec![];
        subs.push(cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { shift: false, .. } = ev {
                this.send_from_composer(window, cx);
            }
        }));
        subs.push(cx.observe(&search, |_, _, cx| cx.notify()));
        subs.push(cx.observe(&picker_search, |_, _, cx| cx.notify()));
        subs.push(cx.observe(&shared, |this, _, cx| {
            this.autopick(cx);
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
            input,
            search,
            picker_search,
            custom_model,
            rename: None,
            confirm_delete: None,
            drawer: false,
            menu: None,
            picker_provider: None,
            scroll: ScrollHandle::new(),
            expanded: HashSet::new(),
            run: None,
            _subs: subs,
        }
    }

    /// First launch: choose a ready provider and its default model.
    fn autopick(&mut self, cx: &mut Context<Self>) {
        self.shared.update(cx, |s, cx| {
            if !s.store.prefs.model.is_empty() || !s.providers_loaded {
                return;
            }
            let ready = |p: &&mv_ai::AiProviderStatus| p.state == AiProviderState::Ready;
            let cand = s.providers.iter().filter(ready).find(|p| p.id == s.store.prefs.provider).or_else(|| s.providers.iter().find(ready)).map(|p| (p.id, p.default_model.clone()));
            if let Some((id, default)) = cand {
                let first = s.models.get(&id).and_then(|m| m.first()).map(|m| m.id.clone());
                match default.or(first) {
                    Some(m) => s.use_in_assistant(id, &m, cx),
                    None => s.load_models(id, false, cx),
                }
            }
        });
    }

    // ── Sending ──────────────────────────────────────────────────────────────

    fn send_from_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() || self.run.is_some() {
            return;
        }
        self.input.update(cx, |s, cx| s.set_value("", window, cx));
        self.send(text, window, cx);
    }

    fn send(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = text.trim().to_string();
        if prompt.is_empty() || self.run.is_some() {
            return;
        }
        self.menu = None;
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
        if prefs.model.is_empty() {
            self.finish_item(&thread_id, &item_id, ItemStatus::Error, Some("Pick a model first — use the model button below the message box.".into()), None, cx);
            return;
        }
        self.scroll.scroll_to_bottom();

        let def = provider_def(prefs.provider);
        let access = Arc::new(Mutex::new(prefs.access));
        let cancel = CancellationToken::new();
        let (tx, mut rx) = mpsc::unbounded::<AgentEvent>();
        let registry = self.shared.read(cx).registry.clone();
        let params = AgentParams { provider: prefs.provider, model: prefs.model.clone(), effort: prefs.effort, access: access.clone(), system: String::new(), history, tools_enabled: def.supports_tools };
        let token = cancel.clone();
        mv_ui::bg::runtime().spawn(async move {
            let mut params = params;
            let (page, section) = tools::page_context().map(|r| (tools::page_name(&r), tools::section_name(&r))).unzip();
            let library = tokio::task::spawn_blocking(|| mv_core::library::open_readonly().and_then(|c| mv_core::library::list_manga(&c)).map(|m| m.len()).ok()).await.ok().flatten();
            params.system = agent::system_prompt(params.tools_enabled, page, section, library, &agent::today_string());
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
        self.run = Some(Run { cancel, access, approvals: HashMap::new(), _task: task });
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
        let (tid, iid) = (thread_id.to_string(), item_id.to_string());
        self.shared.update(cx, |s, cx| {
            let Some(item) = s.store.thread_mut(&tid).and_then(|t| t.items.iter_mut().find(|i| i.id == iid)) else { return };
            for ev in batch {
                match ev {
                    AgentEvent::Delta(StreamEvent::Text { delta }) => item.text.push_str(&delta),
                    AgentEvent::Delta(StreamEvent::Reasoning { delta }) => item.reasoning.get_or_insert_with(String::new).push_str(&delta),
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
                    AgentEvent::Finished { outcome, history } => finished = Some((outcome, history)),
                }
            }
            cx.notify();
        });
        if let Some(run) = self.run.as_mut() {
            for (id, reply) in approvals {
                run.approvals.insert(id, reply);
            }
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
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = self.run.as_mut() {
            run.cancel.cancel();
            run.approvals.clear();
        }
        cx.notify();
    }

    fn decide(&mut self, step_id: &str, ok: bool, cx: &mut Context<Self>) {
        if let Some(run) = self.run.as_mut() {
            if let Some(reply) = run.approvals.remove(step_id) {
                let _ = reply.send(ok);
            }
        }
        cx.notify();
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
        self.menu = None;
        self.input.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    fn select_thread(&mut self, id: String, cx: &mut Context<Self>) {
        if self.run.is_some() {
            return;
        }
        self.shared.update(cx, |s, cx| {
            s.store.active = Some(id);
            s.save();
            cx.notify();
        });
        self.drawer = false;
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn set_pref(&mut self, f: impl FnOnce(&mut crate::store::Prefs), cx: &mut Context<Self>) {
        self.shared.update(cx, |s, cx| {
            f(&mut s.store.prefs);
            s.save();
            cx.notify();
        });
        // A running turn picks up access changes immediately.
        let access = self.shared.read(cx).store.prefs.access;
        if let Some(run) = &self.run {
            *run.access.lock().unwrap_or_else(|e| e.into_inner()) = access;
        }
    }

    fn pick_model(&mut self, provider: AiProviderId, model: String, cx: &mut Context<Self>) {
        self.shared.update(cx, |s, cx| s.use_in_assistant(provider, &model, cx));
        self.menu = None;
        cx.notify();
    }

    fn add_custom_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = value_of(&self.custom_model, cx).trim().to_string();
        let Some(provider) = self.picker_provider.or(Some(self.shared.read(cx).store.prefs.provider)) else { return };
        if id.is_empty() {
            return;
        }
        let reg = self.shared.read(cx).registry.clone();
        let mut extra = reg.read_settings(provider).extra_models.unwrap_or_default();
        if !extra.contains(&id) {
            extra.push(id.clone());
        }
        let _ = reg.update_provider_settings(provider, &AiProviderSettings { extra_models: Some(extra), ..Default::default() });
        set_value(&self.custom_model, "", window, cx);
        self.shared.update(cx, |s, cx| {
            s.models.remove(&provider);
            s.load_models(provider, true, cx);
        });
        self.pick_model(provider, id, cx);
    }

    // ── Rendering ────────────────────────────────────────────────────────────

    fn header(&self, title: String, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(44.))
            .px(px(8.))
            .flex_none()
            .border_b_1()
            .border_color(Tokens::border())
            .child(icon_button("dock-chats", "icons/messages-square.svg", true).on_click(cx.listener(|this, _, _, cx| {
                this.drawer = !this.drawer;
                this.menu = None;
                cx.notify();
            })))
            .child(div().flex_1().min_w_0().truncate().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(title))
            .child(icon_button("dock-new", "icons/plus.svg", self.run.is_none()).on_click(cx.listener(|this, _, w, cx| this.new_chat(w, cx))))
    }

    fn drawer_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let q = value_of(&self.search, cx);
        let (rows, active): (Vec<(String, String, u64)>, Option<String>) = {
            let s = self.shared.read(cx);
            (s.store.search(&q).into_iter().map(|t| (t.id.clone(), t.title.clone(), t.updated_at)).collect(), s.store.active.clone())
        };
        let mut list = div().id("dock-thread-list").flex().flex_col().gap(px(2.)).flex_1().min_h_0().overflow_y_scroll().p(px(6.));
        if rows.is_empty() {
            list = list.child(div().p(px(12.)).child(muted(if q.is_empty() { "No chats yet." } else { "No matching chats." })));
        }
        for (id, title, updated) in rows {
            let is_active = active.as_deref() == Some(id.as_str());
            let renaming = self.rename.as_ref().filter(|r| r.id == id).map(|r| r.input.clone());
            let armed = self.confirm_delete.as_deref() == Some(id.as_str());
            let (id_sel, id_ren, id_del) = (id.clone(), id.clone(), id.clone());
            let row = div()
                .id(SharedString::from(format!("thread-{id}")))
                .group("thread-row")
                .flex()
                .items_center()
                .gap(px(6.))
                .h(px(34.))
                .px(px(10.))
                .rounded(px(8.))
                .cursor_pointer()
                .when(is_active, |d| d.bg(Tokens::active()))
                .hover(|s| s.bg(Tokens::hover()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.rename.is_none() {
                        this.select_thread(id_sel.clone(), cx);
                    }
                }));
            let row = match renaming {
                Some(input) => row.child(div().flex_1().child(field(&input).small())),
                None => row
                    .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).text_color(if is_active { Tokens::text_1() } else { Tokens::text_2() }).child(title.clone()))
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).group_hover("thread-row", |s| s.invisible()).child(ago_ms(updated)))
                    .child(
                        div()
                            .absolute()
                            .right(px(4.))
                            .flex()
                            .items_center()
                            .invisible()
                            .group_hover("thread-row", |s| s.visible())
                            .child(icon_button(SharedString::from(format!("ren-{id}")), "icons/pencil.svg", true).size(px(26.)).on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                let current = this.shared.read(cx).store.thread(&id_ren).map(|t| t.title.clone()).unwrap_or_default();
                                let input = text_state("Chat name", window, cx);
                                set_value(&input, &current, window, cx);
                                let rid = id_ren.clone();
                                let sub = cx.subscribe_in(&input, window, move |this, input, ev: &InputEvent, _w, cx| match ev {
                                    InputEvent::PressEnter { .. } | InputEvent::Blur => {
                                        let name = value_of(input, cx);
                                        let rid = rid.clone();
                                        this.shared.update(cx, |s, cx| {
                                            s.store.rename_thread(&rid, &name);
                                            s.save();
                                            cx.notify();
                                        });
                                        this.rename = None;
                                        cx.notify();
                                    }
                                    _ => {}
                                });
                                this._subs.push(sub);
                                input.update(cx, |s, cx| s.focus(window, cx));
                                this.rename = Some(Rename { id: id_ren.clone(), input });
                                cx.notify();
                            })))
                            .child(
                                icon_button(SharedString::from(format!("del-{id}")), "icons/trash.svg", true)
                                    .size(px(26.))
                                    .when(armed, |d| d.bg(rgba(0xf8717133)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        if this.confirm_delete.as_deref() == Some(id_del.as_str()) {
                                            let id = id_del.clone();
                                            this.shared.update(cx, |s, cx| {
                                                s.store.delete_thread(&id);
                                                s.save();
                                                cx.notify();
                                            });
                                            this.confirm_delete = None;
                                        } else {
                                            this.confirm_delete = Some(id_del.clone());
                                        }
                                        cx.notify();
                                    })),
                            ),
                    ),
            };
            list = list.child(row);
        }
        div()
            .absolute()
            .top_0()
            .left_0()
            .bottom_0()
            .w(px(280.))
            .flex()
            .flex_col()
            .bg(Tokens::sidebar())
            .border_r_1()
            .border_color(Tokens::border())
            .occlude()
            .child(div().p(px(8.)).child(field(&self.search).small().cleanable(true)))
            .child(list)
    }

    fn render_item(&self, item: &ChatItem, last: bool, cx: &mut Context<Self>) -> AnyElement {
        match item.role {
            ItemRole::User => div()
                .flex()
                .justify_end()
                .child(
                    div()
                        .max_w(relative(0.88))
                        .px(px(12.))
                        .py(px(8.))
                        .rounded(px(14.))
                        .bg(Tokens::surface_2())
                        .text_size(px(size::MD))
                        .text_color(Tokens::text_1())
                        .child(item.text.clone()),
                )
                .into_any_element(),
            ItemRole::Assistant => self.render_assistant(item, last, cx),
        }
    }

    fn collapsible(&self, key: String, title: String, open: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let k = key.clone();
        div()
            .id(SharedString::from(format!("hdr-{key}")))
            .flex()
            .items_center()
            .gap(px(5.))
            .cursor_pointer()
            .text_size(px(size::SM))
            .text_color(Tokens::text_3())
            .hover(|s| s.text_color(Tokens::text_1()))
            .child(icon(if open { "icons/chevron-down.svg" } else { "icons/chevron-right.svg" }, Tokens::text_3(), 13.))
            .child(title)
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.expanded.remove(&k) {
                    this.expanded.insert(k.clone());
                }
                cx.notify();
            }))
    }

    fn render_assistant(&self, item: &ChatItem, last: bool, cx: &mut Context<Self>) -> AnyElement {
        let streaming = item.status == ItemStatus::Streaming;
        let mut col = div().flex().flex_col().gap(px(8.)).w_full();

        if let Some(reasoning) = item.reasoning.as_ref().filter(|r| !r.trim().is_empty()) {
            let key = format!("thought:{}", item.id);
            let open = self.expanded.contains(&key);
            let title = if streaming && item.text.is_empty() { "Thinking…" } else { "Thought process" };
            col = col.child(self.collapsible(key, title.into(), open, cx));
            if open {
                col = col.child(div().pl(px(8.)).border_l_1().border_color(Tokens::border()).text_size(px(size::SM)).text_color(Tokens::text_3()).child(reasoning.trim().to_string()));
            }
        }

        if !item.activity.is_empty() {
            let key = format!("activity:{}", item.id);
            let open = streaming != self.expanded.contains(&key);
            col = col.child(self.collapsible(key, format!("Activity · {}", item.activity.len()), open, cx));
            if open {
                col = col.child(
                    div().flex().flex_col().gap(px(2.)).pl(px(8.)).border_l_1().border_color(Tokens::border()).children(
                        item.activity.iter().rev().take(8).rev().map(|a| div().flex().gap(px(6.)).items_center().text_size(px(size::XS)).text_color(Tokens::text_3()).child(icon("icons/terminal.svg", Tokens::text_3(), 11.)).child(div().truncate().child(a.clone()))),
                    ),
                );
            }
        }

        if !item.steps.is_empty() {
            let key = format!("steps:{}", item.id);
            let n = item.steps.len();
            let running = item.steps.iter().any(|s| matches!(s.state, StepState::Running | StepState::AwaitingApproval));
            let open = self.expanded.contains(&key);
            let title = if running { "Working…".to_string() } else { format!("Ran {n} action{}", if n == 1 { "" } else { "s" }) };
            col = col.child(self.collapsible(key, title, open, cx));
            if open {
                col = col.child(div().flex().flex_col().gap(px(4.)).pl(px(8.)).border_l_1().border_color(Tokens::border()).children(item.steps.iter().map(step_row)));
            }
            for st in item.steps.iter().filter(|s| s.state == StepState::AwaitingApproval) {
                col = col.child(self.approval_card(st, cx));
            }
        }

        if !item.text.trim().is_empty() {
            col = col.child(div().text_size(px(size::MD)).text_color(Tokens::text_1()).child(TextView::markdown(SharedString::from(format!("md-{}", item.id)), item.text.clone()).selectable(true)));
        } else if streaming && item.reasoning.is_none() && item.steps.is_empty() && item.activity.is_empty() {
            col = col.child(muted("Thinking…"));
        }

        match item.status {
            ItemStatus::Error => {
                col = col.child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(8.))
                        .p(px(10.))
                        .rounded(px(10.))
                        .bg(rgba(0xf871711a))
                        .border_1()
                        .border_color(rgba(0xf8717140))
                        .child(icon("icons/triangle-alert.svg", Tokens::danger(), 15.))
                        .child(div().flex_1().min_w_0().text_size(px(size::SM)).text_color(Tokens::text_1()).child(item.error.clone().unwrap_or_else(|| "Something went wrong.".into())))
                        .when(last && self.run.is_none(), |d| {
                            d.child(
                                div()
                                    .id("dock-retry")
                                    .flex_none()
                                    .px(px(10.))
                                    .py(px(4.))
                                    .rounded(px(7.))
                                    .cursor_pointer()
                                    .text_size(px(size::SM))
                                    .text_color(Tokens::text_1())
                                    .bg(Tokens::active())
                                    .hover(|s| s.opacity(0.85))
                                    .child("Retry")
                                    .on_click(cx.listener(|this, _, w, cx| this.retry(w, cx))),
                            )
                        }),
                );
            }
            ItemStatus::Stopped => col = col.child(muted("Stopped")),
            _ => {}
        }
        col.into_any_element()
    }

    fn approval_card(&self, st: &ToolStep, cx: &mut Context<Self>) -> impl IntoElement {
        let (allow_id, deny_id) = (st.id.clone(), st.id.clone());
        let args = match &st.args {
            serde_json::Value::Object(m) if !m.is_empty() => m.iter().map(|(k, v)| format!("{k}: {}", v.as_str().map(String::from).unwrap_or_else(|| v.to_string()))).collect::<Vec<_>>().join(", "),
            _ => String::new(),
        };
        let btn = |id: String, label: &'static str, primary: bool| {
            div()
                .id(SharedString::from(format!("{label}-{id}")))
                .flex()
                .items_center()
                .justify_center()
                .h(px(28.))
                .px(px(14.))
                .rounded(px(8.))
                .cursor_pointer()
                .text_size(px(size::SM))
                .font_weight(FontWeight::MEDIUM)
                .map(|d| if primary { d.bg(Tokens::accent()).text_color(Tokens::base()) } else { d.bg(Tokens::active()).text_color(Tokens::text_1()) })
                .hover(|s| s.opacity(0.88))
                .child(label)
        };
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(12.))
            .rounded(px(12.))
            .border_1()
            .border_color(rgba(0xfbbf2459))
            .bg(rgba(0xfbbf2412))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(icon("icons/shield-check.svg", Tokens::warning(), 15.))
                    .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(st.label.clone()))
                    .child(div().flex_1())
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(st.risk.clone())),
            )
            .when(!args.is_empty(), |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(args)))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(btn(allow_id.clone(), "Allow", true).on_click(cx.listener(move |this, _, _, cx| this.decide(&allow_id, true, cx))))
                    .child(btn(deny_id.clone(), "Deny", false).on_click(cx.listener(move |this, _, _, cx| this.decide(&deny_id, false, cx)))),
            )
    }

    fn empty_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let page = tools::page_context();
        let items = suggestions(page.as_ref().map(tools::page_name));
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(14.))
            .size_full()
            .px(px(20.))
            .child(icon("icons/sparkles.svg", Tokens::accent(), 26.))
            .child(div().text_size(px(size::XL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("How can I help?"))
            .child(muted("I can search, open pages and change settings for you."))
            .child(div().flex().flex_col().gap(px(6.)).w_full().children(items.into_iter().enumerate().map(|(i, text)| {
                div()
                    .id(("suggest", i))
                    .px(px(12.))
                    .py(px(9.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(Tokens::border())
                    .cursor_pointer()
                    .text_size(px(size::SM))
                    .text_color(Tokens::text_2())
                    .hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
                    .child(text)
                    .on_click(cx.listener(move |this, _, w, cx| this.send(text.to_string(), w, cx)))
            })))
    }

    fn toggle(view: WeakEntity<Self>, m: Menu) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        move |_, _, cx| {
            let _ = view.update(cx, |this, cx| {
                this.menu = if this.menu == Some(m) { None } else { Some(m) };
                if m == Menu::Model {
                    let p = this.shared.read(cx).store.prefs.provider;
                    this.picker_provider = Some(p);
                    this.shared.update(cx, |s, cx| {
                        s.load_models(p, false, cx);
                        s.refresh_providers(false, cx);
                    });
                }
                cx.notify();
            });
        }
    }

    fn composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (prefs, status) = {
            let s = self.shared.read(cx);
            (s.store.prefs.clone(), s.status(s.store.prefs.provider).cloned())
        };
        let running = self.run.is_some();
        let empty = self.input.read(cx).value().trim().is_empty();
        let model_label = if prefs.model.is_empty() { "Select model".to_string() } else { short_model(&prefs.model) };
        let chip = |id: &'static str, label: SharedString, active: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(4.))
                .h(px(26.))
                .px(px(8.))
                .rounded(px(8.))
                .cursor_pointer()
                .text_size(px(size::XS))
                .text_color(if active { Tokens::text_1() } else { Tokens::text_2() })
                .when(active, |d| d.bg(Tokens::active()))
                .hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
                .child(label)
        };
        let (t_model, t_effort, t_access) = (Self::toggle(cx.weak_entity(), Menu::Model), Self::toggle(cx.weak_entity(), Menu::Effort), Self::toggle(cx.weak_entity(), Menu::Access));

        let model_chip = div()
            .relative()
            .child(chip("chip-model", model_label.into(), self.menu == Some(Menu::Model)).child(icon("icons/chevron-down.svg", Tokens::text_3(), 12.)).on_click(t_model))
            .when(self.menu == Some(Menu::Model), |d| d.child(deferred(anchored().anchor(Anchor::BottomLeft).offset(point(px(0.), px(-6.))).snap_to_window_with_margin(px(8.)).child(self.model_picker(cx))).with_priority(2)));
        let effort_chip = div()
            .relative()
            .child(chip("chip-effort", prefs.effort.as_str().into(), self.menu == Some(Menu::Effort)).on_click(t_effort))
            .when(self.menu == Some(Menu::Effort), |d| {
                d.child(deferred(anchored().anchor(Anchor::BottomLeft).offset(point(px(0.), px(-6.))).snap_to_window_with_margin(px(8.)).child(self.effort_menu(prefs.effort, cx))).with_priority(2))
            });
        let access_label = match prefs.access {
            AiAccessMode::Ask => "Ask",
            AiAccessMode::Auto => "Auto",
            AiAccessMode::Full => "Full",
        };
        let access_chip = div()
            .relative()
            .child(chip("chip-access", access_label.into(), self.menu == Some(Menu::Access)).on_click(t_access))
            .when(self.menu == Some(Menu::Access), |d| {
                d.child(deferred(anchored().anchor(Anchor::BottomLeft).offset(point(px(0.), px(-6.))).snap_to_window_with_margin(px(8.)).child(self.access_menu(prefs.access, cx))).with_priority(2))
            });

        let send = div()
            .id("dock-send")
            .flex()
            .items_center()
            .justify_center()
            .flex_none()
            .size(px(28.))
            .rounded(px(14.))
            .map(|d| {
                if running {
                    d.bg(Tokens::text_1()).cursor_pointer()
                } else if empty {
                    d.bg(Tokens::active())
                } else {
                    d.bg(Tokens::accent()).cursor_pointer()
                }
            })
            .child(icon(if running { "icons/square.svg" } else { "icons/arrow-up.svg" }, if running || !empty { Tokens::base() } else { Tokens::text_3() }, 15.))
            .on_click(cx.listener(move |this, _, w, cx| if this.run.is_some() { this.stop(cx) } else { this.send_from_composer(w, cx) }));

        let provider_note = status.filter(|s| !matches!(s.state, AiProviderState::Ready)).map(|s| format!("{}: {}", s.label, crate::ui::state_text(s.state)));

        div()
            .flex_none()
            .px(px(12.))
            .pb(px(12.))
            .pt(px(6.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .rounded(px(14.))
                    .border_1()
                    .border_color(rgba(0xffffff1f))
                    .bg(Tokens::surface())
                    .child(div().px(px(8.)).pt(px(6.)).child(Textarea::new(&self.input).appearance(false).bordered(false)))
                    .child(div().flex().items_center().gap(px(2.)).px(px(6.)).pb(px(6.)).child(model_chip).child(effort_chip).child(access_chip).child(div().flex_1()).child(send)),
            )
            .when_some(provider_note, |d, n| d.child(div().pt(px(6.)).px(px(4.)).text_size(px(size::XS)).text_color(Tokens::warning()).child(n)))
    }

    fn effort_menu(&self, current: AiEffort, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = [(AiEffort::Low, "Low", "Fast, little reasoning"), (AiEffort::Medium, "Medium", "Balanced"), (AiEffort::High, "High", "Slower, thinks harder")];
        popup_surface().w(px(210.)).p(px(4.)).on_mouse_down_out(cx.listener(|this, _, _, cx| {
            this.menu = None;
            cx.notify();
        })).children(rows.into_iter().map(|(e, label, hint)| {
            menu_row(label, label, Some(hint), e == current).on_click(cx.listener(move |this, _, _, cx| {
                this.set_pref(|p| p.effort = e, cx);
                this.menu = None;
                cx.notify();
            }))
        }))
    }

    fn access_menu(&self, current: AiAccessMode, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = [
            (AiAccessMode::Ask, "Ask", "Approve every action"),
            (AiAccessMode::Auto, "Auto", "Reads and safe actions run; risky ones ask"),
            (AiAccessMode::Full, "Full", "Nothing asks"),
        ];
        popup_surface().w(px(260.)).p(px(4.)).on_mouse_down_out(cx.listener(|this, _, _, cx| {
            this.menu = None;
            cx.notify();
        })).children(rows.into_iter().map(|(m, label, hint)| {
            menu_row(label, label, Some(hint), m == current).on_click(cx.listener(move |this, _, _, cx| {
                this.set_pref(|p| p.access = m, cx);
                this.menu = None;
                cx.notify();
            }))
        }))
    }

    fn model_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let q = value_of(&self.picker_search, cx).to_lowercase();
        let (prefs, providers, models, loading) = {
            let s = self.shared.read(cx);
            (s.store.prefs.clone(), s.providers.clone(), s.models.clone(), s.models_loading.clone())
        };
        let sel = self.picker_provider;

        // Rail: favourites star + every provider.
        let mut rail = div().id("picker-rail").flex().flex_col().gap(px(4.)).items_center().w(px(46.)).py(px(8.)).flex_none().overflow_y_scroll().border_r_1().border_color(Tokens::border());
        rail = rail.child(
            div()
                .id("rail-fav")
                .flex()
                .items_center()
                .justify_center()
                .size(px(30.))
                .rounded(px(8.))
                .cursor_pointer()
                .when(sel.is_none(), |d| d.bg(Tokens::active()))
                .hover(|s| s.bg(Tokens::hover()))
                .child(icon("icons/star.svg", if sel.is_none() { Tokens::warning() } else { Tokens::text_3() }, 15.))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.picker_provider = None;
                    cx.notify();
                })),
        );
        for p in mv_ai::PROVIDERS.iter() {
            let id = p.id;
            let ready = providers.iter().find(|s| s.id == id).map(|s| s.state == AiProviderState::Ready).unwrap_or(false);
            rail = rail.child(
                div()
                    .id(SharedString::from(format!("rail-{}", id.as_str())))
                    .p(px(2.))
                    .rounded(px(9.))
                    .cursor_pointer()
                    .when(sel == Some(id), |d| d.bg(Tokens::active()))
                    .hover(|s| s.bg(Tokens::hover()))
                    .child(monogram(id, !ready))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.picker_provider = Some(id);
                        this.shared.update(cx, |s, cx| s.load_models(id, false, cx));
                        cx.notify();
                    })),
            );
        }

        // Model rows.
        let mut rows: Vec<(AiProviderId, AiModel)> = vec![];
        match sel {
            Some(p) => rows.extend(models.get(&p).into_iter().flatten().cloned().map(|m| (p, m))),
            None => {
                for key in &prefs.favourites {
                    if let Some((pid, mid)) = key.split_once("::") {
                        if let Some(p) = mv_ai::PROVIDERS.iter().find(|d| d.id.as_str() == pid).map(|d| d.id) {
                            let m = models.get(&p).and_then(|l| l.iter().find(|m| m.id == mid).cloned()).unwrap_or_else(|| AiModel::new(p, mid, mid));
                            rows.push((p, m));
                        }
                    }
                }
            }
        }
        rows.retain(|(_, m)| q.is_empty() || m.id.to_lowercase().contains(&q) || m.label.to_lowercase().contains(&q));
        // Favourites float to the top inside a provider.
        rows.sort_by_key(|(p, m)| !prefs.favourites.contains(&model_key(*p, &m.id)));
        let is_loading = sel.map(|p| loading.contains(&p)).unwrap_or(false);
        let status = sel.and_then(|p| providers.iter().find(|s| s.id == p).cloned());

        let list_rows = rows.into_iter().enumerate().map(|(i, (p, m))| {
            let fav = prefs.favourites.contains(&model_key(p, &m.id));
            let current = prefs.provider == p && prefs.model == m.id;
            let (pick_id, fav_id) = (m.id.clone(), m.id.clone());
            div()
                .id(("model-row", i))
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(8.))
                .h(px(34.))
                .rounded(px(8.))
                .cursor_pointer()
                .when(current, |d| d.bg(Tokens::accent_soft()))
                .hover(|s| s.bg(Tokens::hover()))
                .when(sel.is_none(), |d| d.child(monogram(p, false).size(px(20.))))
                .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).text_color(Tokens::text_1()).child(m.label.clone()))
                .when_some(m.size_bytes.filter(|b| *b > 0), |d, b| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(crate::ui::fmt_bytes(b))))
                .child(
                    div()
                        .id(("fav", i))
                        .flex_none()
                        .p(px(3.))
                        .rounded(px(6.))
                        .hover(|s| s.bg(Tokens::active()))
                        .child(icon("icons/star.svg", if fav { Tokens::warning() } else { Tokens::text_3() }, 13.))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.shared.update(cx, |s, cx| {
                                s.store.toggle_favourite(p, &fav_id);
                                s.save();
                                cx.notify();
                            });
                        })),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.pick_model(p, pick_id.clone(), cx)))
        });

        let body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(div().p(px(8.)).child(field(&self.picker_search).small().cleanable(true)))
            .child(
                div()
                    .id("picker-list")
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(6.))
                    .children(list_rows)
                    .when(is_loading, |d| d.child(div().p(px(10.)).child(muted("Loading models…"))))
                    .when_some(status.filter(|s| s.state != AiProviderState::Ready), |d, s| {
                        d.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .p(px(10.))
                                .child(muted(s.detail.clone().unwrap_or_else(|| format!("{} is {}.", s.label, crate::ui::state_text(s.state).to_lowercase()))))
                                .child(
                                    div()
                                        .id("picker-setup")
                                        .text_size(px(size::SM))
                                        .text_color(Tokens::accent())
                                        .cursor_pointer()
                                        .child("Set up in Settings")
                                        .on_click(cx.listener(|this, _, w, cx| {
                                            this.menu = None;
                                            mv_ui::navigate(Route::Settings { tab: Some("providers".into()) }, w, cx);
                                            cx.notify();
                                        })),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .p(px(8.))
                    .border_t_1()
                    .border_color(Tokens::border())
                    .child(div().flex_1().child(field(&self.custom_model).small()))
                    .child(
                        div()
                            .id("custom-use")
                            .px(px(10.))
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .rounded(px(7.))
                            .bg(Tokens::active())
                            .cursor_pointer()
                            .text_size(px(size::SM))
                            .text_color(Tokens::text_1())
                            .hover(|s| s.opacity(0.85))
                            .child("Use")
                            .on_click(cx.listener(|this, _, w, cx| this.add_custom_model(w, cx))),
                    ),
            );

        popup_surface()
            .w(px(380.))
            .h(px(380.))
            .flex_row()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.menu = None;
                cx.notify();
            }))
            .child(rail)
            .child(body)
    }
}

fn push_activity(item: &mut ChatItem, line: String) {
    item.activity.push(line);
    if item.activity.len() > 12 {
        item.activity.remove(0);
    }
}

fn short_model(id: &str) -> String {
    let base = id.rsplit('/').next().unwrap_or(id);
    if base.chars().count() > 22 {
        format!("{}…", base.chars().take(21).collect::<String>())
    } else {
        base.to_string()
    }
}

fn step_row(st: &ToolStep) -> Div {
    let (ico, color) = match st.state {
        StepState::Done => ("icons/check.svg", Tokens::accent()),
        StepState::Error => ("icons/triangle-alert.svg", Tokens::danger()),
        StepState::Denied => ("icons/x.svg", Tokens::text_3()),
        StepState::AwaitingApproval => ("icons/shield-check.svg", Tokens::warning()),
        StepState::Running => ("icons/loader.svg", Tokens::text_3()),
    };
    div()
        .flex()
        .flex_col()
        .gap(px(1.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(size::SM))
                .text_color(Tokens::text_2())
                .child(icon(ico, color, 12.))
                .child(div().flex_1().min_w_0().truncate().child(st.label.clone()))
                .when(st.state == StepState::Denied, |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("declined"))),
        )
        .when_some(st.result.clone().filter(|_| st.state == StepState::Error), |d, r| d.child(div().pl(px(18.)).text_size(px(size::XS)).text_color(Tokens::danger()).child(r)))
}

/// Starter prompts that fit the page the user is on.
pub fn suggestions(page: Option<&str>) -> Vec<&'static str> {
    match page {
        Some("library") | Some("browse") | Some("history") | Some("manga-detail") | Some("reader") => vec!["What manga have I not finished?", "Open my reading history", "Take me to Browse"],
        Some("anime") | Some("anime-detail") => vec!["Search for a popular action anime", "What did I watch recently?", "Open the Discover page"],
        Some("movies") | Some("movie-detail") => vec!["Find a good sci-fi movie", "What did I watch recently?", "Open my favourites"],
        Some("discover") | Some("favorites") | Some("media-history") => vec!["What did I watch recently?", "Find something like Death Note", "Switch to movies"],
        Some("settings") => vec!["Open the AI providers settings", "Where can I download local models?", "Take me back"],
        _ => vec!["What can you do in this app?", "Show me my library", "What did I watch recently?"],
    }
}

impl Render for Dock {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, items) = {
            let s = self.shared.read(cx);
            match s.store.active_thread() {
                Some(t) => (t.title.clone(), t.items.clone()),
                None => ("Assistant".to_string(), vec![]),
            }
        };
        let n = items.len();
        let list = if items.is_empty() {
            div().flex_1().min_h_0().child(self.empty_view(cx)).into_any_element()
        } else {
            let mut col = div().id("dock-messages").flex().flex_col().gap(px(16.)).px(px(14.)).py(px(14.)).flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.scroll);
            for (i, item) in items.iter().enumerate() {
                col = col.child(self.render_item(item, i + 1 == n, cx));
            }
            col.into_any_element()
        };
        let drawer = self.drawer.then(|| self.drawer_view(cx));
        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(Tokens::base())
            .child(self.header(title, cx))
            .child(div().relative().flex().flex_col().flex_1().min_h_0().child(list).children(drawer))
            .child(self.composer(cx))
    }
}
