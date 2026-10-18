//! ComicK: JSON API on api.comick.dev, page images from the `sv-data` script on comick.live
//! (port of `comick-native.ts`).
//!
//! The API sits behind Cloudflare. Requests go out over reqwest first; when a wall / 403 / 503
//! answers, the attached [`crate::WallSolver`] supplies clearance cookies (and in-browser fetches
//! as a last resort). Without a solver a wall surfaces as `SourceError::Wall`.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use mv_net::Opts;
use scraper::Html;
use serde_json::Value;

use crate::source::{Result, Source, SourceCtx, SourceError};
use crate::types::{ChapterInfo, MangaDetails, MangaSearchResult, SourceInfo, Tag};
use crate::util::{encode_component, sel, svg_icon};

pub const ID: &str = "comick";
const API: &str = "https://api.comick.dev";
const WEB: &str = "https://comick.live";
/// Cover host for `b2key` covers.
const CDN: &str = "https://meo.comick.pictures";

pub struct Comick {
    ctx: SourceCtx,
}

impl Comick {
    pub fn new(ctx: SourceCtx) -> Self {
        Self { ctx }
    }

    pub fn source_info() -> SourceInfo {
        SourceInfo {
            id: ID.into(),
            name: "ComicK".into(),
            version: "4.0.0".into(),
            lang: "en".into(),
            nsfw: false,
            requires_wall_solver: false,
            icon: Some(svg_icon("#FF6B35", "CMK", 11)),
        }
    }

    async fn api(&self, path: &str) -> Result<Value> {
        let url = if path.starts_with("http") { path.to_string() } else { format!("{API}/{}", path.trim_start_matches('/')) };
        let opts = Opts::new().header("Accept", "application/json").referer(format!("{WEB}/"));
        self.ctx.fetch_json(ID, &url, &opts).await
    }
}

fn cover_of(c: &Value) -> Option<String> {
    let key = c
        .pointer("/md_covers/0/b2key")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| c.get("cover_url").and_then(Value::as_str).filter(|s| !s.is_empty()))?;
    Some(if key.starts_with("http") { key.to_string() } else { format!("{CDN}/{key}") })
}

pub fn map_comic(c: &Value) -> MangaSearchResult {
    let s = |k: &str| c.get(k).and_then(Value::as_str).filter(|s| !s.is_empty());
    MangaSearchResult {
        source_id: s("hid").or_else(|| s("slug")).unwrap_or("").to_string(),
        title: s("title").or_else(|| s("slug")).unwrap_or("").to_string(),
        cover_url: cover_of(c),
        ..Default::default()
    }
}

/// `v1.0/search` and `top`: a bare array, or `{comics|results|rank: [...]}`.
pub fn parse_comic_list(json: &Value) -> Vec<MangaSearchResult> {
    let list = json
        .as_array()
        .or_else(|| ["comics", "results", "rank"].iter().find_map(|k| json.get(*k).and_then(Value::as_array)));
    list.into_iter().flatten().map(map_comic).filter(|r| !r.source_id.is_empty()).collect()
}

/// `chapter?...` (latest uploads): unique comics of each chapter row.
pub fn parse_latest(json: &Value) -> Vec<MangaSearchResult> {
    let mut seen = std::collections::HashSet::new();
    json.as_array()
        .into_iter()
        .flatten()
        .filter_map(|ch| ch.get("md_comics").or_else(|| ch.get("comic")).filter(|c| c.is_object()))
        .map(map_comic)
        .filter(|r| !r.source_id.is_empty() && seen.insert(r.source_id.clone()))
        .collect()
}

fn names(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|a| a.get("name").and_then(Value::as_str).or_else(|| a.as_str()))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// `comic/{hid}?tachiyomi=true`
pub fn parse_details(json: &Value, manga_id: &str) -> MangaDetails {
    let comic = json.get("comic").filter(|c| c.is_object()).unwrap_or(json);
    let genres: Vec<String> = comic
        .get("md_comic_md_genres")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|g| g.pointer("/md_genres/name").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    let mut authors = names(json.get("authors"));
    if authors.is_empty() {
        authors = names(comic.get("authors").or_else(|| comic.get("md_comic_staff")));
    }
    let artists = names(json.get("artists"));
    let s = |k: &str| comic.get(k).and_then(Value::as_str).filter(|s| !s.is_empty());
    MangaDetails {
        source_id: manga_id.to_string(),
        title: s("title").or_else(|| s("slug")).unwrap_or("Untitled").to_string(),
        author: (!authors.is_empty()).then(|| authors.join(", ")),
        artist: (!artists.is_empty()).then(|| artists.join(", ")),
        description: s("desc").or_else(|| s("description")).map(str::to_string),
        cover_url: cover_of(comic),
        status: Some(
            match comic.get("status").and_then(Value::as_i64) {
                Some(1) => "ongoing",
                Some(2) => "completed",
                _ => "unknown",
            }
            .into(),
        ),
        genres: Some(genres),
    }
}

fn chap_number(c: &Value) -> Option<f64> {
    c.get("chap").and_then(Value::as_str).and_then(|s| s.trim().parse::<f64>().ok()).filter(|n| n.is_finite())
}

fn is_official(c: &Value) -> bool {
    let by_group = c
        .get("md_chapters_groups")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|g| g.pointer("/md_groups/official").and_then(Value::as_bool) == Some(true));
    let by_name = c.get("group_name").and_then(Value::as_array).into_iter().flatten().any(|g| g.as_str() == Some("Official"));
    by_group || by_name
}

fn score(c: &Value) -> f64 {
    let readable = !(c.get("external").map_or(false, |e| !e.is_null() && e != &Value::Bool(false)) || is_official(c));
    (if readable { 1e9 } else { 0.0 }) + c.get("up_count").and_then(Value::as_f64).unwrap_or(0.0)
}

/// Raw chapter rows -> one entry per chapter number (prefers readable groups over official /
/// external ones, then upvotes), ascending; numberless extras last.
pub fn build_chapter_list(raw: &[Value]) -> Vec<ChapterInfo> {
    let mut best: HashMap<String, &Value> = HashMap::new();
    let mut extras: Vec<&Value> = Vec::new();
    for ch in raw {
        match chap_number(ch) {
            None => extras.push(ch),
            Some(n) => {
                let k = n.to_string();
                if best.get(&k).map_or(true, |cur| score(ch) > score(cur)) {
                    best.insert(k, ch);
                }
            }
        }
    }
    let mut numbered: Vec<&Value> = best.into_values().collect();
    numbered.sort_by(|a, b| chap_number(a).partial_cmp(&chap_number(b)).unwrap_or(std::cmp::Ordering::Equal));
    let to_info = |ch: &Value| {
        let n = chap_number(ch);
        let vol = ch.get("vol").and_then(Value::as_str).filter(|v| !v.is_empty()).map(|v| format!("Vol. {v} ")).unwrap_or_default();
        let title = ch
            .get("title")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .or_else(|| n.is_none().then(|| format!("{vol}Extra").trim().to_string()));
        ChapterInfo {
            source_id: ch.get("hid").and_then(Value::as_str).unwrap_or("").to_string(),
            number: n,
            title,
            language: "en".into(),
            scanlation_group: ch.pointer("/group_name/0").and_then(Value::as_str).map(str::to_string),
            published_at: ["publish_at", "created_at"].iter().find_map(|k| ch.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty())).map(str::to_string),
            page_count: None,
        }
    };
    numbered.into_iter().chain(extras).map(to_info).collect()
}

/// comick.live chapter page: image URLs from `<script id="sv-data">`.
pub fn parse_sv_data_images(html: &str) -> Vec<String> {
    let doc = Html::parse_document(html);
    let Some(script) = doc.select(&sel("script#sv-data")).next() else { return Vec::new() };
    let raw: String = script.text().collect();
    let Ok(data) = serde_json::from_str::<Value>(&raw) else { return Vec::new() };
    data.pointer("/chapter/images")
        .or_else(|| data.get("images"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|i| i.get("url").and_then(Value::as_str))
        .filter(|u| !u.is_empty())
        .map(str::to_string)
        .collect()
}

/// `chapter/{hid}` API record -> `(chap, lang, md_images)` used to build the web URL / fallback.
pub fn parse_chapter_record(json: &Value) -> (String, String, Vec<String>) {
    let c = json.get("chapter").unwrap_or(json);
    let chap = c.get("chap").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("1").to_string();
    let lang = c.get("lang").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("en").to_string();
    let imgs = ["md_images", "images"]
        .iter()
        .find_map(|k| c.get(*k).or_else(|| json.get(*k)).and_then(Value::as_array))
        .into_iter()
        .flatten()
        .filter_map(|i| i.get("b2key").or_else(|| i.get("url")).or_else(|| i.get("name")).and_then(Value::as_str))
        .map(|k| if k.starts_with("http") { k.to_string() } else { format!("{CDN}/{k}") })
        .collect();
    (chap, lang, imgs)
}

#[async_trait]
impl Source for Comick {
    fn info(&self) -> SourceInfo {
        Self::source_info()
    }

    async fn search_manga(&self, query: &str, page: u32) -> Result<Vec<MangaSearchResult>> {
        let j = self.api(&format!("v1.0/search?q={}&page={}&limit=30&tachiyomi=true", encode_component(query), page.max(1))).await?;
        Ok(parse_comic_list(&j))
    }

    async fn get_popular_manga(&self, page: u32) -> Result<Vec<MangaSearchResult>> {
        let j = self.api(&format!("top?page={}&limit=30&comic_types=manga&duration=7&tachiyomi=true", page.max(1))).await?;
        Ok(parse_comic_list(&j))
    }

    async fn get_latest_manga(&self, page: u32) -> Result<Vec<MangaSearchResult>> {
        let j = self.api(&format!("chapter?page={}&limit=30&lang=en&tachiyomi=true", page.max(1))).await?;
        Ok(parse_latest(&j))
    }

    async fn get_manga_details(&self, manga_id: &str) -> Result<MangaDetails> {
        Ok(parse_details(&self.api(&format!("comic/{manga_id}?tachiyomi=true")).await?, manga_id))
    }

    async fn get_chapter_list(&self, manga_id: &str) -> Result<Vec<ChapterInfo>> {
        let mut raw: Vec<Value> = Vec::new();
        for page in 1..40 {
            let j = self.api(&format!("comic/{manga_id}/chapters?lang=en&limit=300&page={page}&chap-order=1")).await?;
            let chapters = j.get("chapters").and_then(Value::as_array).cloned().unwrap_or_default();
            let n = chapters.len();
            raw.extend(chapters);
            if n < 300 {
                break;
            }
        }
        Ok(build_chapter_list(&raw))
    }

    async fn get_page_urls(&self, chapter_id: &str) -> Result<Vec<String>> {
        // 1) API chapter record: chap/lang for the web URL, maybe image keys.
        let (mut chap, mut lang, mut api_imgs) = ("1".to_string(), "en".to_string(), Vec::new());
        if let Ok(Ok(j)) = tokio::time::timeout(Duration::from_secs(15), self.api(&format!("chapter/{chapter_id}?tachiyomi=true"))).await {
            (chap, lang, api_imgs) = parse_chapter_record(&j);
        }
        // 2) comick.live HTML (sv-data): primary source of working image URLs. The slug part of the
        //    URL is ignored by the site; the hid prefix identifies the chapter.
        let url = format!("{WEB}/comic/c/{chapter_id}-chapter-{}-{lang}", encode_component(&chap));
        let opts = Opts::new().header("Accept", "text/html,application/xhtml+xml").timeout(Duration::from_secs(15));
        if let Ok(html) = self.ctx.fetch_text(ID, &url, &opts).await {
            let imgs = parse_sv_data_images(&html);
            if !imgs.is_empty() {
                return Ok(imgs);
            }
        }
        if !api_imgs.is_empty() {
            return Ok(api_imgs);
        }
        Err(SourceError::Other("No pages available for this ComicK chapter (it may be external/licensed)".into()))
    }

    async fn get_tag_list(&self) -> Result<Vec<Tag>> {
        match self.api("genre?tachiyomi=true").await {
            Ok(j) => Ok(j
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|g| {
                    let id = g.get("id").map(|v| v.to_string().trim_matches('"').to_string()).or_else(|| g.get("slug").and_then(Value::as_str).map(str::to_string))?;
                    let name = g.get("name").or_else(|| g.get("slug")).and_then(Value::as_str)?.to_string();
                    Some(Tag { id, name })
                })
                .collect()),
            Err(_) => Ok(["Action", "Adventure", "Comedy", "Drama", "Fantasy", "Horror", "Mystery", "Romance", "Sci-fi", "Slice of Life"]
                .iter()
                .enumerate()
                .map(|(i, n)| Tag { id: (i + 1).to_string(), name: n.to_string() })
                .collect()),
        }
    }

    async fn get_manga_by_tag(&self, tag_id: &str, page: u32) -> Result<Vec<MangaSearchResult>> {
        let j = self.api(&format!("v1.0/search?page={}&limit=30&genres={}&tachiyomi=true", page.max(1), encode_component(tag_id))).await?;
        Ok(parse_comic_list(&j))
    }

    fn download_headers(&self) -> Vec<(String, String)> {
        vec![("Referer".into(), format!("{WEB}/"))]
    }
}
