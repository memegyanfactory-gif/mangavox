//! Manga routes (API.md "Manga"): the library, a title's chapters, covers, reader pages and
//! reading progress. Same data and language rules as the desktop Library and Reader.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::Path as UrlPath;
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use mv_core::repo::chapter::{self, Chapter};
use mv_core::repo::manga::{self, Manga};
use serde::Deserialize;
use serde_json::{json, Value};
use ui_manga::{backend, logic, ops, store};

use crate::server::{blocking, ApiError, ApiResult};

/// How long a chapter's page list is kept, so `/page/<n>` does not ask the source every time.
const PAGES_TTL: Duration = Duration::from_secs(10 * 60);
const IMAGE_CACHE: &str = "private, max-age=86400";
/// No chapter has more pages than this.
const MAX_PAGE: usize = 100_000;

pub fn routes() -> Router {
    Router::new()
        .route("/api/library", get(library))
        .route("/api/manga/{id}", get(detail))
        .route("/api/manga/{id}/cover", get(cover))
        .route("/api/manga/{id}/refresh", post(refresh))
        .route("/api/chapter/{id}", get(chapter_info))
        .route("/api/chapter/{id}/page/{n}", get(page))
        .route("/api/chapter/{id}/progress", post(progress))
}

/// Percent-encodes an id for use in a URL path segment.
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn cover_url(m: &Manga) -> Value {
    match m.cover_path.as_deref().filter(|c| !c.trim().is_empty()) {
        Some(_) => json!(format!("/api/manga/{}/cover", enc(&m.id))),
        None => Value::Null,
    }
}

// ---------------------------------------------------------------- library

async fn library() -> ApiResult<Json<Value>> {
    let lang = backend::preferred_language();
    let hidden = backend::browse_only();
    let (mut items, stats) = blocking(move || {
        backend::with_db(|c| {
            let items: Vec<Manga> = manga::list(c)?.into_iter().filter(|m| !hidden.contains(&m.id)).collect();
            Ok((items, store::library_stats(c, &lang)?))
        })
    })
    .await?;
    // Last read first (never-read titles keep the newest-added order after them).
    items.sort_by(|a, b| b.last_read_at.cmp(&a.last_read_at));
    let out: Vec<Value> = items
        .iter()
        .map(|m| {
            let st = stats.get(&m.id);
            json!({
                "id": m.id,
                "title": m.title,
                "author": m.author,
                "cover": cover_url(m),
                "unread": st.map(|s| s.unread as i64).or(m.unread_count).unwrap_or(0),
                "chapters": st.map(|s| s.total as i64).or(m.chapter_count).unwrap_or(0),
                "last_read_at": m.last_read_at,
            })
        })
        .collect();
    Ok(Json(Value::Array(out)))
}

// ---------------------------------------------------------------- one title

fn chapter_json(ch: &Chapter) -> Value {
    let pages = ch.page_count.unwrap_or(0);
    let progress = if ch.is_read {
        1.0
    } else if ch.read_progress > 0 && pages > 0 {
        ((ch.read_progress + 1) as f64 / pages as f64).min(1.0)
    } else {
        0.0
    };
    json!({
        "id": ch.id,
        "number": ch.number,
        "title": ch.title,
        "read": ch.is_read,
        "progress": progress,
        "pages": ch.page_count,
        "downloaded": ch.download_status == "complete",
        "audio": ch.tts_status == "complete",
        "video": ch.video_status == "complete",
    })
}

fn manga_json(m: &Manga, chapters: &[Chapter]) -> Value {
    json!({
        "id": m.id,
        "title": m.title,
        "author": m.author,
        "description": m.description,
        "genres": m.genres,
        "status": m.status,
        "cover": cover_url(m),
        "chapters": chapters.iter().map(chapter_json).collect::<Vec<_>>(),
    })
}

async fn load_manga(id: String) -> ApiResult<(Manga, Vec<Chapter>)> {
    let lang = backend::preferred_language();
    let found = blocking(move || {
        backend::with_db(|c| {
            let Some(m) = manga::get(c, &id)? else { return Ok(None) };
            let list = store::chapters(c, &m.id, &lang)?;
            Ok(Some((m, list)))
        })
    })
    .await?;
    found.ok_or_else(|| ApiError::not_found("Manga not found"))
}

async fn detail(UrlPath(id): UrlPath<String>) -> ApiResult<Json<Value>> {
    let (m, list) = load_manga(id).await?;
    Ok(Json(manga_json(&m, &list)))
}

async fn refresh(UrlPath(id): UrlPath<String>) -> ApiResult<Json<Value>> {
    load_manga(id.clone()).await?;
    ops::refresh_manga(id.clone()).await.map_err(|e| ApiError(axum::http::StatusCode::BAD_GATEWAY, format!("Could not refresh from the source: {e}")))?;
    let (m, list) = load_manga(id).await?;
    Ok(Json(manga_json(&m, &list)))
}

fn image_response(bytes: Vec<u8>, path: &Path) -> Response {
    let mime = mime_guess::from_path(path).first().filter(|m| m.type_() == "image").map(|m| m.to_string()).unwrap_or_else(|| sniff_mime(&bytes).to_string());
    let mut r = bytes.into_response();
    if let Ok(v) = HeaderValue::from_str(&mime) {
        r.headers_mut().insert(header::CONTENT_TYPE, v);
    }
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(IMAGE_CACHE));
    r
}

fn sniff_mime(b: &[u8]) -> &'static str {
    if b.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if b.starts_with(b"GIF8") {
        "image/gif"
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "image/jpeg"
    }
}

async fn read_image(path: PathBuf) -> ApiResult<Response> {
    let bytes = tokio::fs::read(&path).await.map_err(|_| ApiError::not_found("Image not found"))?;
    Ok(image_response(bytes, &path))
}

async fn cover(UrlPath(id): UrlPath<String>) -> ApiResult<Response> {
    let m = blocking(move || backend::with_db(|c| manga::get(c, &id))).await?.ok_or_else(|| ApiError::not_found("Manga not found"))?;
    let url = m.cover_path.clone().filter(|c| !c.trim().is_empty()).ok_or_else(|| ApiError::not_found("No cover"))?;
    let local = {
        let u = url.clone();
        tokio::task::spawn_blocking(move || mv_core::covers::cached_cover(&u)).await.ok().flatten()
    };
    let path = match local {
        Some(p) => p,
        None => backend::ensure_cover(url, Some(m.source_plugin.clone())).await.ok_or_else(|| ApiError::not_found("Cover not available"))?,
    };
    read_image(path).await
}

// ---------------------------------------------------------------- chapters and pages

#[derive(Clone)]
enum Page {
    Local(PathBuf),
    Remote(String),
}

struct CachedPages {
    at: Instant,
    pages: Arc<Vec<Page>>,
}

static PAGES: Mutex<Option<HashMap<String, CachedPages>>> = Mutex::new(None);

fn cached_pages(chapter_id: &str) -> Option<Arc<Vec<Page>>> {
    let mut g = PAGES.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    map.retain(|_, c| c.at.elapsed() < PAGES_TTL);
    map.get(chapter_id).map(|c| c.pages.clone())
}

fn remember_pages(chapter_id: &str, pages: Arc<Vec<Page>>) {
    PAGES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(chapter_id.to_string(), CachedPages { at: Instant::now(), pages });
}

/// Downloaded page files of a chapter, in order (same rule as the desktop reader).
fn local_pages(ch: &Chapter) -> Vec<PathBuf> {
    if ch.download_status != "complete" {
        return vec![];
    }
    let Some(dir) = ch.pages_dir.as_deref() else { return vec![] };
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()).is_some_and(|e| ["jpg", "jpeg", "png", "webp", "gif"].contains(&e.to_lowercase().as_str()))
                && std::fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false)
        })
        .collect();
    v.sort();
    v
}

/// The source plugin a chapter's pages come from.
fn source_of(m: &Manga, ch: &Chapter) -> String {
    if ch.source_plugin.is_empty() {
        m.source_plugin.clone()
    } else {
        ch.source_plugin.clone()
    }
}

/// A chapter's pages: downloaded files, else the source's page URLs (cached for a few minutes).
async fn pages_of(ch: &Chapter, source: &str) -> ApiResult<Arc<Vec<Page>>> {
    if let Some(p) = cached_pages(&ch.id) {
        return Ok(p);
    }
    let local = {
        let c = ch.clone();
        tokio::task::spawn_blocking(move || local_pages(&c)).await.unwrap_or_default()
    };
    let pages: Vec<Page> = if !local.is_empty() {
        local.into_iter().map(Page::Local).collect()
    } else {
        let src = backend::source(source).await.map_err(|e| ApiError(axum::http::StatusCode::BAD_GATEWAY, e.to_string()))?;
        let urls = src.get_page_urls(&ch.source_id).await.map_err(|e| ApiError(axum::http::StatusCode::BAD_GATEWAY, e.to_string()))?;
        if urls.is_empty() {
            return Err(ApiError(axum::http::StatusCode::BAD_GATEWAY, "The source returned no pages for this chapter".into()));
        }
        urls.into_iter().map(Page::Remote).collect()
    };
    let pages = Arc::new(pages);
    remember_pages(&ch.id, pages.clone());
    Ok(pages)
}

async fn load_chapter(id: String) -> ApiResult<(Manga, Chapter)> {
    let found = blocking(move || {
        backend::with_db(|c| {
            let Some(ch) = chapter::get(c, &id)? else { return Ok(None) };
            let Some(m) = manga::get(c, &ch.manga_id)? else { return Ok(None) };
            Ok(Some((m, ch)))
        })
    })
    .await?;
    found.ok_or_else(|| ApiError::not_found("Chapter not found"))
}

async fn chapter_info(UrlPath(id): UrlPath<String>) -> ApiResult<Json<Value>> {
    let (m, ch) = load_chapter(id).await?;
    let lang = backend::preferred_language();
    let (mid, cid) = (m.id.clone(), ch.id.clone());
    let list = blocking(move || {
        backend::with_db(|c| {
            let list = store::chapters(c, &mid, &lang)?;
            // The open chapter is always part of the list, even in another language.
            if list.iter().any(|x| x.id == cid) {
                Ok(list)
            } else {
                Ok(chapter::list(c, &mid)?)
            }
        })
    })
    .await?;
    let pages = pages_of(&ch, &source_of(&m, &ch)).await?;
    let start = if ch.read_progress > 0 && !ch.is_read { logic::clamp_page(ch.read_progress, pages.len()) } else { 0 };
    let i = list.iter().position(|x| x.id == ch.id);
    let prev = i.and_then(|i| i.checked_sub(1)).and_then(|j| list.get(j)).map(|c| c.id.clone());
    let next = i.and_then(|i| list.get(i + 1)).map(|c| c.id.clone());
    Ok(Json(json!({
        "id": ch.id,
        "manga_id": m.id,
        "manga_title": m.title,
        "title": ch.title,
        "number": ch.number,
        "pages": pages.len(),
        "start": start,
        "prev": prev,
        "next": next,
    })))
}

async fn fetch_page(page: &Page, source: &str) -> ApiResult<PathBuf> {
    match page {
        Page::Local(p) => Ok(p.clone()),
        Page::Remote(url) => backend::download_page(url.clone(), Some(source.to_string())).await.map_err(|e| ApiError(axum::http::StatusCode::BAD_GATEWAY, format!("Could not load the page: {e}"))),
    }
}

async fn page(UrlPath((id, n)): UrlPath<(String, usize)>) -> ApiResult<Response> {
    let (m, ch) = load_chapter(id).await?;
    let source = source_of(&m, &ch);
    let pages = pages_of(&ch, &source).await?;
    let p = pages.get(n).ok_or_else(|| ApiError::not_found("No such page"))?;
    let path = fetch_page(p, &source).await?;
    // Warm the next pages so swiping on stays quick.
    let ahead: Vec<Page> = pages.iter().skip(n + 1).take(3).filter(|p| matches!(p, Page::Remote(_))).cloned().collect();
    if !ahead.is_empty() {
        let source = source.clone();
        tokio::spawn(async move {
            for p in ahead {
                let _ = fetch_page(&p, &source).await;
            }
        });
    }
    read_image(path).await
}

#[derive(Deserialize)]
struct ProgressBody {
    page: usize,
    #[serde(default)]
    total: usize,
}

async fn progress(UrlPath(id): UrlPath<String>, Json(b): Json<ProgressBody>) -> ApiResult<Json<Value>> {
    let (m, ch) = load_chapter(id).await?;
    let total = if b.total > 0 { b.total } else { cached_pages(&ch.id).map(|p| p.len()).unwrap_or(0) };
    // Stored as i64 (and `page + 1` is computed), so never accept an absurd page number.
    if (total > 0 && b.page >= total) || b.page > MAX_PAGE || total > MAX_PAGE {
        return Err(ApiError::bad_request("page is past the end of the chapter"));
    }
    blocking(move || backend::with_db(|c| store::save_progress(c, &m.id, &ch.id, b.page, total))).await?;
    Ok(Json(json!({})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_encoded() {
        assert_eq!(enc("abc-123_x.y~"), "abc-123_x.y~");
        assert_eq!(enc("a/b c"), "a%2Fb%20c");
    }

    #[test]
    fn mime_sniffing() {
        assert_eq!(sniff_mime(&[0x89, b'P', b'N', b'G', 0]), "image/png");
        assert_eq!(sniff_mime(b"RIFF\0\0\0\0WEBPVP8 "), "image/webp");
        assert_eq!(sniff_mime(&[0xff, 0xd8, 0xff]), "image/jpeg");
    }
}
