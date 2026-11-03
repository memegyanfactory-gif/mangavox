//! Competitor tracker: a long-running watch on manga / manhwa / webtoon recap channels, read from
//! public YouTube pages only (no API key, no quota). The app calls [`run`] every hour while it is
//! open; each pass discovers a few more channels, rescans the stalest ones (subscribers, keywords,
//! every listed video's views), reads some watch pages (exact publish time, tags, description) and
//! checks thumbnails. What changed is kept as history: titles, descriptions and thumbnails (the
//! image is saved only when its bytes changed), plus views and subscribers over time.
//!
//! Files under `<data>/youtube/competitors/`: `channels.json`, `videos/<channel id>.json`,
//! `runs.json`, `state.json`, `thumbs/<video id>/<unix>.jpg`.
//!
//! Page parsing ([`scrape`]) and the change detection ([`track`]) are pure functions so they are
//! tested offline; [`analysis`] turns the stored history into numbers and the AI's [`brief`].
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::trends::{channel_url, page_json, parse_channel, search_url};

pub mod analysis;
pub mod scrape;
pub mod store;
pub mod track;

pub use analysis::{brief, channel_stats, hourly_gains, velocity};
pub use store::{dir, CompStore};

use scrape::{Fail, Net};

/// A point in a history: unix seconds and the value then.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Point {
    pub at: i64,
    pub value: u64,
}

/// A title or description as of `at`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Change {
    pub at: i64,
    pub value: String,
}

/// One saved thumbnail image.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ThumbVersion {
    pub at: i64,
    pub url: String,
    /// SHA-256 of the image bytes, hex.
    pub hash: String,
    /// Path relative to the competitors folder (see [`CompStore::thumb_path`]).
    pub file: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CompChannel {
    /// `UC...`
    pub id: String,
    pub handle: String,
    pub title: String,
    pub avatar: String,
    pub description: String,
    pub keywords: Vec<String>,
    pub country: String,
    pub joined: String,
    pub subscribers: u64,
    pub total_views: u64,
    pub video_count: u64,
    pub first_seen: i64,
    pub last_scanned: i64,
    /// How the tracker found it ("search: manhwa recap", "added by user").
    pub found_via: String,
    pub subs_history: Vec<Point>,
    /// Share of its recent long videos that are recaps, 0..1.
    pub recap_share: f32,
    pub pinned: bool,
    pub hidden: bool,
    /// When the about data (country, joined, total views) was last read.
    pub about_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CompVideo {
    pub id: String,
    pub channel_id: String,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub duration_secs: u64,
    pub published_at: i64,
    /// False while `published_at` is only the "N hours ago" estimate.
    pub published_exact: bool,
    pub first_seen: i64,
    pub last_seen: i64,
    pub views: u64,
    pub likes: Option<u64>,
    pub comments: Option<u64>,
    pub views_history: Vec<Point>,
    /// Oldest first; the first entry is the first title seen.
    pub titles: Vec<Change>,
    pub descriptions: Vec<Change>,
    pub thumbnails: Vec<ThumbVersion>,
    pub category: String,
    /// Derived from the title and tags.
    pub keywords: Vec<String>,
    /// When the watch page was last read (0 = never).
    pub details_at: i64,
    /// When the thumbnail was last downloaded and compared (0 = never).
    pub thumb_checked: i64,
}

/// What one [`run`] may spend.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Budget {
    /// Search pages (discovery).
    pub searches: usize,
    /// Channel pages scanned (pinned channels always are, on top).
    pub channels: usize,
    /// Watch pages read.
    pub watch_pages: usize,
    /// Thumbnails downloaded.
    pub thumbnails: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self { searches: 4, channels: 30, watch_pages: 40, thumbnails: 80 }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RunSummary {
    pub at: i64,
    pub secs: f64,
    pub new_channels: usize,
    pub channels_scanned: usize,
    pub videos_seen: usize,
    pub new_videos: usize,
    pub title_changes: usize,
    pub thumb_changes: usize,
    pub description_changes: usize,
    pub pages_read: usize,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChannelStats {
    pub videos: usize,
    pub uploads_per_week: f64,
    pub avg_views: f64,
    pub median_views: u64,
    pub views_last_24h: u64,
    pub avg_duration_secs: f64,
    /// Uploads per local hour of day (exact publish times only).
    pub upload_hours: [u32; 24],
    /// Uploads per weekday, Monday = 0.
    pub upload_days: [u32; 7],
    pub title_changes: usize,
    pub thumb_changes: usize,
    pub top_tags: Vec<(String, u32)>,
    pub subs_gained_7d: i64,
}

/// Small bookkeeping between runs (`state.json`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct State {
    /// Next discovery query.
    pub cursor: usize,
    /// Manga names seen in tracked videos and how often; they become "<name> recap" searches.
    pub topics: BTreeMap<String, u32>,
}

/// Search filters (`sp`): this week by views, this month by views, this week by relevance, newest.
const FILTERS: &[&str] = &["CAMSAggD", "CAMSAggE", "EgIIAw%3D%3D", "CAI%3D"];

/// Shortest video that counts as a recap when discovering channels.
const MIN_RECAP_SECS: u64 = 240;

const BASE_QUERIES: &[&str] = &[
    "manhwa recap", "manga recap", "webtoon recap", "manhua recap", "manhwa recap full", "manga recap full story",
    "anime recap manhwa", "manhwa explained", "manga explained", "webtoon explained", "manhua explained", "manhwa recap english",
    "manhwa recap overpowered", "manhwa recap regression", "manhwa recap reincarnated", "manhwa recap system", "manhwa recap romance",
    "manhwa recap action", "manhwa recap isekai", "manhwa recap dungeon", "manhwa recap revenge", "manhwa recap tower",
    "manga recap ending", "manga recap season", "manhwa ending explained", "manhua recap cultivation", "manhwa recap martial arts",
    "manhwa recap villainess", "manhwa recap hunter", "manhwa recap necromancer", "manhwa recap academy", "manhwa recap max level",
    "korean manhwa story", "chinese manhua story", "webtoon full story", "manga story explained", "manhwa recap complete",
    "manhwa recap bl", "manga summarized", "manhwa recap new chapter",
];

/// The next discovery search: queries rotate fastest, then the filters.
pub fn pick_search(queries: &[String], cursor: usize) -> Option<(&str, &'static str)> {
    if queries.is_empty() {
        return None;
    }
    Some((queries[cursor % queries.len()].as_str(), FILTERS[(cursor / queries.len()) % FILTERS.len()]))
}

/// Every query discovery rotates through: the fixed list, then "<manga> recap" for the names seen
/// most in tracked videos, then the recap-related keywords channels list for themselves.
pub fn queries(state: &State, channels: &[CompChannel]) -> Vec<String> {
    let mut out: Vec<String> = BASE_QUERIES.iter().map(|s| s.to_string()).collect();
    let mut seen: HashSet<String> = out.iter().cloned().collect();
    let mut add = |q: String, out: &mut Vec<String>| {
        let q = q.trim().to_lowercase();
        if !q.is_empty() && seen.insert(q.clone()) {
            out.push(q);
        }
    };
    let mut topics: Vec<(&String, &u32)> = state.topics.iter().filter(|(_, n)| **n >= 2).collect();
    topics.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (t, _) in topics.into_iter().take(60) {
        add(if t.contains("recap") { t.clone() } else { format!("{t} recap") }, &mut out);
    }
    let mut kws: Vec<&String> = channels.iter().flat_map(|c| &c.keywords).filter(|k| track::is_recap_text(k) && k.split_whitespace().count() <= 5).collect();
    kws.sort();
    kws.dedup();
    for k in kws.into_iter().take(40) {
        add(k.clone(), &mut out);
    }
    out
}

/// One hourly pass: discovery, channel scans, watch pages, thumbnails. A blocked or rate-limited
/// YouTube stops the pass early with `error` set; everything read until then is kept.
pub async fn run(store: &CompStore, budget: Budget, progress: impl Fn(String) + Send + Sync + 'static) -> Result<RunSummary> {
    let started = Instant::now();
    let now = chrono::Utc::now().timestamp();
    let net = Net::new()?;
    let mut sum = RunSummary { at: now, ..Default::default() };
    let progress: &(dyn Fn(String) + Send + Sync) = &progress;
    if let Err(e) = run_pass(store, &budget, &net, progress, now, &mut sum).await {
        sum.error = Some(e.to_string());
    }
    if sum.error.is_none() && net.blocked() {
        sum.error = Some("YouTube is rate-limiting or asking for a check (HTTP 429 or a consent page). Stopped early; the next run continues.".into());
    }
    sum.pages_read = net.pages();
    sum.secs = started.elapsed().as_secs_f64();
    store.push_run(&sum)?;
    progress(format!("Competitors: {} channels scanned, {} new videos", sum.channels_scanned, sum.new_videos));
    Ok(sum)
}

async fn run_pass(store: &CompStore, budget: &Budget, net: &Net, progress: &(dyn Fn(String) + Send + Sync), now: i64, sum: &mut RunSummary) -> Result<()> {
    let mut channels = store.channels();
    let mut state = store.state();

    // 1. Discovery.
    let mut known: HashSet<String> = channels.iter().map(|c| c.id.clone()).collect();
    let qs = queries(&state, &channels);
    for _ in 0..budget.searches {
        if net.blocked() {
            break;
        }
        let Some((q, sp)) = pick_search(&qs, state.cursor) else { break };
        state.cursor += 1;
        progress(format!("Competitors: searching \"{q}\""));
        let Ok(html) = net.text(&search_url(q, sp)).await else { continue };
        let Some(data) = page_json(&html, "ytInitialData") else { continue };
        for h in scrape::search_hits(&data) {
            if h.video.length_secs < MIN_RECAP_SECS || !track::is_recap_text(&h.video.title) || !h.channel_id.starts_with("UC") || !known.insert(h.channel_id.clone()) {
                continue;
            }
            channels.push(CompChannel { id: h.channel_id, handle: h.handle, title: h.channel, first_seen: now, found_via: format!("search: {q}"), ..Default::default() });
            sum.new_channels += 1;
        }
    }

    // 2. Channel scans: pinned, then never scanned, then the stalest.
    let mut order: Vec<usize> = (0..channels.len()).filter(|&i| !channels[i].hidden).collect();
    order.sort_by_key(|&i| (!channels[i].pinned, channels[i].last_scanned > 0, channels[i].last_scanned));
    let pinned = order.iter().filter(|&&i| channels[i].pinned).count();
    order.truncate(budget.channels.max(pinned));
    let mut loaded: HashMap<String, Vec<CompVideo>> = HashMap::new();
    let mut tasks = tokio::task::JoinSet::new();
    for &i in &order {
        let (net, id, about) = (net.clone(), channels[i].id.clone(), track::about_due(&channels[i], now));
        tasks.spawn(async move { (i, scrape::scan_channel(&net, &id, about).await) });
    }
    let total = order.len();
    let mut done = 0;
    while let Some(r) = tasks.join_next().await {
        done += 1;
        let Ok((i, res)) = r else { continue };
        progress(format!("Competitors: scanned channels ({done}/{total})"));
        match res {
            Ok((page, about)) => {
                let id = channels[i].id.clone();
                let videos = loaded.entry(id).or_insert_with(|| store.videos(&channels[i].id));
                let c = track::apply_scan(&mut channels[i], videos, &page, about.as_ref(), now);
                sum.channels_scanned += 1;
                sum.videos_seen += c.videos_seen;
                sum.new_videos += c.new_videos;
                sum.title_changes += c.title_changes;
            }
            // Rotate a channel that cannot be read behind the others.
            Err(Fail::Other(_)) => channels[i].last_scanned = now,
            Err(Fail::Blocked(_)) => {}
        }
    }

    // 3. Watch pages: unknown publish times first, then the newest not read in a day.
    let mut jobs: Vec<(u8, std::cmp::Reverse<i64>, String, String)> = Vec::new();
    for (cid, vs) in &loaded {
        for v in vs {
            let class = if !v.published_exact { 0 } else if v.details_at == 0 || (now - v.published_at < 30 * track::DAY && now - v.details_at >= track::DAY) { 1 } else { continue };
            jobs.push((class, std::cmp::Reverse(v.published_at), cid.clone(), v.id.clone()));
        }
    }
    jobs.sort();
    jobs.truncate(budget.watch_pages);
    let mut tasks = tokio::task::JoinSet::new();
    for (_, _, cid, vid) in jobs {
        let net = net.clone();
        tasks.spawn(async move {
            let r = scrape::read_watch(&net, &vid).await;
            (cid, vid, r)
        });
    }
    let total = tasks.len();
    let mut done = 0;
    while let Some(r) = tasks.join_next().await {
        done += 1;
        let Ok((cid, vid, Ok(w))) = r else { continue };
        progress(format!("Competitors: read video pages ({done}/{total})"));
        if let Some(v) = loaded.get_mut(&cid).and_then(|vs| vs.iter_mut().find(|v| v.id == vid)) {
            let c = track::apply_watch(v, &w, now);
            sum.title_changes += c.title as usize;
            sum.description_changes += c.description as usize;
        }
    }

    // 4. Thumbnails: the least recently compared first; young videos hourly, old ones daily.
    let mut jobs: Vec<(i64, std::cmp::Reverse<i64>, String, String)> = Vec::new();
    for (cid, vs) in &loaded {
        for v in vs.iter().filter(|v| track::thumb_due(v, now)) {
            jobs.push((v.thumb_checked, std::cmp::Reverse(v.published_at), cid.clone(), v.id.clone()));
        }
    }
    jobs.sort();
    jobs.truncate(budget.thumbnails);
    let mut tasks = tokio::task::JoinSet::new();
    for (_, _, cid, vid) in jobs {
        let net = net.clone();
        tasks.spawn(async move {
            let r = net.bytes(&scrape::thumb_url(&vid)).await;
            (cid, vid, r)
        });
    }
    let total = tasks.len();
    let mut done = 0;
    while let Some(r) = tasks.join_next().await {
        done += 1;
        let Ok((cid, vid, res)) = r else { continue };
        progress(format!("Competitors: thumbnails ({done}/{total})"));
        let Some(v) = loaded.get_mut(&cid).and_then(|vs| vs.iter_mut().find(|v| v.id == vid)) else { continue };
        match res {
            Ok(bytes) if bytes.len() > 500 => match track::apply_thumb(v, &bytes, now) {
                track::ThumbChange::Same => {}
                ch => {
                    match store.save_thumb(&vid, now, &bytes) {
                        Ok(file) => {
                            let t = v.thumbnails.last_mut();
                            if let Some(t) = t {
                                t.file = file;
                            }
                            sum.thumb_changes += (ch == track::ThumbChange::Changed) as usize;
                        }
                        // Not saved: forget the version so the next run tries again.
                        Err(_) => {
                            v.thumbnails.pop();
                            v.thumb_checked = 0;
                        }
                    }
                }
            },
            Ok(_) | Err(Fail::Other(_)) => v.thumb_checked = now,
            Err(Fail::Blocked(_)) => {}
        }
    }

    // 5. Save: videos, then channels (the user's pins and hides made meanwhile win), topics, state.
    for &i in &order {
        if let Some(vs) = loaded.get(&channels[i].id) {
            channels[i].recap_share = track::recap_share(vs);
            track::count_topics(&mut state.topics, vs, now);
        }
    }
    track::trim_topics(&mut state.topics, 300);
    for (cid, vs) in &loaded {
        store.save_videos(cid, vs)?;
    }
    store.merge_save_channels(channels)?;
    store.save_state(&state)?;
    Ok(())
}

/// Adds a channel by `@handle`, channel id or link (and reads it once). A hidden channel comes back.
pub async fn add_channel(store: &CompStore, input: &str) -> Result<CompChannel> {
    let url = channel_url(input).ok_or_else(|| anyhow!("Enter the channel as @handle or its link"))?;
    let net = Net::new()?;
    let html = net.text(&url).await.map_err(|e| anyhow!("{e}"))?;
    let data = page_json(&html, "ytInitialData").ok_or_else(|| anyhow!("Could not read the channel page"))?;
    let page = parse_channel(&data);
    let id = scrape::channel_id(&data).ok_or_else(|| anyhow!("No channel found at {url}"))?;
    if page.name.is_empty() {
        return Err(anyhow!("No channel found at {url}"));
    }
    let now = chrono::Utc::now().timestamp();
    let mut channels = store.channels();
    let i = match channels.iter().position(|c| c.id == id) {
        Some(i) => i,
        None => {
            channels.push(CompChannel { id: id.clone(), first_seen: now, found_via: "added by user".into(), ..Default::default() });
            channels.len() - 1
        }
    };
    channels[i].hidden = false;
    let mut videos = store.videos(&id);
    track::apply_scan(&mut channels[i], &mut videos, &page, None, now);
    store.save_videos(&id, &videos)?;
    let out = channels[i].clone();
    store.merge_save_channels(channels)?;
    store.set_hidden(&id, false)?;
    Ok(out)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_defaults() {
        let b = Budget::default();
        assert_eq!((b.searches, b.channels, b.watch_pages, b.thumbnails), (4, 30, 40, 80));
    }

    #[test]
    fn searches_rotate_through_queries_then_filters() {
        let qs = vec!["a".to_string(), "b".to_string()];
        assert_eq!(pick_search(&qs, 0), Some(("a", FILTERS[0])));
        assert_eq!(pick_search(&qs, 1), Some(("b", FILTERS[0])));
        assert_eq!(pick_search(&qs, 2), Some(("a", FILTERS[1])));
        assert_eq!(pick_search(&qs, 2 * FILTERS.len()), Some(("a", FILTERS[0])));
        assert_eq!(pick_search(&[], 3), None);
    }

    #[test]
    fn queries_grow_from_topics_and_channel_keywords() {
        let mut st = State::default();
        st.topics.insert("solo leveling".into(), 5);
        st.topics.insert("one off".into(), 1);
        let ch = CompChannel { keywords: vec!["manhwa recap".into(), "Manga Recaps Daily".into(), "cooking".into()], ..Default::default() };
        let q = queries(&st, &[ch]);
        assert!(q.len() >= 40);
        assert!(q.contains(&"solo leveling recap".to_string()));
        assert!(!q.contains(&"one off recap".to_string()));
        assert!(q.contains(&"manga recaps daily".to_string()));
        assert!(!q.contains(&"cooking".to_string()));
        let mut d = q.clone();
        d.sort();
        d.dedup();
        assert_eq!(d.len(), q.len());
    }

    /// Live smoke test: `cargo test -p mv-youtube competitors -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live_run() {
        let d = tempfile::tempdir().unwrap();
        let store = CompStore::new(d.path());
        let s = run(&store, Budget { searches: 1, channels: 2, watch_pages: 4, thumbnails: 4 }, |m| println!("{m}")).await.unwrap();
        println!("{s:?}");
        let chs = store.channels();
        println!("{} channels", chs.len());
        for c in chs.iter().filter(|c| c.last_scanned > 0) {
            println!("{} {} subs={} country={} kw={:?}", c.title, c.handle, c.subscribers, c.country, c.keywords);
            let vs = store.videos(&c.id);
            println!("  {} videos; first: {:?}", vs.len(), vs.first().map(|v| (&v.title, v.views, v.published_exact, v.duration_secs, v.thumbnails.len())));
        }
        println!("{}", brief(&store, s.at, 0));
        assert!(s.pages_read > 0);
    }
}
