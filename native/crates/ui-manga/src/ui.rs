//! Building blocks shared by the manga pages: the Mihon-style cover grid and its card, count
//! badges, underline tabs, the detail page's action tiles, row thumbnails and a dropdown whose
//! menu can scroll (chapter lists, the source picker). Everything takes colours from `Tokens`,
//! icons from Phosphor and the kit's state layers and focus ring.
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::Selectable;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::{easing, icon as icon_px, radius};
use mv_ui::{glyphs, icon_ph, ph, ph_icon, shimmer, size, skeleton, spinner, tip, CoverSource, Interactive, Ph, Tokens, Weight};

use crate::backend;

/// Smallest cover width in the grids; cards stretch to fill the row.
pub const MIN_CARD: f32 = 140.0;
/// Gap between cards (both directions).
pub const GAP: f32 = 16.0;
/// Covers are 2:3.
pub const COVER_RATIO: f32 = 1.5;
/// Width of the expanded app sidebar, used until the real width has been measured.
const SIDEBAR_GUESS: f32 = 244.0;
/// Hover group of a cover card, for overlay controls that appear with it.
pub const CARD_GROUP: &str = "manga-card";

/// Loading / ready / failed state of an async page section.
pub enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

pub fn cover_src(map: &HashMap<String, PathBuf>, url: Option<&str>) -> Option<CoverSource> {
    url.and_then(|u| map.get(u)).cloned().map(CoverSource::File)
}

/// Search field with a leading magnifier and a clear button.
pub fn search_box(state: &Entity<InputState>) -> Div {
    div().w(px(240.)).child(Input::new(state).cleanable(true).prefix(icon_ph(glyphs::SEARCH, Tokens::text_3(), icon_px::SM)))
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
    /// Browse: the title is already in the library (dimmed cover, filled bookmark badge).
    pub in_library: bool,
    /// Filled gold star in the top-right corner.
    pub favorite: bool,
    /// Chapters read, 0 to 1, drawn as a thin accent line along the bottom edge.
    pub progress: Option<f32>,
    /// Being opened: a scrim with a spinner over the cover.
    pub busy: bool,
    /// Overlay control in the bottom-right corner (Continue reading); the title leaves room for it.
    pub action: Option<AnyElement>,
}

fn cover_placeholder(icon: Ph) -> AnyElement {
    div().size_full().flex().items_center().justify_center().bg(Tokens::surface_2()).child(ph_icon(icon, Weight::Duotone, Tokens::text_3(), icon_px::HERO)).into_any_element()
}

/// Cover image with a loading shimmer and a broken-image fallback.
fn cover_img(c: CoverSource) -> Img {
    c.into_img()
        .size_full()
        .object_fit(ObjectFit::Cover)
        .with_loading(|| div().relative().size_full().overflow_hidden().bg(Tokens::surface_2()).child(shimmer()).into_any_element())
        .with_fallback(|| cover_placeholder(ph::IMAGE_BROKEN))
}

/// Mihon "compact grid" card: 2:3 cover, title over a bottom scrim, status badges top-left, a
/// favourite star top-right and reading progress along the bottom. It lifts on hover, settles
/// when pressed and is a Tab stop with a focus ring; long titles show in full in a tooltip.
/// The returned box is the fixed hover and click target; only the face inside it lifts, so the
/// target never slides out from under the pointer (a hover that moves its own hit area flickers
/// along the edge it leaves behind).
/// Material 3 cards: <https://m3.material.io/components/cards/guidelines>; Mihon library grid.
pub fn manga_card(id: impl Into<ElementId>, info: CardInfo, grid: Grid) -> Stateful<Div> {
    let (w, h) = (grid.card_w, grid.card_h());
    let long_title = info.title.chars().count() > (w / 6.5 * 2.) as usize;
    let (title, aria) = (info.title.clone(), info.title.clone());
    let reserve = info.action.is_some();
    let face = div()
        .id("card-face")
        .absolute()
        .top_0()
        .left_0()
        .w(px(w))
        .h(px(h))
        .rounded(px(radius::MD))
        .overflow_hidden()
        .bg(Tokens::surface_2())
        .border_1()
        .border_color(Tokens::border())
        .group_hover(CARD_GROUP, |s| s.top(px(-3.)).border_color(Tokens::border_strong()).shadow_lg())
        .group_active(CARD_GROUP, |s| s.top(px(-1.)).shadow_md())
        .map(|d| match info.cover {
            Some(c) => d.child(cover_img(c).when(info.in_library, |i| i.opacity(0.5))),
            None => d.child(cover_placeholder(glyphs::MANGA)),
        })
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px((h * 0.45).max(64.)))
                .bg(linear_gradient(180., linear_color_stop(Tokens::scrim().alpha(0.), 0.), linear_color_stop(Tokens::scrim().alpha(0.9), 1.))),
        )
        .child(
            div()
                .id("card-title")
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .pl(px(8.))
                .pr(px(if reserve { 42. } else { 8. }))
                .pb(px(if info.progress.is_some() { 10. } else { 7. }))
                .text_size(px(size::SM))
                .line_height(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(Tokens::text_1())
                .line_clamp(2)
                .when(long_title, |d| d.tooltip(tip(title)))
                .child(info.title),
        )
        .child(
            div()
                .absolute()
                .top(px(6.))
                .left(px(6.))
                .flex()
                .rounded(px(radius::XS))
                .overflow_hidden()
                .when(info.in_library, |d| d.child(badge(Some(glyphs::SAVED), "In library", BadgeKind::Accent)))
                .when(info.unread > 0, |d| d.child(badge(None, info.unread.to_string(), BadgeKind::Accent)))
                .when(info.downloaded > 0, |d| d.child(badge(Some(glyphs::DOWNLOAD), info.downloaded.to_string(), BadgeKind::Neutral))),
        )
        .when(info.favorite, |d| {
            d.child(
                div()
                    .absolute()
                    .top(px(6.))
                    .right(px(6.))
                    .size(px(22.))
                    .rounded_full()
                    .bg(Tokens::glass())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(ph_icon(glyphs::FAVORITES, Weight::Fill, Tokens::favorite(), icon_px::XS)),
            )
        })
        .when_some(info.progress, |d, p| {
            d.child(div().absolute().left_0().right_0().bottom_0().h(px(3.)).bg(Tokens::scrim()).child(div().h_full().w(relative(p.clamp(0., 1.))).bg(Tokens::accent())))
        })
        .when(info.busy, |d| {
            d.child(div().absolute().top_0().left_0().size_full().flex().items_center().justify_center().bg(Tokens::scrim()).child(spinner("card-busy", Tokens::text_1(), icon_px::XL)))
        })
        .children(info.action);
    div()
        .id(id)
        .relative()
        .flex_none()
        .w(px(w))
        .h(px(h))
        .rounded(px(radius::MD))
        .group(CARD_GROUP)
        .role(Role::Link)
        .aria_label(aria)
        .cursor_pointer()
        .focus_ring()
        .child(face)
}

/// Shimmering placeholder with the card's exact size (loading state).
pub fn skeleton_cover(grid: Grid) -> Div {
    skeleton(px(grid.card_w), px(grid.card_h()), radius::MD)
}

/// A few rows of skeleton cards.
pub fn skeleton_rows(grid: Grid, rows: usize) -> Div {
    div().flex().flex_col().children((0..rows).map(move |_| grid.row().children((0..grid.cols).map(move |_| skeleton_cover(grid)))))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BadgeKind {
    Accent,
    Neutral,
}

/// Small count / status tag on a cover. Adjacent badges join into one pill (Mihon style).
fn badge(icon: Option<Ph>, text: impl Into<SharedString>, kind: BadgeKind) -> Div {
    let (bg, fg) = match kind {
        BadgeKind::Accent => (Tokens::accent(), Tokens::on_accent()),
        BadgeKind::Neutral => (Tokens::surface_3(), Tokens::text_1()),
    };
    div()
        .flex()
        .items_center()
        .gap(px(3.))
        .px(px(5.))
        .h(px(18.))
        .bg(bg)
        .text_size(px(size::XS))
        .font_weight(FontWeight::BOLD)
        .text_color(fg)
        .when_some(icon, |d, i| d.child(ph_icon(i, if kind == BadgeKind::Accent { Weight::Fill } else { Weight::Bold }, fg, 11.)))
        .child(text.into())
}

/// Selected-filter pill with a remove mark; the whole pill removes it.
pub fn removable_chip(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let label: SharedString = label.into();
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(28.))
        .pl(px(12.))
        .pr(px(8.))
        .rounded(px(radius::PILL))
        .bg(Tokens::accent_soft())
        .border_1()
        .border_color(Tokens::accent_border())
        .text_size(px(size::SM))
        .font_weight(FontWeight::MEDIUM)
        .text_color(Tokens::accent())
        .role(Role::Button)
        .aria_label(SharedString::from(format!("Remove {label}")))
        .hover(|s| s.bg(Tokens::accent_a(0.22)))
        .pressable()
        .child(label)
        .child(icon_ph(ph::X, Tokens::accent(), icon_px::XS))
}

// ---------------------------------------------------------------- controls

/// Underlined tab (category / mode tabs) with an optional icon (filled when selected) and a
/// muted count. A Tab stop with the kit's focus ring.
pub fn tab(id: impl Into<ElementId>, icon: Option<Ph>, label: impl Into<SharedString>, count: Option<usize>, active: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(40.))
        .px(px(4.))
        .mr(px(18.))
        .border_b_2()
        .text_size(px(size::MD))
        .whitespace_nowrap()
        .group("mv-tab")
        .role(Role::Tab)
        .aria_selected(active)
        .map(|d| {
            if active {
                d.border_color(Tokens::accent()).text_color(Tokens::text_1()).font_weight(FontWeight::MEDIUM)
            } else {
                d.border_color(Tokens::base().alpha(0.)).text_color(Tokens::text_3()).hover(|s| s.text_color(Tokens::text_1()))
            }
        })
        .cursor_pointer()
        .focus_ring()
        .when_some(icon, |d, i| {
            d.child(
                ph_icon(i, if active { Weight::Fill } else { Weight::Bold }, if active { Tokens::accent() } else { Tokens::text_3() }, icon_px::SM)
                    .when(!active, |s| s.group_hover("mv-tab", |s| s.text_color(Tokens::text_1()))),
            )
        })
        .child(label.into())
        .when_some(count, |d, n| {
            d.child(
                div()
                    .px(px(6.))
                    .rounded(px(radius::PILL))
                    .bg(if active { Tokens::accent_soft() } else { Tokens::surface_2() })
                    .text_size(px(size::XS))
                    .text_color(if active { Tokens::accent() } else { Tokens::text_3() })
                    .child(n.to_string()),
            )
        })
}

/// Large action with its icon above the label (Mihon's manga action row). A toggle's on state
/// shows the fill-weight icon in `color`, a tinted container and a semibold label, so it never
/// rests on colour alone. `pop` (a counter bumped on each toggle) replays a short scale pop;
/// `busy` turns the icon into a spinner. Add `.tooltip(..)` and `.on_click(..)`.
/// Material 3 icon buttons (toggle = selected icon + container): <https://m3.material.io/components/icon-buttons/overview>;
/// WCAG 1.4.11 non-text contrast: <https://www.w3.org/WAI/WCAG22/Understanding/non-text-contrast.html>
pub fn action_tile(id: &'static str, icon: Ph, label: impl Into<SharedString>, on: bool, color: Rgba, busy: bool, pop: u64) -> Stateful<Div> {
    let fg = if on { color } else { Tokens::text_2() };
    let svg = ph_icon(icon, if on { Weight::Fill } else { Weight::Regular }, fg, icon_px::LG);
    let glyph: AnyElement = if busy {
        spinner(SharedString::from(format!("{id}-spin")), fg, icon_px::LG).into_any_element()
    } else if pop > 0 && !mv_ui::motion::reduced() {
        svg.with_animation(ElementId::NamedInteger(SharedString::from(format!("{id}-pop")), pop), Animation::new(Duration::from_millis(260)).with_easing(easing::standard), move |s, t| {
            let bump = (t * std::f32::consts::PI).sin();
            let k = if on { 1. + 0.3 * bump } else { 1. - 0.12 * bump };
            s.with_transformation(Transformation::scale(gpui_kit::size(k, k)))
        })
        .into_any_element()
    } else {
        svg.into_any_element()
    };
    let hover_bg = if on { color.alpha(0.22) } else { Tokens::hover() };
    div()
        .id(id)
        .flex()
        .flex_none()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(4.))
        .min_w(px(88.))
        .h(px(60.))
        .px(px(10.))
        .rounded(px(radius::LG))
        .text_size(px(size::XS))
        .font_weight(if on { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
        .text_color(fg)
        .role(Role::Button)
        .when(on, |d| d.bg(color.alpha(0.14)))
        .hover(move |s| s.bg(hover_bg))
        .pressable()
        .child(glyph)
        .child(label.into())
}

/// Book cover thumbnail for list rows (History, Downloads), with loading and fallback states.
pub fn thumb(cover: Option<CoverSource>, w: f32, h: f32) -> Div {
    div()
        .w(px(w))
        .h(px(h))
        .flex_none()
        .rounded(px(radius::SM))
        .overflow_hidden()
        .bg(Tokens::surface_2())
        .border_1()
        .border_color(Tokens::border())
        .map(|d| match cover {
            Some(c) => d.child(cover_img(c)),
            None => d.child(div().size_full().flex().items_center().justify_center().child(ph_icon(glyphs::MANGA, Weight::Duotone, Tokens::text_3(), icon_px::LG))),
        })
}

// ---------------------------------------------------------------- menus

/// One entry of a local dropdown or context menu (mirrors `mv_ui::MenuItem`, whose fields the
/// kit keeps private, so the same list can fill a scrolling dropdown and a right-click menu).
/// What a menu entry runs when picked.
pub type EntryHandler = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone)]
pub enum Entry {
    Action {
        label: SharedString,
        icon: Option<Ph>,
        hint: Option<SharedString>,
        checked: bool,
        danger: bool,
        handler: EntryHandler,
    },
    Separator,
    Heading(SharedString),
}

impl Entry {
    /// An action, named by its outcome ("Delete download").
    pub fn new(label: impl Into<SharedString>, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        Entry::Action { label: label.into(), icon: None, hint: None, checked: false, danger: false, handler: Rc::new(handler) }
    }

    pub fn icon(mut self, i: Ph) -> Self {
        if let Entry::Action { icon, .. } = &mut self {
            *icon = Some(i);
        }
        self
    }

    /// Muted second line ("Read", "Page 5", "EN · 18+").
    pub fn hint(mut self, text: impl Into<SharedString>) -> Self {
        if let Entry::Action { hint, .. } = &mut self {
            *hint = Some(text.into());
        }
        self
    }

    /// Marks the current choice with a trailing check.
    pub fn checked(mut self, on: bool) -> Self {
        if let Entry::Action { checked, .. } = &mut self {
            *checked = on;
        }
        self
    }

    /// Destructive action in danger red; put it last, after a separator.
    pub fn danger(mut self) -> Self {
        if let Entry::Action { danger, .. } = &mut self {
            *danger = true;
        }
        self
    }
}

fn entry_row(e: &Entry, lead: bool) -> Div {
    let Entry::Action { label, icon, hint, checked, danger, .. } = e else { return div() };
    let fg = if *danger {
        Tokens::danger()
    } else {
        Tokens::text_1()
    };
    let icon_fg = if *danger { fg } else { Tokens::text_2() };
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .w_full()
        .min_h(px(30.))
        .text_size(px(size::MD))
        .text_color(fg)
        .when(lead, |d| {
            d.child(match icon {
                Some(i) => ph_icon(*i, Weight::Regular, icon_fg, icon_px::MD).into_any_element(),
                None => div().size(px(icon_px::MD)).flex_none().into_any_element(),
            })
        })
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(div().truncate().child(label.clone()))
                .when_some(hint.clone(), |d, h| d.child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(h))),
        )
        .when(*checked, |d| d.child(ph_icon(ph::CHECK, Weight::Bold, Tokens::accent(), icon_px::SM)))
}

/// Adds `entries` to a gpui-component `PopupMenu` (dropdowns and `.context_menu(..)`).
pub fn fill_menu(menu: PopupMenu, entries: &[Entry]) -> PopupMenu {
    let lead = entries.iter().any(|e| matches!(e, Entry::Action { icon: Some(_), .. }));
    entries.iter().fold(menu, |m, e| match e {
        Entry::Separator => m.separator(),
        Entry::Heading(l) => m.label(l.clone()),
        Entry::Action { handler, .. } => {
            let row = e.clone();
            let h = handler.clone();
            m.item(PopupMenuItem::element(move |_, _| entry_row(&row, lead)).on_click(move |_, w, cx| h(w, cx)))
        }
    })
}

/// Trigger for a gpui-component dropdown built from our own element, for menus that need a max
/// height and scrolling or a custom trigger (a progress ring, a source avatar), which
/// `mv_ui::menu_button` does not offer. Shown pressed (and at full opacity) while open; the
/// tooltip is only attached while closed so it never covers the menu.
pub struct MenuTrigger {
    el: Stateful<Div>,
    tooltip: SharedString,
    open: bool,
}

/// Wraps `el` (give it an id, visuals and `.pressable()`) as a dropdown trigger.
pub fn menu_trigger(el: Stateful<Div>, tooltip: impl Into<SharedString>) -> MenuTrigger {
    let tooltip: SharedString = tooltip.into();
    MenuTrigger { el: el.role(Role::Button).aria_label(tooltip.clone()), tooltip, open: false }
}

/// A 32px icon-button-shaped trigger around any small element (icon, ring).
pub fn icon_trigger(id: impl Into<ElementId>, content: impl IntoElement, tooltip: impl Into<SharedString>) -> MenuTrigger {
    menu_trigger(
        div().id(id).flex().flex_none().items_center().justify_center().size(px(32.)).rounded(px(radius::MD)).hover(|s| s.bg(Tokens::hover())).pressable().child(content),
        tooltip,
    )
}

impl Styled for MenuTrigger {
    fn style(&mut self) -> &mut StyleRefinement {
        self.el.style()
    }
}

impl InteractiveElement for MenuTrigger {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.el.interactivity()
    }
}

impl Selectable for MenuTrigger {
    fn selected(mut self, on: bool) -> Self {
        self.open = on;
        self
    }

    fn is_selected(&self) -> bool {
        self.open
    }
}

impl IntoElement for MenuTrigger {
    type Element = Stateful<Div>;
    fn into_element(self) -> Stateful<Div> {
        let MenuTrigger { el, tooltip, open } = self;
        // Open, it stays fully shown even when it is a hover-revealed row control.
        el.aria_expanded(open).map(|d| if open { d.bg(Tokens::active()).opacity(1.) } else { d.tooltip(tip(tooltip)) })
    }
}

impl DropdownMenu for MenuTrigger {}

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
