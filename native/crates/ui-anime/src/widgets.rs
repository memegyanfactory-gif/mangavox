//! The engine's shared building blocks (design 4.4), so every step screen looks like the frame:
//! column surfaces and the three-column workspace, cards, section labels, spec chips, the
//! select-like field and pills, status chips (draft, needs review, approved, rejected, locked,
//! outdated), the "Auto" chip row of the inspectors, inline errors and info lines, strength dots,
//! character markers, thumbnails and plain-English time formatting. Usage: UI-GUIDE.md.
//!
//! Everything here takes its colours, radii, sizes and spacing from `mv_ui::theme`.


use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_anime::Status;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::{icon_ph, icon_tag, ph, ph_icon, size, tip, Hue, Interactive as _, Ph, Tokens, Tone, Weight};

use crate::style::{BULK_ASK_ABOVE, COLUMN_HEAD_H, GUTTER, LIST_ROW_H, LIST_W, LIST_W_NARROW, NARROW, PANEL_W, ROW_LABEL_W};

// ---------------------------------------------------------------- surfaces and layout

/// A column surface (Studio's `widgets::column`): rounded `sidebar` fill, hairline border and a
/// soft light edge along its top. Flex column; give it a width or `flex_1`.
pub fn column() -> Div {
    div()
        .relative()
        .flex()
        .flex_col()
        .min_h_0()
        .rounded(px(radius::LG))
        .overflow_hidden()
        .bg(Tokens::sidebar())
        .border_1()
        .border_color(Tokens::border())
        .child(div().absolute().top_0().left(px(space::LG)).right(px(space::LG)).h(px(1.)).bg(Tokens::glass_border()))
}

/// The left column of a step (`LIST_W` wide, full height).
pub fn list_column() -> Div {
    column().flex_none().w(px(LIST_W)).h_full()
}

/// The centre column of a step (takes the rest of the width).
pub fn center_column() -> Div {
    column().flex_1().min_w_0().h_full()
}

/// The right column of a step (`PANEL_W` wide): the Generation panel or an inspector.
pub fn panel_column() -> Div {
    column().flex_none().w(px(PANEL_W)).h_full()
}

/// The window is narrower than [`NARROW`]: compact step bar, right panel as a drawer.
pub fn is_narrow(window: &Window) -> bool {
    f32::from(window.viewport_size().width) < NARROW
}

/// A step's whole body: optional list column, the centre, and an optional right panel, with
/// the standard gutters. Below [`NARROW`] the panel floats over the centre's right edge as a
/// drawer; pass `None` to close it (open it from the selected item).
pub fn workspace(window: &Window, list: Option<AnyElement>, center: impl IntoElement, panel: Option<AnyElement>) -> Div {
    let narrow = is_narrow(window);
    div()
        .relative()
        .flex()
        .size_full()
        .min_h_0()
        .gap(px(GUTTER))
        .when_some(list, |d, l| d.child(list_column().when(narrow, |c| c.w(px(LIST_W_NARROW))).child(l)))
        .child(center_column().child(center))
        .when_some(panel, |d, p| {
            if narrow {
                d.child(div().absolute().top_0().bottom_0().right_0().w(px(PANEL_W)).child(panel_column().size_full().shadow_lg().child(p)))
            } else {
                d.child(panel_column().child(p))
            }
        })
}

/// The header row of a column: an uppercase label, then a spacer, so a count or buttons added
/// with `.child(..)` sit on the right.
pub fn column_head(title: &str) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(space::SM))
        .h(px(COLUMN_HEAD_H))
        .px(px(space::LG))
        .border_b_1()
        .border_color(Tokens::border())
        .child(label_caps(title))
        .child(div().flex_1())
}

/// The scrolling body under a [`column_head`]: padding MD, children MD apart.
pub fn column_body(id: impl Into<ElementId>) -> Stateful<Div> {
    div().id(id).flex().flex_col().flex_1().min_h_0().gap(px(space::MD)).p(px(space::MD)).overflow_y_scroll()
}

/// A content card: `surface` fill, hairline, large radius, padding LG, children MD apart.
pub fn card() -> Div {
    div().flex().flex_col().gap(px(space::MD)).p(px(space::LG)).rounded(px(radius::LG)).bg(Tokens::surface()).border_1().border_color(Tokens::border())
}

/// A clickable card or tile: `border_2 accent` + `accent_soft` when selected, hairline and
/// `surface` otherwise, `hover` fill. Add `.on_click(..)` and children.
pub fn selectable(id: impl Into<ElementId>, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_col()
        .rounded(px(radius::LG))
        .cursor_pointer()
        .map(|d| if selected { d.border_2().border_color(Tokens::accent()).bg(Tokens::accent_soft()) } else { d.border_1().border_color(Tokens::border()).bg(Tokens::surface()).hover(|s| s.bg(Tokens::hover())) })
}

/// A row of a scene or shot list: `mv_ui::list_row` at a fixed height that never shrinks, so a
/// long list scrolls instead of squashing its rows.
pub fn list_item(id: impl Into<ElementId>, selected: bool) -> Stateful<Div> {
    mv_ui::list_row(id, selected).flex_none().h(px(LIST_ROW_H))
}

/// Hover actions on a tile: put it inside a `.relative().group(group)` tile; it shows on hover.
pub fn hover_actions(group: impl Into<SharedString>) -> Div {
    div().absolute().top(px(6.)).right(px(6.)).flex().gap(px(space::XS)).invisible().group_hover(group.into(), |s| s.visible())
}

/// A labelled section inside a column or inspector: the uppercase label, then the caller's
/// children SM apart.
pub fn section(title: &str) -> Div {
    div().flex().flex_col().gap(px(space::SM)).child(label_caps(title))
}

/// One inspector row: a fixed-width XS label, then the control.
pub fn inspector_row(label: impl Into<SharedString>, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_start()
        .gap(px(space::SM))
        .child(div().flex_none().w(px(ROW_LABEL_W)).pt(px(5.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(label.into()))
        // A column, so a chip row inside takes exactly the control's width and wraps there.
        .child(div().flex().flex_col().flex_1().min_w_0().child(control))
}

// ---------------------------------------------------------------- text and chips

/// Uppercase XS semibold `text_2` label (thumb_view `label_caps`).
pub fn label_caps(text: &str) -> Div {
    div().flex_none().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_2()).child(text.to_uppercase())
}

/// [`label_caps`] with a counter on the right ("REFERENCES        7 / 14").
pub fn label_row(text: &str, right: impl Into<SharedString>) -> Div {
    div().flex().items_center().gap(px(space::SM)).child(label_caps(text)).child(div().flex_1()).child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(right.into()))
}

/// A small grey chip: "35 mm", "4.2 s", "1K".
pub fn spec(text: impl Into<SharedString>) -> Div {
    div().flex_none().flex().items_center().px(px(6.)).h(px(20.)).rounded(px(radius::SM)).bg(Tokens::surface_3()).text_size(px(size::XS)).text_color(Tokens::text_2()).whitespace_nowrap().child(text.into())
}

/// A 30 px option pill (aspect, size, count): `surface_2`, hover fill, SM semibold.
pub fn pill(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(30.))
        .px(px(10.))
        .rounded(px(radius::MD))
        .bg(Tokens::surface_2())
        .cursor_pointer()
        .hover(|s| s.bg(Tokens::hover()))
        .text_size(px(size::SM))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(Tokens::text_1())
        .whitespace_nowrap()
}

/// A select-like field (the Generation panel's model field): 36 px, `surface_2`, the value in
/// SM semibold and a caret; `border_strong` while its menu is `open`.
pub fn field(id: impl Into<ElementId>, icon: Option<Ph>, value: impl Into<SharedString>, open: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .gap(px(space::SM))
        .h(px(36.))
        .px(px(space::MD))
        .rounded(px(radius::LG))
        .bg(Tokens::surface_2())
        .border_1()
        .border_color(if open { Tokens::border_strong() } else { Tokens::surface_2() })
        .cursor_pointer()
        .hover(|s| s.bg(Tokens::hover()))
        .when_some(icon, |d, i| d.child(icon_ph(i, Tokens::text_1(), icon_px::SM)))
        .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(value.into()))
        .child(icon_ph(ph::CARET_DOWN, Tokens::text_2(), icon_px::XS))
}

// ---------------------------------------------------------------- statuses

/// The words of a card status.
pub fn status_label(s: Status) -> &'static str {
    match s {
        Status::Draft | Status::Unknown => "Draft",
        Status::NeedsReview => "Needs review",
        Status::Approved => "Approved",
        Status::Rejected => "Rejected",
    }
}

/// Status chip: Draft neutral, Needs review info, Approved success, Rejected danger, each with
/// its own icon so the status never rests on colour alone.
pub fn status_chip(s: Status) -> Div {
    let (icon, tone) = match s {
        Status::Draft | Status::Unknown => (ph::CIRCLE_DASHED, Tone::Neutral),
        Status::NeedsReview => (ph::EYE, Tone::Info),
        Status::Approved => (ph::CHECK, Tone::Success),
        Status::Rejected => (ph::X, Tone::Danger),
    };
    icon_tag(icon, status_label(s), tone)
}

/// "Locked": never changed automatically.
pub fn locked_chip() -> Div {
    icon_tag(ph::LOCK_SIMPLE, "Locked", Tone::Neutral)
}

/// "Outdated": an input changed since it was made. Warning tone, never red.
pub fn outdated_tag() -> Div {
    icon_tag(ph::ARROWS_CLOCKWISE, "Outdated", Tone::Warning)
}

/// A card's marks in one row: status, then Locked and Outdated when they apply.
pub fn card_marks(status: Status, locked: bool, outdated: bool) -> Div {
    div().flex().flex_wrap().items_center().gap(px(space::XS)).child(status_chip(status)).when(locked, |d| d.child(locked_chip())).when(outdated, |d| d.child(outdated_tag()))
}

// ---------------------------------------------------------------- the "Auto" chip row

type PickHandler<T> = Rc<dyn Fn(Option<T>, &mut Window, &mut App)>;

/// A chip row whose first chip is "Auto" (design 4.4, 5.4). Not `locked`: Auto is selected and
/// names the value the AI chose ("Auto · 35 mm"). `locked` (the user picked it): that chip is
/// selected in accent with a 6 px dot meaning "set by you"; pressing Auto resets it.
/// `on_pick` gets `Some(value)` for a pick and `None` for Auto.
pub fn auto_chips<T: Copy + PartialEq + 'static>(id: impl Into<SharedString>, options: impl IntoIterator<Item = (T, impl Into<SharedString>)>, value: T, locked: bool) -> AutoChips<T> {
    AutoChips { id: id.into(), options: options.into_iter().map(|(v, l)| (v, l.into())).collect(), value, locked, unset: None, on_pick: None }
}

/// See [`auto_chips`].
pub struct AutoChips<T> {
    id: SharedString,
    options: Vec<(T, SharedString)>,
    value: T,
    locked: bool,
    /// Shown after "Auto · " when the AI has not chosen anything yet ("not set").
    unset: Option<SharedString>,
    on_pick: Option<PickHandler<T>>,
}

/// Chip labels longer than this are shortened with an ellipsis and say themselves in full in a
/// tooltip.
const CHIP_FULL_CHARS: usize = 22;

impl<T: Copy + PartialEq + 'static> AutoChips<T> {
    pub fn on_pick(mut self, f: impl Fn(Option<T>, &mut Window, &mut App) + 'static) -> Self {
        self.on_pick = Some(Rc::new(f));
        self
    }

    /// What "Auto" says when the value matches no option ("Auto · not set").
    pub fn unset(mut self, label: impl Into<SharedString>) -> Self {
        self.unset = Some(label.into());
        self
    }

    pub fn build(self) -> Div {
        let AutoChips { id, options, value, locked, unset, on_pick } = self;
        let shown = options.iter().find(|(v, _)| *v == value).map(|(_, l)| l.clone()).or(unset);
        let auto_label: SharedString = match (&shown, locked) {
            (Some(l), false) => format!("Auto · {l}").into(),
            _ => "Auto".into(),
        };
        let auto_tip: SharedString = if locked {
            "Back to Auto: your pick is cleared and the AI chooses again".into()
        } else if auto_label.chars().count() > CHIP_FULL_CHARS {
            format!("{auto_label}: the AI chose this from the scene").into()
        } else {
            "The AI chooses this from the scene".into()
        };
        let pick = on_pick.clone();
        let auto = auto_chip(ElementId::NamedInteger(id.clone(), 0), auto_label, !locked).tooltip(tip(auto_tip)).when_some(pick, |d, f| d.on_click(move |_, w, cx| f(None, w, cx)));
        let mut row = div().flex().flex_wrap().min_w_0().gap(px(space::XS)).child(auto);
        for (i, (v, label)) in options.into_iter().enumerate() {
            let on = locked && v == value;
            let long = label.chars().count() > CHIP_FULL_CHARS;
            let full = label.clone();
            let f = on_pick.clone();
            row = row.child(
                option_chip(ElementId::NamedInteger(id.clone(), i as u64 + 1), label, on)
                    .when(on, |d| d.tooltip(tip(if long { format!("{full} · set by you; the AI keeps it") } else { "Set by you; the AI keeps it".to_string() })))
                    .when(!on && long, |d| d.tooltip(tip(full.clone())))
                    .when_some(f, |d, f| d.on_click(move |_, w, cx| f(Some(v), w, cx))),
            );
        }
        row
    }
}

impl<T: Copy + PartialEq + 'static> IntoElement for AutoChips<T> {
    type Element = Div;
    fn into_element(self) -> Div {
        self.build()
    }
}

/// A chip's words: one line, shortened with an ellipsis when the chip meets its row's edge.
fn chip_text(label: SharedString) -> Div {
    div().min_w_0().truncate().child(label)
}

fn chip_shell(id: ElementId) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .min_w_0()
        .max_w(relative(1.))
        .overflow_hidden()
        .items_center()
        .gap(px(5.))
        .h(px(26.))
        .px(px(9.))
        .rounded(px(radius::SM))
        .border_1()
        .text_size(px(size::SM))
        .whitespace_nowrap()
        .cursor_pointer()
        .role(Role::Button)
        .active(|s| s.bg(Tokens::pressed()))
        .focus_ring()
}

fn auto_chip(id: ElementId, label: SharedString, on: bool) -> Stateful<Div> {
    chip_shell(id)
        .aria_selected(on)
        .map(|d| if on { d.bg(Tokens::active()).border_color(Tokens::border_strong()).text_color(Tokens::text_1()).font_weight(FontWeight::MEDIUM) } else { d.border_color(Tokens::border()).text_color(Tokens::text_2()).hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1())) })
        .child(div().flex_none().child(icon_ph(ph::SPARKLE, if on { Tokens::accent() } else { Tokens::text_3() }, icon_px::XS)))
        .child(chip_text(label))
}

fn option_chip(id: ElementId, label: SharedString, on: bool) -> Stateful<Div> {
    chip_shell(id)
        .aria_selected(on)
        .map(|d| if on { d.bg(Tokens::accent_soft()).border_color(Tokens::accent_border()).text_color(Tokens::accent()).font_weight(FontWeight::MEDIUM) } else { d.border_color(Tokens::border()).text_color(Tokens::text_2()).hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1())) })
        .child(chip_text(label))
        .when(on, |d| d.child(div().flex_none().size(px(6.)).rounded_full().bg(Tokens::accent())))
}

// ---------------------------------------------------------------- messages

/// Colour family of an [`info_line`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineTone {
    Neutral,
    Info,
    Success,
    Warning,
    Danger,
}

fn line_colors(t: LineTone) -> (Rgba, Rgba) {
    match t {
        LineTone::Neutral => (Tokens::text_2(), Tokens::surface_2()),
        LineTone::Info => (Tokens::info(), Tokens::info_soft()),
        LineTone::Success => (Tokens::success(), Tokens::success_soft()),
        LineTone::Warning => (Tokens::warning(), Tokens::warning_soft()),
        LineTone::Danger => (Tokens::danger(), Tokens::danger_soft()),
    }
}

/// One quiet line on a tinted fill, like the line under Generate ("Credit-free on Magnific ·
/// about 45 s each"): `px MD py 6 rounded MD`, XS icon and XS text.
pub fn info_line(icon: Ph, tone: LineTone, text: impl Into<SharedString>) -> Div {
    let (fg, bg) = line_colors(tone);
    div().flex().items_start().gap(px(6.)).px(px(space::MD)).py(px(6.)).rounded(px(radius::MD)).bg(bg).child(div().pt(px(1.)).child(icon_ph(icon, fg, icon_px::XS))).child(div().flex_1().min_w_0().text_size(px(size::XS)).line_height(px(16.)).text_color(fg).whitespace_normal().child(text.into()))
}

/// A notice for a side panel or a narrow column: icon and title on one line, the body under
/// them, and the caller's action under that (`.child(btn(..).small())`), so the words always
/// get the panel's whole width instead of a column squeezed beside a link.
pub fn notice(icon: Ph, tone: LineTone, title: impl Into<SharedString>, body: impl Into<SharedString>) -> Div {
    let (fg, bg) = line_colors(tone);
    div()
        .flex()
        .flex_col()
        .items_start()
        .gap(px(space::XS))
        .p(px(space::MD))
        .rounded(px(radius::LG))
        .bg(bg)
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(space::SM))
                .w_full()
                .child(div().flex_none().pt(px(1.)).child(icon_ph(icon, fg, icon_px::SM)))
                .child(div().flex_1().min_w_0().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).line_height(px(18.)).text_color(Tokens::text_1()).whitespace_normal().child(title.into())),
        )
        .child(div().w_full().pl(px(icon_px::SM + space::SM)).text_size(px(size::XS)).line_height(px(16.)).text_color(Tokens::text_2()).whitespace_normal().child(body.into()))
}

/// A one-line warning with its action at the end, on a quiet neutral fill (it reads as "still to
/// do", never as an error, in every theme). Used for "The bible is not approved yet".
pub fn warning_strip(icon: Ph, text: impl Into<SharedString>, action: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(space::SM))
        .pl(px(space::MD))
        .pr(px(space::XS))
        .py(px(space::XS))
        .rounded(px(radius::MD))
        .bg(Tokens::surface_2())
        .border_1()
        .border_color(Tokens::border())
        .child(div().flex_none().child(icon_ph(icon, Tokens::warning(), icon_px::SM)))
        .child(div().flex_1().min_w_0().text_size(px(size::XS)).line_height(px(16.)).text_color(Tokens::text_2()).whitespace_normal().child(text.into()))
        .child(div().flex_none().child(action))
}

// ---------------------------------------------------------------- bulk actions

/// Typical seconds of one job of a kind before the queue has timed one on this PC.
fn typical_secs(kind: &str) -> f32 {
    match kind {
        "plan_scene" => 120.,
        "adapt_scene" => 90.,
        "frame" | "sheet" => 60.,
        "voice" => 15.,
        _ => 60.,
    }
}

/// "about 38 min" for `n` jobs of a kind (`JobKind::name()`): the measured mean when there is
/// one, else a typical time.
pub fn bulk_time(kind: &str, n: usize) -> String {
    let each = crate::queue_strip::Eta::current().mean(kind).filter(|m| m.is_finite() && *m > 0.).unwrap_or_else(|| typical_secs(kind));
    format!("about {}", fmt_duration((each * n as f32).round() as u64))
}

/// A bulk action of `n` jobs asks first ([`confirm_bulk`]); a handful or fewer
/// ([`BULK_ASK_ABOVE`]) just start.
pub fn asks_first(n: usize) -> bool {
    n > BULK_ASK_ABOVE
}

/// The question before a bulk action: the count in the title, what happens and the time in the
/// body. `go` runs on confirm (later, from the dialog: it may update the caller's entity).
#[allow(clippy::too_many_arguments)]
pub fn confirm_bulk(
    n: usize,
    kind: &'static str,
    title: impl Into<SharedString>,
    what: impl Into<SharedString>,
    confirm: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
    go: impl Fn(&mut Window, &mut App) + 'static,
) {
    let what: SharedString = what.into();
    let body = format!("{what} It takes {} in all. It runs in the queue, so you can keep working; \"Cancel all waiting\" in the queue stops what has not started.", bulk_time(kind, n));
    mv_ui::confirm_dialog(title, body, confirm, false).icon(ph::QUEUE).on_confirm(move |w, cx| go(w, cx)).open(window, cx);
}

/// An inline error block (thumb_view's): `danger_soft` fill, `danger_border`, a filled warning
/// icon and SM text. Add a dismiss or retry button with `.child(..)`.
pub fn error_block(text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_start()
        .gap(px(space::SM))
        .p(px(space::MD))
        .rounded(px(radius::LG))
        .bg(Tokens::danger_soft())
        .border_1()
        .border_color(Tokens::danger_border())
        .child(div().pt(px(1.)).child(ph_icon(ph::WARNING_CIRCLE, Weight::Fill, Tokens::danger(), icon_px::SM)))
        .child(div().flex_1().min_w_0().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_1()).whitespace_normal().child(text.into()))
}

/// The empty state inside a gallery: dashed outline, 32 px duotone icon, MD semibold title,
/// XS body (max 380 px). Use `mv_ui::empty_view(..).compact()` inside lists instead.
pub fn empty_gallery(icon: Ph, title: impl Into<SharedString>, body: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(space::SM))
        .min_h(px(240.))
        .p(px(space::LG))
        .rounded(px(radius::LG))
        .border_1()
        .border_dashed()
        .border_color(Tokens::border())
        .child(ph_icon(icon, Weight::Duotone, Tokens::text_3(), icon_px::HERO))
        .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title.into()))
        .child(div().max_w(px(380.)).text_center().text_size(px(size::XS)).text_color(Tokens::text_3()).whitespace_normal().child(body.into()))
}

// ---------------------------------------------------------------- small pieces

/// Strength as dots ("●●○"): `n` filled of `max`, accent on `track`.
pub fn strength_dots(n: u8, max: u8) -> Div {
    div().flex().flex_none().items_center().gap(px(3.)).children((0..max).map(|i| div().size(px(6.)).rounded_full().bg(if i < n { Tokens::accent() } else { Tokens::track() })))
}

/// The marker colour of the `i`-th character (maps, cast lists): the 8 categorical hues.
pub fn hue_for(i: usize) -> Hue {
    Hue::ALL[i % Hue::ALL.len()]
}

/// Two letters for a character marker, the same on every screen and on the maps:
/// "Sung Jinwoo" → "SJ", "Jinwoo" → "Ji", "" → "?".
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split(|c: char| c.is_whitespace() || c == '_' || c == '-').filter(|w| !w.is_empty()).collect();
    match words.as_slice() {
        [] => "?".into(),
        [one] => {
            let mut cs = one.chars();
            let first: String = cs.next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
            first + &cs.next().map(|c| c.to_lowercase().collect::<String>()).unwrap_or_default()
        }
        [a, b, ..] => [a, b].iter().filter_map(|w| w.chars().next()).flat_map(|c| c.to_uppercase()).collect(),
    }
}

/// A round character marker: the hue's soft fill, its initials in the hue.
pub fn marker(i: usize, name: &str, px_size: f32) -> Div {
    let hue = hue_for(i);
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(px_size))
        .rounded_full()
        .bg(Tokens::hue_soft(hue))
        .text_size(px((px_size * 0.42).max(size::XS - 1.)))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(Tokens::hue(hue))
        .child(initials(name))
}

/// A picture thumbnail of `w` x `h` through the shared thumbnail cache, or an image icon on
/// `surface_2` when there is no file yet. The file is not looked at here (render runs every
/// frame): a missing or unreadable one falls back to the same icon.
pub fn thumb(path: Option<PathBuf>, w: f32, h: f32) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(w))
        .h(px(h))
        .rounded(px(radius::MD))
        .overflow_hidden()
        .bg(Tokens::surface_2())
        .map(|d| match path {
            Some(p) => d.child(img(mv_ui::thumbs::cover_source(p)).size_full().object_fit(ObjectFit::Cover).with_loading(loading_fill).with_fallback(|| icon_ph(ph::IMAGE, Tokens::text_3(), icon_px::MD).into_any_element())),
            None => d.child(icon_ph(ph::IMAGE, Tokens::text_3(), icon_px::MD)),
        })
}

/// What a picture shows while it loads: the shared shimmer on `surface_2`.
pub fn loading_fill() -> AnyElement {
    div().relative().size_full().overflow_hidden().bg(Tokens::surface_2()).child(mv_ui::shimmer()).into_any_element()
}

// ---------------------------------------------------------------- formatting

/// "3:42" for seconds.
pub fn fmt_secs(t: f32) -> String {
    let t = if t.is_finite() { t.max(0.).round() as u64 } else { 0 };
    format!("{}:{:02}", t / 60, t % 60)
}

/// A wait or a run in plain words: "12 s", "4 min", "1 h 05 min".
pub fn fmt_duration(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 => format!("{} min", (secs + 30) / 60),
        _ => format!("{} h {:02} min", secs / 3600, (secs % 3600) / 60),
    }
}

/// Engine timestamps in seconds: values written in milliseconds are scaled down, so either
/// unit reads right.
pub fn unix_secs(t: i64) -> i64 {
    if t > 100_000_000_000 {
        t / 1000
    } else {
        t
    }
}

/// Seconds since the Unix epoch now.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// "just now", "4 min ago", "2 h ago", "3 days ago" for a timestamp (seconds or ms).
pub fn ago(t: i64, now: i64) -> String {
    let s = (now - unix_secs(t)).max(0) as u64;
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        _ => {
            let d = s / 86_400;
            format!("{d} day{} ago", plural(d as usize))
        }
    }
}

pub fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// "1 shot", "3 shots".
pub fn count(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", plural(n))
}

#[cfg(test)]
mod tests {
    // Explicit imports: with test-support, `gpui_kit::*` carries a `test` macro that shadows `#[test]`.
    use super::{ago, count, fmt_duration, fmt_secs, hue_for, initials, status_label, unix_secs};
    use mv_anime::Status;

    #[test]
    fn times_read_naturally() {
        assert_eq!(fmt_secs(161.4), "2:41");
        assert_eq!(fmt_secs(f32::NAN), "0:00");
        assert_eq!(fmt_duration(38), "38 s");
        assert_eq!(fmt_duration(240), "4 min");
        assert_eq!(fmt_duration(3900), "1 h 05 min");
        assert_eq!(ago(1_000, 1_030), "just now");
        assert_eq!(ago(1_000_000, 1_000_300), "5 min ago");
        assert_eq!(ago(0, 90_000), "1 day ago");
    }

    #[test]
    fn milliseconds_read_as_seconds() {
        assert_eq!(unix_secs(1_763_600_000), 1_763_600_000);
        assert_eq!(unix_secs(1_763_600_000_123), 1_763_600_000);
        assert_eq!(ago(1_763_600_000_000, 1_763_600_120), "2 min ago");
    }

    #[test]
    fn names_and_counts() {
        assert_eq!(initials("Sung Jinwoo"), "SJ");
        assert_eq!(initials("ahn"), "Ah");
        assert_eq!(initials("  "), "?");
        assert_eq!(count(1, "shot"), "1 shot");
        assert_eq!(count(3, "shot"), "3 shots");
        assert_ne!(hue_for(0), hue_for(1));
        assert_eq!(hue_for(0), hue_for(8));
    }

    #[test]
    fn statuses_have_words() {
        assert_eq!(status_label(Status::NeedsReview), "Needs review");
        assert_eq!(status_label(Status::Unknown), "Draft");
    }
}
