//! Dashboard (Studio's three card columns, then every recap Studio made with where it stands on
//! YouTube), Analytics, Insights and Settings.
use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::{external_link, inline_alert, ph, segmented, setting_row, switch, AlertKind};
use mv_youtube::{Action, Severity, Suggestion, Trend, UploadState};

use crate::chart::trend_chip;
use crate::hub::{self, fmt_num, fmt_when, Hub};
use crate::page::{yt_card as card, yt_card_title as card_title, yt_hint as hint, Focus, Tab, YouTubePage};
use crate::when;
use crate::yt;

/// One recap Studio made, joined with its upload and its video on YouTube.
pub(crate) struct RecapRow {
    pub title: String,
    pub sub: String,
    pub thumb: String,
    pub status: (String, u32),
    pub when: Option<i64>,
    pub video_id: Option<String>,
    pub item_id: Option<String>,
    pub chapter_id: String,
    pub views: Option<u64>,
    pub comments: Option<u64>,
    pub likes: Option<u64>,
    pub duration: Option<u64>,
}

/// Every finished recap (newest uploads first, then what is not uploaded yet).
pub(crate) fn recap_rows(h: &Hub) -> Vec<RecapRow> {
    let mut rows: Vec<RecapRow> = h
        .episodes
        .iter()
        .map(|e| {
            let item = h.queue.iter().filter(|i| i.chapter_id.as_deref() == Some(e.chapter_id.as_str())).max_by_key(|i| i.created_at);
            let video = item.and_then(|i| i.video_id.as_deref()).and_then(|id| h.video(id));
            let status = match (item.map(|i| i.state), video) {
                (_, Some(v)) if v.is_scheduled() => ("Scheduled".to_string(), yt::LINK),
                (_, Some(v)) if v.privacy == "public" => ("Published".to_string(), yt::GREEN),
                (_, Some(v)) => (capitalize(&v.privacy), yt::TEXT_2),
                (Some(UploadState::Uploading), _) => (format!("Uploading {:.0}%", item.map(|i| i.progress * 100.).unwrap_or(0.)), yt::LINK),
                (Some(UploadState::Scheduled), _) => ("Scheduled".to_string(), yt::LINK),
                (Some(UploadState::Published), _) => ("Published".to_string(), yt::GREEN),
                (Some(UploadState::Failed), _) => ("Upload failed".to_string(), yt::ERROR),
                (Some(UploadState::Planned), _) => ("Queued".to_string(), yt::AMBER),
                (None, _) => ("Rendered".to_string(), yt::TEXT_2),
            };
            let thumb = video.map(|v| v.thumbnail.clone()).filter(|t| !t.is_empty()).or_else(|| h.local_thumbs.get(&e.chapter_id).cloned()).or_else(|| h.covers.get(&e.manga_id).map(|p| p.to_string_lossy().into_owned())).unwrap_or_default();
            RecapRow {
                title: video.map(|v| v.title.clone()).or_else(|| item.map(|i| i.title.clone())).unwrap_or_else(|| format!("{} · {}", e.manga_title, e.label)),
                sub: format!("{} · {}", e.manga_title, e.label),
                thumb,
                status,
                when: video.and_then(|v| v.publish_at.or(Some(v.published_at))).or_else(|| item.and_then(|i| i.publish_at)),
                video_id: video.map(|v| v.id.clone()).or_else(|| item.and_then(|i| i.video_id.clone())),
                item_id: item.map(|i| i.id.clone()),
                chapter_id: e.chapter_id.clone(),
                views: video.map(|v| v.views),
                comments: video.map(|v| v.comments),
                likes: video.map(|v| v.likes),
                duration: video.map(|v| v.duration_secs).filter(|d| *d > 0),
            }
        })
        .collect();
    rows.sort_by_key(|r| (r.video_id.is_none(), std::cmp::Reverse(r.when.unwrap_or(0))));
    rows
}

/// A dashboard column: Studio's cards are 394 px, narrower windows wrap them.
fn column() -> Div {
    div().flex().flex_col().gap(px(yt::GAP)).flex_1().min_w(px(320.)).max_w(px(yt::CARD_W + 40.))
}

impl YouTubePage {
    // ------------------------------------------------------------ dashboard

    pub(crate) fn dashboard(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let (auto_on, job) = {
            let r = crate::automation::runner(cx).read(cx);
            (r.state.full_auto, r.state.jobs.iter().rev().find(|j| j.active()).cloned())
        };
        let h = self.hub.read(cx);
        let days = 28;
        let views: Vec<f64> = h.channel_daily.iter().map(|d| d.views as f64).collect();
        let minutes: Vec<f64> = h.channel_daily.iter().map(|d| d.watch_minutes).collect();
        let subs: Vec<f64> = h.channel_daily.iter().map(|d| d.subs_gained as f64).collect();
        let last = |v: &[f64]| v.iter().rev().take(days).sum::<f64>();

        // Column 1: latest video performance, published videos.
        let latest = h.videos.iter().find(|v| v.privacy == "public").cloned();
        let latest_card = card().child(yt::card_title("Latest video performance")).child(match latest {
            Some(v) => {
                let trend = h.trend_of(&v.id);
                let daily: Vec<f64> = h.video_daily.get(&v.id).map(|d| d.iter().map(|s| s.views as f64).collect()).unwrap_or_default();
                let id = v.id.clone();
                let id2 = v.id.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .child(yt::thumb(&v.thumbnail, 344., Some(yt::duration(v.duration_secs))).w_full())
                            .child(div().absolute().left_0().right_0().bottom_0().p(px(12.)).bg(yt::black(0.55)).rounded_b(px(12.)).child(yt::para(15., yt::TEXT, 2, v.title.clone()).text_color(rgb(0xffffff)))),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(16.))
                            .child(yt::stat(ph::CHART_BAR, fmt_num(v.views)))
                            .child(yt::stat(ph::CHAT_CIRCLE, fmt_num(v.comments)))
                            .child(yt::stat(ph::THUMBS_UP, fmt_num(v.likes)))
                            .child(div().flex_1())
                            .child(trend_chip(trend)),
                    )
                    .child(yt::caption(format!("Published {}", when::when(v.published_at))))
                    .when(daily.len() > 1, |d| {
                        let labels: Vec<String> = (0..daily.len()).map(|i| format!("Day {}", i + 1)).collect();
                        d.child(yt::area_chart("dash-latest", &daily, &labels, yt::LINK, 64.))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(8.))
                            .child(yt::pill_icon("dash-edit-latest", ph::SPARKLE, "Improve with AI").on_click(cx.listener(move |this, _, w, cx| {
                                this.tab = Tab::Content;
                                this.open_editor(Focus::Video(id.clone()), w, cx);
                            })))
                            .child(yt::pill_icon("dash-latest-comments", ph::CHAT_CIRCLE, "See comments").on_click(cx.listener(move |this, _, _, cx| {
                                this.comments.video = Some(id2.clone());
                                this.set_tab(Tab::Community, cx);
                            }))),
                    )
                    .into_any_element()
            }
            None => hint("Your newest public video shows up here with its first days.").into_any_element(),
        });

        let published: Vec<_> = h.videos.iter().take(5).cloned().collect();
        let published_card = card()
            .child(yt::card_title("Published videos"))
            .when(published.is_empty(), |d| d.child(hint("No videos on the channel yet.")))
            .children(published.into_iter().enumerate().map(|(i, v)| {
                let id = v.id.clone();
                div()
                    .id(("dash-pub", i))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .min_w_0()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.tab = Tab::Content;
                        this.open_editor(Focus::Video(id.clone()), w, cx);
                    }))
                    .child(yt::thumb(&v.thumbnail, 64., None))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(yt::line(14., FontWeight::NORMAL, yt::TEXT, v.title.clone()))
                            .child(div().flex().gap(px(16.)).child(yt::stat(ph::CHART_BAR, fmt_num(v.views))).child(yt::stat(ph::CHAT_CIRCLE, fmt_num(v.comments))).child(yt::stat(ph::THUMBS_UP, fmt_num(v.likes)))),
                    )
            }))
            .child(div().child(yt::pill("dash-go-videos", "Go to videos").on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Content, cx)))));

        // Column 2: channel analytics, recent comments.
        let subscribers = h.channel.as_ref().map(|c| c.subscribers.to_string()).unwrap_or_else(|| "—".into());
        let gained = last(&subs) as i64;
        let top48: Vec<(String, u64)> = {
            let mut v: Vec<(String, u64)> = h.video_daily.iter().filter_map(|(id, d)| Some((h.video(id)?.title.clone(), d.iter().rev().take(2).map(|s| s.views).sum::<u64>()))).collect();
            v.sort_by_key(|x| std::cmp::Reverse(x.1));
            v.truncate(3);
            v
        };
        let analytics_card = card()
            .child(yt::card_title("Channel analytics"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(yt::text(14., 20., FontWeight::NORMAL, yt::TEXT).child("Current subscribers"))
                    .child(yt::text(32., 44., FontWeight::BOLD, yt::TEXT).child(subscribers))
                    .child(div().flex().gap(px(4.)).child(yt::text(12., 18., FontWeight::NORMAL, if gained >= 0 { yt::GREEN } else { yt::ERROR }).child(format!("{gained:+}"))).child(yt::caption(format!("in last {days} days")))),
            )
            .child(yt::divider())
            .child(yt::heading("Summary", Some(format!("Last {days} days"))))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(metric_with_delta("Views", fmt_num(last(&views) as u64), crate::chart::change(&views, days)))
                    .child(metric_with_delta("Watch time (hours)", format!("{:.1}", last(&minutes) / 60.), crate::chart::change(&minutes, days))),
            )
            .child(yt::divider())
            .child(yt::heading("Top content", Some("Last 48 hours · Views".into())))
            .when(top48.is_empty(), |d| d.child(yt::caption("No views in the last 48 hours yet")))
            .children(top48.into_iter().map(|(t, n)| yt::metric_row(t, fmt_num(n))))
            .child(div().child(yt::pill("dash-go-analytics", "Go to channel analytics").on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Analytics, cx)))));

        let recent: Vec<_> = h.threads.iter().filter(|t| !t.top.mine).take(3).cloned().collect();
        let unanswered = h.comments_unanswered();
        let comments_card = card()
            .child(div().flex().items_center().gap(px(8.)).child(yt::card_title("Recent comments").flex_1()).when(unanswered > 0, |d| d.child(yt::badge(format!("{unanswered} to answer"), 0xffffff, yt::c(yt::RED).into()))))
            .child(yt::caption("Channel comments"))
            .when(recent.is_empty(), |d| d.child(hint(if h.account.can_write_comments() { "No comments yet." } else { "Open Community to read and answer comments." })))
            .children(recent.into_iter().map(|t| {
                div()
                    .flex()
                    .gap(px(12.))
                    .min_w_0()
                    .child(yt::avatar(&t.top.author_avatar, 32.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(div().flex().gap(px(6.)).min_w_0().child(yt::line(12., FontWeight::MEDIUM, yt::TEXT, t.top.author.clone())).child(yt::text(12., 18., FontWeight::NORMAL, yt::TEXT_2).flex_none().child(format!("· {}", hub::fmt_ago(t.top.published_at)))))
                            .child(yt::para(14., yt::TEXT, 2, t.top.text.clone())),
                    )
                    .child(yt::thumb(&h.video(&t.video_id).map(|v| v.thumbnail.clone()).unwrap_or_default(), 64., None))
            }))
            .child(div().child(yt::pill("dash-go-comments", "View more").on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Community, cx)))));

        // Column 3: automation, up next, what to do next.
        let auto_card = card()
            .child(div().flex().items_center().gap(px(8.)).child(yt::card_title("Automation").flex_1()).child(if auto_on { yt::status("Full auto on", yt::GREEN) } else { yt::status("Manual", yt::TEXT_2) }))
            .child(match &job {
                Some(j) => div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .min_w_0()
                    .child(yt::line(14., FontWeight::MEDIUM, yt::TEXT, j.pick.title.clone()))
                    .child(yt::caption(format!("{} · {}", j.stage.label(), j.step)))
                    .child(div().h(px(4.)).w_full().rounded(px(2.)).bg(yt::tone(0.1)).child(div().h_full().rounded(px(2.)).w(relative(j.progress())).bg(yt::c(yt::RED))))
                    .into_any_element(),
                None => hint("Nothing in production. Discover finds the manga viewers want next.").into_any_element(),
            })
            .child(div().child(yt::pill("dash-go-auto", "Open automation").on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Automation, cx)))));

        let upcoming: Vec<_> = h.upcoming().into_iter().take(5).cloned().collect();
        let schedule_card = card()
            .child(yt::card_title("Up next"))
            .when(upcoming.is_empty(), |d| d.child(hint("Nothing scheduled. Queue finished recaps in Schedule and they go out in chapter order.")))
            .children(upcoming.into_iter().map(|i| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .min_w_0()
                    .child(yt::icon_of(if i.state == UploadState::Scheduled { ph::CALENDAR_CHECK } else { ph::HOURGLASS }, 18., yt::TEXT_2))
                    .child(yt::line(14., FontWeight::NORMAL, yt::TEXT, i.title.clone()).flex_1())
                    .child(yt::text(12., 18., FontWeight::NORMAL, yt::TEXT_2).flex_none().whitespace_nowrap().child(i.publish_at.map(fmt_when).unwrap_or_default()))
            }))
            .child(div().child(yt::pill("dash-schedule", "Open schedule").on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Schedule, cx)))));

        let tips: Vec<Suggestion> = h.suggestions().into_iter().take(3).collect();
        let tips_card = card()
            .child(yt::card_title("What to do next"))
            .when(tips.is_empty(), |d| d.child(hint("No alerts. Your releases are on track.")))
            .children(tips.into_iter().enumerate().map(|(i, s)| self.suggestion_row(("dash-tip", i), s, cx)));

        let rows = recap_rows(self.hub.read(cx));
        div()
            .flex()
            .flex_col()
            .gap(px(yt::GAP))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_start()
                    .gap(px(yt::GAP))
                    .child(column().child(latest_card).child(published_card))
                    .child(column().child(analytics_card).child(comments_card))
                    .child(column().child(auto_card).child(schedule_card).child(tips_card)),
            )
            .child(self.recaps_table(rows, cx))
            .into_any_element()
    }

    /// Every recap Studio made and where it stands on YouTube.
    fn recaps_table(&self, rows: Vec<RecapRow>, cx: &mut Context<Self>) -> Div {
        let header = div()
            .flex()
            .items_center()
            .gap(px(16.))
            .px(px(8.))
            .pb(px(8.))
            .border_b_1()
            .border_color(yt::tone(0.1))
            .child(div().flex_1().min_w_0().child(yt::text(12., 18., FontWeight::MEDIUM, yt::TEXT_2).child("Recap")))
            .child(yt::th(150., false, "Status"))
            .child(yt::th(150., false, "Date"))
            .child(yt::th(70., true, "Views"))
            .child(yt::th(80., true, "Comments"))
            .child(yt::th(60., true, "Likes"))
            .child(yt::col(40., true));
        card()
            .gap(px(8.))
            .child(div().flex().items_center().gap(px(12.)).child(yt::card_title("Studio recaps").flex_1()).child(yt::caption(format!("{} made in Studio", rows.len()))))
            .child(hint("Everything Studio rendered, each linked to its video on YouTube: rendered, queued, scheduled or published, with its numbers."))
            .child(header)
            .when(rows.is_empty(), |d| d.child(div().py(px(24.)).child(hint("No finished recaps yet. Make one in Studio or let Automation make one."))))
            .children(rows.into_iter().take(60).enumerate().map(|(i, r)| {
                let (vid, item, chapter) = (r.video_id.clone(), r.item_id.clone(), r.chapter_id.clone());
                div()
                    .id(("recap-row", i))
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .px(px(8.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .hover(|s| s.bg(yt::tone(0.05)))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, w, cx| {
                        if let Some(id) = vid.clone().filter(|id| this.hub.read(cx).video(id).is_some()) {
                            this.tab = Tab::Content;
                            this.open_editor(Focus::Video(id), w, cx);
                        } else if let Some(id) = item.clone() {
                            this.tab = Tab::Schedule;
                            this.open_editor(Focus::Item(id), w, cx);
                        } else {
                            let ep = this.hub.read(cx).episodes.iter().find(|e| e.chapter_id == chapter).cloned();
                            if let Some(ep) = ep {
                                let id = this.hub.update(cx, |h, cx| h.queue_episode(&ep, cx));
                                if let Some(id) = id {
                                    this.tab = Tab::Schedule;
                                    this.open_editor(Focus::Item(id), w, cx);
                                }
                            }
                        }
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(px(12.))
                            .child(yt::thumb(&r.thumb, 96., r.duration.map(yt::duration)))
                            .child(div().flex().flex_col().flex_1().min_w_0().gap(px(2.)).child(yt::line(14., FontWeight::NORMAL, yt::TEXT, r.title.clone())).child(yt::line(12., FontWeight::NORMAL, yt::TEXT_2, r.sub.clone()))),
                    )
                    .child(yt::col(150., false).child(yt::status(r.status.0.clone(), r.status.1)))
                    .child(yt::col(150., false).child(yt::line(13., FontWeight::NORMAL, yt::TEXT_2, r.when.map(when::when).unwrap_or_else(|| "—".into()))))
                    .child(yt::col(70., true).child(yt::text(14., 20., FontWeight::NORMAL, yt::TEXT).child(r.views.map(fmt_num).unwrap_or_else(|| "—".into()))))
                    .child(yt::col(80., true).child(yt::text(14., 20., FontWeight::NORMAL, yt::TEXT).child(r.comments.map(fmt_num).unwrap_or_else(|| "—".into()))))
                    .child(yt::col(60., true).child(yt::text(14., 20., FontWeight::NORMAL, yt::TEXT).child(r.likes.map(fmt_num).unwrap_or_else(|| "—".into()))))
                    .child(yt::col(40., true).children(r.video_id.clone().map(|id| {
                        div().id(("recap-open", i)).cursor_pointer().tooltip(mv_ui::tip("Watch on YouTube")).on_click(move |_, _, cx| cx.open_url(&format!("https://youtu.be/{id}"))).child(yt::icon_of(ph::ARROW_SQUARE_OUT, 18., yt::TEXT_2))
                    })))
            }))
    }

    // ------------------------------------------------------------ analytics

    pub(crate) fn analytics(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let h = self.hub.read(cx);
        let days = self.range;
        let rows: Vec<_> = h.channel_daily.iter().rev().take(days).rev().cloned().collect();
        let labels: Vec<String> = rows.iter().filter_map(|d| chrono::NaiveDate::parse_from_str(&d.date, "%Y-%m-%d").ok()).map(|d| d.format("%b %-d").to_string()).collect();
        let series = |f: fn(&mv_youtube::DayStat) -> f64| rows.iter().map(f).collect::<Vec<f64>>();
        let all_views: Vec<f64> = h.channel_daily.iter().map(|d| d.views as f64).collect();
        let all_hours: Vec<f64> = h.channel_daily.iter().map(|d| d.watch_minutes / 60.).collect();
        let all_subs: Vec<f64> = h.channel_daily.iter().map(|d| d.subs_gained as f64).collect();
        let total = |v: &[f64]| v.iter().sum::<f64>();
        let views = series(|d| d.views as f64);
        let hours = series(|d| d.watch_minutes / 60.);
        let subs = series(|d| d.subs_gained as f64);
        let avg = series(|d| d.avg_view_secs);
        let avg_view = if avg.is_empty() { 0. } else { avg.iter().sum::<f64>() / avg.iter().filter(|v| **v > 0.).count().max(1) as f64 };
        let range_chips = div().flex().gap(px(8.)).children([(7usize, "Last 7 days"), (28, "Last 28 days"), (90, "Last 90 days")].into_iter().map(|(d, label)| {
            yt::chip(("yt-range", d), label, days == d).on_click(cx.listener(move |this, _, _, cx| {
                this.range = d;
                cx.notify();
            }))
        }));

        // Studio's metric tabs over the chart.
        let tile = |label: &'static str, value: String, change: Option<f64>| {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(2.))
                .flex_1()
                .min_w(px(140.))
                .py(px(16.))
                .px(px(8.))
                .child(yt::line(12., FontWeight::NORMAL, yt::TEXT_2, label))
                .child(yt::text(24., 32., FontWeight::BOLD, yt::TEXT).whitespace_nowrap().child(value))
                .child(match change {
                    Some(c) => yt::delta(c).into_any_element(),
                    None => yt::caption(" ").into_any_element(),
                })
        };
        let headline = format!("Your channel got {} views in the last {days} days", fmt_num(total(&views) as u64));
        let main = card()
            .p(px(0.))
            .gap(px(0.))
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .border_b_1()
                    .border_color(yt::tone(0.1))
                    .bg(yt::tone(0.05))
                    .child(tile("Views", fmt_num(total(&views) as u64), crate::chart::change(&all_views, days)))
                    .child(tile("Watch time (hours)", format!("{:.1}", total(&hours)), crate::chart::change(&all_hours, days)))
                    .child(tile("Subscribers", format!("{:+}", total(&subs) as i64), crate::chart::change(&all_subs, days)))
                    .child(tile("Average view duration", format!("{}:{:02}", avg_view as u64 / 60, avg_view as u64 % 60), None)),
            )
            .child(div().p(px(24.)).child(yt::area_chart("an-views", &views, &labels, yt::LINK, 180.)));

        // Videos ranked by views, with how each is trending.
        let mut ranked: Vec<_> = h.videos.iter().filter(|v| v.privacy != "private").cloned().collect();
        ranked.sort_by_key(|v| std::cmp::Reverse(v.views));
        let top = card().child(yt::card_title("Your top content in this period")).children(ranked.into_iter().take(10).enumerate().map(|(i, v)| {
            let trend = h.trend_of(&v.id);
            let id = v.id.clone();
            div()
                .id(("yt-top", i))
                .flex()
                .items_center()
                .gap(px(16.))
                .p(px(6.))
                .rounded(px(8.))
                .cursor_pointer()
                .hover(|s| s.bg(yt::tone(0.05)))
                .on_click(cx.listener(move |this, _, w, cx| {
                    this.tab = Tab::Content;
                    this.open_editor(Focus::Video(id.clone()), w, cx);
                }))
                .child(yt::text(14., 20., FontWeight::NORMAL, yt::TEXT_2).w(px(20.)).flex_none().child(format!("{}", i + 1)))
                .child(yt::thumb(&v.thumbnail, 96., Some(yt::duration(v.duration_secs))))
                .child(div().flex().flex_col().flex_1().min_w_0().child(yt::line(14., FontWeight::NORMAL, yt::TEXT, v.title.clone())).child(yt::caption(when::date(v.published_at))))
                .child(trend_chip(trend))
                .child(yt::col(80., true).child(yt::text(14., 20., FontWeight::MEDIUM, yt::TEXT).child(fmt_num(v.views))))
        }));

        let side = card()
            .w(px(300.))
            .flex_none()
            .child(yt::card_title("Realtime"))
            .child(div().flex().flex_col().child(yt::text(24., 32., FontWeight::BOLD, yt::TEXT).child(h.channel.as_ref().map(|c| c.subscribers.to_string()).unwrap_or_else(|| "—".into()))).child(yt::caption("Subscribers")))
            .child(yt::divider())
            .child(div().flex().flex_col().child(yt::text(24., 32., FontWeight::BOLD, yt::TEXT).child(fmt_num(all_views.iter().rev().take(2).sum::<f64>() as u64))).child(yt::caption("Views · Last 48 hours")))
            .child(yt::bar_chart("an-48h", &all_views.iter().rev().take(14).rev().copied().collect::<Vec<_>>(), &h.channel_daily.iter().rev().take(14).rev().map(|d| d.date.clone()).collect::<Vec<_>>(), yt::LINK, 48.))
            .child(yt::divider())
            .child(hint("YouTube Analytics lags about two days; the realtime counts come from the channel itself."));

        div()
            .flex()
            .flex_col()
            .gap(px(yt::GAP))
            .child(range_chips)
            .child(yt::text(20., 28., FontWeight::BOLD, yt::TEXT).text_center().child(headline))
            .child(div().flex().flex_wrap().items_start().gap(px(yt::GAP)).child(div().flex().flex_col().gap(px(yt::GAP)).flex_1().min_w(px(420.)).child(main).child(top)).child(side))
            .into_any_element()
    }

    // ------------------------------------------------------------ insights

    pub(crate) fn suggestion_row(&self, id: impl Into<ElementId>, s: Suggestion, cx: &mut Context<Self>) -> Stateful<Div> {
        let (icon, color) = match s.severity {
            Severity::Urgent => (ph::TREND_DOWN, yt::ERROR),
            Severity::Warning => (ph::WARNING, yt::AMBER),
            Severity::Info => (ph::LIGHTBULB, yt::LINK),
        };
        let action = s.action.clone().map(|a| {
            let label = match &a {
                Action::PublishSooner { at, .. } => format!("Move to {}", fmt_when(*at)),
                Action::RefreshMetadata { .. } => "Improve with AI".to_string(),
                Action::QueueNext { .. } => "Queue it".to_string(),
            };
            yt::pill(ElementId::Name(format!("sg-act-{}", s.title).into()), label).on_click(cx.listener(move |this, _, w, cx| this.run_action(a.clone(), w, cx)))
        });
        div()
            .id(id)
            .flex()
            .gap(px(12.))
            .p(px(12.))
            .min_w_0()
            .rounded(px(12.))
            .bg(yt::tone(0.05))
            .child(div().pt(px(2.)).flex_none().child(yt::icon_of(icon, 20., color)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.))
                    .child(yt::para(14., yt::TEXT, 2, s.title.clone()).font_weight(FontWeight::MEDIUM))
                    .child(yt::para(13., yt::TEXT_2, 4, s.body.clone()))
                    .children(action.map(|b| div().pt(px(4.)).child(b))),
            )
    }

    pub(crate) fn run_action(&mut self, a: Action, window: &mut Window, cx: &mut Context<Self>) {
        match a {
            Action::PublishSooner { item_id, at } => self.hub.update(cx, |h, cx| h.reschedule(&item_id, Some(at), cx)),
            Action::RefreshMetadata { video_id } => {
                self.tab = Tab::Content;
                self.open_editor(Focus::Video(video_id), window, cx);
                self.suggest(cx);
            }
            Action::QueueNext { chapter_id } => {
                let ep = self.hub.read(cx).episodes.iter().find(|e| e.chapter_id == chapter_id).cloned();
                if let Some(ep) = ep {
                    let id = self.hub.update(cx, |h, cx| h.queue_episode(&ep, cx));
                    if let Some(id) = id {
                        self.tab = Tab::Schedule;
                        self.open_editor(Focus::Item(id), window, cx);
                        self.suggest(cx);
                    }
                }
            }
        }
    }

    pub(crate) fn insights(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let h = self.hub.read(cx);
        let sugg = h.suggestions();
        let moves = h.moves.clone();
        let auto = h.settings.auto_reschedule;
        let threshold = h.settings.dip_threshold;
        // Every recent video's first days, so the dips the automation reacts to are visible.
        let watched: Vec<_> = h.videos.iter().filter(|v| h.video_daily.contains_key(&v.id)).cloned().collect();
        let rows = watched
            .into_iter()
            .map(|v| {
                let daily: Vec<u64> = h.video_daily.get(&v.id).map(|d| d.iter().map(|s| s.views).collect()).unwrap_or_default();
                let trend = h.trend_of(&v.id);
                let series = h.item_for_video(&v.id).map(|i| format!("{} · {}", i.manga_title.clone().unwrap_or_default(), i.episode_label()));
                let color = if matches!(trend, Trend::Dipping { .. }) { yt::c(yt::ERROR) } else { yt::c(yt::LINK) };
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .py(px(4.))
                    .min_w_0()
                    .child(yt::thumb(&v.thumbnail, 64., None))
                    .child(div().flex().flex_col().flex_1().min_w_0().child(yt::line(14., FontWeight::NORMAL, yt::TEXT, v.title.clone())).child(yt::caption(series.unwrap_or_else(|| format!("Published {}", hub::fmt_ago(v.published_at))))))
                    .child(crate::chart::sparkline(&daily, color))
                    .child(trend_chip(trend))
            })
            .collect::<Vec<_>>();
        div()
            .flex()
            .flex_col()
            .gap(px(yt::GAP))
            .child(
                card()
                    .child(card_title(ph::ROBOT, "Smart upload"))
                    .child(hint(format!(
                        "Watches each new video for its first five days. When one loses {:.0}% of its best day's views, the next chapter of the same series is pulled forward to the next free slot, so viewers stay in the story. Chapters always go out in order.",
                        threshold * 100.
                    )))
                    .child(switch("yt-auto", auto).label("Move the next episode forward automatically").on_click(cx.listener(|this, on: &bool, _, cx| {
                        let mut s = this.hub.read(cx).settings.clone();
                        s.auto_reschedule = *on;
                        this.hub.update(cx, |h, cx| h.save_settings(s, cx));
                    }))),
            )
            .child(card().child(card_title(ph::LIGHTBULB, "Suggestions")).when(sugg.is_empty(), |d| d.child(hint("Nothing needs attention. New videos are checked every few hours."))).children(sugg.into_iter().enumerate().map(|(i, s)| self.suggestion_row(("ins", i), s, cx))))
            .child(card().child(card_title(ph::PULSE, "New videos, first days")).when(rows.is_empty(), |d| d.child(hint("Videos from the last three weeks show their daily views here."))).children(rows))
            .when(!moves.is_empty(), |d| d.child(card().child(card_title(ph::CLOCK_COUNTER_CLOCKWISE, "What the automation changed")).children(moves.into_iter().map(hint))))
            .into_any_element()
    }

    // ------------------------------------------------------------ settings

    pub(crate) fn settings(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let h = self.hub.read(cx);
        let connected = h.account.connected();
        let connecting = h.connecting;
        let channel = h.channel.clone();
        let s = h.settings.clone();
        let creds_set = h.account.credentials.is_set();
        let can_reply = h.account.can_write_comments();
        let quota = h.yt.quota();

        let connect_btn = |id: &'static str, label: &'static str, cx: &mut Context<Self>| {
            yt::pill_primary(id, if connecting { "Waiting for the browser…" } else { label }).on_click(cx.listener(|this, _, _, cx| {
                let id = this.client_id.read(cx).value().to_string();
                let secret = this.client_secret.read(cx).value().to_string();
                if id.trim().is_empty() || secret.trim().is_empty() {
                    YouTubePage::toast_err(cx, "Paste the client ID and secret first");
                    return;
                }
                let yt = this.hub.read(cx).yt.clone();
                let hub = this.hub.clone();
                // Save the client first, then sign in with it.
                cx.spawn(async move |_, cx| {
                    let _ = mv_ui::bg::run(async move { yt.set_credentials(mv_youtube::Credentials { client_id: id.trim().into(), client_secret: secret.trim().into() }).await }).await;
                    hub.update(cx, |h, cx| {
                        h.account = h.yt.store().account();
                        h.connect(cx);
                    });
                })
                .detach();
            }))
        };

        let connect = card()
            .child(card_title(ph::YOUTUBE_LOGO, if connected { "Connected channel" } else { "Connect your channel" }))
            .when(!connected, |d| {
                d.child(hint("YouTube needs an OAuth client of your own (free). Once:"))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(hint("1. In Google Cloud Console create a project and enable “YouTube Data API v3” and “YouTube Analytics API”."))
                            .child(hint("2. Set up the OAuth consent screen (External; In production, or add yourself as a test user)."))
                            .child(hint("3. Credentials → Create credentials → OAuth client ID → Desktop app. Paste its ID and secret below.")),
                    )
                    .child(div().flex().flex_wrap().gap(px(16.)).child(external_link("Open Google Cloud credentials", "https://console.cloud.google.com/apis/credentials")).child(external_link("Enable YouTube Data API", "https://console.cloud.google.com/apis/library/youtube.googleapis.com")))
                    .child(setting_row("Client ID", "Ends in .apps.googleusercontent.com", div().w(px(360.)).child(Input::new(&self.client_id))))
                    .child(setting_row("Client secret", "Stays on this PC, in your MangaVox data folder", div().w(px(360.)).child(Input::new(&self.client_secret))))
                    .child(div().flex().child(connect_btn("yt-connect", "Connect YouTube", cx)))
                    .when(creds_set && !connecting, |d| d.child(hint("A browser tab opens for Google sign-in. Pick the channel's account and allow every permission asked (uploads, analytics and comments).")))
            })
            .when_some(channel.clone().filter(|_| connected), |d, c| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(16.))
                        .child(yt::avatar(&c.thumbnail, 48.))
                        .child(div().flex().flex_col().flex_1().min_w(px(200.)).child(yt::line(16., FontWeight::MEDIUM, yt::TEXT, c.title.clone())).child(yt::caption(format!("{} · {} subscribers · {} videos", c.handle, fmt_num(c.subscribers), fmt_num(c.video_count)))))
                        .child(external_link("Open in YouTube Studio", format!("https://studio.youtube.com/channel/{}", c.id)))
                        .child(yt::pill_icon("yt-disconnect", ph::SIGN_OUT, "Disconnect").on_click(cx.listener(|this, _, _, cx| this.hub.update(cx, |h, cx| h.disconnect(cx))))),
                )
            })
            .when(connected && !can_reply, |d| {
                d.child(inline_alert(AlertKind::Warning, "Connect again to answer comments", "MangaVox can now reply to comments from the Community tab. That needs one more permission from Google (manage your YouTube comments), so connect your channel once more and allow it. Your queue and settings stay.").build())
                    .child(div().flex().child(connect_btn("yt-reconnect", "Connect again", cx)))
            });

        let hub = self.hub.clone();
        let save = |f: fn(&mut mv_youtube::Settings, usize), _: &mut Context<Self>| {
            let hub = hub.clone();
            move |ix: &usize, _: &mut Window, cx: &mut App| {
                let mut s = hub.read(cx).settings.clone();
                f(&mut s, *ix);
                hub.update(cx, |h, cx| h.save_settings(s, cx));
            }
        };
        const CADENCE: [u32; 4] = [12, 24, 48, 72];
        const HOURS: [u8; 6] = [9, 12, 15, 18, 20, 22];
        const DIPS: [f32; 3] = [0.25, 0.4, 0.6];
        let plan = card()
            .child(card_title(ph::CALENDAR_DOTS, "Release plan"))
            .child(setting_row(
                "How often",
                "Time between two releases of the queue",
                segmented("yt-cadence", ["Twice a day", "Daily", "Every 2 days", "Every 3 days"], CADENCE.iter().position(|c| *c == s.cadence_hours).unwrap_or(1)).small().on_change(save(|s, i| s.cadence_hours = CADENCE[i], cx)),
            ))
            .child(setting_row(
                "Release time",
                // A golden time from Schedule can be any hour; none of the presets is lit then.
                if HOURS.contains(&s.publish_hour) { "Local time videos go public. Schedule › Golden upload time picks the best one".to_string() } else { format!("{} your time, the golden time picked in Schedule", when::hour(s.publish_hour as usize)) },
                segmented("yt-hour", ["9 AM", "12 PM", "3 PM", "6 PM", "8 PM", "10 PM"], HOURS.iter().position(|h| *h == s.publish_hour).unwrap_or(HOURS.len())).small().on_change(save(|s, i| s.publish_hour = HOURS[i], cx)),
            ))
            .child(setting_row(
                "Dip sensitivity",
                "Drop from a video's best day that counts as a dip",
                segmented("yt-dip", ["25%", "40%", "60%"], DIPS.iter().position(|d| (*d - s.dip_threshold).abs() < 0.01).unwrap_or(1)).small().on_change(save(|s, i| s.dip_threshold = DIPS[i], cx)),
            ))
            .child(setting_row(
                "Visibility",
                "Your own uploads stay private until you pick Unlisted or Public (Automation's always go public at their release time)",
                segmented("yt-privacy", ["Private", "Unlisted", "Public"], match s.privacy.as_str() {
                    "unlisted" => 1,
                    "public" => 2,
                    _ => 0,
                })
                .small()
                .on_change(save(|s, i| s.privacy = ["private", "unlisted", "public"][i.min(2)].into(), cx)),
            ))
            .child(setting_row(
                "Category",
                "YouTube category for new uploads",
                segmented("yt-cat", ["Film & Animation", "Entertainment"], if s.category_id == "24" { 1 } else { 0 }).small().on_change(save(|s, i| s.category_id = if i == 1 { "24".into() } else { "1".into() }, cx)),
            ))
            .child(setting_row(
                "Series playlists",
                "Add each episode to a “<manga> Recap” playlist so viewers watch in order",
                switch("yt-pl", s.series_playlists).on_click(cx.listener(|this, on: &bool, _, cx| {
                    let mut s = this.hub.read(cx).settings.clone();
                    s.series_playlists = *on;
                    this.hub.update(cx, |h, cx| h.save_settings(s, cx));
                })),
            ))
            .child(setting_row(
                "Made for kids",
                "Required by YouTube; recaps are usually not",
                switch("yt-kids", s.made_for_kids).on_click(cx.listener(|this, on: &bool, _, cx| {
                    let mut s = this.hub.read(cx).settings.clone();
                    s.made_for_kids = *on;
                    this.hub.update(cx, |h, cx| h.save_settings(s, cx));
                })),
            ));

        let now = hub::now();
        let quota_card = card()
            .child(card_title(ph::GAUGE, "API quota today"))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(32.))
                    .child(div().flex().flex_col().child(yt::text(24., 32., FontWeight::BOLD, yt::TEXT).child(fmt_num(quota.left(now) as u64))).child(yt::caption("units left (estimate)")))
                    .child(div().flex().flex_col().child(yt::text(24., 32., FontWeight::BOLD, yt::TEXT).child(quota.uploads_left(now).to_string())).child(yt::caption("uploads still fit")))
                    .child(div().flex().flex_col().child(yt::text(24., 32., FontWeight::BOLD, yt::TEXT).child(quota.calls.to_string())).child(yt::caption("API calls today"))),
            )
            .child(hint(format!(
                "Google gives a project 10,000 units a day; it resets at midnight Pacific ({}). An upload costs about 1,600, a reply 50, reading the channel or 100 comments 1. Analytics, competitor research and trends use no quota. Background refreshes pause when what is left would not cover an upload.",
                when::when(mv_youtube::quota::next_reset(now))
            )));

        div().flex().flex_col().gap(px(yt::GAP)).child(connect).child(quota_card).child(plan).into_any_element()
    }
}

fn metric_with_delta(label: &'static str, value: String, change: Option<f64>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .min_w_0()
        .child(yt::line(14., FontWeight::NORMAL, yt::TEXT, label).flex_1())
        .child(yt::text(14., 20., FontWeight::NORMAL, yt::TEXT).flex_none().child(value))
        .children(change.map(yt::delta))
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}
