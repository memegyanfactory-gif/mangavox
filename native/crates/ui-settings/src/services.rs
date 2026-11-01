//! Settings › AI services: keys for image / video and voice services that are not chat models
//! (those live in AI providers). Each key is saved to config.json, checked with a free call to
//! its service, and can be removed. Keys are never shown again after saving, only masked.
use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ai::{KeySource, ServiceDef, ServiceKind, Services, TestResult, SERVICES};
use mv_ui::toast::{self, Toast};
use mv_ui::{badge, btn, external_link, ph, setting_row, setting_section, size, ButtonKind, Tokens, Tone};

use crate::ui::divided;

#[derive(Clone)]
enum Check {
    Running,
    Done(TestResult),
}

pub struct ServicesPanel {
    services: Arc<Services>,
    inputs: HashMap<&'static str, Entity<InputState>>,
    checks: HashMap<&'static str, Check>,
    _subs: Vec<Subscription>,
}

pub fn create(window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| ServicesPanel::new(window, cx)).into()
}

/// "sk_a…9f2c" for a saved key.
fn masked(k: &str) -> String {
    let n = k.chars().count();
    if n <= 8 {
        return "••••".into();
    }
    format!("{}…{}", k.chars().take(4).collect::<String>(), k.chars().skip(n - 4).collect::<String>())
}

impl ServicesPanel {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let services = Arc::new(Services::new(mv_core::paths::config()));
        let mut inputs = HashMap::new();
        let mut subs = vec![];
        for def in SERVICES {
            let id = def.id;
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(def.placeholder).masked(true));
            subs.push(cx.subscribe_in(&input, window, move |this, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.save(id, window, cx);
                }
            }));
            inputs.insert(id, input);
        }
        ServicesPanel { services, inputs, checks: HashMap::new(), _subs: subs }
    }

    /// Saves what the field holds, then tests it.
    fn save(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.inputs.get(id).cloned() else { return };
        let k = input.read(cx).value().trim().to_string();
        if k.is_empty() {
            return;
        }
        if self.services.set_key(id, Some(&k)).is_err() {
            toast::show(cx, Toast::error("Couldn't save the key: config.json isn't writable."));
            return;
        }
        input.update(cx, |s, cx| s.set_value("", window, cx));
        toast::show(cx, Toast::success("Key saved"));
        self.test(id, cx);
    }

    fn test(&mut self, id: &'static str, cx: &mut Context<Self>) {
        self.checks.insert(id, Check::Running);
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let r = mv_ui::bg::run(async move { services.test(id).await }).await.unwrap_or_else(|e| TestResult { ok: false, message: e.to_string(), ms: 0 });
            let _ = this.update(cx, |this, cx| {
                this.checks.insert(id, Check::Done(r));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn remove(&mut self, id: &'static str, cx: &mut Context<Self>) {
        if self.services.set_key(id, None).is_err() {
            toast::show(cx, Toast::error("Couldn't remove the key: config.json isn't writable."));
            return;
        }
        self.checks.remove(id);
        toast::show(cx, Toast::info("Key removed"));
        cx.notify();
    }

    fn row(&self, def: &'static ServiceDef, cx: &mut Context<Self>) -> Div {
        let id = def.id;
        let key = self.services.key(id);
        let source = self.services.key_source(id);
        let check = self.checks.get(id).cloned();
        let (tone, text) = match (&key, &check) {
            (None, _) => (Tone::Neutral, "No key"),
            (Some(_), Some(Check::Running)) => (Tone::Info, "Checking…"),
            (Some(_), Some(Check::Done(r))) if r.ok => (Tone::Success, "Connected"),
            (Some(_), Some(Check::Done(_))) => (Tone::Danger, "Not working"),
            (Some(_), None) => (Tone::Accent, "Saved"),
        };
        let input = self.inputs[id].clone();
        let control = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(badge(text, tone))
            .when_some(key.as_deref().map(masked), |d, m| d.child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(m)))
            .child(div().w(px(220.)).child(Input::new(&input).small().mask_toggle()))
            .child(btn(SharedString::from(format!("svc-save-{id}")), "Save", ButtonKind::Secondary).small().icon(ph::KEY).on_click(cx.listener(move |this, _: &ClickEvent, w, cx| this.save(id, w, cx))))
            .when(key.is_some(), |d| {
                d.child(
                    btn(SharedString::from(format!("svc-test-{id}")), "Test", ButtonKind::Ghost)
                        .small()
                        .icon(ph::PLUGS_CONNECTED)
                        .loading(matches!(check, Some(Check::Running)))
                        .tooltip("A free check with the service; spends no credits")
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.test(id, cx))),
                )
            })
            .when(source == Some(KeySource::Saved), |d| {
                d.child(btn(SharedString::from(format!("svc-rm-{id}")), "Remove", ButtonKind::DangerGhost).small().icon(ph::TRASH).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.remove(id, cx))))
            });
        let hint = match source {
            Some(KeySource::Env) => format!("{} · from an environment variable", def.description),
            _ => def.description.to_string(),
        };
        let result = match check {
            Some(Check::Done(r)) => Some(
                div()
                    .pb(px(10.))
                    .text_size(px(size::SM))
                    .whitespace_normal()
                    .text_color(if r.ok { Tokens::success() } else { Tokens::danger() })
                    .child(if r.ms > 0 { format!("{} · {} ms", r.message, r.ms) } else { r.message }),
            ),
            _ => None,
        };
        div()
            .flex()
            .flex_col()
            .child(setting_row(def.label, hint, control))
            .child(div().flex().items_center().gap(px(12.)).pb(px(8.)).mt(px(-6.)).child(external_link("Get a key", def.key_url)))
            .children(result)
    }

    fn section(&self, kind: ServiceKind, title: &str, desc: &str, cx: &mut Context<Self>) -> Div {
        let mut s = setting_section(title.to_string(), Some(desc.to_string().into()));
        for (i, def) in SERVICES.iter().filter(|d| d.kind == kind).enumerate() {
            let row = self.row(def, cx);
            s = s.child(if i == 0 { row } else { divided(row) });
        }
        s
    }
}

impl Render for ServicesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("services-panel")
            .flex()
            .flex_col()
            .gap(px(20.))
            .w_full()
            .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).whitespace_normal().child(
                "Keys for image, video and voice services. Each key stays in this PC's config file and is only sent to its own service. Chat models (OpenAI, Together, Kimi…) are under AI providers; the Thumbnails generator uses Magnific (Settings › Magnific).",
            ))
            .child(self.section(ServiceKind::Image, "Images and video", "Text-to-image and video generation.", cx))
            .child(self.section(ServiceKind::Voice, "Voices", "Cloud text-to-speech.", cx))
    }
}
