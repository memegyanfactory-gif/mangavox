//! "CLI tools" page (installed agent CLIs, versions, updates) and the update banner shown on AI pages.
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::{button, size, ButtonKind, Tokens};

use crate::cli_updates::CLIS;
use crate::ui::{card, divided, pill, row};
use crate::{Tab, SettingsView};

impl SettingsView {
    pub(crate) fn render_cli_tools(&self, cx: &mut Context<Self>) -> Div {
        let (checking, checked, auto, notify) = {
            let s = self.cli.read(cx);
            (s.checking, s.checked, s.auto_update, s.notify_updates)
        };
        let mut list = card("Agent CLIs", Some("Command-line agents the assistant can use. Updates run in the background."));
        for (n, s) in CLIS.iter().enumerate() {
            let info = self.cli.read(cx).info(s.id).cloned().unwrap_or_default();
            let hint = match (&info.installed, &info.latest) {
                (None, _) if !checked => "Checking…".to_string(),
                (None, _) => format!("Not installed (`{}` was not found on PATH)", s.bin),
                (Some(i), Some(l)) if info.update_available() => format!("Installed {i} · Latest {l}"),
                (Some(i), Some(_)) => format!("Installed {i} · Up to date"),
                (Some(i), None) => format!("Installed {i}"),
            };
            let hint = match &info.result {
                Some((_, text)) => format!("{hint}\n{text}"),
                None => hint,
            };
            let mut control = div().flex().items_center().gap(px(8.));
            if info.update_available() {
                control = control.child(pill(Tokens::accent().into(), "Update available"));
            }
            if info.installed.is_some() {
                let id = s.id;
                let label = if info.busy { "Updating…" } else { "Update" };
                let kind = if info.update_available() { ButtonKind::Primary } else { ButtonKind::Secondary };
                control = control.child(
                    button(SharedString::from(format!("cli-update-{id}")), label, kind)
                        .when(info.busy, |b| b.opacity(0.6))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.cli.update(cx, |c, cx| c.run_update(id, false, cx));
                        })),
                );
            }
            let label = div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(div().flex_none().child(ui_assistant::provider_logo(s.provider, 16.)))
                .child(s.name);
            let r = row_with_logo(label, &hint, control, info.result.as_ref().is_some_and(|(ok, _)| !*ok));
            list = list.child(if n == 0 { r } else { divided(r) });
        }

        let check_label = if checking { "Checking…" } else { "Check now" };
        let settings = card("Updates", Some("Checked at start and every 6 hours."))
            .child(row(
                "Notify me when a CLI update is available",
                "Shows a banner on the AI pages and a dot on this page",
                Switch::new("cli-notify").checked(notify).on_change(cx.listener(|this, on: &bool, _, cx| {
                    this.cli.update(cx, |c, cx| c.set_notify_updates(*on, cx));
                })),
            ))
            .child(divided(row(
                "Update CLIs automatically",
                "Installs new versions silently when they are found",
                Switch::new("cli-auto").checked(auto).on_change(cx.listener(|this, on: &bool, _, cx| {
                    this.cli.update(cx, |c, cx| c.set_auto_update(*on, cx));
                })),
            )))
            .child(divided(row(
                "Check for updates",
                "Looks up the newest versions now",
                button("cli-check", check_label, ButtonKind::Secondary)
                    .when(checking, |b| b.opacity(0.6))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.cli.update(cx, |c, cx| c.refresh(cx));
                    })),
            )));

        div().flex().flex_col().gap(px(20.)).child(list).child(settings)
    }

    /// Update banners at the top of the AI pages; empty elsewhere.
    pub(crate) fn render_cli_banners(&self, cx: &mut Context<Self>) -> Vec<Div> {
        if !matches!(self.tab, Tab::Providers | Tab::Models | Tab::CliTools) {
            return Vec::new();
        }
        let banners = self.cli.read(cx).banners();
        banners
            .into_iter()
            .enumerate()
            .map(|(i, (key, text))| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .bg(Tokens::surface_2())
                    .border_1()
                    .border_color(Tokens::accent())
                    .text_size(px(size::SM))
                    .text_color(Tokens::text_1())
                    .child(div().flex_1().min_w_0().child(text))
                    .when(self.tab != Tab::CliTools, |d| {
                        d.child(button(SharedString::from(format!("cli-banner-view-{i}")), "View", ButtonKind::Secondary).on_click(
                            cx.listener(|this, _: &ClickEvent, window, cx| this.select_tab(Tab::CliTools, window, cx)),
                        ))
                    })
                    .child(button(SharedString::from(format!("cli-banner-dismiss-{i}")), "Dismiss", ButtonKind::Ghost).on_click(cx.listener(
                        move |this, _: &ClickEvent, _, cx| {
                            let key = key.clone();
                            this.cli.update(cx, |c, cx| c.dismiss(&key, cx));
                        },
                    )))
            })
            .collect()
    }
}

/// Like `ui::row` but with a custom label element and an optional error-coloured hint.
fn row_with_logo(label: impl IntoElement, hint: &str, control: impl IntoElement, error: bool) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(24.))
        .py(px(12.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .flex_1()
                .min_w_0()
                .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(label))
                .child(div().text_size(px(size::SM)).text_color(if error { Tokens::danger() } else { Tokens::text_3() }).child(hint.to_string())),
        )
        .child(div().flex().items_center().gap(px(6.)).flex_none().child(control))
}
