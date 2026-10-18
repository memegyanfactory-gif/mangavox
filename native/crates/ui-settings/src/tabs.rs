//! Tab bodies and the small layout helpers they share.
use gpui_kit::component::input::Input;
use gpui_kit::component::select::Select;
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::{button, size, ButtonKind, Tokens};

use crate::{data, Notice, SettingsView};

/// Bordered panel with a title and optional muted description; callers append rows.
fn card(title: &str, desc: Option<&str>) -> Div {
    let mut head = div().flex().flex_col().gap(px(4.)).child(
        div()
            .text_size(px(size::LG))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(Tokens::text_1())
            .child(title.to_string()),
    );
    if let Some(d) = desc {
        head = head.child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(d.to_string()));
    }
    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .p(px(20.))
        .rounded(px(12.))
        .bg(Tokens::surface())
        .border_1()
        .border_color(Tokens::border())
        .child(head)
}

/// Label and muted hint on the left, control on the right.
fn row(label: &str, hint: &str, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .flex_1()
                .min_w_0()
                .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(label.to_string()))
                .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(hint.to_string())),
        )
        .child(control)
}

/// Thin divider between rows inside a card.
fn divided(row: Div) -> Div {
    row.pt(px(14.)).border_t_1().border_color(Tokens::border())
}

fn field(label: &str, control: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(div().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_2()).child(label.to_string()))
        .child(control)
}

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
            .gap(px(20.))
            .child(
                card("Your profile", Some("Shown across MangaVox. Stored locally in your config file."))
                    .child(field("Display name", Input::new(&self.name)))
                    .child(field("Email", Input::new(&self.email)))
                    .child(actions(
                        button("profile-save", "Save profile", ButtonKind::Primary)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.save_profile(cx))),
                    )),
            )
    }

    pub(crate) fn render_playback(&self, cx: &mut Context<Self>) -> Div {
        let p = &self.playback;
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(
                card("Playback", None)
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
                button("playback-save", "Save playback settings", ButtonKind::Primary)
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
                    .w(px(236.))
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
            .child(card("Theme", Some("Each preset changes the whole palette immediately.")).child(grid))
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
        let option = |id: &'static str, title: &'static str, badge: &'static str, desc: &'static str, active: bool, mode: &'static str| {
            div()
                .id(id)
                .flex_1()
                .p(px(18.))
                .rounded(px(12.))
                .bg(Tokens::surface())
                .border_1()
                .border_color(if active { Tokens::accent() } else { Tokens::border() })
                .cursor_pointer()
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_mode(mode, cx)))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title))
                        .when(active, |d| d.child(div().text_size(px(size::SM)).text_color(Tokens::accent()).child("Active"))),
                )
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(badge.to_uppercase()))
                .child(div().mt(px(4.)).text_size(px(size::SM)).text_color(Tokens::text_2()).child(desc))
        };

        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(
                "Switch at any time. Changing modes never removes your library, downloads, or settings.",
            ))
            .child(
                div()
                    .flex()
                    .gap(px(12.))
                    .child(option(
                        "mode-entertainment",
                        "MangaVox",
                        "Entertainment",
                        "Manga, anime, movies, reader, library, favorites, history, downloads, subtitles, and themes. Assistant and production controls stay hidden.",
                        !studio,
                        "entertainment",
                    ))
                    .child(option(
                        "mode-studio",
                        "Manga Studio",
                        "Full workspace",
                        "Unlocks Assistant, Text, Audio, Video, Dashboard, Pipeline, AI Models, and production controls.",
                        studio,
                        "studio",
                    )),
            )
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

        div().flex().flex_col().gap(px(20.)).child(folders).child(cache)
    }

    pub(crate) fn render_about(&self, cx: &mut Context<Self>) -> Div {
        let version = env!("CARGO_PKG_VERSION");
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(
                card("MangaVox", None)
                    .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(format!("Version {version}")))
                    .child(
                        div()
                            .text_size(px(size::SM))
                            .text_color(Tokens::text_2())
                            .child("Manga-to-voice platform: browse, read, and convert manga to audio and video. Processing runs locally on your machine."),
                    ),
            )
            .child(
                card("Data", None).child(row(
                    "Data folder",
                    &data::data_dir().display().to_string(),
                    button("about-data", "Open folder", ButtonKind::Secondary)
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.open_folder(data::data_dir(), cx))),
                )),
            )
            .child(
                card("Links", None).child(row(
                    "Source repository",
                    "github.com/memegyanfactory-gif/mangavox",
                    button("about-repo", "Open", ButtonKind::Ghost).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                        cx.open_url("https://github.com/memegyanfactory-gif/mangavox");
                    })),
                )),
            )
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
