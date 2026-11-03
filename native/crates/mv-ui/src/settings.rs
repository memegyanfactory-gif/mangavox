//! The one layout every Settings page and embedded settings panel uses, so pages stop styling
//! rows one by one and nothing drifts: a fixed content column, groups with a title and an
//! optional one-line description above a bordered card, and rows inside the card separated by
//! hairlines. Label and description sit on the left and wrap; the control sits on the right.
//! Wide controls (an input with a button, a list, a picker grid) go under the text with
//! [`SettingsRow::stacked`] instead of pushing past the card edge.
//!
//! References: Windows 11 settings cards (header + description left, control right, content
//! wraps under the header when the card is narrow,
//! <https://learn.microsoft.com/en-us/dotnet/communitytoolkit/windows/settingscontrols/settingscard>),
//! WinUI app settings guidance (group under short section headers, single scrollable column
//! with a constrained width,
//! <https://learn.microsoft.com/en-us/windows/apps/design/app-settings/guidelines-for-app-settings>)
//! and macOS System Settings' grouped form (leading labels, trailing controls, rows grouped in
//! rounded sections, <https://developer.apple.com/documentation/swiftui/formstyle/grouped>).
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::feedback::AlertKind;
use crate::icons::{ph_icon, Ph, Weight};
use crate::theme::{icon as icon_px, radius, size, Tokens};

/// Width of the settings content column (rows, notes, headers), excluding the page padding.
pub const SETTINGS_CONTENT_W: f32 = 760.;
/// Horizontal page padding around the content column.
pub const SETTINGS_PAGE_X: f32 = 40.;
/// Space above the page heading. Every Settings page and sub-page starts its heading here.
pub const SETTINGS_PAGE_TOP: f32 = 32.;
/// Space between groups on a page.
pub const SETTINGS_GROUP_GAP: f32 = 28.;
/// Inner horizontal padding of a row (and of anything else inside a group card).
pub const SETTINGS_ROW_X: f32 = 16.;
/// Smallest row height, so rows with a switch and rows with a button line up.
pub const SETTINGS_ROW_MIN_H: f32 = 56.;

/// The scrollable page column every Settings page uses: content width, page padding and the
/// heading's top offset are the same everywhere, so the heading never moves between pages.
pub fn settings_column() -> Div {
    div()
        .w_full()
        .max_w(px(SETTINGS_CONTENT_W + 2. * SETTINGS_PAGE_X))
        .px(px(SETTINGS_PAGE_X))
        .pt(px(SETTINGS_PAGE_TOP))
        .pb(px(48.))
        .flex()
        .flex_col()
        .gap(px(24.))
}

/// Settings group: a title and optional description above a card of rows. Add rows with
/// `.child(..)`; hairlines between them are drawn for you.
/// `settings_group("Episodes", Some("What happens when an episode ends.".into())).child(settings_row(..))`.
pub fn settings_group(title: impl Into<SharedString>, desc: Option<SharedString>) -> SettingsGroup {
    SettingsGroup { title: title.into(), desc, trailing: Vec::new(), rows: Vec::new(), bare: false, danger: false }
}

/// See [`settings_group`].
pub struct SettingsGroup {
    title: SharedString,
    desc: Option<SharedString>,
    trailing: Vec<AnyElement>,
    rows: Vec<AnyElement>,
    bare: bool,
    danger: bool,
}

impl SettingsGroup {
    /// Element at the right end of the group title (a "Saved" mark, an "Add" button).
    pub fn trailing(mut self, el: impl IntoElement) -> Self {
        self.trailing.push(el.into_any_element());
        self
    }

    /// No card: the children sit straight under the title (a grid of theme tiles, a list of
    /// cards that are surfaces themselves).
    pub fn bare(mut self) -> Self {
        self.bare = true;
        self
    }

    /// Danger-edged card for irreversible actions, with a warning glyph by the title (GitHub's
    /// danger zone: <https://docs.github.com/en/repositories/creating-and-managing-repositories/deleting-a-repository>).
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    fn build(self) -> Div {
        let SettingsGroup { title, desc, trailing, rows, bare, danger } = self;
        let has_head = !title.is_empty() || desc.is_some() || !trailing.is_empty();
        let head = div()
            .flex()
            .items_end()
            .gap(px(12.))
            .px(px(4.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .when(!title.is_empty(), |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .when(danger, |d| d.child(ph_icon(crate::icons::ph::WARNING, Weight::Fill, Tokens::danger(), icon_px::SM)))
                                .child(div().text_size(px(size::MD)).line_height(px(20.)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title)),
                        )
                    })
                    .when_some(desc, |d, t| d.child(div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_3()).child(t))),
            )
            .when(!trailing.is_empty(), |d| d.child(div().flex().flex_none().items_center().gap(px(8.)).children(trailing)));
        let body = if bare {
            div().flex().flex_col().gap(px(12.)).children(rows)
        } else {
            let mut card = div()
                .flex()
                .flex_col()
                .rounded(px(radius::LG))
                .border_1()
                .border_color(if danger { Tokens::danger_border() } else { Tokens::border() })
                .bg(if danger { Tokens::surface().blend(Tokens::danger().alpha(0.04)) } else { Tokens::surface() });
            for (i, row) in rows.into_iter().enumerate() {
                card = card.child(div().flex().flex_col().min_w_0().when(i > 0, |d| d.border_t_1().border_color(Tokens::border())).child(row));
            }
            card
        };
        div().flex().flex_col().gap(px(10.)).min_w_0().when(has_head, |d| d.child(head)).child(body)
    }
}

impl ParentElement for SettingsGroup {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.rows.extend(elements);
    }
}

impl IntoElement for SettingsGroup {
    type Element = Div;
    fn into_element(self) -> Div {
        self.build()
    }
}

/// One setting inside a [`settings_group`]: label and optional description on the left (both
/// wrap), the control on the right. `.icon(..)` adds a leading glyph, `.stacked()` puts the
/// control under the text at full width, `.below(..)` adds content under the description (a
/// path, a validation message).
pub fn settings_row(label: impl Into<SharedString>, hint: impl Into<SharedString>, control: impl IntoElement) -> SettingsRow {
    SettingsRow { label: label.into(), hint: hint.into(), control: Some(control.into_any_element()), icon: None, stacked: false, below: Vec::new(), muted: false }
}

/// A row with text only (a fact, a note), no control.
pub fn settings_text_row(label: impl Into<SharedString>, hint: impl Into<SharedString>) -> SettingsRow {
    SettingsRow { label: label.into(), hint: hint.into(), control: None, icon: None, stacked: false, below: Vec::new(), muted: false }
}

/// See [`settings_row`].
pub struct SettingsRow {
    label: SharedString,
    hint: SharedString,
    control: Option<AnyElement>,
    icon: Option<Ph>,
    stacked: bool,
    below: Vec<AnyElement>,
    muted: bool,
}

impl SettingsRow {
    /// Leading 16px glyph, for rows that name a thing (a folder, a link, a device).
    pub fn icon(mut self, icon: Ph) -> Self {
        self.icon = Some(icon);
        self
    }

    /// The control goes under the label at full width (inputs with buttons, lists, grids).
    pub fn stacked(mut self) -> Self {
        self.stacked = true;
        self
    }

    /// Extra content under the description (a path chip, an error line, a meter).
    pub fn below(mut self, el: impl IntoElement) -> Self {
        self.below.push(el.into_any_element());
        self
    }

    /// Dims the label for a setting that does not apply right now (its control is disabled).
    pub fn muted(mut self, muted: bool) -> Self {
        self.muted = muted;
        self
    }

    fn build(self) -> Div {
        let SettingsRow { label, hint, control, icon, stacked, below, muted } = self;
        let text = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(2.))
            .when(!label.is_empty(), |d| {
                d.child(div().text_size(px(size::MD)).line_height(px(20.)).font_weight(FontWeight::MEDIUM).text_color(if muted { Tokens::text_3() } else { Tokens::text_1() }).child(label))
            })
            .when(!hint.is_empty(), |d| d.child(div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_3()).child(hint)))
            .when(!below.is_empty(), |d| d.child(div().flex().flex_col().gap(px(6.)).pt(px(4.)).min_w_0().children(below)));
        let lead = icon.map(|i| div().flex_none().pt(px(2.)).child(ph_icon(i, Weight::Regular, Tokens::text_2(), icon_px::MD)));
        let base = div().flex().px(px(SETTINGS_ROW_X)).py(px(12.)).min_h(px(SETTINGS_ROW_MIN_H)).min_w_0();
        if stacked {
            base.flex_col().gap(px(10.)).child(div().flex().items_start().gap(px(12.)).min_w_0().children(lead).child(text)).when_some(control, |d, c| d.child(div().flex().flex_col().min_w_0().w_full().child(c)))
        } else {
            base.items_center()
                .gap(px(16.))
                .child(div().flex().items_start().gap(px(12.)).flex_1().min_w_0().children(lead).child(text))
                // The control column never takes more than about two thirds of the row, so the
                // label always keeps room to wrap instead of being pushed out.
                .when_some(control, |d, c| d.child(div().flex().flex_none().max_w(relative(0.66)).flex_wrap().items_center().justify_end().gap(px(8.)).child(c)))
        }
    }
}

impl IntoElement for SettingsRow {
    type Element = Div;
    fn into_element(self) -> Div {
        self.build()
    }
}

/// Free-form content inside a group card with the row padding (a list, a meter, a chart).
pub fn settings_block(content: impl IntoElement) -> Div {
    div().flex().flex_col().min_w_0().px(px(SETTINGS_ROW_X)).py(px(12.)).child(content)
}

/// Inline note on a Settings page: tinted by kind, icon, wrapping text and an optional action
/// that sits at the end of the text and drops under it when space runs out, never outside the
/// note. Fluent 2 MessageBar: <https://fluent2.microsoft.design/components/web/react/core/messagebar/usage>.
pub fn settings_note(kind: AlertKind, text: impl Into<SharedString>) -> SettingsNote {
    SettingsNote { kind, text: text.into(), icon: None, action: None }
}

/// See [`settings_note`].
pub struct SettingsNote {
    kind: AlertKind,
    text: SharedString,
    icon: Option<Ph>,
    action: Option<AnyElement>,
}

impl SettingsNote {
    /// Replaces the kind's default glyph.
    pub fn icon(mut self, icon: Ph) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn action(mut self, el: impl IntoElement) -> Self {
        self.action = Some(el.into_any_element());
        self
    }

    fn build(self) -> Div {
        let SettingsNote { kind, text, icon, action } = self;
        let (fg, soft, edge, default_icon) = kind.colors();
        div()
            .flex()
            .items_start()
            .gap(px(10.))
            .min_w_0()
            .px(px(14.))
            .py(px(10.))
            .rounded(px(radius::LG))
            .bg(soft)
            .border_1()
            .border_color(edge.alpha(edge.a * 0.6))
            .child(div().flex_none().pt(px(2.)).child(ph_icon(icon.unwrap_or(default_icon), Weight::Fill, fg, icon_px::SM)))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .flex_wrap()
                    .items_center()
                    .gap_x(px(12.))
                    .gap_y(px(8.))
                    .child(div().flex_1().min_w(px(220.)).text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_1()).child(text))
                    .children(action.map(|a| div().flex_none().child(a))),
            )
    }
}

impl IntoElement for SettingsNote {
    type Element = Div;
    fn into_element(self) -> Div {
        self.build()
    }
}

/// Bumped when a key in Settings › AI services is saved or removed, so pages that use those
/// services (Studio's Thumbnails generator) can look again without depending on Settings.
pub struct ServiceKeysChanged(pub u64);
impl Global for ServiceKeysChanged {}

pub fn notify_service_keys(cx: &mut App) {
    let n = cx.try_global::<ServiceKeysChanged>().map(|g| g.0).unwrap_or(0);
    cx.set_global(ServiceKeysChanged(n + 1));
}
