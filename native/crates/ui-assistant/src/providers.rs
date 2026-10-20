//! Settings → AI Providers panel: one collapsible card per provider, grouped by where it runs.
use std::collections::{HashMap, HashSet};

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ai::{AiProviderId, AiProviderKind, AiProviderSettings, AiProviderState, AiProviderStatus};
use mv_ui::theme::{icon as icon_px, radius};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, copy_button, empty_view, external_link, glyphs, icon_tag, ph, ph_icon, size, skeleton_block, tip, ButtonKind, Interactive as _, Ph, Tokens, Tone, Weight, MONO_FONT};

use crate::shared::{shared, Shared};
use crate::ui::{field, muted, password_state, provider_logo, set_value, status_pill, text_state, value_of};

struct Fields {
    key: Entity<InputState>,
    url: Entity<InputState>,
    model: Entity<InputState>,
    _subs: Vec<Subscription>,
}

#[derive(Clone)]
enum Test {
    Running,
    Done { ok: bool, message: String, ms: u64 },
}

pub struct ProvidersPanel {
    shared: Entity<Shared>,
    fields: HashMap<AiProviderId, Fields>,
    prefilled: HashSet<AiProviderId>,
    tests: HashMap<AiProviderId, Test>,
    /// Cards the user opened. The provider in use opens by default.
    expanded: HashSet<AiProviderId>,
    seeded: bool,
    _subs: Vec<Subscription>,
}

pub fn create(_window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| {
        let shared = shared(cx);
        shared.update(cx, |s, cx| s.refresh_providers(true, cx));
        let sub = cx.observe(&shared, |_, _, cx| cx.notify());
        ProvidersPanel { shared, fields: HashMap::new(), prefilled: HashSet::new(), tests: HashMap::new(), expanded: HashSet::new(), seeded: false, _subs: vec![sub] }
    })
    .into()
}

impl ProvidersPanel {
    /// Writes a settings patch off the UI thread and confirms with a toast.
    fn patch(&mut self, id: AiProviderId, patch: AiProviderSettings, note: Option<String>, cx: &mut Context<Self>) {
        let reg = self.shared.read(cx).registry.clone();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || reg.update_provider_settings(id, &patch)).await;
            let _ = this.update(cx, |this, cx| {
                if matches!(res, Ok(Ok(()))) {
                    if let Some(n) = note {
                        toast::show(cx, Toast::success(n));
                    }
                } else {
                    toast::show(cx, Toast::error("Couldn't save the setting: config.json isn't writable."));
                }
                this.shared.update(cx, |s, cx| {
                    s.models.remove(&id);
                    s.refresh_providers(true, cx);
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// What the card's fields hold that isn't saved yet: a pasted key, or a URL / default model
    /// typed without pressing Enter. Kit buttons don't take focus, so clicking Test or Use in
    /// assistant never blurs the field; those flush it first instead of acting on the old value.
    fn take_unsaved(&mut self, id: AiProviderId, window: &mut Window, cx: &mut Context<Self>) -> Option<AiProviderSettings> {
        let (key, url, model) = {
            let f = self.fields.get(&id)?;
            (f.key.clone(), f.url.clone(), f.model.clone())
        };
        let (url0, model0) = self.stored(id, cx);
        let mut p = AiProviderSettings::default();
        let k = value_of(&key, cx).trim().to_string();
        if !k.is_empty() {
            p.api_key = Some(k);
            p.enabled = Some(true);
            set_value(&key, "", window, cx);
        }
        let u = value_of(&url, cx).trim().to_string();
        if u != url0 {
            p.base_url = Some(u);
        }
        let m = value_of(&model, cx).trim().to_string();
        if m != model0 {
            p.default_model = Some(m);
        }
        (p != AiProviderSettings::default()).then_some(p)
    }

    fn test(&mut self, id: AiProviderId, window: &mut Window, cx: &mut Context<Self>) {
        let unsaved = self.take_unsaved(id, window, cx);
        self.tests.insert(id, Test::Running);
        let reg = self.shared.read(cx).registry.clone();
        cx.spawn(async move |this, cx| {
            // Save first, so the test reads what the fields show.
            let saved = match unsaved {
                Some(p) => {
                    let (reg, key) = (reg.clone(), p.api_key.is_some());
                    let ok = matches!(mv_ui::bg::blocking(move || reg.update_provider_settings(id, &p)).await, Ok(Ok(())));
                    Some((ok, key))
                }
                None => None,
            };
            let r = mv_ui::bg::run(async move { reg.test_provider(id, None).await }).await;
            let t = match r {
                Ok(r) => Test::Done { ok: r.ok, message: r.message, ms: r.ms },
                Err(e) => Test::Done { ok: false, message: e.to_string(), ms: 0 },
            };
            let _ = this.update(cx, |this, cx| {
                this.tests.insert(id, t);
                match saved {
                    Some((false, _)) => toast::show(cx, Toast::error("Couldn't save the setting: config.json isn't writable.")),
                    Some((true, true)) => toast::show(cx, Toast::success("API key saved")),
                    _ => {}
                }
                if saved.is_some() {
                    this.shared.update(cx, |s, cx| {
                        s.models.remove(&id);
                        s.refresh_providers(true, cx);
                    });
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn use_in_assistant(&mut self, id: AiProviderId, typed: String, cx: &mut Context<Self>) {
        let sh = self.shared.clone();
        let reg = sh.read(cx).registry.clone();
        let default = sh.read(cx).status(id).and_then(|s| s.default_model.clone());
        cx.spawn(async move |this, cx| {
            let pick = match Some(typed).filter(|t| !t.trim().is_empty()).or(default) {
                Some(m) => Some(m),
                None => mv_ui::bg::run(async move { reg.list_models(id, false).await }).await.ok().and_then(|l| l.first().map(|m| m.id.clone())),
            };
            let _ = this.update(cx, |this, cx| {
                match pick {
                    Some(m) => {
                        this.shared.update(cx, |s, cx| s.use_in_assistant(id, &m, cx));
                        toast::show(cx, Toast::success(format!("The assistant now uses {m}")).icon(glyphs::ASSISTANT));
                    }
                    None => toast::show(cx, Toast::warning("No model available yet. Set a default model first.")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Saved URL / default model, as the fields show them.
    fn stored(&self, id: AiProviderId, cx: &App) -> (String, String) {
        let s = self.shared.read(cx);
        let st = s.status(id);
        let url = st.and_then(|st| st.base_url.clone().filter(|u| Some(u) != st.default_base_url.as_ref())).unwrap_or_default();
        let model = st.and_then(|st| st.default_model.clone()).unwrap_or_default();
        (url, model)
    }

    /// Fields save when you press Enter or leave them (no separate Save buttons), and only when
    /// the value changed.
    fn save_url(&mut self, id: AiProviderId, value: String, cx: &mut Context<Self>) {
        let v = value.trim().to_string();
        if v == self.stored(id, cx).0 {
            return;
        }
        self.patch(id, AiProviderSettings { base_url: Some(v), ..Default::default() }, Some("Server URL saved".into()), cx);
    }

    fn save_model(&mut self, id: AiProviderId, value: String, cx: &mut Context<Self>) {
        let v = value.trim().to_string();
        if v == self.stored(id, cx).1 {
            return;
        }
        self.patch(id, AiProviderSettings { default_model: Some(v), ..Default::default() }, Some("Default model saved".into()), cx);
    }

    fn save_key(&mut self, id: AiProviderId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.fields.get(&id).map(|f| f.key.clone()) else { return };
        let k = value_of(&key, cx).trim().to_string();
        if k.is_empty() {
            return;
        }
        set_value(&key, "", window, cx);
        self.patch(id, AiProviderSettings { api_key: Some(k), enabled: Some(true), ..Default::default() }, Some("API key saved".into()), cx);
    }

    fn ensure_fields(&mut self, st: &AiProviderStatus, window: &mut Window, cx: &mut Context<Self>) {
        let id = st.id;
        if let std::collections::hash_map::Entry::Vacant(slot) = self.fields.entry(id) {
            let key = password_state(if st.has_key { "Key saved. Paste a new one to replace it" } else { "Paste your API key" }, window, cx);
            let url = text_state(st.default_base_url.as_deref().unwrap_or("Server URL"), window, cx);
            let model = text_state("Default model id", window, cx);
            let subs = vec![
                cx.subscribe_in(&key, window, move |this, _, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        this.save_key(id, window, cx);
                    }
                }),
                cx.subscribe_in(&url, window, move |this, input, ev: &InputEvent, _, cx| {
                    if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        let v = value_of(input, cx);
                        this.save_url(id, v, cx);
                    }
                }),
                cx.subscribe_in(&model, window, move |this, input, ev: &InputEvent, _, cx| {
                    if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        let v = value_of(input, cx);
                        this.save_model(id, v, cx);
                    }
                }),
            ];
            slot.insert(Fields { key, url, model, _subs: subs });
        }
        if self.prefilled.insert(id) {
            let f = &self.fields[&id];
            if let Some(u) = st.base_url.as_deref().filter(|u| Some(*u) != st.default_base_url.as_deref()) {
                set_value(&f.url, u, window, cx);
            }
            if let Some(m) = &st.default_model {
                set_value(&f.model, m, window, cx);
            }
        }
    }

    fn card(&mut self, st: &AiProviderStatus, current: bool, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let id = st.id;
        self.ensure_fields(st, window, cx);
        let (key, url, model) = {
            let f = &self.fields[&id];
            (f.key.clone(), f.url.clone(), f.model.clone())
        };
        let open = self.expanded.contains(&id);
        let test = self.tests.get(&id).cloned();
        let enabled = st.enabled;
        let ready = st.state == AiProviderState::Ready;
        let sid = id.as_str();
        let label = st.label.clone();

        let header = div()
            .id(SharedString::from(format!("prov-head-{sid}")))
            .flex()
            .items_center()
            .gap(px(12.))
            .min_h(px(56.))
            .px(px(14.))
            .py(px(10.))
            .map(|d| if open { d.rounded_t(px(radius::LG)) } else { d.rounded(px(radius::LG)) })
            .role(Role::Button)
            .aria_expanded(open)
            .hover(|s| s.bg(Tokens::hover()))
            .pressable()
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.expanded.remove(&id) {
                    this.expanded.insert(id);
                }
                cx.notify();
            }))
            .child(div().flex().flex_none().items_center().justify_center().size(px(32.)).rounded(px(radius::MD)).bg(Tokens::surface_2()).opacity(if st.state == AiProviderState::Disabled { 0.45 } else { 1. }).child(provider_logo(id, icon_px::LG)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(label.clone()))
                            .when(current, |d| d.child(icon_tag(ph::CHECK, "In use", Tone::Accent)))
                            .when_some(st.version.clone(), |d, v| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(v))),
                    )
                    .child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(st.description.clone())),
            )
            // A local app that isn't installed: its download is one click away without opening
            // the card (the copyable install command is inside).
            .when_some(download_url(id).filter(|_| !open && st.state == AiProviderState::NotInstalled), |d, u| {
                d.child(btn(SharedString::from(format!("get-{sid}")), format!("Get {label}"), ButtonKind::Ghost).small().trailing_icon(ph::ARROW_UP_RIGHT).tooltip(u).on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    cx.open_url(u);
                }))
            })
            .child(status_pill(st.state))
            // The switch acts on its own; keep its click from also opening the card.
            .child(
                div().id(SharedString::from(format!("en-wrap-{sid}"))).flex_none().on_click(|_, _, cx| cx.stop_propagation()).child(
                    mv_ui::switch(SharedString::from(format!("en-{sid}")), enabled)
                        .tooltip(if enabled { format!("Turn off {label}") } else { format!("Turn on {label}") })
                        .on_click(cx.listener(move |this, on: &bool, _, cx| {
                            let lbl = this.shared.read(cx).status(id).map(|s| s.label.clone()).unwrap_or_default();
                            let note = if *on { format!("{lbl} turned on") } else { format!("{lbl} turned off") };
                            this.patch(id, AiProviderSettings { enabled: Some(*on), ..Default::default() }, Some(note), cx);
                        })),
                ),
            )
            .child(ph_icon(if open { ph::CARET_UP } else { ph::CARET_DOWN }, Weight::Bold, Tokens::text_3(), icon_px::SM));

        let key_url = st.key_url.clone();
        let show_hint = matches!(st.state, AiProviderState::NotInstalled | AiProviderState::Offline) && st.kind != AiProviderKind::Api;
        let body = open.then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .px(px(14.))
                .pt(px(12.))
                .pb(px(14.))
                .border_t_1()
                .border_color(Tokens::border())
                .when_some(st.detail.clone().filter(|_| !ready && !show_hint), |d, t| d.child(muted(t)))
                .when_some(st.install_hint.clone().filter(|_| show_hint), |d, h| d.child(install_block(id, &label, &h)))
                .when(st.needs_key, |d| {
                    d.child(field_row(
                        "API key",
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .flex_1()
                            .min_w_0()
                            .child(div().flex_1().min_w_0().child(field(&key).small().mask_toggle()))
                            .child(btn(SharedString::from(format!("savekey-{sid}")), "Save key", ButtonKind::Secondary).small().icon(ph::KEY).on_click(cx.listener(move |this, _, w, cx| this.save_key(id, w, cx))))
                            .when(st.has_key, |d| {
                                d.child(btn(SharedString::from(format!("rmkey-{sid}")), "Remove", ButtonKind::DangerGhost).small().icon(glyphs::DELETE).on_click(cx.listener(move |this, _, _, cx| {
                                    this.patch(id, AiProviderSettings { api_key: Some(String::new()), ..Default::default() }, Some("API key removed".into()), cx)
                                })))
                            }),
                    ))
                    .when_some(key_url, |d, u| d.child(div().pl(px(116.)).child(external_link("Get an API key", u))))
                })
                .when(st.kind != AiProviderKind::Cli && (st.default_base_url.is_some() || id == AiProviderId::Custom), |d| d.child(field_row("Server URL", div().flex_1().min_w_0().child(field(&url).small()))))
                .child(field_row("Default model", div().flex_1().min_w_0().child(field(&model).small())))
                .child(div().pl(px(116.)).child(muted("Fields save when you press Enter or click away.")))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            btn(SharedString::from(format!("test-{sid}")), "Test connection", ButtonKind::Secondary)
                                .small()
                                .icon(ph::PLUGS_CONNECTED)
                                .loading(matches!(test, Some(Test::Running)))
                                .on_click(cx.listener(move |this, _, w, cx| this.test(id, w, cx))),
                        )
                        .map(|d| {
                            if current {
                                d
                            } else {
                                // One primary action: only on an expanded, ready card that isn't in use.
                                let use_btn = btn(SharedString::from(format!("use-{sid}")), "Use in assistant", ButtonKind::Primary).small().icon(glyphs::ASSISTANT);
                                let use_btn = if ready {
                                    let model = model.clone();
                                    use_btn.on_click(cx.listener(move |this, _, w, cx| {
                                        let m = value_of(&model, cx);
                                        // A URL or key typed without Enter applies too.
                                        if let Some(p) = this.take_unsaved(id, w, cx) {
                                            this.patch(id, p, None, cx);
                                        }
                                        this.use_in_assistant(id, m, cx)
                                    }))
                                } else {
                                    use_btn.disabled(format!("{label} isn't ready yet"))
                                };
                                d.child(use_btn)
                            }
                        })
                        .children(test.and_then(|t| test_result(sid, t))),
                )
        });

        div()
            .flex()
            .flex_col()
            .rounded(px(radius::LG))
            .border_1()
            .border_color(if current { Tokens::accent_border() } else { Tokens::border() })
            .bg(Tokens::surface())
            .child(header)
            .children(body)
    }
}

/// Label column plus control, so the fields line up across the card.
fn field_row(label: &'static str, control: impl IntoElement) -> Div {
    div().flex().items_center().gap(px(12.)).child(div().w(px(104.)).flex_none().text_size(px(size::SM)).text_color(Tokens::text_2()).child(label)).child(control)
}

fn test_result(sid: &str, t: Test) -> Option<AnyElement> {
    match t {
        Test::Running => None,
        Test::Done { ok: true, ms, .. } => Some(
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .text_size(px(size::SM))
                .text_color(Tokens::success())
                .child(ph_icon(ph::CHECK_CIRCLE, Weight::Fill, Tokens::success(), icon_px::SM))
                .child(if ms > 0 { format!("Connected · {ms} ms") } else { "Connected".to_string() })
                .into_any_element(),
        ),
        Test::Done { ok: false, message, .. } => {
            let short = message.lines().next().unwrap_or("").chars().take(90).collect::<String>();
            Some(
                div()
                    .id(SharedString::from(format!("test-res-{sid}")))
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(px(5.))
                    .text_size(px(size::SM))
                    .text_color(Tokens::danger())
                    .tooltip(tip(message.clone()))
                    .child(ph_icon(ph::X_CIRCLE, Weight::Fill, Tokens::danger(), icon_px::SM))
                    .child(div().min_w_0().truncate().child(if short.is_empty() { "Connection failed".to_string() } else { short }))
                    .into_any_element(),
            )
        }
    }
}

/// The command a provider's install hint asks you to run, so it can be copied.
pub fn install_command(hint: &str) -> Option<String> {
    let h = hint.trim();
    if h.starts_with("npm ") {
        return Some(h.split(',').next().unwrap_or(h).trim().to_string());
    }
    let start = h.find('`')? + 1;
    let len = h[start..].find('`')?;
    Some(h[start..start + len].to_string()).filter(|c| !c.is_empty())
}

/// Where to get a provider that runs on this PC.
fn download_url(id: AiProviderId) -> Option<&'static str> {
    match id {
        AiProviderId::Ollama => Some("https://ollama.com/download"),
        AiProviderId::Lmstudio => Some("https://lmstudio.ai"),
        AiProviderId::Llamacpp => Some("https://github.com/ggml-org/llama.cpp/releases"),
        _ => None,
    }
}

/// Install hint with a copyable command and a download link.
fn install_block(id: AiProviderId, label: &str, hint: &str) -> Div {
    let sid = id.as_str();
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(hint.to_string()))
        .when_some(install_command(hint), |d, c| {
            d.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(32.))
                    .pl(px(10.))
                    .pr(px(2.))
                    .rounded(px(radius::SM))
                    .bg(Tokens::well())
                    .border_1()
                    .border_color(Tokens::border())
                    .child(ph_icon(ph::TERMINAL_WINDOW, Weight::Bold, Tokens::text_3(), icon_px::SM))
                    .child(div().flex_1().min_w_0().truncate().font_family(MONO_FONT).text_size(px(size::SM)).text_color(Tokens::text_1()).child(c.clone()))
                    .child(copy_button(SharedString::from(format!("copy-cmd-{sid}")), c).small()),
            )
        })
        .when_some(download_url(id), |d, u| d.child(external_link(format!("Download {label}"), u)))
}

/// Uppercase group header with its icon and how many providers in it are ready.
fn group_header(icon: Ph, title: &str, ready: usize, total: usize) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .mt(px(16.))
        .mb(px(2.))
        .px(px(2.))
        .child(ph_icon(icon, Weight::Bold, Tokens::text_3(), icon_px::SM))
        .child(div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_3()).child(title.to_uppercase()))
        .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(if ready > 0 { format!("{ready} of {total} ready") } else { format!("{total}") }))
}

impl Render for ProvidersPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (providers, loading, prefs_provider, in_use_model) = {
            let s = self.shared.read(cx);
            (s.providers.clone(), s.providers_loading, s.store.prefs.provider, !s.store.prefs.model.is_empty())
        };
        if !self.seeded && !providers.is_empty() {
            self.seeded = true;
            if in_use_model {
                self.expanded.insert(prefs_provider);
            }
        }
        // Embedded in Settings, which draws the page title; keep only the refresh control.
        let mut root = div().flex().flex_col().gap(px(10.)).w_full().child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(div().flex_1().text_size(px(size::SM)).text_color(Tokens::text_3()).child(if loading { "Checking which providers are available…" } else { "Turn a provider on, add its key if it needs one, then test it." }))
                .child(btn("prov-refresh", "Refresh", ButtonKind::Ghost).small().icon(glyphs::REFRESH).loading(loading).tooltip("Detect providers again").on_click(cx.listener(|this, _, _, cx| this.shared.update(cx, |s, cx| s.refresh_providers(true, cx))))),
        );
        if providers.is_empty() {
            root = root.child(if loading {
                div().flex().flex_col().gap(px(10.)).children((0..4).map(|_| skeleton_block(relative(1.), px(56.)))).into_any_element()
            } else {
                empty_view(ph::PLUGS, "No providers found", "Refresh to detect local apps, coding agents and API keys again.").into_any_element()
            });
        }
        for (title, kind, icon) in [("On this PC", AiProviderKind::Local, ph::DESKTOP), ("Coding agents", AiProviderKind::Cli, ph::TERMINAL_WINDOW), ("Cloud APIs", AiProviderKind::Api, ph::CLOUD)] {
            let group: Vec<&AiProviderStatus> = providers.iter().filter(|p| p.kind == kind).collect();
            if group.is_empty() {
                continue;
            }
            let ready = group.iter().filter(|p| p.state == AiProviderState::Ready).count();
            root = root.child(group_header(icon, title, ready, group.len()));
            for st in group {
                let current = in_use_model && st.id == prefs_provider;
                root = root.child(self.card(st, current, window, cx));
            }
        }
        div().id("providers-panel").w_full().child(root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn install_commands_are_extracted() {
        assert_eq!(install_command("npm i -g @openai/codex, then run `codex login`.").as_deref(), Some("npm i -g @openai/codex"));
        assert_eq!(install_command("Run `llama-server -m model.gguf --jinja` (port 8080).").as_deref(), Some("llama-server -m model.gguf --jinja"));
        assert_eq!(install_command("Install from ollama.com, then start Ollama."), None);
        assert!(download_url(AiProviderId::Ollama).is_some());
        assert!(download_url(AiProviderId::Openai).is_none());
    }
}
