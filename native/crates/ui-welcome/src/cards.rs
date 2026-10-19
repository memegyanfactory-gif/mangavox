//! Building blocks of the setup screens. Local for now; candidates for mv-ui (listed in the
//! design log): `option_card`, `icon_tile`, `summary_row`.
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::{icon as icon_px, radius, ramp};
use mv_ui::{icon_ph, ph_icon, selected_badge, size, Hue, Interactive as _, Ph, RampExt as _, Tokens, Weight};

/// A choice shown as a card: surface, 12px corners and a 1px border; selected adds the accent
/// border, the soft accent fill and the filled check badge in the corner (the same look as
/// the Audio voice tiles). Hover strengthens the border; it is a Tab stop with a focus ring.
/// Material 3 cards (selected state): <https://m3.material.io/components/cards/guidelines>
pub fn option_card(id: impl Into<ElementId>, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .relative()
        .flex()
        .gap(px(14.))
        .p(px(16.))
        .rounded(px(radius::LG))
        .border_1()
        .map(|d| if selected { d.bg(Tokens::accent_soft()).border_color(Tokens::accent()) } else { d.bg(Tokens::surface()).border_color(Tokens::border()) })
        .when(!selected, |d| d.hover(|s| s.border_color(Tokens::border_strong()).bg(Tokens::surface_2())))
        .pressable()
        .when(selected, |d| d.child(div().absolute().top(px(10.)).right(px(10.)).child(selected_badge())))
}

/// Square icon on a soft category tint, leading an option card.
pub fn icon_tile(icon: Ph, hue: Hue, selected: bool) -> Div {
    let (bg, fg) = if selected { (Tokens::accent(), Tokens::on_accent()) } else { (Tokens::hue_soft(hue), Tokens::hue(hue)) };
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(40.))
        .rounded(px(radius::MD))
        .bg(bg)
        .child(ph_icon(icon, if selected { Weight::Fill } else { Weight::Regular }, fg, icon_px::LG))
}

/// Title and one-line description inside a card.
pub fn card_text(title: impl Into<SharedString>, body: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .gap(px(2.))
        .pr(px(24.))
        .child(div().ramp(ramp::BODY_STRONG).text_color(Tokens::text_1()).child(title.into()))
        .child(div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_2()).child(body.into()))
}

/// Screen heading: title and a short explanation.
pub fn heading(title: impl Into<SharedString>, body: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(div().ramp(ramp::TITLE_3).text_color(Tokens::text_1()).child(title.into()))
        .child(div().max_w(px(600.)).ramp(ramp::BODY).text_color(Tokens::text_2()).child(body.into()))
}

/// One line of the summary on the last screen: a filled check for something set up, a dashed
/// circle for something left for later, then the detail and an optional action.
pub fn summary_row(done: bool, title: impl Into<SharedString>, detail: impl Into<SharedString>, action: Option<AnyElement>) -> Div {
    let (glyph, color) = if done { (mv_ui::ph::CHECK_CIRCLE, Tokens::accent()) } else { (mv_ui::ph::CIRCLE_DASHED, Tokens::text_3()) };
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .py(px(12.))
        .child(ph_icon(glyph, if done { Weight::Fill } else { Weight::Regular }, color, icon_px::LG))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(div().ramp(ramp::BODY_STRONG).text_color(Tokens::text_1()).child(title.into()))
                .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(detail.into())),
        )
        .children(action)
}

/// Small label with an icon (requirements strip, notes).
pub fn fact(icon: Ph, text: impl Into<SharedString>) -> Div {
    div().flex().items_center().gap(px(6.)).text_size(px(size::SM)).text_color(Tokens::text_2()).child(icon_ph(icon, Tokens::text_3(), icon_px::SM)).child(text.into())
}
