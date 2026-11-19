//! Competitors, before a channel is picked: what is working across every tracked channel. The best
//! videos of the month, which length and which upload hours win, the words winning titles share,
//! the tags everyone uses, who is gaining subscribers, and the titles that were changed. Each video
//! is measured against its own channel's usual views (see `mv_youtube::competitors::overview`).
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::ph;
use mv_youtube::competitors::{self, Overview};

use crate::competitors_view::{tag_chip, thumb_src};
use crate::hub::fmt_num;
use crate::page::{yt_hint as hint, YouTubePage};
use crate::{when, yt};

/// "×2.4" (one decimal under 10, none above).
fn times(x: f64) -> String {
    if x >= 10. {
        format!("×{x:.0}")
    } else {
        format!("×{x:.1}")
    }
}

/// A section: an icon, its heading and the line that says how to read it.
fn section(icon: mv_ui::Ph, title: &'static str, sub: String) -> Div {
    div().flex().flex_col().gap(px(12.)).min_w_0().child(
        div()
            .flex()
            .items_start()
            .gap(px(10.))
            .min_w_0()
            .child(div().flex_none().mt(px(2.)).child(yt::icon_of(icon, 18., yt::TEXT_2)))
            .child(div().flex().flex_col().flex_1().min_w_0().child(yt::text(16., 22., FontWeight::MEDIUM, yt::TEXT).child(title)).child(hint(sub))),
    )
}

impl YouTubePage {
    pub(crate) fn rivals_overview(&mut self, o: &Overview, store: &competitors::CompStore, cx: &mut Context<Self>) -> Div {
        let header = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .min_w_0()
            .child(yt::line(20., FontWeight::BOLD, yt::TEXT, format!("What is working across {} channels", fmt_num(o.channels as u64))))
            .child(hint(format!(
                "From {} videos ({} read in full so far; the rest are read as the hourly research gets to them). A video is judged against its own channel's usual views, so a small channel's hit counts as much as a big channel's. Pick a channel on the left for its own numbers.",
                fmt_num(o.videos as u64),
                fmt_num(o.read_in_full as u64)
            )));

        // The best videos of the last 30 days.
        let best = section(ph::FIRE, "Best videos of the last 30 days", "Most views a day since they went up. Click one for its history.".into()).child(div().flex().flex_wrap().gap(px(16.)).children(o.top.iter().enumerate().map(|(i, t)| {
            let (ch, vid) = (t.video.channel_id.clone(), t.video.id.clone());
            let v = &t.video;
            div()
                .id(("ro-top", i))
                .flex()
                .flex_col()
                .gap(px(6.))
                .w(px(232.))
                .p(px(8.))
                .rounded(px(12.))
                .cursor_pointer()
                .hover(|s| s.bg(yt::tone(0.06)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.rivals.channel = Some(ch.clone());
                    this.rivals.video = Some(vid.clone());
                    cx.notify();
                }))
                .child(yt::thumb(&thumb_src(store, v), 216., Some(yt::duration(v.duration_secs)).filter(|_| v.duration_secs > 0)))
                .child(yt::para(13., yt::TEXT, 2, v.title.clone()).font_weight(FontWeight::MEDIUM))
                .child(yt::line(12., FontWeight::NORMAL, yt::TEXT_2, t.channel.clone()))
                .child(yt::line(12., FontWeight::NORMAL, yt::TEXT_2, format!("{} views · {} a day · {}", fmt_num(v.views), fmt_num(t.views_per_day as u64), when::when(v.published_at))))
                .children(t.lift.filter(|l| *l >= 1.5).map(|l| div().flex().child(yt::badge(format!("{} its channel's usual", times(l)), 0x0f0f0f, yt::c(if l >= 2. { yt::GREEN } else { yt::AMBER }).into()))))
        })));

        // Which length works: one row a band, the bar is how it does against the channel's usual.
        let trusted = |b: &competitors::overview::LengthBand| b.videos >= 8;
        let best_band = o.lengths.iter().filter(|b| trusted(b)).max_by(|a, b| a.hit_rate.total_cmp(&b.hit_rate)).filter(|b| b.hit_rate > 0.).map(|b| b.label);
        let scale = o.lengths.iter().map(|b| b.hit_rate).fold(0.01f64, f64::max);
        let lengths = section(ph::TIMER, "Which length works", "How many videos of each length became hits: at least twice their channel's usual views.".into()).child(div().flex().flex_col().gap(px(8.)).children(o.lengths.iter().map(|b| {
            let on = best_band == Some(b.label);
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .min_w_0()
                .child(yt::text(13., 18., if on { FontWeight::MEDIUM } else { FontWeight::NORMAL }, if on { yt::TEXT } else { yt::TEXT_2 }).w(px(96.)).flex_none().whitespace_nowrap().child(b.label))
                .child(div().flex_1().min_w(px(60.)).h(px(10.)).rounded(px(5.)).bg(yt::tone(0.08)).child(div().h_full().rounded(px(5.)).w(relative((b.hit_rate / scale).clamp(0.02, 1.) as f32)).bg(yt::c(if on { yt::GREEN } else { yt::LINK })).opacity(if trusted(b) { 1. } else { 0.4 })))
                .child(yt::text(13., 18., FontWeight::MEDIUM, if on { yt::GREEN } else { yt::TEXT }).w(px(44.)).flex_none().whitespace_nowrap().child(format!("{:.0}%", b.hit_rate * 100.)))
                .child(yt::text(12., 18., FontWeight::NORMAL, yt::TEXT_3).w(px(170.)).flex_none().whitespace_nowrap().child(format!("{} videos · typical {}", fmt_num(b.videos as u64), times(b.lift))))
        })))
        .when(o.lengths.is_empty(), |d| d.child(yt::caption("Not enough videos with a known length yet.")))
        .when(o.lengths.iter().any(|b| !trusted(b)), |d| d.child(yt::caption("Faded bars have fewer than 8 videos: too few to trust.")));

        // When they upload.
        let hours: Vec<f64> = o.upload_hours.iter().map(|n| *n as f64).collect();
        let hour_labels: Vec<String> = (0..24).map(|h| format!("{} · uploads", when::hour(h))).collect();
        const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        let days: Vec<f64> = o.upload_days.iter().map(|n| *n as f64).collect();
        let peak = |a: &[u32]| a.iter().enumerate().max_by_key(|(_, n)| **n).filter(|(_, n)| **n > 0).map(|(i, _)| i);
        let busiest = match (peak(&o.upload_hours), peak(&o.upload_days)) {
            (Some(h), Some(d)) => format!("Busiest: {} around {}. Your time ({}), from the {} uploads whose exact time has been read.", DAYS[d], when::hour(h), when::zone(), fmt_num(o.timed as u64)),
            _ => "Exact upload times are read a few videos at a time by the hourly research.".into(),
        };
        let timing = section(ph::CLOCK, "When they upload", busiest).when(o.timed > 0, |d| {
            d.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(24.))
                    .child(div().flex_1().min_w(px(260.)).child(yt::bar_chart("ro-hours", &hours, &hour_labels, yt::PURPLE, 64.)))
                    .child(div().w(px(200.)).child(yt::bar_chart("ro-days", &days, &DAYS.iter().map(|d| d.to_string()).collect::<Vec<_>>(), yt::PURPLE, 64.))),
            )
        });

        // Words winning titles share.
        let words_sub = if o.words.is_empty() {
            format!("Needs more winners to compare: {} so far, out of {} videos old enough to judge.", o.winners, fmt_num(o.compared as u64))
        } else {
            format!(
                "{} videos did at least twice their channel's usual. Their titles use these words more often than other titles do (×2 = twice as often). Winning titles run about {} characters; all titles, {}.",
                fmt_num(o.winners as u64),
                o.winner_title_chars,
                o.title_chars
            )
        };
        let words = section(ph::TEXT_AA, "Words in winning titles", words_sub).child(div().flex().flex_wrap().gap(px(6.)).children(o.words.iter().map(|w| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(6.))
                .h(px(28.))
                .px(px(12.))
                .rounded(px(14.))
                .bg(yt::tone(0.1))
                .child(yt::line(13., FontWeight::MEDIUM, yt::TEXT, w.word.clone()))
                .child(yt::text(12., 16., FontWeight::NORMAL, yt::GREEN).flex_none().child(times(w.lift)))
                .child(yt::text(11., 14., FontWeight::NORMAL, yt::TEXT_3).flex_none().child(format!("{} wins", w.winners)))
        })));

        let tags = section(ph::HASH, "Tags everyone uses", "The most used tags on the videos read in full, with how many videos carry each.".into())
            .child(div().flex().flex_wrap().gap(px(6.)).children(o.tags.iter().map(|(t, n)| tag_chip(t.clone(), Some(*n)))))
            .when(o.tags.is_empty(), |d| d.child(yt::caption("Tags are read from each video's own page, a few an hour.")));

        // Who is growing.
        let movers = section(ph::TREND_UP, "Gaining subscribers this week", "From the readings taken so far; YouTube rounds subscriber counts, so small gains show late.".into())
            .child(div().flex().flex_col().gap(px(2.)).children(o.movers.iter().enumerate().map(|(i, m)| {
                let id = m.channel_id.clone();
                div()
                    .id(("ro-mover", i))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .hover(|s| s.bg(yt::tone(0.05)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.rivals.channel = Some(id.clone());
                        this.rivals.video = None;
                        cx.notify();
                    }))
                    .child(yt::avatar(&m.avatar, 32.))
                    .child(yt::line(14., FontWeight::MEDIUM, yt::TEXT, m.title.clone()).flex_1())
                    .child(yt::text(13., 18., FontWeight::NORMAL, yt::TEXT_2).flex_none().child(format!("{} subscribers", fmt_num(m.subscribers))))
                    .child(yt::text(13., 18., FontWeight::MEDIUM, yt::GREEN).flex_none().w(px(72.)).text_right().child(format!("+{}", fmt_num(m.gained.max(0) as u64))))
            })))
            .when(o.movers.is_empty(), |d| d.child(yt::caption("Needs two readings of a channel a few days apart; it fills in as the research keeps running.")));

        let changes = (!o.changes.is_empty()).then(|| {
            section(ph::PENCIL_SIMPLE, "Titles changed after upload", "A channel rewrites a title when the first one is not pulling. Newest first, last 30 days.".into()).child(div().flex().flex_col().gap(px(4.)).children(o.changes.iter().enumerate().map(|(i, c)| {
                let (ch, vid) = (c.channel_id.clone(), c.video_id.clone());
                div()
                    .id(("ro-change", i))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .px(px(8.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .hover(|s| s.bg(yt::tone(0.05)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.rivals.channel = Some(ch.clone());
                        this.rivals.video = Some(vid.clone());
                        cx.notify();
                    }))
                    .child(yt::caption(format!("{} · {}", c.channel, when::when(c.at))))
                    .child(yt::para(13., yt::TEXT_3, 2, c.before.clone()).line_through())
                    .child(yt::para(13., yt::TEXT, 2, c.after.clone()))
            })))
        });

        div()
            .flex()
            .flex_col()
            .gap(px(24.))
            .p(px(24.))
            .child(header)
            .child(yt::divider())
            .when(!o.top.is_empty(), |d| d.child(best).child(yt::divider()))
            .child(div().flex().flex_wrap().gap(px(32.)).child(lengths.flex_1().min_w(px(380.))).child(timing.flex_1().min_w(px(380.))))
            .child(yt::divider())
            .child(words)
            .child(tags)
            .child(yt::divider())
            .child(movers)
            .children(changes)
    }
}
