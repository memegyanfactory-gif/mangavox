//! e-hentai: server-rendered HTML for listings, the official `api.php` (gdata) for metadata,
//! per-image viewer pages for the real image URLs (port of `ehentai-native.ts`).
//!
//! Gallery ids are `"{gid}/{token}"`; a gallery is a single chapter with the same id.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use mv_net::Opts;
use regex::Regex;
use scraper::Html;
use serde::Deserialize;
use serde_json::json;

use crate::source::{Result, Source, SourceCtx, SourceError};
use crate::types::{ChapterInfo, MangaDetails, MangaSearchResult, SourceInfo, Tag};
use crate::util::{decode_entities, encode_component, sel, svg_icon, text_of};

pub const ID: &str = "ehentai";
const BASE: &str = "https://e-hentai.org";
const API: &str = "https://api.e-hentai.org/api.php";
/// nw=1 skips the "Offensive For Everyone" interstitial.
const COOKIE: &str = "nw=1; sl=dm_2";

static GALLERY_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/g/(\d+)/([0-9a-f]{10})/?$").unwrap());
static VIEWER_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"href="(https?://e[-x]hentai\.org/s/[0-9a-f]+/\d+-\d+)""#).unwrap());
static COVER_HOST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"ehgt\.org|exhentai|hath\.network|/t/|/w/").unwrap());

pub struct EHentai {
    ctx: SourceCtx,
}

impl EHentai {
    pub fn new(ctx: SourceCtx) -> Self {
        // Be gentle: e-hentai bans IPs that hammer it.
        ctx.http.set_host_interval("e-hentai.org", Duration::from_millis(300));
        ctx.http.set_host_interval("api.e-hentai.org", Duration::from_millis(300));
        let _ = ctx.http.add_cookie_header(BASE, COOKIE);
        let _ = ctx.http.add_cookie_header(API, COOKIE);
        Self { ctx }
    }

    pub fn source_info() -> SourceInfo {
        SourceInfo {
            id: ID.into(),
            name: "e-hentai".into(),
            version: "2.0.0".into(),
            lang: "en".into(),
            nsfw: true,
            requires_wall_solver: false,
            icon: Some(svg_icon("#4a9d5b", "EH", 13)),
        }
    }

    async fn html(&self, url: &str) -> Result<String> {
        let opts = Opts::new().header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8").cookie(COOKIE);
        let mut last: Option<SourceError> = None;
        for attempt in 0..3u64 {
            match self.ctx.fetch(ID, url, &opts).await {
                Ok(r) => {
                    let text = r.text();
                    // Definitive answers are not retried.
                    return check_blocked(r.status, &text, url).map(|_| text);
                }
                Err(e) => {
                    let retry = matches!(e, SourceError::Net(_));
                    last = Some(e);
                    if !retry {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(600 * (attempt + 1))).await;
                }
            }
        }
        Err(last.unwrap_or_else(|| SourceError::Other("e-hentai: request failed".into())))
    }

    async fn list_page(&self, path: &str) -> Result<Vec<MangaSearchResult>> {
        let html = self.html(&format!("{BASE}{path}")).await?;
        let res = parse_gallery_list(&html);
        if res.is_empty() && !Regex::new(r"No hits found|No unfiltered results|class=.itg|Showing 0").unwrap().is_match(&html) {
            return Err(SourceError::Parse("e-hentai: could not read the gallery list (unexpected page layout)".into()));
        }
        Ok(res)
    }

    async fn gdata(&self, gallery_id: &str) -> Result<GData> {
        let (gid, token) = gallery_id.split_once('/').ok_or_else(|| SourceError::Parse(format!("e-hentai: bad gallery id {gallery_id:?}")))?;
        let gid: u64 = gid.parse().map_err(|_| SourceError::Parse(format!("e-hentai: bad gallery id {gallery_id:?}")))?;
        let body = json!({ "method": "gdata", "gidlist": [[gid, token]], "namespace": 1 });
        let r = self.ctx.http.post_json(API, &body, &Opts::new().cookie(COOKIE)).await?;
        let status = r.status;
        let text = r.text();
        parse_gdata(&text).map_err(|e| match e {
            SourceError::Parse(m) => SourceError::Parse(format!("{m} (HTTP {status}: {})", text.chars().take(120).collect::<String>())),
            e => e,
        })
    }
}

/// Human-readable errors for e-hentai's block / ban / sad-panda pages.
pub fn check_blocked(status: u16, text: &str, url: &str) -> Result<()> {
    let err = |m: String| Err(SourceError::Other(m));
    if status == 404 {
        return Err(SourceError::NotFound(format!("e-hentai: gallery not found or removed ({url})")));
    }
    if matches!(status, 403 | 503 | 429) {
        return err(format!("e-hentai: request refused (HTTP {status}). Your IP may be rate limited or temporarily banned"));
    }
    if status >= 400 {
        return err(format!("e-hentai: HTTP {status} for {url}"));
    }
    let head: String = text.chars().take(600).collect();
    if text.contains("IP address has been temporarily banned") {
        let when = Regex::new(r"(?i)ban expires in ([^<.]+)").unwrap().captures(text).map(|c| format!(" (expires in {})", c[1].trim())).unwrap_or_default();
        return err(format!("e-hentai: your IP is temporarily banned for excessive requests{when}. Try again later"));
    }
    let lower = text.to_ascii_lowercase();
    if (lower.contains("gallery not available") || lower.contains("this gallery has been removed") || lower.contains("has been taken down"))
        && !text.contains("id=\"gdt\"")
    {
        return err("e-hentai: this gallery is not available (removed or deleted)".into());
    }
    let hl = head.to_ascii_lowercase();
    if hl.contains("content warning") && hl.contains("offensive for everyone") && !text.contains("class=\"itg") && !text.contains("id=\"gdt\"") {
        return err("e-hentai: content warning page was not bypassed".into());
    }
    if text.len() < 400 && !lower.contains("<html") {
        return err("e-hentai: received an empty/\"sad panda\" response (access denied for this client)".into());
    }
    Ok(())
}

/// Gallery listing in any display mode (compact / extended `<tr>`, thumbnail `div.gl1t`).
pub fn parse_gallery_list(html: &str) -> Vec<MangaSearchResult> {
    let doc = Html::parse_document(html);
    let chunk_sel = sel("tr, div.gl1t");
    let a_sel = sel("a[href]");
    let glink = sel("div.glink");
    let img_sel = sel("img");
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for chunk in doc.select(&chunk_sel) {
        let Some((id, _)) = chunk.select(&a_sel).find_map(|a| {
            let href = a.value().attr("href")?;
            let c = GALLERY_LINK.captures(href)?;
            if !(href.contains("e-hentai.org") || href.contains("exhentai.org")) {
                return None;
            }
            Some((format!("{}/{}", &c[1], &c[2]), ()))
        }) else {
            continue;
        };
        if seen.contains(&id) {
            continue;
        }
        let mut title = chunk.select(&glink).next().map(text_of).unwrap_or_default();
        if title.is_empty() {
            title = chunk
                .select(&img_sel)
                .find_map(|i| i.value().attr("title").or_else(|| i.value().attr("alt")).filter(|t| !t.is_empty()))
                .unwrap_or("")
                .to_string();
        }
        if title.is_empty() {
            continue;
        }
        let cover = chunk.select(&img_sel).find_map(|i| {
            let v = i.value();
            let cand = v.attr("data-src").or_else(|| v.attr("src").filter(|s| !s.starts_with("data:")))?;
            (COVER_HOST.is_match(cand) && !cand.contains("/g/t.png")).then(|| cand.to_string())
        });
        seen.insert(id.clone());
        out.push(MangaSearchResult { source_id: id, title, cover_url: cover, ..Default::default() });
    }
    out
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct GData {
    #[serde(default)]
    pub gid: u64,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub title_jpn: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub thumb: String,
    #[serde(default)]
    pub uploader: String,
    #[serde(default)]
    pub filecount: String,
    #[serde(default)]
    pub rating: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// `api.php` gdata response -> first gallery.
pub fn parse_gdata(body: &str) -> Result<GData> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|_| SourceError::Parse("e-hentai: metadata API returned no data".into()))?;
    let g = v.pointer("/gmetadata/0").ok_or_else(|| SourceError::Parse("e-hentai: metadata API returned no data".into()))?;
    let g: GData = serde_json::from_value(g.clone()).map_err(|e| SourceError::Parse(format!("e-hentai: unexpected metadata shape: {e}")))?;
    if let Some(e) = &g.error {
        return Err(SourceError::Other(format!("e-hentai: gallery unavailable ({e})")));
    }
    Ok(g)
}

fn bare(t: &str) -> String {
    match t.split_once(':') {
        Some((ns, rest)) if ns.chars().all(|c| c.is_ascii_lowercase()) => rest.to_string(),
        _ => t.to_string(),
    }
}

fn by_ns(g: &GData, ns: &str) -> Vec<String> {
    let p = format!("{ns}:");
    g.tags.iter().filter(|t| t.starts_with(&p)).map(|t| bare(t)).collect()
}

pub fn details_from_gdata(g: &GData, manga_id: &str) -> MangaDetails {
    let artists = by_ns(g, "artist");
    let groups = by_ns(g, "group");
    let genres_raw: Vec<String> = g
        .tags
        .iter()
        .filter(|t| !["artist:", "group:", "language:", "parody:", "character:", "cosplayer:", "reclass:"].iter().any(|p| t.starts_with(p)))
        .map(|t| bare(t))
        .collect();
    let langs: Vec<String> = by_ns(g, "language").into_iter().filter(|l| l != "translated").collect();
    let parody = by_ns(g, "parody");
    let mut desc: Vec<String> = Vec::new();
    if !g.category.is_empty() {
        desc.push(format!("Category: {}", g.category));
    }
    if !langs.is_empty() {
        desc.push(format!("Language: {}", langs.join(", ")));
    }
    if !parody.is_empty() {
        desc.push(format!("Parody: {}", parody.iter().take(4).cloned().collect::<Vec<_>>().join(", ")));
    }
    if !g.filecount.is_empty() {
        desc.push(format!("{} pages", g.filecount));
    }
    if !g.rating.is_empty() {
        desc.push(format!("Rating: {}/5", g.rating));
    }
    if !g.uploader.is_empty() {
        desc.push(format!("Uploader: {}", g.uploader));
    }
    let mut genres: Vec<String> = Vec::new();
    for x in std::iter::once(g.category.clone()).chain(genres_raw).filter(|s| !s.is_empty()) {
        if !genres.contains(&x) {
            genres.push(x);
        }
    }
    let title = if !g.title.is_empty() { &g.title } else if !g.title_jpn.is_empty() { &g.title_jpn } else { "Untitled" };
    let who = if !artists.is_empty() { Some(artists.join(", ")) } else if !groups.is_empty() { Some(groups.join(", ")) } else { None };
    MangaDetails {
        source_id: manga_id.to_string(),
        title: decode_entities(title),
        author: who,
        artist: (!artists.is_empty()).then(|| artists.join(", ")),
        description: (!desc.is_empty()).then(|| desc.join(" | ")),
        cover_url: Some(g.thumb.clone()).filter(|s| !s.is_empty()),
        status: Some("completed".into()),
        genres: Some(genres),
    }
}

pub fn chapter_from_gdata(g: &GData, manga_id: &str) -> ChapterInfo {
    ChapterInfo {
        source_id: manga_id.to_string(),
        number: Some(1.0),
        title: Some(decode_entities(if g.title.is_empty() { "Chapter 1" } else { &g.title })),
        language: "en".into(),
        scanlation_group: None,
        published_at: None,
        page_count: g.filecount.parse::<u32>().ok().filter(|n| *n > 0),
    }
}

/// Per-image viewer links on one gallery thumbnail page, in order, de-duplicated.
pub fn extract_viewer_links(html: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    VIEWER_LINK.captures_iter(html).map(|c| c[1].to_string()).filter(|u| seen.insert(u.clone())).collect()
}

/// Outcome of reading one viewer page.
#[derive(Debug, PartialEq)]
pub enum ViewerImage {
    Url(String),
    /// "You have exceeded your image viewing limits".
    LimitHit,
    Missing,
}

pub fn parse_viewer_image(html: &str) -> ViewerImage {
    if html.contains("exceeded your image viewing limits") || html.contains("509.gif") {
        return ViewerImage::LimitHit;
    }
    let doc = Html::parse_document(html);
    match doc.select(&sel("img#img[src]")).next().and_then(|i| i.value().attr("src")) {
        Some(u) => ViewerImage::Url(u.to_string()),
        None => ViewerImage::Missing,
    }
}

/// `category:name` / `female:big breasts` / legacy `tag:vanilla` / bare slug -> `f_search` query.
pub fn tag_query(tag_id: &str) -> String {
    match tag_id.split_once(':') {
        None => format!("\"{}$\"", tag_id.replace('-', " ")),
        Some((ns, name)) => {
            let name = name.replace('-', " ");
            if ns == "category" {
                format!("category:{name}")
            } else {
                format!("{}\"{name}$\"", if ns == "tag" { String::new() } else { format!("{ns}:") })
            }
        }
    }
}

fn title_case(s: &str) -> String {
    s.split(' ')
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn static_tags() -> Vec<Tag> {
    let mut v: Vec<Tag> = [
        ("category:doujinshi", "Doujinshi"),
        ("category:manga", "Manga"),
        ("category:artist cg", "Artist CG"),
        ("category:game cg", "Game CG"),
        ("language:english", "English"),
        ("language:japanese", "Japanese"),
    ]
    .iter()
    .map(|(i, n)| Tag::grouped(i.to_string(), n.to_string(), if i.starts_with("category:") { "Categories" } else { "Languages" }))
    .collect();
    let mut ns = |ns: &str, names: &[&str]| {
        v.extend(names.iter().map(|n| Tag::grouped(format!("{ns}:{n}"), title_case(n), &title_case(ns))));
    };
    ns("other", &["full color", "uncensored", "story arc"]);
    ns(
        "female",
        &[
            "big breasts", "schoolgirl uniform", "stockings", "glasses", "maid", "swimsuit", "milf", "tentacles", "monster girl",
            "ahegao", "netorare", "sole female",
        ],
    );
    ns("male", &["yaoi", "sole male", "shota"]);
    ns("female", &["futanari", "yuri"]);
    v
}

#[async_trait]
impl Source for EHentai {
    fn info(&self) -> SourceInfo {
        Self::source_info()
    }

    async fn search_manga(&self, query: &str, page: u32) -> Result<Vec<MangaSearchResult>> {
        self.list_page(&format!("/?f_search={}&page={}", encode_component(query), page.max(1) - 1)).await
    }

    async fn get_popular_manga(&self, page: u32) -> Result<Vec<MangaSearchResult>> {
        // /popular is one curated list (not paginated).
        if page > 1 {
            return Ok(Vec::new());
        }
        self.list_page("/popular").await
    }

    async fn get_latest_manga(&self, page: u32) -> Result<Vec<MangaSearchResult>> {
        self.list_page(&format!("/?page={}", page.max(1) - 1)).await
    }

    async fn get_manga_details(&self, manga_id: &str) -> Result<MangaDetails> {
        Ok(details_from_gdata(&self.gdata(manga_id).await?, manga_id))
    }

    async fn get_chapter_list(&self, manga_id: &str) -> Result<Vec<ChapterInfo>> {
        Ok(vec![chapter_from_gdata(&self.gdata(manga_id).await?, manga_id)])
    }

    async fn get_page_urls(&self, chapter_id: &str) -> Result<Vec<String>> {
        let total: usize = self.gdata(chapter_id).await?.filecount.parse().unwrap_or(0);
        // 1. Per-image viewer links from the gallery's thumbnail pages.
        let mut viewer: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        for p in 0..200 {
            let html = self.html(&format!("{BASE}/g/{chapter_id}/?p={p}&nw=always")).await?;
            let mut added = 0;
            for l in extract_viewer_links(&html) {
                if seen.insert(l.clone()) {
                    viewer.push(l);
                    added += 1;
                }
            }
            if added == 0 || (total > 0 && viewer.len() >= total) {
                break;
            }
        }
        if viewer.is_empty() {
            return Err(SourceError::Parse("e-hentai: no pages found in this gallery".into()));
        }
        // 2. Resolve each viewer page to the full image URL (bounded concurrency, order kept).
        let limit_hit = AtomicBool::new(false);
        let results: Vec<Option<String>> = stream::iter(viewer.clone())
            .map(|url: String| {
                let limit_hit = &limit_hit;
                async move {
                    if limit_hit.load(Ordering::Relaxed) {
                        return None;
                    }
                    match self.html(&url).await.map(|h| parse_viewer_image(&h)) {
                        Ok(ViewerImage::Url(u)) => Some(u),
                        Ok(ViewerImage::LimitHit) => {
                            limit_hit.store(true, Ordering::Relaxed);
                            None
                        }
                        _ => None,
                    }
                }
            })
            .buffered(8)
            .collect()
            .await;
        let out: Vec<String> = results.into_iter().flatten().collect();
        if out.is_empty() {
            return Err(if limit_hit.load(Ordering::Relaxed) {
                SourceError::Other("e-hentai: image viewing limit exceeded for this IP. Wait for it to reset or use a different network".into())
            } else {
                SourceError::Parse("e-hentai: could not extract any image URLs".into())
            });
        }
        Ok(out)
    }

    async fn get_tag_list(&self) -> Result<Vec<Tag>> {
        Ok(static_tags())
    }

    async fn get_manga_by_tag(&self, tag_id: &str, page: u32) -> Result<Vec<MangaSearchResult>> {
        self.get_manga_by_tags(&[tag_id.to_string()], page).await
    }

    async fn get_manga_by_tags(&self, tag_ids: &[String], page: u32) -> Result<Vec<MangaSearchResult>> {
        if tag_ids.is_empty() {
            return self.get_popular_manga(page).await;
        }
        let q = tag_ids.iter().map(|t| tag_query(t)).collect::<Vec<_>>().join(" ");
        self.list_page(&format!("/?f_search={}&page={}", encode_component(&q), page.max(1) - 1)).await
    }

    fn supports_multi_tag(&self) -> bool {
        true
    }

    fn download_headers(&self) -> Vec<(String, String)> {
        vec![("Referer".into(), "https://e-hentai.org/".into())]
    }
}
