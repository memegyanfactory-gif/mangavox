//! Browse (API.md "Browse"): the installed manga sources the desktop Browse lists (built-in sites
//! the user added, repository plugins), their Popular / Latest / Search pages with covers, opening
//! a title (saved like the desktop does: browse-only until added), adding it to the library or
//! Favorites, and queueing chapter downloads in the desktop download manager.
//!
//! Covers come through `/api/sources/cover`, fetched by the source's own backend (referers, wall
//! solver) into the shared covers cache. Only URLs this server itself handed out are fetched: each
//! cover link carries a keyed hash of (source, url) made with a per-run secret.
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use axum::extract::{Path as UrlPath, Query};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use mv_core::repo::{chapter, manga};
use mv_sources::{MangaSearchResult, Source, SourceError, SourceInfo};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use ui_manga::backend;

use crate::server::{blocking, ApiError, ApiResult};

pub fn routes() -> Router {
    Router::new()
        .route("/api/sources", get(sources))
        .route("/api/sources/cover", get(cover))
        .route("/api/sources/{sid}/browse", get(browse))
        .route("/api/sources/{sid}/open", post(open))
        .route("/api/manga/{id}/library", post(library))
        .route("/api/manga/{id}/favorite", post(favorite))
        .route("/api/manga/{id}/download", post(download))
}

/// How long one source request may take before the phone is told it timed out.
const SOURCE_SECS: u64 = 30;
/// Longest search text and source title id accepted.
const MAX_QUERY: usize = 200;
const MAX_ID: usize = 512;
/// Chapters one download / Studio request may name.
pub(crate) const MAX_CHAPTERS: usize = 3000;
/// Results remembered for opening a title without asking its source for the list again.
const KEEP_RESULTS: usize = 4000;

// ---------------------------------------------------------------- helpers

/// The sources Browse lists on the desktop: every installed plugin and the built-in sites the user
/// added (adult sites only appear when the user added them there, after the 18+ check).
pub(crate) fn visible(all: Vec<SourceInfo>) -> Vec<SourceInfo> {
    backend::installed_sources(all)
}

/// A source the desktop Browse lists, else 404.
async fn source_for(sid: &str) -> ApiResult<std::sync::Arc<dyn Source>> {
    let reg = backend::registry().await;
    if !visible(reg.list()).iter().any(|s| s.id == sid) {
        return Err(ApiError::not_found("No such source"));
    }
    reg.get(sid).ok_or_else(|| ApiError::not_found("No such source"))
}

fn secret() -> &'static [u8; 32] {
    static S: OnceLock<[u8; 32]> = OnceLock::new();
    S.get_or_init(rand::random)
}

/// The keyed hash a cover link carries.
pub(crate) fn cover_key(source: &str, url: &str) -> String {
    let mut h = Sha256::new();
    h.update(secret());
    h.update(source.as_bytes());
    h.update([0]);
    h.update(url.as_bytes());
    h.finalize().iter().take(16).map(|b| format!("{b:02x}")).collect()
}

fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `/api/sources/cover?...` for a cover the source gave, or `None` for no (or an odd) URL.
pub(crate) fn cover_link(source: &str, url: Option<&str>) -> Option<String> {
    let url = url.map(str::trim).filter(|u| u.starts_with("http://") || u.starts_with("https://"))?;
    Some(format!("/api/sources/cover?s={}&u={}&k={}", pct(source), pct(url), cover_key(source, url)))
}

/// One search result as the phone shows it.
pub(crate) fn result_json(source: &str, r: &MangaSearchResult) -> Value {
    json!({
        "id": r.source_id,
        "title": r.title,
        "cover": cover_link(source, r.cover_url.as_deref()),
        "author": r.author,
        "status": r.status,
        "adult": r.adult,
    })
}

fn results() -> std::sync::MutexGuard<'static, HashMap<(String, String), MangaSearchResult>> {
    static R: OnceLock<Mutex<HashMap<(String, String), MangaSearchResult>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|e| e.into_inner())
}

fn source_err(e: SourceError) -> ApiError {
    match e {
        SourceError::Unsupported(what) => ApiError(StatusCode::UNPROCESSABLE_ENTITY, format!("This source has no {what} list")),
        SourceError::Wall { .. } | SourceError::SolverRequired(_) => ApiError(StatusCode::BAD_GATEWAY, "The site asks for a browser check. Open this source once in MangaVox on the PC, then try again.".into()),
        e => ApiError(StatusCode::BAD_GATEWAY, format!("The source did not answer: {e}")),
    }
}

/// A JSON body, with a JSON error when it does not parse.
fn body<T: serde::de::DeserializeOwned>(raw: &[u8]) -> ApiResult<T> {
    serde_json::from_slice(raw).map_err(|e| ApiError::bad_request(format!("Bad request body: {e}")))
}

// ---------------------------------------------------------------- GET /api/sources

async fn sources() -> ApiResult<Json<Value>> {
    let reg = backend::registry().await;
    let list: Vec<Value> = visible(reg.list()).into_iter().map(|s| json!({ "id": s.id, "name": s.name, "lang": s.lang, "nsfw": s.nsfw })).collect();
    Ok(Json(json!({ "sources": list })))
}

// ---------------------------------------------------------------- GET /api/sources/<sid>/browse

#[derive(Deserialize)]
struct BrowseQ {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    q: String,
    page: Option<u32>,
}

async fn browse(UrlPath(sid): UrlPath<String>, Query(q): Query<BrowseQ>) -> ApiResult<Json<Value>> {
    let page = q.page.unwrap_or(1).clamp(1, 1000);
    let query = q.q.trim().to_string();
    let kind = match q.kind.as_str() {
        "" | "popular" => "popular",
        "latest" => "latest",
        "search" if query.is_empty() => return Err(ApiError::bad_request("Type something to search for")),
        "search" if query.chars().count() > MAX_QUERY => return Err(ApiError::bad_request("That search is too long")),
        "search" => "search",
        _ => return Err(ApiError::bad_request("\"kind\" must be popular, latest or search")),
    };
    let src = source_for(&sid).await?;
    let fut = async {
        match kind {
            "latest" => src.get_latest_manga(page).await,
            "search" => src.search_manga(&query, page).await,
            _ => src.get_popular_manga(page).await,
        }
    };
    let list = backend::within(SOURCE_SECS, fut).await.ok_or_else(|| ApiError(StatusCode::GATEWAY_TIMEOUT, "The source took too long to answer".into()))?.map_err(source_err)?;
    {
        let mut keep = results();
        if keep.len() + list.len() > KEEP_RESULTS {
            keep.clear();
        }
        for r in &list {
            keep.insert((sid.clone(), r.source_id.clone()), r.clone());
        }
    }
    let items: Vec<Value> = list.iter().map(|r| result_json(&sid, r)).collect();
    Ok(Json(json!({ "source": sid, "kind": kind, "page": page, "items": items, "more": !items.is_empty() })))
}

// ---------------------------------------------------------------- GET /api/sources/cover

#[derive(Deserialize)]
struct CoverQ {
    s: String,
    u: String,
    k: String,
}

async fn cover(Query(q): Query<CoverQ>) -> ApiResult<Response> {
    use subtle::ConstantTimeEq as _;
    let want = cover_key(&q.s, &q.u);
    if want.as_bytes().ct_eq(q.k.as_bytes()).unwrap_u8() != 1 {
        return Err(ApiError(StatusCode::FORBIDDEN, "not a cover this server listed".into()));
    }
    let path = backend::ensure_cover(q.u.clone(), Some(q.s.clone())).await.ok_or_else(|| ApiError::not_found("Cover not available"))?;
    let (bytes, path) = blocking(move || Ok((std::fs::read(&path)?, path))).await?;
    let mime = mime_guess::from_path(&path).first_raw().filter(|m| m.starts_with("image/")).unwrap_or("image/jpeg");
    let mut r = (StatusCode::OK, bytes).into_response();
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=604800"));
    Ok(r)
}

// ---------------------------------------------------------------- POST /api/sources/<sid>/open

#[derive(Deserialize)]
struct OpenBody {
    id: String,
}

/// Saves the title and its chapters like the desktop's Browse does (browse-only until added; in
/// private mode the new row is noted for clearing) and returns its library id.
async fn open(UrlPath(sid): UrlPath<String>, raw: axum::body::Bytes) -> ApiResult<Json<Value>> {
    let b: OpenBody = body(&raw)?;
    let id = b.id.trim().to_string();
    if id.is_empty() || id.len() > MAX_ID {
        return Err(ApiError::bad_request("Bad title id"));
    }
    let src = source_for(&sid).await?;
    let known = results().get(&(sid.clone(), id.clone())).cloned();
    let r = match known {
        Some(r) => r,
        None => {
            let d = backend::within(SOURCE_SECS, src.get_manga_details(&id)).await.ok_or_else(|| ApiError(StatusCode::GATEWAY_TIMEOUT, "The source took too long to answer".into()))?.map_err(source_err)?;
            MangaSearchResult { source_id: id, title: d.title, cover_url: d.cover_url, author: d.author, status: d.status, ..Default::default() }
        }
    };
    let m = ui_manga::ops::prepare_from_source(sid, r).await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, format!("Could not open this title: {e:#}")))?;
    Ok(Json(json!({ "manga_id": m.id })))
}

// ---------------------------------------------------------------- library / favorites

#[derive(Deserialize)]
struct OnBody {
    on: bool,
}

async fn known_manga(id: String) -> ApiResult<manga::Manga> {
    blocking(move || backend::with_db(|c| manga::get(c, &id))).await?.ok_or_else(|| ApiError::not_found("Manga not found"))
}

pub(crate) fn in_library(id: &str) -> bool {
    !backend::browse_only().contains(id)
}

async fn library(UrlPath(id): UrlPath<String>, raw: axum::body::Bytes) -> ApiResult<Json<Value>> {
    let b: OnBody = body(&raw)?;
    let m = known_manga(id).await?;
    let mid = m.id.clone();
    let fav = blocking(move || {
        backend::set_in_library(&mid, b.on);
        // A favourite lives in the library: taking a title out also unpins it.
        if !b.on && m.is_favorite {
            backend::with_db(|c| manga::toggle_favorite(c, &mid))?;
        }
        Ok(b.on && m.is_favorite)
    })
    .await?;
    backend::bump_data();
    Ok(Json(json!({ "in_library": b.on, "favorite": fav })))
}

async fn favorite(UrlPath(id): UrlPath<String>, raw: axum::body::Bytes) -> ApiResult<Json<Value>> {
    let b: OnBody = body(&raw)?;
    let m = known_manga(id).await?;
    let mid = m.id.clone();
    blocking(move || {
        if m.is_favorite != b.on {
            backend::with_db(|c| manga::toggle_favorite(c, &mid))?;
        }
        // Favouriting a browse-only title adds it to the library (as on the desktop).
        if b.on {
            backend::set_in_library(&mid, true);
        }
        Ok(())
    })
    .await?;
    backend::bump_data();
    Ok(Json(json!({ "favorite": b.on, "in_library": b.on || in_library(&m.id) })))
}

// ---------------------------------------------------------------- downloads

#[derive(Deserialize)]
pub(crate) struct ChaptersBody {
    /// Chapter ids of this manga; `None` = all of them.
    #[serde(default)]
    pub chapters: Option<Vec<String>>,
}

/// The manga and the chapters of it a request picked (all when it names none), in reading order.
pub(crate) async fn picked(id: String, want: Option<Vec<String>>) -> ApiResult<(manga::Manga, Vec<chapter::Chapter>)> {
    if want.as_ref().is_some_and(|w| w.len() > MAX_CHAPTERS) {
        return Err(ApiError::bad_request("Too many chapters at once"));
    }
    let lang = backend::preferred_language();
    let (m, list) = blocking(move || {
        backend::with_db(|c| {
            let Some(m) = manga::get(c, &id)? else { return Ok((None, vec![])) };
            let list = ui_manga::store::chapters(c, &m.id, &lang)?;
            Ok((Some(m), list))
        })
    })
    .await?;
    let m = m.ok_or_else(|| ApiError::not_found("Manga not found"))?;
    let list: Vec<chapter::Chapter> = match want {
        Some(w) => list.into_iter().filter(|c| w.contains(&c.id)).collect(),
        None => list,
    };
    if list.is_empty() {
        return Err(ApiError::bad_request("None of these chapters belong to this manga"));
    }
    Ok((m, list))
}

/// Queues `chapters` in the desktop download manager (already saved or queued ones are skipped
/// there). Returns how many were asked for.
pub(crate) async fn queue_downloads(m: &manga::Manga, chapters: &[chapter::Chapter]) -> ApiResult<usize> {
    let jobs: Vec<(String, String, String, String)> =
        chapters.iter().filter(|c| c.download_status != "complete").map(|c| (m.id.clone(), m.title.clone(), c.id.clone(), ui_manga::logic::chapter_short(c))).collect();
    let n = jobs.len();
    if n > 0 {
        crate::bridge::on_ui(move |_, cx| ui_manga::downloads(cx).update(cx, |d, cx| d.enqueue_chapters(jobs, cx))).await.map_err(ApiError::unavailable)?;
    }
    Ok(n)
}

async fn download(UrlPath(id): UrlPath<String>, raw: axum::body::Bytes) -> ApiResult<Json<Value>> {
    let b: ChaptersBody = body(&raw)?;
    let (m, list) = picked(id, b.chapters).await?;
    let queued = queue_downloads(&m, &list).await?;
    Ok(Json(json!({ "queued": queued, "saved": list.len() - queued })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_links_are_keyed_to_this_server() {
        let l = cover_link("mangadex", Some("https://uploads.example/c/1.jpg?x=1&y")).unwrap();
        assert!(l.starts_with("/api/sources/cover?s=mangadex&u=https%3A%2F%2Fuploads.example%2Fc%2F1.jpg%3Fx%3D1%26y&k="), "{l}");
        let k = l.rsplit("k=").next().unwrap();
        assert_eq!(k, cover_key("mangadex", "https://uploads.example/c/1.jpg?x=1&y"));
        assert_ne!(k, cover_key("other", "https://uploads.example/c/1.jpg?x=1&y"), "bound to the source");
        assert_ne!(k, cover_key("mangadex", "http://127.0.0.1/"), "bound to the url");
        assert_eq!(cover_link("s", None), None);
        assert_eq!(cover_link("s", Some("file:///etc/passwd")), None);
        assert_eq!(cover_link("s", Some("  ")), None);
    }

    #[test]
    fn results_map_to_json() {
        let r = MangaSearchResult { source_id: "abc".into(), title: "Solo".into(), cover_url: Some("https://x.example/a.png".into()), adult: true, ..Default::default() };
        let v = result_json("src", &r);
        assert_eq!((v["id"].as_str(), v["title"].as_str(), v["adult"].as_bool()), (Some("abc"), Some("Solo"), Some(true)));
        assert!(v["cover"].as_str().unwrap().starts_with("/api/sources/cover?s=src&u="));
        assert!(v.get("author").is_some() && v.get("status").is_some());
    }
}
