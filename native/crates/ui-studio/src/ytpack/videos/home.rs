//! The home grid: YouTube's mini guide, the filter chips and the rich grid, with the user's
//! rendered videos placed among real ones about the same and similar manga.
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::{ph, ph_icon, Weight};

use super::library::MyVideo;
use super::related::YtVideo;
use super::{yt, Chip, Rel, VideosPage};

/// Narrowest grid card before a column is dropped.
const MIN_CARD: f32 = 320.;
const GAP: f32 = 16.;
const GUIDE_W: f32 = 72.;

enum Item<'a> {
    Mine(&'a MyVideo),
    Real(&'a YtVideo),
}

/// Moves up to `n` videos not shown yet from `pool` to `out`.
fn take<'a>(pool: &mut std::collections::VecDeque<&'a YtVideo>, n: usize, seen: &mut std::collections::HashSet<String>, out: &mut Vec<Item<'a>>) {
    let mut k = 0;
    while k < n {
        let Some(r) = pool.pop_front() else { break };
        if seen.insert(r.id.clone()) {
            out.push(Item::Real(r));
            k += 1;
        }
    }
}

fn age_of(t: std::time::SystemTime) -> u64 {
    t.elapsed().map(|d| d.as_secs()).unwrap_or(0)
}

impl VideosPage {
    /// What the grid shows for the chip: the user's videos, each followed by real ones of its
    /// manga, then the rest of the real ones.
    fn feed(&self) -> Vec<Item<'_>> {
        let mine: Vec<&MyVideo> = self
            .my_videos()
            .iter()
            .filter(|v| match &self.chip {
                Chip::Manga(id) => v.manga_id == *id,
                _ => true,
            })
            .collect();
        if self.chip == Chip::Yours {
            return mine.into_iter().map(Item::Mine).collect();
        }
        let mut pools: Vec<(String, std::collections::VecDeque<&YtVideo>)> = vec![];
        for v in &mine {
            if !pools.iter().any(|(id, _)| *id == v.manga_id) {
                pools.push((v.manga_id.clone(), self.related_for(&v.manga_id).iter().collect()));
            }
        }
        let mut seen = std::collections::HashSet::new();
        let mut out = vec![];
        for v in &mine {
            out.push(Item::Mine(v));
            if let Some((_, pool)) = pools.iter_mut().find(|(id, _)| *id == v.manga_id) {
                take(pool, 2, &mut seen, &mut out);
            }
        }
        // The rest, alternating between the manga.
        loop {
            let before = out.len();
            for (_, pool) in pools.iter_mut() {
                take(pool, 1, &mut seen, &mut out);
            }
            if out.len() == before || out.len() >= 60 {
                break;
            }
        }
        out
    }

    pub(super) fn home(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let content_w = (self.width.get() - GUIDE_W - 48.).max(MIN_CARD);
        let cols = (((content_w + GAP) / (MIN_CARD + GAP)).floor() as usize).clamp(1, 6);
        let card_w = (content_w - GAP * (cols as f32 - 1.)) / cols as f32;
        let main = match &self.videos {
            crate::page::Load::Loading => self.skeleton(cols, card_w).into_any_element(),
            crate::page::Load::Failed(e) => yt::text(14., 20., FontWeight::NORMAL, yt::TEXT_2).p(px(24.)).child(format!("Could not read Studio's videos: {e}")).into_any_element(),
            crate::page::Load::Ready(v) if v.is_empty() => self.empty(cx).into_any_element(),
            crate::page::Load::Ready(_) => {
                let items = self.feed();
                let cards: Vec<AnyElement> = items
                    .iter()
                    .enumerate()
                    .map(|(i, it)| match it {
                        Item::Mine(v) => self.my_card(i, v, card_w, cx).into_any_element(),
                        Item::Real(r) => real_card(i, r, card_w).into_any_element(),
                    })
                    .collect();
                div().flex().flex_wrap().gap_x(px(GAP)).gap_y(px(40.)).children(cards).into_any_element()
            }
        };
        div()
            .flex()
            .w_full()
            .child(guide())
            .child(div().flex().flex_col().flex_1().min_w_0().px(px(24.)).child(self.chips(cx)).child(self.related_note(cx)).child(div().pt(px(12.)).pb(px(40.)).child(main)))
            .into_any_element()
    }

    fn chips(&self, cx: &mut Context<Self>) -> Div {
        let mut row = div().flex().items_center().gap(px(12.)).h(px(56.)).overflow_hidden();
        let add = |row: Div, id: (&'static str, usize), label: String, chip: Chip, cx: &mut Context<Self>| {
            let on = self.chip == chip;
            row.child(yt::chip(id, label, on).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.chip = chip.clone();
                this.menu = None;
                cx.notify();
            })))
        };
        row = add(row, ("ytv-chip", 0), "All".into(), Chip::All, cx);
        row = add(row, ("ytv-chip", 1), "Your videos".into(), Chip::Yours, cx);
        for (i, (id, title)) in self.manga_ids().into_iter().enumerate() {
            row = add(row, ("ytv-chip-m", i), title, Chip::Manga(id), cx);
        }
        row
    }

    /// While related videos load, or when they could not.
    fn related_note(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = !self.refreshing.is_empty();
        let loading = self.related.values().any(|r| matches!(r, Rel::Loading));
        let failed = self.rel_error.values().next().cloned();
        let empty = !busy && self.related.values().any(|r| matches!(r, Rel::Ready(r) if r.videos.is_empty()));
        let line = match (loading, busy, failed) {
            (true, _, _) => Some("Finding real videos about these manga on YouTube…".to_string()),
            (false, true, _) => Some("Checking YouTube for the newest videos…".to_string()),
            (false, false, Some(e)) => Some(format!("Real videos could not be updated: {e}")),
            _ if empty => Some("No related videos were found on YouTube.".to_string()),
            _ => None,
        };
        div().when_some(line, |d, l| {
            d.flex().items_center().gap(px(12.)).pb(px(4.)).child(yt::text(12., 18., FontWeight::NORMAL, yt::TEXT_2).child(l)).when(!busy, |d| {
                d.child(yt::chip("ytv-retry", "Try again", false).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.asked.clear();
                    this.load_related(cx);
                    cx.notify();
                })))
            })
        })
    }

    fn empty(&self, _cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(12.))
            .pt(px(120.))
            .child(ph_icon(ph::MONITOR_PLAY, Weight::Duotone, yt::c(yt::TEXT_2), 96.))
            .child(yt::text(20., 28., FontWeight::BOLD, yt::TEXT).child("No rendered videos yet"))
            .child(yt::text(14., 20., FontWeight::NORMAL, yt::TEXT_2).child("Render a recap in Studio and it shows up here, next to real videos about the same manga."))
            .child(
                div()
                    .id("ytv-go-studio")
                    .mt(px(8.))
                    .px(px(16.))
                    .h(px(36.))
                    .flex()
                    .items_center()
                    .rounded(px(18.))
                    .bg(yt::c(yt::TEXT))
                    .cursor_pointer()
                    .child(yt::text(14., 36., FontWeight::MEDIUM, yt::BG).child("Open Studio"))
                    .on_click(|_, w, cx| mv_ui::navigate(mv_ui::Route::Studio, w, cx)),
            )
    }

    fn skeleton(&self, cols: usize, w: f32) -> Div {
        div().flex().flex_wrap().gap_x(px(GAP)).gap_y(px(40.)).children((0..cols * 2).map(move |_| {
            div()
                .w(px(w))
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(div().w_full().aspect_ratio(16. / 9.).rounded(px(12.)).bg(yt::tone(0.1)))
                .child(
                    div()
                        .flex()
                        .gap(px(12.))
                        .child(div().size(px(36.)).rounded_full().bg(yt::tone(0.1)))
                        .child(div().flex_1().flex().flex_col().gap(px(8.)).child(div().h(px(20.)).w(relative(0.9)).rounded(px(4.)).bg(yt::tone(0.1))).child(div().h(px(20.)).w(relative(0.6)).rounded(px(4.)).bg(yt::tone(0.1)))),
                )
        }))
    }

    /// One of the user's videos: opens the watch page; ⋮ picks the thumbnail it previews.
    fn my_card(&self, i: usize, v: &MyVideo, w: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let thumb = self.thumbs.shown.get(&v.key()).cloned().flatten().map(ImageSource::from);
        let open = v.clone();
        let key = v.key();
        let menu_open = self.menu.as_deref() == Some(key.as_str());
        let group = SharedString::from(format!("ytv-card-{i}"));
        let meta = format!("{} • {}", yt::views(0), yt::ago_long(age_of(v.rendered)));
        div()
            .id(("ytv-mine", i))
            .group(group.clone())
            .relative()
            .w(px(w))
            .flex()
            .flex_col()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open(open.clone(), cx)))
            .child(yt::thumbnail(thumb, yt::clock(v.secs), 12., 8.))
            .child(details(
                yt::avatar(&self.channel.avatar, &self.channel.display_name(), 36.),
                v.title.clone(),
                self.channel.display_name(),
                false,
                meta,
                Some(self.dots(("ytv-dots", i), key, group, menu_open, cx).into_any_element()),
            ))
            .when(menu_open, |d| d.child(self.thumb_menu(v, cx)))
    }

    /// The ⋮ button beside a card's title (shown on hover, or while its menu is open).
    fn dots(&self, id: (&'static str, usize), key: String, group: SharedString, open: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id(id)
            .flex_none()
            .size(px(36.))
            .mr(px(-8.))
            .mt(px(-6.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .when(!open, |d| d.invisible().group_hover(group, |s| s.visible()))
            .hover(|s| s.bg(yt::tone(0.1)))
            .child(ph_icon(ph::DOTS_THREE_VERTICAL, Weight::Bold, yt::c(yt::TEXT), 20.))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                this.menu = if this.menu.as_deref() == Some(key.as_str()) { None } else { Some(key.clone()) };
                cx.notify();
            }))
    }

    /// YouTube's menu, listing the thumbnails made for the manga to preview on this video.
    pub(super) fn thumb_menu(&self, v: &MyVideo, cx: &mut Context<Self>) -> impl IntoElement {
        let own = self.thumbs.own.get(&v.key()).cloned().flatten();
        let made = self.thumbs.gallery.get(&v.manga_id).cloned().unwrap_or_default();
        let row = |id: (&'static str, usize), pic: Option<std::path::PathBuf>, label: String, on: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(16.))
                .py(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(yt::tone(0.1)))
                .child(div().flex_none().w(px(96.)).h(px(54.)).rounded(px(4.)).overflow_hidden().bg(yt::tone(0.1)).when_some(pic, |d, p| d.child(img(p).size_full().rounded(px(4.)).object_fit(ObjectFit::Cover))))
                .child(yt::text(14., 20., FontWeight::NORMAL, yt::TEXT).flex_1().min_w_0().truncate().child(label))
                .child(div().flex_none().w(px(20.)).when(on, |d| d.child(ph_icon(ph::CHECK, Weight::Bold, yt::c(yt::TEXT), 20.))))
        };
        let mut list = div().id("ytv-thumb-list").flex().flex_col().max_h(px(360.)).overflow_y_scroll();
        let manga_pick = self.thumbs.manga.get(&v.manga_id).cloned().flatten();
        let video = v.clone();
        list = list.child(row(("ytv-pick-default", 0), manga_pick.clone(), "Same as the manga's videos".into(), own.is_none()).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            cx.stop_propagation();
            this.pick(&video, None, cx);
        })));
        for (k, p) in made.iter().enumerate() {
            let video = v.clone();
            let (p, made_at) = (p.0.clone(), p.1);
            let path = p.clone();
            let age = age_of(made_at);
            let on = own.as_deref() == Some(p.as_path());
            list = list.child(row(("ytv-pick", k), Some(p.clone()), format!("Made {}", yt::ago_long(age)), on).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                this.pick(&video, Some(path.clone()), cx);
            })));
        }
        let pop = div()
            .id("ytv-menu-pop")
            .w(px(340.))
            .py(px(8.))
            .rounded(px(12.))
            .bg(yt::c(yt::MENU))
            .shadow_lg()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.menu = None;
                cx.notify();
            }))
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(yt::text(16., 22., FontWeight::MEDIUM, yt::TEXT).px(px(16.)).py(px(8.)).child("Preview thumbnail"))
            .child(list)
            .when(made.is_empty(), |d| d.child(yt::text(12., 18., FontWeight::NORMAL, yt::TEXT_2).px(px(16.)).py(px(8.)).whitespace_normal().child("No thumbnails made for this manga yet. Make some in Studio › Thumbnails.")))
            .child(
                div()
                    .id("ytv-make-thumbs")
                    .mt(px(8.))
                    .border_t_1()
                    .border_color(yt::tone(0.2))
                    .px(px(16.))
                    .pt(px(8.))
                    .cursor_pointer()
                    .child(yt::text(14., 20., FontWeight::NORMAL, 0x3ea6ff).child("Make thumbnails…"))
                    .on_click({
                        let id = v.manga_id.clone();
                        move |_, w, cx| {
                            let mut p = super::super::magnific::Prefs::load();
                            p.manga = id.clone();
                            p.save();
                            mv_ui::navigate(mv_ui::Route::Thumbnails, w, cx);
                        }
                    }),
            );
        div().absolute().top_full().right_0().child(deferred(anchored().anchor(Anchor::TopRight).snap_to_window_with_margin(px(8.)).child(pop)).with_priority(3))
    }
}

/// Avatar, title, channel and the views line under a grid thumbnail.
fn details(avatar: Div, title: String, channel: String, verified: bool, meta: String, trailing: Option<AnyElement>) -> Div {
    div()
        .flex()
        .items_start()
        .pt(px(12.))
        .child(avatar.mr(px(12.)))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .pr(px(24.))
                .child(yt::text(16., 22., FontWeight::MEDIUM, yt::TEXT).line_clamp(2).mb(px(4.)).child(title))
                .child(div().flex().items_center().gap(px(4.)).child(yt::text(14., 20., FontWeight::NORMAL, yt::TEXT_2).truncate().child(channel)).when(verified, |d| d.child(yt::verified(14.))))
                .child(yt::text(14., 20., FontWeight::NORMAL, yt::TEXT_2).truncate().child(meta)),
        )
        .children(trailing)
}

/// A real video: opens on youtube.com.
fn real_card(i: usize, r: &YtVideo, w: f32) -> Stateful<Div> {
    let url = r.url();
    let thumb = (!r.thumb.is_empty()).then(|| ImageSource::from(SharedString::from(r.thumb.clone())));
    let meta = if r.published.is_empty() { yt::views(r.views) } else { format!("{} • {}", yt::views(r.views), r.published) };
    div()
        .id(("ytv-real", i))
        .w(px(w))
        .flex()
        .flex_col()
        .cursor_pointer()
        .on_click(move |_, _, cx| cx.open_url(&url))
        .child(yt::thumbnail(thumb, r.length.clone(), 12., 8.))
        .child(details(yt::avatar(&r.avatar, &r.channel, 36.), r.title.clone(), r.channel.clone(), r.verified, meta, None))
}

/// YouTube's mini guide (narrow windows): Home, Shorts, Subscriptions, You.
fn guide() -> Div {
    let item = |icon, label: &'static str, on: bool| {
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(6.))
            .w(px(64.))
            .pt(px(16.))
            .pb(px(14.))
            .rounded(px(10.))
            .hover(|s| s.bg(yt::tone(0.1)))
            .child(ph_icon(icon, if on { Weight::Fill } else { Weight::Regular }, yt::c(yt::TEXT), 24.))
            .child(yt::text(10., 14., FontWeight::NORMAL, yt::TEXT).child(label))
    };
    div()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(GUIDE_W))
        .px(px(4.))
        .pt(px(4.))
        .child(item(ph::HOUSE_SIMPLE, "Home", true))
        .child(item(ph::FILM_SLATE, "Shorts", false))
        .child(item(ph::TELEVISION_SIMPLE, "Subscriptions", false))
        .child(item(ph::USER_CIRCLE, "You", false))
}
