//! Competitors: every recap channel the hourly research has found, as a sheet you can sort and
//! search; pick one for its numbers (subscribers over time, uploads per week, when it uploads, its
//! keywords and tags) and its videos; pick a video for its whole history: views hour by hour,
//! every title it had, every thumbnail it had (each stored once, when it changed).
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::{ph, spinner};
use mv_youtube::competitors::{self, CompChannel, CompVideo};

use crate::hub::{self, fmt_num};
use crate::page::{yt_hint as hint, YouTubePage};
use crate::{rivals, when, yt};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sort {
    Subscribers,
    Growth,
    Newest,
    Pinned,
}

/// How the open channel's videos are ordered.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum VideoSort {
    Newest,
    Views,
    Fastest,
    Changed,
    Longest,
}

/// The channel list and the video list each scroll in a box of their own this tall, so the
/// numbers beside them stay in view (the page itself no longer runs on and on).
const LIST_H: f32 = 620.;
const VIDEOS_H: f32 = 560.;
/// Videos listed for a channel.
const VIDEOS_SHOWN: usize = 300;

pub(crate) struct RivalsState {
    pub sort: Sort,
    pub vsort: VideoSort,
    pub search: Entity<InputState>,
    pub channel: Option<String>,
    pub video: Option<String>,
    /// The open channel's videos, with the tracker version they were read at.
    videos: Option<(String, u64, Vec<CompVideo>)>,
    pub show: usize,
}

impl RivalsState {
    pub fn new(window: &mut Window, cx: &mut Context<YouTubePage>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search channels, keywords, countries"));
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        Self { sort: Sort::Subscribers, vsort: VideoSort::Newest, search, channel: None, video: None, videos: None, show: 60 }
    }
}

/// Subscribers gained over the last 7 days, from the channel's history.
fn subs_gain(c: &CompChannel, now: i64) -> i64 {
    let last = c.subs_history.last().map(|p| p.value as i64).unwrap_or(c.subscribers as i64);
    let week = c.subs_history.iter().find(|p| p.at >= now - 7 * 86_400).map(|p| p.value as i64).unwrap_or(last);
    last - week
}

fn thumb_src(store: &competitors::CompStore, v: &CompVideo) -> String {
    match v.thumbnails.last() {
        Some(t) if !t.file.is_empty() => store.thumb_path(t).to_string_lossy().into_owned(),
        Some(t) if !t.url.is_empty() => t.url.clone(),
        _ => format!("https://i.ytimg.com/vi/{}/mqdefault.jpg", v.id),
    }
}

impl YouTubePage {
    pub(crate) fn competitors(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let tracker = rivals::tracker(cx);
        let t = tracker.read(cx);
        let now = hub::now();
        let running = t.running;
        let step = t.step.lock().map(|s| s.clone()).unwrap_or_default();
        let error = t.error.clone();
        let version = t.version;
        let channels = t.channels.clone();
        let runs = t.runs.clone();
        let last_run = t.last_run();
        let next_run = t.next_run();
        let store_root = t.store.root().to_path_buf();
        let store = competitors::CompStore::new(store_root);

        // The open channel's videos (re-read after each research pass).
        if let Some(id) = self.rivals.channel.clone() {
            if self.rivals.videos.as_ref().is_none_or(|(c, v, _)| c != &id || *v != version) {
                self.rivals.videos = Some((id.clone(), version, store.videos(&id)));
            }
        }

        let total_videos: u64 = channels.iter().map(|c| c.video_count).sum();
        let day = now - 86_400;
        let recent: Vec<_> = runs.iter().filter(|r| r.at >= day).collect();
        let changes_24h: usize = recent.iter().map(|r| r.title_changes + r.thumb_changes + r.description_changes).sum();
        let new_videos_24h: usize = recent.iter().map(|r| r.new_videos).sum();

        let tile = |label: &'static str, value: String, sub: String| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .flex_1()
                .min_w(px(150.))
                .p(px(16.))
                .rounded(px(12.))
                .border_1()
                .border_color(yt::tone(0.2))
                .child(yt::line(12., FontWeight::NORMAL, yt::TEXT_2, label))
                .child(yt::text(24., 32., FontWeight::BOLD, yt::TEXT).whitespace_nowrap().child(value))
                .child(yt::line(12., FontWeight::NORMAL, yt::TEXT_2, sub))
        };
        let head = div()
            .flex()
            .flex_wrap()
            .gap(px(12.))
            .child(tile("Channels tracked", fmt_num(channels.len() as u64), format!("{} pinned", channels.iter().filter(|c| c.pinned).count())))
            .child(tile("Their videos", fmt_num(total_videos), format!("{new_videos_24h} new in 24 hours")))
            .child(tile("Changes spotted", fmt_num(changes_24h as u64), "Titles, thumbnails, descriptions · 24 h".into()))
            .child(tile(
                "Research",
                if last_run == 0 { "Not yet".into() } else { when::clock(last_run) },
                if running { "Running now".into() } else if last_run == 0 { "First pass starts soon".into() } else { format!("Next {}", when::until(next_run - now)) },
            ));

        let status = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .min_w_0()
            .when(running, |d| d.child(spinner("rv-spin", yt::c(yt::LINK), 16.)).child(yt::line(13., FontWeight::NORMAL, yt::TEXT_2, step.clone()).flex_1()))
            .when(!running, |d| d.child(yt::line(13., FontWeight::NORMAL, yt::TEXT_2, "Every hour while MangaVox is open: finds more recap channels, reads their new uploads' exact times, views, tags and descriptions, and keeps every title and thumbnail change. Public pages only, no API quota.").flex_1()))
            .children(error.map(|e| yt::line(13., FontWeight::NORMAL, yt::ERROR, e).max_w(px(360.))))
            .child(yt::pill_icon("rv-run", ph::ARROWS_CLOCKWISE, if running { "Researching…" } else { "Research now" }).on_click(cx.listener(|_, _, _, cx| rivals::tracker(cx).update(cx, |t, cx| t.run_now(cx)))));

        let activity = (runs.len() > 1).then(|| {
            let last: Vec<_> = runs.iter().rev().take(48).rev().collect();
            let values: Vec<f64> = last.iter().map(|r| (r.new_videos + r.title_changes + r.thumb_changes) as f64).collect();
            let labels: Vec<String> = last.iter().map(|r| format!("{} · {} new videos, {} title and {} thumbnail changes", when::when(r.at), r.new_videos, r.title_changes, r.thumb_changes)).collect();
            yt::card().child(yt::card_title("What changed, research by research")).child(yt::bar_chart("rv-activity", &values, &labels, yt::LINK, 56.))
        });

        if channels.is_empty() {
            return div()
                .flex()
                .flex_col()
                .gap(px(yt::GAP))
                .child(head)
                .child(status)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(12.))
                        .py(px(48.))
                        .child(yt::icon_of(ph::BINOCULARS, 64., yt::TEXT_3))
                        .child(yt::text(16., 22., FontWeight::MEDIUM, yt::TEXT).child(if running { "Finding recap channels…" } else { "No competitors yet" }))
                        .child(yt::para(14., yt::TEXT_2, 3, "The first research pass finds the manga and manhwa recap channels on YouTube. It runs on its own within a minute of opening MangaVox, then every hour.").max_w(px(480.)).text_center()),
                )
                .into_any_element();
        }

        // The sheet: every channel, sorted and searched.
        let q = self.rivals.search.read(cx).value().to_lowercase();
        let q = q.trim().to_string();
        let mut list: Vec<CompChannel> = channels
            .into_iter()
            .filter(|c| !c.hidden)
            .filter(|c| q.is_empty() || c.title.to_lowercase().contains(&q) || c.handle.to_lowercase().contains(&q) || c.country.to_lowercase().contains(&q) || c.keywords.iter().any(|k| k.to_lowercase().contains(&q)))
            .collect();
        match self.rivals.sort {
            Sort::Subscribers => list.sort_by_key(|c| std::cmp::Reverse(c.subscribers)),
            Sort::Growth => list.sort_by_key(|c| std::cmp::Reverse(subs_gain(c, now))),
            Sort::Newest => list.sort_by_key(|c| std::cmp::Reverse(c.first_seen)),
            Sort::Pinned => list.sort_by_key(|c| (!c.pinned, std::cmp::Reverse(c.subscribers))),
        }
        let shown = list.len();
        let sorts = div().flex().flex_wrap().gap(px(8.)).children([(Sort::Subscribers, "Most subscribers"), (Sort::Growth, "Growing fastest"), (Sort::Newest, "Newly found"), (Sort::Pinned, "Pinned first")].into_iter().enumerate().map(|(i, (s, label))| {
            yt::chip(("rv-sort", i), label, self.rivals.sort == s).on_click(cx.listener(move |this, _, _, cx| {
                this.rivals.sort = s;
                cx.notify();
            }))
        }));
        let open = self.rivals.channel.clone();
        let rows = list.iter().take(self.rivals.show).enumerate().map(|(i, c)| {
            let id = c.id.clone();
            let on = open.as_deref() == Some(c.id.as_str());
            let gain = subs_gain(c, now);
            div()
                .id(("rv-ch", i))
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(8.))
                .py(px(8.))
                .min_w_0()
                .rounded(px(8.))
                .cursor_pointer()
                .when(on, |d| d.bg(yt::tone(0.1)))
                .when(!on, |d| d.hover(|s| s.bg(yt::tone(0.05))))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.rivals.channel = Some(id.clone());
                    this.rivals.video = None;
                    cx.notify();
                }))
                .child(yt::avatar(&c.avatar, 36.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(div().flex().items_center().gap(px(6.)).min_w_0().child(yt::line(14., FontWeight::MEDIUM, yt::TEXT, c.title.clone())).when(c.pinned, |d| d.child(yt::icon_of(ph::BOOKMARK_SIMPLE, 12., yt::AMBER))))
                        .child(yt::line(12., FontWeight::NORMAL, yt::TEXT_2, [c.handle.clone(), c.country.clone()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")))
                        .child(yt::line(
                            11.,
                            FontWeight::NORMAL,
                            yt::TEXT_3,
                            [
                                format!("{} videos", fmt_num(c.video_count)),
                                if c.total_views > 0 { format!("{} views in all", fmt_num(c.total_views)) } else { String::new() },
                                if c.total_views > 0 && c.video_count > 0 { format!("{} a video", fmt_num(c.total_views / c.video_count)) } else { String::new() },
                                if c.last_scanned > 0 { format!("checked {}", hub::fmt_ago(c.last_scanned)) } else { String::new() },
                            ]
                            .into_iter()
                            .filter(|s| !s.is_empty())
                            .collect::<Vec<_>>()
                            .join(" · "),
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_end()
                        .flex_none()
                        .child(yt::text(14., 20., FontWeight::MEDIUM, yt::TEXT).child(fmt_num(c.subscribers)))
                        .child(yt::text(12., 16., FontWeight::NORMAL, if gain > 0 { yt::GREEN } else { yt::TEXT_2 }).child(if gain != 0 { format!("{gain:+} · 7 d") } else { format!("{} videos", fmt_num(c.video_count)) })),
                )
        });
        let sheet = yt::card()
            .w(px(400.))
            .flex_none()
            .gap(px(12.))
            .child(div().flex().items_center().gap(px(8.)).child(yt::card_title("Channels").flex_1()).child(yt::caption(format!("{shown} shown"))))
            .child(div().h(px(36.)).px(px(12.)).flex().items_center().rounded(px(18.)).bg(yt::tone(0.08)).child(Input::new(&self.rivals.search).appearance(false)))
            .child(sorts)
            .child(div().id("rv-channels-scroll").flex().flex_col().gap(px(2.)).max_h(px(LIST_H)).overflow_y_scroll().children(rows))
            .when(shown > self.rivals.show, |d| {
                d.child(yt::pill("rv-more", format!("Show more ({} left)", shown - self.rivals.show)).on_click(cx.listener(|this, _, _, cx| {
                    this.rivals.show += 60;
                    cx.notify();
                })))
            });

        let detail = match open.and_then(|id| list.iter().find(|c| c.id == id).cloned()) {
            Some(c) => self.channel_detail(&c, &store, now, cx).into_any_element(),
            None => yt::card()
                .flex_1()
                .min_w(px(420.))
                .items_center()
                .py(px(48.))
                .child(yt::icon_of(ph::BINOCULARS, 48., yt::TEXT_3))
                .child(yt::text(16., 22., FontWeight::MEDIUM, yt::TEXT).child("Pick a channel"))
                .child(hint("Its subscribers over time, when it uploads, its keywords, and every video with its views hour by hour and the titles and thumbnails it changed."))
                .into_any_element(),
        };

        div()
            .flex()
            .flex_col()
            .gap(px(yt::GAP))
            .child(head)
            .child(status)
            .children(activity)
            .child(div().flex().items_start().gap(px(yt::GAP)).child(sheet).child(detail))
            .into_any_element()
    }

    fn channel_detail(&mut self, c: &CompChannel, store: &competitors::CompStore, now: i64, cx: &mut Context<Self>) -> Div {
        let videos: Vec<CompVideo> = self.rivals.videos.as_ref().filter(|(id, _, _)| id == &c.id).map(|(_, _, v)| v.clone()).unwrap_or_default();
        let stats = competitors::channel_stats(c, &videos, now, hub::utc_offset());
        let (id, pinned) = (c.id.clone(), c.pinned);
        let url = if c.handle.starts_with('@') { format!("https://www.youtube.com/{}", c.handle) } else { format!("https://www.youtube.com/channel/{}", c.id) };
        let header = div()
            .flex()
            .gap(px(16.))
            .min_w_0()
            .child(yt::avatar(&c.avatar, 72.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.))
                    .child(yt::line(20., FontWeight::BOLD, yt::TEXT, c.title.clone()))
                    .child(yt::line(13., FontWeight::NORMAL, yt::TEXT_2, [c.handle.clone(), format!("{} subscribers", fmt_num(c.subscribers)), format!("{} videos", fmt_num(c.video_count)), c.country.clone(), c.joined.clone()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")))
                    .when(!c.description.is_empty(), |d| d.child(yt::para(13., yt::TEXT_2, 3, c.description.clone())))
                    .child(yt::caption(format!("Tracked since {} · found via “{}” · checked {}", when::date(c.first_seen), c.found_via, hub::fmt_ago(c.last_scanned.max(1))))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(px(8.))
                    .child(yt::pill_icon("rv-pin", ph::BOOKMARK_SIMPLE, if pinned { "Unpin" } else { "Pin: check every hour" }).on_click(cx.listener(move |_, _, _, cx| rivals::tracker(cx).update(cx, |t, cx| t.set_pinned(&id, !pinned, cx)))))
                    .child(yt::pill_icon("rv-open", ph::ARROW_SQUARE_OUT, "Open channel").on_click(move |_, _, cx| cx.open_url(&url))),
            );

        let kpi = |label: &'static str, value: String| div().flex().flex_col().flex_1().min_w(px(110.)).child(yt::line(12., FontWeight::NORMAL, yt::TEXT_2, label)).child(yt::text(20., 28., FontWeight::BOLD, yt::TEXT).whitespace_nowrap().child(value));
        let kpis = div()
            .flex()
            .flex_wrap()
            .gap(px(16.))
            .child(kpi("Subscribers", fmt_num(c.subscribers)))
            .child(kpi("Views in all", if c.total_views > 0 { fmt_num(c.total_views) } else { "—".into() }))
            .child(kpi("Uploads / week", format!("{:.1}", stats.uploads_per_week)))
            .child(kpi("Average views", fmt_num(stats.avg_views as u64)))
            .child(kpi("Median views", fmt_num(stats.median_views)))
            .child(kpi("Views, last 24 h", fmt_num(stats.views_last_24h)))
            .child(kpi("Typical length", yt::duration(stats.avg_duration_secs as u64)))
            .child(kpi("Subscribers, 7 d", format!("{:+}", stats.subs_gained_7d)))
            .child(kpi("Title changes", stats.title_changes.to_string()))
            .child(kpi("Thumbnail changes", stats.thumb_changes.to_string()));

        let subs_chart = (c.subs_history.len() > 1).then(|| {
            let values: Vec<f64> = c.subs_history.iter().map(|p| p.value as f64).collect();
            let labels: Vec<String> = c.subs_history.iter().map(|p| when::when(p.at)).collect();
            div().flex().flex_col().gap(px(8.)).child(yt::heading("Subscribers over time", None)).child(yt::area_chart("rv-subs", &values, &labels, yt::LINK, 90.))
        });
        let hours: Vec<f64> = stats.upload_hours.iter().map(|n| *n as f64).collect();
        let hour_labels: Vec<String> = (0..24).map(|h| format!("{} · uploads", when::hour(h))).collect();
        const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        let days: Vec<f64> = stats.upload_days.iter().map(|n| *n as f64).collect();
        let day_labels: Vec<String> = DAYS.iter().map(|d| d.to_string()).collect();
        let timing = div()
            .flex()
            .flex_wrap()
            .gap(px(24.))
            .child(div().flex().flex_col().flex_1().min_w(px(280.)).gap(px(8.)).child(yt::heading("When it uploads", Some(format!("Your time ({})", when::zone())))).child(yt::bar_chart("rv-hours", &hours, &hour_labels, yt::PURPLE, 64.)))
            .child(div().flex().flex_col().w(px(200.)).gap(px(8.)).child(yt::heading("Which days", None)).child(yt::bar_chart("rv-days", &days, &day_labels, yt::PURPLE, 64.)));

        let tags = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(yt::heading("Keywords and tags", None))
            .child(div().flex().flex_wrap().gap(px(6.)).children(c.keywords.iter().take(14).map(|k| tag_chip(k.clone(), None))).children(stats.top_tags.iter().take(16).map(|(t, n)| tag_chip(t.clone(), Some(*n)))));

        let mut sorted = videos.clone();
        let changes_of = |v: &CompVideo| v.titles.len().saturating_sub(1) + v.thumbnails.len().saturating_sub(1) + v.descriptions.len().saturating_sub(1);
        match self.rivals.vsort {
            VideoSort::Newest => sorted.sort_by_key(|v| std::cmp::Reverse(v.published_at)),
            VideoSort::Views => sorted.sort_by_key(|v| std::cmp::Reverse(v.views)),
            VideoSort::Fastest => sorted.sort_by(|a, b| competitors::velocity(b, now).total_cmp(&competitors::velocity(a, now))),
            VideoSort::Changed => sorted.sort_by_key(|v| std::cmp::Reverse((changes_of(v), v.published_at))),
            VideoSort::Longest => sorted.sort_by_key(|v| std::cmp::Reverse(v.duration_secs)),
        }
        let vsorts = div().flex().flex_wrap().gap(px(8.)).mt(px(8.)).children([(VideoSort::Newest, "Newest"), (VideoSort::Views, "Most views"), (VideoSort::Fastest, "Fastest right now"), (VideoSort::Changed, "Most changed"), (VideoSort::Longest, "Longest")].into_iter().enumerate().map(|(i, (s, label))| {
            yt::chip(("rv-vsort", i), label, self.rivals.vsort == s).on_click(cx.listener(move |this, _, _, cx| {
                this.rivals.vsort = s;
                cx.notify();
            }))
        }));
        let read = videos.iter().filter(|v| v.published_exact).count();
        let open_video = self.rivals.video.clone();
        let table = div()
            .flex()
            .flex_col()
            .child(yt::heading("Videos", Some(format!("{} tracked ({read} read in full, the rest are read as the hourly research gets to them) · click one for its history", sorted.len()))))
            .child(vsorts)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .h(px(36.))
                    .mt(px(8.))
                    .px(px(8.))
                    .border_b_1()
                    .border_color(yt::tone(0.1))
                    .child(div().flex_1().min_w_0().child(yt::text(12., 18., FontWeight::MEDIUM, yt::TEXT_2).child("Video")))
                    .child(yt::th(140., false, "Published"))
                    .child(yt::th(64., true, "Length"))
                    .child(yt::th(70., true, "Views"))
                    .child(yt::th(60., true, "Likes"))
                    .child(yt::th(70., true, "Comments"))
                    .child(yt::th(70., true, "Per hour"))
                    .child(yt::th(70., true, "Changes")),
            )
            .child(div().id("rv-videos-scroll").flex().flex_col().max_h(px(VIDEOS_H)).overflow_y_scroll().children(sorted.iter().take(VIDEOS_SHOWN).enumerate().map(|(i, v)| {
                let vid = v.id.clone();
                let on = open_video.as_deref() == Some(v.id.as_str());
                let changes = changes_of(v);
                let dash = |n: Option<u64>| n.map(fmt_num).unwrap_or_else(|| "—".into());
                div()
                    .id(("rv-v", i))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(8.))
                    .py(px(6.))
                    .min_w_0()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(yt::tone(0.1)))
                    .when(!on, |d| d.hover(|s| s.bg(yt::tone(0.05))))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.rivals.video = if this.rivals.video.as_deref() == Some(vid.as_str()) { None } else { Some(vid.clone()) };
                        cx.notify();
                    }))
                    .child(div().flex().flex_1().min_w_0().items_center().gap(px(10.)).child(yt::thumb(&thumb_src(store, v), 96., Some(yt::duration(v.duration_secs)).filter(|_| v.duration_secs > 0))).child(yt::para(13., yt::TEXT, 2, v.title.clone()).flex_1()))
                    .child(yt::col(140., false).child(div().flex().flex_col().min_w_0().child(yt::line(13., FontWeight::NORMAL, yt::TEXT, if v.published_at > 0 { when::when(v.published_at) } else { "—".into() })).child(yt::caption(if v.published_exact { "exact" } else { "estimated" }))))
                    .child(yt::col(64., true).child(yt::text(13., 18., FontWeight::NORMAL, yt::TEXT_2).child(if v.duration_secs > 0 { yt::duration(v.duration_secs) } else { "—".into() })))
                    .child(yt::col(70., true).child(yt::text(13., 18., FontWeight::NORMAL, yt::TEXT).child(if v.views == 0 && !v.published_exact { "—".to_string() } else { fmt_num(v.views) })))
                    .child(yt::col(60., true).child(yt::text(13., 18., FontWeight::NORMAL, yt::TEXT_2).child(dash(v.likes))))
                    .child(yt::col(70., true).child(yt::text(13., 18., FontWeight::NORMAL, yt::TEXT_2).child(dash(v.comments))))
                    .child(yt::col(70., true).child(yt::text(13., 18., FontWeight::NORMAL, yt::TEXT_2).child(format!("{:.0}", competitors::velocity(v, now)))))
                    .child(yt::col(70., true).child(if changes > 0 { yt::badge(format!("{changes}×"), 0x0f0f0f, yt::c(yt::AMBER).into()).into_any_element() } else { yt::caption("—").into_any_element() }))
            })))
            .when(sorted.len() > VIDEOS_SHOWN, |d| d.child(yt::caption(format!("The first {VIDEOS_SHOWN} of {} in this order.", sorted.len()))));

        let video_detail = open_video.and_then(|id| videos.iter().find(|v| v.id == id).cloned()).map(|v| self.video_history(&v, store));

        yt::card()
            .flex_1()
            .min_w(px(460.))
            .gap(px(20.))
            .child(header)
            .child(yt::divider())
            .child(kpis)
            .child(yt::divider())
            .children(video_detail)
            .child(table)
            .child(yt::divider())
            .children(subs_chart)
            .child(timing)
            .child(tags)
    }

    /// One competitor video's history: views hour by hour, titles, thumbnails, description edits.
    fn video_history(&self, v: &CompVideo, store: &competitors::CompStore) -> Div {
        let gains = competitors::hourly_gains(v);
        let (values, labels): (Vec<f64>, Vec<String>) = gains.iter().rev().take(72).rev().map(|(at, n)| (*n as f64, format!("{} · +{} views", when::when(*at), fmt_num(*n)))).unzip();
        let url = format!("https://youtu.be/{}", v.id);
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .p(px(16.))
            .rounded(px(12.))
            .bg(yt::tone(0.05))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(16.))
                    .min_w_0()
                    .child(yt::thumb(&thumb_src(store, v), 200., Some(yt::duration(v.duration_secs)).filter(|_| v.duration_secs > 0)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(4.))
                            .child(yt::para(16., yt::TEXT, 3, v.title.clone()).font_weight(FontWeight::MEDIUM))
                            .child(yt::caption(format!(
                                "{} views{}{} · published {}{}",
                                fmt_num(v.views),
                                v.likes.map(|l| format!(" · {} likes", fmt_num(l))).unwrap_or_default(),
                                v.comments.map(|c| format!(" · {} comments", fmt_num(c))).unwrap_or_default(),
                                when::when(v.published_at),
                                if v.published_exact { "" } else { " (estimated)" }
                            )))
                            .child(yt::caption({
                                let age_h = ((hub::now() - v.published_at).max(3600) as f64) / 3600.;
                                let mut parts = vec![format!("{} long", if v.duration_secs > 0 { yt::duration(v.duration_secs) } else { "length not read yet".into() }), format!("{} views a day since it went up", fmt_num((v.views as f64 / (age_h / 24.).max(1. / 24.)) as u64)), format!("{:.0} views an hour right now", competitors::velocity(v, hub::now()))];
                                if let (Some(l), true) = (v.likes, v.views > 0) {
                                    parts.push(format!("{:.1}% of viewers liked it", l as f64 * 100. / v.views as f64));
                                }
                                if !v.category.is_empty() {
                                    parts.push(v.category.clone());
                                }
                                parts.push(format!("first seen {} · last checked {}", when::when(v.first_seen), hub::fmt_ago(v.last_seen.max(1))));
                                parts.join(" · ")
                            }))
                            .when(!v.description.trim().is_empty(), |d| d.child(yt::para(12., yt::TEXT_2, 5, v.description.trim().to_string())))
                            .child(div().flex().flex_wrap().gap(px(6.)).children(v.tags.iter().take(20).map(|t| tag_chip(t.clone(), None))))
                            .child(div().child(yt::pill_icon(SharedString::from(format!("rv-watch-{}", v.id)), ph::PLAY, "Watch").on_click(move |_, _, cx| cx.open_url(&url)))),
                    ),
            )
            .child(div().flex().flex_col().gap(px(8.)).child(yt::heading("Views gained, hour by hour", Some(if gains.is_empty() { "Builds up with each hourly research pass".into() } else { format!("Last {} readings", values.len()) }))).when(!values.is_empty(), |d| d.child(yt::bar_chart("rv-gains", &values, &labels, yt::GREEN, 80.))))
            .when(v.titles.len() > 1, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(yt::heading(format!("Title changed {} time{}", v.titles.len() - 1, if v.titles.len() == 2 { "" } else { "s" }), None))
                        .children(v.titles.iter().rev().enumerate().map(|(i, t)| {
                            div().flex().gap(px(12.)).min_w_0().child(yt::text(12., 18., FontWeight::NORMAL, yt::TEXT_2).w(px(150.)).flex_none().whitespace_nowrap().child(when::when(t.at))).child(yt::para(13., if i == 0 { yt::TEXT } else { yt::TEXT_2 }, 2, t.value.clone()).flex_1())
                        })),
                )
            })
            .when(!v.thumbnails.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(yt::heading(if v.thumbnails.len() > 1 { format!("Thumbnail changed {} time{}", v.thumbnails.len() - 1, if v.thumbnails.len() == 2 { "" } else { "s" }) } else { "Thumbnail (unchanged so far)".to_string() }, None))
                        .child(div().flex().flex_wrap().gap(px(12.)).children(v.thumbnails.iter().rev().map(|t| {
                            let src = if t.file.is_empty() { t.url.clone() } else { store.thumb_path(t).to_string_lossy().into_owned() };
                            div().flex().flex_col().gap(px(4.)).child(yt::thumb(&src, 160., None)).child(yt::caption(when::when(t.at)))
                        }))),
                )
            })
            .when(v.descriptions.len() > 1, |d| d.child(yt::caption(format!("Description edited {} time{} (last {})", v.descriptions.len() - 1, if v.descriptions.len() == 2 { "" } else { "s" }, v.descriptions.last().map(|c| when::when(c.at)).unwrap_or_default()))))
    }
}

fn tag_chip(label: String, n: Option<u32>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .h(px(24.))
        .px(px(10.))
        .rounded(px(12.))
        .bg(yt::tone(0.1))
        .max_w(px(220.))
        .child(yt::line(12., FontWeight::NORMAL, yt::TEXT, label))
        .children(n.map(|n| yt::text(11., 14., FontWeight::NORMAL, yt::TEXT_2).flex_none().child(format!("{n}"))))
}
