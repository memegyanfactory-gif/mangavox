//! Small, consistent building blocks. Keep pages quiet: one primary action,
//! secondary actions as ghost buttons, icons only where they carry meaning.
use std::path::PathBuf;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::theme::{size, Tokens};

/// Lucide icon from the bundled set, e.g. `icon("icons/play.svg", Tokens::text_2(), 16.)`.
pub fn icon(path: &'static str, color: Rgba, px_size: f32) -> Svg {
    svg().path(path).size(px(px_size)).flex_none().text_color(color)
}

/// Page title row: title, muted subtitle, then right-aligned actions.
pub fn page_header(title: impl Into<SharedString>, subtitle: Option<SharedString>) -> Div {
    div()
        .flex()
        .items_baseline()
        .gap(px(10.))
        .mb(px(20.))
        .child(div().text_size(px(size::XL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title.into()))
        .when_some(subtitle, |d, s| d.child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(s)))
}

/// Standard scrollable page body with page padding.
pub fn page(id: impl Into<ElementId>) -> Stateful<Div> {
    div().id(id).size_full().overflow_y_scroll().px(px(size::PAGE_X)).py(px(size::PAGE_Y))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
}

pub fn button(id: impl Into<ElementId>, label: impl Into<SharedString>, kind: ButtonKind) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h(px(32.))
        .px(px(12.))
        .rounded(px(8.))
        .text_size(px(size::SM))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .map(|d| match kind {
            ButtonKind::Primary => d.bg(Tokens::accent()).text_color(Tokens::base()).hover(|s| s.opacity(0.9)),
            ButtonKind::Secondary => d.bg(Tokens::surface_2()).border_1().border_color(Tokens::border()).text_color(Tokens::text_1()).hover(|s| s.bg(Tokens::active())),
            ButtonKind::Ghost => d.text_color(Tokens::text_2()).hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1())),
        })
        .child(label.into())
}

/// Square icon-only button (title bars, toolbars).
pub fn icon_button(id: impl Into<ElementId>, path: &'static str, enabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(30.))
        .rounded(px(8.))
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(Tokens::hover())))
        .child(icon(path, if enabled { Tokens::text_2() } else { rgba(0x7d859066) }, 16.))
}

/// Pill used for filters / tabs. `active` gets the neutral highlight.
pub fn chip(id: impl Into<ElementId>, label: impl Into<SharedString>, active: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(28.))
        .px(px(11.))
        .rounded(px(999.))
        .text_size(px(size::SM))
        .cursor_pointer()
        .border_1()
        .map(|d| {
            if active {
                d.bg(Tokens::active()).border_color(rgba(0xffffff26)).text_color(Tokens::text_1())
            } else {
                d.border_color(Tokens::border()).text_color(Tokens::text_2()).hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
            }
        })
        .child(label.into())
}

/// Poster/cover card. Covers are local paths or http URLs.
pub fn cover_card(
    id: impl Into<ElementId>,
    title: SharedString,
    sub: SharedString,
    cover: Option<CoverSource>,
    badge: Option<SharedString>,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_col()
        .gap(px(8.))
        .w(px(150.))
        .cursor_pointer()
        .child(
            div()
                .relative()
                .w(px(150.))
                .h(px(214.))
                .rounded(px(10.))
                .overflow_hidden()
                .bg(Tokens::surface_2())
                .border_1()
                .border_color(Tokens::border())
                .when_some(cover, |d, c| d.child(c.into_img().size_full().object_fit(ObjectFit::Cover)))
                .when_some(badge, |d, b| {
                    d.child(
                        div()
                            .absolute()
                            .top(px(6.))
                            .right(px(6.))
                            .px(px(6.))
                            .py(px(1.))
                            .rounded(px(6.))
                            .bg(Tokens::scrim())
                            .text_size(px(size::XS))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(Tokens::text_1())
                            .child(b),
                    )
                }),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(div().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).truncate().child(title))
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).truncate().child(sub)),
        )
}

#[derive(Clone, Debug)]
pub enum CoverSource {
    File(PathBuf),
    Url(SharedString),
}

impl CoverSource {
    pub fn into_img(self) -> Img {
        match self {
            CoverSource::File(p) => img(p),
            CoverSource::Url(u) => img(u),
        }
    }
}

/// Grey placeholder shaped like a cover card (loading state).
pub fn skeleton_card() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .w(px(150.))
        .child(div().w(px(150.)).h(px(214.)).rounded(px(10.)).bg(Tokens::surface_2()))
        .child(div().w(px(110.)).h(px(10.)).rounded(px(4.)).bg(Tokens::surface_2()))
}

/// Centered message for empty / error / not-yet states.
pub fn empty_state(title: impl Into<SharedString>, body: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .min_h(px(240.))
        .items_center()
        .justify_center()
        .gap(px(6.))
        .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title.into()))
        .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(body.into()))
}

pub fn error_state(message: impl Into<SharedString>) -> Div {
    empty_state("Something went wrong", message)
}

/// Section heading inside a page (e.g. "Continue watching").
pub fn section_title(text: impl Into<SharedString>) -> Div {
    div().mt(px(8.)).mb(px(12.)).text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(text.into())
}

/// Horizontal list of cards that wraps.
pub fn card_grid() -> Div {
    div().flex().flex_wrap().gap(px(20.))
}

struct Placeholder(SharedString);
impl Render for Placeholder {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(empty_state(self.0.clone(), "Coming soon."))
    }
}

/// A simple "coming soon" view, used by stubs.
pub fn placeholder_view(title: impl Into<SharedString>, cx: &mut App) -> AnyView {
    let t = title.into();
    cx.new(|_| Placeholder(t)).into()
}
