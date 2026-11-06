//! History routes (API.md "History"): what the desktop's History pages show, for the phone's
//! History view. Manga reading history (one entry per manga, the chapter to resume) and watch
//! history (anime, movies, series: the resume point per title), newest first, paged so a long
//! history never arrives in one piece. Removing one entry works like the desktop's remove button.
//! Showing history is always fine; nothing here records anything (so private mode does not matter).
use axum::extract::Query;
use axum::routing::{get, post};
use axum::{Json, Router};
use mv_core::repo::history::{self, ReadingHistory};
use mv_core::repo::media::{self, PlaybackState};
use serde::Deserialize;
use serde_json::{json, Value};
use ui_manga::{backend, logic};

use crate::server::{blocking, ApiError, ApiResult};

/// Entries one page may hold, and the default.
const MAX_LIMIT: usize = 100;
const DEFAULT_LIMIT: usize = 30;
/// How much history is read from the database (the desktop shows 200 watched titles and at most
/// 500 manga).
const READ_MAX: usize = 500;
const MAX_ID: usize = 512;

pub fn routes() -> Router {
    Router::new().route("/api/history", get(list)).route("/api/history/remove", post(remove))
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

// ---------------------------------------------------------------- pure helpers

/// Days since 1970-01-01 for a calendar date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// SQLite's `YYYY-MM-DD HH:MM:SS` (UTC, what `read_at` holds) as unix milliseconds.
fn sqlite_utc_ms(s: &str) -> Option<i64> {
    let (date, time) = s.trim().split_once([' ', 'T'])?;
    let mut d = date.split('-');
    let y: i64 = d.next()?.parse().ok()?;
    let mo: i64 = d.next()?.parse().ok()?;
    let da: i64 = d.next()?.parse().ok()?;
    let mut t = time.trim_end_matches('Z').split(':');
    let h: i64 = t.next()?.parse().ok()?;
    let mi: i64 = t.next()?.parse().ok()?;
    let sec: i64 = t.next().unwrap_or("0").split('.').next()?.parse().ok()?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&da) || !(0..24).contains(&h) || !(0..60).contains(&mi) || !(0..61).contains(&sec) {
        return None;
    }
    Some((days_from_civil(y, mo, da) * 86_400 + h * 3600 + mi * 60 + sec) * 1000)
}

/// "Ch. 12 - Title", "Ch. 12", the title alone, or "Oneshot": the desktop's wording.
fn chapter_text(number: Option<f64>, title: Option<&str>) -> String {
    let title = title.map(str::trim).filter(|t| !t.is_empty());
    match (number, title) {
        (Some(n), Some(t)) => format!("Ch. {} - {t}", logic::fmt_number(n)),
        (Some(n), None) => format!("Ch. {}", logic::fmt_number(n)),
        (None, Some(t)) => t.to_string(),
        (None, None) => "Oneshot".into(),
    }
}

/// A media id's `(query field, id)` for the player: `anime:mal:<id>` -> `("mal", id)`,
/// `movie:imdb:<id>` / `series:imdb:<id>` -> `("imdb", id)`. Anything else (the desktop cannot
/// open it either) is `None`.
fn media_ref(media_id: &str) -> Option<(&'static str, &str)> {
    let mut p = media_id.splitn(3, ':');
    let (kind, src, id) = (p.next()?, p.next()?, p.next()?);
    let ok = !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'_' | b'-'));
    match (kind, src) {
        ("anime", "mal") if ok => Some(("mal", id)),
        ("movie" | "series", "imdb") if ok => Some(("imdb", id)),
        _ => None,
    }
}

/// One page of a list: the slice and the offset of the next page (`None` at the end).
fn page_of<T>(all: Vec<T>, offset: usize, limit: usize) -> (usize, Vec<T>, Option<usize>) {
    let total = all.len();
    let items: Vec<T> = all.into_iter().skip(offset).take(limit).collect();
    let end = offset.saturating_add(items.len());
    (total, items, (end < total).then_some(end))
}

fn manga_item(h: &ReadingHistory) -> Value {
    let cover = h.cover_url.as_deref().filter(|c| !c.trim().is_empty()).map(|_| format!("/api/manga/{}/cover", enc(&h.manga_id)));
    json!({
        "kind": "manga",
        "key": h.manga_id,
        "manga_id": h.manga_id,
        "chapter_id": h.chapter_id,
        "title": h.manga_title,
        "chapter": chapter_text(h.chapter_number, h.chapter_title.as_deref()),
        "cover": cover,
        "page": h.page,
        "at": sqlite_utc_ms(&h.read_at),
    })
}

/// Only web artwork goes to the phone (through `/api/img`); a path on the PC means nothing there.
fn poster_of(url: &Option<String>) -> Option<&str> {
    url.as_deref().filter(|u| u.starts_with("http://") || u.starts_with("https://"))
}

fn watch_item(h: &PlaybackState) -> Option<Value> {
    let (field, id) = media_ref(&h.media_id)?;
    let progress = if h.duration_secs > 0.0 { (h.position_secs / h.duration_secs).clamp(0.0, 1.0) } else { 0.0 };
    let mut v = json!({
        "kind": h.kind,
        "key": h.media_id,
        "media_id": h.media_id,
        "title": h.title,
        "poster": poster_of(&h.cover_url),
        "season": h.season_number,
        "episode": h.episode_number,
        "position": h.position_secs,
        "duration": h.duration_secs,
        "progress": progress,
        "completed": h.completed,
        "at": h.updated_at,
    });
    v[field] = json!(id);
    Some(v)
}

// ---------------------------------------------------------------- routes

#[derive(Deserialize)]
struct ListQuery {
    /// `manga` (default) or `watch`.
    #[serde(default)]
    kind: String,
    /// For `watch`: `anime`, `movie` or `series`; empty is everything.
    #[serde(default, rename = "type")]
    media: String,
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
}

async fn list(Query(q): Query<ListQuery>) -> ApiResult<Json<Value>> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let (kind, items): (&str, Vec<Value>) = match q.kind.as_str() {
        "" | "manga" => {
            let rows = blocking(|| backend::with_db(|c| history::list(c, READ_MAX))).await?;
            ("manga", rows.iter().map(manga_item).collect())
        }
        "watch" => {
            let media_kind = match q.media.as_str() {
                "" | "all" => None,
                "anime" => Some("anime"),
                "movie" => Some("movie"),
                "series" => Some("series"),
                _ => return Err(ApiError::bad_request("type must be anime, movie or series")),
            };
            let rows = blocking(move || backend::with_db(|c| media::list_history(c, media_kind, None, READ_MAX))).await?;
            ("watch", rows.iter().filter_map(watch_item).collect())
        }
        _ => return Err(ApiError::bad_request("kind must be manga or watch")),
    };
    let (total, items, next) = page_of(items, q.offset, limit);
    Ok(Json(json!({ "kind": kind, "items": items, "total": total, "next": next, "private": mv_core::privacy::active() })))
}

#[derive(Deserialize)]
struct RemoveBody {
    kind: String,
    /// The entry's `key`: a manga id, or a media id.
    id: String,
}

async fn remove(Json(b): Json<RemoveBody>) -> ApiResult<Json<Value>> {
    if b.id.is_empty() || b.id.len() > MAX_ID {
        return Err(ApiError::bad_request("not an id"));
    }
    let id = b.id;
    match b.kind.as_str() {
        "manga" => {
            blocking(move || backend::with_db(|c| history::remove(c, &id))).await?;
            // Pages the desktop keeps alive (Library, History) reread after this.
            backend::bump_data();
        }
        "watch" => {
            blocking(move || backend::with_db(|c| media::remove_history(c, &id))).await?;
        }
        _ => return Err(ApiError::bad_request("kind must be manga or watch")),
    }
    Ok(Json(json!({})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_times_become_unix_ms() {
        assert_eq!(sqlite_utc_ms("1970-01-01 00:00:00"), Some(0));
        assert_eq!(sqlite_utc_ms("2024-03-01 12:30:15"), Some(1_709_296_215_000));
        assert_eq!(sqlite_utc_ms("2024-03-01T12:30:15Z"), Some(1_709_296_215_000));
        assert_eq!(sqlite_utc_ms("2024-13-01 00:00:00"), None);
        assert_eq!(sqlite_utc_ms("yesterday"), None);
    }

    #[test]
    fn chapters_read_like_the_desktop() {
        assert_eq!(chapter_text(Some(12.0), Some(" Title ")), "Ch. 12 - Title");
        assert_eq!(chapter_text(Some(12.5), None), "Ch. 12.5");
        assert_eq!(chapter_text(None, Some("Special")), "Special");
        assert_eq!(chapter_text(None, Some("  ")), "Oneshot");
    }

    #[test]
    fn media_ids_map_to_player_fields() {
        assert_eq!(media_ref("anime:mal:21"), Some(("mal", "21")));
        assert_eq!(media_ref("series:imdb:tt0944947"), Some(("imdb", "tt0944947")));
        assert_eq!(media_ref("movie:imdb:tt1"), Some(("imdb", "tt1")));
        assert_eq!(media_ref("anime:imdb:tt1"), None);
        assert_eq!(media_ref("movie:imdb:../x"), None);
        assert_eq!(media_ref("junk"), None);
    }

    #[test]
    fn paging_reports_the_next_offset() {
        let all: Vec<u32> = (0..5).collect();
        let (total, items, next) = page_of(all.clone(), 0, 2);
        assert_eq!((total, items, next), (5, vec![0, 1], Some(2)));
        let (_, items, next) = page_of(all.clone(), 4, 2);
        assert_eq!((items, next), (vec![4], None));
        let (_, items, next) = page_of(all, 9, 2);
        assert_eq!((items.len(), next), (0, None));
    }

    #[test]
    fn watch_items_carry_what_the_player_needs() {
        let h = PlaybackState {
            media_id: "series:imdb:tt9".into(),
            kind: "series".into(),
            title: "Show".into(),
            cover_url: Some("https://x/p.jpg".into()),
            provider_id: None,
            episode_number: Some(3),
            season_number: Some(2),
            position_secs: 300.0,
            duration_secs: 1200.0,
            completed: false,
            updated_at: 1_700_000_000_000,
        };
        let v = watch_item(&h).unwrap();
        assert_eq!(v["imdb"], "tt9");
        assert_eq!((v["season"].as_i64(), v["episode"].as_i64()), (Some(2), Some(3)));
        assert_eq!(v["progress"].as_f64(), Some(0.25));
        assert_eq!(v["poster"], "https://x/p.jpg");
        let local = PlaybackState { cover_url: Some("C:/covers/a.jpg".into()), ..h.clone() };
        assert!(watch_item(&local).unwrap()["poster"].is_null());
        assert!(watch_item(&PlaybackState { media_id: "other".into(), ..h }).is_none());
    }
}
