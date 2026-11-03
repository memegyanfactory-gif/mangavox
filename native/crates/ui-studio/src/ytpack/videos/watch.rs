//! The watch page: YouTube's player (2026 controls: round play button, volume and time pills, the
//! right-hand pill, the red-to-pink progress bar) playing the rendered file, then the title, the
//! channel row with Subscribe and the like / share / save buttons, the description box, the
//! comments header, and the related column of real videos.
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::{ph, ph_icon, Weight};

use super::library::MyVideo;
use super::related::YtVideo;
use super::{yt, VideosPage};

const SECONDARY_W: f32 = 388.;
const RELATED_THUMB_W: f32 = 242.;
/// Narrower than this, the related column moves under the video (as on youtube.com).
const TWO_COLUMNS: f32 = 1000.;
const HIDE_AFTER: Duration = Duration::from_secs(3);

/// The fraction of `b`'s width at `x`.
fn frac(b: Bounds<Pixels>, x: Pixels) -> Option<f32> {
    let w = f32::from(b.size.width);
    (w > 0.).then(|| (f32::from(x - b.origin.x) / w).clamp(0., 1.))
}

impl VideosPage {
    pub(super) fn watch_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let page_w = self.width.get();
        let two = page_w >= TWO_COLUMNS;
        let theater = self.watch.as_ref().is_some_and(|w| w.theater);
        let primary_w = if two && !theater { (page_w - 16. - 16. - SECONDARY_W - 16.).max(480.) } else { page_w - 32. };
        let player_h = if theater { (page_w * 9. / 16.).min(f32::from(window.viewport_size().height) - 169.).max(240.) } else { primary_w * 9. / 16. };
        let player = self.player_view(if theater { page_w } else { primary_w }, player_h, theater, window, cx);
        let info = self.info(cx);
        let side = self.side_column(cx);
        if theater {
            div()
                .flex()
                .flex_col()
                .w_full()
                .child(div().w_full().h(px(player_h)).bg(yt::black(1.)).child(player))
                .child(
                    div()
                        .flex()
                        .w_full()
                        .gap(px(16.))
                        .px(px(16.))
                        .map(|d| if two { d.child(div().flex_1().min_w_0().child(info)).child(div().flex_none().w(px(SECONDARY_W)).pt(px(12.)).child(side)) } else { d.flex_col().child(info).child(side) }),
                )
                .into_any_element()
        } else if two {
            div()
                .flex()
                .w_full()
                .gap(px(16.))
                .pl(px(16.))
                .pr(px(16.))
                .pt(px(12.))
                .child(div().flex().flex_col().w(px(primary_w)).flex_none().child(div().w(px(primary_w)).h(px(player_h)).child(player)).child(info))
                .child(div().flex_none().w(px(SECONDARY_W)).child(side))
                .into_any_element()
        } else {
            div().flex().flex_col().w_full().px(px(16.)).pt(px(12.)).child(div().w(px(primary_w)).h(px(player_h)).child(player)).child(info).child(side).into_any_element()
        }
    }

    fn player_view(&mut self, w: f32, h: f32, theater: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(watch) = self.watch.as_mut() else { return div().into_any_element() };
        let (frame, pos, dur, playing, muted, volume) = match watch.player.as_mut() {
            Some(p) => {
                p.advance();
                for old in p.retired.drain(..) {
                    cx.drop_image(old, Some(window));
                }
                if p.playing() || p.waiting() {
                    window.request_animation_frame();
                }
                (p.shown.as_ref().map(|s| s.1.clone()), p.position(), p.duration, p.playing(), p.muted, p.volume)
            }
            None => (None, 0., watch.video.secs, false, false, 1.),
        };
        let shown_pos = watch.drag.map(|f| f as f64 * dur).unwrap_or(pos);
        let idle = watch.moved.elapsed() > HIDE_AFTER;
        let controls = !playing || watch.drag.is_some() || (watch.hover_player && !idle);
        // While playing, every frame redraws, which also hides the controls when idle.
        let played = if dur > 0. { (shown_pos / dur).clamp(0., 1.) as f32 } else { 0. };
        let loaded = if dur > 0. { ((pos + 60.) / dur).clamp(0., 1.) as f32 } else { 0. };
        let (bar, vol_b) = (watch.bar.clone(), watch.vol.clone());
        let hover_vol = watch.hover_volume;
        let radius = if theater { 0. } else { 12. };

        let video = div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .when_some(frame, |d, f| d.child(img(f).size_full().rounded(px(radius)).object_fit(ObjectFit::Contain)));

        let bar_el = {
            let bar = bar.clone();
            div()
                .id("ytv-bar")
                .absolute()
                .left(px(12.))
                .right(px(12.))
                .bottom(px(54.))
                .h(px(16.))
                .flex()
                .items_center()
                .cursor_pointer()
                .group("ytv-bar")
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(4.))
                        .group_hover("ytv-bar", |s| s.h(px(6.)))
                        .rounded(px(3.))
                        .bg(hsla(0., 0., 0.16, 0.6))
                        .child(div().absolute().left_0().top_0().bottom_0().w(relative(loaded)).rounded(px(3.)).bg(yt::tone(0.4)))
                        .child(div().absolute().left_0().top_0().bottom_0().w(relative(played)).rounded(px(3.)).bg(linear_gradient(90., linear_color_stop(yt::c(yt::RED), 0.8), linear_color_stop(yt::c(yt::PINK), 1.))))
                        .child(div().absolute().top(px(-4.)).left(relative(played)).ml(px(-6.5)).size(px(13.)).rounded_full().bg(yt::c(yt::RED)).invisible().group_hover("ytv-bar", |s| s.visible())),
                )
                .child(canvas(move |b, _, _| bar.set(b), |_, _, _, _| {}).absolute().left_0().right_0().top_0().bottom_0())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        if let Some(w) = this.watch.as_mut() {
                            w.drag = frac(w.bar.get(), ev.position.x);
                        }
                        cx.notify();
                    }),
                )
        };

        let pill = |d: Div| d.h(px(40.)).rounded(px(28.)).bg(yt::black(0.3));
        let ctl_icon = |icon: mv_ui::Ph, weight: Weight| ph_icon(icon, weight, yt::c(0xffffff), 24.);
        let play_btn = div()
            .id("ytv-play")
            .flex()
            .items_center()
            .justify_center()
            .size(px(40.))
            .rounded_full()
            .bg(yt::black(0.3))
            .cursor_pointer()
            .child(ctl_icon(if playing { ph::PAUSE } else { ph::PLAY }, Weight::Fill))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                this.toggle_play(cx);
            }));
        let vol_icon = if muted || volume == 0. { ph::SPEAKER_X } else if volume < 0.5 { ph::SPEAKER_LOW } else { ph::SPEAKER_HIGH };
        let volume_pill = pill(div())
            .id("ytv-volume")
            .flex()
            .items_center()
            .ml(px(12.))
            .on_hover(cx.listener(|this, on: &bool, _, cx| {
                if let Some(w) = this.watch.as_mut() {
                    w.hover_volume = *on;
                }
                cx.notify();
            }))
            .child(
                div()
                    .id("ytv-mute")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(40.))
                    .cursor_pointer()
                    .child(ctl_icon(vol_icon, Weight::Fill))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        if let Some(p) = this.watch.as_mut().and_then(|w| w.player.as_mut()) {
                            let (v, m) = (p.volume, !p.muted);
                            p.set_volume(v, m);
                        }
                        cx.notify();
                    })),
            )
            .when(hover_vol, |d| {
                let vol_b = vol_b.clone();
                let level = if muted { 0. } else { volume };
                d.child(
                    div()
                        .id("ytv-vol-slider")
                        .relative()
                        .w(px(52.))
                        .h(px(40.))
                        .mr(px(12.))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .child(div().relative().w_full().h(px(3.)).rounded(px(2.)).bg(yt::tone(0.2)).child(div().absolute().left_0().top_0().bottom_0().w(relative(level)).rounded(px(2.)).bg(yt::c(0xffffff))))
                        .child(div().absolute().left(relative(level)).ml(px(-6.)).size(px(12.)).rounded_full().bg(yt::c(0xffffff)))
                        .child(canvas(move |b, _, _| vol_b.set(b), |_, _, _, _| {}).absolute().inset_0())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                if let Some(w) = this.watch.as_mut() {
                                    if let (Some(f), Some(p)) = (frac(w.vol.get(), ev.position.x), w.player.as_mut()) {
                                        p.set_volume(f, false);
                                    }
                                }
                                cx.notify();
                            }),
                        ),
                )
            });
        let time_pill = pill(div())
            .flex()
            .items_center()
            .ml(px(8.))
            .px(px(12.))
            .text_size(px(14.))
            .font_weight(FontWeight::MEDIUM)
            .child(div().text_color(yt::c(0xffffff)).child(yt::clock(shown_pos)))
            .child(div().text_color(yt::c(0xeeeeee)).child(format!(" / {}", yt::clock(dur))));
        let right_btn = |id: &'static str, icon: mv_ui::Ph| div().id(id).flex().items_center().justify_center().w(px(48.)).h(px(40.)).cursor_pointer().child(ctl_icon(icon, Weight::Regular));
        let autoplay = div().flex().items_center().justify_center().w(px(48.)).h(px(40.)).child(
            div()
                .relative()
                .w(px(36.))
                .h(px(14.))
                .rounded(px(7.))
                .bg(yt::tone(0.3))
                .child(div().absolute().left(px(-2.)).top(px(-3.)).size(px(20.)).rounded_full().bg(yt::c(0xbdbdbd)).flex().items_center().justify_center().child(ph_icon(ph::PAUSE, Weight::Fill, yt::c(0x0f0f0f), 11.))),
        );
        let right = pill(div())
            .flex()
            .items_center()
            .px(px(4.))
            .child(autoplay)
            .child(right_btn("ytv-cc", ph::SUBTITLES))
            .child(right_btn("ytv-settings", ph::GEAR_SIX))
            .child(right_btn("ytv-theater", if theater { ph::CORNERS_IN } else { ph::FRAME_CORNERS }).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                if let Some(w) = this.watch.as_mut() {
                    w.theater = !w.theater;
                }
                cx.notify();
            })))
            .child(right_btn("ytv-full", ph::CORNERS_OUT).on_click(cx.listener(|_, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                window.toggle_fullscreen();
            })));
        let chrome = div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(160.))
            .bg(linear_gradient(180., linear_color_stop(yt::black(0.), 0.), linear_color_stop(yt::black(0.65), 1.)))
            .child(bar_el)
            .child(div().absolute().left(px(12.)).right(px(12.)).bottom(px(8.)).h(px(40.)).flex().items_center().child(play_btn).child(volume_pill).child(time_pill).child(div().flex_1()).child(right));

        div()
            .id("ytv-player")
            .relative()
            .size_full()
            .rounded(px(radius))
            .overflow_hidden()
            .bg(yt::black(1.))
            .cursor(if controls { CursorStyle::Arrow } else { CursorStyle::default() })
            .on_hover(cx.listener(|this, on: &bool, _, cx| {
                if let Some(w) = this.watch.as_mut() {
                    w.hover_player = *on;
                    w.moved = Instant::now();
                }
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if let Some(w) = this.watch.as_mut() {
                    w.moved = Instant::now();
                    if w.drag.is_some() {
                        w.drag = frac(w.bar.get(), ev.position.x);
                    }
                }
                cx.notify();
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.end_drag(cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.end_drag(cx)))
            .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                if ev.click_count() == 2 {
                    if let Some(w) = this.watch.as_mut() {
                        w.theater = !w.theater;
                    }
                    cx.notify();
                } else {
                    this.toggle_play(cx);
                }
            }))
            .child(video)
            .when(controls, |d| d.child(chrome))
            .w(px(w))
            .h(px(h))
            .into_any_element()
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        let Some(w) = self.watch.as_mut() else { return };
        if let (Some(f), Some(p)) = (w.drag.take(), w.player.as_mut()) {
            let t = f as f64 * p.duration;
            p.seek(t);
        }
        cx.notify();
    }

    fn toggle_play(&mut self, cx: &mut Context<Self>) {
        if let Some(w) = self.watch.as_mut() {
            w.moved = Instant::now();
            if let Some(p) = w.player.as_mut() {
                let on = !p.playing();
                p.set_playing(on);
            }
        }
        self.watch_route(cx);
        cx.notify();
    }

    /// Pauses the video when the user leaves the page (the page stays alive in history).
    pub(super) fn watch_route(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(300)).await;
            let go_on = this.update(cx, |this, cx| {
                let here = mv_ui::nav::current_route(cx) == Some(mv_ui::Route::YouTubeVideos);
                match this.watch.as_mut().and_then(|w| w.player.as_mut()) {
                    Some(p) if p.playing() && !here => {
                        p.set_playing(false);
                        cx.notify();
                        false
                    }
                    Some(p) => p.playing(),
                    None => false,
                }
            });
            if !matches!(go_on, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// Title, channel row, actions, description and the comments header.
    fn info(&self, cx: &mut Context<Self>) -> Div {
        let Some(w) = self.watch.as_ref() else { return div() };
        let v = &w.video;
        let name = self.channel.display_name();
        let subs = match self.channel.subscribers {
            Some(1) => "1 subscriber".to_string(),
            Some(n) => format!("{} subscribers", super::super::logic::short_count(n)),
            None => "No subscribers".to_string(),
        };
        let menu_open = self.menu.as_deref() == Some(v.key().as_str());
        let owner = div()
            .flex()
            .items_center()
            .child(yt::avatar(&self.channel.avatar, &name, 40.).mr(px(12.)))
            .child(div().flex().flex_col().mr(px(24.)).child(yt::text(16., 22., FontWeight::MEDIUM, yt::TEXT).child(name)).child(yt::text(12., 18., FontWeight::NORMAL, yt::TEXT_2).child(subs)))
            .child(div().h(px(36.)).px(px(16.)).flex().items_center().rounded(px(18.)).bg(yt::c(yt::TEXT)).child(yt::text(14., 36., FontWeight::MEDIUM, yt::BG).child("Subscribe")));
        let like = div()
            .flex()
            .items_center()
            .h(px(36.))
            .rounded(px(18.))
            .bg(yt::tone(0.1))
            .child(div().flex().items_center().gap(px(6.)).pl(px(12.)).pr(px(12.)).h_full().child(ph_icon(ph::THUMBS_UP, Weight::Regular, yt::c(yt::TEXT), 22.)))
            .child(div().w(px(1.)).h(px(24.)).bg(yt::tone(0.2)))
            .child(div().flex().items_center().px(px(12.)).h_full().child(ph_icon(ph::THUMBS_DOWN, Weight::Regular, yt::c(yt::TEXT), 22.)));
        let key = v.key();
        let actions = div()
            .relative()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(like)
            .child(yt::round_button("ytv-share", ph::ARROW_U_UP_RIGHT, 36.))
            .child(yt::round_button("ytv-download", ph::DOWNLOAD_SIMPLE, 36.))
            .child(yt::round_button("ytv-save", ph::BOOKMARK_SIMPLE, 36.))
            .child(
                yt::round_button("ytv-more", ph::DOTS_THREE, 36.).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.menu = if this.menu.as_deref() == Some(key.as_str()) { None } else { Some(key.clone()) };
                    cx.notify();
                })),
            )
            .when(menu_open, |d| d.child(self.thumb_menu(v, cx)));
        let age = v.rendered.elapsed().map(|d| d.as_secs()).unwrap_or(0);
        let more = w.more;
        let desc = div()
            .id("ytv-desc")
            .mt(px(12.))
            .p(px(12.))
            .rounded(px(12.))
            .bg(yt::tone(0.1))
            .when(!more, |d| d.cursor_pointer().hover(|s| s.bg(yt::tone(0.2))))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                if let Some(w) = this.watch.as_mut() {
                    w.more = true;
                }
                cx.notify();
            }))
            .child(yt::text(14., 20., FontWeight::MEDIUM, yt::TEXT).child(format!("{}  {}", yt::views(0), yt::ago_long(age))))
            .child(yt::text(14., 20., FontWeight::NORMAL, yt::TEXT).whitespace_normal().when(!more, |d| d.line_clamp(2)).child(v.description.clone()))
            .child(
                div()
                    .id("ytv-desc-toggle")
                    .when(more, |d| d.mt(px(20.)))
                    .child(yt::text(14., 20., FontWeight::MEDIUM, yt::TEXT).child(if more { "Show less" } else { "...more" }))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        if let Some(w) = this.watch.as_mut() {
                            w.more = !w.more;
                        }
                        cx.notify();
                    })),
            );
        let comments = div()
            .flex()
            .flex_col()
            .mt(px(24.))
            .mb(px(32.))
            .gap(px(24.))
            .child(div().flex().items_center().gap(px(32.)).child(yt::text(20., 28., FontWeight::BOLD, yt::TEXT).child("0 Comments")).child(div().flex().items_center().gap(px(8.)).child(ph_icon(ph::SORT_ASCENDING, Weight::Regular, yt::c(yt::TEXT), 24.)).child(yt::text(14., 20., FontWeight::MEDIUM, yt::TEXT).child("Sort by"))))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(16.))
                    .child(yt::avatar(&self.channel.avatar, &self.channel.display_name(), 40.))
                    .child(div().flex_1().pt(px(8.)).pb(px(4.)).border_b_1().border_color(yt::tone(0.2)).child(yt::text(14., 20., FontWeight::NORMAL, yt::TEXT_2).child("Add a comment..."))),
            );
        div()
            .flex()
            .flex_col()
            .child(yt::text(20., 28., FontWeight::BOLD, yt::TEXT).mt(px(12.)).line_clamp(2).child(v.title.clone()))
            .child(div().flex().flex_wrap().items_center().justify_between().gap(px(12.)).mt(px(8.)).child(owner).child(actions))
            .child(desc)
            .child(comments)
    }

    /// Real videos (and the user's other videos of the same manga) as YouTube's related column.
    fn side_column(&self, cx: &mut Context<Self>) -> Div {
        let Some(w) = self.watch.as_ref() else { return div() };
        let v = &w.video;
        let mine: Vec<&MyVideo> = self.my_videos().iter().filter(|o| o.manga_id == v.manga_id && o.file != v.file).collect();
        let real = self.related_for(&v.manga_id);
        let mut col = div().flex().flex_col().gap(px(8.)).pb(px(24.));
        if real.is_empty() {
            let busy = self.refreshing.contains(&v.manga_id);
            let msg = match (busy, self.related.get(&v.manga_id)) {
                (true, _) => "Finding real videos about this manga on YouTube…".to_string(),
                (false, Some(super::Rel::Failed(e))) => format!("Real videos could not be loaded: {e}"),
                _ => "No related videos were found on YouTube.".to_string(),
            };
            col = col.child(div().flex().flex_col().items_start().gap(px(8.)).child(yt::text(12., 18., FontWeight::NORMAL, yt::TEXT_2).whitespace_normal().child(msg)).when(!busy, |d| {
                d.child(yt::chip("ytv-side-retry", "Try again", false).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.asked.clear();
                    this.load_related(cx);
                    cx.notify();
                })))
            }));
        }
        let mut mine_iter = mine.into_iter();
        for (i, r) in real.iter().take(30).enumerate() {
            // The user's other videos sit where YouTube puts "from this channel" ones.
            if i == 1 || i == 4 {
                if let Some(m) = mine_iter.next() {
                    col = col.child(self.compact_mine(i, m, cx));
                }
            }
            col = col.child(compact_real(i, r));
        }
        for (k, m) in mine_iter.enumerate() {
            col = col.child(self.compact_mine(100 + k, m, cx));
        }
        col
    }

    fn compact_mine(&self, i: usize, m: &MyVideo, cx: &mut Context<Self>) -> Stateful<Div> {
        let thumb = self.thumbs.shown.get(&m.key()).cloned().flatten().map(ImageSource::from);
        let age = m.rendered.elapsed().map(|d| d.as_secs()).unwrap_or(0);
        let open = m.clone();
        compact(("ytv-side-mine", i), thumb, yt::clock(m.secs), m.title.clone(), self.channel.display_name(), false, format!("{} · {}", 0, yt::ago_short(&yt::ago_long(age))))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.close(cx);
                this.open(open.clone(), cx);
                this.watch_route(cx);
            }))
    }
}

fn compact_real(i: usize, r: &YtVideo) -> Stateful<Div> {
    let url = r.url();
    let thumb = (!r.thumb.is_empty()).then(|| ImageSource::from(SharedString::from(r.thumb.clone())));
    let meta = if r.published.is_empty() { super::super::logic::short_count(r.views) } else { format!("{} · {}", super::super::logic::short_count(r.views), yt::ago_short(&r.published)) };
    compact(("ytv-side", i), thumb, r.length.clone(), r.title.clone(), r.channel.clone(), r.verified, meta).on_click(move |_, _, cx| cx.open_url(&url))
}

/// One row of the related column: thumbnail left, title / channel / "▷ 134K · 2mo ago" right.
fn compact(id: (&'static str, usize), thumb: Option<ImageSource>, length: String, title: String, channel: String, verified: bool, meta: String) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_start()
        .cursor_pointer()
        .child(div().flex_none().w(px(RELATED_THUMB_W)).mr(px(12.)).child(yt::thumbnail(thumb, length, 8., 4.)))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .pr(px(16.))
                .child(yt::text(14., 20., FontWeight::MEDIUM, yt::TEXT).line_clamp(3).mb(px(4.)).child(title))
                .child(div().flex().items_center().gap(px(4.)).child(yt::text(12., 18., FontWeight::NORMAL, yt::TEXT_2).truncate().child(channel)).when(verified, |d| d.child(yt::verified(12.))))
                .child(div().flex().items_center().gap(px(2.)).child(ph_icon(ph::PLAY, Weight::Regular, yt::c(yt::TEXT_2), 12.)).child(yt::text(12., 18., FontWeight::NORMAL, yt::TEXT_2).child(meta))),
        )
}
