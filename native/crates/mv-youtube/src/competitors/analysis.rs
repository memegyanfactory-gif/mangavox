//! Numbers from the stored history: views gained per hour, velocity, per-channel statistics, and
//! [`brief`], the analysed text the AI chat reads. Everything is a pure function over the stored
//! data (times in unix seconds; `offset` is the local zone in seconds east of UTC).
use std::collections::{BTreeMap, HashMap, HashSet};

use super::track::{DAY, HOUR};
use super::{ChannelStats, CompChannel, CompStore, CompVideo, Point, RunSummary};
use crate::timing::local_day_hour;
use crate::trends::{candidates, median, normalize, TrendVideo};

/// Views gained between consecutive history points, stamped with the later point's time.
pub fn hourly_gains(v: &CompVideo) -> Vec<(i64, u64)> {
    v.views_history.windows(2).map(|w| (w[1].at, w[1].value.saturating_sub(w[0].value))).collect()
}

/// Recent views per hour: the gain over the last ~3 hours of history (or all of it when shorter);
/// with a single point, views over the video's age. 0 when the history is more than a week stale.
pub fn velocity(v: &CompVideo, now: i64) -> f64 {
    if let Some(last) = v.views_history.last() {
        if now - last.at > 7 * DAY {
            return 0.0;
        }
        let base = v.views_history.iter().rev().find(|p| last.at - p.at >= 3 * HOUR).or(v.views_history.first());
        if let Some(b) = base {
            let hours = (last.at - b.at) as f64 / 3600.0;
            if hours >= 0.25 {
                return last.value.saturating_sub(b.value) as f64 / hours;
            }
        }
    }
    if v.published_at > 0 && v.published_at < now {
        v.views as f64 / ((now - v.published_at) as f64 / 3600.0).max(1.0)
    } else {
        0.0
    }
}

/// Views gained in the window `[since, last point]`; a longer gap between points is scaled down to
/// the window (a channel scanned every few days still gives a per-day figure).
fn gain_since(v: &CompVideo, since: i64, window: i64) -> u64 {
    let pts = &v.views_history;
    let Some(last) = pts.last() else { return 0 };
    if last.at < since {
        return 0;
    }
    let Some(base) = pts.iter().rev().find(|p| p.at <= since).or(pts.first()) else { return 0 };
    let (gain, span) = (last.value.saturating_sub(base.value) as f64, last.at - base.at);
    if span <= 0 {
        0
    } else if span > window {
        (gain * window as f64 / span as f64).round() as u64
    } else {
        gain as u64
    }
}

/// Views per hour inside `[from, to]` from the points there; needs two points 30 minutes apart.
fn rate_between(pts: &[Point], from: i64, to: i64) -> Option<f64> {
    let inside: Vec<&Point> = pts.iter().filter(|p| p.at >= from && p.at <= to).collect();
    let (a, b) = (inside.first()?, inside.last()?);
    let hours = (b.at - a.at) as f64 / 3600.0;
    (hours >= 0.5).then(|| b.value.saturating_sub(a.value) as f64 / hours)
}

fn points_gain(p: &[Point], now: i64, window: i64) -> i64 {
    let (Some(last), Some(first)) = (p.last(), p.first()) else { return 0 };
    let base = p.iter().rev().find(|x| x.at <= now - window).unwrap_or(first);
    let span = last.at - base.at;
    let gain = last.value as i64 - base.value as i64;
    if span > window {
        (gain as f64 * window as f64 / span as f64).round() as i64
    } else {
        gain
    }
}

/// What a channel's stored videos and subscriber history say.
pub fn channel_stats(ch: &CompChannel, videos: &[CompVideo], now: i64, offset: i32) -> ChannelStats {
    let mut s = ChannelStats { videos: videos.len(), ..Default::default() };
    if videos.is_empty() {
        s.subs_gained_7d = points_gain(&ch.subs_history, now, 7 * DAY);
        return s;
    }
    let views: Vec<u64> = videos.iter().map(|v| v.views).collect();
    s.avg_views = views.iter().sum::<u64>() as f64 / views.len() as f64;
    s.median_views = median(views);
    let durations: Vec<u64> = videos.iter().map(|v| v.duration_secs).filter(|d| *d > 0).collect();
    if !durations.is_empty() {
        s.avg_duration_secs = durations.iter().sum::<u64>() as f64 / durations.len() as f64;
    }
    s.views_last_24h = videos.iter().map(|v| gain_since(v, now - DAY, DAY)).sum();

    // Uploads per week over the last 90 days, or over what the stored list covers when shorter.
    let recent: Vec<i64> = videos.iter().map(|v| v.published_at).filter(|&p| p > 0 && now - p <= 90 * DAY).collect();
    if let Some(oldest) = videos.iter().map(|v| v.published_at).filter(|&p| p > 0).min() {
        let span_days = (((now - oldest) as f64) / DAY as f64).clamp(7.0, 90.0);
        s.uploads_per_week = recent.len() as f64 / (span_days / 7.0);
    }
    for v in videos.iter().filter(|v| v.published_exact) {
        let (d, h) = local_day_hour(v.published_at, offset);
        s.upload_days[d] += 1;
        s.upload_hours[h] += 1;
    }
    s.title_changes = videos.iter().map(|v| v.titles.len().saturating_sub(1)).sum();
    s.thumb_changes = videos.iter().map(|v| v.thumbnails.len().saturating_sub(1)).sum();
    s.top_tags = top_counts(videos.iter().flat_map(|v| v.tags.iter().map(|t| t.to_lowercase())), 10, 1);
    s.subs_gained_7d = points_gain(&ch.subs_history, now, 7 * DAY);
    s
}

/// The `max` most frequent items seen at least `min` times, ties alphabetical.
fn top_counts(items: impl Iterator<Item = String>, max: usize, min: u32) -> Vec<(String, u32)> {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for i in items.filter(|i| !i.trim().is_empty()) {
        *counts.entry(i).or_default() += 1;
    }
    let mut v: Vec<(String, u32)> = counts.into_iter().filter(|(_, n)| *n >= min).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v.truncate(max);
    v
}

// ------------------------------------------------------------------------------- brief

/// Words that say nothing about why a title works.
const STOP: &[&str] = &[
    "the", "and", "with", "from", "that", "this", "into", "have", "your", "what", "when", "will", "recap", "recaps", "manga", "manhwa", "manhua", "webtoon",
    "full", "story", "explained", "part", "chapter", "chapters", "ending", "episode", "episodes", "season",
];

const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Channels whose videos are read for the brief (the biggest, the pinned, the latest scanned).
const SAMPLE: usize = 150;

/// 1234 -> "1.2K", 3_400_000 -> "3.4M".
pub fn fmt_n(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}K", n as f64 / 1e3),
        1_000_000..=999_999_999 => format!("{:.1}M", n as f64 / 1e6),
        _ => format!("{:.1}B", n as f64 / 1e9),
    }
}

fn signed(n: i64) -> String {
    format!("{}{}", if n < 0 { "-" } else { "+" }, fmt_n(n.unsigned_abs()))
}

fn local(at: i64, offset: i32) -> String {
    chrono::DateTime::from_timestamp(at + offset as i64, 0).map(|d| d.format("%b %-d, %-I:%M %p").to_string()).unwrap_or_default()
}

fn name(c: &CompChannel) -> String {
    [&c.title, &c.handle, &c.id].into_iter().find(|s| !s.is_empty()).cloned().unwrap_or_default()
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(n).collect::<String>().trim_end())
    }
}

fn ago(secs: i64) -> String {
    match secs {
        i64::MIN..=5399 => format!("{} min ago", (secs / 60).max(1)),
        5400..=172_799 => format!("{:.0} h ago", secs as f64 / 3600.0),
        _ => format!("{} days ago", secs / DAY),
    }
}

/// Appends lines until the size budget is spent.
struct Out(String);

impl Out {
    const MAX: usize = 5900;
    fn line(&mut self, s: impl AsRef<str>) {
        if self.0.len() + s.as_ref().len() < Self::MAX {
            self.0.push_str(s.as_ref());
            self.0.push('\n');
        }
    }
}

fn manga_of(v: &CompVideo) -> Option<String> {
    let t = TrendVideo { title: v.title.clone(), description: v.description.clone(), keywords: v.tags.clone(), ..Default::default() };
    candidates(&t).into_iter().next()
}

/// The analysed summary of everything tracked, as plain text for the AI chat (about 6000
/// characters at most). Videos are read for the biggest, the pinned and the most recently
/// scanned channels only.
pub fn brief(store: &CompStore, now: i64, offset: i32) -> String {
    let channels: Vec<CompChannel> = store.channels().into_iter().filter(|c| !c.hidden).collect();
    let mut by_subs: Vec<&CompChannel> = channels.iter().collect();
    by_subs.sort_by(|a, b| b.subscribers.cmp(&a.subscribers));
    let mut by_scan: Vec<&CompChannel> = channels.iter().filter(|c| c.last_scanned > 0).collect();
    by_scan.sort_by(|a, b| b.last_scanned.cmp(&a.last_scanned));
    let mut seen = HashSet::new();
    let sample: Vec<&CompChannel> = by_subs.iter().take(SAMPLE).copied().chain(channels.iter().filter(|c| c.pinned)).chain(by_scan.into_iter().take(100)).filter(|c| seen.insert(c.id.clone())).collect();
    let videos: HashMap<String, Vec<CompVideo>> = sample.iter().map(|c| (c.id.clone(), store.videos(&c.id))).filter(|(_, v)| !v.is_empty()).collect();
    build_brief(&channels, &videos, &store.runs(), now, offset)
}

/// [`brief`] over data already loaded (`videos` by channel id).
pub fn build_brief(channels: &[CompChannel], videos: &HashMap<String, Vec<CompVideo>>, runs: &[RunSummary], now: i64, offset: i32) -> String {
    let mut o = Out(String::new());
    if channels.is_empty() {
        return "COMPETITORS: nothing tracked yet. The tracker finds manga/manhwa recap channels on YouTube each hour while the app is open.".into();
    }
    let by_id: HashMap<&str, &CompChannel> = channels.iter().map(|c| (c.id.as_str(), c)).collect();
    let all: Vec<&CompVideo> = videos.values().flatten().collect();
    let since = channels.iter().map(|c| c.first_seen).filter(|&t| t > 0).min().unwrap_or(now);
    o.line("COMPETITOR TRACKER (YouTube manga/manhwa/webtoon recap channels)");
    o.line(format!(
        "Tracking {} channels ({} pinned, {} scanned at least once) since {}; {} videos in the analysed sample from {} channels.",
        channels.len(),
        channels.iter().filter(|c| c.pinned).count(),
        channels.iter().filter(|c| c.last_scanned > 0).count(),
        local(since, offset),
        all.len(),
        videos.len()
    ));
    if let Some(r) = runs.last() {
        o.line(format!("Last run {}: {} channels scanned, {} new videos, {} new channels{}.", ago(now - r.at), r.channels_scanned, r.new_videos, r.new_channels, r.error.as_ref().map(|e| format!(", stopped early: {}", clip(e, 60))).unwrap_or_default()));
    }

    let stats: HashMap<&str, ChannelStats> = videos.iter().filter_map(|(id, vs)| by_id.get(id.as_str()).map(|c| (c.id.as_str(), channel_stats(c, vs, now, offset)))).collect();
    let line_for = |c: &CompChannel| {
        if c.last_scanned == 0 {
            return format!("- {} (not scanned yet)", clip(&name(c), 32));
        }
        let s = stats.get(c.id.as_str());
        let mut l = format!("- {}{}: {} subs", clip(&name(c), 32), if c.country.is_empty() { String::new() } else { format!(" ({})", c.country) }, fmt_n(c.subscribers));
        if let Some(s) = s {
            l += &format!(" ({} in 7d), {:.1} uploads/week, avg {} views, {} views in the last 24h", signed(s.subs_gained_7d), s.uploads_per_week, fmt_n(s.avg_views as u64), fmt_n(s.views_last_24h));
        }
        l
    };
    let mut top: Vec<&CompChannel> = channels.iter().collect();
    top.sort_by(|a, b| b.subscribers.cmp(&a.subscribers));
    o.line("\nTOP CHANNELS BY SUBSCRIBERS");
    top.iter().take(8).for_each(|c| o.line(line_for(c)));
    let mut hot: Vec<(&CompChannel, u64)> = stats.iter().filter_map(|(id, s)| by_id.get(id).map(|c| (*c, s.views_last_24h))).filter(|(_, v)| *v > 0).collect();
    hot.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.id.cmp(&b.0.id)));
    if !hot.is_empty() {
        o.line("\nMOST VIEWS GAINED IN THE LAST 24 HOURS");
        hot.iter().take(8).for_each(|(c, _)| o.line(line_for(c)));
    }

    // Best recent videos.
    let mut recent: Vec<&CompVideo> = all.iter().copied().filter(|v| v.published_at > 0 && now - v.published_at <= 14 * DAY && v.views > 0).collect();
    let score = |v: &CompVideo| v.views as f64 / (((now - v.published_at) as f64 / 3600.0).max(12.0));
    recent.sort_by(|a, b| score(b).total_cmp(&score(a)));
    let best: Vec<&CompVideo> = recent.iter().take(15).copied().collect();
    if !best.is_empty() {
        o.line("\nBEST RECENT VIDEOS (last 14 days, views per hour of age)");
        for v in best.iter().take(8) {
            let who = by_id.get(v.channel_id.as_str()).map(|c| name(c)).unwrap_or_default();
            o.line(format!("- \"{}\" - {}: {} views, {:.0}/h, {} min, published {}", clip(&v.title, 70), clip(&who, 24), fmt_n(v.views), score(v), v.duration_secs / 60, local(v.published_at, offset)));
        }
    }
    if best.len() >= 3 {
        o.line(format!("\nWHAT THE TOP {} HAVE IN COMMON", best.len()));
        let n = best.len() as f64;
        let pct = |f: &dyn Fn(&CompVideo) -> bool| (best.iter().filter(|v| f(v)).count() as f64 / n * 100.0).round();
        let durs: Vec<u64> = best.iter().map(|v| v.duration_secs).filter(|d| *d > 0).collect();
        if !durs.is_empty() {
            let (lo, hi) = (durs.iter().min().unwrap() / 60, durs.iter().max().unwrap() / 60);
            o.line(format!("- Length: median {} min (range {}-{} min).", median(durs.clone()) / 60, lo, hi));
        }
        let avg_len = best.iter().map(|v| v.title.chars().count()).sum::<usize>() as f64 / n;
        o.line(format!(
            "- Titles: {:.0} characters on average; {}% have a number, {}% a question or exclamation mark, {}% an ALL-CAPS word, {}% say 'ending' or 'full'.",
            avg_len,
            pct(&|v| v.title.chars().any(|c| c.is_ascii_digit())),
            pct(&|v| v.title.contains('?') || v.title.contains('!')),
            pct(&|v| v.title.split_whitespace().any(|w| w.len() >= 3 && w.chars().all(|c| c.is_ascii_uppercase()))),
            pct(&|v| { let t = v.title.to_lowercase(); t.contains("ending") || t.contains("full") }),
        ));
        let words = top_counts(
            best.iter().flat_map(|v| {
                let set: HashSet<String> = v.title.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 4 && !STOP.contains(w)).map(str::to_string).collect();
                set.into_iter()
            }),
            8,
            2,
        );
        if !words.is_empty() {
            o.line(format!("- Title words: {}.", words.iter().map(|(w, c)| format!("{w} ({c})")).collect::<Vec<_>>().join(", ")));
        }
        let tags = top_counts(best.iter().flat_map(|v| v.tags.iter().map(|t| t.to_lowercase())), 8, 2);
        if !tags.is_empty() {
            o.line(format!("- Tags: {}.", tags.iter().map(|(w, c)| format!("{w} ({c})")).collect::<Vec<_>>().join(", ")));
        }
        let (mut hours, mut days) = ([0u32; 24], [0u32; 7]);
        for v in best.iter().filter(|v| v.published_exact) {
            let (d, h) = local_day_hour(v.published_at, offset);
            days[d] += 1;
            hours[h] += 1;
        }
        let top_idx = |a: &[u32], k: usize| {
            let mut i: Vec<usize> = (0..a.len()).filter(|&i| a[i] > 0).collect();
            i.sort_by(|&x, &y| a[y].cmp(&a[x]).then(x.cmp(&y)));
            i.truncate(k);
            i
        };
        let (th, td) = (top_idx(&hours, 3), top_idx(&days, 3));
        if !th.is_empty() {
            o.line(format!(
                "- Uploaded (local time): mostly {} around {}.",
                td.iter().map(|&d| DAYS[d]).collect::<Vec<_>>().join("/"),
                th.iter().map(|h| format!("{} {}", if h % 12 == 0 { 12 } else { h % 12 }, if *h < 12 { "AM" } else { "PM" })).collect::<Vec<_>>().join(", ")
            ));
        }
    }

    // Changes in the last week.
    let week = now - 7 * DAY;
    let rates = |v: &CompVideo, at: i64| {
        let (b, a) = (rate_between(&v.views_history, at - DAY, at), rate_between(&v.views_history, at, at + DAY));
        match (b, a) {
            (Some(b), Some(a)) => format!(" Views/h {:.0} -> {:.0}.", b, a),
            _ => " (too little view data around it yet)".into(),
        }
    };
    let who = |v: &CompVideo| by_id.get(v.channel_id.as_str()).map(|c| name(c)).unwrap_or_default();
    let mut titled: Vec<(&CompVideo, i64)> = all.iter().filter_map(|v| v.titles.last().filter(|c| v.titles.len() > 1 && c.at >= week).map(|c| (*v, c.at))).collect();
    titled.sort_by(|a, b| b.1.cmp(&a.1));
    if !titled.is_empty() {
        o.line(format!("\nTITLE CHANGES IN THE LAST 7 DAYS ({} videos)", titled.len()));
        for (v, at) in titled.iter().take(6) {
            let old = &v.titles[v.titles.len() - 2].value;
            o.line(format!("- {} ({}): \"{}\" -> \"{}\".{}", clip(&who(v), 22), local(*at, offset), clip(old, 55), clip(&v.title, 55), rates(v, *at)));
        }
    }
    let mut thumbed: Vec<(&CompVideo, i64)> = all.iter().filter_map(|v| v.thumbnails.last().filter(|c| v.thumbnails.len() > 1 && c.at >= week).map(|c| (*v, c.at))).collect();
    thumbed.sort_by(|a, b| b.1.cmp(&a.1));
    if !thumbed.is_empty() {
        o.line(format!("\nTHUMBNAIL CHANGES IN THE LAST 7 DAYS ({} videos)", thumbed.len()));
        for (v, at) in thumbed.iter().take(6) {
            o.line(format!("- {} ({}): \"{}\".{}", clip(&who(v), 22), local(*at, offset), clip(&v.title, 60), rates(v, *at)));
        }
    }

    // Hottest manga this week.
    let mut manga: HashMap<String, (String, HashSet<&str>, usize, u64)> = HashMap::new();
    for v in all.iter().filter(|v| v.published_at > 0 && now - v.published_at <= 7 * DAY) {
        let Some(n) = manga_of(v) else { continue };
        let e = manga.entry(normalize(&n)).or_insert_with(|| (n, HashSet::new(), 0, 0));
        e.1.insert(v.channel_id.as_str());
        e.2 += 1;
        e.3 += v.views;
    }
    let mut ranked: Vec<_> = manga.into_values().collect();
    ranked.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(b.3.cmp(&a.3)).then(a.0.cmp(&b.0)));
    if !ranked.is_empty() {
        o.line("\nHOTTEST MANGA THIS WEEK (recaps published in the last 7 days)");
        for (n, chs, vids, views) in ranked.iter().take(10) {
            o.line(format!("- {}: {} channels, {} videos, {} views", clip(n, 40), chs.len(), vids, fmt_n(*views)));
        }
    }
    o.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::competitors::{Change, ThumbVersion};

    fn pt(at: i64, value: u64) -> Point {
        Point { at, value }
    }

    fn video(id: &str, ch: &str, published: i64, history: &[(i64, u64)]) -> CompVideo {
        CompVideo {
            id: id.into(),
            channel_id: ch.into(),
            title: format!("Title {id}"),
            published_at: published,
            published_exact: true,
            views: history.last().map(|p| p.1).unwrap_or(0),
            views_history: history.iter().map(|&(a, v)| pt(a, v)).collect(),
            duration_secs: 600,
            ..Default::default()
        }
    }

    #[test]
    fn gains_between_points() {
        let v = video("a", "c", 0, &[(100, 10), (200, 25), (300, 25), (400, 20)]);
        assert_eq!(hourly_gains(&v), vec![(200, 15), (300, 0), (400, 0)]);
        assert!(hourly_gains(&video("b", "c", 0, &[(1, 1)])).is_empty());
    }

    #[test]
    fn velocity_is_the_recent_rate() {
        let now = 1_000_000;
        // 100 views/h for a long time, then 1000/h over the last three hours.
        let v = video("a", "c", now - 10 * 3600, &[(now - 10 * 3600, 0), (now - 3 * 3600, 700), (now, 3700)]);
        assert!((velocity(&v, now) - 1000.0).abs() < 1e-6);
        // One point: views over age.
        let one = video("b", "c", now - 10 * 3600, &[(now, 500)]);
        assert!((velocity(&one, now) - 50.0).abs() < 1e-6);
        // Stale history.
        assert_eq!(velocity(&video("c", "c", 1, &[(now - 8 * DAY, 5), (now - 8 * DAY + 7200, 50)]), now), 0.0);
        assert_eq!(velocity(&CompVideo::default(), now), 0.0);
    }

    #[test]
    fn stats_for_a_channel() {
        let now = 100 * DAY;
        // Monday 2026-... any: pick a known instant: day index 100 -> weekday (100+3)%7 = 5 (Saturday).
        let mut a = video("a", "c", now - 2 * DAY, &[(now - 30 * HOUR, 100), (now - 20 * HOUR, 400), (now - HOUR, 1000)]);
        a.tags = vec!["Tower".into(), "recap".into()];
        a.titles = vec![Change { at: 1, value: "x".into() }, Change { at: 2, value: "y".into() }];
        a.thumbnails = vec![ThumbVersion::default(), ThumbVersion::default(), ThumbVersion::default()];
        let mut b = video("b", "c", now - 9 * DAY, &[(now - 9 * DAY, 10), (now, 310)]);
        b.tags = vec!["tower".into()];
        b.published_exact = false;
        b.duration_secs = 1200;
        let ch = CompChannel { id: "c".into(), subs_history: vec![pt(now - 8 * DAY, 1000), pt(now - 2 * DAY, 1100), pt(now, 1250)], ..Default::default() };
        let s = channel_stats(&ch, &[a, b], now, 0);
        assert_eq!(s.videos, 2);
        assert_eq!(s.median_views, 1000.max(310));
        assert!((s.avg_views - 655.0).abs() < 1e-9);
        assert!((s.avg_duration_secs - 900.0).abs() < 1e-9);
        // a: base point <= now-24h is (now-30h, 100) -> span 29h > 24h -> 900 * 24/29; b: base is its first point (9d ago), span 9d -> 300 * 1/9.
        assert_eq!(s.views_last_24h, (900.0f64 * 24.0 / 29.0).round() as u64 + (300.0f64 / 9.0).round() as u64);
        assert_eq!((s.title_changes, s.thumb_changes), (1, 2));
        assert_eq!(s.top_tags[0], ("tower".to_string(), 2));
        assert_eq!(s.subs_gained_7d, ((1250 - 1000) as f64 * 7.0 / 8.0).round() as i64);
        assert_eq!(s.upload_days.iter().sum::<u32>(), 1);
        assert_eq!(s.upload_days[(100 - 2 + 3) % 7], 1);
        assert_eq!(s.upload_hours[0], 1);
        // Two uploads in the 9-day span: 2 per (9/7) weeks.
        assert!((s.uploads_per_week - 2.0 / (9.0 / 7.0)).abs() < 1e-9);
        let empty = channel_stats(&ch, &[], now, 0);
        assert_eq!((empty.videos, empty.subs_gained_7d), (0, 219));
    }

    #[test]
    fn local_hours_follow_the_offset() {
        let now = 100 * DAY;
        let v = video("a", "c", 50 * DAY + 23 * HOUR, &[(now, 1)]);
        let s = channel_stats(&CompChannel::default(), &[v], now, 2 * 3600);
        assert_eq!(s.upload_hours[1], 1);
        // 23:00 UTC + 2h is 01:00 the next day.
        assert_eq!(s.upload_days[(50 + 1 + 3) % 7], 1);
    }

    #[test]
    fn numbers_are_short() {
        assert_eq!((fmt_n(950), fmt_n(1500), fmt_n(3_400_000), fmt_n(2_100_000_000)), ("950".into(), "1.5K".into(), "3.4M".into(), "2.1B".into()));
        assert_eq!((signed(-1500), signed(30)), ("-1.5K".to_string(), "+30".to_string()));
    }

    #[test]
    fn the_brief_covers_channels_changes_and_stays_short() {
        let now = 100 * DAY;
        let mut channels = vec![];
        let mut videos = HashMap::new();
        for c in 0..3 {
            let id = format!("UC{c}");
            channels.push(CompChannel { id: id.clone(), title: format!("Channel {c}"), subscribers: 1000 * (c + 1) as u64, first_seen: now - 30 * DAY, last_scanned: now, country: "US".into(), ..Default::default() });
            let mut vs = vec![];
            for k in 0..6 {
                let mut v = video(&format!("{id}v{k}"), &id, now - (k + 1) * DAY, &[(now - 5 * HOUR, 1000 * (k as u64 + 1)), (now - 2 * HOUR, 1500 * (k as u64 + 1)), (now, 2000 * (k as u64 + 1))]);
                v.title = format!("Solo Leveling Recap EP {k}!");
                v.tags = vec!["Solo Leveling".into(), "manhwa recap".into()];
                vs.push(v);
            }
            if c == 0 {
                vs[0].title = "Brand new name".into();
                vs[0].titles = vec![Change { at: now - 3 * DAY, value: "Old name".into() }, Change { at: now - 2 * HOUR, value: "Brand new name".into() }];
                vs[1].thumbnails = vec![ThumbVersion { at: now - 3 * DAY, ..Default::default() }, ThumbVersion { at: now - HOUR, ..Default::default() }];
            }
            videos.insert(id, vs);
        }
        let runs = vec![RunSummary { at: now - 1800, channels_scanned: 30, new_videos: 4, ..Default::default() }];
        let b = build_brief(&channels, &videos, &runs, now, 0);
        assert!(b.len() <= 6000, "{}", b.len());
        for must in ["Tracking 3 channels", "TOP CHANNELS BY SUBSCRIBERS", "Channel 2", "BEST RECENT VIDEOS", "WHAT THE TOP", "TITLE CHANGES", "\"Old name\" -> \"Brand new name\"", "THUMBNAIL CHANGES", "HOTTEST MANGA", "Solo Leveling: 3 channels", "Last run 30 min ago"] {
            assert!(b.contains(must), "missing {must:?} in\n{b}");
        }
        assert!(build_brief(&[], &HashMap::new(), &[], now, 0).contains("nothing tracked"));
    }

    #[test]
    fn brief_reads_the_store() {
        let d = tempfile::tempdir().unwrap();
        let s = CompStore::new(d.path());
        assert!(brief(&s, 1000, 0).contains("nothing tracked"));
        s.save_channels(&[CompChannel { id: "UC1".into(), title: "One".into(), subscribers: 5, first_seen: 10, ..Default::default() }]).unwrap();
        s.save_videos("UC1", &[video("a", "UC1", 900, &[(950, 5)])]).unwrap();
        let b = brief(&s, 1000, 0);
        assert!(b.contains("Tracking 1 channels") && b.contains("One"));
    }
}
