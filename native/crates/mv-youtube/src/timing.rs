//! When to upload: reads the exact publish time of recent recap uploads (each watch page's
//! `publishDate`, no API quota), from the niche searches and then from the busiest channels' own
//! upload lists, and finds the hours and days whose uploads beat their channel's usual views.
//! Saved as `<data>/youtube/timing.json`; [`analyze`] turns it into golden windows in the user's
//! local time.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::trends::{client, fetch, page_json, parse_videos, search_url, SEARCHES};

/// Watch pages read in all (searches first, then the channels' own uploads).
const MAX_PAGES: usize = 320;
/// Channels whose upload list is read too.
const CHANNELS: usize = 14;
/// Uploads younger than this have not settled; they count for "when" but not for "how well".
const SETTLED_SECS: i64 = 2 * 86_400;
/// Older uploads say little about today's schedule.
const MAX_AGE_SECS: i64 = 120 * 86_400;
/// Per channel, uploads needed before its median is trusted.
const MIN_PER_CHANNEL: usize = 3;
/// Window length in hours, and the scored uploads a window needs to be ranked: this many, or 5% of
/// all scored uploads when that is more (a slot with a handful of hits is noise).
const WINDOW: usize = 2;
const MIN_IN_WINDOW: usize = 8;
/// Pulls small windows toward "average" (empirical Bayes shrinkage).
const PRIOR: f64 = 6.;

/// One upload with its exact publish time.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Upload {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub channel_id: String,
    /// Unix seconds (UTC).
    pub at: i64,
    pub views: u64,
    pub length_secs: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Research {
    pub fetched_at: i64,
    pub uploads: Vec<Upload>,
}

/// "2026-10-29T15:09:51-07:00" → Unix seconds; a bare date has no time and gives `None`.
pub fn parse_publish(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s.trim()).ok().map(|d| d.timestamp())
}

/// An upload from a watch page's `ytInitialPlayerResponse`; `None` for lives, Shorts-length
/// videos and pages without a publish time.
pub fn parse_watch(player: &Value) -> Option<Upload> {
    let d = player.get("videoDetails")?;
    if d.get("isLiveContent").and_then(Value::as_bool) == Some(true) || player.get("microformat").and_then(|m| m.pointer("/playerMicroformatRenderer/liveBroadcastDetails")).is_some() {
        return None;
    }
    let s = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let length_secs: u64 = s("lengthSeconds").parse().unwrap_or(0);
    if length_secs < 180 {
        return None;
    }
    let m = player.pointer("/microformat/playerMicroformatRenderer")?;
    let at = m.get("publishDate").or_else(|| m.get("uploadDate")).and_then(Value::as_str).and_then(parse_publish)?;
    Some(Upload { id: s("videoId"), title: s("title"), channel: s("author"), channel_id: s("channelId"), at, views: s("viewCount").parse().unwrap_or(0), length_secs })
}

async fn read_pages(http: &reqwest::Client, ids: Vec<String>, done_before: usize, total: usize, progress: &Arc<dyn Fn(String) + Send + Sync>) -> Vec<Upload> {
    let sem = Arc::new(tokio::sync::Semaphore::new(6));
    let mut tasks = tokio::task::JoinSet::new();
    for id in ids {
        let (http, sem) = (http.clone(), sem.clone());
        tasks.spawn(async move {
            let _permit = sem.acquire().await;
            fetch(&http, &format!("https://www.youtube.com/watch?v={id}")).await.ok().and_then(|h| page_json(&h, "ytInitialPlayerResponse")).and_then(|p| parse_watch(&p))
        });
    }
    let mut out = vec![];
    let mut n = done_before;
    while let Some(r) = tasks.join_next().await {
        n += 1;
        progress(format!("Reading upload times ({n}/{total})"));
        if let Ok(Some(u)) = r {
            out.push(u);
        }
    }
    out
}

/// Searches the niche, reads the uploads' pages, then the busiest channels' recent uploads.
pub async fn research(progress: impl Fn(String) + Send + Sync + 'static) -> Result<Research> {
    let progress: Arc<dyn Fn(String) + Send + Sync> = Arc::new(progress);
    let http = client()?;
    // The usual niche searches, plus newest-first ones (`CAI%3D`), so today's habits show.
    let mut queries: Vec<(String, String)> = SEARCHES.iter().map(|(q, sp)| (q.to_string(), sp.to_string())).collect();
    for q in ["manhwa recap", "manga recap", "manhua recap"] {
        queries.push((q.into(), "CAI%253D".into()));
    }
    let mut tasks = tokio::task::JoinSet::new();
    for (q, sp) in &queries {
        let (http, url) = (http.clone(), search_url(q, sp));
        tasks.spawn(async move { fetch(&http, &url).await.ok().and_then(|h| page_json(&h, "ytInitialData")).map(|d| parse_videos(&d)).unwrap_or_default() });
    }
    let mut ids: Vec<String> = vec![];
    let mut seen = HashSet::new();
    let mut searched = 0;
    while let Some(r) = tasks.join_next().await {
        searched += 1;
        progress(format!("Searching recap uploads ({searched}/{})", queries.len()));
        for v in r.unwrap_or_default() {
            let long = v.length_secs >= 180 || v.length_secs == 0;
            if long && v.age_hours * 3600. <= MAX_AGE_SECS as f64 && seen.insert(v.id.clone()) {
                ids.push(v.id);
            }
        }
    }
    if ids.is_empty() {
        return Err(anyhow!("YouTube returned no recap videos. Check the internet connection and try again."));
    }
    let first: Vec<String> = ids.into_iter().take(MAX_PAGES / 2).collect();
    let estimate = MAX_PAGES.min(first.len() * 2);
    let mut uploads = read_pages(&http, first.clone(), 0, estimate, &progress).await;

    // The channels seen most get their own upload list read: a channel's schedule shows there.
    let mut per: HashMap<String, usize> = HashMap::new();
    for u in &uploads {
        *per.entry(u.channel_id.clone()).or_default() += 1;
    }
    let mut channels: Vec<(String, usize)> = per.into_iter().filter(|(c, _)| !c.is_empty()).collect();
    channels.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut more: Vec<String> = vec![];
    for (i, (cid, _)) in channels.iter().take(CHANNELS).enumerate() {
        progress(format!("Reading channels ({}/{})", i + 1, channels.len().min(CHANNELS)));
        let Ok(html) = fetch(&http, &format!("https://www.youtube.com/channel/{cid}/videos")).await else { continue };
        let Some(data) = page_json(&html, "ytInitialData") else { continue };
        for v in parse_videos(&data).into_iter().take(15) {
            let age = (v.age_hours * 3600.) as i64;
            if age <= MAX_AGE_SECS && seen.insert(v.id.clone()) {
                more.push(v.id);
            }
        }
    }
    more.truncate(MAX_PAGES.saturating_sub(first.len()));
    let total = first.len() + more.len();
    uploads.extend(read_pages(&http, more, first.len(), total, &progress).await);
    let now = chrono::Utc::now().timestamp();
    uploads.retain(|u| now - u.at <= MAX_AGE_SECS && now >= u.at);
    if uploads.len() < 10 {
        return Err(anyhow!("Only {} upload times could be read; try again in a minute.", uploads.len()));
    }
    Ok(Research { fetched_at: now, uploads })
}

// ---------------------------------------------------------------- analysis

/// Monday = 0 … Sunday = 6, and the hour, of `at` in the zone `offset` seconds east of UTC.
pub fn local_day_hour(at: i64, offset: i32) -> (usize, usize) {
    let t = at + offset as i64;
    let days = t.div_euclid(86_400);
    // 1970-01-01 was a Thursday.
    (((days + 3).rem_euclid(7)) as usize, (t.rem_euclid(86_400) / 3600) as usize)
}

/// Uploads and how they did, in one hour or day.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cell {
    pub uploads: usize,
    /// Uploads old enough to score, and the sum of their log2(views / channel median).
    pub scored: usize,
    pub log_sum: f64,
}

impl Cell {
    /// Shrunk mean log2 lift: small samples pull toward 0 (average).
    pub fn score(&self) -> f64 {
        self.log_sum / (self.scored as f64 + PRIOR)
    }
    /// Views against the channel's usual, e.g. 1.4 for "40% more".
    pub fn lift(&self) -> f64 {
        if self.scored == 0 {
            1.
        } else {
            (self.log_sum / self.scored as f64).exp2()
        }
    }
}

/// A recommended upload window in local time.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub start_hour: usize,
    pub hours: usize,
    pub cell: Cell,
    /// Share of all uploads in this window (how crowded it is).
    pub share: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Analysis {
    pub uploads: usize,
    pub channels: usize,
    /// [weekday][hour].
    pub heat: Vec<Vec<Cell>>,
    pub by_hour: Vec<Cell>,
    pub by_day: Vec<Cell>,
    /// Best windows first (by how uploads there do); at most 3, not overlapping.
    pub golden: Vec<Window>,
    /// Weekdays whose uploads do best, best first (only those above average).
    pub best_days: Vec<usize>,
    /// The window most channels upload in.
    pub busiest: Option<Window>,
    /// No channel had enough uploads to judge performance: `golden` falls back to the busiest.
    pub by_habit: bool,
}

fn window_at(by_hour: &[Cell], start: usize, total: usize) -> Window {
    let mut c = Cell::default();
    for h in 0..WINDOW {
        let x = by_hour[(start + h) % 24];
        c.uploads += x.uploads;
        c.scored += x.scored;
        c.log_sum += x.log_sum;
    }
    Window { start_hour: start, hours: WINDOW, cell: c, share: c.uploads as f64 / total.max(1) as f64 }
}

fn apart(a: usize, b: usize) -> bool {
    let d = a.abs_diff(b);
    d.min(24 - d) >= WINDOW + 1
}

/// The golden windows and days of `r` in the zone `offset` seconds east of UTC.
pub fn analyze(r: &Research, offset: i32) -> Analysis {
    let settled: Vec<&Upload> = r.uploads.iter().filter(|u| r.fetched_at - u.at >= SETTLED_SECS && u.views > 0).collect();
    let mut by_channel: HashMap<&str, Vec<u64>> = HashMap::new();
    for u in &settled {
        by_channel.entry(u.channel_id.as_str()).or_default().push(u.views);
    }
    let medians: HashMap<&str, f64> = by_channel
        .into_iter()
        .filter(|(_, v)| v.len() >= MIN_PER_CHANNEL)
        .map(|(k, mut v)| {
            v.sort_unstable();
            (k, v[v.len() / 2].max(1) as f64)
        })
        .collect();
    let mut heat = vec![vec![Cell::default(); 24]; 7];
    let mut by_hour = vec![Cell::default(); 24];
    let mut by_day = vec![Cell::default(); 7];
    for u in &r.uploads {
        let (d, h) = local_day_hour(u.at, offset);
        let lift = (r.fetched_at - u.at >= SETTLED_SECS && u.views > 0).then(|| medians.get(u.channel_id.as_str())).flatten().map(|m| (u.views as f64 / m).log2().clamp(-3., 3.));
        for c in [&mut heat[d][h], &mut by_hour[h], &mut by_day[d]] {
            c.uploads += 1;
            if let Some(l) = lift {
                c.scored += 1;
                c.log_sum += l;
            }
        }
    }
    let total = r.uploads.len();
    let windows: Vec<Window> = (0..24).map(|s| window_at(&by_hour, s, total)).collect();
    let busiest = windows.iter().max_by_key(|w| w.cell.uploads).cloned().filter(|w| w.cell.uploads > 0);
    let by_habit = medians.is_empty();
    let mut ranked: Vec<&Window> = if by_habit {
        let mut w: Vec<&Window> = windows.iter().filter(|w| w.cell.uploads > 0).collect();
        w.sort_by(|a, b| b.cell.uploads.cmp(&a.cell.uploads));
        w
    } else {
        let need = MIN_IN_WINDOW.max((by_hour.iter().map(|c| c.scored).sum::<usize>() as f64 * 0.05).ceil() as usize);
        let mut w: Vec<&Window> = windows.iter().filter(|w| w.cell.scored >= need).collect();
        w.sort_by(|a, b| b.cell.score().total_cmp(&a.cell.score()).then(b.cell.scored.cmp(&a.cell.scored)));
        w
    };
    let mut golden: Vec<Window> = vec![];
    for w in ranked.drain(..) {
        if golden.len() == 3 {
            break;
        }
        if golden.iter().all(|g| apart(g.start_hour, w.start_hour)) {
            golden.push(w.clone());
        }
    }
    let mut days: Vec<usize> = (0..7).filter(|d| by_day[*d].scored >= MIN_IN_WINDOW && by_day[*d].score() > 0.).collect();
    days.sort_by(|a, b| by_day[*b].score().total_cmp(&by_day[*a].score()));
    days.truncate(3);
    Analysis { uploads: total, channels: r.uploads.iter().map(|u| u.channel_id.as_str()).collect::<HashSet<_>>().len(), heat, by_hour, by_day, golden, best_days: days, busiest, by_habit }
}

/// The next time (Unix seconds) the local clock reads `hour`:00, on one of `days` (Monday = 0)
/// when any are given.
pub fn next_at(now: i64, offset: i32, hour: usize, days: &[usize]) -> i64 {
    let local = now + offset as i64;
    let mut t = local.div_euclid(86_400) * 86_400 + hour as i64 * 3600;
    for _ in 0..8 {
        let (d, _) = local_day_hour(t - offset as i64, offset);
        if t > local && (days.is_empty() || days.contains(&d)) {
            return t - offset as i64;
        }
        t += 86_400;
    }
    t - offset as i64
}

pub const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

#[cfg(test)]
mod tests {
    use super::*;

    fn up(ch: &str, at: i64, views: u64) -> Upload {
        Upload { channel_id: ch.into(), at, views, length_secs: 600, ..Default::default() }
    }

    #[test]
    fn publish_dates_with_zones() {
        assert_eq!(parse_publish("2026-10-29T15:09:51-07:00"), Some(1_793_311_791));
        assert_eq!(parse_publish("2026-10-29"), None);
    }

    #[test]
    fn day_and_hour_in_local_time() {
        // 2026-10-29 22:09:51 UTC is a Thursday; +5:30 makes it Friday 03:39.
        assert_eq!(local_day_hour(1_793_311_791, 0), (3, 22));
        assert_eq!(local_day_hour(1_793_311_791, 19_800), (4, 3));
    }

    #[test]
    fn reads_a_watch_page() {
        let p = serde_json::json!({
            "videoDetails": {"videoId": "abc", "title": "T", "author": "C", "channelId": "UC1", "lengthSeconds": "1200", "viewCount": "5000"},
            "microformat": {"playerMicroformatRenderer": {"publishDate": "2026-10-29T15:09:51-07:00"}}
        });
        let u = parse_watch(&p).unwrap();
        assert_eq!((u.at, u.views, u.channel_id.as_str()), (1_793_311_791, 5000, "UC1"));
        let short = serde_json::json!({"videoDetails": {"lengthSeconds": "40"}, "microformat": {"playerMicroformatRenderer": {"publishDate": "2026-10-29T15:09:51-07:00"}}});
        assert!(parse_watch(&short).is_none());
    }

    #[test]
    fn golden_window_beats_the_channel_median() {
        // Two channels; uploads at 14:00 UTC do 4x their channel's usual, others 1x.
        let day = 86_400;
        let base = 1_790_000_000 - 1_790_000_000 % day;
        let mut uploads = vec![];
        for (ch, scale) in [("A", 1000u64), ("B", 50_000)] {
            for i in 0..10 {
                let hour = if i % 3 == 0 { 14 } else { 3 + (i as i64 % 5) * 2 };
                let views = if hour == 14 { scale * 4 } else { scale };
                uploads.push(up(ch, base + i as i64 * day + hour * 3600, views));
            }
        }
        let r = Research { fetched_at: base + 40 * day, uploads };
        let a = analyze(&r, 0);
        assert!(!a.by_habit);
        let g = &a.golden[0];
        assert!(g.start_hour == 13 || g.start_hour == 14, "{g:?}");
        assert!(g.cell.lift() > 2.);
        // Same answer shifted into +5:30 local time (14:00 UTC is 19:30).
        let a2 = analyze(&r, 19_800);
        assert!(a2.golden[0].start_hour == 18 || a2.golden[0].start_hour == 19);
    }

    #[test]
    fn next_time_today_or_on_the_given_day() {
        let monday_noon = 1_793_660_400 - 1_793_660_400 % 86_400 + 12 * 3600; // a day at 12:00 UTC
        let (d, _) = local_day_hour(monday_noon, 0);
        assert_eq!(next_at(monday_noon, 0, 18, &[]), monday_noon + 6 * 3600);
        assert_eq!(next_at(monday_noon, 0, 9, &[]), monday_noon + 21 * 3600);
        let other = (d + 2) % 7;
        let t = next_at(monday_noon, 0, 18, &[other]);
        assert_eq!(local_day_hour(t, 0), (other, 18));
    }
}

#[cfg(test)]
mod live {
    /// Hits YouTube: `cargo test -p mv-youtube live_timing -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_timing() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = rt.block_on(super::research(|_| {})).unwrap();
        let off = chrono::Local::now().offset().local_minus_utc();
        let a = super::analyze(&r, off);
        println!("{} uploads, {} channels, by_habit {}", a.uploads, a.channels, a.by_habit);
        for g in &a.golden {
            println!("golden {:02}:00 +{}h  lift {:.2}  scored {}  uploads {}  share {:.0}%", g.start_hour, g.hours, g.cell.lift(), g.cell.scored, g.cell.uploads, g.share * 100.);
        }
        println!("best days {:?}", a.best_days.iter().map(|d| super::DAYS[*d]).collect::<Vec<_>>());
        if let Some(b) = &a.busiest {
            println!("busiest {:02}:00 share {:.0}%", b.start_hour, b.share * 100.);
        }
        for h in 0..24 {
            let c = a.by_hour[h];
            println!("{h:02}  uploads {:3}  scored {:3}  lift {:.2}", c.uploads, c.scored, c.lift());
        }
    }
}
