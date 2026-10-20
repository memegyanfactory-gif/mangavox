//! Settings -> Addons: installed Stremio addons, install by manifest URL, turn on or off, remove.
//! Settings draws the page title and description above this panel.
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_media::addons::InstalledAddon;
use mv_ui::theme::{icon as isz, radius};
use mv_ui::toast::{self, Toast};
use mv_ui::{
    bg, btn, confirm_inline, empty_view, glyphs, icon, icon_button, icon_tag, inline_alert, ph, setting_section, size, skeleton_row, switch, tag, tip, AlertKind, ButtonKind,
    Tokens, Tone,
};

use crate::common::{self, registry};
use crate::look;

pub struct AddonsPanel {
    addons: Vec<InstalledAddon>,
    loaded: bool,
    url: Entity<InputState>,
    busy: bool,
    /// Why the last install failed; stays under the field until the next attempt.
    error: Option<String>,
    /// Addon whose Remove is waiting for confirmation.
    confirming: Option<String>,
    _sub: Subscription,
}

impl AddonsPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let url = common::input(window, cx, "https://addon.example.com/manifest.json");
        let sub = cx.subscribe_in(&url, window, |this, _, ev: &InputEvent, w, cx| match ev {
            InputEvent::PressEnter { .. } => this.install(w, cx),
            InputEvent::Change => cx.notify(),
            _ => {}
        });
        let mut s = Self { addons: Vec::new(), loaded: false, url, busy: false, error: None, confirming: None, _sub: sub };
        s.refresh(cx);
        s
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let list = bg::blocking(|| {
                registry().reload();
                registry().list()
            })
            .await
            .unwrap_or_default();
            let _ = this.update(cx, |v, cx| {
                v.addons = list;
                v.loaded = true;
                cx.notify();
            });
        })
        .detach();
    }

    fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.url.read(cx).value().trim().to_string();
        if url.is_empty() || self.busy {
            return;
        }
        // stremio:// links are plain https manifests.
        let url = url.replacen("stremio://", "https://", 1);
        self.busy = true;
        self.error = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let res = bg::run(async move { registry().install(&url).await.map_err(|e| e.to_string()) }).await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update_in(cx, |v, w, cx| {
                v.busy = false;
                match res {
                    Ok(a) => {
                        toast::show(cx, Toast::success(format!("Installed {}", a.manifest.name)).icon(glyphs::SOURCES));
                        v.url.update(cx, |s, cx| s.set_value("", w, cx));
                        v.addons = registry().list();
                    }
                    Err(e) => v.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn remove(&mut self, id: String, cx: &mut Context<Self>) {
        self.confirming = None;
        let name = self.addons.iter().find(|a| a.manifest.id == id).map(|a| a.manifest.name.clone()).unwrap_or_else(|| "Addon".into());
        match registry().uninstall(&id) {
            Ok(()) => toast::show(cx, Toast::success(format!("Removed {name}"))),
            Err(e) => toast::show(cx, Toast::error(format!("Could not remove {name}: {e}"))),
        }
        self.addons = registry().list();
        cx.notify();
    }

    fn toggle(&mut self, id: String, enabled: bool, cx: &mut Context<Self>) {
        if let Err(e) = registry().set_enabled(&id, enabled) {
            toast::show(cx, Toast::error(format!("Could not update the addon: {e}")));
        }
        self.addons = registry().list();
        cx.notify();
    }

    fn focus_url(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.url.update(cx, |s, cx| s.focus(window, cx));
    }

    fn row(&self, i: usize, a: &InstalledAddon, cx: &mut Context<Self>) -> Div {
        let m = &a.manifest;
        let (id, enabled) = (m.id.clone(), a.enabled);
        let has = |r: &str| m.resources.iter().any(|x| x == r);
        let caps = [(has("stream"), ph::PLAY, "Streams"), (has("catalog"), ph::SQUARES_FOUR, "Catalogs"), (has("subtitles"), glyphs::SUBTITLES, "Subtitles")];
        let logo = div()
            .flex_none()
            .size(px(40.))
            .rounded(px(radius::MD))
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_center()
            .bg(Tokens::surface_2())
            .border_1()
            .border_color(Tokens::border())
            .map(|d| match m.icon.clone().filter(|u| u.starts_with("http")) {
                Some(u) => d.child(look::pic_small(u, glyphs::SOURCES, isz::LG)),
                None => d.child(icon(glyphs::SOURCES.regular(), Tokens::text_2(), isz::LG)),
            });
        let controls = if self.confirming.as_deref() == Some(id.as_str()) {
            let yes = id.clone();
            confirm_inline(format!("Remove {}?", m.name), "Remove", true)
                .cancel_label("Keep")
                .on_confirm(cx.listener(move |this, _, _, cx| this.remove(yes.clone(), cx)))
                .on_cancel(cx.listener(|this, _, _, cx| {
                    this.confirming = None;
                    cx.notify();
                }))
                .into_any_element()
        } else {
            let (t, r) = (id.clone(), id.clone());
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().id(("addon-switch-tip", i)).tooltip(tip(if enabled { "Turn off: stop using it to find streams" } else { "Turn on" })).child(switch(("addon-switch", i), enabled).on_click(cx.listener(move |this, on: &bool, _, cx| this.toggle(t.clone(), *on, cx)))))
                .child(icon_button(("addon-remove", i), glyphs::DELETE, "Remove addon").danger().on_click(cx.listener(move |this, _, _, cx| {
                    this.confirming = Some(r.clone());
                    cx.notify();
                })))
                .into_any_element()
        };
        div()
            .flex()
            .items_start()
            .gap(px(12.))
            .py(px(14.))
            .when(i > 0, |d| d.border_t_1().border_color(Tokens::border()))
            .child(logo)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .min_w_0()
                            .child(div().min_w_0().truncate().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(if enabled { Tokens::text_1() } else { Tokens::text_3() }).child(SharedString::from(m.name.clone())))
                            .child(tag(format!("v{}", m.version), Tone::Neutral))
                            .when(!enabled, |d| d.child(tag("Off", Tone::Neutral))),
                    )
                    .children(m.description.clone().filter(|t| !t.trim().is_empty()).map(|t| div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_2()).line_clamp(2).child(SharedString::from(t))))
                    .child(div().flex().flex_wrap().gap(px(6.)).children(caps.into_iter().filter(|c| c.0).map(|(_, g, l)| icon_tag(g, l, Tone::Neutral))))
                    .child(
                        div()
                            .id(("addon-url", i))
                            .text_size(px(size::XS))
                            .text_color(Tokens::text_3())
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis_middle()
                            .tooltip(tip(a.source_url.clone()))
                            .child(SharedString::from(a.source_url.clone())),
                    ),
            )
            .child(div().flex_none().pt(px(4.)).child(controls))
    }
}

impl Render for AddonsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let empty_url = self.url.read(cx).value().trim().is_empty();
        let install = btn("addon-install", if self.busy { "Installing…" } else { "Install" }, ButtonKind::Primary).icon(ph::DOWNLOAD_SIMPLE).loading(self.busy);
        let install = if empty_url && !self.busy { install.disabled("Paste a manifest URL first") } else { install.on_click(cx.listener(|this, _, w, cx| this.install(w, cx))) };
        let n = self.addons.len();
        let list = if !self.loaded {
            div().flex().flex_col().child(skeleton_row()).child(skeleton_row()).into_any_element()
        } else if n == 0 {
            empty_view(glyphs::SOURCES, "No addons yet", "Addons find the streams for movies, series and anime. Paste an addon's manifest URL above to install one.")
                .compact()
                .action(btn("addon-paste", "Paste a manifest URL", ButtonKind::Secondary).icon(ph::LINK_SIMPLE).on_click(cx.listener(|this, _, w, cx| this.focus_url(w, cx))))
                .into_any_element()
        } else {
            div().flex().flex_col().children(self.addons.iter().enumerate().map(|(i, a)| self.row(i, a, cx))).into_any_element()
        };
        div()
            .id("addons-panel")
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                setting_section("Install an addon", Some("Paste the manifest URL from the addon's website. It usually ends in manifest.json.".into()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .pt(px(6.))
                            .child(div().flex_1().child(Input::new(&self.url).cleanable(true).prefix(icon(ph::LINK_SIMPLE.bold(), Tokens::text_3(), isz::SM))))
                            .child(install),
                    )
                    .when_some(self.error.clone(), |d, e| d.child(div().pt(px(10.)).child(inline_alert(AlertKind::Error, "Could not install this addon", e).dismissible(cx.listener(|this, _, _, cx| {
                        this.error = None;
                        cx.notify();
                    }))))),
            )
            .child(setting_section(if self.loaded { format!("Installed ({n})") } else { "Installed".to_string() }, None).child(list))
    }
}
