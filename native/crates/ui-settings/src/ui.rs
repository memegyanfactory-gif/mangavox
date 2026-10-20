//! Layout pieces shared by every Settings page that the mv-ui kit does not have yet: the rail
//! item and page heading with a category-coloured icon tile, a setting row with a leading icon,
//! the danger zone, the "Default" star tag and small fact chips. Everything else comes straight
//! from the kit (`setting_section`, `setting_row`, `btn`, `switch`, `status_tag`, ...).
//! Candidates for promotion to mv-ui are marked `Promote:`.
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::{icon as icon_px, radius};
use mv_ui::{ph, ph_icon, size, Hue, Interactive as _, Ph, Tokens, Weight};

/// Icon on a category-coloured tile. Soft tint with a hue glyph at rest; `filled` (the selected
/// rail item) is a solid hue with a dark fill-weight glyph, so selection changes shape and fill,
/// not just colour (Fluent 2 iconography: filled marks selection,
/// <https://fluent2.microsoft.design/iconography>; macOS System Settings colour tiles).
/// Promote: `mv_ui::hue_tile`.
pub fn hue_tile(icon: Ph, hue: Hue, side: f32, filled: bool) -> Div {
    let glyph = (side * 0.6).round();
    let (bg, fg, weight) = if filled {
        (Tokens::hue(hue), Tokens::base(), Weight::Fill)
    } else {
        (Tokens::hue_soft(hue), Tokens::hue(hue), if glyph <= icon_px::SM { Weight::Bold } else { Weight::Regular })
    };
    div().flex().flex_none().items_center().justify_center().size(px(side)).rounded(px((side * 0.28).round())).bg(bg).child(ph_icon(icon, weight, fg, glyph))
}

/// Large duotone tile for page headings and hero cards.
pub fn hero_tile(icon: Ph, hue: Hue, side: f32) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(side))
        .rounded(px(radius::LG))
        .bg(Tokens::hue_soft(hue))
        .child(ph_icon(icon, Weight::Duotone, Tokens::hue(hue), (side * 0.55).round()))
}

/// Settings rail destination: the kit `nav_item` states (active container, 3px accent indicator,
/// hover, pressed, focus ring) with a 22px hue tile instead of a bare glyph, so 15 destinations
/// scan by colour and shape. Apple HIG sidebars: <https://developer.apple.com/design/human-interface-guidelines/sidebars>.
/// Promote: `nav_item(..).tile(Hue)`.
pub fn rail_item(id: impl Into<ElementId>, icon: Ph, hue: Hue, label: impl Into<SharedString>, active: bool) -> Stateful<Div> {
    let label: SharedString = label.into();
    div()
        .id(id)
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(10.))
        .w_full()
        .h(px(34.))
        .px(px(8.))
        .rounded(px(radius::MD))
        .text_size(px(size::MD))
        .text_color(if active { Tokens::text_1() } else { Tokens::text_2() })
        .role(Role::Link)
        .aria_label(label.clone())
        .aria_selected(active)
        .map(|d| {
            if active {
                d.bg(Tokens::active()).font_weight(FontWeight::MEDIUM).hover(|s| s.bg(Tokens::active_hover()))
            } else {
                d.hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
            }
        })
        .pressable()
        .when(active, |d| d.child(div().absolute().left(px(0.)).top(px(10.)).w(px(3.)).h(px(14.)).rounded_full().bg(Tokens::accent())))
        .child(hue_tile(icon, hue, 22., active))
        .child(div().flex_1().min_w_0().truncate().child(label))
}

/// Page heading: a 40px hue tile matching the rail item, the title and a one-line description.
/// Promote: `page_title(..).hue(Hue)`.
pub fn page_head(icon: Ph, hue: Hue, title: impl Into<SharedString>, desc: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(14.))
        .child(hero_tile(icon, hue, 40.))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(div().text_size(px(size::XL)).line_height(px(28.)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title.into()))
                .child(div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_3()).child(desc.into())),
        )
}

/// `setting_row` with a leading 16px icon, for rows that name a thing (CPU, a folder, a link).
/// Promote: `setting_row(..).icon(Ph)`.
pub fn icon_row(icon: Ph, label: impl Into<SharedString>, hint: impl Into<SharedString>, control: impl IntoElement) -> Div {
    let hint: SharedString = hint.into();
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(24.))
        .py(px(12.))
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(12.))
                .flex_1()
                .min_w_0()
                .child(div().pt(px(2.)).child(ph_icon(icon, Weight::Regular, Tokens::text_2(), icon_px::MD)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .flex_1()
                        .min_w_0()
                        .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(label.into()))
                        .when(!hint.is_empty(), |d| d.child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(hint))),
                ),
        )
        .child(div().flex().flex_none().items_center().gap(px(6.)).child(control))
}

/// Row for a folder or file: icon, label, optional hint and the path as a middle-truncated
/// monospace chip with a copy button (both ends of a path carry meaning), actions on the right.
pub fn path_row(icon: Ph, label: impl Into<SharedString>, hint: impl Into<SharedString>, path: impl Into<SharedString>, control: impl IntoElement) -> Div {
    let hint: SharedString = hint.into();
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.))
        .py(px(12.))
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(12.))
                .flex_1()
                .min_w_0()
                .child(div().pt(px(2.)).child(ph_icon(icon, Weight::Regular, Tokens::text_2(), icon_px::MD)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .flex_1()
                        .min_w_0()
                        .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(label.into()))
                        .when(!hint.is_empty(), |d| d.child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(hint)))
                        .child(div().max_w(px(460.)).child(mv_ui::path_chip(path))),
                ),
        )
        .child(div().flex().flex_none().items_center().gap(px(2.)).child(control))
}

/// Hairline above a row, so rows inside a section read as one list.
pub fn divided(row: Div) -> Div {
    row.border_t_1().border_color(Tokens::border())
}

/// Bordered surface for cards inside a section (job cards, engine cards).
pub fn surface() -> Div {
    div().flex().flex_col().rounded(px(radius::LG)).bg(Tokens::surface()).border_1().border_color(Tokens::border())
}

/// GitHub-style danger zone: irreversible actions grouped at the bottom of the page in one
/// danger-edged surface. <https://docs.github.com/en/repositories/creating-and-managing-repositories/deleting-a-repository>.
/// Promote: `mv_ui::danger_zone`.
pub fn danger_zone(title: impl Into<SharedString>, desc: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .mt(px(8.))
        .px(px(16.))
        .pt(px(14.))
        .pb(px(4.))
        .rounded(px(radius::LG))
        .border_1()
        .border_color(Tokens::danger_border())
        .bg(Tokens::danger().alpha(0.04))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(ph_icon(ph::WARNING, Weight::Fill, Tokens::danger(), icon_px::MD))
                .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title.into())),
        )
        .child(div().pt(px(2.)).pb(px(6.)).text_size(px(size::SM)).text_color(Tokens::text_3()).child(desc.into()))
}

/// "★ Default" in the favourite gold with a FILLED star: the default narrator mark.
/// Promote: `icon_tag(..)` with a fill weight (it is always bold today).
pub fn default_tag() -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .h(px(20.))
        .pl(px(5.))
        .pr(px(7.))
        .rounded(px(radius::PILL))
        .bg(Tokens::favorite().alpha(0.14))
        .text_size(px(size::XS))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(Tokens::favorite())
        .whitespace_nowrap()
        .child(ph_icon(ph::STAR, Weight::Fill, Tokens::favorite(), icon_px::XS))
        .child("Default")
}

/// Status pill with an icon, so status never rests on colour alone (WCAG 1.4.1).
/// Covers the states `status_tag` has no kind for (warning, idle). Promote: `StatusKind::Warning`
/// and `StatusKind::Idle`.
pub fn state_pill(icon: Ph, color: Rgba, fill: Rgba, text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(5.))
        .h(px(22.))
        .px(px(8.))
        .rounded(px(radius::PILL))
        .bg(fill)
        .text_size(px(size::XS))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .whitespace_nowrap()
        .child(ph_icon(icon, Weight::Bold, color, icon_px::XS))
        .child(text.into())
}

/// Inline fact: bold 12px icon and a short value ("354 MB", "English, Hindi").
pub fn fact(icon: Ph, text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(5.))
        .text_size(px(size::SM))
        .text_color(Tokens::text_2())
        .whitespace_nowrap()
        .child(ph_icon(icon, Weight::Bold, Tokens::text_3(), icon_px::XS))
        .child(text.into())
}

/// Labelled 1-5 meter ("Quality ▮▮▮▮▯") built on the kit `meter`.
pub fn meter_fact(icon: Ph, label: &'static str, value: u8) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .text_size(px(size::SM))
        .text_color(Tokens::text_2())
        .child(ph_icon(icon, Weight::Bold, Tokens::text_3(), icon_px::XS))
        .child(label)
        .child(mv_ui::meter(value as f32, 5))
}

/// Passive "Saved" confirmation shown next to a section for a moment after an autosave.
/// NN/g: confirm system status near the element it describes,
/// <https://www.nngroup.com/articles/indicators-validations-notifications/>.
pub fn saved_mark() -> AnyElement {
    mv_ui::appear(
        "saved-mark",
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(px(size::XS))
            .text_color(Tokens::text_3())
            .child(ph_icon(ph::CHECK, Weight::Bold, Tokens::success(), icon_px::XS))
            .child("Saved"),
    )
}

/// Monospace command with a copy button ("winget install ...").
pub fn command_chip(id: impl Into<ElementId>, cmd: impl Into<SharedString>) -> Div {
    let cmd: SharedString = cmd.into();
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .min_w_0()
        .h(px(28.))
        .pl(px(8.))
        .pr(px(2.))
        .rounded(px(radius::SM))
        .bg(Tokens::well())
        .border_1()
        .border_color(Tokens::border())
        .child(ph_icon(ph::TERMINAL_WINDOW, Weight::Bold, Tokens::text_3(), icon_px::SM))
        .child(div().flex_1().min_w_0().truncate().font_family(mv_ui::MONO_FONT).text_size(px(size::SM)).text_color(Tokens::text_2()).child(cmd.clone()))
        .child(mv_ui::copy_button(id, cmd))
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
