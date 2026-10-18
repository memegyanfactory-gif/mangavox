//! MangaDex via its public REST API (port of `mangadex-native.ts`, minus the Node library and
//! browser-session fallbacks: reqwest + the optional `WallSolver` cover that).

use std::collections::HashMap;

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use mv_net::Opts;
use serde_json::Value;

use crate::source::{Result, Source, SourceCtx, SourceError};
use crate::types::{ChapterInfo, MangaDetails, MangaSearchResult, SourceInfo, Tag};
use crate::util::svg_icon;

pub const ID: &str = "mangadex";
const API: &str = "https://api.mangadex.org";
const RATINGS: [&str; 4] = ["safe", "suggestive", "erotica", "pornographic"];
const PAGE_LEN: u32 = 20;

pub struct MangaDex {
    ctx: SourceCtx,
}

impl MangaDex {
    pub fn new(ctx: SourceCtx) -> Self {
        Self { ctx }
    }

    pub fn source_info() -> SourceInfo {
        SourceInfo {
            id: ID.into(),
            name: "MangaDex".into(),
            version: "2.0.0".into(),
            lang: "en".into(),
            nsfw: false,
            requires_wall_solver: false,
            icon: Some(svg_icon("#FF6740", "MD", 14)),
        }
    }

    async fn json(&self, path_and_query: Vec<(String, String)>, path: &str) -> Result<Value> {
        let mut opts = Opts::new().header("Accept", "application/json");
        opts.query = path_and_query;
        self.ctx.fetch_json(ID, &format!("{API}{path}"), &opts).await
    }

    async fn manga_list(&self, q: ListQuery) -> Result<Vec<MangaSearchResult>> {
        let json = self.json(list_query(&q), "/manga").await?;
        let items = parse_manga_list(&json);
        if q.readable_only {
            Ok(self.filter_readable(items).await)
        } else {
            Ok(items)
        }
    }

    /// Drop titles whose English chapters are all external (unreadable in-app). Fails open.
    async fn filter_readable(&self, items: Vec<MangaSearchResult>) -> Vec<MangaSearchResult> {
        let ids: Vec<String> = items.iter().map(|i| i.source_id.clone()).collect();
        let keep: Vec<bool> = stream::iter(ids)
            .map(|id| async move {
                let mut q = vec![
                    ("limit".to_string(), "1".to_string()),
                    ("includeExternalUrl".to_string(), "0".to_string()),
                    ("translatedLanguage[]".to_string(), "en".to_string()),
                ];
                q.extend(RATINGS.iter().map(|r| ("contentRating[]".to_string(), r.to_string())));
                match self.json(q, &format!("/manga/{id}/feed")).await {
                    Ok(j) => j.get("total").and_then(Value::as_i64) != Some(0),
                    Err(_) => true,
                }
            })
            .buffered(4)
            .collect()
            .await;
        items.into_iter().zip(keep).filter_map(|(i, k)| k.then_some(i)).collect()
    }
}

struct ListQuery {
    title: Option<String>,
    order: &'static str,
    page: u32,
    tag: Option<String>,
    readable_only: bool,
}

fn list_query(q: &ListQuery) -> Vec<(String, String)> {
    let offset = (q.page.max(1) - 1) * PAGE_LEN;
    let mut v = vec![("limit".to_string(), PAGE_LEN.to_string()), ("offset".to_string(), offset.to_string())];
    if let Some(t) = &q.title {
        v.push(("title".into(), t.clone()));
    }
    v.push(("includes[]".into(), "cover_art".into()));
    v.push(("includes[]".into(), "author".into()));
    v.push((format!("order[{}]", q.order), "desc".into()));
    for r in RATINGS {
        v.push(("contentRating[]".into(), r.into()));
    }
    if q.readable_only {
        v.push(("hasAvailableChapters".into(), "true".into()));
        v.push(("availableTranslatedLanguage[]".into(), "en".into()));
    }
    if let Some(t) = &q.tag {
        v.push(("includedTags[]".into(), t.clone()));
    }
    v
}

#[async_trait]
impl Source for MangaDex {
    fn info(&self) -> SourceInfo {
        Self::source_info()
    }

    async fn search_manga(&self, query: &str, page: u32) -> Result<Vec<MangaSearchResult>> {
        self.manga_list(ListQuery { title: Some(query.into()), order: "relevance", page, tag: None, readable_only: false }).await
    }

    async fn get_popular_manga(&self, page: u32) -> Result<Vec<MangaSearchResult>> {
        self.manga_list(ListQuery { title: None, order: "followedCount", page, tag: None, readable_only: true }).await
    }

    async fn get_latest_manga(&self, page: u32) -> Result<Vec<MangaSearchResult>> {
        self.manga_list(ListQuery { title: None, order: "latestUploadedChapter", page, tag: None, readable_only: true }).await
    }

    async fn get_manga_details(&self, manga_id: &str) -> Result<MangaDetails> {
        let q = ["cover_art", "author", "artist"].iter().map(|i| ("includes[]".to_string(), i.to_string())).collect();
        let json = self.json(q, &format!("/manga/{manga_id}")).await?;
        parse_details(&json)
    }

    async fn get_chapter_list(&self, manga_id: &str) -> Result<Vec<ChapterInfo>> {
        // 500 per page. External-only chapters (externalUrl, pages = 0) cannot be read in-app.
        const PAGE_SIZE: usize = 500;
        let mut all: Vec<Value> = Vec::new();
        let mut offset = 0usize;
        let mut total = usize::MAX;
        while offset < total && offset < 20_000 {
            let mut q = vec![
                ("limit".to_string(), PAGE_SIZE.to_string()),
                ("offset".to_string(), offset.to_string()),
                ("translatedLanguage[]".into(), "en".into()),
                ("order[volume]".into(), "asc".into()),
                ("order[chapter]".into(), "asc".into()),
                ("includes[]".into(), "scanlation_group".into()),
                ("includeExternalUrl".into(), "0".into()),
            ];
            q.extend(RATINGS.iter().map(|r| ("contentRating[]".to_string(), r.to_string())));
            let json = self.json(q, &format!("/manga/{manga_id}/feed")).await?;
            let data = json.get("data").and_then(Value::as_array).cloned().unwrap_or_default();
            total = json.get("total").and_then(Value::as_u64).map(|t| t as usize).unwrap_or(offset + data.len());
            if data.is_empty() {
                break;
            }
            offset += data.len();
            all.extend(data);
        }
        Ok(build_chapter_list(&all))
    }

    async fn get_page_urls(&self, chapter_id: &str) -> Result<Vec<String>> {
        let info = self.json(Vec::new(), &format!("/chapter/{chapter_id}")).await.ok();
        let srv = self.json(Vec::new(), &format!("/at-home/server/{chapter_id}")).await?;
        parse_at_home(&srv, info.as_ref())
    }

    async fn get_tag_list(&self) -> Result<Vec<Tag>> {
        Ok(parse_tags(&self.json(Vec::new(), "/manga/tag").await?))
    }

    async fn get_manga_by_tag(&self, tag_id: &str, page: u32) -> Result<Vec<MangaSearchResult>> {
        self.manga_list(ListQuery { title: None, order: "followedCount", page, tag: Some(tag_id.into()), readable_only: true }).await
    }
}

// ── Parsers (pure) ──────────────────────────────────────────────────

/// `{ "en": "...", "ja-ro": "..." }` -> best string.
pub fn local_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Object(m) => ["en", "ja-ro", "ja"]
            .iter()
            .find_map(|k| m.get(*k).and_then(Value::as_str))
            .or_else(|| m.values().find_map(Value::as_str))
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    }
}

fn rels<'a>(m: &'a Value, ty: &'a str) -> impl Iterator<Item = &'a Value> {
    m.get("relationships").and_then(Value::as_array).into_iter().flatten().filter(move |r| r.get("type").and_then(Value::as_str) == Some(ty))
}

fn rel_name(m: &Value, ty: &str) -> Option<String> {
    rels(m, ty).find_map(|r| r.pointer("/attributes/name").and_then(Value::as_str)).map(str::to_string)
}

pub fn cover_url(m: &Value) -> Option<String> {
    let id = m.get("id")?.as_str()?;
    let file = rels(m, "cover_art").find_map(|r| r.pointer("/attributes/fileName").and_then(Value::as_str))?;
    Some(format!("https://uploads.mangadex.org/covers/{id}/{file}.256.jpg"))
}

fn str_attr(m: &Value, key: &str) -> Option<String> {
    m.pointer(&format!("/attributes/{key}")).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

pub fn manga_to_result(m: &Value) -> MangaSearchResult {
    MangaSearchResult {
        source_id: m.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
        title: local_string(m.pointer("/attributes/title").unwrap_or(&Value::Null)),
        alt_titles: None,
        cover_url: cover_url(m),
        author: rel_name(m, "author"),
        status: str_attr(m, "status"),
    }
}

/// `GET /manga?...` -> results (entries without an id are skipped).
pub fn parse_manga_list(json: &Value) -> Vec<MangaSearchResult> {
    json.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(manga_to_result)
        .filter(|r| !r.source_id.is_empty())
        .collect()
}

/// `GET /manga/{id}?includes[]=cover_art&includes[]=author&includes[]=artist`
pub fn parse_details(json: &Value) -> Result<MangaDetails> {
    let m = json.get("data").filter(|d| d.is_object()).ok_or_else(|| SourceError::NotFound("MangaDex: title not found".into()))?;
    let genres: Vec<String> = m
        .pointer("/attributes/tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|t| local_string(t.pointer("/attributes/name").unwrap_or(&Value::Null)))
        .filter(|s| !s.is_empty())
        .collect();
    Ok(MangaDetails {
        source_id: m.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
        title: local_string(m.pointer("/attributes/title").unwrap_or(&Value::Null)),
        author: rel_name(m, "author"),
        artist: rel_name(m, "artist"),
        description: Some(local_string(m.pointer("/attributes/description").unwrap_or(&Value::Null))).filter(|s| !s.is_empty()),
        cover_url: cover_url(m),
        status: str_attr(m, "status"),
        genres: Some(genres),
    })
}

fn chapter_number(c: &Value) -> Option<f64> {
    c.pointer("/attributes/chapter").and_then(Value::as_str).and_then(|s| s.trim().parse::<f64>().ok()).filter(|n| n.is_finite())
}

fn pages_of(c: &Value) -> u64 {
    c.pointer("/attributes/pages").and_then(Value::as_u64).unwrap_or(0)
}

fn chapter_to_info(c: &Value) -> ChapterInfo {
    let a = c.get("attributes").unwrap_or(&Value::Null);
    let n = chapter_number(c);
    let group = rels(c, "scanlation_group").find_map(|r| r.pointer("/attributes/name").and_then(Value::as_str));
    let vol = a.get("volume").and_then(Value::as_str).filter(|v| !v.is_empty()).map(|v| format!("Vol. {v} ")).unwrap_or_default();
    let title = a
        .get("title")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .or_else(|| n.is_none().then(|| format!("{vol}Oneshot").trim().to_string()));
    let published = ["publishAt", "readableAt", "createdAt"].iter().find_map(|k| a.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()));
    ChapterInfo {
        source_id: c.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
        number: n,
        title,
        language: a.get("translatedLanguage").and_then(Value::as_str).unwrap_or("en").to_string(),
        scanlation_group: group.map(str::to_string),
        published_at: published.map(str::to_string),
        page_count: Some(pages_of(c) as u32).filter(|p| *p > 0),
    }
}

/// Feed items -> readable, de-duplicated (most pages wins per chapter number), ascending list.
/// Numberless chapters (oneshots / extras) go last, oldest first.
pub fn build_chapter_list(all: &[Value]) -> Vec<ChapterInfo> {
    let readable = all.iter().filter(|c| {
        let a = c.get("attributes").unwrap_or(&Value::Null);
        a.get("externalUrl").map_or(true, Value::is_null) && a.get("isUnavailable").and_then(Value::as_bool) != Some(true) && pages_of(c) > 0
    });
    let mut best: HashMap<String, &Value> = HashMap::new();
    let mut no_num: Vec<&Value> = Vec::new();
    for c in readable {
        match chapter_number(c) {
            None => no_num.push(c),
            Some(n) => {
                let key = n.to_string();
                if best.get(&key).map_or(true, |cur| pages_of(c) > pages_of(cur)) {
                    best.insert(key, c);
                }
            }
        }
    }
    let mut numbered: Vec<&Value> = best.into_values().collect();
    numbered.sort_by(|x, y| chapter_number(x).partial_cmp(&chapter_number(y)).unwrap_or(std::cmp::Ordering::Equal));
    no_num.sort_by_key(|c| c.pointer("/attributes/publishAt").and_then(Value::as_str).unwrap_or("").to_string());
    numbered.into_iter().chain(no_num).map(chapter_to_info).collect()
}

/// `GET /at-home/server/{chapterId}` (+ optional `GET /chapter/{id}` to detect external chapters).
pub fn parse_at_home(srv: &Value, chapter_info: Option<&Value>) -> Result<Vec<String>> {
    if let Some(ext) = chapter_info.and_then(|i| i.pointer("/data/attributes/externalUrl")).and_then(Value::as_str) {
        return Err(SourceError::Other(format!("This chapter is hosted externally at: {ext} - it cannot be read in-app")));
    }
    let no_pages = || SourceError::Parse("No pages available for this chapter on MangaDex".into());
    let base = srv.get("baseUrl").and_then(Value::as_str).ok_or_else(no_pages)?;
    let ch = srv.get("chapter").ok_or_else(no_pages)?;
    let hash = ch.get("hash").and_then(Value::as_str).ok_or_else(no_pages)?;
    let files = |k: &str| -> Vec<String> { ch.get(k).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect() };
    let full = files("data");
    if !full.is_empty() {
        return Ok(full.iter().map(|f| format!("{base}/data/{hash}/{f}")).collect());
    }
    let saver = files("dataSaver");
    if !saver.is_empty() {
        return Ok(saver.iter().map(|f| format!("{base}/data-saver/{hash}/{f}")).collect());
    }
    Err(no_pages())
}

/// `GET /manga/tag` -> genre / theme / format tags sorted by name.
pub fn parse_tags(json: &Value) -> Vec<Tag> {
    let mut tags: Vec<Tag> = json
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|t| matches!(t.pointer("/attributes/group").and_then(Value::as_str), Some("genre" | "theme" | "format")))
        .map(|t| Tag {
            id: t.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
            name: local_string(t.pointer("/attributes/name").unwrap_or(&Value::Null)),
        })
        .filter(|t| !t.name.is_empty() && !t.id.is_empty())
        .collect();
    tags.sort_by(|a, b| a.name.cmp(&b.name));
    tags
}
