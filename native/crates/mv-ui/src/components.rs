//! Small, consistent building blocks. Keep pages quiet: one primary action,
//! secondary actions as ghost buttons, icons only where they carry meaning.
//! Buttons and selection controls live in [`crate::controls`], status and feedback in
//! [`crate::feedback`], menus in [`crate::menu`].
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::controls::{btn, ButtonKind, ClickHandler};
use crate::icons::{ph, Ph, Weight};
use crate::interact::{element_builder, glyph, refine, tip, IconSrc, Interactive};
use crate::motion::shimmer;
use crate::theme::{icon as icon_px, radius, size, Tokens};

/// Monospace family for paths, commands and ids (always installed on Windows).
pub const MONO_FONT: &str = "Consolas";

/// Lucide icon from the bundled set, e.g. `icon("icons/play.svg", Tokens::text_2(), 16.)`.
pub fn icon(path: &'static str, color: Rgba, px_size: f32) -> Svg {
    svg().path(path).size(px(px_size)).flex_none().text_color(color)
}

/// Page title row: title, muted subtitle, then right-aligned actions.
/// For an icon, a subtitle line and actions use [`page_title`].
pub fn page_header(title: impl Into<SharedString>, subtitle: Option<SharedString>) -> Div {
    div()
        .flex()
        .items_baseline()
        .gap(px(10.))
        .mb(px(20.))
        .child(div().text_size(px(size::XL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title.into()))
        .when_some(subtitle, |d, s| d.child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(s)))
}

/// Page header builder: optional icon tile, title, subtitle line and right-aligned actions, with
/// the standard 20px gap below (so pages stop wrapping toolbars in margin hacks).
/// `page_title("Library").icon(ph::BOOKS).subtitle("128 series").action(btn(..))`.
/// NN/g visual hierarchy: <https://www.nngroup.com/articles/visual-hierarchy-ux-definition/>
pub fn page_title(title: impl Into<SharedString>) -> PageHeader {
    PageHeader { title: title.into(), icon: None, subtitle: None, actions: Vec::new(), style: StyleRefinement::default() }
}

/// See [`page_title`].
pub struct PageHeader {
    title: SharedString,
    icon: Option<Ph>,
    subtitle: Option<SharedString>,
    actions: Vec<AnyElement>,
    style: StyleRefinement,
}

impl Styled for PageHeader {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl IntoElement for PageHeader {
    type Element = Div;
    fn into_element(self) -> Div {
        self.build()
    }
}

impl PageHeader {
    /// Icon in a tinted 32px tile before the title.
    pub fn icon(mut self, icon: Ph) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn subtitle(mut self, text: impl Into<SharedString>) -> Self {
        self.subtitle = Some(text.into());
        self
    }

    /// Adds one element to the right-aligned action group.
    pub fn action(mut self, el: impl IntoElement) -> Self {
        self.actions.push(el.into_any_element());
        self
    }

    /// Adds several elements to the action group.
    pub fn actions(mut self, els: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.actions.extend(els.into_iter().map(IntoElement::into_any_element));
        self
    }

    pub fn build(self) -> Div {
        let PageHeader { title, icon, subtitle, actions, style } = self;
        let el = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .mb(px(20.))
            .when_some(icon, |d, i| {
                d.child(div().flex().flex_none().items_center().justify_center().size(px(32.)).rounded(px(radius::MD)).bg(Tokens::accent_soft()).child(glyph(IconSrc::Ph(i), Weight::Duotone, Tokens::accent(), icon_px::LG)))
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_size(px(size::XL)).line_height(px(28.)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).truncate().child(title))
                    .when_some(subtitle, |d, s| d.child(div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_3()).truncate().child(s))),
            )
            .when(!actions.is_empty(), |d| d.child(div().flex().flex_none().items_center().gap(px(8.)).children(actions)));
        refine(el, &style)
    }
}

/// Standard scrollable page body with page padding.
pub fn page(id: impl Into<ElementId>) -> Stateful<Div> {
    div().id(id).size_full().overflow_y_scroll().px(px(size::PAGE_X)).py(px(size::PAGE_Y))
}

/// Scrollable page whose position lives in `handle`, so it survives leaving and coming back.
pub fn scroll_page(id: impl Into<ElementId>, handle: &ScrollHandle) -> Stateful<Div> {
    div().id(id).size_full().overflow_y_scroll().track_scroll(handle)
}

/// Pill used for tabs and quick switches. `active` gets the neutral highlight.
/// For selectable filters use [`crate::filter_chip`] (it adds the check).
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
        .role(Role::Button)
        .aria_selected(active)
        .map(|d| {
            if active {
                d.bg(Tokens::active()).border_color(Tokens::border_strong()).text_color(Tokens::text_1())
            } else {
                d.border_color(Tokens::border()).text_color(Tokens::text_2()).hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
            }
        })
        .active(|s| s.bg(Tokens::pressed()))
        .focus_ring()
        .child(label.into())
}

/// Poster/cover card. Covers are local paths or http URLs. Same card as [`cover_tile`], which
/// adds a progress line, a favourite badge and a width.
pub fn cover_card(id: impl Into<ElementId>, title: SharedString, sub: SharedString, cover: Option<CoverSource>, badge: Option<SharedString>) -> Stateful<Div> {
    let mut card = cover_tile(id, title).sub(sub).cover(cover);
    card.badge = badge;
    card.build()
}

/// Cover card builder: a poster that lifts on hover and settles when pressed, a loading shimmer
/// and a broken-image fallback, a 2-line title (full title in a tooltip when it is long),
/// optional reading/watching progress line, favourite star and corner badge.
/// `cover_tile("m-12", title).sub("Ch. 40").cover(src).progress(0.4).favorite(true).on_click(..)`.
/// Material 3 cards: <https://m3.material.io/components/cards/guidelines>
pub fn cover_tile(id: impl Into<ElementId>, title: impl Into<SharedString>) -> CoverCard {
    CoverCard {
        base: div().id(id),
        style: StyleRefinement::default(),
        title: title.into(),
        sub: None,
        cover: None,
        badge: None,
        progress: None,
        favorite: false,
        width: 150.,
        on_click: None,
    }
}

/// See [`cover_tile`].
pub struct CoverCard {
    base: Stateful<Div>,
    style: StyleRefinement,
    title: SharedString,
    sub: Option<SharedString>,
    cover: Option<CoverSource>,
    badge: Option<SharedString>,
    progress: Option<f32>,
    favorite: bool,
    width: f32,
    on_click: Option<ClickHandler>,
}

element_builder!(CoverCard => Stateful<Div>);

impl CoverCard {
    /// Second line (chapter, year, episode count).
    #[allow(clippy::should_implement_trait)]
    pub fn sub(mut self, text: impl Into<SharedString>) -> Self {
        let text = text.into();
        self.sub = (!text.is_empty()).then_some(text);
        self
    }

    pub fn cover(mut self, cover: impl Into<Option<CoverSource>>) -> Self {
        self.cover = cover.into();
        self
    }

    /// Short text in the top-right corner ("12 new", "EN").
    pub fn badge(mut self, text: impl Into<SharedString>) -> Self {
        self.badge = Some(text.into());
        self
    }

    /// Thin accent line along the bottom of the poster, 0 to 1.
    pub fn progress(mut self, p: f32) -> Self {
        self.progress = Some(p.clamp(0., 1.));
        self
    }

    /// Filled gold star in the top-left corner.
    pub fn favorite(mut self, on: bool) -> Self {
        self.favorite = on;
        self
    }

    /// Card width in px (default 150); the poster keeps the 150:214 shape.
    pub fn width(mut self, px_w: f32) -> Self {
        self.width = px_w;
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    pub fn build(self) -> Stateful<Div> {
        let CoverCard { base, style, title, sub, cover, badge, progress, favorite, width: w, on_click } = self;
        let h = (w * 214. / 150.).round();
        let long_title = title.chars().count() > (w / 6.5 * 2.) as usize;
        let placeholder = |icon: Ph| div().size_full().flex().items_center().justify_center().bg(Tokens::surface_2()).child(glyph(IconSrc::Ph(icon), Weight::Duotone, Tokens::text_3(), icon_px::HERO)).into_any_element();
        let poster = div()
            .id("mv-poster")
            .relative()
            .w(px(w))
            .h(px(h))
            .rounded(px(10.))
            .overflow_hidden()
            .bg(Tokens::surface_2())
            .border_1()
            .border_color(Tokens::border())
            .group_hover("mv-cover", |s| s.top(px(-3.)).border_color(Tokens::border_strong()).shadow_lg())
            .group_active("mv-cover", |s| s.top(px(-1.)).shadow_md())
            .map(|d| match cover {
                Some(c) => d.child(
                    c.into_img()
                        .size_full()
                        .object_fit(ObjectFit::Cover)
                        .with_loading(|| div().relative().size_full().overflow_hidden().bg(Tokens::surface_2()).child(shimmer()).into_any_element())
                        .with_fallback(move || placeholder(ph::IMAGE_BROKEN)),
                ),
                None => d.child(placeholder(ph::IMAGE)),
            })
            .when(favorite, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(6.))
                        .left(px(6.))
                        .size(px(22.))
                        .rounded_full()
                        .bg(Tokens::glass())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(glyph(IconSrc::Ph(ph::STAR), Weight::Fill, Tokens::favorite(), icon_px::XS)),
                )
            })
            .when_some(badge, |d, b| {
                d.child(
                    div()
                        .absolute()
                        .top(px(6.))
                        .right(px(6.))
                        .px(px(6.))
                        .py(px(1.))
                        .rounded(px(radius::SM))
                        .bg(Tokens::scrim())
                        .text_size(px(size::XS))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(Tokens::text_1())
                        .child(b),
                )
            })
            .when_some(progress, |d, p| d.child(div().absolute().left_0().right_0().bottom_0().h(px(3.)).bg(Tokens::scrim()).child(div().h_full().w(relative(p)).bg(Tokens::accent()))));
        let el = base
            .flex()
            .flex_col()
            .gap(px(8.))
            .w(px(w))
            .rounded(px(12.))
            .cursor_pointer()
            .group("mv-cover")
            .role(Role::Link)
            .aria_label(title.clone())
            .focus_ring()
            .when_some(on_click, |d, f| d.on_click(move |e, wnd, cx| f(e, wnd, cx)))
            .child(poster)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .id("mv-cover-title")
                            .text_size(px(size::SM))
                            .line_height(px(16.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(Tokens::text_1())
                            .line_clamp(2)
                            .text_ellipsis()
                            .when(long_title, |d| d.tooltip(tip(title.clone())))
                            .child(title),
                    )
                    .when_some(sub, |d, s| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).truncate().child(s))),
            );
        refine(el, &style)
    }
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

/// Centered message for empty / error / not-yet states.
/// For an icon and actions use [`crate::empty_view`] / [`crate::error_view`].
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
/// For an icon, a count or a "See all" action use [`section_header`].
pub fn section_title(text: impl Into<SharedString>) -> Div {
    div().mt(px(8.)).mb(px(12.)).text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(text.into())
}

/// Section heading builder: optional icon, title, count and a trailing action ("See all").
/// `section_header("Continue reading", Some(ph::BOOK_OPEN)).see_all(|_, w, cx| ..)`.
pub fn section_header(title: impl Into<SharedString>, icon: Option<Ph>) -> SectionHeader {
    SectionHeader { title: title.into(), icon, count: None, trailing: Vec::new(), style: StyleRefinement::default() }
}

/// See [`section_header`].
pub struct SectionHeader {
    title: SharedString,
    icon: Option<Ph>,
    count: Option<usize>,
    trailing: Vec<AnyElement>,
    style: StyleRefinement,
}

impl Styled for SectionHeader {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl IntoElement for SectionHeader {
    type Element = Div;
    fn into_element(self) -> Div {
        self.build()
    }
}

impl SectionHeader {
    /// Muted count after the title.
    pub fn count(mut self, n: usize) -> Self {
        self.count = Some(n);
        self
    }

    /// "See all" text button with a caret, at the end of the row.
    pub fn see_all(self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.action("See all", handler)
    }

    /// Any text action at the end of the row.
    pub fn action(mut self, label: impl Into<SharedString>, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        let label = label.into();
        let id = SharedString::from(format!("mv-section-{}-{}", self.title, label));
        self.trailing.push(btn(id, label, ButtonKind::Ghost).small().trailing_icon(ph::CARET_RIGHT).on_click(handler).into_any_element());
        self
    }

    /// Any element at the end of the row (a segmented control, a menu).
    pub fn trailing(mut self, el: impl IntoElement) -> Self {
        self.trailing.push(el.into_any_element());
        self
    }

    pub fn build(self) -> Div {
        let SectionHeader { title, icon, count, trailing, style } = self;
        let el = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .mt(px(8.))
            .mb(px(12.))
            .when_some(icon, |d, i| d.child(glyph(IconSrc::Ph(i), Weight::Regular, Tokens::text_2(), icon_px::LG)))
            .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title))
            .when_some(count, |d, n| d.child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(n.to_string())))
            .child(div().flex_1())
            .children(trailing);
        refine(el, &style)
    }
}

/// Small uppercase group label for panels and settings groups ("VOICES"). Append a trailing
/// element with `.child(..)`.
pub fn section_label(text: impl Into<SharedString>) -> Div {
    let text: SharedString = text.into();
    div().flex().items_center().gap(px(6.)).px(px(2.)).pb(px(4.)).text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_3()).child(text.to_uppercase())
}

/// Secondary text: metadata, hints and counts in `text_3`.
pub fn muted(text: impl Into<SharedString>) -> Div {
    div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(text.into())
}

/// Settings group: a hairline above, a title and an optional one-line description, then its
/// [`setting_row`]s as children. Flat (no card), so groups never nest boxes in boxes.
/// Fluent 2 settings layout: <https://learn.microsoft.com/en-us/windows/apps/design/app-settings/guidelines-for-app-settings>
pub fn setting_section(title: impl Into<SharedString>, desc: Option<SharedString>) -> Div {
    let head = div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .pb(px(6.))
        .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title.into()))
        .when_some(desc, |d, t| d.child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(t)));
    div().flex().flex_col().border_t_1().border_color(Tokens::border()).pt(px(18.)).pb(px(6.)).child(head)
}

/// One setting: label and optional hint on the left, its control (switch, select, button) on the
/// right. Put a hairline between rows with `.border_t_1().border_color(Tokens::border())`.
pub fn setting_row(label: impl Into<SharedString>, hint: impl Into<SharedString>, control: impl IntoElement) -> Div {
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
                .flex_col()
                .gap(px(2.))
                .flex_1()
                .min_w_0()
                .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(label.into()))
                .when(!hint.is_empty(), |d| d.child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(hint))),
        )
        .child(div().flex().flex_none().items_center().gap(px(6.)).child(control))
}

/// Horizontal list of cards that wraps.
pub fn card_grid() -> Div {
    div().flex().flex_wrap().gap(px(20.))
}

/// Copy-to-clipboard icon button (gpui-component `Clipboard`): shows a check for 2 s after copying.
pub fn copy_button(id: impl Into<ElementId>, value: impl Into<SharedString>) -> Clipboard {
    Clipboard::new(id).value(value).tooltip("Copy")
}

/// A file or folder path: monospace, shortened in the middle so both ends stay readable, full
/// path in the tooltip, with a copy button.
pub fn path_chip(path: impl Into<SharedString>) -> Stateful<Div> {
    let path: SharedString = path.into();
    div()
        .id(SharedString::from(format!("mv-path-{path}")))
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
        .child(glyph(IconSrc::Ph(ph::FOLDER_SIMPLE), Weight::Bold, Tokens::text_3(), icon_px::SM))
        .child(
            div()
                .id("mv-path-text")
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis_middle()
                .font_family(MONO_FONT)
                .text_size(px(size::SM))
                .text_color(Tokens::text_2())
                .tooltip(tip(path.clone()))
                .child(path.clone()),
        )
        .child(copy_button("mv-path-copy", path))
}

/// Link that leaves the app: accent text with a trailing arrow, opens `url` in the browser,
/// shows the address in its tooltip.
pub fn external_link(label: impl Into<SharedString>, url: impl Into<SharedString>) -> Stateful<Div> {
    let url: SharedString = url.into();
    let open = url.clone();
    div()
        .id(SharedString::from(format!("mv-link-{url}")))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .rounded(px(radius::XS))
        .text_size(px(size::SM))
        .text_color(Tokens::accent())
        .cursor_pointer()
        .role(Role::Link)
        .hover(|s| s.text_color(Tokens::accent_hover()).underline())
        .focus_ring()
        .tooltip(tip(url))
        .on_click(move |_, _, cx| cx.open_url(&open))
        .child(label.into())
        .child(glyph(IconSrc::Ph(ph::ARROW_UP_RIGHT), Weight::Bold, Tokens::accent(), icon_px::XS))
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

/// Display helper shared by pages: "1.2 GB", "340 MB", "—" for zero.
pub fn fmt_bytes(n: u64) -> String {
    if n == 0 {
        return "—".into();
    }
    let gb = n as f64 / 1024f64.powi(3);
    if gb >= 1.0 {
        format!("{:.*} GB", if gb >= 10.0 { 0 } else { 1 }, gb)
    } else {
        format!("{} MB", (n as f64 / 1024f64.powi(2)).round().max(1.) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::fmt_bytes;

    #[test]
    fn bytes_read_naturally() {
        assert_eq!(fmt_bytes(0), "—");
        assert_eq!(fmt_bytes(500 * 1024 * 1024), "500 MB");
        assert_eq!(fmt_bytes(1536 * 1024 * 1024), "1.5 GB");
        assert_eq!(fmt_bytes(12 * 1024 * 1024 * 1024), "12 GB");
    }
}
