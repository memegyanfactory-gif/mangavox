//! Vortex Scans (vortexscans.org). An "Iken" CMS site: listings, details and the chapter list come
//! from the JSON API on `api.vortexscans.org`; the page list from the server-rendered reader
//! (every page is an `<img data-reader-page-image>` in the HTML, later ones `loading="lazy"`), with
//! the reader's `publicImages` data blob as a fallback.
//!
//! Ids: manga = series slug, chapter = `{series slug}/{chapter slug}` (the reader URL's path under
//! `/series/`). Paid / locked chapters have no readable pages and are left out of the list.

use std::collections::HashSet;

use async_trait::async_trait;
use mv_net::Opts;
use scraper::Html;
use serde_json::Value;

use crate::source::{Result, Source, SourceCtx, SourceError};
use crate::types::{ChapterInfo, MangaDetails, MangaSearchResult, SourceInfo};
use crate::util::{abs_url, encode_component, sel, svg_icon};

pub const ID: &str = "vortexscans";
const API: &str = "https://api.vortexscans.org";
const WEB: &str = "https://vortexscans.org";
const PER_PAGE: u32 = 24;
/// Chapter rows asked for per API call; the list is paged until `totalChapterCount` is reached.
const CHAPTER_TAKE: usize = 500;

pub struct VortexScans {
    ctx: SourceCtx,
}

impl VortexScans {
    pub fn new(ctx: SourceCtx) -> Self {
        Self { ctx }
    }

    pub fn source_info() -> SourceInfo {
        SourceInfo {
            id: ID.into(),
            name: "Vortex Scans".into(),
            version: "1.0.0".into(),
            lang: "en".into(),
            nsfw: false,
            requires_wall_solver: false,
            icon: Some(svg_icon("#7C3AED", "VX", 11)),
        }
    }

    async fn api(&self, path_and_query: &str) -> Result<Value> {
        let url = format!("{API}/api/{}", path_and_query.trim_start_matches('/'));
        let opts = Opts::new().header("Accept", "application/json").referer(format!("{WEB}/"));
        self.ctx.fetch_json(ID, &url, &opts).await
    }

    async fn post(&self, slug: &str) -> Result<Value> {
        self.api(&format!("post?postSlug={}", encode_component(slug))).await
    }
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn is_novel(p: &Value) -> bool {
    p.get("isNovel").and_then(Value::as_bool) == Some(true) || s(p, "seriesType").is_some_and(|t| t.eq_ignore_ascii_case("novel"))
}

fn status_of(p: &Value) -> Option<String> {
    let st = s(p, "seriesStatus")?.to_ascii_lowercase();
    Some(
        match st.as_str() {
            "ongoing" => "ongoing",
            "completed" | "complete" => "completed",
            "hiatus" => "hiatus",
            "cancelled" | "canceled" | "dropped" => "cancelled",
            _ => "unknown",
        }
        .into(),
    )
}

/// `/api/query` and `/api/posts` -> cards (`{posts: [...]}`); novels are skipped.
pub fn parse_posts(json: &Value) -> Vec<MangaSearchResult> {
    let mut seen = HashSet::new();
    json.get("posts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|p| !is_novel(p))
        .filter_map(|p| {
            let slug = s(p, "slug")?;
            let title = s(p, "postTitle")?;
            seen.insert(slug.to_string()).then(|| MangaSearchResult {
                source_id: slug.to_string(),
                title: title.to_string(),
                cover_url: s(p, "featuredImage").and_then(|u| abs_url(WEB, u)),
                status: status_of(p),
                ..Default::default()
            })
        })
        .collect()
}

/// HTML synopsis -> plain text, one paragraph per line.
fn html_text(html: &str) -> String {
    let frag = Html::parse_fragment(&html.replace("<br>", "\n").replace("<br/>", "\n").replace("<br />", "\n"));
    let paras: Vec<String> = frag.select(&sel("p")).map(|p| p.text().collect::<String>().trim().to_string()).filter(|t| !t.is_empty()).collect();
    if paras.is_empty() {
        frag.root_element().text().collect::<String>().trim().to_string()
    } else {
        paras.join("\n")
    }
}

/// `/api/post?postSlug=` -> details.
pub fn parse_details(json: &Value, manga_id: &str) -> Result<MangaDetails> {
    let p = json.get("post").filter(|p| p.is_object()).unwrap_or(json);
    let title = s(p, "postTitle").ok_or_else(|| SourceError::NotFound(format!("Vortex Scans series {manga_id}")))?;
    let genres: Vec<String> = p
        .get("genres")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|g| g.get("name").and_then(Value::as_str).or_else(|| g.as_str()))
        .map(|g| g.trim().to_string())
        .filter(|g| !g.is_empty())
        .collect();
    Ok(MangaDetails {
        source_id: manga_id.to_string(),
        title: title.to_string(),
        author: s(p, "author").map(str::to_string),
        artist: s(p, "artist").map(str::to_string),
        description: s(p, "postContent").map(html_text).filter(|d| !d.is_empty()),
        cover_url: s(p, "featuredImage").and_then(|u| abs_url(WEB, u)),
        status: status_of(p).or(Some("unknown".into())),
        genres: Some(genres),
    })
}

/// The post id the chapter API is keyed by.
pub fn post_id(json: &Value) -> Option<i64> {
    json.pointer("/post/id").or_else(|| json.get("id")).and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
}

/// Raw rows of one `/api/chapters` answer (`{post: {chapters: [...]}, totalChapterCount}`).
pub fn chapter_rows(json: &Value) -> (Vec<Value>, Option<usize>) {
    let rows = json.pointer("/post/chapters").or_else(|| json.get("chapters")).and_then(Value::as_array).cloned().unwrap_or_default();
    let total = json.get("totalChapterCount").and_then(Value::as_u64).map(|t| t as usize);
    (rows, total)
}

/// Raw chapter rows -> readable chapters, ascending. Locked / paid rows (`isAccessible: false`)
/// are dropped: the reader serves no pages for them.
pub fn parse_chapters(series_slug: &str, rows: &[Value]) -> Vec<ChapterInfo> {
    let mut seen = HashSet::new();
    let mut out: Vec<ChapterInfo> = rows
        .iter()
        .filter(|c| {
            let accessible = c.get("isAccessible").and_then(Value::as_bool);
            let locked = c.get("isLocked").and_then(Value::as_bool) == Some(true);
            accessible.unwrap_or(!locked)
        })
        .filter_map(|c| {
            let slug = s(c, "slug")?;
            if !seen.insert(slug.to_string()) {
                return None;
            }
            let number = c.get("number").and_then(|v| v.as_f64().or_else(|| v.as_str()?.trim().parse().ok())).filter(|n| n.is_finite());
            let label = number.map(|n| format!("Chapter {n}")).unwrap_or_else(|| "Chapter".into());
            let title = match s(c, "title") {
                Some(t) => format!("{label}: {t}"),
                None => label,
            };
            Some(ChapterInfo {
                source_id: format!("{series_slug}/{slug}"),
                number,
                title: Some(title),
                language: "en".into(),
                published_at: s(c, "createdAt").map(str::to_string),
                ..Default::default()
            })
        })
        .collect();
    out.sort_by(|a, b| a.number.unwrap_or(-1.0).partial_cmp(&b.number.unwrap_or(-1.0)).unwrap_or(std::cmp::Ordering::Equal));
    out
}

fn img_url(el: scraper::ElementRef<'_>) -> Option<String> {
    let a = el.value();
    let srcset_best = |v: &str| v.split(',').filter_map(|p| p.split_whitespace().next()).last().map(str::to_string);
    ["data-src", "data-lazy-src", "src"]
        .iter()
        .find_map(|k| a.attr(k).map(str::trim).filter(|u| !u.is_empty() && !u.starts_with("data:")).map(str::to_string))
        .or_else(|| a.attr("srcset").and_then(srcset_best))
        .and_then(|u| abs_url(WEB, &u))
}

/// Chapter reader HTML -> every page image URL in reading order.
///
/// 1. `img[data-reader-page-image]`, ordered by `data-reader-index` (all pages are in the HTML;
///    only the first few are `loading="eager"`, which is why a DOM read after render or a
///    "visible images" scrape comes up short).
/// 2. The reader's `publicImages` data (seroval / JSON) when the markup changes.
/// 3. Any image inside the article body.
pub fn parse_reader_images(html: &str) -> Vec<String> {
    let doc = Html::parse_document(html);
    let mut seen = HashSet::new();
    let mut tagged: Vec<(usize, usize, String)> = doc
        .select(&sel("img[data-reader-page-image]"))
        .enumerate()
        .filter_map(|(pos, i)| {
            let idx = i.value().attr("data-reader-index").and_then(|v| v.trim().parse().ok()).unwrap_or(usize::MAX);
            img_url(i).map(|u| (idx, pos, u))
        })
        .collect();
    tagged.sort_by_key(|(idx, pos, _)| (*idx, *pos));
    let urls: Vec<String> = tagged.into_iter().map(|(_, _, u)| u).filter(|u| seen.insert(u.clone())).collect();
    if !urls.is_empty() {
        return urls;
    }
    let blob = parse_public_images(html);
    if !blob.is_empty() {
        return blob;
    }
    doc.select(&sel("section[itemprop=articleBody] img, .comic-images-wrapper img"))
        .filter_map(img_url)
        .filter(|u| seen.insert(u.clone()))
        .collect()
}

/// `publicImages:$R[n]=[{url:"..."}, ...]` (seroval) or `"publicImages":[{"url":"..."}]` (JSON)
/// in the reader's data script -> URLs in order.
pub fn parse_public_images(html: &str) -> Vec<String> {
    let start_re = regex::Regex::new(r#"publicImages"?\s*:\s*(?:\$R\[\d+\]\s*=\s*)?\["#).unwrap();
    let Some(m) = start_re.find(html) else { return Vec::new() };
    // Walk to the matching `]`, skipping string contents (`$R[12]` references nest brackets).
    let bytes = html.as_bytes();
    let (mut depth, mut i, mut in_str, mut esc) = (0i32, m.end() - 1, false, false);
    let end = loop {
        if i >= bytes.len() {
            break bytes.len();
        }
        let c = bytes[i];
        if in_str {
            match (esc, c) {
                (true, _) => esc = false,
                (false, b'\\') => esc = true,
                (false, b'"') => in_str = false,
                _ => {}
            }
        } else {
            match c {
                b'"' => in_str = true,
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        break i + 1;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    };
    let body = &html[m.end() - 1..end];
    let url_re = regex::Regex::new(r#""?url"?\s*:\s*"([^"]+)""#).unwrap();
    let mut seen = HashSet::new();
    url_re
        .captures_iter(body)
        .map(|c| c[1].replace("\\u002F", "/").replace("\\/", "/"))
        .filter_map(|u| abs_url(WEB, &u))
        .filter(|u| seen.insert(u.clone()))
        .collect()
}

/// Reader URL of a `{series}/{chapter}` id (a full URL is accepted too).
pub fn chapter_url(chapter_id: &str) -> String {
    if chapter_id.starts_with("http") {
        return chapter_id.to_string();
    }
    let path = chapter_id.trim_matches('/');
    let path = path.strip_prefix("series/").unwrap_or(path);
    format!("{WEB}/series/{path}")
}

/// Series slug of a manga id (a bare slug, `series/{slug}` or a full series URL).
pub fn series_slug(manga_id: &str) -> &str {
    let t = manga_id.trim().trim_end_matches('/');
    let t = t.rsplit_once("/series/").map_or(t, |(_, r)| r);
    let t = t.strip_prefix("series/").unwrap_or(t);
    t.split(['/', '?', '#']).next().unwrap_or(t)
}

#[async_trait]
impl Source for VortexScans {
    fn info(&self) -> SourceInfo {
        Self::source_info()
    }

    async fn search_manga(&self, query: &str, page: u32) -> Result<Vec<MangaSearchResult>> {
        let j = self.api(&format!("query?page={}&perPage={PER_PAGE}&searchTerm={}", page.max(1), encode_component(query.trim()))).await?;
        Ok(parse_posts(&j))
    }

    async fn get_popular_manga(&self, page: u32) -> Result<Vec<MangaSearchResult>> {
        let j = self.api(&format!("query?page={}&perPage={PER_PAGE}&searchTerm=&orderBy=totalViews&orderDirection=desc", page.max(1))).await?;
        Ok(parse_posts(&j))
    }

    async fn get_latest_manga(&self, page: u32) -> Result<Vec<MangaSearchResult>> {
        let j = self.api(&format!("posts?page={}&perPage={PER_PAGE}&tag=latestUpdate&isNovel=false", page.max(1))).await?;
        Ok(parse_posts(&j))
    }

    async fn get_manga_details(&self, manga_id: &str) -> Result<MangaDetails> {
        let slug = series_slug(manga_id);
        parse_details(&self.post(slug).await?, slug)
    }

    async fn get_chapter_list(&self, manga_id: &str) -> Result<Vec<ChapterInfo>> {
        let slug = series_slug(manga_id);
        let pid = post_id(&self.post(slug).await?).ok_or_else(|| SourceError::NotFound(format!("Vortex Scans series {slug}")))?;
        // Page through the whole list: never trust one answer to hold every chapter.
        let mut rows: Vec<Value> = Vec::new();
        for _ in 0..100 {
            let j = self.api(&format!("chapters?postId={pid}&skip={}&take={CHAPTER_TAKE}&order=desc&userid=", rows.len())).await?;
            let (batch, total) = chapter_rows(&j);
            let n = batch.len();
            rows.extend(batch);
            if n == 0 || total.map_or(n < CHAPTER_TAKE, |t| rows.len() >= t) {
                break;
            }
        }
        Ok(parse_chapters(slug, &rows))
    }

    async fn get_page_urls(&self, chapter_id: &str) -> Result<Vec<String>> {
        let url = chapter_url(chapter_id);
        let opts = Opts::new().header("Accept", "text/html,application/xhtml+xml").referer(format!("{WEB}/"));
        let html = self.ctx.fetch_text(ID, &url, &opts).await?;
        let urls = parse_reader_images(&html);
        if urls.is_empty() {
            let locked = html.contains("isAccessible:!1") || html.contains("\"isAccessible\":false");
            return Err(SourceError::Parse(if locked {
                "Vortex Scans: this chapter is locked (paid / early access)".into()
            } else {
                "Vortex Scans: no page images found for this chapter".into()
            }));
        }
        Ok(urls)
    }

    fn download_headers(&self) -> Vec<(String, String)> {
        vec![("Referer".into(), format!("{WEB}/"))]
    }
}
