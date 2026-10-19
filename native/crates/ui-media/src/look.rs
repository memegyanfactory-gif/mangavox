//! Streaming-service look shared by the Movies (Netflix-style) and Anime (Crunchyroll-style)
//! pages: billboard hero, paged rows with edge arrows, landscape / poster / top-10 cards and
//! the overlay used by the source picker.
use std::collections::HashMap;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::{icon, size, Tokens};

/// Movies accent (Netflix red) and Anime accent (Crunchyroll orange).
pub fn netflix() -> Rgba {
    rgb(0xe50914)
}
pub fn crunchy() -> Rgba {
    rgb(0xf47521)
}

/// Left/right inset of rows and hero text; the row arrows live inside it.
pub const EDGE: f32 = 44.0;

fn ink(alpha: f32) -> Hsla {
    let mut c: Hsla = Tokens::base().into();
    c.a = alpha;
    c
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

/// Cover image that fills its box, with a quiet placeholder while loading or on failure.
/// Served from the disk cache when the artwork was downloaded before.
pub fn pic(url: impl Into<SharedString>) -> Img {
    img(crate::common::image_source(&url.into()))
        .size_full()
        .object_fit(ObjectFit::Cover)
        .with_loading(|| div().size_full().bg(Tokens::surface_2()).into_any_element())
        .with_fallback(|| div().size_full().bg(Tokens::surface_2()).into_any_element())
}

/// Bigger Cinemeta artwork than the catalog's `small` posters.
pub fn hi_res(url: &str) -> String {
    url.replace("/poster/small/", "/poster/medium/")
}

/// Full-bleed artwork with the left and bottom fades that keep the text readable.
pub fn billboard(id: impl Into<ElementId>, height: f32, image: Option<String>) -> Stateful<Div> {
    div()
        .id(id)
        .relative()
        .w_full()
        .h(px(height))
        .overflow_hidden()
        .bg(Tokens::base())
        .when_some(image, |d, u| d.child(div().absolute().inset_0().child(pic(u))))
        .child(div().absolute().inset_0().bg(linear_gradient(90., linear_color_stop(ink(0.92), 0.), linear_color_stop(ink(0.), 0.62))))
        .child(div().absolute().left_0().right_0().top_0().h(px(110.)).bg(linear_gradient(180., linear_color_stop(ink(0.7), 0.), linear_color_stop(ink(0.), 1.))))
        .child(div().absolute().left_0().right_0().bottom_0().h(px(height * 0.42)).bg(linear_gradient(180., linear_color_stop(ink(0.), 0.), linear_color_stop(ink(1.), 1.))))
}

/// Text block placed over a billboard, bottom-left.
pub fn hero_copy(bottom: f32) -> Div {
    div().absolute().left(px(EDGE)).bottom(px(bottom)).w(px(560.)).max_w(relative(0.6)).flex().flex_col().gap(px(14.))
}

/// Big hero title; a transparent logo image when there is one, text otherwise.
pub fn hero_title(title: &str, logo: Option<String>) -> AnyElement {
    let text = SharedString::from(title.to_string());
    let heading = {
        let text = text.clone();
        move || div().text_size(px(44.)).line_height(px(48.)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).line_clamp(2).child(text.clone()).into_any_element()
    };
    match logo {
        Some(u) => div()
            .h(px(110.))
            .flex()
            .items_end()
            .child(img(crate::common::image_source(&u)).h(px(110.)).max_w(px(460.)).object_fit(ObjectFit::Contain).with_fallback(heading.clone()).with_loading(heading))
            .into_any_element(),
        None => heading(),
    }
}

/// Dot-separated metadata line. `lead` (e.g. a rating) is drawn in the accent colour.
pub fn meta_line(lead: Option<(String, Rgba)>, parts: Vec<String>) -> Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(10.))
        .text_size(px(size::MD))
        .text_color(Tokens::text_2())
        .when_some(lead, |d, (t, c)| d.child(div().text_color(c).font_weight(FontWeight::SEMIBOLD).child(SharedString::from(t))))
        .children(parts.into_iter().filter(|p| !p.is_empty()).map(|p| div().child(SharedString::from(p))))
}

/// Small outlined tag ("HD", "TV", a genre).
pub fn tag(text: impl Into<SharedString>) -> Div {
    div()
        .px(px(6.))
        .h(px(20.))
        .flex()
        .items_center()
        .rounded(px(4.))
        .border_1()
        .border_color(rgba(0xffffff40))
        .text_size(px(size::XS))
        .text_color(Tokens::text_2())
        .child(text.into())
}

/// Solid call-to-action ("Play", "Start watching E1").
pub fn cta(id: impl Into<ElementId>, label: impl Into<SharedString>, bg: Rgba, fg: Rgba) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(42.))
        .px(px(22.))
        .rounded(px(6.))
        .bg(bg)
        .text_color(fg)
        .text_size(px(size::LG))
        .font_weight(FontWeight::BOLD)
        .cursor_pointer()
        .hover(|s| s.opacity(0.85))
        .child(icon("icons/play.svg", fg, 18.))
        .child(label.into())
}

/// Translucent secondary action ("More info", "My list").
pub fn soft(id: impl Into<ElementId>, ic: &'static str, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(42.))
        .px(px(20.))
        .rounded(px(6.))
        .bg(rgba(0x6d6d6eb3))
        .text_color(Tokens::text_1())
        .text_size(px(size::LG))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(|s| s.bg(rgba(0x6d6d6e80)))
        .child(icon(ic, Tokens::text_1(), 18.))
        .child(label.into())
}

/// Paged horizontal row. Shows `per_page` items from `start`; arrows at the edges page it,
/// and dashes at the top right show where you are (like Netflix).
pub fn row<V: 'static>(
    cx: &mut Context<V>,
    id: &'static str,
    title: impl Into<SharedString>,
    start: usize,
    per_page: usize,
    gap: f32,
    items: Vec<AnyElement>,
    set: fn(&mut V, &'static str, usize),
) -> Div {
    let total = items.len();
    let pages = total.div_ceil(per_page.max(1));
    let start = start.min(total.saturating_sub(1)) / per_page.max(1) * per_page.max(1);
    let page = start / per_page.max(1);
    let (has_prev, has_next) = (start > 0, start + per_page < total);
    let arrow = |side: &'static str, ic: &'static str, target: usize, cx: &mut Context<V>| {
        div()
            .id((id, if side == "l" { 0usize } else { 1 }))
            .absolute()
            .top_0()
            .bottom_0()
            .when(side == "l", |d| d.left_0())
            .when(side == "r", |d| d.right_0())
            .w(px(EDGE - 6.))
            .flex()
            .items_center()
            .justify_center()
            .bg(ink(0.55))
            .cursor_pointer()
            .opacity(0.)
            .group_hover(id, |s| s.opacity(1.))
            .hover(|s| s.bg(ink(0.8)))
            .child(icon(ic, Tokens::text_1(), 26.))
            .on_click(cx.listener(move |this, _, _, cx| {
                set(this, id, target);
                cx.notify();
            }))
    };
    let prev = arrow("l", "icons/chevron-left.svg", start.saturating_sub(per_page), cx);
    let next = arrow("r", "icons/chevron-right.svg", start + per_page, cx);
    div()
        .group(id)
        .w_full()
        .mb(px(30.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(px(EDGE))
                .mb(px(10.))
                .child(div().text_size(px(size::XL)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child(title.into()))
                .when(pages > 1, |d| {
                    d.child(div().flex().gap(px(3.)).opacity(0.).group_hover(id, |s| s.opacity(1.)).children((0..pages).map(|i| {
                        div().w(px(12.)).h(px(2.)).bg(if i == page { Tokens::text_2() } else { rgba(0xffffff30) })
                    })))
                }),
        )
        .child(
            div()
                .relative()
                .w_full()
                .px(px(EDGE))
                .child(div().flex().gap(px(gap)).overflow_hidden().children(items.into_iter().skip(start).take(per_page)))
                .when(has_prev, |d| d.child(prev))
                .when(has_next, |d| d.child(next)),
        )
}

/// Row offsets keyed by row id, kept on the page so they survive navigation.
pub type RowOffsets = HashMap<&'static str, usize>;

/// Skeleton row while a catalog loads.
pub fn skeleton_row(title: &str, w: f32, h: f32, n: usize) -> Div {
    div()
        .mb(px(30.))
        .child(div().px(px(EDGE)).mb(px(10.)).text_size(px(size::XL)).font_weight(FontWeight::BOLD).text_color(Tokens::text_3()).child(SharedString::from(title.to_string())))
        .child(div().flex().gap(px(8.)).px(px(EDGE)).overflow_hidden().children((0..n).map(|_| div().flex_none().w(px(w)).h(px(h)).rounded(px(6.)).bg(Tokens::surface_2()))))
}

/// 16:9 card with the title over a bottom fade and an optional progress bar.
pub fn landscape_card(
    id: impl Into<ElementId>,
    w: f32,
    image: Option<String>,
    title: &str,
    sub: Option<String>,
    progress: Option<f32>,
    accent: Rgba,
) -> Stateful<Div> {
    let h = w * 9.0 / 16.0;
    let group: SharedString = format!("lc-{}-{}", title, w as u32).into();
    div()
        .id(id)
        .group(group.clone())
        .flex_none()
        .w(px(w))
        .cursor_pointer()
        .child(
            div()
                .relative()
                .w(px(w))
                .h(px(h))
                .rounded(px(6.))
                .overflow_hidden()
                .bg(Tokens::surface_2())
                .border_2()
                .border_color(rgba(0xffffff00))
                .group_hover(group.clone(), |s| s.border_color(rgba(0xffffffd9)))
                .when_some(image, |d, u| d.child(pic(u)))
                .child(div().absolute().left_0().right_0().bottom_0().h(px(h * 0.6)).bg(linear_gradient(180., linear_color_stop(ink(0.), 0.), linear_color_stop(ink(0.9), 1.))))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .opacity(0.)
                        .group_hover(group.clone(), |s| s.opacity(1.))
                        .child(div().size(px(46.)).rounded_full().bg(ink(0.6)).border_2().border_color(Tokens::text_1()).flex().items_center().justify_center().child(icon("icons/play.svg", Tokens::text_1(), 20.))),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(10.))
                        .right(px(10.))
                        .bottom(px(if progress.is_some() { 12. } else { 8. }))
                        .flex()
                        .flex_col()
                        .child(div().text_size(px(size::MD)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).truncate().child(SharedString::from(title.to_string())))
                        .when_some(sub, |d, s| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).truncate().child(SharedString::from(s)))),
                )
                .when_some(progress, |d, p| {
                    d.child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .h(px(4.))
                            .bg(rgba(0xffffff40))
                            .child(div().h_full().w(relative(p.clamp(0.02, 1.0))).bg(accent)),
                    )
                }),
        )
}

/// 2:3 poster with the title below; hovering shows the synopsis over the art (Crunchyroll).
pub fn poster_card(
    id: impl Into<ElementId>,
    w: f32,
    image: Option<String>,
    title: &str,
    sub: String,
    blurb: Option<String>,
    badge: Option<String>,
    accent: Rgba,
) -> Stateful<Div> {
    let h = w * 1.5;
    let group: SharedString = format!("pc-{}-{}", title, w as u32).into();
    div()
        .id(id)
        .group(group.clone())
        .flex_none()
        .w(px(w))
        .flex()
        .flex_col()
        .gap(px(8.))
        .cursor_pointer()
        .child(
            div()
                .relative()
                .w(px(w))
                .h(px(h))
                .rounded(px(4.))
                .overflow_hidden()
                .bg(Tokens::surface_2())
                .when_some(image, |d, u| d.child(pic(u)))
                .when_some(badge, |d, b| {
                    d.child(
                        div()
                            .absolute()
                            .top(px(8.))
                            .left(px(8.))
                            .px(px(6.))
                            .py(px(1.))
                            .rounded(px(3.))
                            .bg(ink(0.8))
                            .text_size(px(size::XS))
                            .font_weight(FontWeight::BOLD)
                            .text_color(Tokens::text_1())
                            .child(SharedString::from(b)),
                    )
                })
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
                        .child(div().text_size(px(size::MD)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).line_clamp(3).child(SharedString::from(title.to_string())))
                        .when_some(blurb, |d, b| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).line_clamp(6).child(SharedString::from(b))))
                        .child(div().flex().items_center().gap(px(6.)).text_size(px(size::SM)).font_weight(FontWeight::BOLD).text_color(accent).child(icon("icons/play.svg", accent, 14.)).child("PLAY")),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).line_clamp(2).group_hover(group, |s| s.text_color(accent)).child(SharedString::from(title.to_string())))
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).truncate().child(SharedString::from(sub))),
        )
}

/// "Top 10" card: a big rank numeral with the poster tucked against it.
pub fn top10_card(id: impl Into<ElementId>, rank: usize, image: Option<String>, h: f32) -> Stateful<Div> {
    let w = h / 1.5;
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_end()
        .cursor_pointer()
        .hover(|s| s.opacity(0.9))
        .child(
            div()
                .w(px(if rank >= 10 { h * 0.74 } else { h * 0.45 }))
                .flex()
                .justify_end()
                .text_size(px(h * 0.9))
                .line_height(px(h * 0.86))
                .font_weight(FontWeight::BLACK)
                .text_color(rgb(0x4b5159))
                .child(SharedString::from(rank.to_string())),
        )
        .child(div().ml(px(-h * 0.08)).w(px(w)).h(px(h)).rounded(px(4.)).overflow_hidden().bg(Tokens::surface_2()).when_some(image, |d, u| d.child(pic(u))))
}

/// Dim the page and show `content` in a centered card (source picker).
pub fn overlay(content: impl IntoElement) -> Div {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(Tokens::scrim())
        .child(div().w(px(680.)).max_w(relative(0.9)).max_h(relative(0.8)).child(content))
}

/// Carousel position dots under a hero.
pub fn dots<V: 'static>(cx: &mut Context<V>, n: usize, current: usize, accent: Rgba, set: fn(&mut V, usize)) -> Div {
    div().flex().gap(px(6.)).children((0..n).map(|i| {
        div()
            .id(("dot", i))
            .h(px(4.))
            .w(px(if i == current { 28. } else { 14. }))
            .rounded(px(2.))
            .cursor_pointer()
            .bg(if i == current { accent } else { rgba(0xffffff4d) })
            .on_click(cx.listener(move |this, _, _, cx| {
                set(this, i);
                cx.notify();
            }))
    }))
}

/// Advance a hero carousel every few seconds while the page is alive.
pub fn rotate<V: 'static>(cx: &mut Context<V>, step: fn(&mut V)) {
    cx.spawn(async move |this, cx| loop {
        cx.background_executor().timer(std::time::Duration::from_secs(9)).await;
        if this
            .update(cx, |v, cx| {
                step(v);
                cx.notify();
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
    let first = text.split("

").map(str::trim).find(|p| !p.is_empty()).unwrap_or("");
    let first = first.split("(Source:").next().unwrap_or(first).split("[Written by").next().unwrap_or(first);
    first.split_whitespace().collect::<Vec<_>>().join(" ")
}
