//! The Series mode (beta) switch on the Studio dashboard: stories written from then on are
//! made like a TV series (a smooth narrator who talks to the viewer, every character speaking
//! their own lines in their own cloned Chatterbox voice, acted with the scene's emotion). The
//! hook, intro, music and effects are made as always. See `mv_recap::series`.
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_recap::series::{Gender, VoiceCast};
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::{badge, icon_ph, ph, size, switch, tip, Tokens, Tone};

use crate::page::StudioPage;

/// What the switch does, for its tooltip and the card.
const WHAT: &str = "Stories written from now on play like a TV series: a smooth narrator talks to you, and every character speaks their own lines in their own voice, acted with the scene's emotion. Hooks, the intro, music and effects stay as they are.";
const NEEDS: &str = "Character voices need an Expressive (Chatterbox) narrator. Each character gets a voice of their own, cloned from one of 28 base voices at different pitches, so even a cast of 50 never shares one. Write the story again (Redo from Story) for chapters made before.";

impl StudioPage {
    /// The Series mode card. `manga`: the open manga, whose voice cast is listed while on.
    pub(crate) fn series_card(&mut self, manga: Option<&str>, cx: &mut Context<Self>) -> AnyElement {
        let on = self.prefs.series_beta;
        let voice = self.prefs.voice.clone().unwrap_or_default();
        let expressive = mv_studio::tts::is_expressive(&voice);
        let toggle = switch("series-beta", on).on_click(cx.listener(|this, on: &bool, _, cx| {
            let on = *on;
            this.prefs.series_beta = on;
            mv_recap::series::set_enabled(on);
            cx.notify();
        }));
        let title = div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .child(icon_ph(ph::FILM_STRIP, if on { Tokens::accent() } else { Tokens::text_2() }, icon_px::SM))
            .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Series mode"))
            .child(badge("BETA", Tone::Warning));
        let line = if on {
            "On · narrator talks to you, characters speak in their own voices"
        } else {
            "Make recaps like a TV series: narrator + a voice for every character"
        };
        let warn = (on && !expressive).then(|| {
            div()
                .text_size(px(size::XS))
                .text_color(Tokens::warning())
                .child("Pick an Expressive (Chatterbox) narrator voice: other voices read the characters' lines themselves.")
        });
        let cast = manga.filter(|_| on).map(|id| cast_list(&VoiceCast::load(id)));
        div()
            .id("series-card")
            .flex()
            .flex_col()
            .gap(px(space::XS))
            .p(px(space::MD))
            .rounded(px(radius::MD))
            .border_1()
            .border_color(if on { Tokens::accent_border() } else { Tokens::border() })
            .bg(if on { Tokens::accent_soft() } else { Tokens::surface() })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::MD))
                    .child(div().flex().flex_col().flex_1().min_w_0().gap(px(2.)).child(title).child(
                        div().id("series-line").truncate().text_size(px(size::XS)).text_color(Tokens::text_2()).tooltip(tip(format!("{WHAT}\n\n{NEEDS}"))).child(line),
                    ))
                    .child(toggle),
            )
            .children(warn)
            .children(cast)
            .into_any_element()
    }
}

/// The open manga's voice cast: who speaks with which voice.
fn cast_list(cast: &VoiceCast) -> AnyElement {
    if cast.voices.is_empty() {
        return div()
            .text_size(px(size::XS))
            .text_color(Tokens::text_3())
            .child("Voice cast: each character gets a voice the first time a Series story gives them a line (kept in voice_cast.json, which you can edit).")
            .into_any_element();
    }
    let n = cast.voices.len();
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(div().text_size(px(size::XS)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_2()).child(format!("Voice cast · {n} character{}", if n == 1 { "" } else { "s" })))
        .child(div().flex().flex_wrap().gap(px(4.)).children(cast.voices.iter().enumerate().map(|(i, v)| {
            let who = match v.gender {
                Gender::Female => "♀",
                Gender::Male => "♂",
            };
            let how = match &v.file {
                Some(f) => format!("your clip {}", f.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()),
                None => format!("{} {:+.1}", v.base, v.pitch),
            };
            div()
                .id(("series-cast", i))
                .flex()
                .items_center()
                .gap(px(4.))
                .h(px(22.))
                .px(px(8.))
                .rounded(px(radius::PILL))
                .bg(Tokens::surface_2())
                .text_size(px(size::XS))
                .text_color(Tokens::text_1())
                .tooltip(tip(format!("{} · voice {how}", v.name)))
                .child(format!("{who} {}", v.name))
                .when(v.locked, |d| d.child(icon_ph(ph::LOCK, Tokens::text_3(), icon_px::XS)))
        })))
        .into_any_element()
}
