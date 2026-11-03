//! What a fresh read changes in the stored data: history points (one per hour for 14 days, one per
//! day after), title / description / thumbnail changes, new videos. Pure functions over the data
//! types so every rule is tested offline.
use std::collections::{BTreeMap, HashMap};

use sha2::{Digest, Sha256};

use super::scrape::{parse_keywords, About, Watch};
use super::{Change, CompChannel, CompVideo, Point, ThumbVersion};
use crate::trends::{candidates, normalize, ChannelPage, TrendVideo};

pub const HOUR: i64 = 3600;
pub const DAY: i64 = 86_400;
/// Points are hourly this long, then daily.
const HOURLY_FOR: i64 = 14 * DAY;
/// Videos kept per channel (the oldest published go first).
const MAX_VIDEOS: usize = 400;
/// The about data is read again after this long.
const ABOUT_EVERY: i64 = 7 * DAY;
/// Videos younger than this have their thumbnail compared every run, older ones daily.
const YOUNG: i64 = 30 * DAY;

// ------------------------------------------------------------------------------- history

/// Adds a point; a second point in the same hour replaces the first (at most one per hour).
pub fn push_point(points: &mut Vec<Point>, at: i64, value: u64) {
    match points.last_mut() {
        Some(last) if last.at.div_euclid(HOUR) == at.div_euclid(HOUR) && at >= last.at => *last = Point { at, value },
        _ => points.push(Point { at, value }),
    }
}

/// Sorts, then keeps one point per hour for the last 14 days and one per day before that (the
/// latest of each slot).
pub fn prune(points: &mut Vec<Point>, now: i64) {
    points.sort_by_key(|p| p.at);
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    let mut last_slot = None;
    for p in points.drain(..) {
        let slot = if p.at >= now - HOURLY_FOR { (true, p.at.div_euclid(HOUR)) } else { (false, p.at.div_euclid(DAY)) };
        if last_slot == Some(slot) {
            out.pop();
        }
        last_slot = Some(slot);
        out.push(p);
    }
    *points = out;
}

/// What happened when a title or description was noted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Noted {
    /// Nothing new.
    Same,
    /// The first value seen.
    First,
    /// It differs from the last one.
    Changed,
}

/// Appends `value` when it differs from the last entry.
pub fn note(list: &mut Vec<Change>, value: &str, now: i64) -> Noted {
    match list.last() {
        Some(c) if c.value == value => Noted::Same,
        last => {
            let first = last.is_none();
            list.push(Change { at: now, value: value.to_string() });
            if first {
                Noted::First
            } else {
                Noted::Changed
            }
        }
    }
}

// ------------------------------------------------------------------------------- scans

/// Counts from applying one channel scan.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScanCounts {
    pub videos_seen: usize,
    pub new_videos: usize,
    pub title_changes: usize,
}

/// Is it time to read the channel's about data?
pub fn about_due(c: &CompChannel, now: i64) -> bool {
    c.about_at == 0 || now - c.about_at >= ABOUT_EVERY
}

/// Applies a channel's `/videos` page: profile, a subscribers point, a views point for every
/// listed video, new videos (published time estimated from "N ago") and title changes.
pub fn apply_scan(ch: &mut CompChannel, videos: &mut Vec<CompVideo>, page: &ChannelPage, about: Option<&About>, now: i64) -> ScanCounts {
    if ch.first_seen == 0 {
        ch.first_seen = now;
    }
    ch.last_scanned = now;
    let keep = |new: &str, old: &mut String| {
        if !new.is_empty() {
            *old = new.to_string();
        }
    };
    keep(&page.name, &mut ch.title);
    keep(&page.handle, &mut ch.handle);
    keep(&page.avatar, &mut ch.avatar);
    keep(&page.description, &mut ch.description);
    if !page.keywords.is_empty() {
        ch.keywords = parse_keywords(&page.keywords);
    }
    if let Some(n) = page.subscribers {
        ch.subscribers = n;
        push_point(&mut ch.subs_history, now, n);
    }
    if let Some(n) = page.video_count {
        ch.video_count = n;
    }
    if let Some(a) = about {
        keep(&a.country, &mut ch.country);
        keep(&a.joined, &mut ch.joined);
        if let Some(n) = a.total_views {
            ch.total_views = n;
        }
        ch.about_at = now;
    }
    prune(&mut ch.subs_history, now);

    let mut counts = ScanCounts::default();
    let mut index: HashMap<String, usize> = videos.iter().enumerate().map(|(i, v)| (v.id.clone(), i)).collect();
    for t in &page.videos {
        counts.videos_seen += 1;
        let i = match index.get(&t.id) {
            Some(&i) => i,
            None => {
                counts.new_videos += 1;
                videos.push(new_video(&ch.id, t, now));
                index.insert(t.id.clone(), videos.len() - 1);
                videos.len() - 1
            }
        };
        let v = &mut videos[i];
        v.last_seen = now;
        // A listing without a count must not zero a known one.
        if t.views > 0 || v.views == 0 {
            v.views = t.views;
            push_point(&mut v.views_history, now, t.views);
        }
        if note(&mut v.titles, &t.title, now) == Noted::Changed {
            counts.title_changes += 1;
        }
        if !t.title.is_empty() {
            v.title = t.title.clone();
        }
        if v.duration_secs == 0 {
            v.duration_secs = t.length_secs;
        }
        prune(&mut v.views_history, now);
    }
    videos.sort_by(|a, b| b.published_at.cmp(&a.published_at));
    videos.truncate(MAX_VIDEOS);
    counts
}

fn new_video(channel_id: &str, t: &TrendVideo, now: i64) -> CompVideo {
    let mut v = CompVideo {
        id: t.id.clone(),
        channel_id: channel_id.to_string(),
        title: t.title.clone(),
        published_at: now - (t.age_hours * 3600.0) as i64,
        first_seen: now,
        duration_secs: t.length_secs,
        ..Default::default()
    };
    v.keywords = derive_keywords(&v);
    v
}

/// What a watch page changed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WatchChange {
    pub title: bool,
    pub description: bool,
}

/// Applies a watch page: exact publish time, tags, description, duration, category, exact views.
pub fn apply_watch(v: &mut CompVideo, w: &Watch, now: i64) -> WatchChange {
    let mut out = WatchChange::default();
    v.details_at = now;
    if let Some(p) = w.published {
        v.published_at = p;
        v.published_exact = true;
    }
    if !w.title.is_empty() {
        out.title = note(&mut v.titles, &w.title, now) == Noted::Changed;
        v.title = w.title.clone();
    }
    if !w.description.is_empty() || !v.descriptions.is_empty() {
        out.description = note(&mut v.descriptions, &w.description, now) == Noted::Changed;
    }
    v.description = w.description.clone();
    if !w.tags.is_empty() {
        v.tags = w.tags.clone();
    }
    if w.duration > 0 {
        v.duration_secs = w.duration;
    }
    if !w.category.is_empty() {
        v.category = w.category.clone();
    }
    if let Some(n) = w.views {
        v.views = n;
        push_point(&mut v.views_history, now, n);
        prune(&mut v.views_history, now);
    }
    v.likes = w.likes.or(v.likes);
    v.comments = w.comments.or(v.comments);
    v.keywords = derive_keywords(v);
    out
}

/// The tags (lower case) followed by the manga names the title and description suggest.
pub fn derive_keywords(v: &CompVideo) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |s: &str| {
        let s = s.trim().to_lowercase();
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    };
    v.tags.iter().take(25).for_each(|t| add(t));
    let t = TrendVideo { title: v.title.clone(), description: v.description.clone(), keywords: v.tags.clone(), ..Default::default() };
    candidates(&t).iter().for_each(|c| add(c));
    out
}

// ------------------------------------------------------------------------------- thumbnails

/// Is the video's thumbnail due for a comparison?
pub fn thumb_due(v: &CompVideo, now: i64) -> bool {
    let young = v.published_at > 0 && now - v.published_at < YOUNG;
    v.thumb_checked == 0 || now - v.thumb_checked >= if young { 45 * 60 } else { DAY }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbChange {
    Same,
    /// The first image stored for the video.
    First,
    Changed,
}

/// SHA-256 of the bytes, hex.
pub fn hash_bytes(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// Compares the downloaded image with the last stored version; for a new one, pushes a
/// [`ThumbVersion`] with an empty `file` (the caller saves the bytes and fills it in).
pub fn apply_thumb(v: &mut CompVideo, bytes: &[u8], now: i64) -> ThumbChange {
    v.thumb_checked = now;
    let hash = hash_bytes(bytes);
    let first = v.thumbnails.is_empty();
    if v.thumbnails.last().is_some_and(|t| t.hash == hash) {
        return ThumbChange::Same;
    }
    v.thumbnails.push(ThumbVersion { at: now, url: super::scrape::thumb_url(&v.id), hash, file: String::new() });
    if first {
        ThumbChange::First
    } else {
        ThumbChange::Changed
    }
}

// ------------------------------------------------------------------------------- recaps

const RECAP_WORDS: &[&str] = &["recap", "manga", "manhwa", "manhua", "webtoon", "explained", "summary", "summarized", "ending"];

/// Does the text talk about manga / manhwa recaps?
pub fn is_recap_text(t: &str) -> bool {
    let l = t.to_lowercase();
    RECAP_WORDS.iter().any(|w| l.contains(w))
}

/// Share of the 20 newest long videos (4+ minutes, or length unknown) whose title is a recap.
pub fn recap_share(videos: &[CompVideo]) -> f32 {
    let mut long: Vec<&CompVideo> = videos.iter().filter(|v| v.duration_secs == 0 || v.duration_secs >= 240).collect();
    long.sort_by(|a, b| b.published_at.cmp(&a.published_at));
    long.truncate(20);
    if long.is_empty() {
        return 0.0;
    }
    long.iter().filter(|v| is_recap_text(&v.title)).count() as f32 / long.len() as f32
}

/// Adds the manga names of the last two weeks' videos to the topic counts (once per video).
pub fn count_topics(topics: &mut BTreeMap<String, u32>, videos: &[CompVideo], now: i64) {
    for v in videos.iter().filter(|v| now - v.published_at < 14 * DAY && v.last_seen >= now) {
        let t = TrendVideo { title: v.title.clone(), description: v.description.clone(), keywords: v.tags.clone(), ..Default::default() };
        if let Some(name) = candidates(&t).first() {
            *topics.entry(normalize(name)).or_default() += 1;
        }
    }
}

/// Keeps the `max` most counted topics.
pub fn trim_topics(topics: &mut BTreeMap<String, u32>, max: usize) {
    if topics.len() <= max {
        return;
    }
    let mut all: Vec<(String, u32)> = std::mem::take(topics).into_iter().collect();
    all.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    all.truncate(max);
    topics.extend(all);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(at: i64, value: u64) -> Point {
        Point { at, value }
    }

    fn listed(id: &str, title: &str, views: u64, hours: f64) -> TrendVideo {
        TrendVideo { id: id.into(), title: title.into(), views, age_hours: hours, ..Default::default() }
    }

    #[test]
    fn one_point_per_hour() {
        let mut p = vec![];
        push_point(&mut p, 3600, 1);
        push_point(&mut p, 3600 + 600, 2);
        push_point(&mut p, 7200 + 5, 3);
        assert_eq!(p, vec![pt(4200, 2), pt(7205, 3)]);
    }

    #[test]
    fn pruning_goes_hourly_then_daily() {
        let now = 100 * DAY;
        let mut p = vec![];
        // Three points an hour apart 20 days ago (one day slot), and three in the last day.
        for k in 0..3 {
            p.push(pt(now - 20 * DAY + k * HOUR, k as u64));
        }
        for k in 0..3 {
            p.push(pt(now - 3 * HOUR + k * HOUR, 10 + k as u64));
        }
        // Two in the same recent hour, out of order.
        p.push(pt(now - 3 * HOUR + 60, 99));
        prune(&mut p, now);
        let old: Vec<_> = p.iter().filter(|x| x.at < now - 14 * DAY).collect();
        assert_eq!(old.len(), 1);
        assert_eq!(old[0].value, 2);
        let recent: Vec<u64> = p.iter().filter(|x| x.at >= now - 14 * DAY).map(|x| x.value).collect();
        assert_eq!(recent, vec![99, 11, 12]);
    }

    #[test]
    fn titles_and_descriptions_keep_a_history() {
        let mut l = vec![];
        assert_eq!(note(&mut l, "A", 1), Noted::First);
        assert_eq!(note(&mut l, "A", 2), Noted::Same);
        assert_eq!(note(&mut l, "B", 3), Noted::Changed);
        assert_eq!(note(&mut l, "A", 4), Noted::Changed);
        assert_eq!(l.iter().map(|c| c.value.as_str()).collect::<String>(), "ABA");
    }

    #[test]
    fn a_scan_adds_videos_points_and_title_changes() {
        let mut ch = CompChannel { id: "UC1".into(), ..Default::default() };
        let mut vs = vec![];
        let mut page = ChannelPage {
            name: "Recaps".into(),
            handle: "@recaps".into(),
            keywords: r#""manga recap" manhwa"#.into(),
            subscribers: Some(1000),
            video_count: Some(50),
            videos: vec![listed("a", "Old title", 100, 5.0), listed("b", "B", 50, 48.0)],
            ..Default::default()
        };
        let now = 1_000_000;
        let c = apply_scan(&mut ch, &mut vs, &page, Some(&About { country: "Japan".into(), joined: "Jan 1, 2020".into(), total_views: Some(9) }), now);
        assert_eq!((c.videos_seen, c.new_videos, c.title_changes), (2, 2, 0));
        assert_eq!((ch.subscribers, ch.country.as_str(), ch.total_views, ch.keywords.len(), ch.first_seen), (1000, "Japan", 9, 2, now));
        assert_eq!(ch.subs_history, vec![pt(now, 1000)]);
        let a = vs.iter().find(|v| v.id == "a").unwrap();
        assert_eq!((a.published_at, a.published_exact), (now - 5 * 3600, false));
        assert_eq!(vs[0].id, "a", "newest published first");

        // Two hours later: a new title, more views, a new subscriber count.
        page.subscribers = Some(1010);
        page.videos = vec![listed("a", "New title", 180, 7.0), listed("c", "C", 5, 1.0)];
        let c = apply_scan(&mut ch, &mut vs, &page, None, now + 2 * HOUR);
        assert_eq!((c.new_videos, c.title_changes), (1, 1));
        assert_eq!(ch.subs_history.len(), 2);
        let a = vs.iter().find(|v| v.id == "a").unwrap();
        assert_eq!(a.titles.iter().map(|t| t.value.as_str()).collect::<Vec<_>>(), ["Old title", "New title"]);
        assert_eq!(a.views_history, vec![pt(now, 100), pt(now + 2 * HOUR, 180)]);
        assert_eq!(a.published_at, now - 5 * 3600, "the estimate is not moved");
        assert_eq!(vs.len(), 3);

        // A listing that lost its view count does not zero the video.
        page.videos = vec![listed("a", "New title", 0, 9.0)];
        apply_scan(&mut ch, &mut vs, &page, None, now + 3 * HOUR);
        assert_eq!(vs.iter().find(|v| v.id == "a").unwrap().views, 180);
    }

    #[test]
    fn a_watch_page_sets_the_exact_time_and_records_description_changes() {
        let mut v = new_video("UC1", &listed("a", "Solo Leveling Recap", 1, 3.0), 10_000);
        let mut w = Watch { published: Some(5555), title: "Solo Leveling Recap".into(), description: "Manhwa: Solo Leveling\nfirst".into(), tags: vec!["Solo Leveling".into(), "Recap".into()], duration: 900, category: "Entertainment".into(), views: Some(77), likes: Some(5), comments: None };
        let c = apply_watch(&mut v, &w, 20_000);
        assert_eq!((c.title, c.description), (false, false));
        assert_eq!((v.published_at, v.published_exact, v.duration_secs, v.views, v.likes, v.comments), (5555, true, 900, 77, Some(5), None));
        assert_eq!(v.descriptions.len(), 1);
        assert!(v.keywords.contains(&"solo leveling".to_string()));
        w.description = "Manhwa: Solo Leveling\nsecond".into();
        w.title = "Solo Leveling Recap (FULL)".into();
        w.comments = Some(3);
        let c = apply_watch(&mut v, &w, 30_000);
        assert_eq!((c.title, c.description), (true, true));
        assert_eq!((v.descriptions.len(), v.titles.len(), v.comments), (2, 2, Some(3)));
        assert_eq!(v.title, "Solo Leveling Recap (FULL)");
    }

    #[test]
    fn thumbnails_are_stored_only_when_the_hash_changes() {
        let mut v = CompVideo { id: "vid".into(), published_at: 1, ..Default::default() };
        assert_eq!(apply_thumb(&mut v, b"one", 100), ThumbChange::First);
        assert_eq!(apply_thumb(&mut v, b"one", 200), ThumbChange::Same);
        assert_eq!(v.thumbnails.len(), 1);
        assert_eq!(v.thumb_checked, 200);
        assert_eq!(apply_thumb(&mut v, b"two", 300), ThumbChange::Changed);
        assert_eq!(v.thumbnails.len(), 2);
        assert_eq!(v.thumbnails[1].hash, hash_bytes(b"two"));
        assert_eq!(v.thumbnails[0].hash.len(), 64);
        assert_eq!(v.thumbnails[1].url, "https://i.ytimg.com/vi/vid/mqdefault.jpg");
        // Changing back to the first image is a change too.
        assert_eq!(apply_thumb(&mut v, b"one", 400), ThumbChange::Changed);
    }

    #[test]
    fn young_thumbnails_are_checked_hourly_old_ones_daily() {
        let now = 100 * DAY;
        let young = CompVideo { published_at: now - DAY, thumb_checked: now - HOUR, ..Default::default() };
        let old = CompVideo { published_at: now - 60 * DAY, thumb_checked: now - HOUR, ..Default::default() };
        let never = CompVideo { published_at: now - 60 * DAY, ..Default::default() };
        assert!(thumb_due(&young, now) && !thumb_due(&old, now) && thumb_due(&never, now));
        assert!(thumb_due(&CompVideo { thumb_checked: now - 2 * DAY, ..old }, now));
    }

    #[test]
    fn recap_share_and_topics() {
        let v = |t: &str, d: u64, at: i64| CompVideo { title: t.into(), duration_secs: d, published_at: at, last_seen: 1000, ..Default::default() };
        let vs = vec![v("Manhwa recap", 600, 3), v("My vlog", 900, 2), v("Short manga clip", 30, 1), v("Webtoon explained", 0, 4)];
        assert!((recap_share(&vs) - 2.0 / 3.0).abs() < 1e-6);
        assert_eq!(recap_share(&[]), 0.0);

        let mut topics = BTreeMap::new();
        let mut a = v("Recap", 600, 1000 - 10);
        a.tags = vec!["Tower of God".into()];
        count_topics(&mut topics, &[a.clone(), a], 1000);
        assert_eq!(topics.get("tower of god"), Some(&2));
        topics.insert("zzz two".into(), 1);
        trim_topics(&mut topics, 1);
        assert_eq!(topics.len(), 1);
    }

    #[test]
    fn about_is_read_weekly() {
        let mut c = CompChannel::default();
        assert!(about_due(&c, 5));
        c.about_at = 100;
        assert!(!about_due(&c, 100 + DAY) && about_due(&c, 100 + 8 * DAY));
    }
}
