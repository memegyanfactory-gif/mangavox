//! Real YouTube videos to show next to the user's own: recaps of the same manga and of similar
//! manga (its genres, its characters from the script) that are doing well, read from public
//! YouTube pages with the YouTube package's scraper (`scrape`: search results, and the related
//! column of the best recap's own watch page, which is what YouTube itself suggests next to it).
//! Saved per manga in `studio/<manga>/youtube_related.json` and read again after a day.
use std::path::PathBuf;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::super::logic::{self, parse_age_days, parse_views, Matcher, Video};
use super::super::scrape;

/// Shorts and clips are not what a recap competes with.
const MIN_SECS: u64 = 180;
/// Recaps scoring this or more are about the same manga.
const SAME: f32 = 0.6;

/// One real video as YouTube shows it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct YtVideo {
    pub id: String,
    pub title: String,
    pub channel: String,
    /// The channel's avatar (search results only).
    pub avatar: String,
    pub verified: bool,
    pub views: u64,
    /// "2 months ago".
    pub published: String,
    pub age_days: Option<u32>,
    /// "1:02:03".
    pub length: String,
    /// Best thumbnail URL YouTube gave.
    pub thumb: String,
    pub snippet: String,
    /// About the same manga (else a similar one).
    pub same: bool,
}

impl YtVideo {
    pub fn url(&self) -> String {
        format!("https://www.youtube.com/watch?v={}", self.id)
    }

    /// Views per day since upload: the traction the list is sorted by.
    pub fn traction(&self) -> f64 {
        self.views as f64 / self.age_days.unwrap_or(30).max(1) as f64
    }

    fn secs(&self) -> u64 {
        self.length.split(':').filter_map(|p| p.trim().parse::<u64>().ok()).fold(0, |a, p| a * 60 + p)
    }

    fn as_video(&self) -> Video {
        Video { id: self.id.clone(), title: self.title.clone(), channel: self.channel.clone(), snippet: self.snippet.clone(), views: self.views, ..Default::default() }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Related {
    /// Best first: same manga by traction, then similar manga.
    pub videos: Vec<YtVideo>,
    pub fetched_at: u64,
}

impl Related {
    fn path(manga_id: &str) -> PathBuf {
        mv_recap::Brain::dir(manga_id).join("youtube_related.json")
    }
    pub fn load(manga_id: &str) -> Option<Related> {
        std::fs::read(Self::path(manga_id)).ok().and_then(|b| serde_json::from_slice(&b).ok())
    }
    pub fn save(&self, manga_id: &str) {
        let p = Self::path(manga_id);
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(b) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(p, b);
        }
    }
}

/// Manga whose related videos are being searched right now (one search per manga at a time,
/// whichever page asked).
static RUNNING: std::sync::Mutex<Option<std::collections::HashSet<String>>> = std::sync::Mutex::new(None);

/// Claims the search for `manga_id`; false when one is already running.
pub fn start(manga_id: &str) -> bool {
    RUNNING.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(manga_id.to_string())
}

pub fn finish(manga_id: &str) {
    if let Some(s) = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        s.remove(manga_id);
    }
}

pub fn running(manga_id: &str) -> bool {
    RUNNING.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|s| s.contains(manga_id))
}

/// What the related videos are picked by.
#[derive(Debug, Clone, Default)]
pub struct Brief {
    pub title: String,
    /// Other titles of the manga (from the YouTube package's research).
    pub aliases: Vec<String>,
    pub genres: Vec<String>,
    /// Lead characters' names, from the script.
    pub cast: Vec<String>,
}

/// The search queries for a manga: its recaps, its hero, and recaps of its genre.
pub fn queries(b: &Brief) -> Vec<String> {
    let mut q = vec![format!("{} recap", b.title)];
    if let Some(a) = b.aliases.iter().find(|a| a.is_ascii() && logic::normalize(a) != logic::normalize(&b.title)) {
        q.push(format!("{a} manhwa recap"));
    }
    if let Some(hero) = b.cast.first() {
        q.push(format!("{hero} {}", b.title));
    }
    let genres: Vec<&str> = b.genres.iter().map(String::as_str).filter(|g| !matches!(g.to_lowercase().as_str(), "manhwa" | "manga" | "manhua" | "webtoon" | "full color" | "long strip")).take(2).collect();
    q.push(if genres.is_empty() { "manhwa recap".to_string() } else { format!("{} manhwa recap", genres.join(" ")) });
    q
}

fn thumb_of(thumbs: Option<&Value>) -> String {
    thumbs.and_then(Value::as_array).and_then(|a| a.iter().filter_map(|t| t.get("url").and_then(Value::as_str)).last()).unwrap_or("").to_string()
}

/// A search result (`videoRenderer`).
fn from_renderer(r: &Value) -> Option<YtVideo> {
    let id = r.get("videoId")?.as_str()?.to_string();
    let views_text = r.get("viewCountText").map(scrape::text).unwrap_or_default();
    if views_text.contains("watching") {
        return None;
    }
    let published = r.get("publishedTimeText").map(scrape::text).unwrap_or_default();
    let snippet = r
        .get("detailedMetadataSnippets")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|s| s.get("snippetText")).map(scrape::text).collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    let verified = r.get("ownerBadges").and_then(Value::as_array).is_some_and(|b| b.iter().any(|x| x.pointer("/metadataBadgeRenderer/style").and_then(Value::as_str).is_some_and(|s| s.contains("VERIFIED"))));
    Some(YtVideo {
        title: r.get("title").map(scrape::text).unwrap_or_default(),
        channel: r.get("ownerText").or_else(|| r.get("longBylineText")).map(scrape::text).unwrap_or_default(),
        avatar: thumb_of(r.pointer("/channelThumbnailSupportedRenderers/channelThumbnailWithLinkRenderer/thumbnail/thumbnails")),
        verified,
        views: parse_views(&views_text)?,
        age_days: parse_age_days(&published),
        published,
        length: r.get("lengthText").map(scrape::text).unwrap_or_default(),
        thumb: thumb_of(r.pointer("/thumbnail/thumbnails")),
        snippet,
        id,
        same: false,
    })
}

/// A watch page's related column (`lockupViewModel`): channel on the first metadata row, views
/// and age on the second.
fn from_lockup(l: &Value) -> Option<YtVideo> {
    if l.get("contentType").and_then(Value::as_str) != Some("LOCKUP_CONTENT_TYPE_VIDEO") {
        return None;
    }
    let meta = l.pointer("/metadata/lockupMetadataViewModel")?;
    let rows = meta.pointer("/metadata/contentMetadataViewModel/metadataRows").and_then(Value::as_array)?;
    let parts = |r: &Value| -> Vec<(String, String)> {
        r.get("metadataParts")
            .and_then(Value::as_array)
            .map(|ps| ps.iter().map(|p| (p.pointer("/text/content").and_then(Value::as_str).unwrap_or("").to_string(), p.get("accessibilityLabel").and_then(Value::as_str).unwrap_or("").to_string())).collect())
            .unwrap_or_default()
    };
    let channel = rows.first().map(parts).and_then(|p| p.into_iter().next()).map(|p| p.0).unwrap_or_default();
    let second = rows.get(1).map(parts).unwrap_or_default();
    let views = second.iter().find_map(|(t, a)| [a, t].into_iter().find(|s| s.to_lowercase().contains("view")).and_then(|s| parse_views(s)))?;
    let published = second.iter().map(|p| p.0.clone()).find(|s| s.contains("ago")).unwrap_or_default();
    let mut badges = vec![];
    scrape::collect(l, "thumbnailBadgeViewModel", &mut badges);
    let length = badges.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).find(|t| t.contains(':')).unwrap_or("").to_string();
    let verified = meta.to_string().contains("CHECK_CIRCLE_FILLED");
    Some(YtVideo {
        id: l.get("contentId")?.as_str()?.to_string(),
        title: meta.pointer("/title/content").and_then(Value::as_str).unwrap_or("").to_string(),
        channel,
        verified,
        views,
        age_days: parse_age_days(&published),
        published,
        length,
        thumb: thumb_of(l.pointer("/contentImage/thumbnailViewModel/image/sources")),
        ..Default::default()
    })
}

/// Every video on a search or watch page, in page order.
pub fn videos_in(data: &Value) -> Vec<YtVideo> {
    let (mut rs, mut ls) = (vec![], vec![]);
    scrape::collect(data, "videoRenderer", &mut rs);
    scrape::collect(data, "lockupViewModel", &mut ls);
    rs.into_iter().filter_map(from_renderer).chain(ls.into_iter().filter_map(from_lockup)).collect()
}

async fn search(q: &str) -> Result<Vec<YtVideo>> {
    let html = scrape::page(&format!("https://www.youtube.com/results?search_query={}&sp=EgIQAQ%3D%3D&hl=en&gl=US", scrape::enc(q))).await?;
    Ok(scrape::initial_data(&html).map(|d| videos_in(&d)).unwrap_or_default())
}

async fn watch_related(id: &str) -> Result<Vec<YtVideo>> {
    let html = scrape::page(&format!("https://www.youtube.com/watch?v={id}&hl=en&gl=US")).await?;
    Ok(scrape::initial_data(&html).map(|d| videos_in(&d)).unwrap_or_default().into_iter().filter(|v| v.id != id).collect())
}

/// Teaches the Brain what does well around this manga.
fn learn(videos: &[YtVideo]) {
    let seen: Vec<mv_core::brain::SeenVideo> = videos.iter().map(|v| mv_core::brain::SeenVideo { title: &v.title, channel: &v.channel, views: v.views, age_days: v.age_days, length_secs: v.secs() }).collect();
    mv_core::brain::learn_from_videos(&seen, "YouTube videos (related)");
}

/// Sorts found videos into the related list: long uploads only, no repeats, the same manga first
/// (best traction first), then similar manga.
pub fn rank(found: Vec<YtVideo>, b: &Brief) -> Vec<YtVideo> {
    let matcher = Matcher { manga: vec![(b.title.clone(), std::iter::once(b.title.clone()).chain(b.aliases.iter().cloned()).collect(), b.cast.clone())] };
    let mut seen = std::collections::HashSet::new();
    let mut v: Vec<YtVideo> = found
        .into_iter()
        .filter(|v| !v.title.is_empty() && (v.length.is_empty() || v.secs() >= MIN_SECS))
        .filter(|v| seen.insert(v.id.clone()))
        .map(|mut v| {
            v.same = matcher.best(&v.as_video()).1 >= SAME;
            v
        })
        .collect();
    // What the Brain learned does well (title words, channels) lifts a video a little.
    // Recaps are what the user's video competes with: long uploads and recap-worded titles first,
    // then official clips and edits. What the Brain learned does well lifts a video a little.
    let score = |v: &YtVideo| {
        let t = v.title.to_lowercase();
        let recap = v.secs() >= 20 * 60 || ["recap", "story", "explained", "summary", "full", "manhwa", "manga"].iter().any(|w| t.contains(w));
        v.traction() * if recap { 4. } else { 1. } * (1. + 0.5 * mv_core::brain::boost(&v.title, &v.channel) as f64)
    };
    v.sort_by(|a, b| b.same.cmp(&a.same).then(score(b).total_cmp(&score(a))));
    v
}

/// Finds the related videos for a manga. Blocking network work: run it in the background.
pub async fn fetch(b: Brief) -> Result<Related> {
    let qs = queries(&b);
    let results = futures::future::join_all(qs.iter().map(|q| search(q))).await;
    let failed = results.iter().filter(|r| r.is_err()).count();
    if failed == results.len() {
        bail!("could not reach YouTube (offline, or YouTube blocked the request)");
    }
    let mut found: Vec<YtVideo> = results.into_iter().flatten().flatten().collect();
    // What YouTube itself shows next to the best recap of this manga.
    let first = rank(found.clone(), &b).into_iter().find(|v| v.same);
    if let Some(top) = first {
        found.extend(watch_related(&top.id).await.unwrap_or_default());
    }
    let videos = rank(found, &b);
    if videos.is_empty() {
        // The pages loaded but held no videos: a consent or bot page, or YouTube changed them.
        bail!("YouTube answered, but no videos could be read from its pages");
    }
    learn(&videos);
    Ok(Related { videos, fetched_at: super::super::now() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn v(id: &str, title: &str, views: u64, age: u32, len: &str) -> YtVideo {
        YtVideo { id: id.into(), title: title.into(), views, age_days: Some(age), length: len.into(), ..Default::default() }
    }

    #[test]
    fn queries_use_titles_hero_and_genres() {
        let b = Brief { title: "Solo Leveling".into(), aliases: vec!["Na Honjaman Level Up".into(), "나 혼자만 레벨업".into()], genres: vec!["Action".into(), "Manhwa".into(), "Fantasy".into()], cast: vec!["Sung Jinwoo".into()] };
        assert_eq!(queries(&b), vec!["Solo Leveling recap", "Na Honjaman Level Up manhwa recap", "Sung Jinwoo Solo Leveling", "Action Fantasy manhwa recap"]);
    }

    #[test]
    fn rank_puts_the_same_manga_first_and_drops_shorts() {
        let b = Brief { title: "Solo Leveling".into(), ..Default::default() };
        let found = vec![
            v("a", "Top 10 OP manhwa", 900_000, 10, "12:00"),
            v("b", "Solo Leveling recap part 1", 50_000, 50, "1:02:00"),
            v("c", "Solo Leveling full recap", 400_000, 20, "3:10:00"),
            v("d", "Solo Leveling #shorts", 5_000_000, 2, "0:45"),
            v("c", "Solo Leveling full recap", 400_000, 20, "3:10:00"),
        ];
        let ids: Vec<String> = rank(found, &b).into_iter().map(|v| v.id).collect();
        assert_eq!(ids, vec!["c", "b", "a"]);
    }

    #[test]
    fn reads_search_results_and_watch_page_lockups() {
        let data = json!({"contents": [
            {"videoRenderer": {"videoId": "x1", "title": {"runs": [{"text": "Solo Leveling Recap"}]}, "ownerText": {"runs": [{"text": "Recap Guy"}]},
              "viewCountText": {"simpleText": "1,234,567 views"}, "publishedTimeText": {"simpleText": "3 weeks ago"}, "lengthText": {"simpleText": "2:01:05"},
              "thumbnail": {"thumbnails": [{"url": "small.jpg"}, {"url": "big.jpg"}]},
              "ownerBadges": [{"metadataBadgeRenderer": {"style": "BADGE_STYLE_TYPE_VERIFIED"}}],
              "channelThumbnailSupportedRenderers": {"channelThumbnailWithLinkRenderer": {"thumbnail": {"thumbnails": [{"url": "avatar.jpg"}]}}}}},
            {"lockupViewModel": {"contentId": "y2", "contentType": "LOCKUP_CONTENT_TYPE_VIDEO",
              "contentImage": {"thumbnailViewModel": {"image": {"sources": [{"url": "y.jpg"}]}, "overlays": [{"thumbnailBottomOverlayViewModel": {"badges": [{"thumbnailBadgeViewModel": {"text": "11:56:00"}}]}}]}},
              "metadata": {"lockupMetadataViewModel": {"title": {"content": "Next Recap"}, "metadata": {"contentMetadataViewModel": {"metadataRows": [
                {"metadataParts": [{"text": {"content": "Ninja Recaps"}}]},
                {"metadataParts": [{"text": {"content": "134K views"}}, {"text": {"content": "2 months ago"}}]}]}}}}}}
        ]});
        let vs = videos_in(&data);
        assert_eq!(vs.len(), 2);
        assert_eq!((vs[0].views, vs[0].age_days, vs[0].thumb.as_str(), vs[0].avatar.as_str(), vs[0].verified), (1_234_567, Some(21), "big.jpg", "avatar.jpg", true));
        assert_eq!((vs[1].channel.as_str(), vs[1].views, vs[1].length.as_str(), vs[1].age_days), ("Ninja Recaps", 134_000, "11:56:00", Some(60)));
    }

    /// Hits YouTube: `cargo test -p ui-studio live_related -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_related() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let b = Brief { title: "Solo Leveling".into(), genres: vec!["Action".into(), "Fantasy".into()], cast: vec!["Sung Jinwoo".into()], ..Default::default() };
        let r = rt.block_on(fetch(b)).unwrap();
        for v in r.videos.iter().take(25) {
            println!("{} {:>9} {:>8} {:<10} {:<24} {} | {}", if v.same { "*" } else { " " }, v.views, v.length, v.published, v.channel, v.title, v.avatar.len());
        }
        assert!(r.videos.iter().filter(|v| v.same).count() >= 3);
    }
}
