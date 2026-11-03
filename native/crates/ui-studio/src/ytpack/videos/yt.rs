//! YouTube's dark theme as measured on youtube.com (desktop, 2026): colours, the Roboto / Arial
//! type scale, the duration badge, avatars, the logo and the round tonal buttons. Everything on the
//! preview pages is drawn with these, not with the app's own theme.
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::{ph_icon, Ph, Weight};

pub const BG: u32 = 0x0f0f0f;
pub const TEXT: u32 = 0xf1f1f1;
pub const TEXT_2: u32 = 0xaaaaaa;
pub const RED: u32 = 0xff0033;
pub const PINK: u32 = 0xff2791;
pub const MENU: u32 = 0x282828;
pub const SEARCH_BORDER: u32 = 0x303030;
pub const SEARCH_BTN: u32 = 0x222222;
pub const SEARCH_BG: u32 = 0x121212;

pub fn c(hex: u32) -> Rgba {
    rgb(hex)
}

/// White at `a` (tonal buttons, chips, the description box).
pub fn tone(a: f32) -> Hsla {
    hsla(0., 0., 1., a)
}

pub fn black(a: f32) -> Hsla {
    hsla(0., 0., 0., a)
}

/// Roboto when it is installed (YouTube's font), else Arial (YouTube's own fallback).
pub fn font(cx: &App) -> SharedString {
    static PICK: std::sync::OnceLock<SharedString> = std::sync::OnceLock::new();
    PICK.get_or_init(|| {
        let names = cx.text_system().all_font_names();
        SharedString::from(if names.iter().any(|n| n == "Roboto") { "Roboto" } else { "Arial" })
    })
    .clone()
}

/// Text at YouTube's size / line height / weight.
pub fn text(size: f32, line: f32, weight: FontWeight, color: u32) -> Div {
    div().text_size(px(size)).line_height(px(line)).font_weight(weight).text_color(c(color))
}

/// "1:02:03" from seconds.
pub fn clock(secs: f64) -> String {
    let s = secs.max(0.).floor() as u64;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// YouTube's view counts: "No views", "1 view", "834 views", "1.2K views".
pub fn views(n: u64) -> String {
    match n {
        0 => "No views".into(),
        1 => "1 view".into(),
        n => format!("{} views", super::super::logic::short_count(n)),
    }
}

/// "3 hours ago", "2 months ago" (the home grid) for an age in seconds.
pub fn ago_long(secs: u64) -> String {
    let unit = |n: u64, u: &str| format!("{n} {u}{} ago", if n == 1 { "" } else { "s" });
    match secs {
        0..=59 => unit(secs.max(1), "second"),
        60..=3599 => unit(secs / 60, "minute"),
        3600..=86_399 => unit(secs / 3600, "hour"),
        86_400..=604_799 => unit(secs / 86_400, "day"),
        604_800..=2_591_999 => unit(secs / 604_800, "week"),
        2_592_000..=31_535_999 => unit(secs / 2_592_000, "month"),
        _ => unit(secs / 31_536_000, "year"),
    }
}

/// "2mo ago", "10d ago" (the watch page's related column) from YouTube's long form.
pub fn ago_short(long: &str) -> String {
    let l = long.trim();
    let (Some(n), Some(unit)) = (l.split_whitespace().next().filter(|n| n.chars().all(|c| c.is_ascii_digit())), l.split_whitespace().nth(1)) else { return l.to_string() };
    let u = match unit.trim_end_matches('s') {
        "second" => "s",
        "minute" => "m",
        "hour" => "h",
        "day" => "d",
        "week" => "w",
        "month" => "mo",
        "year" => "y",
        _ => return l.to_string(),
    };
    format!("{n}{u} ago")
}

/// The duration badge in a thumbnail's bottom-right corner.
pub fn duration_badge(label: String) -> Div {
    div().px(px(4.)).py(px(1.)).rounded(px(4.)).bg(black(0.6)).child(text(12., 18., FontWeight::MEDIUM, 0xffffff).child(label))
}

/// A 16:9 thumbnail with its duration badge; `radius` 12 on the home grid, 8 on the watch page.
pub fn thumbnail(src: Option<ImageSource>, length: String, radius: f32, inset: f32) -> Div {
    div()
        .relative()
        .w_full()
        .aspect_ratio(16. / 9.)
        .rounded(px(radius))
        .overflow_hidden()
        .bg(tone(0.1))
        // Images are not clipped by their parent: they take the corners themselves.
        .when_some(src, |d, s| d.child(img(s).absolute().inset_0().size_full().rounded(px(radius)).object_fit(ObjectFit::Cover)))
        .when(!length.is_empty(), |d| d.child(div().absolute().right(px(inset)).bottom(px(inset)).child(duration_badge(length))))
}

/// A round avatar: the picture, else YouTube's coloured circle with the first letter.
pub fn avatar(url: &str, name: &str, d: f32) -> Div {
    let base = div().flex_none().size(px(d)).rounded_full().overflow_hidden();
    if !url.is_empty() {
        return base.bg(tone(0.1)).child(img(SharedString::from(url.to_string())).size_full().rounded_full().object_fit(ObjectFit::Cover));
    }
    // YouTube's default avatars: a solid colour picked by the name, the initial in white.
    const COLORS: [u32; 8] = [0x5c6bc0, 0x00897b, 0xef6c00, 0x8e24aa, 0x3949ab, 0xc2185b, 0x00838f, 0x6d4c41];
    let k = name.bytes().fold(0usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize)) % COLORS.len();
    let initial = name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    base.bg(c(COLORS[k])).flex().items_center().justify_center().child(text(d * 0.5, d, FontWeight::NORMAL, 0xffffff).child(initial))
}

/// The verified check after a channel name.
pub fn verified(size: f32) -> AnyElement {
    ph_icon(mv_ui::ph::CHECK_CIRCLE, Weight::Fill, c(TEXT_2), size).into_any_element()
}

/// The YouTube logo: the red rounded play button and the wordmark.
pub fn logo() -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(2.))
        .child(
            div()
                .w(px(29.))
                .h(px(20.))
                .rounded(px(6.))
                .bg(c(RED))
                .flex()
                .items_center()
                .justify_center()
                .child(ph_icon(mv_ui::ph::PLAY, Weight::Fill, c(0xffffff), 11.)),
        )
        .child(div().text_size(px(20.)).line_height(px(20.)).font_weight(FontWeight::BOLD).text_color(c(0xffffff)).child("YouTube"))
}

/// A round tonal icon button (the watch page's Share, Download, Save, …).
pub fn round_button(id: impl Into<ElementId>, icon: Ph, d: f32) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(d))
        .rounded_full()
        .bg(tone(0.1))
        .hover(|s| s.bg(tone(0.2)))
        .cursor_pointer()
        .child(ph_icon(icon, Weight::Regular, c(TEXT), 22.))
}

/// A plain icon button with a hover circle (the masthead's).
pub fn icon_button(id: impl Into<ElementId>, icon: Ph, d: f32, size: f32) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(d))
        .rounded_full()
        .hover(|s| s.bg(tone(0.1)))
        .cursor_pointer()
        .child(ph_icon(icon, Weight::Regular, c(TEXT), size))
}

/// A filter chip under the masthead: white when selected.
pub fn chip(id: impl Into<ElementId>, label: impl Into<SharedString>, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .h(px(32.))
        .px(px(12.))
        .rounded(px(8.))
        .cursor_pointer()
        .map(|d| if on { d.bg(c(TEXT)) } else { d.bg(tone(0.1)).hover(|s| s.bg(tone(0.2))) })
        .child(text(14., 20., FontWeight::MEDIUM, if on { BG } else { TEXT }).whitespace_nowrap().child(label.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    // The glob above brings GPUI's own `test` attribute in: the standard one wins explicitly.
    use ::core::prelude::v1::test;

    #[test]
    fn youtube_texts() {
        assert_eq!(clock(7265.4), "2:01:05");
        assert_eq!(clock(59.), "0:59");
        assert_eq!(views(0), "No views");
        assert_eq!(views(1), "1 view");
        assert_eq!(views(134_000), "134K views");
        assert_eq!(views(1_234_567), "1.2M views");
        assert_eq!(ago_long(3 * 3600), "3 hours ago");
        assert_eq!(ago_long(86_400), "1 day ago");
        assert_eq!(ago_short("2 months ago"), "2mo ago");
        assert_eq!(ago_short("10 days ago"), "10d ago");
        assert_eq!(ago_short("Streamed 1 year ago"), "Streamed 1 year ago");
    }
}
