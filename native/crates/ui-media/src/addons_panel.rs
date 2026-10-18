//! Settings -> Addons: installed Stremio addons, install by manifest URL, enable/disable, remove.
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_media::addons::InstalledAddon;
use mv_ui::{bg, button, empty_state, size, ButtonKind, Tokens};

use crate::common::{self, muted, registry};

pub struct AddonsPanel {
    addons: Vec<InstalledAddon>,
    url: Entity<InputState>,
    busy: bool,
    message: Option<(bool, String)>,
    _sub: Subscription,
}

impl AddonsPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let url = common::input(window, cx, "https://addon.example.com/manifest.json");
        let sub = cx.subscribe_in(&url, window, |this, _, ev: &InputEvent, w, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.install(w, cx);
            }
        });
        let mut s = Self { addons: Vec::new(), url, busy: false, message: None, _sub: sub };
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
        self.message = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let res = bg::run(async move { registry().install(&url).await.map_err(|e| e.to_string()) }).await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update_in(cx, |v, w, cx| {
                v.busy = false;
                match res {
                    Ok(a) => {
                        v.message = Some((true, format!("Installed {}", a.manifest.name)));
                        v.url.update(cx, |s, cx| s.set_value("", w, cx));
                        v.addons = registry().list();
                    }
                    Err(e) => v.message = Some((false, format!("Could not install: {e}"))),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn remove(&mut self, id: String, cx: &mut Context<Self>) {
        let res = registry().uninstall(&id);
        self.message = res.err().map(|e| (false, format!("Could not remove: {e}")));
        self.addons = registry().list();
        cx.notify();
    }

    fn toggle(&mut self, id: String, enabled: bool, cx: &mut Context<Self>) {
        let res = registry().set_enabled(&id, enabled);
        self.message = res.err().map(|e| (false, format!("Could not update: {e}")));
        self.addons = registry().list();
        cx.notify();
    }
}

impl Render for AddonsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.addons.iter().enumerate().map(|(i, a)| {
            let (id, enabled) = (a.manifest.id.clone(), a.enabled);
            let id2 = id.clone();
            let m = &a.manifest;
            let caps = [
                m.resources.iter().any(|r| r == "stream").then_some("streams"),
                m.resources.iter().any(|r| r == "catalog").then_some("catalogs"),
                m.resources.iter().any(|r| r == "subtitles").then_some("subtitles"),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", ");
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .p(px(12.))
                .rounded(px(10.))
                .bg(Tokens::surface())
                .border_1()
                .border_color(Tokens::border())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(size::MD))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if enabled { Tokens::text_1() } else { Tokens::text_3() })
                                .truncate()
                                .child(SharedString::from(format!("{}  v{}", m.name, m.version))),
                        )
                        .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).truncate().child(SharedString::from(
                            if caps.is_empty() { a.source_url.clone() } else { format!("{caps}  ·  {}", a.source_url) },
                        ))),
                )
                .child(
                    button(("addon-toggle", i), if enabled { "Enabled" } else { "Disabled" }, if enabled { ButtonKind::Secondary } else { ButtonKind::Ghost })
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle(id.clone(), !enabled, cx))),
                )
                .child(button(("addon-remove", i), "Remove", ButtonKind::Ghost).on_click(cx.listener(move |this, _, _, cx| this.remove(id2.clone(), cx))))
        });
        let msg = self.message.as_ref().map(|(ok, t)| {
            div().mt(px(8.)).text_size(px(size::SM)).text_color(if *ok { Tokens::accent() } else { Tokens::danger() }).child(SharedString::from(t.clone()))
        });
        div()
            .id("addons-panel")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Addons"))
            .child(muted("Stremio-compatible addons provide the streams for anime, movies and series. Paste a manifest URL to install one."))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .my(px(8.))
                    .child(div().flex_1().child(Input::new(&self.url)))
                    .child(button("addon-install", if self.busy { "Installing..." } else { "Install" }, ButtonKind::Primary).on_click(cx.listener(|this, _, w, cx| this.install(w, cx)))),
            )
            .children(msg)
            .children(if self.addons.is_empty() { None } else { Some(div().flex().flex_col().gap(px(8.)).children(rows)) })
            .when(self.addons.is_empty(), |d| d.child(empty_state("No addons installed", "Install a Stremio addon above to find streams.")))
    }
}
