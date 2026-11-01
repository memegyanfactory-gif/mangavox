//! Tab bodies: General, Playback, Appearance, App mode, Studio and About. Every control applies
//! and saves as it changes (NN/g, toggle switches take immediate effect:
//! <https://www.nngroup.com/articles/toggle-switch-guidelines/>); text fields save when you leave
//! them, with a passive "Saved" mark and validation under the field.
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::select::Select;
use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::{icon as icon_px, radius, Palette, Preset, PresetGroup};
use mv_ui::{
    btn, glyphs, icon_button, ph, ph_icon, segmented, settings_block, settings_group, settings_note, settings_row, size, switch, AlertKind, ButtonKind, Interactive as _, Tokens, Tone, Weight,
};

use crate::ui::{icon_row, path_row};
use crate::{data, SettingsView};

/// Resolutions Studio renders (`StudioPrefs::resolution`).
const RESOLUTIONS: [&str; 4] = ["720p", "1080p", "1440p", "4k"];

/// Studio's default speaking-rate slider: 0.5× to 2×, in 0.05 steps.
pub fn speed_slider(_window: &mut Window, cx: &mut Context<SettingsView>) -> Entity<SliderState> {
    cx.new(|_| SliderState::new().min(0.5).max(2.0).step(0.05).default_value(1.0))
}

/// Slider value to the stored speed, rounded to the step so 1.0 stays exactly 1.0.
pub fn round_speed(v: f32) -> f64 {
    ((v as f64) * 20.0).round() / 20.0
}

/// Input with the "Saved" mark beside it. 280px wide, but it gives way on a narrow window
/// instead of pushing past the card.
fn field(state: &Entity<InputState>, saved: bool) -> Div {
    div().flex().items_center().justify_end().gap(px(8.)).min_w_0().when(saved, |d| d.child(crate::ui::saved_mark())).child(div().w(px(280.)).min_w(px(160.)).child(Input::new(state)))
}

/// Validation message under a field's label.
fn field_error(e: SharedString) -> Div {
    div()
        .flex()
        .items_start()
        .gap(px(4.))
        .text_size(px(size::XS))
        .line_height(px(16.))
        .text_color(Tokens::danger())
        .child(div().flex_none().pt(px(2.)).child(ph_icon(ph::WARNING_CIRCLE, Weight::Fill, Tokens::danger(), icon_px::XS)))
        .child(div().flex_1().min_w_0().child(e))
}

impl SettingsView {
    pub(crate) fn render_profile(&self, _cx: &mut Context<Self>) -> Div {
        div().flex().flex_col().gap(px(mv_ui::settings::SETTINGS_GROUP_GAP)).child(
            settings_group("Profile", Some("Shown across MangaVox. Changes save when you leave a field or press Enter.".into()))
                .child(settings_row("Display name", "How MangaVox greets you", field(&self.name, self.saved_now("name"))))
                .child(settings_row("Email", "Optional. Never sent anywhere", field(&self.email, self.saved_now("email"))).when_some(self.email_error.clone(), |r, e| r.below(field_error(e)))),
        )
    }

    pub(crate) fn render_playback(&self, cx: &mut Context<Self>) -> Div {
        let p = &self.playback;
        div()
            .flex()
            .flex_col()
            .gap(px(mv_ui::settings::SETTINGS_GROUP_GAP))
            .child(
                settings_group("Episodes", Some("What happens when an episode ends.".into()))
                    .child(settings_row(
                        "Auto-play next episode",
                        "Start the next episode when the current one ends",
                        switch("pb-autoplay", p.auto_play).on_click(cx.listener(|this, on: &bool, _, cx| {
                            this.playback.auto_play = *on;
                            this.save_playback(cx);
                        })),
                    ))
                    .child(
                        settings_row(
                            "Auto-play countdown",
                            if p.auto_play { "How long to wait before the next episode starts" } else { "Turn on auto-play to use a countdown" },
                            Select::new(&self.timer).w(px(120.)).disabled(!p.auto_play),
                        )
                        .muted(!p.auto_play),
                    )
                    .child(settings_row(
                        "Skip intro automatically",
                        "Jump past opening sequences when they are detected",
                        switch("pb-skip-intro", p.skip_intro).on_click(cx.listener(|this, on: &bool, _, cx| {
                            this.playback.skip_intro = *on;
                            this.save_playback(cx);
                        })),
                    ))
                    .child(settings_row(
                        "Pre-download next episode",
                        "Fetch the following episode in the background",
                        switch("pb-predownload", p.pre_download_next).on_click(cx.listener(|this, on: &bool, _, cx| {
                            this.playback.pre_download_next = *on;
                            this.save_playback(cx);
                        })),
                    )),
            )
            .child(
                settings_group("Defaults", Some("Used when a source offers several options.".into()))
                    .child(settings_row("Subtitle language", "Selected automatically when available", Select::new(&self.subtitle).w(px(180.))))
                    .child(settings_row("Preferred stream quality", "Streams at this quality are picked first", Select::new(&self.quality).w(px(180.)))),
            )
    }

    /// One theme preset: a miniature of the app in that preset's real colours (sidebar, a card,
    /// an accent button) and a strip of its swatches, so the choice can be judged before it is
    /// made. Apple HIG dark mode, preview appearance choices in place:
    /// <https://developer.apple.com/design/human-interface-guidelines/dark-mode>.
    fn preset_tile(&self, preset: &'static Preset, cx: &mut Context<Self>) -> Stateful<Div> {
        let pal = Palette::from_preset(preset);
        let active = self.preset == preset.id;
        let id = preset.id;
        let line = |w: f32, color: Rgba| div().h(px(4.)).w(relative(w)).rounded_full().bg(color);
        let sidebar = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(6.))
            .w(px(28.))
            .h_full()
            .pt(px(10.))
            .bg(pal.sidebar)
            .border_r_1()
            .border_color(pal.border)
            .child(div().size(px(10.)).rounded(px(radius::XS)).bg(pal.accent))
            .children((0..3).map(move |_| div().w(px(12.)).h(px(3.)).rounded_full().bg(pal.text_3)));
        let content = div()
            .flex()
            .flex_col()
            .flex_1()
            .gap(px(7.))
            .p(px(9.))
            .child(line(0.5, pal.text_1))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .justify_between()
                    .p(px(7.))
                    .rounded(px(radius::SM))
                    .bg(pal.surface)
                    .border_1()
                    .border_color(pal.border)
                    .child(div().flex().flex_col().gap(px(4.)).child(line(0.8, pal.text_2)).child(line(0.55, pal.text_3)))
                    .child(div().flex().items_center().gap(px(4.)).child(div().h(px(11.)).w(px(30.)).rounded_full().bg(pal.accent)).child(div().size(px(8.)).rounded_full().bg(pal.accent_2))),
            );
        let preview = div()
            .relative()
            .flex()
            .h(px(84.))
            .rounded(px(radius::MD))
            .overflow_hidden()
            .bg(pal.base)
            .border_1()
            .border_color(pal.border_strong)
            .child(sidebar)
            .child(content)
            .when(active, |d| d.child(div().absolute().top(px(6.)).right(px(6.)).child(mv_ui::selected_badge())));
        // Page, card, text, accent, second colour: the five colours a preset is made of.
        let swatches = div().flex().items_center().gap(px(3.)).children([pal.base, pal.surface, pal.text_1, pal.accent, pal.accent_2].map(|c| {
            div().size(px(10.)).rounded_full().bg(c).border_1().border_color(Tokens::border_strong())
        }));
        div()
            .id(id)
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(8.))
            .min_w_0()
            .rounded(px(radius::LG))
            .border_1()
            .border_color(if active { Tokens::accent() } else { Tokens::border() })
            .bg(if active { Tokens::accent_soft() } else { Tokens::surface() })
            .hover(move |s| s.border_color(if active { Tokens::accent() } else { Tokens::border_strong() }))
            .pressable()
            .role(Role::RadioButton)
            .aria_selected(active)
            .aria_label(preset.name)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_preset(id, cx)))
            .child(preview)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(2.))
                    .min_w_0()
                    .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(preset.name))
                    .when(preset.id == data::DEFAULT_PRESET, |d| d.child(mv_ui::tag("Default", Tone::Neutral)))
                    .child(swatches),
            )
            .child(div().px(px(2.)).h(px(32.)).text_size(px(size::XS)).line_height(px(16.)).text_color(Tokens::text_3()).line_clamp(2).text_ellipsis().child(preset.description))
    }

    pub(crate) fn render_appearance(&self, cx: &mut Context<Self>) -> Div {
        let mut page = div().flex().flex_col().gap(px(mv_ui::settings::SETTINGS_GROUP_GAP));
        for group in PresetGroup::ALL {
            let mut grid = div().grid().grid_cols(3).gap(px(12.));
            for preset in data::PRESETS.iter().filter(|p| p.group == group) {
                grid = grid.child(self.preset_tile(preset, cx));
            }
            let title = format!("{} themes", group.label());
            page = page.child(settings_group(title, Some(group.description().into())).bare().child(grid));
        }
        page.child(settings_group("Motion", None).child(settings_row(
            "Reduce motion",
            "Stops shimmer, fades and pops, and slows spinners. Windows' \"Animation effects\" setting is always respected.",
            switch("ap-reduce-motion", self.reduce_motion).on_click(cx.listener(|this, on: &bool, _, cx| this.set_reduce_motion(*on, cx))),
        )))
    }

    pub(crate) fn render_mode(&self, cx: &mut Context<Self>) -> Div {
        let studio = self.mode == "studio";
        let state = |on: bool| if on { mv_ui::tag("On", Tone::Success) } else { mv_ui::tag("Off", Tone::Neutral) };
        div().flex().flex_col().gap(px(mv_ui::settings::SETTINGS_GROUP_GAP)).child(
            settings_group("Workspace", Some("Changing modes never removes your library, downloads or settings.".into()))
                .child(settings_row(
                    "Studio mode",
                    "Off keeps MangaVox to manga, anime and movies. On adds the two tools below to the sidebar, plus Studio settings.",
                    switch("mode-studio", studio).on_click(cx.listener(|this, on: &bool, _, cx| this.set_mode(if *on { "studio" } else { "entertainment" }, cx))),
                ))
                .child(icon_row(glyphs::STUDIO, "Studio", "Reads manga chapters and turns them into narrated recap videos.", state(studio)))
                .child(icon_row(glyphs::ASSISTANT, "Assistant", "Chat with local or cloud AI, with tools that act on your library.", state(studio))),
        )
    }

    pub(crate) fn render_studio(&self, cx: &mut Context<Self>) -> Div {
        let p = &self.studio;
        // The live default narrator (`voices::default_voice`, what Audio and Studio narrate with),
        // not the copy read from prefs.json when Settings opened.
        let voice = mv_studio::tts::find_voice(self.narrator_id()).map(|v| v.label.to_string()).unwrap_or_else(|| "Not chosen yet".into());
        let res_ix = RESOLUTIONS.iter().position(|r| *r == p.resolution).unwrap_or(1);
        div()
            .flex()
            .flex_col()
            .gap(px(mv_ui::settings::SETTINGS_GROUP_GAP))
            .child(
                settings_group("Narration", Some("Defaults for new Studio projects.".into()))
                    .child(settings_row(
                        "Default voice",
                        voice,
                        btn("studio-voice", "Choose in Audio", ButtonKind::Secondary)
                            .icon(ph::WAVEFORM)
                            .trailing_icon(ph::CARET_RIGHT)
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.select_tab(crate::Tab::Audio, window, cx))),
                    ))
                    .child(settings_row(
                        "Speed",
                        "Speaking rate. 1.00× is normal.",
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .child(div().w(px(180.)).child(Slider::new(&self.studio_speed)))
                            .child(div().w(px(48.)).text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("{:.2}×", p.speed)))
                            .child(self.speed_preview(p.speed, cx)),
                    )),
            )
            .child(
                settings_group("Output", None).child(settings_row(
                    "Default resolution",
                    "Video size for exported projects",
                    segmented("studio-res", RESOLUTIONS, res_ix).on_change(cx.listener(|this, ix: &usize, _, cx| this.set_resolution(RESOLUTIONS[*ix], cx))),
                )),
            )
            .child(settings_note(AlertKind::Info, "Pick the AI model in Studio's model button; it uses the same model as the Assistant."))
    }

    pub(crate) fn render_about(&self, cx: &mut Context<Self>) -> Div {
        let version = env!("CARGO_PKG_VERSION");
        let data_dir = data::data_dir();
        let config = mv_core::paths::config();
        let reveal = |id: &'static str, path: std::path::PathBuf, folder: bool, cx: &mut Context<Self>| {
            icon_button(id, ph::FOLDER_OPEN, "Show in Explorer").on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if folder {
                    this.open_folder(path.clone(), cx);
                } else {
                    cx.reveal_path(&path);
                }
            }))
        };
        let hero = settings_block(
            div()
                .flex()
                .items_center()
                .gap(px(16.))
                .py(px(4.))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(px(52.))
                        .rounded(px(radius::XL))
                        .shadow(vec![BoxShadow { color: Tokens::accent_a(0.25).into(), offset: point(px(0.), px(4.)), blur_radius: px(18.), spread_radius: px(0.), inset: false }])
                        .child(mv_ui::app_logo(52.)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .flex_1()
                        .min_w_0()
                        .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("MangaVox"))
                        .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(format!("Version {version} · Made by Aayush Datta")))
                        .child(div().pt(px(4.)).text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_2()).child(
                            "Browse, read and watch, then turn manga into narrated audio and video. Processing runs locally on your machine.",
                        )),
                ),
        );
        div()
            .flex()
            .flex_col()
            .gap(px(mv_ui::settings::SETTINGS_GROUP_GAP))
            .child(settings_group("", None).child(hero).child(settings_block(div().flex().flex_col().min_w_0().child(mv_ui::update::about(cx)))))
            .child(settings_group("Setup", None).child(icon_row(
                ph::SPARKLE,
                "Welcome setup",
                "Choose sources, addons and voices again. Nothing changes until you confirm a screen.",
                btn("about-welcome", "Run setup again", ButtonKind::Secondary).icon(ph::ARROW_CLOCKWISE).on_click(|_, w, cx| mv_ui::navigate(mv_ui::Route::Welcome, w, cx)),
            )))
            .child(
                settings_group("Data", None)
                    .child(path_row(glyphs::FOLDER, "Data folder", "Move it in Storage", data_dir.display().to_string(), reveal("about-data", data_dir.clone(), true, cx)))
                    .child(path_row(ph::FILE_TEXT, "Config file", "", config.display().to_string(), reveal("about-config", config.clone(), false, cx))),
            )
            .child(
                settings_group("Links", None)
                    .child(icon_row(
                        ph::GITHUB_LOGO,
                        "Source code",
                        "github.com/memegyanfactory-gif/mangavox",
                        btn("about-repo", "View source", ButtonKind::Ghost).trailing_icon(ph::ARROW_UP_RIGHT).on_click(|_, _, cx| cx.open_url("https://github.com/memegyanfactory-gif/mangavox")),
                    ))
                    .child(icon_row(
                        ph::BUG,
                        "Report a problem",
                        "Opens the issue tracker in your browser",
                        btn("about-issues", "Report", ButtonKind::Ghost).trailing_icon(ph::ARROW_UP_RIGHT).on_click(|_, _, cx| cx.open_url(crate::HELP_URL)),
                    )),
            )
    }
}
