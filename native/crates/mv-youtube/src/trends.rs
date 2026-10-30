//! Studio › Suggestions: what manga recap viewers watch right now, read from public YouTube pages
//! (no API key, no quota). A crawl runs several recap searches (this week, this month, by views),
//! reads the top videos' pages for their tags and description, works out which manga each recap
//! covers, and ranks the manga by demand, growth and competition. [`channel_report`] scores a
//! channel (the user's own) against what works in the niche and lists what to fix first.
//!
//! The page data is YouTube's `ytInitialData` / `ytInitialPlayerResponse` JSON; every parser here
//! is a pure function over it so it can be tested offline.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

/// Recap searches a crawl runs, each with an upload-date / sort filter (YouTube's `sp` codes).
pub const SEARCHES: &[(&str, &str)] = &[
    ("manhwa recap", "CAMSAggD"),       // this week, by views
    ("manga recap", "CAMSAggD"),
    ("manhua recap", "CAMSAggD"),
    ("manhwa recap", "CAMSAggE"),       // this month, by views
    ("manga recap", "CAMSAggE"),
    ("manhwa recap", "EgIIAw%3D%3D"),   // this week, by relevance
    ("manga recap", "EgIIAw%3D%3D"),
    ("manga explained", "CAMSAggD"),
    ("webtoon recap", "CAMSAggD"),
];

/// How many of the crawled videos have their own page read (tags, full description).
const DETAIL_PAGES: usize = 60;

/// One recap video found on YouTube.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrendVideo {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub views: u64,
    /// Hours since upload.
    pub age_hours: f64,
    pub length_secs: u64,
    /// Search snippet, or the full description once the video page was read.
    pub description: String,
    /// The uploader's tags (video page only).
    pub keywords: Vec<String>,
    /// Manga names this recap may cover, most likely first. After a crawl: the confirmed manga's
    /// name and other names, or nothing.
    pub candidates: Vec<String>,
    /// The manga it covers, confirmed in the MangaDex catalogue.
    pub manga: Option<MangaRef>,
}

/// A manga confirmed in the MangaDex catalogue (which also lists manhwa and manhua).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MangaRef {
    pub id: String,
    /// The title the recap's name matched, as MangaDex writes it.
    pub name: String,
    /// Its other Latin-script titles (for searching the user's sources).
    pub aliases: Vec<String>,
    pub cover: String,
}

impl TrendVideo {
    pub fn url(&self) -> String {
        format!("https://www.youtube.com/watch?v={}", self.id)
    }

    pub fn thumbnail(&self) -> String {
        format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", self.id)
    }

    /// Views per day since upload (a few hours count as a quarter day at least).
    pub fn velocity(&self) -> f64 {
        self.views as f64 / (self.age_hours / 24.0).max(0.25)
    }
}

/// A manga that recap videos are being made about, with how they do.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrendManga {
    /// Best name found (a tag or a "Manhwa: ..." line), used to search the sources.
    pub name: String,
    /// Other names seen for it.
    pub aliases: Vec<String>,
    /// Ids of its recap videos in [`Trends::videos`], fastest first.
    pub videos: Vec<String>,
    /// Views per day of its three fastest recaps together.
    pub demand: f64,
    /// Share of that demand from recaps uploaded in the last 7 days.
    pub growth: f64,
    /// Distinct channels recapping it.
    pub channels: usize,
    pub newest_hours: f64,
    /// 0-100: demand, growth and (low) competition together. See [`opportunity`].
    pub score: u8,
    pub mangadex_id: String,
    pub cover: String,
}

/// One crawl.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Trends {
    /// Unix seconds.
    pub fetched_at: i64,
    pub videos: Vec<TrendVideo>,
    /// Best opportunity first.
    pub manga: Vec<TrendManga>,
    /// Median views of the crawled recaps (what a typical recap gets), for the channel meter.
    pub niche_median_views: u64,
}

impl Trends {
    pub fn video(&self, id: &str) -> Option<&TrendVideo> {
        self.videos.iter().find(|v| v.id == id)
    }

    /// Recaps up to `max_hours` old, fastest growing first.
    pub fn rising(&self, max_hours: f64) -> Vec<&TrendVideo> {
        let mut v: Vec<&TrendVideo> = self.videos.iter().filter(|v| v.age_hours <= max_hours).collect();
        v.sort_by(|a, b| b.velocity().total_cmp(&a.velocity()));
        v
    }
}

// ------------------------------------------------------------------------------- page parsing

/// The JSON assigned to `var <name> = {...};` in a YouTube page.
pub fn page_json(html: &str, name: &str) -> Option<Value> {
    for marker in [format!("var {name} = "), format!("window[\"{name}\"] = "), format!("{name} = ")] {
        let Some(i) = html.find(&marker) else { continue };
        let rest = &html[i + marker.len()..];
        let end = json_end(rest)?;
        if let Ok(v) = serde_json::from_str(&rest[..end]) {
            return Some(v);
        }
    }
    None
}

/// Byte length of the JSON object at the start of `s` (strings and escapes respected).
fn json_end(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.first() != Some(&b'{') {
        return None;
    }
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    for (i, &c) in b.iter().enumerate() {
        if in_str {
            match (esc, c) {
                (true, _) => esc = false,
                (false, b'\\') => esc = true,
                (false, b'"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// `{"simpleText": ..}` or `{"runs": [{"text": ..}]}` (or a plain string / `{"content": ..}`).
pub fn text(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    if let Some(s) = v.get("simpleText").and_then(Value::as_str) {
        return s.to_string();
    }
    if let Some(s) = v.get("content").and_then(Value::as_str) {
        return s.to_string();
    }
    v.get("runs").and_then(Value::as_array).map(|r| r.iter().filter_map(|x| x.get("text").and_then(Value::as_str)).collect()).unwrap_or_default()
}

/// "170,977 views", "1.2M views", "12K", "No views" -> count.
pub fn parse_count(s: &str) -> Option<u64> {
    let t = s.trim().to_ascii_lowercase();
    if t.starts_with("no ") {
        return Some(0);
    }
    let num: String = t.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',').collect();
    if num.is_empty() {
        return None;
    }
    let rest = t[num.len()..].trim_start();
    let n: f64 = num.replace(',', "").parse().ok()?;
    let mult = match rest.chars().next() {
        Some('k') => 1e3,
        Some('m') => 1e6,
        Some('b') => 1e9,
        _ => 1.0,
    };
    Some((n * mult).round() as u64)
}

/// "9h ago", "1d ago", "2 weeks ago", "Streamed 3 days ago", "1 year ago" -> hours.
pub fn parse_age_hours(s: &str) -> Option<f64> {
    let t = s.to_ascii_lowercase();
    let t = t.trim_start_matches("streamed ").trim_start_matches("premiered ").trim();
    let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
    let n: f64 = digits.parse().ok()?;
    let unit = t[digits.len()..].trim_start();
    let h = match unit {
        u if u.starts_with("sec") || u.starts_with("s ") => 1.0 / 3600.0,
        u if u.starts_with("min") => 1.0 / 60.0,
        u if u.starts_with("mo") => 24.0 * 30.0,
        u if u.starts_with('m') => 1.0 / 60.0,
        u if u.starts_with('h') => 1.0,
        u if u.starts_with('d') => 24.0,
        u if u.starts_with('w') => 24.0 * 7.0,
        u if u.starts_with('y') => 24.0 * 365.0,
        _ => return None,
    };
    Some(n * h)
}

/// "1:02:03" / "12:34" -> seconds.
fn parse_length(s: &str) -> u64 {
    s.split(':').filter_map(|p| p.trim().parse::<u64>().ok()).fold(0, |acc, p| acc * 60 + p)
}

/// Every object under `key` anywhere in `v`.
fn find_all<'a>(v: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k == key {
                    out.push(x);
                } else {
                    find_all(x, key, out);
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| find_all(x, key, out)),
        _ => {}
    }
}

fn video_from_renderer(r: &Value) -> Option<TrendVideo> {
    let id = r.get("videoId")?.as_str()?.to_string();
    let views = r.get("viewCountText").map(text).and_then(|s| parse_count(&s)).unwrap_or(0);
    let age = r.get("publishedTimeText").map(text).and_then(|s| parse_age_hours(&s))?;
    let snippet = r
        .get("detailedMetadataSnippets")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|s| s.get("snippetText"))
        .map(text)
        .or_else(|| r.get("descriptionSnippet").map(text))
        .unwrap_or_default();
    Some(TrendVideo {
        id,
        title: r.get("title").map(text).unwrap_or_default(),
        channel: r.get("ownerText").or_else(|| r.get("longBylineText")).map(text).unwrap_or_default(),
        views,
        age_hours: age,
        length_secs: r.get("lengthText").map(text).map(|s| parse_length(&s)).unwrap_or(0),
        description: snippet,
        ..Default::default()
    })
}

/// The newer `lockupViewModel` cards (channel pages): id, title, and the "N views" / "N ago" parts.
fn video_from_lockup(l: &Value) -> Option<TrendVideo> {
    let id = l.get("contentId")?.as_str()?.to_string();
    if l.get("contentType").and_then(Value::as_str).is_some_and(|t| !t.contains("VIDEO")) {
        return None;
    }
    let meta = l.get("metadata")?.get("lockupMetadataViewModel")?;
    let title = meta.get("title").map(text).unwrap_or_default();
    let mut parts = Vec::new();
    find_all(meta, "content", &mut parts);
    let parts: Vec<String> = parts.into_iter().filter_map(|p| p.as_str().map(str::to_string)).collect();
    let views = parts.iter().find(|p| p.to_ascii_lowercase().contains("view")).and_then(|p| parse_count(p)).unwrap_or(0);
    let age = parts.iter().find(|p| p.to_ascii_lowercase().ends_with("ago")).and_then(|p| parse_age_hours(p))?;
    Some(TrendVideo { id, title, views, age_hours: age, ..Default::default() })
}

/// The videos of a search results or channel page (`ytInitialData`), in page order, no repeats.
pub fn parse_videos(data: &Value) -> Vec<TrendVideo> {
    let mut found = Vec::new();
    for key in ["videoRenderer", "gridVideoRenderer"] {
        let mut rs = Vec::new();
        find_all(data, key, &mut rs);
        found.extend(rs.into_iter().filter_map(video_from_renderer));
    }
    let mut ls = Vec::new();
    find_all(data, "lockupViewModel", &mut ls);
    found.extend(ls.into_iter().filter_map(video_from_lockup));
    let mut seen = HashSet::new();
    found.retain(|v| seen.insert(v.id.clone()));
    found
}

/// Fills in what a video's own page adds (`ytInitialPlayerResponse`): exact views, tags and the
/// full description.
pub fn apply_player(v: &mut TrendVideo, player: &Value) {
    let Some(d) = player.get("videoDetails") else { return };
    if let Some(n) = d.get("viewCount").and_then(Value::as_str).and_then(|s| s.parse().ok()) {
        v.views = n;
    }
    if let Some(k) = d.get("keywords").and_then(Value::as_array) {
        v.keywords = k.iter().filter_map(Value::as_str).map(str::to_string).collect();
    }
    if let Some(s) = d.get("shortDescription").and_then(Value::as_str) {
        v.description = s.to_string();
    }
}

// --------------------------------------------------------------------------- which manga?

/// Words that make a tag about recaps in general rather than one manga.
const GENERIC: &[&str] = &[
    "recap", "recaps", "manga", "manhwa", "manhua", "webtoon", "anime", "explained", "summary", "summarized", "chapter", "chapters", "ch",
    "episode", "full", "season", "series", "recommendation", "recommendations", "review", "story", "english", "sub", "dub", "new", "best",
    "top", "mc", "overpowered", "op", "isekai", "romance", "action", "fantasy", "reincarnation", "reincarnated", "regression", "system",
    "comic", "comics", "audiobook", "storytime", "edit", "amv", "shorts", "short", "video", "part", "movie", "explain", "spoilers",
];

pub fn normalize(name: &str) -> String {
    name.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn generic_share(name: &str) -> f64 {
    let words: Vec<String> = normalize(name).split(' ').map(str::to_string).collect();
    if words.is_empty() {
        return 1.0;
    }
    words.iter().filter(|w| GENERIC.contains(&w.as_str()) || w.chars().all(|c| c.is_ascii_digit())).count() as f64 / words.len() as f64
}

/// Is this tag/line plausibly a manga title (not "manhwa recap", not a channel name, not "ch 1-51")?
fn plausible(name: &str, channel: &str) -> bool {
    let n = normalize(name);
    let words = n.split(' ').filter(|w| !w.is_empty()).count();
    words >= 2 && n.len() <= 90 && generic_share(name) < 0.5 && n != normalize(channel)
}

/// Manga names a recap may cover, most likely first: a "Manhwa: X" / "Title: X" line in the
/// description, then the uploader's tags (the first tag is usually the title), then quoted text.
pub fn candidates(v: &TrendVideo) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let push = |s: &str, out: &mut Vec<String>| {
        let s = s.trim().trim_matches(|c: char| c == '"' || c == '\'' || c == '*' || c == '-' || c == ':' || c == '|').trim();
        if plausible(s, &v.channel) && !out.iter().any(|o| normalize(o) == normalize(s)) {
            out.push(s.to_string());
        }
    };
    for line in v.description.lines() {
        let l = line.trim().trim_start_matches(|c: char| !c.is_alphanumeric());
        let lower = l.to_lowercase();
        for label in ["manhwa name", "manga name", "manhua name", "webtoon name", "original title", "title", "manhwa", "manga", "manhua", "webtoon", "name", "source"] {
            if let Some(rest) = lower.strip_prefix(label) {
                let rest_trim = rest.trim_start();
                if let Some(after) = rest_trim.strip_prefix(':').or_else(|| rest_trim.strip_prefix('-')).or_else(|| rest_trim.strip_prefix('–')) {
                    // Same offset in the original line (lowercasing can change byte lengths).
                    let value = l.get(l.len().saturating_sub(after.len())..).unwrap_or(after);
                    push(value.split(['|', '#', '(', '[']).next().unwrap_or(""), &mut out);
                }
                break;
            }
        }
    }
    for k in &v.keywords {
        push(k, &mut out);
    }
    for q in quoted(&v.title) {
        push(&q, &mut out);
    }
    out
}

/// Some channels put the same manga tags on every upload. A name one channel uses on three or
/// more of the crawled videos says nothing about any one of them, so it is dropped there.
pub fn drop_channel_tags(videos: &mut [TrendVideo]) {
    let mut uses: HashMap<(String, String), usize> = HashMap::new();
    for v in videos.iter() {
        for c in &v.candidates {
            *uses.entry((normalize(&v.channel), normalize(c))).or_default() += 1;
        }
    }
    for v in videos.iter_mut() {
        let ch = normalize(&v.channel);
        v.candidates.retain(|c| uses.get(&(ch.clone(), normalize(c))).copied().unwrap_or(0) < 3);
    }
}

fn quoted(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (open, close) in [('"', '"'), ('“', '”'), ('「', '」')] {
        let mut rest = s;
        while let Some(i) = rest.find(open) {
            let after = &rest[i + open.len_utf8()..];
            let Some(j) = after.find(close) else { break };
            out.push(after[..j].to_string());
            rest = &after[j + close.len_utf8()..];
        }
    }
    out
}

/// How alike two titles are: 1.0 for the same words, by shared words otherwise (one title
/// inside the other counts as a near match).
pub fn similarity(a: &str, b: &str) -> f64 {
    let (na, nb) = (normalize(a), normalize(b));
    if na.is_empty() || nb.is_empty() {
        return 0.0;
    }
    if na == nb {
        return 1.0;
    }
    if (na.len() >= 6 && nb.contains(&na)) || (nb.len() >= 6 && na.contains(&nb)) {
        return 0.9;
    }
    let wa: HashSet<&str> = na.split(' ').collect();
    let wb: HashSet<&str> = nb.split(' ').collect();
    let inter = wa.intersection(&wb).count() as f64;
    inter / (wa.len().max(wb.len()) as f64)
}

/// Latin-script text (English and romanized titles; not Korean, Japanese, Thai...).
fn latin(s: &str) -> bool {
    s.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_ascii() || ('\u{00C0}'..='\u{024F}').contains(&c))
}

/// The manga in a MangaDex `/manga?title=` answer whose title or other name matches `name`
/// (similarity 0.85+), with its cover.
pub fn parse_mangadex(json: &Value, name: &str) -> Option<MangaRef> {
    for m in json.get("data")?.as_array()? {
        let a = m.get("attributes")?;
        let mut titles: Vec<String> = a.get("title").and_then(Value::as_object).map(|o| o.values().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
        if let Some(alts) = a.get("altTitles").and_then(Value::as_array) {
            titles.extend(alts.iter().filter_map(Value::as_object).flat_map(|o| o.values()).filter_map(Value::as_str).map(str::to_string));
        }
        let Some(hit) = titles.iter().filter(|t| similarity(t, name) >= 0.85).max_by(|x, y| similarity(x, name).total_cmp(&similarity(y, name))).cloned() else { continue };
        let id = m.get("id")?.as_str()?.to_string();
        let cover = m
            .get("relationships")
            .and_then(Value::as_array)
            .and_then(|r| r.iter().find(|x| x.get("type").and_then(Value::as_str) == Some("cover_art")))
            .and_then(|c| c.get("attributes")?.get("fileName")?.as_str())
            .map(|f| format!("https://uploads.mangadex.org/covers/{id}/{f}.256.jpg"))
            .unwrap_or_default();
        let mut aliases: Vec<String> = Vec::new();
        for t in titles.into_iter().filter(|t| latin(t) && normalize(t) != normalize(&hit)) {
            if !aliases.iter().any(|x| normalize(x) == normalize(&t)) {
                aliases.push(t);
            }
        }
        aliases.truncate(5);
        return Some(MangaRef { id, name: hit, aliases, cover });
    }
    None
}

async fn mangadex_lookup(http: &reqwest::Client, name: &str) -> Option<MangaRef> {
    let q = url::form_urlencoded::byte_serialize(name.as_bytes()).collect::<String>();
    let url = format!(
        "https://api.mangadex.org/manga?title={q}&limit=5&includes[]=cover_art&contentRating[]=safe&contentRating[]=suggestive&contentRating[]=erotica&order[relevance]=desc"
    );
    let json: Value = http.get(&url).send().await.ok()?.json().await.ok()?;
    parse_mangadex(&json, name)
}

// ------------------------------------------------------------------------------- ranking

/// 0-100 for one manga: demand (its percentile among the crawled manga) 50%, growth (share of
/// demand from this week's recaps) 25%, competition (fewer channels on it is better) 25%.
pub fn opportunity(demand_pct: f64, growth: f64, channels: usize) -> u8 {
    let competition = 1.0 / (1.0 + (channels.saturating_sub(1)) as f64 * 0.35);
    (100.0 * (0.5 * demand_pct.clamp(0.0, 1.0) + 0.25 * growth.clamp(0.0, 1.0) + 0.25 * competition)).round().clamp(0.0, 100.0) as u8
}

/// Groups recaps by the manga they cover (videos with no plausible name are left out) and ranks
/// the manga by [`opportunity`].
pub fn rank(videos: &[TrendVideo]) -> Vec<TrendManga> {
    let mut groups: Vec<(String, Vec<String>, Vec<&TrendVideo>)> = Vec::new();
    for v in videos {
        let Some(best) = v.candidates.first() else { continue };
        let id = v.manga.as_ref().map(|m| m.id.as_str());
        let slot = groups.iter().position(|(name, aliases, vs)| {
            let same_id = id.is_some() && vs[0].manga.as_ref().map(|m| m.id.as_str()) == id;
            same_id || v.candidates.iter().any(|c| similarity(c, name) >= 0.85 || aliases.iter().any(|a| similarity(c, a) >= 0.85))
        });
        match slot {
            Some(i) => {
                let g = &mut groups[i];
                for c in &v.candidates {
                    if !g.1.iter().any(|a| normalize(a) == normalize(c)) && normalize(c) != normalize(&g.0) {
                        g.1.push(c.clone());
                    }
                }
                g.2.push(v);
            }
            None => groups.push((best.clone(), v.candidates[1..].to_vec(), vec![v])),
        }
    }
    let mut out: Vec<TrendManga> = groups
        .into_iter()
        .map(|(name, aliases, mut vs)| {
            vs.sort_by(|a, b| b.velocity().total_cmp(&a.velocity()));
            let demand: f64 = vs.iter().take(3).map(|v| v.velocity()).sum();
            let total: f64 = vs.iter().map(|v| v.velocity()).sum::<f64>().max(1.0);
            let fresh: f64 = vs.iter().filter(|v| v.age_hours <= 24.0 * 7.0).map(|v| v.velocity()).sum();
            let channels = vs.iter().map(|v| normalize(&v.channel)).collect::<HashSet<_>>().len();
            let found = vs.iter().find_map(|v| v.manga.as_ref());
            TrendManga {
                name,
                aliases: aliases.into_iter().take(5).collect(),
                videos: vs.iter().map(|v| v.id.clone()).collect(),
                demand,
                growth: (fresh / total).max(0.0),
                channels,
                newest_hours: vs.iter().map(|v| v.age_hours).fold(f64::MAX, f64::min),
                score: 0,
                mangadex_id: found.map(|m| m.id.clone()).unwrap_or_default(),
                cover: found.map(|m| m.cover.clone()).unwrap_or_default(),
            }
        })
        .collect();
    let mut demands: Vec<f64> = out.iter().map(|m| m.demand).collect();
    demands.sort_by(f64::total_cmp);
    let n = demands.len().max(1) as f64;
    for m in &mut out {
        let below = demands.iter().filter(|d| **d < m.demand).count() as f64;
        m.score = opportunity((below + 1.0) / n, m.growth, m.channels);
    }
    out.sort_by(|a, b| b.score.cmp(&a.score).then(b.demand.total_cmp(&a.demand)));
    out
}

fn median(mut v: Vec<u64>) -> u64 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    v[v.len() / 2]
}

// ------------------------------------------------------------------------------- network

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().user_agent(UA).timeout(Duration::from_secs(25)).build()?)
}

async fn fetch(http: &reqwest::Client, url: &str) -> Result<String> {
    let r = http
        .get(url)
        .header("Accept-Language", "en-US,en;q=0.9")
        // Skips the EU consent interstitial.
        .header("Cookie", "CONSENT=YES+1; SOCS=CAI")
        .send()
        .await
        .with_context(|| format!("loading {url}"))?;
    if !r.status().is_success() {
        return Err(anyhow!("YouTube answered {} for {url}", r.status()));
    }
    Ok(r.text().await?)
}

fn search_url(query: &str, sp: &str) -> String {
    format!("https://www.youtube.com/results?search_query={}&sp={sp}", url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>())
}

/// Runs [`SEARCHES`], reads the most promising videos' pages, and ranks the manga. Long videos
/// only (Shorts are not recaps); `progress` gets a line per step.
pub async fn crawl(progress: impl Fn(String) + Send + Sync + 'static) -> Result<Trends> {
    let http = client()?;
    let mut tasks = tokio::task::JoinSet::new();
    for (q, sp) in SEARCHES {
        let (http, url) = (http.clone(), search_url(q, sp));
        tasks.spawn(async move { fetch(&http, &url).await.ok().and_then(|h| page_json(&h, "ytInitialData")).map(|d| parse_videos(&d)).unwrap_or_default() });
    }
    let mut by_id: HashMap<String, TrendVideo> = HashMap::new();
    let mut pages = 0;
    while let Some(r) = tasks.join_next().await {
        pages += 1;
        progress(format!("Searched YouTube ({pages}/{})", SEARCHES.len()));
        for v in r.unwrap_or_default() {
            if v.length_secs >= 180 || v.length_secs == 0 {
                by_id.entry(v.id.clone()).or_insert(v);
            }
        }
    }
    if by_id.is_empty() {
        return Err(anyhow!("YouTube returned no recap videos. Check the internet connection and try again."));
    }
    let mut videos: Vec<TrendVideo> = by_id.into_values().collect();
    videos.sort_by(|a, b| b.velocity().total_cmp(&a.velocity()));

    // The fastest recaps get their page read: tags and the full description name the manga.
    let sem = Arc::new(tokio::sync::Semaphore::new(6));
    let mut details = tokio::task::JoinSet::new();
    for (i, v) in videos.iter().take(DETAIL_PAGES).enumerate() {
        let (http, url, sem) = (http.clone(), v.url(), sem.clone());
        details.spawn(async move {
            let _permit = sem.acquire().await;
            (i, fetch(&http, &url).await.ok().and_then(|h| page_json(&h, "ytInitialPlayerResponse")))
        });
    }
    let mut done = 0;
    while let Some(r) = details.join_next().await {
        done += 1;
        progress(format!("Reading recap videos ({done}/{})", DETAIL_PAGES.min(videos.len())));
        if let Ok((i, Some(p))) = r {
            apply_player(&mut videos[i], &p);
        }
    }
    for v in &mut videos {
        v.candidates = candidates(v);
    }
    drop_channel_tags(&mut videos);

    // Only names that are real manga count: each is looked up in the MangaDex catalogue.
    let cache: Arc<std::sync::Mutex<HashMap<String, Option<MangaRef>>>> = Arc::default();
    let sem = Arc::new(tokio::sync::Semaphore::new(3));
    let mut checks = tokio::task::JoinSet::new();
    for (i, v) in videos.iter().enumerate().filter(|(_, v)| !v.candidates.is_empty()) {
        let (http, sem, cache, names) = (http.clone(), sem.clone(), cache.clone(), v.candidates.iter().take(4).cloned().collect::<Vec<_>>());
        checks.spawn(async move {
            for name in names {
                let key = normalize(&name);
                let known = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key).cloned();
                let found = match known {
                    Some(f) => f,
                    None => {
                        let _permit = sem.acquire().await;
                        let f = mangadex_lookup(&http, &name).await;
                        cache.lock().unwrap_or_else(|e| e.into_inner()).insert(key, f.clone());
                        f
                    }
                };
                if found.is_some() {
                    return (i, found);
                }
            }
            (i, None)
        });
    }
    let total = checks.len();
    let mut done = 0;
    let mut confirmed: HashMap<usize, MangaRef> = HashMap::new();
    while let Some(r) = checks.join_next().await {
        done += 1;
        progress(format!("Matching manga ({done}/{total})"));
        if let Ok((i, Some(m))) = r {
            confirmed.insert(i, m);
        }
    }
    for (i, v) in videos.iter_mut().enumerate() {
        match confirmed.remove(&i) {
            Some(m) => {
                v.candidates = std::iter::once(m.name.clone()).chain(m.aliases.iter().cloned()).collect();
                v.manga = Some(m);
            }
            None => v.candidates.clear(),
        }
    }
    let manga = rank(&videos);
    let niche_median_views = median(videos.iter().map(|v| v.views).collect());
    Ok(Trends { fetched_at: chrono::Utc::now().timestamp(), videos, manga, niche_median_views })
}

// ------------------------------------------------------------------------------- channel meter

/// What a channel's public page shows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelPage {
    pub name: String,
    pub handle: String,
    pub avatar: String,
    pub description: String,
    pub keywords: String,
    pub has_banner: bool,
    pub subscribers: Option<u64>,
    pub video_count: Option<u64>,
    /// Newest first.
    pub videos: Vec<TrendVideo>,
}

/// A channel's `/videos` page (`ytInitialData`).
pub fn parse_channel(data: &Value) -> ChannelPage {
    let meta = data.get("metadata").and_then(|m| m.get("channelMetadataRenderer"));
    let s = |k: &str| meta.and_then(|m| m.get(k)).and_then(Value::as_str).unwrap_or("").to_string();
    let avatar = meta
        .and_then(|m| m.get("avatar"))
        .and_then(|a| a.get("thumbnails"))
        .and_then(Value::as_array)
        .and_then(|t| t.first())
        .and_then(|t| t.get("url"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let header = data.get("header").cloned().unwrap_or(Value::Null);
    let mut strings = Vec::new();
    find_all(&header, "content", &mut strings);
    let mut texts: Vec<String> = strings.into_iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
    let mut sub_text = Vec::new();
    find_all(&header, "subscriberCountText", &mut sub_text);
    texts.extend(sub_text.into_iter().map(text));
    let count_of = |word: &str| texts.iter().find(|t| t.to_ascii_lowercase().trim_end().ends_with(word) || t.to_ascii_lowercase().trim_end().ends_with(&format!("{word}s"))).and_then(|t| parse_count(t));
    let handle = texts.iter().find(|t| t.starts_with('@')).cloned().unwrap_or_default();
    let mut banner = Vec::new();
    find_all(&header, "imageBannerViewModel", &mut banner);
    let mut old_banner = Vec::new();
    find_all(&header, "banner", &mut old_banner);
    ChannelPage {
        name: s("title"),
        handle,
        avatar,
        description: s("description"),
        keywords: s("keywords"),
        has_banner: !banner.is_empty() || !old_banner.is_empty(),
        subscribers: count_of("subscriber"),
        video_count: count_of("video"),
        videos: parse_videos(data.get("contents").unwrap_or(&Value::Null)),
    }
}

/// `@handle`, a channel URL or a channel id -> its `/videos` page URL.
pub fn channel_url(input: &str) -> Option<String> {
    let t = input.trim().trim_end_matches('/');
    if t.is_empty() {
        return None;
    }
    let path = if let Some(i) = t.find("youtube.com/") {
        let p = &t[i + "youtube.com/".len()..];
        p.split(['?', '#']).next().unwrap_or(p).trim_end_matches("/videos").trim_end_matches("/featured").to_string()
    } else if t.starts_with("UC") && t.len() == 24 && !t.contains(' ') {
        format!("channel/{t}")
    } else {
        format!("@{}", t.trim_start_matches('@'))
    };
    if path.contains(' ') || path.is_empty() {
        return None;
    }
    Some(format!("https://www.youtube.com/{path}/videos"))
}

pub async fn fetch_channel(input: &str) -> Result<ChannelPage> {
    let url = channel_url(input).ok_or_else(|| anyhow!("Enter your channel as @handle or its link"))?;
    let html = fetch(&client()?, &url).await?;
    let data = page_json(&html, "ytInitialData").ok_or_else(|| anyhow!("Could not read the channel page"))?;
    let page = parse_channel(&data);
    if page.name.is_empty() {
        return Err(anyhow!("No channel found at {url}"));
    }
    Ok(page)
}

/// One improvement for the channel and the points it is worth.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tip {
    pub points: u8,
    pub text: String,
}

/// The channel meter.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ChannelScore {
    /// 0-100.
    pub score: u8,
    /// "Just starting" ... "Thriving".
    pub level: String,
    /// (part, points, out of)
    pub parts: Vec<(String, u8, u8)>,
    /// Biggest gains first.
    pub tips: Vec<Tip>,
}

fn is_recap_title(t: &str) -> bool {
    let l = t.to_lowercase();
    ["recap", "manga", "manhwa", "manhua", "webtoon", "explained"].iter().any(|w| l.contains(w))
}

/// Scores a channel 0-100: profile 25, content 20, consistency 20, performance 25 (median views
/// against `niche_median_views`), niche and titles 10. Every missing point becomes a tip.
pub fn score_channel(c: &ChannelPage, niche_median_views: u64) -> ChannelScore {
    let mut parts = Vec::new();
    let mut tips: Vec<Tip> = Vec::new();
    let mut tip = |points: u8, text: String| {
        if points > 0 {
            tips.push(Tip { points, text });
        }
    };

    // Profile (25).
    let desc = c.description.trim();
    let desc_pts: u8 = if desc.chars().count() >= 150 { 8 } else if desc.is_empty() { 0 } else { 4 };
    tip(8 - desc_pts, "Write a channel description (150+ characters) that says what you post: \"Manga and manhwa recaps, full stories explained, new episodes every week.\" Put the words people search for (manga recap, manhwa recap) in the first line.".into());
    let niche_pts: u8 = if is_recap_title(desc) { 4 } else { 0 };
    tip(4 - niche_pts, "Mention \"manga recap\" and \"manhwa recap\" in the description so YouTube knows which viewers to show you to.".into());
    let kw_pts: u8 = if c.keywords.trim().is_empty() { 0 } else { 5 };
    tip(5 - kw_pts, "Add channel keywords (YouTube Studio › Customization › Basic info): manga recap, manhwa recap, manhua recap, webtoon recap, manga explained.".into());
    let banner_pts: u8 = if c.has_banner { 8 } else { 0 };
    tip(8 - banner_pts, "Add a channel banner that shows your style and upload days (\"New recaps Mon · Wed · Fri\").".into());
    let profile = desc_pts + niche_pts + kw_pts + banner_pts;
    parts.push(("Profile".to_string(), profile, 25));

    // Content (20).
    let count = c.video_count.unwrap_or(c.videos.len() as u64);
    let content: u8 = match count {
        0 => 0,
        1..=4 => 6,
        5..=14 => 12,
        15..=49 => 16,
        _ => 20,
    };
    tip(
        20 - content,
        if count == 0 {
            "Upload your first recaps: start with 3 of the top suggestions below, so new viewers have more than one video to binge.".into()
        } else {
            format!("Keep building the library ({count} videos now): channels with 50+ recaps get far more suggested-video traffic.")
        },
    );
    parts.push(("Videos".to_string(), content, 20));

    // Consistency (20): uploads in the last 30 days.
    let recent = c.videos.iter().filter(|v| v.age_hours <= 24.0 * 30.0).count();
    let consistency: u8 = match recent {
        0 => 0,
        1..=3 => 5,
        4..=7 => 10,
        8..=11 => 15,
        _ => 20,
    };
    tip(20 - consistency, format!("Post on a fixed schedule: 3 recaps a week (12 a month) is the pace that grows recap channels. Last 30 days: {recent}."));
    parts.push(("Consistency".to_string(), consistency, 20));

    // Performance (25): typical views against what recaps in the niche get.
    let settled: Vec<u64> = c.videos.iter().filter(|v| v.age_hours >= 48.0).map(|v| v.views).collect();
    let performance: u8 = if settled.is_empty() {
        0
    } else {
        let ratio = median(settled.clone()) as f64 / niche_median_views.max(1) as f64;
        match ratio {
            r if r >= 1.0 => 25,
            r if r >= 0.3 => 18,
            r if r >= 0.1 => 12,
            r if r >= 0.02 => 6,
            _ => 2,
        }
    };
    tip(
        25 - performance,
        if settled.is_empty() {
            "Views come once there are videos: recap the high-scoring manga below while they are rising, and use the title style of the fastest videos in the Fastest growing tab.".into()
        } else {
            format!(
                "Your typical video gets {} views; recaps in the niche get about {}. Pick higher-scoring manga, put the hook in the first 5 seconds and make thumbnails with one face and 3 big words.",
                median(settled),
                niche_median_views
            )
        },
    );
    parts.push(("Performance".to_string(), performance, 25));

    // Niche and titles (10).
    let titles: Vec<&str> = c.videos.iter().map(|v| v.title.as_str()).collect();
    let niche_and_titles: u8 = if titles.is_empty() {
        0
    } else {
        let on_niche = titles.iter().filter(|t| is_recap_title(t)).count() as f64 / titles.len() as f64;
        let good_len = titles.iter().filter(|t| (35..=75).contains(&t.chars().count())).count() as f64 / titles.len() as f64;
        (5.0 * on_niche + 5.0 * good_len).round() as u8
    };
    tip(10 - niche_and_titles, "Keep every video in the niche and end titles with \"| Manhwa Recap\" or \"| Manga Recap\"; aim for 45-70 characters with the hook first.".into());
    parts.push(("Niche & titles".to_string(), niche_and_titles, 10));

    let score: u8 = parts.iter().map(|p| p.1).sum::<u8>().min(100);
    tips.sort_by(|a, b| b.points.cmp(&a.points));
    let level = match score {
        0..=19 => "Just starting",
        20..=39 => "Building",
        40..=59 => "Growing",
        60..=79 => "Strong",
        _ => "Thriving",
    }
    .to_string();
    ChannelScore { score, level, parts, tips }
}

// ------------------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn counts_and_ages() {
        assert_eq!(parse_count("170,977 views"), Some(170_977));
        assert_eq!(parse_count("1.2M views"), Some(1_200_000));
        assert_eq!(parse_count("12K subscribers"), Some(12_000));
        assert_eq!(parse_count("No views"), Some(0));
        assert_eq!(parse_count("views"), None);
        assert_eq!(parse_age_hours("9h ago"), Some(9.0));
        assert_eq!(parse_age_hours("1d ago"), Some(24.0));
        assert_eq!(parse_age_hours("2 weeks ago"), Some(336.0));
        assert_eq!(parse_age_hours("Streamed 3 days ago"), Some(72.0));
        assert_eq!(parse_age_hours("1 month ago"), Some(720.0));
        assert_eq!(parse_age_hours("30 minutes ago"), Some(0.5));
        assert_eq!(parse_length("1:02:03"), 3723);
    }

    #[test]
    fn finds_the_page_json_with_braces_in_strings() {
        let html = r#"<script>var ytInitialData = {"a":"x } y","b":{"c":1}};</script>"#;
        assert_eq!(page_json(html, "ytInitialData"), Some(json!({"a":"x } y","b":{"c":1}})));
        assert_eq!(page_json("nothing", "ytInitialData"), None);
    }

    #[test]
    fn parses_search_renderers_and_lockups() {
        let data = json!({"contents": [
            {"videoRenderer": {"videoId": "a1", "title": {"runs": [{"text": "He Regressed | Manhwa Recap"}]}, "ownerText": {"runs": [{"text": "Chan"}]},
              "viewCountText": {"simpleText": "9,797 views"}, "publishedTimeText": {"simpleText": "9h ago"}, "lengthText": {"simpleText": "1:00:00"},
              "detailedMetadataSnippets": [{"snippetText": {"runs": [{"text": "Manhwa: "}, {"text": "Solo Leveling"}]}}]}},
            {"videoRenderer": {"videoId": "a1", "publishedTimeText": {"simpleText": "9h ago"}}},
            {"lockupViewModel": {"contentId": "b2", "contentType": "LOCKUP_CONTENT_TYPE_VIDEO", "metadata": {"lockupMetadataViewModel": {
              "title": {"content": "Episode 1"}, "metadata": {"contentMetadataViewModel": {"metadataRows": [{"metadataParts": [{"text": {"content": "1.2K views"}}, {"text": {"content": "3 days ago"}}]}]}}}}}}
        ]});
        let v = parse_videos(&data);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].id.as_str(), v[0].views, v[0].age_hours, v[0].length_secs, v[0].channel.as_str()), ("a1", 9797, 9.0, 3600, "Chan"));
        assert_eq!(v[0].description, "Manhwa: Solo Leveling");
        assert_eq!((v[1].id.as_str(), v[1].views, v[1].age_hours), ("b2", 1200, 72.0));
    }

    fn video(id: &str, channel: &str, views: u64, hours: f64, keywords: &[&str], desc: &str) -> TrendVideo {
        let mut v = TrendVideo {
            id: id.into(),
            title: "He Became OP | Manhwa Recap".into(),
            channel: channel.into(),
            views,
            age_hours: hours,
            keywords: keywords.iter().map(|s| s.to_string()).collect(),
            description: desc.into(),
            ..Default::default()
        };
        v.candidates = candidates(&v);
        v
    }

    #[test]
    fn manga_names_come_from_labels_then_tags() {
        let v = video("1", "BrielVC", 1, 1.0, &["Trapped in a Crazy World as an Extra", "manhwa recap", "manhwa ch 1-51", "isekai manhwa", "BrielVC"], "Hook!\nManhwa: Extra of a Crazy World | ch 1-51");
        assert_eq!(v.candidates, vec!["Extra of a Crazy World", "Trapped in a Crazy World as an Extra"]);
        let none = video("2", "X", 1, 1.0, &["manga recap", "anime explained"], "Subscribe!");
        assert!(none.candidates.is_empty());
        // A tag one channel puts on every upload is not about any one of them.
        let mut vs: Vec<TrendVideo> = (0..3).map(|i| video(&i.to_string(), "Spam", 1, 1.0, &["High School Mercenary", &format!("Own Title {i}")], "")).collect();
        drop_channel_tags(&mut vs);
        assert_eq!(vs[0].candidates, vec!["Own Title 0"]);
    }

    #[test]
    fn ranks_by_demand_growth_and_competition() {
        let vs = vec![
            video("a", "A", 900_000, 24.0 * 5.0, &["Solo Leveling Ragnarok"], ""),
            video("b", "B", 100_000, 24.0 * 3.0, &["solo leveling: ragnarok"], ""),
            video("c", "C", 5_000, 24.0 * 25.0, &["Quiet Little Manga"], ""),
        ];
        let m = rank(&vs);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].name, "Solo Leveling Ragnarok");
        assert_eq!(m[0].videos, vec!["a", "b"]);
        assert_eq!(m[0].channels, 2);
        assert!(m[0].score > m[1].score);
        assert_eq!(opportunity(1.0, 1.0, 1), 100);
        assert!(opportunity(1.0, 1.0, 6) < opportunity(1.0, 1.0, 2));
    }

    #[test]
    fn mangadex_confirms_names_and_gives_covers() {
        let json = json!({"data": [
            {"id": "other", "attributes": {"title": {"en": "Something Else"}, "altTitles": []}, "relationships": []},
            {"id": "md1", "attributes": {"title": {"ko-ro": "Iphak Yongbyeong"}, "altTitles": [{"ko": "입학용병"}, {"en": "Teenage Mercenary"}, {"en": "Mercenary Enrollment"}]},
             "relationships": [{"type": "author"}, {"type": "cover_art", "attributes": {"fileName": "c.jpg"}}]}
        ]});
        let m = parse_mangadex(&json, "mercenary enrollment").unwrap();
        assert_eq!((m.id.as_str(), m.name.as_str()), ("md1", "Mercenary Enrollment"));
        assert_eq!(m.aliases, vec!["Iphak Yongbyeong", "Teenage Mercenary"]);
        assert_eq!(m.cover, "https://uploads.mangadex.org/covers/md1/c.jpg.256.jpg");
        assert!(parse_mangadex(&json, "manhwa hindi dubbed").is_none());
    }

    #[test]
    fn similarity_matches_variants() {
        assert_eq!(similarity("Solo Leveling", "solo leveling"), 1.0);
        assert!(similarity("Solo Leveling", "Solo Leveling: Ragnarok") >= 0.85);
        assert!(similarity("The Extra", "Return of the Mount Hua Sect") < 0.5);
    }

    #[test]
    fn channel_urls() {
        assert_eq!(channel_url("@MyChan").as_deref(), Some("https://www.youtube.com/@MyChan/videos"));
        assert_eq!(channel_url("MyChan").as_deref(), Some("https://www.youtube.com/@MyChan/videos"));
        assert_eq!(channel_url("https://www.youtube.com/@MyChan/featured").as_deref(), Some("https://www.youtube.com/@MyChan/videos"));
        assert_eq!(channel_url("UCvGdkyPR3c_078ukpgno9sA").as_deref(), Some("https://www.youtube.com/channel/UCvGdkyPR3c_078ukpgno9sA/videos"));
        assert_eq!(channel_url("  "), None);
    }

    #[test]
    fn an_empty_channel_scores_low_with_tips() {
        let s = score_channel(&ChannelPage { name: "Chan".into(), ..Default::default() }, 50_000);
        assert!(s.score < 10, "{}", s.score);
        assert_eq!(s.level, "Just starting");
        assert!(s.tips.len() >= 5);
        assert!(s.tips.windows(2).all(|w| w[0].points >= w[1].points));
        let busy = ChannelPage {
            name: "Chan".into(),
            description: "Manga recap and manhwa recap channel. ".repeat(5),
            keywords: "manga recap".into(),
            has_banner: true,
            video_count: Some(60),
            videos: (0..14).map(|i| TrendVideo { title: "He Became the Strongest Hunter | Manhwa Recap".into(), views: 80_000, age_hours: 50.0 + i as f64 * 40.0, ..Default::default() }).collect(),
            ..Default::default()
        };
        assert_eq!(score_channel(&busy, 50_000).score, 100);
    }
}
