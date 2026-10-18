//! Building blocks shared by the manga pages (Mihon-style cover grid, badges, tabs, states).
//! Everything here uses the mv-ui tokens and type scale so the pages stay consistent.
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::{icon, size, CoverSource, Tokens};

use crate::backend;

/// Smallest cover width in the grids; cards stretch to fill the row.
pub const MIN_CARD: f32 = 140.0;
/// Gap between cards (both directions).
pub const GAP: f32 = 16.0;
/// Covers are 2:3.
pub const COVER_RATIO: f32 = 1.5;
/// Width of the expanded app sidebar, used until the real width has been measured.
const SIDEBAR_GUESS: f32 = 244.0;

/// Loading / ready / failed state of an async page section.
pub enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

pub fn cover_src(map: &HashMap<String, PathBuf>, url: Option<&str>) -> Option<CoverSource> {
    url.and_then(|u| map.get(u)).cloned().map(CoverSource::File)
}

pub fn search_box(state: &Entity<InputState>) -> Div {
    div().w(px(240.)).child(Input::new(state).cleanable(true))
}

pub fn muted(text: impl Into<SharedString>) -> Div {
    div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(text.into())
}

// ---------------------------------------------------------------- layout helpers

/// Invisible element that reports its parent's width into `slot` (re-rendering on change).
/// Put it inside a `.relative()` container.
pub fn width_probe<T: 'static>(cx: &Context<T>, slot: fn(&mut T) -> &mut f32) -> impl IntoElement {
    let entity = cx.entity().downgrade();
    canvas(
        move |bounds, window, cx| {
            let w = f32::from(bounds.size.width);
            let changed = entity
                .update(cx, |t, _| {
                    let s = slot(t);
                    let changed = (*s - w).abs() > 0.5;
                    *s = w;
                    changed
                })
                .unwrap_or(false);
            if changed {
                let entity = entity.clone();
                window.on_next_frame(move |_, cx| {
                    let _ = entity.update(cx, |_, cx| cx.notify());
                });
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Grid geometry from the measured list width (rows carry the page padding themselves);
/// falls back to a viewport estimate before the first measurement.
#[derive(Clone, Copy)]
pub struct Grid {
    pub cols: usize,
    pub card_w: f32,
}

impl Grid {
    pub fn new(measured: f32, window: &Window) -> Self {
        let width = if measured > 1.0 { measured - size::PAGE_X * 2.0 } else { f32::from(window.viewport_size().width) - SIDEBAR_GUESS - size::PAGE_X * 2.0 };
        let (cols, card_w) = crate::logic::grid_layout(width, MIN_CARD, GAP);
        Self { cols, card_w }
    }
    pub fn card_h(&self) -> f32 {
        (self.card_w * COVER_RATIO).floor()
    }
    pub fn row_h(&self) -> f32 {
        self.card_h() + GAP
    }
    pub fn rows(&self, n: usize) -> usize {
        n.div_ceil(self.cols)
    }
    /// One grid row (left aligned, fixed height so `uniform_list` can virtualize it).
    pub fn row(&self) -> Div {
        div().flex().gap(px(GAP)).h(px(self.row_h())).px(px(size::PAGE_X))
    }
}

// ---------------------------------------------------------------- cover card

/// Everything a cover card can show.
#[derive(Default)]
pub struct CardInfo {
    pub title: SharedString,
    pub cover: Option<CoverSource>,
    pub unread: usize,
    pub downloaded: usize,
    pub in_library: bool,
}

/// Mihon "compact grid" card: 2:3 cover, title over a bottom gradient, badges top-left.
pub fn manga_card(id: impl Into<ElementId>, info: CardInfo, grid: Grid) -> Stateful<Div> {
    let (w, h) = (grid.card_w, grid.card_h());
    div()
        .id(id)
        .relative()
        .flex_none()
        .w(px(w))
        .h(px(h))
        .rounded(px(8.))
        .overflow_hidden()
        .bg(Tokens::surface_2())
        .border_1()
        .border_color(Tokens::border())
        .cursor_pointer()
        .hover(|s| s.border_color(rgba(0xffffff40)))
        .map(|d| match info.cover {
            Some(c) => d.child(c.into_img().size_full().object_fit(ObjectFit::Cover).when(info.in_library, |i| i.opacity(0.45))),
            None => d.child(div().size_full().flex().items_center().justify_center().child(icon("icons/book-open.svg", rgba(0xffffff1f), 32.))),
        })
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px((h * 0.45).max(64.)))
                .bg(linear_gradient(180., linear_color_stop(rgba(0x00000000), 0.), linear_color_stop(rgba(0x000000e6), 1.))),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .px(px(8.))
                .pb(px(7.))
                .text_size(px(size::SM))
                .line_height(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(Tokens::text_1())
                .line_clamp(2)
                .child(info.title),
        )
        .child(
            div()
                .absolute()
                .top(px(6.))
                .left(px(6.))
                .flex()
                .rounded(px(5.))
                .overflow_hidden()
                .when(info.in_library, |d| d.child(badge("In library", BadgeKind::Accent)))
                .when(info.unread > 0, |d| d.child(badge(info.unread.to_string(), BadgeKind::Accent)))
                .when(info.downloaded > 0, |d| d.child(badge(info.downloaded.to_string(), BadgeKind::Neutral))),
        )
}

/// Grey placeholder with the card's exact size (loading state).
pub fn skeleton_cover(grid: Grid) -> Div {
    div().flex_none().w(px(grid.card_w)).h(px(grid.card_h())).rounded(px(8.)).bg(Tokens::surface_2())
}

/// A few rows of skeleton cards.
pub fn skeleton_rows(grid: Grid, rows: usize) -> Div {
    div().flex().flex_col().children((0..rows).map(move |_| grid.row().children((0..grid.cols).map(move |_| skeleton_cover(grid)))))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BadgeKind {
    Accent,
    Neutral,
}

/// Small square-ish count / status tag. Adjacent badges join into one pill (Mihon style).
pub fn badge(text: impl Into<SharedString>, kind: BadgeKind) -> Div {
    div()
        .px(px(5.))
        .h(px(18.))
        .flex()
        .items_center()
        .text_size(px(size::XS))
        .font_weight(FontWeight::BOLD)
        .map(|d| match kind {
            BadgeKind::Accent => d.bg(Tokens::accent()).text_color(Tokens::base()),
            BadgeKind::Neutral => d.bg(rgb(0x3b4450)).text_color(Tokens::text_1()),
        })
        .child(text.into())
}

/// Outlined tag for genres / source language / NSFW.
pub fn tag(text: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .px(px(8.))
        .py(px(2.))
        .rounded(px(999.))
        .border_1()
        .border_color(Tokens::border())
        .text_size(px(size::XS))
        .text_color(Tokens::text_2())
        .child(text.into())
}

// ---------------------------------------------------------------- controls

/// Underlined tab (category / mode tabs). `count` renders as a muted number.
pub fn tab(id: impl Into<ElementId>, label: impl Into<SharedString>, count: Option<usize>, active: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(6.))
        .h(px(36.))
        .px(px(4.))
        .mr(px(16.))
        .border_b_2()
        .cursor_pointer()
        .text_size(px(size::MD))
        .map(|d| {
            if active {
                d.border_color(Tokens::accent()).text_color(Tokens::text_1()).font_weight(FontWeight::MEDIUM)
            } else {
                d.border_color(rgba(0x00000000)).text_color(Tokens::text_3()).hover(|s| s.text_color(Tokens::text_1()))
            }
        })
        .child(label.into())
        .when_some(count, |d, n| {
            d.child(
                div()
                    .px(px(6.))
                    .rounded(px(999.))
                    .bg(if active { Tokens::accent_soft() } else { Tokens::surface_2() })
                    .text_size(px(size::XS))
                    .text_color(if active { Tokens::accent() } else { Tokens::text_3() })
                    .child(n.to_string()),
            )
        })
}

/// Ghost toolbar button with an icon and a label; `active` gets the accent tint.
pub fn tool_button(id: impl Into<ElementId>, icon_path: &'static str, label: impl Into<SharedString>, active: bool) -> Stateful<Div> {
    let fg = if active { Tokens::accent() } else { Tokens::text_2() };
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(30.))
        .px(px(10.))
        .rounded(px(8.))
        .cursor_pointer()
        .text_size(px(size::SM))
        .text_color(fg)
        .when(active, |d| d.bg(Tokens::accent_soft()))
        .hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
        .child(icon(icon_path, fg, 15.))
        .child(label.into())
}

/// Icon-only round button with a tooltip.
pub fn icon_tip(id: impl Into<ElementId>, icon_path: &'static str, tip: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(32.))
        .rounded(px(8.))
        .cursor_pointer()
        .hover(|s| s.bg(Tokens::hover()))
        .tooltip(move |w, cx| Tooltip::new(tip).build(w, cx))
        .child(icon(icon_path, Tokens::text_2(), 16.))
}

/// Large action with an icon above the label (Mihon's manga action row).
pub fn action(id: impl Into<ElementId>, icon_path: &'static str, label: impl Into<SharedString>, active: bool) -> Stateful<Div> {
    let fg = if active { Tokens::accent() } else { Tokens::text_2() };
    div()
        .id(id)
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(4.))
        .min_w(px(84.))
        .h(px(56.))
        .px(px(10.))
        .rounded(px(10.))
        .cursor_pointer()
        .text_size(px(size::XS))
        .text_color(fg)
        .hover(|s| s.bg(Tokens::hover()))
        .child(icon(icon_path, fg, 20.))
        .child(label.into())
}

/// Spinning loader icon.
pub fn spinner(id: impl Into<ElementId>, px_size: f32) -> impl IntoElement {
    icon("icons/loader-circle.svg", Tokens::text_3(), px_size)
        .with_animation(id, Animation::new(Duration::from_millis(900)).repeat(), |s, delta| s.with_transformation(Transformation::rotate(percentage(delta))))
}

// ---------------------------------------------------------------- states

/// Centered icon + title + body (empty / error / not-yet), with an optional action below.
pub fn state_view(icon_path: &'static str, title: impl Into<SharedString>, body: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .min_h(px(280.))
        .items_center()
        .justify_center()
        .gap(px(8.))
        .child(
            div()
                .size(px(56.))
                .mb(px(4.))
                .rounded(px(999.))
                .bg(Tokens::surface_2())
                .flex()
                .items_center()
                .justify_center()
                .child(icon(icon_path, Tokens::text_3(), 26.)),
        )
        .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title.into()))
        .child(div().max_w(px(420.)).text_center().text_size(px(size::SM)).text_color(Tokens::text_3()).child(body.into()))
}

pub fn error_view(message: impl Into<SharedString>) -> Div {
    state_view("icons/triangle-alert.svg", "Something went wrong", message)
}

/// Inline status line (notes after an action). Errors use the danger colour.
pub fn note_line(text: impl Into<SharedString>, error: bool) -> Div {
    div().text_size(px(size::SM)).text_color(if error { Tokens::danger() } else { Tokens::text_3() }).child(text.into())
}

// ---------------------------------------------------------------- covers

/// Download missing covers (6 at a time) into the shared cache, calling `store` to reach the
/// page's `url -> file` map, and repaint after each batch.
pub fn spawn_cover_downloads<T: 'static>(
    cx: &mut Context<T>,
    missing: Vec<(String, Option<String>)>,
    store: impl Fn(&mut T) -> &mut HashMap<String, PathBuf> + 'static,
) {
    if missing.is_empty() {
        return;
    }
    cx.spawn(async move |this, cx| {
        for chunk in missing.chunks(6) {
            let futs: Vec<_> = chunk
                .iter()
                .cloned()
                .map(|(u, s)| {
                    let key = u.clone();
                    let f = mv_ui::bg::run(backend::ensure_cover(u, s));
                    async move { (key, f.await.ok().flatten()) }
                })
                .collect();
            let results = futures::future::join_all(futs).await;
            let alive = this.update(cx, |t, cx| {
                for (u, p) in results {
                    if let Some(p) = p {
                        store(t).insert(u, p);
                    }
                }
                cx.notify();
            });
            if alive.is_err() {
                break;
            }
        }
    })
    .detach();
}
