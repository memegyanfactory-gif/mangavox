//! Tab bodies and the small layout helpers they share.
use gpui_kit::component::input::Input;
use gpui_kit::component::select::Select;
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::{button, size, ButtonKind, Tokens};

use crate::ui::{card, divided, format_bytes, row};
use crate::{data, Notice, SettingsView};

/// Input on the right of a setting row.
fn input_box(state: &Entity<gpui_kit::component::input::InputState>) -> Div {
    div().w(px(280.)).child(Input::new(state))
}

/// Right-aligned primary action under a section.
fn actions(child: impl IntoElement) -> Div {
    div().flex().justify_end().child(child)
}

pub fn notice_bar(n: &Notice) -> Div {
    div()
        .px(px(12.))
        .py(px(8.))
        .rounded(px(8.))
        .bg(Tokens::surface_2())
        .border_1()
        .border_color(Tokens::border())
        .text_size(px(size::SM))
        .text_color(if n.error { Tokens::danger() } else { Tokens::text_2() })
        .child(n.text.clone())
}

impl SettingsView {
    pub(crate) fn render_profile(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                card("Profile", Some("Shown across MangaVox."))
                    .child(row("Display name", "How MangaVox greets you", input_box(&self.name)))
                    .child(divided(row("Email", "Optional. Never sent anywhere", input_box(&self.email)))),
            )
            .child(actions(
                button("profile-save", "Save changes", ButtonKind::Primary)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.save_profile(cx))),
            ))
    }

    pub(crate) fn render_playback(&self, cx: &mut Context<Self>) -> Div {
        let p = &self.playback;
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(
                card("Episodes", Some("What happens when an episode ends."))
                    .child(row(
                        "Auto-play next episode",
                        "Start the next episode when the current one ends",
                        Switch::new("pb-autoplay")
                            .checked(p.auto_play)
                            .on_change(cx.listener(|this, on: &bool, _, cx| {
                                this.playback.auto_play = *on;
                                cx.notify();
                            })),
                    ))
                    .child(divided(row(
                        "Auto-play countdown",
                        "How long to wait before the next episode starts",
                        Select::new(&self.timer).w(px(120.)),
                    )))
                    .child(divided(row(
                        "Skip intro automatically",
                        "Jump past opening sequences when they are detected",
                        Switch::new("pb-skip-intro")
                            .checked(p.skip_intro)
                            .on_change(cx.listener(|this, on: &bool, _, cx| {
                                this.playback.skip_intro = *on;
                                cx.notify();
                            })),
                    )))
                    .child(divided(row(
                        "Pre-download next episode",
                        "Fetch the following episode in the background",
                        Switch::new("pb-predownload")
                            .checked(p.pre_download_next)
                            .on_change(cx.listener(|this, on: &bool, _, cx| {
                                this.playback.pre_download_next = *on;
                                cx.notify();
                            })),
                    ))),
            )
            .child(
                card("Defaults", Some("Used when a source offers several options."))
                    .child(row(
                        "Default subtitle language",
                        "Selected automatically when available",
                        Select::new(&self.subtitle).w(px(180.)),
                    ))
                    .child(divided(row(
                        "Preferred stream quality",
                        "Auto-select streams at this quality when available",
                        Select::new(&self.quality).w(px(120.)),
                    ))),
            )
            .child(actions(
                button("playback-save", "Save changes", ButtonKind::Primary)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.save_playback(cx))),
            ))
    }

    pub(crate) fn render_appearance(&self, cx: &mut Context<Self>) -> Div {
        let mut grid = div().flex().flex_wrap().gap(px(12.));
        for preset in data::PRESETS {
            let active = self.preset == preset.id;
            let id = preset.id;
            grid = grid.child(
                div()
                    .id(id)
                    .w(px(216.))
                    .p(px(10.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(if active { Tokens::accent() } else { Tokens::border() })
                    .bg(rgb(preset.base))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_preset(id, cx)))
                    .child(
                        div()
                            .h(px(48.))
                            .mb(px(10.))
                            .rounded(px(6.))
                            .flex()
                            .items_end()
                            .gap(px(6.))
                            .p(px(8.))
                            .child(div().size(px(14.)).rounded_full().bg(rgb(preset.accent)))
                            .child(div().size(px(14.)).rounded_full().bg(rgb(preset.secondary))),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(preset.name))
                            .when(preset.id == data::DEFAULT_PRESET, |d| {
                                d.child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child("Default"))
                            })
                            .when(active, |d| d.child(div().ml_auto().text_size(px(size::SM)).text_color(Tokens::accent()).child("Active"))),
                    )
                    .child(div().mt(px(2.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(preset.description)),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(card("Theme", Some("Each preset changes the whole palette.")).child(div().pt(px(6.)).pb(px(14.)).child(grid)))
            .child(card("Motion", None).child(row(
                "Reduce ambient motion",
                "Slows background animation further. The system reduced-motion setting is always respected.",
                Switch::new("ap-reduce-motion")
                    .checked(self.reduce_motion)
                    .on_change(cx.listener(|this, on: &bool, _, cx| this.set_reduce_motion(*on, cx))),
            )))
    }

    pub(crate) fn render_mode(&self, cx: &mut Context<Self>) -> Div {
        let studio = self.mode == "studio";
        div().flex().flex_col().gap(px(16.)).child(
            card("Workspace", Some("Changing modes never removes your library, downloads, or settings.")).child(row(
                "Studio mode",
                "Adds the Studio workspace and the Assistant to the sidebar, plus Studio settings. Off keeps the app to manga, anime and movies.",
                Switch::new("mode-studio")
                    .checked(studio)
                    .on_change(cx.listener(|this, on: &bool, _, cx| this.set_mode(if *on { "studio" } else { "entertainment" }, cx))),
            )),
        )
    }

    pub(crate) fn render_studio(&self, cx: &mut Context<Self>) -> Div {
        let p = &self.studio;
        let res_btn = |id: &'static str, v: &'static str, cx: &mut Context<Self>| {
            let kind = if p.resolution == v { ButtonKind::Primary } else { ButtonKind::Secondary };
            button(id, v, kind).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_resolution(v, cx)))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                card("Narration", Some("Defaults for new Studio projects."))
                    .child(row("Default voice", "Voice name used for narration", div().w(px(280.)).child(Input::new(&self.studio_voice))))
                    .child(divided(row("Speed", "Speaking rate, 1.0 is normal", div().w(px(120.)).child(Input::new(&self.studio_speed))))),
            )
            .child(
                card("Output", None).child(row(
                    "Default resolution",
                    "Video size for exported projects",
                    div().flex().gap(px(6.)).child(res_btn("studio-720", "720p", cx)).child(res_btn("studio-1080", "1080p", cx)),
                )),
            )
            .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(
                "Pick the AI model in Studio's model button; it uses the same model as the Assistant.",
            ))
            .child(actions(
                button("studio-save", "Save changes", ButtonKind::Primary)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.save_studio(cx))),
            ))
    }

    pub(crate) fn render_storage(&self, cx: &mut Context<Self>) -> Div {
        let mut folders = card(
            "Download folders",
            Some("Where each kind of download is saved. Changes apply to new downloads; existing files stay where they are."),
        );
        for (idx, (kind, label, hint)) in data::STORAGE_KINDS.iter().enumerate() {
            let custom = self.storage.get(idx).cloned().unwrap_or_default();
            let effective = if custom.is_empty() {
                data::storage_default(kind)
            } else {
                std::path::PathBuf::from(&custom)
            };
            let shown = effective.display().to_string();
            let open_path = effective.clone();
            let mut buttons = div()
                .flex()
                .items_center()
                .gap(px(6.))
                .flex_none()
                .child(
                    button(("storage-open", idx), "Open", ButtonKind::Ghost)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_folder(open_path.clone(), cx))),
                )
                .child(
                    button(("storage-change", idx), "Change…", ButtonKind::Secondary)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.pick_storage(idx, cx))),
                );
            if !custom.is_empty() {
                buttons = buttons.child(
                    button(("storage-reset", idx), "Reset", ButtonKind::Ghost)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.reset_storage(idx, cx))),
                );
            }
            let entry = div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(16.))
                .py(px(12.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .flex_1()
                        .min_w_0()
                        .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(*label))
                        .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(*hint))
                        .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).truncate().child(shown)),
                )
                .child(buttons);
            folders = folders.child(if idx == 0 { entry } else { divided(entry) });
        }

        let cache_text = match self.cache_bytes {
            Some(b) => format_bytes(b),
            None => "Calculating…".to_string(),
        };
        let cache = card(
            "Cache",
            Some("Cover images and other cached data. Your library and downloads are not affected."),
        )
        .child(row(
            "Cache size",
            &cache_text,
            div()
                .flex()
                .gap(px(6.))
                .child(
                    button("cache-open", "Open folder", ButtonKind::Ghost)
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.open_folder(data::cache_dir(), cx))),
                )
                .child(
                    button("cache-clear", "Clear cache", ButtonKind::Secondary)
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.clear_cache(cx))),
                ),
        ));

        div().flex().flex_col().gap(px(20.)).child(self.storage_mgr.clone()).child(folders).child(cache)
    }

    pub(crate) fn render_about(&self, cx: &mut Context<Self>) -> Div {
        let version = env!("CARGO_PKG_VERSION");
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .pb(px(6.))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(px(44.))
                            .rounded(px(12.))
                            .bg(Tokens::accent_soft())
                            .text_size(px(size::LG))
                            .font_weight(FontWeight::BOLD)
                            .text_color(Tokens::accent())
                            .child("MV"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("MangaVox"))
                            .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(format!("Version {version} · Made by Aayush Datta")))
                            .child(div().pt(px(4.)).text_size(px(size::SM)).text_color(Tokens::text_2()).child(
                                "Browse, read and watch, then turn manga into narrated audio and video. Processing runs locally on your machine.",
                            )),
                    ),
            )
            .child(card("Updates", None).child(mv_ui::update::about(cx)))
            .child(
                card("Data", None)
                    .child(row(
                        "Data folder",
                        &data::data_dir().display().to_string(),
                        button("about-data", "Open folder", ButtonKind::Secondary)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.open_folder(data::data_dir(), cx))),
                    ))
                    .child(divided(row(
                        "Config file",
                        &mv_core::paths::config().display().to_string(),
                        button("about-config", "Show", ButtonKind::Ghost).on_click(|_, _, cx| cx.reveal_path(&mv_core::paths::config())),
                    ))),
            )
            .child(
                card("Links", None)
                    .child(row(
                        "Source code",
                        "github.com/memegyanfactory-gif/mangavox",
                        button("about-repo", "Open", ButtonKind::Ghost).on_click(|_, _, cx| cx.open_url("https://github.com/memegyanfactory-gif/mangavox")),
                    ))
                    .child(divided(row(
                        "Report a problem",
                        "Opens the issue tracker in your browser",
                        button("about-issues", "Open", ButtonKind::Ghost).on_click(|_, _, cx| cx.open_url(crate::HELP_URL)),
                    ))),
            )
    }
}
