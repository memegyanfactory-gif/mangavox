//! Player-local building blocks for controls drawn over video. They follow the mv-ui kit (state
//! layers, focus ring, tooltips with shortcut hints, Phosphor weights, tokens) but are sized for a
//! full-screen picture, where the kit's 28 to 40px controls are too small. Candidates for mv-ui.
use gpui_kit::component::{IconName, IconNamed as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::{icon as icon_px, radius, size, Tokens};
use mv_ui::{icon, kbd, ph, tip, tip_kbd, Interactive as _, Ph};

use crate::logic::{self, MENU_ROW_H};

/// Pure black behind and around the picture (letterbox), from the scrim token.
pub fn video_bg() -> Rgba {
    Tokens::scrim().alpha(1.)
}

/// Black at `a` over video: gradients behind the bars, vignettes.
pub fn shade(a: f32) -> Rgba {
    Tokens::scrim().alpha(a)
}

/// Light at `a` over video: state layers, the scrub track, buffered ranges.
pub fn light(a: f32) -> Rgba {
    Tokens::text_1().alpha(a)
}

/// Box for an overlay glyph: 20px of padding, so a 24px glyph gets a 44px target (Apple HIG,
/// 44x44 pt minimum; WCAG 2.5.8 target size).
pub fn ctl_box(glyph_px: f32) -> f32 {
    glyph_px + 20.
}

/// The shell every overlay control shares: round light hover and pressed layers, a Tab stop with
/// the kit focus ring, a tooltip naming the action with its shortcut (Apple HIG keyboards: show
/// the shortcut where the command lives), and clicks that never reach the video (pause).
/// `open` marks a control whose menu is showing: a steady highlight and no tooltip over the menu.
pub fn ctl_shell(id: impl Into<ElementId>, box_px: f32, tooltip: impl Into<SharedString>, keys: Option<&'static str>, open: bool) -> Stateful<Div> {
    let tooltip: SharedString = tooltip.into();
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(box_px))
        .rounded_full()
        .role(Role::Button)
        .aria_label(tooltip.clone())
        .cursor_pointer()
        .when(open, |d| d.bg(light(0.16)).aria_expanded(true))
        // Hovering an open control adds to its highlight instead of dimming it (Material 3: the
        // hover layer stacks on the selected state).
        .hover(move |s| s.bg(light(if open { 0.2 } else { 0.14 })))
        .active(|s| s.bg(light(0.22)))
        .focus_ring()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .when(!open, |d| match keys {
            Some(k) => d.tooltip(tip_kbd(tooltip, k)),
            None => d.tooltip(tip(tooltip)),
        })
}

/// Icon-only overlay control. `glyph` is an asset path so the caller picks the weight:
/// `ph::PLAY.fill()` for transport, `.bold()` for chrome on moving video.
pub fn ctl(id: impl Into<ElementId>, glyph: &'static str, glyph_px: f32, tooltip: impl Into<SharedString>, keys: Option<&'static str>) -> Stateful<Div> {
    ctl_shell(id, ctl_box(glyph_px), tooltip, keys, false).child(icon(glyph, Tokens::text_1(), glyph_px))
}

/// Overlay control showing a short text instead of a glyph ("1.5×" on the speed button), so a
/// non-default state is visible at a glance (NN/g heuristic 1, visibility of system status).
pub fn text_ctl(id: impl Into<ElementId>, text: impl Into<SharedString>, glyph_px: f32, tooltip: impl Into<SharedString>, open: bool) -> Stateful<Div> {
    ctl_shell(id, ctl_box(glyph_px), tooltip, None, open).child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(text.into()))
}

/// Rewind / forward: the circular arrow with "10" inside (Netflix, YouTube).
pub fn jump_ctl(id: &'static str, glyph: &'static str, glyph_px: f32, tooltip: &'static str, keys: &'static str) -> Stateful<Div> {
    ctl(id, glyph, glyph_px, tooltip, Some(keys)).relative().child(
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .pt(px(1.))
            .text_size(px(9.5))
            .font_weight(FontWeight::BOLD)
            .text_color(Tokens::text_1())
            .child("10"),
    )
}

/// Window caption button for the overlay's own title area, drawn like the shell's TitleBar
/// (same caption glyphs, danger fill behind Close) so entering the player changes nothing up there.
pub fn frame_btn(id: &'static str, glyph: IconName, close: bool, tooltip: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(46.))
        .h(px(34.))
        .role(Role::Button)
        .aria_label(tooltip)
        .cursor_pointer()
        .group(id)
        .hover(move |s| if close { s.bg(Tokens::danger()) } else { s.bg(light(0.12)) })
        .active(move |s| if close { s.bg(Tokens::danger().alpha(0.8)) } else { s.bg(light(0.2)) })
        .focus_ring()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .tooltip(tip(tooltip))
        .child(svg().path(glyph.path()).size(px(icon_px::SM)).flex_none().text_color(Tokens::text_1()).when(close, |s| s.group_hover(id, |s| s.text_color(Tokens::base()))))
}

/// Floating glass surface over video (menus, Up Next, the shortcut sheet). Blocks clicks to the
/// video underneath.
pub fn glass_panel() -> Div {
    div().flex().flex_col().rounded(px(radius::LG)).bg(Tokens::glass_strong()).border_1().border_color(Tokens::glass_border()).shadow_lg().occlude()
}

/// Row of the Audio & Subtitles menu. Same metrics and states as `mv_ui::menu_row` (radius, hover
/// layer, accent check, Tab stop) plus a leading slot for the language flag; the selected row is
/// semibold with a check, so the state never rests on colour alone (WCAG 1.4.1).
/// Candidate for `menu_row(..).leading(el)` in mv-ui.
pub fn track_row(id: impl Into<ElementId>, lead: Option<AnyElement>, label: impl Into<SharedString>, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(10.))
        .h(px(MENU_ROW_H))
        .px(px(10.))
        .rounded(px(radius::SM))
        .role(Role::MenuItem)
        .aria_selected(selected)
        .interactive()
        .text_size(px(size::MD))
        .text_color(Tokens::text_1())
        .when(selected, |d| d.font_weight(FontWeight::SEMIBOLD))
        .children(lead)
        .child(div().flex_1().min_w_0().truncate().child(label.into()))
        .child(div().w(px(icon_px::SM)).flex_none().when(selected, |d| d.child(icon(ph::CHECK.bold(), Tokens::accent(), icon_px::SM))))
}

/// Menu column heading: glyph and title.
pub fn menu_head(title: &'static str, glyph: Ph) -> Div {
    div()
        .flex_none()
        .h(px(logic::MENU_HEAD_H))
        .px(px(18.))
        .flex()
        .items_center()
        .gap(px(10.))
        .text_size(px(size::LG))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(Tokens::text_1())
        .child(icon(glyph.regular(), Tokens::text_2(), icon_px::LG))
        .child(title)
}

/// Small flag image (flagcdn) for a track's country, or a neutral globe.
pub fn flag(country: Option<String>) -> AnyElement {
    match country {
        Some(cc) => img(logic::flag_url(&cc))
            .flex_none()
            .w(px(22.))
            .h(px(16.))
            .rounded(px(3.))
            .object_fit(ObjectFit::Cover)
            .with_fallback(|| flag_glyph(ph::GLOBE))
            .into_any_element(),
        None => flag_glyph(ph::GLOBE),
    }
}

/// A glyph in the flag slot (globe for unknown languages, the off state of subtitles).
pub fn flag_glyph(glyph: Ph) -> AnyElement {
    div().flex_none().w(px(22.)).h(px(16.)).flex().items_center().justify_center().child(icon(glyph.bold(), Tokens::text_3(), icon_px::SM)).into_any_element()
}

/// Peers / speed / progress pill on the loader.
pub fn stat_chip(glyph: Ph, text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .h(px(28.))
        .px(px(12.))
        .rounded(px(radius::PILL))
        .bg(Tokens::glass())
        .border_1()
        .border_color(Tokens::glass_border())
        .text_size(px(size::SM))
        .text_color(Tokens::text_1())
        .child(icon(glyph.bold(), Tokens::accent(), icon_px::SM))
        .child(text.into())
}

/// One line of the shortcut sheet: what it does, then its key caps (alternatives, "/").
pub fn shortcut_row(label: &'static str, keys: &[&'static str]) -> Div {
    key_line(label, keys, "/")
}

/// A shortcut over a run of keys, e.g. 0 – 9.
pub fn shortcut_range(label: &'static str, first: &'static str, last: &'static str) -> Div {
    key_line(label, &[first, last], "–")
}

fn key_line(label: &'static str, keys: &[&'static str], sep: &'static str) -> Div {
    let mut caps = div().flex().flex_none().items_center().gap(px(4.));
    for (i, k) in keys.iter().enumerate() {
        if i > 0 {
            caps = caps.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(sep));
        }
        caps = caps.child(kbd(*k));
    }
    div().flex().items_center().justify_between().gap(px(16.)).h(px(28.)).text_size(px(size::MD)).text_color(Tokens::text_2()).child(label).child(caps)
}
