//! Reading YouTube for the competitor tracker: a polite client (250-700 ms between requests, three
//! in flight at most, stops when YouTube pushes back) and pure parsers over the pages' JSON.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use rand::Rng;
use serde_json::{json, Value};

use crate::timing::parse_publish;
use crate::trends::{client, find_all, page_json, parse_channel, parse_count, text, video_from_renderer, ChannelPage, TrendVideo};

/// Why a request failed.
#[derive(Clone, Debug, PartialEq)]
pub enum Fail {
    /// Rate limit, captcha or consent page: the run should stop.
    Blocked(String),
    Other(String),
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fail::Blocked(s) | Fail::Other(s) => f.write_str(s),
        }
    }
}

/// The shared client of one run. Cheap to clone.
#[derive(Clone)]
pub struct Net {
    http: reqwest::Client,
    blocked: Arc<AtomicBool>,
    pages: Arc<AtomicUsize>,
    sem: Arc<tokio::sync::Semaphore>,
}

impl Net {
    pub fn new() -> Result<Self> {
        Ok(Self { http: client()?, blocked: Arc::default(), pages: Arc::default(), sem: Arc::new(tokio::sync::Semaphore::new(3)) })
    }

    /// True once YouTube asked the run to stop.
    pub fn blocked(&self) -> bool {
        self.blocked.load(Ordering::Relaxed)
    }

    /// Requests sent so far.
    pub fn pages(&self) -> usize {
        self.pages.load(Ordering::Relaxed)
    }

    /// Waits for a slot and the polite gap; `Err` when the run was stopped meanwhile.
    async fn slot(&self) -> Result<tokio::sync::OwnedSemaphorePermit, Fail> {
        if self.blocked() {
            return Err(Fail::Blocked("stopped".into()));
        }
        let permit = self.sem.clone().acquire_owned().await.map_err(|e| Fail::Other(e.to_string()))?;
        let ms = rand::thread_rng().gen_range(250..=700);
        tokio::time::sleep(Duration::from_millis(ms)).await;
        if self.blocked() {
            return Err(Fail::Blocked("stopped".into()));
        }
        self.pages.fetch_add(1, Ordering::Relaxed);
        Ok(permit)
    }

    fn check(&self, r: &reqwest::Response) -> Result<(), Fail> {
        let consent = r.url().host_str().is_some_and(|h| h.starts_with("consent.")) || r.url().path().starts_with("/sorry");
        if r.status().as_u16() == 429 || consent {
            self.blocked.store(true, Ordering::Relaxed);
            return Err(Fail::Blocked(format!("YouTube answered {} for {}", r.status(), r.url().host_str().unwrap_or("youtube.com"))));
        }
        if !r.status().is_success() {
            return Err(Fail::Other(format!("YouTube answered {} for {}", r.status(), r.url())));
        }
        Ok(())
    }

    fn get(&self, url: &str) -> reqwest::RequestBuilder {
        // The cookie skips the EU consent interstitial.
        self.http.get(url).header("Accept-Language", "en-US,en;q=0.9").header("Cookie", "CONSENT=YES+1; SOCS=CAI")
    }

    pub async fn text(&self, url: &str) -> Result<String, Fail> {
        let _slot = self.slot().await?;
        let r = self.get(url).send().await.map_err(|e| Fail::Other(e.to_string()))?;
        self.check(&r)?;
        r.text().await.map_err(|e| Fail::Other(e.to_string()))
    }

    pub async fn bytes(&self, url: &str) -> Result<Vec<u8>, Fail> {
        let _slot = self.slot().await?;
        let r = self.get(url).send().await.map_err(|e| Fail::Other(e.to_string()))?;
        self.check(&r)?;
        r.bytes().await.map(|b| b.to_vec()).map_err(|e| Fail::Other(e.to_string()))
    }

    async fn post_json(&self, url: &str, body: &Value) -> Result<Value, Fail> {
        let _slot = self.slot().await?;
        let r = self.http.post(url).header("Cookie", "CONSENT=YES+1; SOCS=CAI").json(body).send().await.map_err(|e| Fail::Other(e.to_string()))?;
        self.check(&r)?;
        r.json().await.map_err(|e| Fail::Other(e.to_string()))
    }
}

pub fn watch_url(id: &str) -> String {
    format!("https://www.youtube.com/watch?v={id}")
}

pub fn thumb_url(id: &str) -> String {
    format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg")
}

// ------------------------------------------------------------------------------- search

/// A video in a search result with its uploader.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hit {
    pub video: TrendVideo,
    pub channel_id: String,
    pub handle: String,
    pub channel: String,
}

/// The videos of a search page with their channel ids and handles.
pub fn search_hits(data: &Value) -> Vec<Hit> {
    let mut rs = Vec::new();
    find_all(data, "videoRenderer", &mut rs);
    let mut out: Vec<Hit> = Vec::new();
    for r in rs {
        let Some(video) = video_from_renderer(r) else { continue };
        let owner = r.get("ownerText").or_else(|| r.get("longBylineText"));
        let endpoint = owner.and_then(|o| o.pointer("/runs/0/navigationEndpoint/browseEndpoint"));
        let s = |k: &str| endpoint.and_then(|e| e.get(k)).and_then(Value::as_str).unwrap_or("").to_string();
        let handle = s("canonicalBaseUrl").rsplit('/').next().filter(|h| h.starts_with('@')).unwrap_or("").to_string();
        let hit = Hit { channel: video.channel.clone(), channel_id: s("browseId"), handle, video };
        if !hit.channel_id.is_empty() && !out.iter().any(|h| h.video.id == hit.video.id) {
            out.push(hit);
        }
    }
    out
}

// ------------------------------------------------------------------------------- channels

/// The `UC...` id of a channel page.
pub fn channel_id(data: &Value) -> Option<String> {
    data.pointer("/metadata/channelMetadataRenderer/externalId").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

/// A channel's keywords string: `"manga recap" manhwa "solo leveling"` -> three keywords.
pub fn parse_keywords(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted) = (Vec::new(), String::new(), false);
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        let k = cur.trim().to_string();
        if !k.is_empty() && !out.contains(&k) {
            out.push(k);
        }
        cur.clear();
    };
    for c in s.chars() {
        match c {
            '"' => {
                flush(&mut cur, &mut out);
                quoted = !quoted;
            }
            c if c.is_whitespace() && !quoted => flush(&mut cur, &mut out),
            c => cur.push(c),
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// What a channel's about data adds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct About {
    pub country: String,
    pub joined: String,
    pub total_views: Option<u64>,
}

impl About {
    fn is_empty(&self) -> bool {
        self.country.is_empty() && self.joined.is_empty() && self.total_views.is_none()
    }
}

/// Country, join date and total views from the about data: the `aboutChannelViewModel` (current
/// pages) or the older `channelAboutFullMetadataRenderer`.
pub fn parse_about(data: &Value) -> About {
    let mut about = About::default();
    for key in ["aboutChannelViewModel", "channelAboutFullMetadataRenderer"] {
        let mut found = Vec::new();
        find_all(data, key, &mut found);
        for m in found {
            if about.country.is_empty() {
                about.country = m.get("country").map(text).unwrap_or_default();
            }
            if about.joined.is_empty() {
                let j = m.get("joinedDateText").map(text).unwrap_or_default();
                about.joined = j.trim().trim_start_matches("Joined").trim().to_string();
            }
            if about.total_views.is_none() {
                about.total_views = m.get("viewCountText").map(text).and_then(|s| parse_count(&s));
            }
        }
    }
    about
}

/// Every string under the page header, in page order.
fn header_strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| header_strings(x, out)),
        Value::Object(m) => m.values().for_each(|x| header_strings(x, out)),
        _ => {}
    }
}

/// The subscriber and video counts and the handle from the page header when `parse_channel` found
/// none (the current header nests them under a `content` object).
pub fn fill_header(page: &mut ChannelPage, data: &Value) {
    let mut all = Vec::new();
    header_strings(data.get("header").unwrap_or(&Value::Null), &mut all);
    // The spoken labels ("646 thousand subscribers") would read as 646.
    all.retain(|s| !["thousand", "million", "billion"].iter().any(|w| s.contains(w)));
    let ends = |s: &str, w: &str| { let l = s.trim().to_ascii_lowercase(); l.ends_with(w) || l.ends_with(&format!("{w}s")) };
    if page.subscribers.is_none() {
        page.subscribers = all.iter().filter(|s| s.len() < 40 && ends(s, "subscriber")).find_map(|s| parse_count(s));
    }
    if page.video_count.is_none() {
        page.video_count = all.iter().filter(|s| s.len() < 40 && ends(s, "video")).find_map(|s| parse_count(s));
    }
    if page.handle.is_empty() {
        page.handle = all.iter().find(|s| s.starts_with('@') && !s.contains(' ')).cloned().unwrap_or_default();
    }
}

/// The channel's `/videos` page with its id, plus its about data when `want_about`.
pub async fn scan_channel(net: &Net, id: &str, want_about: bool) -> Result<(ChannelPage, Option<About>), Fail> {
    let html = net.text(&format!("https://www.youtube.com/channel/{id}/videos")).await?;
    let data = page_json(&html, "ytInitialData").ok_or_else(|| Fail::Other("Could not read the channel page".into()))?;
    let mut page = parse_channel(&data);
    fill_header(&mut page, &data);
    if page.name.is_empty() {
        return Err(Fail::Other(format!("No channel at {id}")));
    }
    if page.handle.is_empty() {
        page.handle = data.pointer("/metadata/channelMetadataRenderer/vanityChannelUrl").and_then(Value::as_str).and_then(|u| u.rsplit('/').next()).filter(|h| h.starts_with('@')).unwrap_or("").to_string();
    }
    let about = if want_about { fetch_about(net, id).await } else { None };
    Ok((page, about))
}

/// Best effort: the `/about` page, then the continuation it points to. `None` when unreachable.
async fn fetch_about(net: &Net, id: &str) -> Option<About> {
    let html = net.text(&format!("https://www.youtube.com/channel/{id}/about")).await.ok()?;
    let data = page_json(&html, "ytInitialData")?;
    let about = parse_about(&data);
    if !about.is_empty() {
        return Some(about);
    }
    let mut tokens = Vec::new();
    find_all(&data, "continuationCommand", &mut tokens);
    let token = tokens.iter().find_map(|t| t.get("token").and_then(Value::as_str))?;
    let key = html.split("\"INNERTUBE_API_KEY\":\"").nth(1).and_then(|r| r.split('"').next()).unwrap_or("");
    let url = if key.is_empty() { "https://www.youtube.com/youtubei/v1/browse?prettyPrint=false".to_string() } else { format!("https://www.youtube.com/youtubei/v1/browse?prettyPrint=false&key={key}") };
    let body = json!({ "context": { "client": { "clientName": "WEB", "clientVersion": "2.20241030.01.00", "hl": "en", "gl": "US" } }, "continuation": token });
    let about = parse_about(&net.post_json(&url, &body).await.ok()?);
    (!about.is_empty()).then_some(about)
}

// ------------------------------------------------------------------------------- watch pages

/// What a watch page tells about one video.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Watch {
    /// Exact publish time, unix seconds.
    pub published: Option<i64>,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub duration: u64,
    pub category: String,
    pub views: Option<u64>,
    pub likes: Option<u64>,
    pub comments: Option<u64>,
}

/// From `ytInitialPlayerResponse` (details and publish time) and, when read, `ytInitialData`
/// (likes and comment count).
pub fn parse_watch_page(player: &Value, data: Option<&Value>) -> Option<Watch> {
    let d = player.get("videoDetails")?;
    let m = player.pointer("/microformat/playerMicroformatRenderer");
    let s = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let published = m.and_then(|m| m.get("publishDate").or_else(|| m.get("uploadDate"))).and_then(Value::as_str).and_then(parse_publish);
    let likes = m.and_then(|m| m.get("likeCount")).and_then(Value::as_str).and_then(|s| s.parse().ok()).or_else(|| data.and_then(likes_of));
    Some(Watch {
        published,
        title: s("title"),
        description: s("shortDescription"),
        tags: d.get("keywords").and_then(Value::as_array).map(|k| k.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default(),
        duration: s("lengthSeconds").parse().unwrap_or(0),
        category: m.and_then(|m| m.get("category")).and_then(Value::as_str).unwrap_or("").to_string(),
        views: s("viewCount").parse().ok(),
        likes,
        comments: data.and_then(comments_of),
    })
}

/// The like count in the page data: the like button's number, or its "like this video along with
/// N other people" label.
fn likes_of(data: &Value) -> Option<u64> {
    let mut v = Vec::new();
    find_all(data, "likeCountIfIndifferentNumber", &mut v);
    if let Some(n) = v.iter().filter_map(|x| x.as_str()).find_map(parse_count) {
        return Some(n);
    }
    let mut labels = Vec::new();
    for key in ["accessibilityText", "label"] {
        find_all(data, key, &mut labels);
    }
    labels.iter().filter_map(|l| l.as_str()).find_map(|l| {
        let rest = l.split("like this video along with ").nth(1)?;
        parse_count(rest).map(|n| n + 1)
    })
}

/// The comment count: the comments entry point's `commentCount` or the header's count.
fn comments_of(data: &Value) -> Option<u64> {
    let mut v = Vec::new();
    find_all(data, "commentCount", &mut v);
    if let Some(n) = v.iter().find_map(|x| parse_count(&text(x))) {
        return Some(n);
    }
    let mut h = Vec::new();
    find_all(data, "commentsHeaderRenderer", &mut h);
    h.iter().find_map(|x| x.get("countText").and_then(|c| parse_count(&text(c))))
}

/// Reads a watch page.
pub async fn read_watch(net: &Net, id: &str) -> Result<Watch, Fail> {
    let html = net.text(&watch_url(id)).await?;
    let player = page_json(&html, "ytInitialPlayerResponse").ok_or_else(|| Fail::Other("No player data on the video page".into()))?;
    let data = page_json(&html, "ytInitialData");
    parse_watch_page(&player, data.as_ref()).ok_or_else(|| Fail::Other("No video details on the page".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_with_quoted_phrases() {
        assert_eq!(parse_keywords(r#""manga recap" manhwa "solo leveling" manhwa"#), vec!["manga recap", "manhwa", "solo leveling"]);
        assert!(parse_keywords("  ").is_empty());
    }

    #[test]
    fn search_results_carry_the_channel() {
        let data = json!({ "contents": [
            { "videoRenderer": {
                "videoId": "abc123", "title": { "runs": [{ "text": "Manhwa Recap: Tower" }] },
                "ownerText": { "runs": [{ "text": "Recap Guy", "navigationEndpoint": { "browseEndpoint": { "browseId": "UC1234567890123456789012", "canonicalBaseUrl": "/@recapguy" } } }] },
                "viewCountText": { "simpleText": "12,345 views" }, "publishedTimeText": { "simpleText": "5 hours ago" }, "lengthText": { "simpleText": "12:30" } } },
            { "videoRenderer": { "videoId": "nolive", "title": { "simpleText": "no age" } } }
        ]});
        let hits = search_hits(&data);
        assert_eq!(hits.len(), 1);
        let h = &hits[0];
        assert_eq!((h.channel_id.as_str(), h.handle.as_str(), h.channel.as_str()), ("UC1234567890123456789012", "@recapguy", "Recap Guy"));
        assert_eq!((h.video.views, h.video.length_secs), (12_345, 750));
    }

    #[test]
    fn about_from_the_view_model_and_the_old_renderer() {
        let new = json!({ "x": { "aboutChannelViewModel": { "country": "Japan", "joinedDateText": { "content": "Joined Mar 5, 2021" }, "viewCountText": "1,234,567 views" } } });
        let a = parse_about(&new);
        assert_eq!((a.country.as_str(), a.joined.as_str(), a.total_views), ("Japan", "Mar 5, 2021", Some(1_234_567)));
        let old = json!({ "channelAboutFullMetadataRenderer": { "country": { "simpleText": "India" }, "joinedDateText": { "runs": [{ "text": "Joined " }, { "text": "Jan 1, 2020" }] } } });
        let a = parse_about(&old);
        assert_eq!((a.country.as_str(), a.joined.as_str(), a.total_views), ("India", "Jan 1, 2020", None));
        assert!(parse_about(&json!({})).is_empty());
    }

    #[test]
    fn counts_nested_under_the_header_content() {
        let data = json!({ "header": { "pageHeaderRenderer": { "content": { "pageHeaderViewModel": { "metadata": { "rows": [
            { "parts": [{ "text": { "content": "@recaps" } }] },
            { "parts": [{ "text": { "content": "646K subscribers" }, "accessibilityLabel": "646 thousand subscribers" }, { "text": { "content": "1,234 videos" } }] } ] } } } } } });
        let mut page = ChannelPage::default();
        fill_header(&mut page, &data);
        assert_eq!((page.subscribers, page.video_count, page.handle.as_str()), (Some(646_000), Some(1234), "@recaps"));
        let mut known = ChannelPage { subscribers: Some(5), ..Default::default() };
        fill_header(&mut known, &data);
        assert_eq!(known.subscribers, Some(5));
    }

    #[test]
    fn channel_id_from_metadata() {
        let d = json!({ "metadata": { "channelMetadataRenderer": { "externalId": "UCabc" } } });
        assert_eq!(channel_id(&d).as_deref(), Some("UCabc"));
        assert_eq!(channel_id(&json!({})), None);
    }

    #[test]
    fn watch_page_details() {
        let player = json!({
            "videoDetails": { "videoId": "v", "title": "T", "shortDescription": "D", "keywords": ["a", "b"], "lengthSeconds": "600", "viewCount": "4200" },
            "microformat": { "playerMicroformatRenderer": { "publishDate": "2026-10-29T15:09:51-07:00", "category": "Entertainment" } }
        });
        let data = json!({
            "a": { "likeCountIfIndifferentNumber": "1,234" },
            "b": { "commentsEntryPointHeaderRenderer": { "commentCount": { "simpleText": "56" } } }
        });
        let w = parse_watch_page(&player, Some(&data)).unwrap();
        assert_eq!(w.published, Some(1_793_311_791));
        assert_eq!((w.title.as_str(), w.description.as_str(), w.duration, w.views), ("T", "D", 600, Some(4200)));
        assert_eq!((w.tags.len(), w.category.as_str(), w.likes, w.comments), (2, "Entertainment", Some(1234), Some(56)));
        let bare = parse_watch_page(&player, None).unwrap();
        assert_eq!((bare.likes, bare.comments), (None, None));
        assert!(parse_watch_page(&json!({}), None).is_none());
    }

    #[test]
    fn likes_from_the_accessibility_label() {
        let data = json!({ "button": { "accessibilityText": "like this video along with 999 other people" } });
        assert_eq!(likes_of(&data), Some(1000));
        let header = json!({ "commentsHeaderRenderer": { "countText": { "runs": [{ "text": "1,024" }, { "text": " Comments" }] } } });
        assert_eq!(comments_of(&header), Some(1024));
    }
}
