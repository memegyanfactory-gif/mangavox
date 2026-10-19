//! Layout pieces shared by every Settings page: page title, section cards, setting rows,
//! status pills and icon tiles. One spacing scale (4-pt grid), one accent.
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::{icon, size, Tokens};

/// Page title with a one-line description underneath.
pub fn page_title(title: impl Into<SharedString>, desc: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(div().text_size(px(size::XL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title.into()))
        .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(desc.into()))
}

/// Small uppercase label for a group of rows or nav items.
pub fn section_label(text: &str) -> Div {
    div()
        .text_size(px(size::XS))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(Tokens::text_3())
        .child(text.to_uppercase())
}

/// Bordered surface; callers add rows.
pub fn surface() -> Div {
    div().flex().flex_col().rounded(px(12.)).bg(Tokens::surface()).border_1().border_color(Tokens::border())
}

/// Section card: title + optional description, then rows (`row`, `divided`).
pub fn card(title: &str, desc: Option<&str>) -> Div {
    let head = div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .pb(px(6.))
        .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title.to_string()))
        .when_some(desc, |d, t| d.child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(t.to_string())));
    surface().px(px(20.)).pt(px(16.)).pb(px(6.)).child(head)
}

/// Label and description on the left, control on the right.
pub fn row(label: &str, hint: &str, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(24.))
        .py(px(12.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .flex_1()
                .min_w_0()
                .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(label.to_string()))
                .when(!hint.is_empty(), |d| d.child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(hint.to_string()))),
        )
        .child(div().flex().items_center().gap(px(6.)).flex_none().child(control))
}

/// Hairline above a row so rows inside a card read as a list.
pub fn divided(row: Div) -> Div {
    row.border_t_1().border_color(Tokens::border())
}

/// Coloured dot + text in a soft rounded pill.
pub fn pill(dot: Rgba, text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(7.))
        .h(px(24.))
        .px(px(10.))
        .rounded(px(999.))
        .bg(rgba(0xffffff0a))
        .border_1()
        .border_color(Tokens::border())
        .text_size(px(size::SM))
        .text_color(Tokens::text_2())
        .min_w_0()
        .child(div().flex_none().size(px(7.)).rounded_full().bg(dot))
        .child(div().truncate().child(text.into()))
}

/// Tiny label chip (quant, "Vision", "In use").
pub fn tag(text: impl Into<SharedString>, color: Rgba) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(px(18.))
        .px(px(6.))
        .rounded(px(5.))
        .bg(rgba(0xffffff0d))
        .text_size(px(size::XS))
        .text_color(color)
        .child(text.into())
}

/// Square icon tile used for categories, jobs and engines.
pub fn tile(path: &'static str, side: f32) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(side))
        .rounded(px(side * 0.25))
        .bg(Tokens::surface_2())
        .border_1()
        .border_color(Tokens::border())
        .child(icon(path, Tokens::text_2(), side * 0.5))
}

/// Accent text that acts like a link.
pub fn link(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .text_size(px(size::SM))
        .text_color(Tokens::accent())
        .cursor_pointer()
        .hover(|s| s.opacity(0.8))
        .child(text.into())
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn bytes() {
        assert_eq!(super::format_bytes(512), "512 B");
        assert_eq!(super::format_bytes(1536), "1.5 KB");
        assert_eq!(super::format_bytes(5 * 1024 * 1024 * 1024), "5.0 GB");
        assert_eq!(super::format_bytes(412 * 1024 * 1024 * 1024), "412 GB");
    }
}
