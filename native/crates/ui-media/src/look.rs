//! Streaming-service look shared by the Movies (Netflix-style) and Anime (Crunchyroll-style)
//! pages: billboard hero, paged rows with edge arrows, landscape / poster / top-10 cards, hero
//! dots and the dismissible overlay used by the source picker.
//!
//! Colours come from `Tokens` (the page accent is the Appearance preset's accent, so every
//! preset applies here too), icons from Phosphor, sizes from `mv_ui::theme` scales.
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::{duration, easing, icon as isz, radius, ramp};
use mv_ui::{
    badge, btn, focus_ring_shadow, glyphs, icon, icon_tag, ph, ph_icon, progress_bar, rating_badge, shimmer, size, skeleton, skeleton_line, tag, tip, Btn,
    ButtonKind, Interactive, Ph, RampExt, Tokens, Tone, Weight,
};

/// Click handler shared by builders here (`cx.listener(..)` fits).
pub type OnClick = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// Left/right inset of rows and hero text; the row arrows live inside it.
pub const EDGE: f32 = 44.0;

/// The page background at `alpha`, for the fades and glass layers over artwork.
fn ink(alpha: f32) -> Hsla {
    let mut c: Hsla = Tokens::base().into();
    c.a = alpha;
    c
}

/// Text colour at `alpha`, for hover outlines and inactive dots over artwork.
fn light(alpha: f32) -> Rgba {
    Tokens::text_1().alpha(alpha)
}

/// Visible width of a page, from its scroll container (falls back to the window minus the sidebar).
pub fn page_width(scroll: &ScrollHandle, window: &Window) -> f32 {
    let w = f32::from(scroll.bounds().size.width);
    if w > 100.0 {
        w
    } else {
        f32::from(window.viewport_size().width) - 244.0
    }
}

/// How many cards of `card` width fit in a row of `width`.
pub fn per_row(width: f32, card: f32, gap: f32) -> usize {
    (((width - EDGE * 2.0 + gap) / (card + gap)).floor() as usize).max(1)
}

/// A card's hover group, keyed by its unique id (never by its title: the same show can sit in
/// two rows, and a title-keyed group lit up both copies).
pub fn group_of(prefix: &str, key: (&'static str, usize)) -> SharedString {
    format!("{prefix}-{}-{}", key.0, key.1).into()
}

/// Placeholder for art that failed to load: a duotone glyph on `surface_2`, so broken art never
/// looks like art that is still loading.
fn art_missing(glyph: Ph, px_size: f32) -> AnyElement {
    div().size_full().flex().items_center().justify_center().bg(Tokens::surface_2()).child(ph_icon(glyph, Weight::Duotone, Tokens::text_3(), px_size)).into_any_element()
}

fn art_loading() -> AnyElement {
    div().relative().size_full().overflow_hidden().bg(Tokens::surface_2()).child(shimmer()).into_any_element()
}

/// Cover image that fills its box: a shimmer while loading and a broken-image glyph on failure.
/// Served from the disk cache when the artwork was downloaded before.
pub fn pic(url: impl Into<SharedString>) -> Img {
    img(crate::common::image_source(&url.into())).size_full().object_fit(ObjectFit::Cover).with_loading(art_loading).with_fallback(|| art_missing(ph::IMAGE_BROKEN, isz::HERO))
}

/// [`pic`] for small boxes (logos, thumbnails), with a `glyph_px` fallback glyph that fits the box
/// (a 20px glyph was clipped in the 16px source-row logo).
pub fn pic_small(url: impl Into<SharedString>, fallback: Ph, glyph_px: f32) -> Img {
    img(crate::common::image_source(&url.into())).size_full().object_fit(ObjectFit::Cover).with_loading(art_loading).with_fallback(move || art_missing(fallback, glyph_px))
}

/// Full-bleed backdrop. Transparent while loading, so the slide underneath stays until the new
/// art is ready; the page colour on failure (a glyph would sit in the middle of the hero).
fn backdrop(url: impl Into<SharedString>) -> Img {
    img(crate::common::image_source(&url.into()))
        .size_full()
        .object_fit(ObjectFit::Cover)
        .with_loading(|| div().size_full().into_any_element())
        .with_fallback(|| div().size_full().bg(Tokens::base()).into_any_element())
}

/// Bigger Cinemeta artwork than the catalog's `small` posters.
pub fn hi_res(url: &str) -> String {
    url.replace("/poster/small/", "/poster/medium/")
}

/// Fades `el` in over `duration::SLOW` with the enter curve; shown at once under reduced motion.
pub fn fade_in<E: IntoElement + Styled + 'static>(id: impl Into<ElementId>, el: E) -> AnyElement {
    if mv_ui::motion::reduced() {
        return el.into_any_element();
    }
    el.with_animation(id, Animation::new(duration::SLOW).with_easing(easing::enter), |e, t| e.opacity(t)).into_any_element()
}

/// Full-bleed artwork with the left and bottom fades that keep the text readable. `under` is the
/// slide being replaced: the new art fades in over it, so a carousel crossfades instead of
/// flashing to black.
pub fn billboard(id: impl Into<ElementId>, height: f32, image: Option<String>, under: Option<String>) -> Stateful<Div> {
    let id: ElementId = id.into();
    let art = ElementId::NamedChild(Arc::new(id.clone()), "art".into());
    let under = under.filter(|u| image.as_ref() != Some(u));
    div()
        .id(id)
        .relative()
        .w_full()
        .h(px(height))
        .overflow_hidden()
        .bg(Tokens::base())
        .when_some(under, |d, u| d.child(div().absolute().inset_0().child(backdrop(u))))
        .when_some(image, |d, u| d.child(fade_in(art, div().absolute().inset_0().child(backdrop(u)))))
        .child(div().absolute().inset_0().bg(linear_gradient(90., linear_color_stop(ink(0.92), 0.), linear_color_stop(ink(0.), 0.62))))
        .child(div().absolute().left_0().right_0().top_0().h(px(110.)).bg(linear_gradient(180., linear_color_stop(ink(0.7), 0.), linear_color_stop(ink(0.), 1.))))
        .child(div().absolute().left_0().right_0().bottom_0().h(px(height * 0.42)).bg(linear_gradient(180., linear_color_stop(ink(0.), 0.), linear_color_stop(ink(1.), 1.))))
}

/// Width of the sources panel docked on the right of a detail hero.
pub const SIDE_PANEL: f32 = 430.;

/// Widest the hero copy may be while the sources panel is docked beside it, with a 32px gap.
/// A share of the width (it was half) ran under the panel at the default window size.
pub fn beside_panel(page_width: f32) -> Pixels {
    px((page_width - EDGE * 2. - SIDE_PANEL - 32.).max(280.))
}

/// Text block placed over a billboard, bottom-left.
pub fn hero_copy(bottom: f32) -> Div {
    div().absolute().left(px(EDGE)).bottom(px(bottom)).w(px(560.)).max_w(relative(0.6)).flex().flex_col().gap(px(14.))
}

fn hero_heading(text: SharedString) -> Div {
    div().ramp(ramp::TITLE_LARGE).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).line_clamp(2).child(text)
}

/// Big hero title; a transparent logo image when there is one, text otherwise.
pub fn hero_title(title: &str, logo: Option<String>) -> AnyElement {
    let text = SharedString::from(title.to_string());
    match logo {
        Some(u) => {
            let (a, b) = (text.clone(), text);
            div()
                .h(px(110.))
                .flex()
                .items_end()
                .child(
                    img(crate::common::image_source(&u))
                        .h(px(110.))
                        .max_w(px(460.))
                        .object_fit(ObjectFit::Contain)
                        .with_fallback(move || hero_heading(a.clone()).into_any_element())
                        .with_loading(move || hero_heading(b.clone()).into_any_element()),
                )
                .into_any_element()
        }
        None => hero_heading(text).into_any_element(),
    }
}

/// Placeholder for a hero whose title is still loading: title and synopsis shaped skeletons.
pub fn hero_skeleton() -> Div {
    div().flex().flex_col().gap(px(14.)).child(skeleton(px(420.), px(44.), radius::MD)).child(skeleton(px(160.), px(18.), radius::SM)).child(skeleton(px(520.), px(66.), radius::MD))
}

/// Metadata line: an optional lead (the rating) then dot-free spaced parts.
pub fn meta_line(lead: Option<AnyElement>, parts: Vec<String>) -> Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(10.))
        .text_size(px(size::MD))
        .text_color(Tokens::text_2())
        .children(lead)
        .children(parts.into_iter().filter(|p| !p.is_empty()).map(|p| div().child(SharedString::from(p))))
}

/// Metadata tag over artwork ("TV", "Series", "4K"): the kit tag on glass so it reads on any art.
pub fn meta_tag(text: impl Into<SharedString>) -> Div {
    tag(text, Tone::Neutral).bg(Tokens::glass()).border_1().border_color(Tokens::glass_border()).text_color(Tokens::text_1())
}

/// [`meta_tag`] with a leading glyph ("4K" with the HD mark).
pub fn meta_icon_tag(glyph: Ph, text: impl Into<SharedString>) -> Div {
    icon_tag(glyph, text, Tone::Neutral).bg(Tokens::glass()).border_1().border_color(Tokens::glass_border()).text_color(Tokens::text_1())
}

/// Quality the found links offer: the resolution with the HD mark, "HDR" as plain text.
pub fn quality_tag(q: &'static str) -> Div {
    if q == "HDR" {
        meta_tag(q)
    } else {
        meta_icon_tag(ph::HIGH_DEFINITION, q)
    }
}

/// Score with a filled gold star, plus a muted source label ("IMDb") when there is one.
pub fn score(score: Option<f64>, icon_size: f32, source: Option<&'static str>) -> Option<AnyElement> {
    let s = score.filter(|s| *s > 0.0)?;
    Some(
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(rating_badge(s as f32, icon_size))
            .when_some(source, |d, src| d.child(div().text_size(px(size::XS)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_3()).child(src)))
            .into_any_element(),
    )
}

/// Round icon button for use over artwork (hero arrows, a card's corner action): a glass disc
/// with a hairline that brightens on hover, so it reads on any image. Tooltip required.
pub fn glass_button(id: impl Into<ElementId>, glyph: Ph, label: impl Into<SharedString>, box_px: f32) -> Stateful<Div> {
    let label: SharedString = label.into();
    let small = box_px <= 28.;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(box_px))
        .rounded_full()
        .bg(Tokens::glass())
        .border_1()
        .border_color(Tokens::glass_border())
        .role(Role::Button)
        .aria_label(label.clone())
        .tooltip(tip(label))
        .pressable()
        .hover(|s| s.bg(light(0.22)).border_color(light(0.35)))
        .child(icon(if small { glyph.bold() } else { glyph.regular() }, Tokens::text_1(), if small { isz::SM } else if box_px >= 40. { isz::XL } else { isz::MD }))
}

/// [`glass_button`] that toggles something on the card (an episode's sources): when `on`, the disc
/// fills with the accent and the glyph switches to its fill weight, so the on state never rests on
/// visibility alone (Material 3 toggle icon buttons). The tooltip names what a click will do.
pub fn glass_toggle(id: impl Into<ElementId>, glyph: Ph, on: bool, label: impl Into<SharedString>, box_px: f32) -> Stateful<Div> {
    if !on {
        return glass_button(id, glyph, label, box_px);
    }
    let label: SharedString = label.into();
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(box_px))
        .rounded_full()
        .bg(Tokens::accent())
        .border_1()
        .border_color(Tokens::accent_border())
        .role(Role::Button)
        .aria_label(label.clone())
        .aria_toggled(Toggled::True)
        .tooltip(tip(label))
        .pressable()
        .hover(|s| s.bg(Tokens::accent_hover()))
        .child(icon(glyph.fill(), Tokens::on_accent(), if box_px <= 28. { isz::SM } else { isz::MD }))
}

/// Where an element was last painted, filled in by [`bounds_probe`].
pub type BoundsSlot = Rc<Cell<Option<Bounds<Pixels>>>>;

/// Invisible layer that records its parent's painted bounds into `slot` (place it in a
/// `relative()` parent). The carousel compares them with the pointer to know whether the hero is
/// hovered: an `on_hover` flag goes stale when a kept-alive page is left while the pointer is over
/// the hero, and the carousel then stayed paused for good.
pub fn bounds_probe(slot: &BoundsSlot) -> impl IntoElement {
    let slot = slot.clone();
    canvas(move |bounds, _, _| slot.set(Some(bounds)), |_, _, _, _| {}).absolute().inset_0()
}

/// The pointer is inside the bounds last recorded in `slot`.
pub fn pointer_over(slot: &BoundsSlot, window: &Window) -> bool {
    slot.get().is_some_and(|b| b.contains(&window.mouse_position()))
}

/// Primary hero action: the filled play glyph, so the main action has the weight of Netflix's
/// and Apple TV's (Fluent 2: filled icons for emphasis).
pub fn hero_play(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Btn {
    btn(id, label, ButtonKind::Primary).large().icon(ph::PLAY.fill())
}

/// Secondary hero action ("More info").
pub fn hero_more(id: impl Into<ElementId>) -> Btn {
    btn(id, "More info", ButtonKind::Secondary).large().icon(ph::INFO)
}

/// My List toggle: "Add to My List" with an outline bookmark, or "In My List" on the tonal accent
/// container with the filled bookmark (Material 3 toggle buttons). Spinner while saving.
pub fn list_button(id: impl Into<ElementId>, saved: bool, saving: bool) -> Btn {
    btn(id, if saved { "In My List" } else { "Add to My List" }, ButtonKind::Secondary)
        .large()
        .icon(glyphs::SAVED)
        .selected(saved)
        .loading(saving)
        .tooltip(if saved { "Remove from My List" } else { "Save to My List to find it later" })
}

/// Resume progress under a hero's play button: a thin bar and "14m left".
pub fn resume_note(id: impl Into<ElementId>, progress: f32, left: String) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(div().w(px(140.)).child(progress_bar(id, progress, false)))
        .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(SharedString::from(left)))
}

/// Section heading on the cinematic pages: a 20px title (the row size), an icon and an
/// optional count.
pub fn media_section(title: impl Into<SharedString>, glyph: Ph, count: Option<String>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(icon(glyph.regular(), Tokens::text_2(), isz::LG))
        .child(div().ramp(ramp::SUBTITLE).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child(title.into()))
        .when_some(count, |d, c| d.child(div().text_size(px(size::MD)).text_color(Tokens::text_3()).child(SharedString::from(c))))
}

/// Paged horizontal row. Shows `per_page` items from `start`; arrows at the edges page it, and
/// dashes at the top right show where you are and jump to a page (like Netflix). `see_all` adds
/// a "See all" action after the title.
#[allow(clippy::too_many_arguments)]
pub fn row<V: 'static>(
    cx: &mut Context<V>,
    id: &'static str,
    title: impl Into<SharedString>,
    start: usize,
    per_page: usize,
    gap: f32,
    items: Vec<AnyElement>,
    set: fn(&mut V, &'static str, usize),
    see_all: Option<OnClick>,
) -> Div {
    let per_page = per_page.max(1);
    let total = items.len();
    let pages = total.div_ceil(per_page);
    let start = start.min(total.saturating_sub(1)) / per_page * per_page;
    let page = start / per_page;
    let (has_prev, has_next) = (start > 0, start + per_page < total);
    let group: SharedString = format!("row-{id}").into();
    let arrow = |left: bool, target: usize, cx: &mut Context<V>| {
        let label = if left { "Previous" } else { "Next" };
        div()
            .id((id, if left { 0usize } else { 1 }))
            .absolute()
            .top_0()
            .bottom_0()
            .when(left, |d| d.left_0())
            .when(!left, |d| d.right_0())
            .w(px(EDGE - 6.))
            .flex()
            .items_center()
            .justify_center()
            .bg(ink(0.55))
            .opacity(0.)
            .group_hover(group.clone(), |s| s.opacity(1.))
            .role(Role::Button)
            .aria_label(label)
            .pressable()
            .hover(|s| s.bg(ink(0.8)))
            // A keyboard user tabbing here sees the arrow even without hovering the row.
            .focus_visible(|s| s.opacity(1.).shadow(focus_ring_shadow()))
            .tooltip(tip(label))
            .child(icon(if left { ph::CARET_LEFT.bold() } else { ph::CARET_RIGHT.bold() }, Tokens::text_1(), isz::XL))
            .on_click(cx.listener(move |this, _, _, cx| {
                set(this, id, target);
                cx.notify();
            }))
    };
    let prev = arrow(true, start.saturating_sub(per_page), cx);
    let next = arrow(false, start + per_page, cx);
    let dashes = (pages > 1).then(|| {
        div().flex().items_center().opacity(0.).group_hover(group.clone(), |s| s.opacity(1.)).children((0..pages).map(|i| {
            let on = i == page;
            let dash_group: SharedString = format!("row-{id}-dash-{i}").into();
            div()
                .id((id, 10 + i))
                .group(dash_group.clone())
                .flex()
                .items_center()
                .justify_center()
                .w(px(24.))
                .h(px(16.))
                .cursor_pointer()
                .tooltip(tip(format!("Page {} of {pages}", i + 1)))
                .child(div().w(px(14.)).h(px(3.)).rounded_full().bg(if on { Tokens::text_2() } else { light(0.22) }).when(!on, |d| d.group_hover(dash_group, |s| s.bg(light(0.6)))))
                .on_click(cx.listener(move |this, _, _, cx| {
                    set(this, id, i * per_page);
                    cx.notify();
                }))
        }))
    });
    div()
        .group(group)
        .w_full()
        .mb(px(30.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(EDGE))
                .mb(px(10.))
                .child(div().ramp(ramp::SUBTITLE).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child(title.into()))
                .when_some(see_all, |d, f| {
                    d.child(btn(SharedString::from(format!("see-all-{id}")), "See all", ButtonKind::Ghost).small().trailing_icon(ph::CARET_RIGHT).on_click(move |e, w, cx| f(e, w, cx)))
                })
                .child(div().flex_1())
                .children(dashes),
        )
        .child(
            div()
                .relative()
                .w_full()
                .px(px(EDGE))
                // Its own id scope: cards keyed ("airing", 0) must not share hover, focus and
                // tooltip state with this row's arrows and page dashes, which use the same keys.
                // The 4px padding (cancelled by the margin) keeps the cards' 2px focus ring inside
                // the clip, which cut it off above and below every card.
                .child(
                    div()
                        .id(SharedString::from(format!("row-{id}-cards")))
                        .flex()
                        .gap(px(gap))
                        .overflow_hidden()
                        .p(px(4.))
                        .m(px(-4.))
                        .children(items.into_iter().skip(start).take(per_page)),
                )
                .when(has_prev, |d| d.child(prev))
                .when(has_next, |d| d.child(next)),
        )
}

/// Row offsets keyed by row id, kept on the page so they survive navigation.
pub type RowOffsets = HashMap<&'static str, usize>;

/// Skeleton row while a catalog loads: the row title and shimmering cards of the right shape.
pub fn skeleton_row(title: &str, w: f32, h: f32, n: usize) -> Div {
    div()
        .mb(px(30.))
        .child(div().px(px(EDGE)).mb(px(10.)).ramp(ramp::SUBTITLE).font_weight(FontWeight::BOLD).text_color(Tokens::text_3()).child(SharedString::from(title.to_string())))
        .child(div().flex().gap(px(8.)).px(px(EDGE)).overflow_hidden().children((0..n).map(|_| skeleton(px(w), px(h), radius::MD))))
}

/// Grid of poster-shaped skeletons (search results, See all, Discover).
pub fn skeleton_posters(n: usize, w: f32) -> Div {
    div().flex().flex_wrap().gap_x(px(16.)).gap_y(px(28.)).children((0..n).map(|_| div().flex().flex_col().gap(px(8.)).child(skeleton(px(w), px(w * 1.5), radius::SM)).child(skeleton_line(px(w * 0.8))).child(skeleton_line(px(w * 0.5)))))
}

/// What hovering a card promises. It must match what a click does (Jakob's law: a play glyph
/// means play).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Hover {
    /// Click plays: a filled play disc appears.
    Play,
    /// Click opens the detail page: just the outline.
    Open,
}

/// 16:9 card with the title over a bottom fade and an optional progress bar.
pub fn landscape_card(key: (&'static str, usize), w: f32, image: Option<String>, title: &str, sub: Option<String>, progress: Option<f32>, hover: Hover) -> Stateful<Div> {
    let h = w * 9.0 / 16.0;
    let group = group_of("lc", key);
    let title: SharedString = title.to_string().into();
    div()
        .id(key)
        .group(group.clone())
        .relative()
        .flex_none()
        .w(px(w))
        .rounded(px(radius::MD))
        .cursor_pointer()
        .role(Role::Link)
        .aria_label(title.clone())
        .focus_ring()
        .child(
            div()
                .relative()
                .w(px(w))
                .h(px(h))
                .rounded(px(radius::MD))
                .overflow_hidden()
                .bg(Tokens::surface_2())
                .border_2()
                .border_color(light(0.))
                .group_hover(group.clone(), |s| s.border_color(light(0.85)))
                .when_some(image, |d, u| d.child(pic(u)))
                .child(div().absolute().left_0().right_0().bottom_0().h(px(h * 0.6)).bg(linear_gradient(180., linear_color_stop(ink(0.), 0.), linear_color_stop(ink(0.9), 1.))))
                .when(hover == Hover::Play, |d| {
                    d.child(
                        div().absolute().inset_0().flex().items_center().justify_center().opacity(0.).group_hover(group.clone(), |s| s.opacity(1.)).child(
                            div().size(px(48.)).rounded_full().bg(ink(0.6)).border_2().border_color(Tokens::text_1()).flex().items_center().justify_center().child(icon(ph::PLAY.fill(), Tokens::text_1(), isz::LG)),
                        ),
                    )
                })
                .child(
                    div()
                        .absolute()
                        .left(px(10.))
                        .right(px(10.))
                        .bottom(px(if progress.is_some() { 12. } else { 8. }))
                        .flex()
                        .flex_col()
                        .child(div().text_size(px(size::MD)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).truncate().child(title))
                        .when_some(sub, |d, s| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).truncate().child(SharedString::from(s)))),
                )
                .when_some(progress, |d, p| d.child(div().absolute().left_0().right_0().bottom_0().h(px(4.)).bg(light(0.25)).child(div().h_full().w(relative(p.clamp(0.02, 1.0))).bg(Tokens::accent())))),
        )
}

/// 2:3 poster with the title below; hovering shows the synopsis over the art (Crunchyroll) and
/// says "Details", because a click opens the detail page.
#[allow(clippy::too_many_arguments)]
pub fn poster_card(key: (&'static str, usize), w: f32, image: Option<String>, title: &str, sub: String, rating: Option<f64>, blurb: Option<String>, corner: Option<String>) -> Stateful<Div> {
    let h = w * 1.5;
    let group = group_of("pc", key);
    let title: SharedString = title.to_string().into();
    let rating = rating.filter(|s| *s > 0.0);
    div()
        .id(key)
        .group(group.clone())
        .flex_none()
        .w(px(w))
        .flex()
        .flex_col()
        .gap(px(8.))
        .rounded(px(radius::SM))
        .cursor_pointer()
        .role(Role::Link)
        .aria_label(title.clone())
        .focus_ring()
        .child(
            div()
                .relative()
                .w(px(w))
                .h(px(h))
                .rounded(px(radius::SM))
                .overflow_hidden()
                .bg(Tokens::surface_2())
                .when_some(image, |d, u| d.child(pic(u)))
                .when_some(corner, |d, b| d.child(div().absolute().top(px(8.)).left(px(8.)).child(badge(b, Tone::Accent))))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .p(px(12.))
                        .flex()
                        .flex_col()
                        .justify_end()
                        .gap(px(8.))
                        .bg(ink(0.88))
                        .opacity(0.)
                        .group_hover(group.clone(), |s| s.opacity(1.))
                        .child(div().text_size(px(size::MD)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).line_clamp(3).child(title.clone()))
                        .when_some(blurb, |d, b| d.child(div().text_size(px(size::XS)).line_height(px(16.)).text_color(Tokens::text_2()).line_clamp(6).child(SharedString::from(b))))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .text_size(px(size::SM))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(Tokens::accent())
                                .child(icon(ph::INFO.bold(), Tokens::accent(), isz::SM))
                                .child("Details"),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).line_clamp(2).group_hover(group, |s| s.text_color(Tokens::accent())).child(title))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .min_w_0()
                        .when_some(rating, |d, s| d.child(rating_badge(s as f32, isz::XS)))
                        .child(div().min_w_0().text_size(px(size::XS)).text_color(Tokens::text_3()).truncate().child(SharedString::from(sub))),
                ),
        )
}

/// Rank numeral drawn as an outline (eight offset copies in `text_3` under a fill in the page
/// colour), like Netflix's Top 10. GPUI text has no stroke, so the outline is composed.
fn outlined_numeral(text: SharedString, font: f32, line: f32) -> Div {
    let stroke = (font / 64.).clamp(1.5, 3.);
    let offsets: [(f32, f32); 8] = [(-1., -1.), (0., -1.), (1., -1.), (-1., 0.), (1., 0.), (-1., 1.), (0., 1.), (1., 1.)];
    div()
        .relative()
        .text_size(px(font))
        .line_height(px(line))
        .font_weight(FontWeight::BLACK)
        .children(offsets.map(|(x, y)| div().absolute().left(px(x * stroke)).top(px(y * stroke)).text_color(Tokens::text_3()).child(text.clone())))
        .child(div().relative().text_color(Tokens::base()).child(text))
}

/// "Top 10" card: an outlined rank numeral with the poster tucked against it. The title is in
/// the tooltip and the accessible label.
pub fn top10_card(key: (&'static str, usize), rank: usize, image: Option<String>, h: f32, title: &str) -> Stateful<Div> {
    let w = h / 1.5;
    let group = group_of("t10", key);
    let title: SharedString = title.to_string().into();
    div()
        .id(key)
        .group(group.clone())
        .flex_none()
        .flex()
        .items_end()
        .rounded(px(radius::SM))
        .cursor_pointer()
        .role(Role::Link)
        .aria_label(SharedString::from(format!("Number {rank}: {title}")))
        .focus_ring()
        .tooltip(tip(title))
        .child(div().w(px(if rank >= 10 { h * 0.74 } else { h * 0.45 })).flex().justify_end().child(outlined_numeral(rank.to_string().into(), h * 0.9, h * 0.86)))
        .child(
            div()
                .ml(px(-h * 0.08))
                .w(px(w))
                .h(px(h))
                .rounded(px(radius::SM))
                .overflow_hidden()
                .bg(Tokens::surface_2())
                .border_2()
                .border_color(light(0.))
                .group_hover(group, |s| s.border_color(light(0.85)))
                .when_some(image, |d, u| d.child(pic(u))),
        )
}

/// Dims the page and shows `content` in a centered card (source picker). The scrim click and
/// Escape close it (Material dialogs); the content card blocks clicks to the scrim. Focus `focus`
/// when the overlay opens so Escape reaches it (a mouse press on kit controls does not move focus).
/// The scrim occludes the page: without that, cards under it kept their hover and the click that
/// dismissed the scrim also opened (or played) the card beneath it.
pub fn overlay(focus: &FocusHandle, content: impl IntoElement, on_close: impl Fn(&mut Window, &mut App) + 'static) -> Stateful<Div> {
    let on_close = Rc::new(on_close);
    let on_esc = on_close.clone();
    div()
        .id("source-overlay")
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(Tokens::scrim())
        .occlude()
        .track_focus(focus)
        .on_key_down(move |ev: &KeyDownEvent, w, cx| {
            if ev.keystroke.key == "escape" {
                cx.stop_propagation();
                on_esc(w, cx);
            }
        })
        .on_mouse_down(MouseButton::Left, move |_, w, cx| on_close(w, cx))
        .child(div().id("source-overlay-card").w(px(680.)).max_w(relative(0.9)).max_h(relative(0.8)).occlude().child(content))
}

/// Carousel position dots under a hero: 4px bars in a 24px hit area (Fitts's law), brighter on
/// hover, with "Slide 3 of 6" tooltips.
pub fn dots<V: 'static>(cx: &mut Context<V>, n: usize, current: usize, set: fn(&mut V, usize)) -> Div {
    div().flex().items_center().children((0..n).map(|i| {
        let on = i == current;
        let group: SharedString = format!("hero-dot-{i}").into();
        let label = format!("Slide {} of {n}", i + 1);
        div()
            .id(("hero-dot", i))
            .group(group.clone())
            .flex()
            .items_center()
            .justify_center()
            .h(px(24.))
            .px(px(5.))
            .rounded(px(radius::XS))
            .cursor_pointer()
            .role(Role::Button)
            .aria_label(SharedString::from(label.clone()))
            .aria_selected(on)
            .focus_ring()
            .tooltip(tip(label))
            .child(div().h(px(4.)).w(px(if on { 28. } else { 14. })).rounded_full().bg(if on { Tokens::accent() } else { light(0.3) }).when(!on, |d| d.group_hover(group, |s| s.bg(light(0.6)))))
            .on_click(cx.listener(move |this, _, _, cx| {
                set(this, i);
                cx.notify();
            }))
    }))
}

/// Calls `step` every 9 s while the page is alive; `step` decides whether to advance (it skips
/// while the hero is hovered, paused, an overlay is open, or motion is reduced). It gets the
/// window so it can check the pointer against the hero's bounds.
pub fn rotate<V: 'static>(window: &mut Window, cx: &mut Context<V>, step: fn(&mut V, &Window) -> bool) {
    cx.spawn_in(window, async move |this, cx| loop {
        cx.background_executor().timer(std::time::Duration::from_secs(9)).await;
        if this
            .update_in(cx, |v, window, cx| {
                if step(v, window) {
                    cx.notify();
                }
            })
            .is_err()
        {
            break;
        }
    })
    .detach();
}

/// First paragraph of a synopsis, without source notes, for heroes and cards.
pub fn blurb(text: &str) -> String {
    let first = text.split("\n\n").map(str::trim).find(|p| !p.is_empty()).unwrap_or("");
    let first = first.split("(Source:").next().unwrap_or(first).split("[Written by").next().unwrap_or(first);
    first.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// "12m left" / "1h 4m left" for a resume point; `None` when nothing meaningful is left.
pub fn time_left(position: f64, duration: f64) -> Option<String> {
    if duration <= 0.0 || position <= 0.0 {
        return None;
    }
    let mins = ((duration - position).max(0.0) / 60.0).round() as u64;
    Some(match (mins / 60, mins % 60) {
        (0, 0) => "Almost done".to_string(),
        (0, m) => format!("{m}m left"),
        (h, 0) => format!("{h}h left"),
        (h, m) => format!("{h}h {m}m left"),
    })
}

#[cfg(test)]
mod tests {
    use super::{blurb, per_row, time_left};

    #[test]
    fn blurb_keeps_the_first_paragraph_only() {
        assert_eq!(blurb("A  hero rises.\n\n(Source: MAL)"), "A hero rises.");
        assert_eq!(blurb("Two\nlines. (Source: X)"), "Two lines.");
        assert_eq!(blurb(""), "");
    }

    #[test]
    fn time_left_reads_naturally() {
        assert_eq!(time_left(600., 1440.), Some("14m left".into()));
        assert_eq!(time_left(60., 3900.), Some("1h 4m left".into()));
        assert_eq!(time_left(0., 1440.), None);
        assert_eq!(time_left(1430., 1440.), Some("Almost done".into()));
    }

    #[test]
    fn rows_fit_at_least_one_card() {
        assert_eq!(per_row(200., 300., 8.), 1);
        assert!(per_row(1400., 300., 8.) >= 4);
    }
}
