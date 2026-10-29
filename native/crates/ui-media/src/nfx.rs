//! Netflix look for the Movies home, measured against netflix.com/browse: the pill tab bar that
//! turns solid on scroll, the inset rounded billboard, rows that slide a page at a time with the
//! neighbouring cards peeking in the gutters, logo-on-art landscape cards with "TOP 10" and
//! status badges, and the hover card that grows out of a card with quick actions.
//! The Anime page keeps `look`'s Crunchyroll style; colours come from `Tokens`.
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::motion::reduced;
use mv_ui::theme::{duration, easing, icon as isz, radius};
use mv_ui::{focus_ring_shadow, icon, ph, tip, Interactive, Ph, Tokens};

use crate::look::{self, BoundsSlot};

/// Left/right gutter of the bar, billboard and rows; the row chevrons and peeking cards live in it.
pub const EDGE: f32 = 48.0;
/// Space between cards (Netflix: 8px at every width).
pub const GAP: f32 = 8.0;
/// Netflix's boxart width; a row fits as many as come closest and stretches them to fill.
const CARD: f32 = 248.0;
/// Billboard corner radius (Netflix: 24px).
const HERO_RADIUS: f32 = 24.0;
/// The hover card is this much wider than the card it grows out of.
const POP_SCALE: f32 = 1.5;
/// How long the pointer rests on a card before its hover card opens.
pub const POP_DELAY: std::time::Duration = std::time::Duration::from_millis(420);
/// How long a row takes to slide one page.
pub const SLIDE: std::time::Duration = std::time::Duration::from_millis(560);

/// The page background at `alpha`, for the fades over artwork.
fn ink(alpha: f32) -> Hsla {
    let mut c: Hsla = Tokens::base().into();
    c.a = alpha;
    c
}

/// Text colour at `alpha`, for the pill fills and inactive tabs.
fn light(alpha: f32) -> Rgba {
    Tokens::text_1().alpha(alpha)
}

/// Cards per page and their width for a page `width` wide.
pub fn card_fit(width: f32) -> (usize, f32) {
    let inner = (width - EDGE * 2.0).max(CARD);
    let per = (((inner + GAP) / (CARD + GAP)).round() as usize).max(2);
    (per, (inner - GAP * (per as f32 - 1.0)) / per as f32)
}

/// Tab in the top bar: muted text, white and on a soft pill when current (Netflix's Home pill).
pub fn nav_tab(id: impl Into<ElementId>, label: &'static str, active: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(34.))
        .px(px(15.))
        .rounded_full()
        .text_size(px(15.))
        .cursor_pointer()
        .role(Role::Tab)
        .aria_selected(active)
        .map(|d| if active { d.bg(light(0.2)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()) } else { d.text_color(light(0.7)).hover(|s| s.text_color(light(0.9))) })
        .focus_ring()
        .child(label)
}

/// The top bar's backdrop: a fade from the page colour while the billboard is under it, a solid
/// bar once the page scrolled (faded in, as Netflix does).
pub fn bar_backdrop(scrolled: bool) -> AnyElement {
    let fade = div().absolute().inset_0().bg(linear_gradient(180., linear_color_stop(ink(0.9), 0.), linear_color_stop(ink(0.), 1.)));
    let solid = div().absolute().inset_0().bg(Tokens::base());
    let solid = if reduced() {
        solid.opacity(if scrolled { 1. } else { 0. }).into_any_element()
    } else {
        let (a, b) = if scrolled { (0., 1.) } else { (1., 0.) };
        solid.with_animation(("nf-bar", scrolled as usize), Animation::new(duration::SLOW).with_easing(easing::standard), move |d, t| d.opacity(a + (b - a) * t)).into_any_element()
    };
    div().absolute().inset_0().child(fade).child(solid).into_any_element()
}

/// Full-bleed art for the billboard and hover cards: transparent while loading (the art under it
/// stays), nothing on failure.
fn art(url: impl Into<SharedString>, r: f32) -> Img {
    img(crate::common::image_source(&url.into()))
        .size_full()
        .object_fit(ObjectFit::Cover)
        .rounded(px(r))
        .with_loading(|| div().size_full().into_any_element())
        .with_fallback(|| div().size_full().into_any_element())
}

/// The inset billboard: art in a rounded card `EDGE` in from each side, darkened on the left for
/// the copy. `under` is the slide being replaced, so the carousel crossfades.
pub fn billboard(id: impl Into<ElementId>, height: f32, image: Option<String>, under: Option<String>) -> Stateful<Div> {
    let id: ElementId = id.into();
    let art_id = ElementId::NamedChild(std::sync::Arc::new(id.clone()), "art".into());
    let under = under.filter(|u| image.as_ref() != Some(u));
    div()
        .id(id)
        .relative()
        .mx(px(EDGE))
        .h(px(height))
        .rounded(px(HERO_RADIUS))
        .overflow_hidden()
        .bg(Tokens::surface())
        .when_some(under, |d, u| d.child(div().absolute().inset_0().child(art(u, 0.))))
        .when_some(image, |d, u| d.child(look::fade_in(art_id, div().absolute().inset_0().child(art(u, 0.)))))
        .child(div().absolute().inset_0().bg(linear_gradient(90., linear_color_stop(ink(0.82), 0.), linear_color_stop(ink(0.), 0.58))))
        .child(div().absolute().left_0().right_0().bottom_0().h(px(height * 0.4)).bg(linear_gradient(180., linear_color_stop(ink(0.), 0.), linear_color_stop(ink(0.5), 1.))))
        .child(corners(HERO_RADIUS))
}

/// Rounds a clipped box's corners: cover-fit art paints past its box and GPUI clips to
/// rectangles, so a page-coloured frame whose inner edge has the radius masks the corners.
fn corners(r: f32) -> Div {
    div().absolute().top(px(-r)).left(px(-r)).right(px(-r)).bottom(px(-r)).border(px(r)).border_color(Tokens::base()).rounded(px(r * 2.))
}

/// Billboard copy block, bottom-left inside the card.
pub fn hero_copy(width: f32) -> Div {
    div().absolute().left(px(40.)).bottom(px(44.)).w(px(width.clamp(320., 640.))).flex().flex_col().gap(px(16.))
}

/// Title art for the billboard (the logo, or the title in bold when there is none).
pub fn hero_logo(title: &str, logo: Option<String>, max_h: f32) -> AnyElement {
    let text: SharedString = title.to_string().into();
    let heading = move |t: SharedString| div().text_size(px(48.)).line_height(px(54.)).font_weight(FontWeight::BLACK).text_color(Tokens::text_1()).line_clamp(2).child(t).into_any_element();
    match logo {
        Some(u) => {
            let (a, b) = (text.clone(), text);
            div()
                .h(px(max_h))
                .flex()
                .items_end()
                .child(img(crate::common::image_source(&u)).max_h(px(max_h)).max_w(px(460.)).object_fit(ObjectFit::Contain).with_fallback(move || heading(a.clone())).with_loading(move || heading(b.clone())))
                .into_any_element()
        }
        None => heading(text),
    }
}

/// "Movie • Action • 2017 • 1h 40m": bold metadata joined by small dots.
pub fn dot_line(parts: Vec<String>, text_px: f32) -> Div {
    let mut d = div().flex().flex_wrap().items_center().gap(px(8.)).text_size(px(text_px)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1());
    for (i, p) in parts.into_iter().filter(|p| !p.is_empty()).enumerate() {
        if i > 0 {
            d = d.child(div().size(px(4.)).rounded_full().bg(light(0.55)));
        }
        d = d.child(div().child(SharedString::from(p)));
    }
    d
}

/// Netflix's white pill Play button.
pub fn play_pill(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let label: SharedString = label.into();
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(44.))
        .pl(px(18.))
        .pr(px(24.))
        .rounded_full()
        .bg(Tokens::text_1())
        .text_color(Tokens::base())
        .text_size(px(17.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .role(Role::Button)
        .aria_label(label.clone())
        .pressable()
        .hover(|s| s.bg(light(0.78)))
        .focus_ring()
        .child(icon(ph::PLAY.fill(), Tokens::base(), isz::LG))
        .child(label)
}

/// The translucent grey pill beside Play ("More Info").
pub fn info_pill(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let label: SharedString = label.into();
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(44.))
        .px(px(20.))
        .rounded_full()
        .bg(Tokens::text_3().alpha(0.45))
        .text_color(Tokens::text_1())
        .text_size(px(17.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .role(Role::Button)
        .aria_label(label.clone())
        .pressable()
        .hover(|s| s.bg(Tokens::text_3().alpha(0.3)))
        .focus_ring()
        .child(label)
}

/// Round action on a hover card: `primary` is the filled white play disc, others are outlined.
pub fn disc(id: impl Into<ElementId>, glyph: Ph, label: impl Into<SharedString>, primary: bool) -> Stateful<Div> {
    let label: SharedString = label.into();
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(36.))
        .rounded_full()
        .cursor_pointer()
        .role(Role::Button)
        .aria_label(label.clone())
        .tooltip(tip(label))
        .pressable()
        .map(|d| {
            if primary {
                d.bg(Tokens::text_1()).hover(|s| s.bg(light(0.8))).child(icon(glyph.fill(), Tokens::base(), isz::MD))
            } else {
                d.bg(ink(0.6)).border_2().border_color(light(0.5)).hover(|s| s.border_color(Tokens::text_1())).child(icon(glyph.bold(), Tokens::text_1(), isz::SM))
            }
        })
}

/// Small outlined box tag ("HD", "13+").
pub fn box_tag(text: impl Into<SharedString>) -> Div {
    div().px(px(5.)).h(px(16.)).flex().items_center().border_1().border_color(light(0.45)).rounded(px(2.)).text_size(px(10.)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_2()).child(text.into())
}

/// Where each card of the visible pages was painted, and the hover card that is open. The page
/// tracks the pointer against these bounds itself: GPUI hover flags go stale under a card that
/// grows over its neighbours.
pub type CardKey = (&'static str, usize);

#[derive(Default)]
pub struct Hovering {
    cards: Rc<RefCell<HashMap<CardKey, Bounds<Pixels>>>>,
    pub popup: BoundsSlot,
    /// The card whose hover card is open.
    pub shown: Option<CardKey>,
    /// The card under the pointer, waiting out `POP_DELAY`.
    pub pending: Option<CardKey>,
    pub generation: u64,
}

impl Hovering {
    /// Invisible layer recording the card's art bounds (place it in the `relative()` art box).
    pub fn probe(&self, key: CardKey) -> impl IntoElement {
        let map = self.cards.clone();
        canvas(
            move |b, _, _| {
                map.borrow_mut().insert(key, b);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    pub fn bounds_of(&self, key: CardKey) -> Option<Bounds<Pixels>> {
        self.cards.borrow().get(&key).copied()
    }

    /// Forget last frame's cards; the probes painted this frame refill the map.
    pub fn begin_frame(&self) {
        self.cards.borrow_mut().clear();
    }

    /// The card under `p`, ignoring the ones peeking into the gutters of `page`.
    pub fn card_at(&self, p: Point<Pixels>, page: Bounds<Pixels>) -> Option<CardKey> {
        let (lo, hi) = (page.origin.x + px(EDGE - 2.), page.origin.x + page.size.width - px(EDGE - 2.));
        self.cards.borrow().iter().find(|(_, b)| b.contains(&p) && b.origin.x >= lo && b.origin.x + b.size.width <= hi).map(|(k, _)| *k)
    }

    /// Closes the hover card and drops the pending one. True when something was open.
    pub fn close(&mut self) -> bool {
        let was = self.shown.is_some();
        self.shown = None;
        self.pending = None;
        self.generation += 1;
        self.popup.set(None);
        was
    }
}

/// A landscape boxart: the art with the title logo on it, an optional "TOP 10" mark, a status
/// badge on the bottom edge ("Recently added") and, for Continue Watching, a progress bar under it.
#[allow(clippy::too_many_arguments)]
pub fn card(hov: &Hovering, key: CardKey, w: f32, image: Option<String>, logo: Option<String>, title: &str, top10: bool, badge: Option<&'static str>, progress: Option<f32>) -> Stateful<Div> {
    let h = w * 9.0 / 16.0;
    let title: SharedString = title.to_string().into();
    let fallback_title = {
        let t = title.clone();
        move || div().text_size(px(18.)).line_height(px(21.)).font_weight(FontWeight::BLACK).text_color(Tokens::text_1()).line_clamp(2).child(t.clone()).into_any_element()
    };
    let name: AnyElement = match logo {
        Some(u) => img(crate::common::image_source(&SharedString::from(u)))
            .max_h(px(h * 0.34))
            .max_w(px(w * 0.62))
            .object_fit(ObjectFit::Contain)
            .with_loading(|| div().into_any_element())
            .with_fallback(fallback_title)
            .into_any_element(),
        None => fallback_title(),
    };
    div()
        .id(key)
        .flex_none()
        .w(px(w))
        .flex()
        .flex_col()
        .gap(px(7.))
        .rounded(px(radius::XS))
        .cursor_pointer()
        .role(Role::Link)
        .aria_label(title)
        .focus_ring()
        .child(
            div()
                .relative()
                .w(px(w))
                .h(px(h))
                .rounded(px(radius::XS))
                .bg(Tokens::surface_2())
                .when_some(image, |d, u| d.child(look::pic(u).rounded(px(radius::XS))))
                .child(div().absolute().left_0().right_0().bottom_0().h(px(h * 0.5)).rounded_b(px(radius::XS)).bg(linear_gradient(180., linear_color_stop(ink(0.), 0.), linear_color_stop(ink(0.55), 1.))))
                .child(div().absolute().left(px(w * 0.07)).right(px(w * 0.07)).bottom(px(h * 0.12)).flex().items_end().child(name))
                .when(top10, |d| d.child(div().absolute().top_0().left_0().child(top10_mark())))
                .when_some(badge, |d, b| {
                    d.child(
                        div().absolute().left_0().right_0().bottom_0().flex().justify_center().child(
                            div().px(px(7.)).h(px(17.)).flex().items_center().rounded_t(px(3.)).bg(Tokens::accent()).text_color(Tokens::on_accent()).text_size(px(11.)).font_weight(FontWeight::BOLD).child(b),
                        ),
                    )
                })
                .child(hov.probe(key)),
        )
        .when_some(progress, |d, p| {
            d.child(div().mx(px(w * 0.2)).h(px(3.)).bg(light(0.3)).child(div().h_full().w(relative(p.clamp(0.02, 1.0))).bg(Tokens::accent())))
        })
}

/// The small "TOP 10" mark Netflix puts in a card's corner.
fn top10_mark() -> Div {
    div()
        .w(px(20.))
        .h(px(26.))
        .rounded_bl(px(2.))
        .rounded_tl(px(radius::XS))
        .bg(Tokens::accent())
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .text_color(Tokens::on_accent())
        .child(div().text_size(px(6.5)).line_height(px(7.)).font_weight(FontWeight::BOLD).child("TOP"))
        .child(div().text_size(px(12.)).line_height(px(12.)).font_weight(FontWeight::BLACK).child("10"))
}

/// The hover card: the card's art grown by half over its neighbours, then the actions and the
/// metadata on a raised panel. `anchor` is the card art's bounds, `page` keeps it on the page.
pub fn hover_card(slot: &BoundsSlot, gen: u64, anchor: Bounds<Pixels>, page: Bounds<Pixels>, image: Option<String>, logo: Option<String>, title: &str, body: impl IntoElement) -> AnyElement {
    let w = f32::from(anchor.size.width);
    let w2 = (w * POP_SCALE).max(300.);
    let ih = w2 * 9. / 16.;
    let (left, right, top) = (f32::from(page.origin.x) + 8., f32::from(page.origin.x + page.size.width) - 8., f32::from(page.origin.y) + 8.);
    let cx = f32::from(anchor.origin.x) + w / 2.;
    let cy = f32::from(anchor.origin.y) + f32::from(anchor.size.height) / 2.;
    let x = (cx - w2 / 2.).min(right - w2).max(left);
    let y = (cy - ih / 2.).max(top);
    // The card grows out of the boxart: it starts at the art's size and place.
    let (dx, dy) = (f32::from(anchor.origin.x) - x, f32::from(anchor.origin.y) - y);
    let title: SharedString = title.to_string().into();
    let t = title.clone();
    let name: AnyElement = match logo {
        Some(u) => img(crate::common::image_source(&SharedString::from(u)))
            .max_h(px(ih * 0.3))
            .max_w(px(w2 * 0.55))
            .object_fit(ObjectFit::Contain)
            .with_loading(|| div().into_any_element())
            .with_fallback(move || div().text_size(px(20.)).font_weight(FontWeight::BLACK).text_color(Tokens::text_1()).line_clamp(2).child(t.clone()).into_any_element())
            .into_any_element(),
        None => div().text_size(px(20.)).font_weight(FontWeight::BLACK).text_color(Tokens::text_1()).line_clamp(2).child(title.clone()).into_any_element(),
    };
    let shadow = vec![BoxShadow { color: ink(0.75), offset: point(px(0.), px(10.)), blur_radius: px(28.), spread_radius: px(0.), inset: false }];
    let popup = div()
        .id(("nf-pop", gen as usize))
        .relative()
        .w(px(w2))
        .rounded(px(radius::SM))
        .bg(Tokens::surface())
        .shadow(shadow)
        .block_mouse_except_scroll()
        .child(look::bounds_probe(slot))
        .child(
            div()
                .relative()
                .w_full()
                .aspect_ratio(16. / 9.)
                .rounded_t(px(radius::SM))
                .bg(Tokens::surface_2())
                .when_some(image, |d, u| d.child(div().absolute().inset_0().child(look::pic(u).rounded_t(px(radius::SM)))))
                .child(div().absolute().left_0().right_0().bottom_0().h(relative(0.45)).bg(linear_gradient(180., linear_color_stop(ink(0.), 0.), linear_color_stop(ink(0.5), 1.))))
                .child(div().absolute().left(px(14.)).right(px(14.)).bottom(px(12.)).flex().items_end().child(name)),
        )
        .child(div().px(px(16.)).pt(px(14.)).pb(px(18.)).flex().flex_col().gap(px(12.)).child(body));
    let popup = if reduced() {
        popup.into_any_element()
    } else {
        popup
            .with_animation(("nf-pop-in", gen as usize), Animation::new(duration::BASE).with_easing(easing::enter), move |d, t| {
                d.w(px(w + (w2 - w) * t)).ml(px(dx * (1. - t))).mt(px(dy * (1. - t))).opacity(0.4 + 0.6 * t)
            })
            .into_any_element()
    };
    deferred(anchored().position(point(px(x), px(y))).child(popup)).with_priority(1).into_any_element()
}

/// One Netflix row: the title (and an "Explore All" link that shows on hover), then the cards
/// edge to edge, sliding a page at a time between `from` and `start` with the neighbouring cards
/// peeking into the gutters. `make(i)` builds item `i`; only the items in view are built.
#[allow(clippy::too_many_arguments)]
pub fn row<V: 'static>(
    cx: &mut Context<V>,
    id: &'static str,
    title: impl Into<SharedString>,
    total: usize,
    item_w: f32,
    per: usize,
    (from, start): (usize, usize),
    mut make: impl FnMut(usize, &mut Context<V>) -> AnyElement,
    set: fn(&mut V, &'static str, usize),
    explore: Option<look::OnClick>,
) -> Div {
    let per = per.max(1);
    let last_page = total.saturating_sub(1) / per * per;
    let start = start.min(last_page);
    let from = from.min(last_page);
    let (has_prev, has_next) = (start > 0, start + per < total);
    let pages = total.div_ceil(per);
    let page = start / per;
    let group: SharedString = format!("nf-row-{id}").into();
    let lo = from.min(start).saturating_sub(1);
    let hi = (from.max(start) + per + 1).min(total);
    let step = item_w + GAP;
    let at = move |s: usize| EDGE - (s - lo) as f32 * step;
    let items: Vec<AnyElement> = (lo..hi).map(|i| make(i, cx)).collect();
    let strip = div().flex().gap(px(GAP)).children(items);
    let (a, b) = (at(from), at(start));
    let strip = if from != start && !reduced() {
        strip.with_animation(SharedString::from(format!("nf-slide-{id}-{from}-{start}")), Animation::new(SLIDE).with_easing(easing::standard), move |d, t| d.ml(px(a + (b - a) * t))).into_any_element()
    } else {
        strip.ml(px(b)).into_any_element()
    };
    let arrow = |left: bool, target: usize, cx: &mut Context<V>| {
        let label = if left { "Previous" } else { "Next" };
        div()
            .id((id, if left { 0usize } else { 1 }))
            .absolute()
            .top(px(4.))
            .bottom(px(4.))
            .when(left, |d| d.left_0().rounded_r(px(radius::XS)))
            .when(!left, |d| d.right_0().rounded_l(px(radius::XS)))
            .w(px(EDGE - 4.))
            .flex()
            .items_center()
            .justify_center()
            .bg(ink(0.5))
            .opacity(0.)
            .group_hover(group.clone(), |s| s.opacity(1.))
            .role(Role::Button)
            .aria_label(label)
            .pressable()
            .hover(|s| s.bg(ink(0.75)))
            .focus_visible(|s| s.opacity(1.).shadow(focus_ring_shadow()))
            .tooltip(tip(label))
            .child(icon(if left { ph::CARET_LEFT.bold() } else { ph::CARET_RIGHT.bold() }, Tokens::text_1(), isz::HERO))
            // The card peeking out under the chevron must not open as well.
            .block_mouse_except_scroll()
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                set(this, id, target);
                cx.notify();
            }))
    };
    let prev = arrow(true, start.saturating_sub(per), cx);
    let next = arrow(false, start + per, cx);
    let dashes = (pages > 1).then(|| {
        div().flex().items_center().gap(px(2.)).opacity(0.).group_hover(group.clone(), |s| s.opacity(1.)).children((0..pages).map(|i| {
            let on = i == page;
            div()
                .id((id, 10 + i))
                .h(px(14.))
                .flex()
                .items_center()
                .cursor_pointer()
                .child(div().w(px(12.)).h(px(2.)).bg(if on { Tokens::text_2() } else { light(0.25) }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    set(this, id, i * per);
                    cx.notify();
                }))
        }))
    });
    let title_group: SharedString = format!("nf-row-title-{id}").into();
    div()
        .group(group)
        .w_full()
        .mb(px(34.))
        .child(
            div()
                .flex()
                .items_end()
                .gap(px(12.))
                .px(px(EDGE))
                .mb(px(10.))
                .child(
                    div()
                        .id(SharedString::from(format!("nf-row-head-{id}")))
                        .group(title_group.clone())
                        .flex()
                        .items_end()
                        .gap(px(10.))
                        .child(div().text_size(px(21.)).line_height(px(26.)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title.into()))
                        .when_some(explore, |d, f| {
                            d.cursor_pointer().on_click(move |e, w, cx| f(e, w, cx)).child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(2.))
                                    .pb(px(3.))
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(Tokens::accent_2())
                                    .opacity(0.)
                                    .group_hover(title_group.clone(), |s| s.opacity(1.))
                                    .child("Explore All")
                                    .child(icon(ph::CARET_RIGHT.bold(), Tokens::accent_2(), isz::XS)),
                            )
                        }),
                )
                .child(div().flex_1())
                .children(dashes),
        )
        .child(
            div()
                .relative()
                .w_full()
                // The 4px padding keeps the cards' focus ring inside the clip.
                .child(div().id(SharedString::from(format!("nf-row-{id}-cards"))).w_full().overflow_hidden().py(px(4.)).child(strip))
                .when(has_prev, |d| d.child(prev))
                .when(has_next, |d| d.child(next)),
        )
}

/// Skeleton row while a catalog loads.
pub fn skeleton_row(title: &str, w: f32, h: f32, n: usize) -> Div {
    div()
        .mb(px(34.))
        .child(div().px(px(EDGE)).mb(px(10.)).text_size(px(21.)).line_height(px(26.)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_3()).child(SharedString::from(title.to_string())))
        .child(div().flex().gap(px(GAP)).px(px(EDGE)).py(px(4.)).overflow_hidden().children((0..n).map(|_| mv_ui::skeleton(px(w), px(h), radius::XS))))
}

#[cfg(test)]
mod tests {
    use super::card_fit;

    #[test]
    fn cards_fill_the_row() {
        for width in [900., 1280., 1600., 1920.] {
            let (per, w) = card_fit(width);
            assert!(per >= 2);
            let used = per as f32 * w + (per as f32 - 1.) * super::GAP;
            assert!((used - (width - super::EDGE * 2.)).abs() < 0.5, "{width}: {per} x {w}");
            assert!((200. ..=320.).contains(&w), "{width}: card {w}");
        }
    }
}
