//! "CLI tools" page (installed agent CLIs, versions, updates) and the update banner shown on AI pages.
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::radius;
use mv_ui::{btn, disclosure, inline_alert, ph, setting_row, setting_section, size, skeleton_line, status_tag, switch, AlertKind, ButtonKind, StatusKind, Tokens};

use crate::cli_updates::{tail_lines, CLIS};
use crate::ui::divided;
use crate::{SettingsView, Tab};

impl SettingsView {
    pub(crate) fn render_cli_tools(&self, cx: &mut Context<Self>) -> Div {
        let (checking, checked, auto, notify) = {
            let s = self.cli.read(cx);
            (s.checking, s.checked, s.auto_update, s.notify_updates)
        };
        let mut list = setting_section("Agent CLIs", Some("Command-line agents the assistant can use. Updates run in the background.".into()));
        for (n, s) in CLIS.iter().enumerate() {
            let info = self.cli.read(cx).info(s.id).cloned().unwrap_or_default();
            let hint: AnyElement = match (&info.installed, &info.latest) {
                (None, _) if !checked => skeleton_line(px(160.)).mt(px(4.)).into_any_element(),
                (None, _) => hint_text(format!("Not installed (`{}` was not found on PATH)", s.bin)),
                (Some(i), Some(l)) if info.update_available() => hint_text(format!("Installed {i} · {l} is available")),
                (Some(i), Some(_)) => hint_text(format!("Installed {i} · Up to date")),
                (Some(i), None) => hint_text(format!("Installed {i}")),
            };
            let id = s.id;
            let mut control = div().flex().items_center().gap(px(8.));
            // The last run's outcome as an icon chip; the raw output sits behind "Show log".
            if let Some((ok, _)) = &info.result {
                control = control.child(if *ok { status_tag(StatusKind::Done).label("Updated") } else { status_tag(StatusKind::Failed).label("Update failed") });
            }
            if info.installed.is_some() {
                let kind = if info.update_available() { ButtonKind::Primary } else { ButtonKind::Secondary };
                control = control.child(
                    btn(SharedString::from(format!("cli-update-{id}")), if info.busy { "Updating…" } else { "Update" }, kind)
                        .small()
                        .icon(ph::ARROW_CIRCLE_UP)
                        .loading(info.busy)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.cli.update(cx, |c, cx| c.run_update(id, false, cx));
                        })),
                );
            }
            let log = info.result.as_ref().map(|(_, text)| text.clone()).filter(|t| !t.trim().is_empty());
            let row = div()
                .flex()
                .flex_col()
                .py(px(12.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(24.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(12.))
                                .flex_1()
                                .min_w_0()
                                .child(div().flex_none().child(ui_assistant::provider_logo(s.provider, 20.)))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.))
                                        .flex_1()
                                        .min_w_0()
                                        .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(s.name))
                                        .child(hint),
                                ),
                        )
                        .child(div().flex().flex_none().items_center().child(control)),
                )
                .when_some(log, |d, text| {
                    let summary = tail_lines(&text);
                    d.child(
                        div().pl(px(28.)).pt(px(4.)).child(
                            disclosure(SharedString::from(format!("cli-log-{id}")), "Show log").trailing(div().max_w(px(320.)).truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(summary)).content(
                                div()
                                    .id(SharedString::from(format!("cli-log-box-{id}")))
                                    .max_h(px(160.))
                                    .overflow_y_scroll()
                                    .p(px(8.))
                                    .rounded(px(radius::SM))
                                    .bg(Tokens::well())
                                    .border_1()
                                    .border_color(Tokens::border())
                                    .font_family(mv_ui::MONO_FONT)
                                    .text_size(px(size::XS))
                                    .text_color(Tokens::text_2())
                                    .child(text),
                            ),
                        ),
                    )
                });
            list = list.child(if n == 0 { row } else { divided(row) });
        }

        let settings = setting_section("Updates", Some("Checked at start and every 6 hours.".into()))
            .child(setting_row(
                "Notify me when a CLI update is available",
                "Shows a banner on the AI pages and a dot on this page",
                switch("cli-notify", notify).on_click(cx.listener(|this, on: &bool, _, cx| {
                    this.cli.update(cx, |c, cx| c.set_notify_updates(*on, cx));
                })),
            ))
            .child(divided(setting_row(
                "Update CLIs automatically",
                "Installs new versions silently when they are found",
                switch("cli-auto", auto).on_click(cx.listener(|this, on: &bool, _, cx| {
                    this.cli.update(cx, |c, cx| c.set_auto_update(*on, cx));
                })),
            )))
            .child(divided(setting_row(
                "Check for updates",
                "Looks up the newest versions now",
                btn("cli-check", if checking { "Checking…" } else { "Check now" }, ButtonKind::Secondary)
                    .icon(ph::ARROWS_CLOCKWISE)
                    .loading(checking)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.cli.update(cx, |c, cx| c.refresh(cx));
                    })),
            )));

        div().flex().flex_col().gap(px(20.)).child(list).when_some(self.render_app_tools(cx), |d, s| d.child(s)).child(settings)
    }

    /// Opt-ins that let Grok and Antigravity load the app's tools (a `mangavox` entry in their own
    /// MCP settings). `None` while neither is installed.
    fn render_app_tools(&self, cx: &mut Context<Self>) -> Option<Div> {
        let here = std::env::current_exe().ok();
        let mut section = setting_section("App tools", Some("Claude Code, Codex, Gemini CLI and OpenCode load MangaVox's tools by themselves. These two need a one-time opt-in.".into()));
        let mut rows = 0;
        for s in CLIS.iter().filter(|s| mv_ai::providers::cli_mcp::bridged(s.provider)) {
            let info = self.cli.read(cx).info(s.id).cloned().unwrap_or_default();
            if info.installed.is_none() {
                continue;
            }
            let on = info.bridge.as_ref().is_some_and(|e| e.enabled);
            let file = match s.provider {
                mv_ai::AiProviderId::GrokCli => "~/.grok/config.toml",
                _ => "~/.gemini/config/mcp_config.json",
            };
            let mut hint = format!("Adds a \"mangavox\" MCP server entry to {}'s own settings ({file}); turning this off removes it.", s.name);
            if let Some(e) = info.bridge.as_ref().filter(|e| here.as_ref().is_some_and(|h| !same_path(h, &e.command))) {
                hint = format!("Uses another MangaVox copy ({}). Turn off and on again to use this one.", e.command);
            }
            if info.bridge_busy {
                hint = if on { "Removing the entry…".into() } else { "Adding the entry…".into() };
            }
            if let Some(err) = &info.bridge_error {
                hint = err.clone();
            }
            let id = s.id;
            let row = setting_row(
                format!("Let {} use MangaVox's tools", s.name),
                hint,
                switch(SharedString::from(format!("cli-bridge-{id}")), on).on_click(cx.listener(move |this, on: &bool, _, cx| {
                    let on = *on;
                    this.cli.update(cx, |c, cx| c.set_bridge(id, on, cx));
                })),
            );
            section = section.child(if rows == 0 { row } else { divided(row) });
            rows += 1;
        }
        (rows > 0).then_some(section)
    }

    /// Update banners at the top of the AI pages; empty elsewhere.
    pub(crate) fn render_cli_banners(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if !matches!(self.tab, Tab::Providers | Tab::Models | Tab::CliTools) {
            return Vec::new();
        }
        let banners = self.cli.read(cx).banners();
        banners
            .into_iter()
            .map(|(key, text)| {
                // "<cli>@<version>" keys are pending updates; the rest say an update finished.
                let pending = key.contains('@');
                let k = key.clone();
                let mut a = inline_alert(if pending { AlertKind::Info } else { AlertKind::Success }, text, "").dismissible(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let key = k.clone();
                    this.cli.update(cx, |c, cx| c.dismiss(&key, cx));
                }));
                if pending && self.tab != Tab::CliTools {
                    a = a.action("View", cx.listener(|this, _: &ClickEvent, window, cx| this.select_tab(Tab::CliTools, window, cx)));
                }
                a.into_any_element()
            })
            .collect()
    }
}

/// The running exe and the one an entry names, ignoring case and separators on Windows.
fn same_path(exe: &std::path::Path, named: &str) -> bool {
    let norm = |s: &str| if cfg!(windows) { s.replace('/', "\\").to_lowercase() } else { s.to_string() };
    norm(&exe.to_string_lossy()) == norm(named)
}

fn hint_text(text: String) -> AnyElement {
    div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(text).into_any_element()
}
