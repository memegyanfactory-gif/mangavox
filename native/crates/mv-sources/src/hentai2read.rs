//! Hentai2Read: an Inertia.js app, every page embeds its props as JSON in
//! `<script data-page="app" type="application/json">` (port of `hentai2read-native.ts`).
//!
//! Manga id = url path without the leading slash (`doujinshi/4967-chou-love-ru`);
//! chapter id = `reader/{slug}/{chapter_id}`.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use mv_net::Opts;
use regex::Regex;
use scraper::Html;
use serde_json::Value;

use crate::source::{Result, Source, SourceCtx, SourceError};
use crate::types::{ChapterInfo, MangaDetails, MangaSearchResult, SourceInfo, Tag};
use crate::util::{abs_url, encode_component, sel, svg_icon};

pub const ID: &str = "hentai2read";
const BASE: &str = "https://hentai2read.com";
const CACHE_TTL: Duration = Duration::from_secs(120);

static CHAPTER_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^/(reader/[^/]+/\d+)").unwrap());
static TAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());

pub struct Hentai2Read {
    ctx: SourceCtx,
    cache: Mutex<HashMap<String, (Instant, Value)>>,
}

impl Hentai2Read {
    pub fn new(ctx: SourceCtx) -> Self {
        Self { ctx, cache: Mutex::new(HashMap::new()) }
    }

    pub fn source_info() -> SourceInfo {
        SourceInfo {
            id: ID.into(),
            name: "Hentai2Read".into(),
            version: "2.0.0".into(),
            lang: "en".into(),
            nsfw: true,
            requires_wall_solver: false,
            icon: Some(svg_icon("#c0392b", "H2R", 11)),
        }
    }

    /// Fetch a page and return its Inertia `props` (details + chapters share one fetch via a short cache).
    async fn props(&self, path: &str, fresh: bool) -> Result<Value> {
        let url = if path.starts_with("http") { path.to_string() } else { format!("{BASE}{path}") };
        if !fresh {
            if let Some((t, v)) = self.cache.lock().unwrap().get(&url) {
                if t.elapsed() < CACHE_TTL {
                    return Ok(v.clone());
                }
            }
        }
        let opts = Opts::new().header("Accept", "text/html,application/xhtml+xml");
        let r = self.ctx.fetch(ID, &url, &opts).await?;
        if r.status == 404 {
            return Err(SourceError::NotFound(format!("Hentai2Read: not found ({path})")));
        }
        if r.status >= 400 {
            return Err(SourceError::Other(format!("Hentai2Read: HTTP {} for {path}", r.status)));
        }
        let props = parse_props(&r.text()).map_err(|e| match e {
            SourceError::Other(m) => SourceError::Other(format!("{m} ({path})")),
            e => e,
        })?;
        let mut cache = self.cache.lock().unwrap();
        if cache.len() >= 16 {
            cache.clear();
        }
        cache.insert(url, (Instant::now(), props.clone()));
        Ok(props)
    }
}

/// Extract `props` from the embedded Inertia page JSON. Error pages become errors.
pub fn parse_props(html: &str) -> Result<Value> {
    let doc = Html::parse_document(html);
    let script = doc
        .select(&sel("script[data-page=\"app\"]"))
        .next()
        .ok_or_else(|| SourceError::Parse("Hentai2Read: page data not found (site layout changed or a challenge page was served)".into()))?;
    let raw: String = script.text().collect();
    let page: Value = serde_json::from_str(&raw).map_err(|e| SourceError::Parse(format!("Hentai2Read: page data is not valid JSON: {e}")))?;
    if page.get("component").and_then(Value::as_str) == Some("Error") {
        let title = page.pointer("/props/title").and_then(Value::as_str).unwrap_or("error page");
        return Err(SourceError::Other(format!("Hentai2Read: {title}")));
    }
    page.get("props").cloned().ok_or_else(|| SourceError::Parse("Hentai2Read: page has no props".into()))
}

fn abs(u: Option<&str>) -> Option<String> {
    u.and_then(|u| abs_url(BASE, u))
}

/// `props.books.data` of search / browse / tag pages.
pub fn map_books(props: &Value) -> Vec<MangaSearchResult> {
    props
        .pointer("/books/data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|b| MangaSearchResult {
            source_id: b.get("url").and_then(Value::as_str).unwrap_or("").trim_start_matches('/').to_string(),
            title: b.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
            cover_url: abs(b.get("cover").and_then(Value::as_str)),
            ..Default::default()
        })
        .filter(|b| !b.source_id.is_empty() && !b.title.is_empty())
        .collect()
}

fn group_names(book: &Value, ty: &str) -> Vec<String> {
    book.get("groups")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|g| g.get("type").and_then(Value::as_str) == Some(ty))
        .flat_map(|g| g.get("tags").and_then(Value::as_array).into_iter().flatten())
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// Book page props -> details.
pub fn parse_details(props: &Value, manga_id: &str) -> Result<MangaDetails> {
    let b = props.get("book").filter(|b| b.is_object()).ok_or_else(|| SourceError::Parse("Hentai2Read: gallery data missing".into()))?;
    let artists = group_names(b, "artist");
    let status = b.pointer("/status/label").and_then(Value::as_str).unwrap_or("").to_lowercase();
    let author = if !artists.is_empty() {
        Some(artists.join(", "))
    } else {
        group_names(b, "author").into_iter().next().or_else(|| group_names(b, "circle").into_iter().next())
    };
    let description = match b.get("description").and_then(Value::as_str).filter(|d| !d.is_empty()) {
        Some(d) => Some(TAGS.replace_all(d, "").trim().to_string()).filter(|s| !s.is_empty()),
        None => b.get("alt").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string),
    };
    Ok(MangaDetails {
        source_id: manga_id.to_string(),
        title: b.get("title").and_then(Value::as_str).filter(|t| !t.is_empty()).unwrap_or("Untitled").to_string(),
        author,
        artist: (!artists.is_empty()).then(|| artists.join(", ")),
        description,
        cover_url: abs(b.get("cover").and_then(Value::as_str)),
        status: Some(if status.contains("complete") { "completed" } else if status.contains("ongoing") { "ongoing" } else { "unknown" }.into()),
        genres: Some(b.get("tags").and_then(Value::as_array).into_iter().flatten().filter_map(|t| t.get("name").and_then(Value::as_str)).map(str::to_string).collect()),
    })
}

/// Book page props -> chapters, ascending.
pub fn parse_chapters(props: &Value, manga_id: &str) -> Vec<ChapterInfo> {
    let slug = manga_id.rsplit('/').next().unwrap_or(manga_id);
    let mut out: Vec<ChapterInfo> = props
        .get("chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(i, c)| {
            let url = c.get("url").and_then(Value::as_str).unwrap_or("");
            let id_field = c.get("id").map(|v| v.to_string().trim_matches('"').to_string()).unwrap_or_default();
            let number_s = c.get("number").map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).unwrap_or_default();
            let number = number_s.trim().parse::<f64>().ok().unwrap_or((i + 1) as f64);
            let title = match c.get("title").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                Some(t) => format!("Chapter {number_s}: {t}"),
                None => format!("Chapter {number_s}"),
            };
            ChapterInfo {
                source_id: CHAPTER_PATH.captures(url).map(|m| m[1].to_string()).unwrap_or_else(|| format!("reader/{slug}/{id_field}")),
                number: Some(number),
                title: Some(title),
                language: "en".into(),
                published_at: c.get("date").and_then(Value::as_str).filter(|d| !d.is_empty()).map(str::to_string),
                ..Default::default()
            }
        })
        .collect();
    out.sort_by(|a, b| a.number.partial_cmp(&b.number).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Reader page props -> absolute image URLs.
pub fn parse_images(props: &Value) -> Vec<String> {
    props.get("images").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).filter_map(|u| abs_url(BASE, u)).collect()
}

/// `/tags/category` props -> tags (id = url path without leading slash).
pub fn parse_tags(props: &Value) -> Vec<Tag> {
    props
        .pointer("/tags/data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let url = t.get("url").and_then(Value::as_str)?;
            let name = t.get("name").and_then(Value::as_str)?;
            (!url.is_empty() && !name.is_empty()).then(|| Tag { id: url.trim_start_matches('/').to_string(), name: name.to_string() })
        })
        .collect()
}

fn page_path(base: &str, page: u32) -> String {
    if page > 1 {
        format!("{base}/page-{page}")
    } else {
        base.to_string()
    }
}

#[async_trait]
impl Source for Hentai2Read {
    fn info(&self) -> SourceInfo {
        Self::source_info()
    }

    async fn search_manga(&self, query: &str, page: u32) -> Result<Vec<MangaSearchResult>> {
        let extra = if page > 1 { format!("&page={page}") } else { String::new() };
        Ok(map_books(&self.props(&format!("/search?q={}{extra}", encode_component(query)), false).await?))
    }

    async fn get_popular_manga(&self, page: u32) -> Result<Vec<MangaSearchResult>> {
        Ok(map_books(&self.props(&page_path("/browse/most-viewed", page), false).await?))
    }

    async fn get_latest_manga(&self, page: u32) -> Result<Vec<MangaSearchResult>> {
        Ok(map_books(&self.props(&page_path("/browse", page), false).await?))
    }

    async fn get_manga_details(&self, manga_id: &str) -> Result<MangaDetails> {
        parse_details(&self.props(&format!("/{}", manga_id.trim_start_matches('/')), false).await?, manga_id)
    }

    async fn get_chapter_list(&self, manga_id: &str) -> Result<Vec<ChapterInfo>> {
        Ok(parse_chapters(&self.props(&format!("/{}", manga_id.trim_start_matches('/')), false).await?, manga_id))
    }

    async fn get_page_urls(&self, chapter_id: &str) -> Result<Vec<String>> {
        let id = chapter_id.trim_start_matches('/');
        let imgs = parse_images(&self.props(&format!("/{id}/page-1"), true).await?);
        if imgs.is_empty() {
            return Err(SourceError::Parse("Hentai2Read: no page images found for this chapter".into()));
        }
        Ok(imgs)
    }

    async fn get_tag_list(&self) -> Result<Vec<Tag>> {
        let tags = parse_tags(&self.props("/tags/category", false).await?);
        if tags.is_empty() {
            return Ok(vec![
                Tag { id: "category/8-big-breasts".into(), name: "Big Breasts".into() },
                Tag { id: "category/2-anal".into(), name: "Anal".into() },
            ]);
        }
        Ok(tags)
    }

    async fn get_manga_by_tag(&self, tag_id: &str, page: u32) -> Result<Vec<MangaSearchResult>> {
        let id = if tag_id.contains('/') { tag_id.trim_start_matches('/').to_string() } else { format!("category/{tag_id}") };
        Ok(map_books(&self.props(&page_path(&format!("/{id}"), page), false).await?))
    }

    fn download_headers(&self) -> Vec<(String, String)> {
        vec![("Referer".into(), "https://hentai2read.com/".into())]
    }
}
