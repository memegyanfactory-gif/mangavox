//! Settings → AI Providers panel.
use std::collections::{HashMap, HashSet};

use gpui_kit::component::input::InputState;
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ai::{AiProviderId, AiProviderKind, AiProviderSettings, AiProviderState, AiProviderStatus};
use mv_ui::{button, size, ButtonKind, Tokens};

use crate::shared::{shared, Shared};
use crate::ui::{field, provider_logo, muted, password_state, set_value, status_pill, tag, text_state, value_of};

struct Fields {
    key: Entity<InputState>,
    url: Entity<InputState>,
    model: Entity<InputState>,
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
    notes: HashMap<AiProviderId, String>,
    _subs: Vec<Subscription>,
}

pub fn create(_window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| {
        let shared = shared(cx);
        shared.update(cx, |s, cx| s.refresh_providers(true, cx));
        let sub = cx.observe(&shared, |_, _, cx| cx.notify());
        ProvidersPanel { shared, fields: HashMap::new(), prefilled: HashSet::new(), tests: HashMap::new(), notes: HashMap::new(), _subs: vec![sub] }
    })
    .into()
}

impl ProvidersPanel {
    fn patch(&mut self, id: AiProviderId, patch: AiProviderSettings, note: Option<&str>, cx: &mut Context<Self>) {
        let reg = self.shared.read(cx).registry.clone();
        if let Some(n) = note {
            self.notes.insert(id, n.to_string());
        }
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || reg.update_provider_settings(id, &patch)).await;
            let _ = this.update(cx, |this, cx| {
                if !matches!(res, Ok(Ok(()))) {
                    this.notes.insert(id, "Could not write config.json".into());
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

    fn test(&mut self, id: AiProviderId, cx: &mut Context<Self>) {
        self.tests.insert(id, Test::Running);
        let reg = self.shared.read(cx).registry.clone();
        cx.spawn(async move |this, cx| {
            let r = mv_ui::bg::run(async move { reg.test_provider(id, None).await }).await;
            let t = match r {
                Ok(r) => Test::Done { ok: r.ok, message: r.message, ms: r.ms },
                Err(e) => Test::Done { ok: false, message: e.to_string(), ms: 0 },
            };
            let _ = this.update(cx, |this, cx| {
                this.tests.insert(id, t);
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
                        this.notes.insert(id, format!("The assistant now uses {m}."));
                    }
                    None => {
                        this.notes.insert(id, "No model available yet — set a default model first.".into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn card(&mut self, st: &AiProviderStatus, current: bool, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let id = st.id;
        if !self.fields.contains_key(&id) {
            let f = Fields { key: password_state(if st.has_key { "Key saved — paste a new one to replace" } else { "API key" }, window, cx), url: text_state(st.default_base_url.as_deref().unwrap_or("Server URL"), window, cx), model: text_state("Default model", window, cx) };
            self.fields.insert(id, f);
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
        let (key, url, model) = {
            let f = &self.fields[&id];
            (f.key.clone(), f.url.clone(), f.model.clone())
        };
        let test = self.tests.get(&id).cloned();
        let note = self.notes.get(&id).cloned();
        let enabled = st.enabled;
        let sid = id.as_str();
        let small_btn = |label: &'static str, kind: ButtonKind| button(SharedString::from(format!("{label}-{sid}")), label, kind).h(px(28.)).px(px(10.));

        let toggle = div()
            .id(SharedString::from(format!("en-{sid}")))
            .flex()
            .items_center()
            .w(px(36.))
            .h(px(20.))
            .px(px(2.))
            .rounded(px(10.))
            .cursor_pointer()
            .bg(if enabled { Tokens::accent() } else { Tokens::active() })
            .when(enabled, |d| d.justify_end())
            .child(div().size(px(16.)).rounded(px(8.)).bg(if enabled { Tokens::base() } else { Tokens::text_2() }))
            .on_click(cx.listener(move |this, _, _, cx| this.patch(id, AiProviderSettings { enabled: Some(!enabled), ..Default::default() }, None, cx)));

        let key_url = st.key_url.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(18.))
            .rounded(px(12.))
            .border_1()
            .border_color(if current { rgba(0x2bd49a66) } else { Tokens::border() })
            .bg(Tokens::surface())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(div().flex().items_center().justify_center().size(px(34.)).rounded(px(9.)).bg(Tokens::surface_2()).opacity(if st.state == AiProviderState::Disabled { 0.4 } else { 1. }).child(provider_logo(id, 20.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().flex().items_center().gap(px(8.)).child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(st.label.clone())).when(current, |d| d.child(tag("In use", Tokens::accent()))).when_some(st.version.clone(), |d, v| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(v))))
                            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(st.description.clone())),
                    )
                    .child(status_pill(st.state))
                    .child(toggle),
            )
            .when_some(st.detail.clone().filter(|_| st.state != AiProviderState::Ready), |d, t| d.child(muted(t)))
            .when_some(st.install_hint.clone().filter(|_| st.state == AiProviderState::NotInstalled), |d, h| d.child(div().px(px(10.)).py(px(6.)).rounded(px(8.)).bg(Tokens::surface_2()).text_size(px(size::SM)).text_color(Tokens::text_2()).child(h)))
            .when(st.needs_key, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(div().flex_1().child(field(&key).small().mask_toggle()))
                        .child(small_btn("Save key", ButtonKind::Secondary).on_click(cx.listener({
                            let key = key.clone();
                            move |this, _, w, cx| {
                                let k = value_of(&key, cx).trim().to_string();
                                if k.is_empty() {
                                    return;
                                }
                                set_value(&key, "", w, cx);
                                this.patch(id, AiProviderSettings { api_key: Some(k), enabled: Some(true), ..Default::default() }, Some("Key saved."), cx);
                            }
                        })))
                        .when(st.has_key, |d| d.child(small_btn("Remove", ButtonKind::Ghost).on_click(cx.listener(move |this, _, _, cx| this.patch(id, AiProviderSettings { api_key: Some(String::new()), ..Default::default() }, Some("Key removed."), cx))))),
                )
                .when_some(key_url, |d, u| d.child(div().id(SharedString::from(format!("getkey-{sid}"))).text_size(px(size::XS)).text_color(Tokens::accent()).cursor_pointer().child("Get an API key").on_click(move |_, _, cx| cx.open_url(&u))))
            })
            .when(st.kind != AiProviderKind::Cli && (st.default_base_url.is_some() || id == AiProviderId::Custom), |d| {
                d.child(
                    div().flex().items_center().gap(px(6.)).child(div().flex_1().child(field(&url).small())).child(small_btn("Save URL", ButtonKind::Secondary).on_click(cx.listener({
                        let url = url.clone();
                        move |this, _, _, cx| {
                            let u = value_of(&url, cx).trim().to_string();
                            this.patch(id, AiProviderSettings { base_url: Some(u), ..Default::default() }, Some("Server URL saved."), cx);
                        }
                    }))),
                )
            })
            .child(
                div().flex().items_center().gap(px(6.)).child(div().flex_1().child(field(&model).small())).child(small_btn("Save model", ButtonKind::Secondary).on_click(cx.listener({
                    let model = model.clone();
                    move |this, _, _, cx| {
                        let m = value_of(&model, cx).trim().to_string();
                        this.patch(id, AiProviderSettings { default_model: Some(m), ..Default::default() }, Some("Default model saved."), cx);
                    }
                }))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(small_btn("Test", ButtonKind::Secondary).on_click(cx.listener(move |this, _, _, cx| this.test(id, cx))))
                    .child(small_btn("Use in assistant", ButtonKind::Primary).on_click(cx.listener({
                        let model = model.clone();
                        move |this, _, _, cx| {
                            let m = value_of(&model, cx);
                            this.use_in_assistant(id, m, cx)
                        }
                    })))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(size::XS)).map(|d| match &test {
                        Some(Test::Running) => d.text_color(Tokens::text_3()).child("Testing…"),
                        Some(Test::Done { ok, message, ms }) => d.text_color(if *ok { Tokens::accent() } else { Tokens::danger() }).child(format!("{message} ({ms} ms)")),
                        None => d.text_color(Tokens::text_3()).child(note.unwrap_or_default()),
                    })),
            )
    }
}

impl Render for ProvidersPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (providers, loading, prefs_provider, in_use_model) = {
            let s = self.shared.read(cx);
            (s.providers.clone(), s.providers_loading, s.store.prefs.provider, !s.store.prefs.model.is_empty())
        };
        // Embedded in Settings, which draws the page title; keep only the refresh control.
        let mut root = div().flex().flex_col().gap(px(10.)).w_full().child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(div().flex_1().text_size(px(size::SM)).text_color(Tokens::text_3()).child(if loading { "Checking which providers are available…" } else { "Turn a provider on, add its key if it needs one, then test it." }))
                .child(button("prov-refresh", "Refresh", ButtonKind::Ghost).h(px(28.)).on_click(cx.listener(|this, _, _, cx| this.shared.update(cx, |s, cx| s.refresh_providers(true, cx))))),
        );
        if providers.is_empty() {
            root = root.child(muted(if loading { "Detecting providers…" } else { "No providers found." }));
        }
        for (title, kind) in [("On this PC", AiProviderKind::Local), ("Coding agents", AiProviderKind::Cli), ("Cloud APIs", AiProviderKind::Api)] {
            let group: Vec<&AiProviderStatus> = providers.iter().filter(|p| p.kind == kind).collect();
            if group.is_empty() {
                continue;
            }
            root = root.child(div().mt(px(12.)).mb(px(2.)).text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_3()).child(title.to_uppercase()));
            for st in group {
                let current = in_use_model && st.id == prefs_provider;
                root = root.child(self.card(st, current, window, cx));
            }
        }
        div().id("providers-panel").w_full().child(root)
    }
}
