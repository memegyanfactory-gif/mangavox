//! YouTube Studio's dark theme, as measured on studio.youtube.com (desktop, November 2026): page
//! #0f0f0f, text #f1f1f1 / #aaaaaa, cards with a 1 px white-20% border and 16 px corners, 18 px
//! bold card titles, 32 px bold page titles and big numbers, tonal pill buttons (white 10%, 36 px
//! tall), 40 px nav rows with 8 px corners, the 248 px nav with a 112 px avatar. The YouTube,
//! Competitors and Automation pages draw with these instead of the app theme, so they look like
//! the Studio the user knows. Roboto when installed, else Arial (YouTube's own fallback).
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::{ph_icon, Ph, Weight};

pub const BG: u32 = 0x0f0f0f;
/// Raised surfaces: menus, the selected table row, popovers.
pub const RAISED: u32 = 0x1f1f1f;
pub const MENU: u32 = 0x282828;
pub const TEXT: u32 = 0xf1f1f1;
pub const TEXT_2: u32 = 0xaaaaaa;
pub const TEXT_3: u32 = 0x717171;
pub const LINK: u32 = 0x3ea6ff;
pub const RED: u32 = 0xff0033;
pub const GREEN: u32 = 0x2ba640;
pub const AMBER: u32 = 0xffb300;
pub const ERROR: u32 = 0xff4e45;
pub const PURPLE: u32 = 0xa57cf4;

pub const NAV_W: f32 = 248.;
pub const CARD_W: f32 = 394.;
pub const GAP: f32 = 24.;

/// The app runs a light theme: Studio's light look (white page, near-black text) instead.
pub fn light() -> bool {
    mv_ui::theme::palette().light
}

/// A colour of the dark theme above, or its Studio-light counterpart in a light theme.
pub fn c(hex: u32) -> Rgba {
    if !light() {
        return rgb(hex);
    }
    rgb(match hex {
        BG => 0xffffff,
        RAISED => 0xf2f2f2,
        MENU => 0xffffff,
        TEXT => 0x0d0d0d,
        TEXT_2 => 0x606060,
        TEXT_3 => 0x909090,
        LINK => 0x065fd4,
        GREEN => 0x107516,
        AMBER => 0xb26a00,
        ERROR => 0xcc0000,
        0xd9d9d9 => 0x272727,
        h => h,
    })
}

/// White at `a` (black at half `a` in light): borders (0.2), dividers (0.1), tonal buttons and
/// the active nav row (0.1).
pub fn tone(a: f32) -> Hsla {
    if light() {
        hsla(0., 0., 0., a * 0.5)
    } else {
        hsla(0., 0., 1., a)
    }
}

pub fn black(a: f32) -> Hsla {
    hsla(0., 0., 0., a)
}

/// Roboto when it is installed, else Arial.
pub fn font(cx: &App) -> SharedString {
    static PICK: std::sync::OnceLock<SharedString> = std::sync::OnceLock::new();
    PICK.get_or_init(|| {
        let names = cx.text_system().all_font_names();
        SharedString::from(if names.iter().any(|n| n == "Roboto") { "Roboto" } else { "Arial" })
    })
    .clone()
}

/// Text at a size / line height / weight / colour. Long text is cut with an ellipsis unless the
/// caller wraps it.
pub fn text(size: f32, line: f32, weight: FontWeight, color: u32) -> Div {
    div().text_size(px(size)).line_height(px(line)).font_weight(weight).text_color(c(color))
}

/// One line, cut with "…" to fit (never spills out of its box).
pub fn line(size: f32, weight: FontWeight, color: u32, s: impl Into<SharedString>) -> Div {
    text(size, size + 6., weight, color).min_w_0().truncate().child(s.into())
}

/// Wrapped body text clamped to `lines`.
pub fn para(size: f32, color: u32, lines: usize, s: impl Into<SharedString>) -> Div {
    text(size, size + 6., FontWeight::NORMAL, color).min_w_0().whitespace_normal().line_clamp(lines).child(s.into())
}

/// "Channel dashboard": 32 px bold.
pub fn page_title(s: impl Into<SharedString>) -> Div {
    text(32., 44., FontWeight::BOLD, TEXT).min_w_0().truncate().child(s.into())
}

/// A Studio card: 1 px white-20% border, 16 px corners, 24 px padding.
pub fn card() -> Div {
    div().flex().flex_col().min_w_0().gap(px(16.)).p(px(24.)).rounded(px(16.)).bg(c(BG)).border_1().border_color(tone(0.2))
}

/// A card's title: 18 px bold.
pub fn card_title(s: impl Into<SharedString>) -> Div {
    text(18., 26., FontWeight::BOLD, TEXT).min_w_0().truncate().child(s.into())
}

/// A card section heading ("Summary"): 16 px medium, with an optional grey line under it.
pub fn heading(s: impl Into<SharedString>, sub: Option<String>) -> Div {
    div().flex().flex_col().min_w_0().child(text(16., 22., FontWeight::MEDIUM, TEXT).child(s.into())).children(sub.map(|t| line(12., FontWeight::NORMAL, TEXT_2, t)))
}

/// The grey 12 px line under a heading ("Last 28 days").
pub fn caption(s: impl Into<SharedString>) -> Div {
    line(12., FontWeight::NORMAL, TEXT_2, s)
}

/// A 1 px white-10% rule.
pub fn divider() -> Div {
    div().h(px(1.)).w_full().flex_none().bg(tone(0.1))
}

/// "Views ........ 82": a label and a right-aligned value on one row.
pub fn metric_row(label: impl Into<SharedString>, value: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .min_w_0()
        .child(line(14., FontWeight::NORMAL, TEXT, label).flex_1())
        .child(text(14., 20., FontWeight::NORMAL, TEXT).flex_none().child(value.into()))
}

/// Studio's tonal pill button (white 10%, 36 px, 18 px corners).
pub fn pill(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h(px(36.))
        .px(px(16.))
        .rounded(px(18.))
        .bg(tone(0.1))
        .hover(|s| s.bg(tone(0.2)))
        .cursor_pointer()
        .child(text(14., 36., FontWeight::MEDIUM, TEXT).whitespace_nowrap().child(label.into()))
}

/// A pill with a leading icon.
pub fn pill_icon(id: impl Into<ElementId>, icon: Ph, label: impl Into<SharedString>) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(36.))
        .pl(px(12.))
        .pr(px(16.))
        .rounded(px(18.))
        .bg(tone(0.1))
        .hover(|s| s.bg(tone(0.2)))
        .cursor_pointer()
        .child(icon_of(icon, 18., TEXT))
        .child(text(14., 36., FontWeight::MEDIUM, TEXT).whitespace_nowrap().child(label))
}

/// The white call-to-action pill ("Upload videos", "Approve").
pub fn pill_primary(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .h(px(36.))
        .px(px(16.))
        .rounded(px(18.))
        .bg(c(TEXT))
        .hover(|s| s.bg(c(0xd9d9d9)))
        .cursor_pointer()
        .child(text(14., 36., FontWeight::MEDIUM, BG).whitespace_nowrap().child(label.into()))
}

/// A round 40 px icon button.
pub fn icon_btn(id: impl Into<ElementId>, icon: Ph, tip: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(40.))
        .rounded_full()
        .border_1()
        .border_color(tone(0.2))
        .hover(|s| s.bg(tone(0.1)))
        .cursor_pointer()
        .tooltip(mv_ui::tip(tip))
        .child(icon_of(icon, 20., TEXT))
}

pub fn icon_of(icon: Ph, size: f32, color: u32) -> Svg {
    ph_icon(icon, Weight::Regular, c(color), size)
}

/// A filter chip ("Published", "Most relevant"): 32 px, 8 px corners, white 10% (white when on).
pub fn chip(id: impl Into<ElementId>, label: impl Into<SharedString>, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .h(px(32.))
        .px(px(12.))
        .rounded(px(8.))
        .when(on, |d| d.bg(c(TEXT)))
        .when(!on, |d| d.bg(tone(0.1)).hover(|s| s.bg(tone(0.2))))
        .cursor_pointer()
        .child(text(14., 20., FontWeight::MEDIUM, if on { BG } else { TEXT }).whitespace_nowrap().child(label.into()))
}

/// A coloured status label with a dot ("● Scheduled").
pub fn status(label: impl Into<SharedString>, color: u32) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .child(div().size(px(8.)).flex_none().rounded_full().bg(c(color)))
        .child(text(13., 18., FontWeight::MEDIUM, color).whitespace_nowrap().child(label.into()))
}

/// A small filled badge ("NEW", "AUTO").
pub fn badge(label: impl Into<SharedString>, fg: u32, bg: Hsla) -> Div {
    div().flex_none().px(px(6.)).h(px(18.)).flex().items_center().rounded(px(4.)).bg(bg).child(text(11., 14., FontWeight::BOLD, fg).whitespace_nowrap().child(label.into()))
}

/// A round avatar from a URL (grey disc when empty).
pub fn avatar(url: &str, d: f32) -> Div {
    let base = div().flex_none().size(px(d)).rounded_full().overflow_hidden().bg(tone(0.1));
    if url.is_empty() {
        base
    } else {
        base.child(img(SharedString::from(url.to_string())).size_full().object_fit(ObjectFit::Cover))
    }
}

/// A 16:9 thumbnail (URL or local file) with an optional duration badge bottom-right.
pub fn thumb(src: &str, w: f32, duration: Option<String>) -> Div {
    let base = div().relative().flex_none().w(px(w)).h(px(w * 9. / 16.)).rounded(px(if w >= 200. { 12. } else { 6. })).overflow_hidden().bg(tone(0.1));
    let base = if src.is_empty() {
        base.flex().items_center().justify_center().child(icon_of(mv_ui::ph::FILM_STRIP, 20., TEXT_3))
    } else if src.starts_with("http") {
        base.child(img(SharedString::from(src.to_string())).size_full().object_fit(ObjectFit::Cover))
    } else {
        base.child(img(std::path::PathBuf::from(src)).size_full().object_fit(ObjectFit::Cover))
    };
    base.children(duration.filter(|d| !d.is_empty()).map(|d| {
        div().absolute().bottom(px(4.)).right(px(4.)).px(px(4.)).rounded(px(4.)).bg(black(0.8)).child(text(12., 18., FontWeight::MEDIUM, 0xffffff).child(d))
    }))
}

/// "1:02:03" from seconds.
pub fn duration(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// An icon and a small grey number (the dashboard's "views · comments · likes" row).
pub fn stat(icon: Ph, value: impl Into<SharedString>) -> Div {
    div().flex().flex_none().items_center().gap(px(4.)).child(icon_of(icon, 16., TEXT_2)).child(text(12., 18., FontWeight::NORMAL, TEXT_2).whitespace_nowrap().child(value.into()))
}

/// A column of fixed width in a table header or row, right-aligned for numbers.
pub fn col(w: f32, right: bool) -> Div {
    div().flex_none().w(px(w)).min_w_0().overflow_hidden().when(right, |d| d.flex().justify_end())
}

/// A table header cell (12 px, grey).
pub fn th(w: f32, right: bool, s: &'static str) -> Div {
    col(w, right).child(text(12., 18., FontWeight::MEDIUM, TEXT_2).whitespace_nowrap().child(s))
}

/// Line chart with an area fill, oldest left: Studio's analytics chart. Hover shows `labels[i]`.
pub fn area_chart(id: &'static str, values: &[f64], labels: &[String], color: u32, height: f32) -> Div {
    let max = values.iter().copied().fold(0f64, f64::max).max(1.);
    let n = values.len().max(1);
    let col = c(color);
    let mut fill = col;
    fill.a = 0.18;
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .relative()
                .h(px(height))
                .w_full()
                .border_b_1()
                .border_color(tone(0.2))
                .flex()
                .items_end()
                .children(values.iter().enumerate().map(|(i, v)| {
                    let h = (*v / max) as f32 * (height - 6.);
                    let label = labels.get(i).cloned().unwrap_or_default();
                    div()
                        .id((id, i))
                        .flex_1()
                        .h_full()
                        .flex()
                        .flex_col()
                        .justify_end()
                        .hover(|s| s.bg(tone(0.05)))
                        .tooltip(mv_ui::tip(format!("{label}: {}", fmt_value(*v))))
                        .child(div().w_full().h(px(2.)).bg(col))
                        .child(div().w_full().h(px(h.max(0.))).bg(fill))
                })),
        )
        .when(n > 1, |d| {
            let first = labels.first().cloned().unwrap_or_default();
            let last = labels.last().cloned().unwrap_or_default();
            d.child(div().flex().justify_between().child(caption(first)).child(caption(last)))
        })
}

/// Bars, oldest left, with hover values.
pub fn bar_chart(id: &'static str, values: &[f64], labels: &[String], color: u32, height: f32) -> Div {
    let max = values.iter().copied().fold(0f64, f64::max).max(1.);
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(div().flex().items_end().gap(px(2.)).h(px(height)).border_b_1().border_color(tone(0.2)).children(values.iter().enumerate().map(|(i, v)| {
            let h = ((*v / max) as f32 * (height - 4.)).max(if *v > 0. { 2. } else { 1. });
            let label = labels.get(i).cloned().unwrap_or_default();
            div()
                .id((id, i))
                .flex_1()
                .min_w(px(2.))
                .h(px(h))
                .rounded_t(px(2.))
                .bg(if *v > 0. { c(color).into() } else { tone(0.1) })
                .opacity(0.85)
                .hover(|s| s.opacity(1.))
                .tooltip(mv_ui::tip(format!("{label}: {}", fmt_value(*v))))
        })))
        .when(labels.len() > 1, |d| d.child(div().flex().justify_between().child(caption(labels.first().cloned().unwrap_or_default())).child(caption(labels.last().cloned().unwrap_or_default()))))
}

/// 1234 → "1.2K".
pub fn num(n: u64) -> String {
    crate::hub::fmt_num(n)
}

fn fmt_value(v: f64) -> String {
    if v.fract() == 0. {
        num(v as u64)
    } else {
        format!("{v:.1}")
    }
}

/// "+12%" / "-4%" in green / red.
pub fn delta(change: f64) -> Div {
    let (color, s) = if change >= 0. { (GREEN, format!("+{:.0}%", change * 100.)) } else { (ERROR, format!("{:.0}%", change * 100.)) };
    text(12., 18., FontWeight::MEDIUM, color).whitespace_nowrap().child(s)
}
