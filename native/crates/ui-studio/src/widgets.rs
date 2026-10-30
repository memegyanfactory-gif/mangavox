//! Studio's own building blocks on top of the mv-ui kit: one icon per phase, scene kind, story
//! feel and narrator delivery (so a meaning has one glyph everywhere in Studio), the card and
//! column surfaces, the editor frame with a visible focus ring, the side-panel tab bar and the
//! reset confirmation that lists what goes. Candidates for mv-ui are listed in
//! `native/docs/design/studio.md`.
use std::rc::Rc;
use std::time::SystemTime;

use gpui_kit::component::WindowExt as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::{duration, easing, icon as icon_px, radius, ramp, space};
use mv_ui::{btn, glyphs, icon_ph, inline_alert, ph, ph_icon, pulse_dot, size, tip, AlertKind, ButtonKind, Hue, Interactive as _, Ph, RampExt as _, Tokens, Weight};

use crate::page::Phase;

// ---------------------------------------------------------------- meanings → glyphs

/// One glyph per Studio phase (Carbon progress indicator: each step names its stage).
pub(crate) fn phase_icon(p: Phase) -> Ph {
    match p {
        Phase::Read => glyphs::READ,
        Phase::Transcript => ph::FILE_TEXT,
        Phase::Story => glyphs::STORY,
        Phase::Audio => glyphs::NARRATE,
        Phase::Music => glyphs::MUSIC,
        Phase::Video => glyphs::VIDEO,
    }
}

/// Icon and hue of a scene kind (`mv_recap::Scene::kind`), so kinds never rest on colour alone
/// (WCAG 1.4.1).
pub(crate) fn kind_style(kind: &str) -> (Ph, Hue) {
    match kind {
        "fight" => (ph::SWORD, Hue::Rose),
        "action" => (ph::LIGHTNING, Hue::Orange),
        "dialogue" => (ph::CHAT_CIRCLE, Hue::Slate),
        "emotional" => (ph::MASK_SAD, Hue::Violet),
        "comedy" => (ph::SMILEY, Hue::Amber),
        "reveal" => (ph::EYE, Hue::Blue),
        "calm" => (ph::CLOUD, Hue::Green),
        "flashback" => (ph::CLOCK_COUNTER_CLOCKWISE, Hue::Cyan),
        "romance" => (ph::HEART, Hue::Rose),
        "horror" => (ph::GHOST, Hue::Violet),
        _ => (ph::MASK_HAPPY, Hue::Slate),
    }
}

/// Icon for a story feel (`mv_recap::STYLES`).
pub(crate) fn feel_icon(style: &str) -> Ph {
    match style {
        "Happy" => ph::SMILEY,
        "Warm" => ph::HEART,
        "Cinematic" => ph::FILM_SLATE,
        "Normal narration" => ph::MICROPHONE,
        "Classic" => ph::BOOK_OPEN,
        "Old-timer" => ph::CLOCK_COUNTER_CLOCKWISE,
        "Charm" => ph::SPARKLE,
        "Exotic" => ph::GLOBE,
        "Depth" => ph::MOUNTAINS,
        "Hard" => ph::FIRE,
        "Soft" => ph::FEATHER,
        _ => ph::MASK_HAPPY,
    }
}

/// Narrator directions offered per story passage: (label, scene kind, intensity 0-10, icon).
/// ElevenLabs v3 audio tags as presets: <https://elevenlabs.io/blog/v3-audiotags>
pub(crate) const DELIVERIES: &[(&str, &str, u8, Ph)] = &[
    ("Calm", "calm", 2, ph::CLOUD),
    ("Neutral", "dialogue", 4, ph::CHAT_CIRCLE),
    ("Warm", "romance", 5, ph::HEART),
    ("Happy", "comedy", 6, ph::SMILEY),
    ("Sad", "emotional", 6, ph::MASK_SAD),
    ("Memory", "flashback", 4, ph::CLOCK_COUNTER_CLOCKWISE),
    ("Tense", "horror", 7, ph::GHOST),
    ("Reveal", "reveal", 8, ph::EYE),
    ("Excited", "action", 8, ph::LIGHTNING),
    ("Intense", "fight", 9, ph::FIRE),
];

/// The preset a delivery matches, if any.
pub(crate) fn delivery_preset(d: &mv_recap::Delivery) -> Option<&'static (&'static str, &'static str, u8, Ph)> {
    DELIVERIES.iter().find(|(_, k, i, _)| *k == d.kind && *i == d.intensity)
}

// ---------------------------------------------------------------- surfaces

/// A content card: surface fill, hairline, large radius, 16px padding.
pub(crate) fn card() -> Div {
    div().flex().flex_col().gap(px(space::MD)).p(px(space::LG)).rounded(px(radius::LG)).bg(Tokens::surface()).border_1().border_color(Tokens::border())
}

/// A Studio column: rounded card with a soft light edge along its top.
pub(crate) fn column(d: Div) -> Div {
    d.relative()
        .rounded(px(radius::LG))
        .overflow_hidden()
        .bg(Tokens::sidebar())
        .border_1()
        .border_color(Tokens::border())
        .child(div().absolute().top_0().left(px(space::LG)).right(px(space::LG)).h(px(1.)).bg(Tokens::glass_border()))
}

/// Frame around an inline editor: a hover border, and the 2px focus ring while it has focus
/// (WCAG 2.4.7: <https://www.w3.org/WAI/WCAG22/Understanding/focus-visible.html>).
pub(crate) fn field_frame(id: impl Into<ElementId>, focused: bool) -> Stateful<Div> {
    div()
        .id(id)
        .rounded(px(radius::MD))
        .bg(Tokens::surface())
        .border_1()
        .p(px(space::XS))
        .map(|d| if focused { d.border_color(Tokens::focus_ring()).shadow(mv_ui::focus_ring_shadow()) } else { d.border_color(Tokens::border()).hover(|s| s.border_color(Tokens::border_strong())) })
}

/// Small metadata pill: icon plus text, quiet (stats next to a title).
pub(crate) fn stat_pill(icon: Ph, text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(space::XS))
        .h(px(22.))
        .px(px(space::SM))
        .rounded(px(radius::PILL))
        .bg(Tokens::hover())
        .text_size(px(size::XS))
        .text_color(Tokens::text_2())
        .whitespace_nowrap()
        .child(icon_ph(icon, Tokens::text_3(), icon_px::XS))
        .child(text.into())
}

/// Scene tag: the kind's icon and tonal hue, the mood, and a 5-bar intensity meter with its
/// value in the tooltip.
pub(crate) fn scene_tag(id: impl Into<ElementId>, s: &mv_recap::Scene) -> Stateful<Div> {
    let (icon, hue) = kind_style(&s.kind);
    let color = Tokens::hue(hue);
    let label = if s.mood.is_empty() { s.kind.clone() } else { format!("{} · {}", s.kind, s.mood) };
    let tip_text = if s.intensity > 0 { format!("{label} · intensity {}/10", s.intensity.min(10)) } else { label.clone() };
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(5.))
        .h(px(20.))
        .px(px(6.))
        .rounded(px(radius::XS))
        .bg(Tokens::hue_soft(hue))
        .text_size(px(size::XS))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .tooltip(tip(tip_text))
        .child(icon_ph(icon, color, icon_px::XS))
        .child(div().truncate().child(label))
        .when(s.intensity > 0, |d| {
            d.child(div().flex().gap(px(1.)).children((0..5).map(|i| div().w(px(3.)).h(px(8.)).rounded(px(1.)).bg(if (i as u8) * 2 < s.intensity { color } else { Tokens::active() }))))
        })
}

// ---------------------------------------------------------------- foldable section

/// Header row of a section that folds away: a caret that turns as it opens, the section's icon
/// and title, and a one-line summary (so a folded section still says what it holds). The caller
/// adds `on_click` and shows the body while `open`.
/// NN/g accordions: <https://www.nngroup.com/articles/accordions-on-desktop/>
pub(crate) fn fold_head(id: &'static str, icon: Ph, title: &'static str, summary: impl Into<SharedString>, open: bool) -> Stateful<Div> {
    let (from, to) = if open { (0., 0.25) } else { (0.25, 0.) };
    let caret = icon_ph(ph::CARET_RIGHT, Tokens::text_3(), icon_px::SM);
    // Keyed by the state, so the turn plays once each time it flips.
    let caret = if mv_ui::motion::reduced() {
        caret.with_transformation(Transformation::rotate(percentage(to))).into_any_element()
    } else {
        caret
            .with_animation(ElementId::NamedInteger(SharedString::from(format!("{id}-caret")), open as u64), Animation::new(duration::BASE).with_easing(easing::standard), move |s, t| {
                s.with_transformation(Transformation::rotate(percentage(from + (to - from) * t)))
            })
            .into_any_element()
    };
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(space::SM))
        .h(px(30.))
        .px(px(space::SM))
        .rounded(px(radius::MD))
        .role(Role::Button)
        .aria_expanded(open)
        .aria_label(SharedString::from(title))
        .pressable()
        .hover(|s| s.bg(Tokens::hover()))
        .child(caret)
        .child(icon_ph(icon, if open { Tokens::accent() } else { Tokens::text_2() }, icon_px::SM))
        .child(div().flex_none().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title))
        .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).text_color(Tokens::text_3()).child(summary.into()))
}

// ---------------------------------------------------------------- tab bar

/// Segmented tab bar with icons (fill weight on the current tab) and an optional live dot, for
/// panels whose tabs can be busy in the background (Fluent 2 TabList, filled icon = selected).
pub(crate) fn tab_bar<T: Copy + PartialEq + 'static>(
    id: &'static str,
    tabs: &[(T, &'static str, Ph, bool)],
    current: T,
    on_pick: impl Fn(&T, &mut Window, &mut App) + 'static,
) -> Div {
    let on_pick = Rc::new(on_pick);
    div().flex().items_center().gap(px(2.)).p(px(3.)).rounded(px(radius::MD)).bg(Tokens::well()).border_1().border_color(Tokens::border()).children(tabs.iter().enumerate().map(
        |(i, &(t, label, icon, live))| {
            let on = t == current;
            let f = on_pick.clone();
            let fg = if on { Tokens::text_1() } else { Tokens::text_2() };
            div()
                .id((id, i))
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .h(px(28.))
                .rounded(px(radius::SM))
                .text_size(px(size::SM))
                .text_color(fg)
                .role(Role::Tab)
                .aria_selected(on)
                .map(|d| if on { d.bg(Tokens::surface_3()).border_1().border_color(Tokens::border_strong()).font_weight(FontWeight::MEDIUM) } else { d.hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1())) })
                .pressable()
                .on_click(move |_, w, cx| f(&t, w, cx))
                .child(ph_icon(icon, if on { Weight::Fill } else { Weight::Bold }, if on { Tokens::accent() } else { Tokens::text_3() }, icon_px::SM))
                .child(label)
                .when(live && !on, |d| d.child(pulse_dot((id, 100 + i), Tokens::accent(), 6.)))
        },
    ))
}

// ---------------------------------------------------------------- reset confirmation

/// What a reset confirmation shows.
pub(crate) struct ResetDialog {
    pub title: String,
    pub body: String,
    /// One row per thing that goes: icon, name, detail ("48 MB").
    pub rows: Vec<(Ph, String, String)>,
    /// Shown as a warning above the buttons (manual edits that are lost).
    pub warning: Option<String>,
    /// Outcome label, e.g. "Reset 3 steps".
    pub confirm: String,
    pub cancel: String,
}

/// Modal confirmation for a reset, listing exactly what goes. Like `mv_ui::confirm_dialog`:
/// nothing is focused when it opens, Enter never confirms, Esc / backdrop / Cancel keep the work,
/// buttons are labelled by outcome, the destructive one last.
/// NN/g confirmation dialogs: <https://www.nngroup.com/articles/confirmation-dialog/>
pub(crate) fn open_reset_dialog(d: ResetDialog, on_confirm: impl Fn(&mut Window, &mut App) + 'static, window: &mut Window, cx: &mut App) {
    let on_confirm = Rc::new(on_confirm);
    let ResetDialog { title, body, rows, warning, confirm, cancel } = d;
    window.open_dialog(cx, move |dialog, _, _| {
        let yes = on_confirm.clone();
        let heading = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(ph_icon(ph::WARNING_CIRCLE, Weight::Fill, Tokens::danger(), icon_px::LG))
            .child(div().ramp(ramp::SUBTITLE_2).text_color(Tokens::text_1()).child(title.clone()));
        let list = div().flex().flex_col().mt(px(space::MD)).rounded(px(radius::MD)).border_1().border_color(Tokens::border()).bg(Tokens::well()).children(rows.iter().enumerate().map(|(i, (icon, name, detail))| {
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(space::MD))
                .py(px(space::SM))
                .when(i > 0, |d| d.border_t_1().border_color(Tokens::border()))
                .child(icon_ph(*icon, Tokens::text_2(), icon_px::MD))
                .child(div().flex_1().min_w_0().truncate().text_size(px(size::MD)).text_color(Tokens::text_1()).child(name.clone()))
                .child(div().flex_none().text_size(px(size::SM)).text_color(Tokens::text_3()).child(detail.clone()))
        }));
        let footer = div()
            .flex()
            .justify_end()
            .gap(px(space::SM))
            .w_full()
            .child(btn("studio-reset-keep", cancel.clone(), ButtonKind::Secondary).on_click(|_, w, cx| w.close_dialog(cx)))
            .child(btn("studio-reset-go", confirm.clone(), ButtonKind::Danger).icon(ph::ARROW_COUNTER_CLOCKWISE).on_click(move |_, w, cx| {
                w.close_dialog(cx);
                yes(w, cx);
            }));
        dialog
            .w(px(460.))
            .title(heading)
            .child(div().text_size(px(size::MD)).line_height(px(20.)).text_color(Tokens::text_2()).child(body.clone()))
            .child(list)
            .when_some(warning.clone(), |d, w| d.child(inline_alert(AlertKind::Warning, w, "").mt(px(space::MD))))
            .footer(footer)
            // Enter never confirms a reset; Esc and the backdrop keep the work.
            .on_ok(|_, _, _| false)
            .on_cancel(|_, _, _| true)
    });
}

// ---------------------------------------------------------------- formatting

/// "3:42" for seconds.
pub(crate) fn fmt_secs(t: f32) -> String {
    let t = if t.is_finite() { t.max(0.).round() as u64 } else { 0 };
    format!("{}:{:02}", t / 60, t % 60)
}

/// "just now", "4 min ago", "2 h ago", "3 days ago".
pub(crate) fn ago(t: SystemTime) -> String {
    let s = SystemTime::now().duration_since(t).map(|d| d.as_secs()).unwrap_or(0);
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        _ => {
            let d = s / 86_400;
            format!("{d} day{} ago", if d == 1 { "" } else { "s" })
        }
    }
}

/// Length of a WAV from its header (no samples read).
pub(crate) fn wav_secs(path: &std::path::Path) -> Option<f32> {
    let r = hound::WavReader::open(path).ok()?;
    let spec = r.spec();
    (spec.sample_rate > 0).then(|| r.duration() as f32 / spec.sample_rate as f32)
}

#[cfg(test)]
mod tests {
    // Explicit imports: with test-support, `gpui_kit::*` carries a `test` macro that shadows `#[test]`.
    use super::{ago, feel_icon, fmt_secs, kind_style, phase_icon, Phase, DELIVERIES};
    use mv_ui::{ph, Ph};
    use std::time::SystemTime;

    #[test]
    fn every_phase_kind_and_feel_has_its_own_glyph() {
        let icons: Vec<Ph> = Phase::ALL.iter().map(|p| phase_icon(*p)).collect();
        for (i, a) in icons.iter().enumerate() {
            assert!(!icons[i + 1..].contains(a), "phase icons are unique");
        }
        for k in mv_studio::tts::SCENE_KINDS.iter().filter(|k| **k != "other") {
            assert_ne!(kind_style(k).0, ph::MASK_HAPPY, "{k} has its own icon");
        }
        for s in mv_recap::STYLES {
            assert_ne!(feel_icon(s), ph::MASK_HAPPY, "{s} has its own icon");
        }
        for (_, kind, i, _) in DELIVERIES {
            assert!(mv_studio::tts::SCENE_KINDS.contains(kind) && *i <= 10, "{kind} is a kind the narration knows");
        }
    }

    #[test]
    fn times_read_naturally() {
        assert_eq!(fmt_secs(222.4), "3:42");
        assert_eq!(fmt_secs(f32::NAN), "0:00");
        let now = SystemTime::now();
        assert_eq!(ago(now), "just now");
        assert_eq!(ago(now - std::time::Duration::from_secs(300)), "5 min ago");
        assert_eq!(ago(now - std::time::Duration::from_secs(90_000)), "1 day ago");
    }
}
