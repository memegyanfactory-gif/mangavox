//! What Studio made for a chapter, for the phone reader and the manga page (API.md "Chapter media"):
//! the narration with its timeline (so the reader can follow the panel being narrated), the
//! chapter video and its thumbnail, and a manga's chapter videos for "Play all".
//!
//! Stand-in: the shared `mv_recap::media::chapter_media(output_dir) -> ChapterMedia` is being
//! written by the desktop side and does not build yet. [`chapter_media`] here reads the same files
//! with the same meaning (`sync_map.json` `beat_segments`: page, focus_point `[ymin, xmin, ymax,
//! xmax]` on 0..1000, start_ms, end_ms / duration_ms; `narration.wav` when it is newer than the
//! sync map; `video_final.mp4`; `thumbnail.jpg`; the manga's shared thumbnail in its Studio folder).
//! Once that module builds, swap [`chapter_media`] / [`manga_thumb`] for it; the JSON stays the same.
//!
//! Files are only ever found by fixed names inside the chapter's (or the manga's) Studio folder,
//! from ids the library database knows; nothing in a request names a path.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use axum::extract::{Path as UrlPath, Query, Request};
use axum::http::{header, HeaderValue};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use mv_core::repo::{chapter, manga};
use serde_json::{json, Value};
use ui_manga::backend;

use crate::api_manga::{load_chapter, reader_files};
use crate::server::{blocking, ApiError, ApiResult};

pub fn routes() -> Router {
    Router::new()
        .route("/api/chapter/{id}/media", get(media_info))
        .route("/api/chapter/{id}/media/{kind}", get(media_file))
        .route("/api/manga/{id}/videos", get(manga_videos))
        .route("/api/manga/{id}/thumbnail", get(manga_thumbnail))
}

// The same names as `mv_recap::media` / `mv_recap::thumb`.
const SYNC_MAP: &str = "sync_map.json";
const NARRATION: &str = "narration.wav";
const VIDEO: &str = "video_final.mp4";
const THUMB: &str = "thumbnail.jpg";
/// A manga's shared thumbnail, best first: the continuous video's, then the AI and local bases.
const MANGA_THUMBS: [&str; 3] = ["full_recap_thumbnail.jpg", "cover_thumb_16x9.jpg", "cover_local_16x9.jpg"];

/// One narrated passage.
#[derive(Clone, Debug, PartialEq)]
pub struct Beat {
    pub start_ms: u64,
    pub end_ms: u64,
    /// 0-based page in Studio's page order.
    pub page: usize,
    /// The part of the page being read as `[x, y, w, h]` (0..1); `None` for the whole page.
    pub focus: Option<[f32; 4]>,
}

/// What Studio left in a chapter folder (the shared contract's fields the phone uses).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChapterMedia {
    pub narration: Option<PathBuf>,
    pub timeline: Vec<Beat>,
    pub total_ms: u64,
    pub video: Option<PathBuf>,
    pub thumbnail: Option<PathBuf>,
}

fn num(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64).filter(|f| f.is_finite() && *f >= 0.)
}

/// `[ymin, xmin, ymax, xmax]` on 0..1000 (how the narrator writes focus points) as `[x, y, w, h]`
/// on 0..1; `None` for a box that is empty or covers (nearly) the whole page.
pub fn focus_from_point(v: &Value) -> Option<[f32; 4]> {
    let a: Vec<f32> = v.as_array()?.iter().filter_map(|x| x.as_f64().map(|f| (f as f32 / 1000.).clamp(0., 1.))).collect();
    let [y0, x0, y1, x1] = a[..] else { return None };
    let (x0, x1, y0, y1) = (x0.min(x1), x0.max(x1), y0.min(y1), y0.max(y1));
    let (w, h) = (x1 - x0, y1 - y0);
    (w > 0.01 && h > 0.01 && (w < 0.99 || h < 0.99)).then_some([x0, y0, w, h])
}

/// The timeline from `sync_map.json`: `beat_segments` (older maps: `segments`), time ordered, and
/// the total length.
pub fn parse_timeline(map: &Value) -> (Vec<Beat>, u64) {
    let rows: &[Value] = match map.get("beat_segments").and_then(Value::as_array) {
        Some(a) if !a.is_empty() => a,
        _ => map.get("segments").and_then(Value::as_array).map_or(&[][..], Vec::as_slice),
    };
    let mut beats: Vec<Beat> = rows
        .iter()
        .filter_map(|b| {
            let page = num(b, "page")? as usize;
            let start = num(b, "start_ms")? as u64;
            let end = num(b, "end_ms").map(|e| e as u64).or_else(|| num(b, "duration_ms").map(|d| start + d as u64))?;
            (end > start).then(|| Beat { start_ms: start, end_ms: end, page, focus: b.get("focus_point").and_then(focus_from_point) })
        })
        .collect();
    beats.sort_by_key(|b| b.start_ms);
    let total = num(map, "total_duration_ms").map_or(0, |t| t as u64).max(beats.last().map_or(0, |b| b.end_ms));
    (beats, total)
}

fn file(p: PathBuf) -> Option<PathBuf> {
    p.metadata().ok().filter(|m| m.is_file() && m.len() > 0).map(|_| p)
}

fn modified(p: &Path) -> Option<std::time::SystemTime> {
    p.metadata().ok()?.modified().ok()
}

/// Reads a chapter's Studio folder (stand-in for `mv_recap::media::chapter_media`).
pub fn chapter_media(dir: &Path) -> ChapterMedia {
    let map_path = dir.join(SYNC_MAP);
    let (timeline, total_ms) = std::fs::read(&map_path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).map(|m| parse_timeline(&m)).unwrap_or_default();
    // The joined narration only counts when it is newer than the sync map (else it is stale).
    let narration = file(dir.join(NARRATION)).filter(|n| !timeline.is_empty() && modified(n) >= modified(&map_path));
    ChapterMedia { narration, timeline, total_ms, video: file(dir.join(VIDEO)), thumbnail: file(dir.join(THUMB)) }
}

/// A manga's one shared thumbnail, in its Studio folder.
fn manga_thumb(manga_id: &str) -> Option<PathBuf> {
    let dir = mv_core::paths::studio_projects().join(mv_core::paths::safe_name(manga_id));
    MANGA_THUMBS.iter().find_map(|n| file(dir.join(n)))
}

/// Percent-encodes one URL path segment.
fn enc(s: &str) -> String {
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

/// The answer for `/api/chapter/<id>/media`; `page` maps a Studio page to the reader's. `clips`:
/// the per-page narration clips exist (`/api/chapter/<id>/recap` plays them when there is no
/// joined `narration.wav` yet).
pub fn media_json(chapter_id: &str, m: &ChapterMedia, clips: bool, poster_fallback: bool, page: impl Fn(usize) -> usize) -> Value {
    let base = format!("/api/chapter/{}/media", enc(chapter_id));
    let url = |on: bool, kind: &str| on.then(|| format!("{base}/{kind}"));
    json!({
        "listen": !m.timeline.is_empty() && (m.narration.is_some() || clips),
        "narration": url(m.narration.is_some() && !m.timeline.is_empty(), "narration"),
        "video": url(m.video.is_some(), "video"),
        "thumbnail": url(m.thumbnail.is_some() || poster_fallback, "thumbnail"),
        "total_ms": m.total_ms,
        "timeline": m.timeline.iter().map(|b| json!({ "start_ms": b.start_ms, "end_ms": b.end_ms, "page": page(b.page), "focus": b.focus })).collect::<Vec<_>>(),
    })
}

// ---------------------------------------------------------------- routes

async fn media_info(UrlPath(id): UrlPath<String>) -> ApiResult<Json<Value>> {
    let (m, ch) = load_chapter(id).await?;
    let (mid, cid) = (m.id.clone(), ch.id.clone());
    let (media, clips, map, read, mthumb) = blocking(move || {
        let dir = mv_core::paths::studio_chapter(&mid, &cid);
        let map: Option<Value> = std::fs::read(dir.join(SYNC_MAP)).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let read: Option<Value> = std::fs::read(dir.join("studio").join("read.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let clips = ui_manga::recap::find_in(&dir).is_some_and(|r| r.can_read_aloud());
        Ok((chapter_media(&dir), clips, map, read, manga_thumb(&mid).is_some()))
    })
    .await?;
    let listen = !media.timeline.is_empty() && (media.narration.is_some() || clips);
    if !listen && media.video.is_none() {
        return Err(ApiError::not_found("Studio made nothing for this chapter yet"));
    }
    // Studio's pages are matched to the reader's by file name (read.json names them).
    let files = if media.timeline.is_empty() { vec![] } else { reader_files(&m, &ch).await.unwrap_or_default() };
    let names = map.as_ref().map(|v| page_files(v, read.as_ref())).unwrap_or_default();
    let page = |p: usize| reader_page(&names, p, &files);
    Ok(Json(media_json(&ch.id, &media, clips, mthumb, page)))
}

/// Studio page -> its file name: `read.json`'s pages, else a whole-page beat's `panel_image`
/// (panel crops, `pNNNN_MM.jpg`, name no page).
pub fn page_files(map: &Value, read: Option<&Value>) -> HashMap<usize, String> {
    let mut files = read.map(|r| ui_manga::recap::panel_bands(r).1).unwrap_or_default();
    let crop = |n: &str| n.strip_prefix('p').and_then(|s| s.split('.').next()).and_then(|s| s.split_once('_')).is_some_and(|(a, b)| a.parse::<usize>().is_ok() && b.parse::<usize>().is_ok());
    for b in map.get("beat_segments").and_then(Value::as_array).into_iter().flatten() {
        let (Some(page), Some(img)) = (num(b, "page"), b.get("panel_image").and_then(Value::as_str)) else { continue };
        let name = img.replace('\\', "/").rsplit('/').next().unwrap_or_default().to_string();
        if !name.is_empty() && !crop(&name) {
            files.entry(page as usize).or_insert(name);
        }
    }
    files
}

/// The reader page showing Studio page `p`: matched by file name, else the same index.
fn reader_page(names: &HashMap<usize, String>, p: usize, reader: &[String]) -> usize {
    if reader.is_empty() {
        return p;
    }
    names.get(&p).and_then(|f| reader.iter().position(|r| r.eq_ignore_ascii_case(f))).unwrap_or(p).min(reader.len() - 1)
}

/// Range-capable file answer (`ServeFile` handles Range / HEAD / If-Modified-Since).
async fn serve(path: PathBuf, download: Option<String>, req: Request) -> ApiResult<Response> {
    let res = tower_http::services::ServeFile::new(&path).try_call(req).await.map_err(|e| ApiError::internal(format!("Could not read the file: {e}")))?;
    let (mut parts, body) = res.into_parts();
    parts.headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    if let Some(name) = download {
        let ascii: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || " .-_()".contains(c) { c } else { '_' }).collect();
        if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{ascii}\"")) {
            parts.headers.insert(header::CONTENT_DISPOSITION, v);
        }
    }
    Ok(Response::from_parts(parts, axum::body::Body::new(body)))
}

/// `p` when it really is a file inside the Studio projects folder (symlinks followed).
fn inside_projects(p: &Path) -> Option<PathBuf> {
    let real = p.canonicalize().ok()?;
    let root = mv_core::paths::studio_projects().canonicalize().ok()?;
    (real.starts_with(&root) && real.is_file()).then_some(real)
}

async fn media_file(UrlPath((id, kind)): UrlPath<(String, String)>, Query(q): Query<HashMap<String, String>>, req: Request) -> ApiResult<Response> {
    if !matches!(kind.as_str(), "video" | "narration" | "thumbnail") {
        return Err(ApiError::not_found("not found"));
    }
    let (m, ch) = load_chapter(id).await?;
    let label = ui_studio::data::chapter_label(ch.number, ch.title.as_deref());
    let (mid, cid, k) = (m.id.clone(), ch.id.clone(), kind.clone());
    let path = blocking(move || {
        let media = chapter_media(&mv_core::paths::studio_chapter(&mid, &cid));
        let p = match k.as_str() {
            "video" => media.video,
            "narration" => media.narration.filter(|_| !media.timeline.is_empty()),
            _ => media.thumbnail.or_else(|| manga_thumb(&mid)),
        };
        Ok(p.and_then(|p| inside_projects(&p)))
    })
    .await?
    .ok_or_else(|| ApiError::not_found("Studio has not made this yet"))?;
    let download = (kind == "video" && q.get("download").is_some_and(|v| v == "1")).then(|| format!("{} - {label}.mp4", m.title));
    serve(path, download, req).await
}

/// `/api/manga/<id>/videos`: the chapter videos Studio made, in chapter order, with the manga's
/// shared thumbnail.
async fn manga_videos(UrlPath(id): UrlPath<String>) -> ApiResult<Json<Value>> {
    let out = blocking(move || {
        let (m, chapters) = backend::with_db(|c| Ok((manga::get(c, &id)?, chapter::list(c, &id)?)))?;
        let Some(m) = m else { return Ok(None) };
        let thumb = manga_thumb(&m.id).is_some();
        let mut seen = std::collections::HashSet::new();
        let videos: Vec<Value> = chapters
            .iter()
            .filter(|c| seen.insert(mv_core::paths::safe_name(&c.id)))
            .filter_map(|c| {
                let dir = mv_core::paths::studio_chapter(&m.id, &c.id);
                let v = file(dir.join(VIDEO))?;
                let base = format!("/api/chapter/{}/media", enc(&c.id));
                let poster = (file(dir.join(THUMB)).is_some() || thumb).then(|| format!("{base}/thumbnail"));
                Some(json!({
                    "chapter_id": c.id, "number": c.number, "label": ui_studio::data::chapter_label(c.number, c.title.as_deref()),
                    "url": format!("{base}/video"), "poster": poster, "size": v.metadata().map(|m| m.len()).unwrap_or(0),
                }))
            })
            .collect();
        Ok(Some(json!({
            "manga_id": m.id, "title": m.title,
            "thumbnail": thumb.then(|| format!("/api/manga/{}/thumbnail", enc(&m.id))),
            "videos": videos,
        })))
    })
    .await?;
    out.map(Json).ok_or_else(|| ApiError::not_found("Manga not found"))
}

async fn manga_thumbnail(UrlPath(id): UrlPath<String>, req: Request) -> ApiResult<Response> {
    let path = blocking(move || {
        let known = backend::with_db(|c| manga::get(c, &id))?.is_some();
        Ok(known.then(|| manga_thumb(&id)).flatten().and_then(|p| inside_projects(&p)))
    })
    .await?
    .ok_or_else(|| ApiError::not_found("No thumbnail yet"))?;
    serve(path, None, req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_reads_the_sync_map() {
        let map = json!({
            "total_duration_ms": 9000,
            "beat_segments": [
                { "page": 1, "start_ms": 4000, "duration_ms": 2000, "focus_point": [500, 100, 300, 900] },
                { "page": 0, "start_ms": 0, "end_ms": 4000, "focus_point": [0, 0, 1000, 1000] },
                { "page": 3, "start_ms": 7000, "duration_ms": 0 },
                { "page": 3, "start_ms": 7000, "end_ms": 8000, "focus_point": [1, 2] },
            ]
        });
        let (b, total) = parse_timeline(&map);
        assert_eq!(total, 9000);
        assert_eq!(b.len(), 3, "zero-length beats are dropped");
        assert_eq!((b[0].page, b[0].start_ms, b[0].end_ms, b[0].focus), (0, 0, 4000, None), "the whole page is no focus");
        // [ymin, xmin, ymax, xmax] on 0..1000, corners in any order -> [x, y, w, h].
        let f = b[1].focus.unwrap();
        for (got, want) in f.iter().zip([0.1, 0.3, 0.8, 0.2]) {
            assert!((got - want).abs() < 1e-6, "{f:?}");
        }
        assert_eq!(b[1].end_ms, 6000);
        assert_eq!(b[2].focus, None);
        assert_eq!(parse_timeline(&json!({})), (vec![], 0));
        assert_eq!(parse_timeline(&json!({ "segments": [{ "page": 0, "start_ms": 0, "end_ms": 5 }] })).0.len(), 1);
    }

    #[test]
    fn media_is_found_by_fixed_names_and_mapped_to_json() {
        let d = std::env::temp_dir().join(format!("mv-phone-media-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(chapter_media(&d), ChapterMedia::default());
        std::fs::write(d.join(SYNC_MAP), br#"{"beat_segments":[{"page":2,"start_ms":0,"end_ms":10}]}"#).unwrap();
        std::fs::write(d.join(NARRATION), b"x").unwrap();
        std::fs::write(d.join(VIDEO), b"x").unwrap();
        std::fs::write(d.join(THUMB), b"").unwrap(); // empty: not a thumbnail
        let m = chapter_media(&d);
        assert_eq!(m.narration, Some(d.join(NARRATION)));
        assert_eq!(m.video, Some(d.join(VIDEO)));
        assert_eq!(m.thumbnail, None);
        assert_eq!((m.timeline.len(), m.total_ms), (1, 10));
        let v = media_json("c 1", &m, false, false, |p| p + 10);
        assert_eq!(v["listen"], true);
        assert_eq!(v["narration"], "/api/chapter/c%201/media/narration");
        assert_eq!(v["video"], "/api/chapter/c%201/media/video");
        assert_eq!(v["thumbnail"], Value::Null);
        assert_eq!(v["timeline"][0]["page"], 12);
        assert_eq!(v["timeline"][0]["focus"], Value::Null);
        // The manga's shared thumbnail stands in for a chapter without its own.
        assert_eq!(media_json("c", &m, false, true, |p| p)["thumbnail"], "/api/chapter/c/media/thumbnail");
        // Only per-page clips: listen through them, no joined narration offered.
        let clips_only = ChapterMedia { narration: None, ..m.clone() };
        let v = media_json("c", &clips_only, true, false, |p| p);
        assert_eq!((v["listen"].clone(), v["narration"].clone()), (json!(true), Value::Null));
        // Without a timeline nothing can be followed.
        let bare = ChapterMedia { timeline: vec![], ..m };
        assert_eq!(media_json("c", &bare, true, false, |p| p)["listen"], false);
        let _ = std::fs::remove_dir_all(&d);
    }
}
