//! Movies routes (API.md "Movies"): Cinemeta catalogs and search from the desktop's caches, title
//! details with episodes, "play on the PC", and navigating the desktop app.
use std::collections::BTreeMap;

use axum::extract::{Path as UrlPath, Query};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use mv_media::types::AnimeSearchResult;
use mv_ui::Route;
use serde::Deserialize;
use serde_json::{json, Value};
use ui_media::remote;

use crate::bridge;
use crate::server::{ApiError, ApiResult};

pub fn routes() -> Router {
    Router::new()
        .route("/api/movies", get(home))
        .route("/api/movies/search", get(search))
        .route("/api/movies/play", post(play))
        .route("/api/movies/{imdb}", get(detail))
        .route("/api/pc/open", post(open))
}

fn item(r: &AnimeSearchResult) -> Value {
    json!({
        "id": r.id,
        "imdb": r.imdb_id,
        "kind": remote::kind_of(r),
        "title": r.title,
        "year": r.year,
        "description": r.description,
        "genres": r.genres.clone().unwrap_or_default(),
        "score": r.score,
        "poster": r.cover_url,
        "background": remote::background_of(r),
        "logo": remote::logo_of(r),
    })
}

#[derive(Deserialize)]
struct TabQuery {
    #[serde(default)]
    tab: String,
}

async fn home(Query(q): Query<TabQuery>) -> Json<Value> {
    let rows = remote::home(&q.tab).await;
    // Billboard: the first Top 10 row's first title with artwork and a synopsis (like the desktop).
    let hero = rows.iter().find(|r| r.top10).and_then(|r| r.items.iter().find(|i| i.description.is_some() && i.imdb_id.is_some())).map(item);
    // Rows that failed are left out; nothing at all means the PC could not reach the catalogs.
    let error = rows.is_empty().then_some("Movies could not be loaded. Check the PC's internet connection.");
    let rows: Vec<Value> = rows.iter().map(|r| json!({ "id": r.id, "title": r.title, "top10": r.top10, "items": r.items.iter().map(item).collect::<Vec<_>>() })).collect();
    Json(json!({ "hero": hero, "rows": rows, "error": error }))
}

/// Source lookup errors in words for the phone (where to fix them on the PC).
pub(crate) fn friendly(e: String) -> String {
    if e == remote::NO_ADDONS {
        "No streaming addons are installed. Add one in MangaVox on your PC: Settings → Addons.".into()
    } else {
        e
    }
}

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
}

async fn search(Query(q): Query<SearchQuery>) -> ApiResult<Json<Value>> {
    let q = q.q.trim();
    if q.is_empty() {
        return Ok(Json(json!([])));
    }
    let found = remote::search(q).await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(Value::Array(found.iter().map(item).collect())))
}

#[derive(Deserialize)]
struct KindQuery {
    kind: Option<String>,
}

/// A Cinemeta id (`tt123`, `kitsu:45`): it goes into the Cinemeta URL path and the cache key, so
/// no slashes, dots or anything else that could change which URL is fetched.
fn title_id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b':' || b == b'_' || b == b'-')
}

async fn detail(UrlPath(imdb): UrlPath<String>, Query(k): Query<KindQuery>) -> ApiResult<Json<Value>> {
    if !title_id_ok(&imdb) {
        return Err(ApiError::bad_request("not a title id"));
    }
    let kind = match k.kind.as_deref() {
        Some("series") => "series",
        Some("movie") => "movie",
        Some(_) => return Err(ApiError::bad_request("kind must be movie or series")),
        None => "",
    };
    let (d, kind) = match kind {
        "" => match remote::details(&imdb, "movie").await {
            Ok(d) => (d, "movie"),
            Err(_) => (remote::details(&imdb, "series").await.map_err(ApiError::not_found)?, "series"),
        },
        k => (remote::details(&imdb, k).await.map_err(ApiError::not_found)?, k),
    };
    let mut v = item(&d);
    v["kind"] = json!(kind);
    v["cast"] = json!(d.cast.clone().unwrap_or_default());
    v["runtime"] = json!(d.duration_minutes);
    let seasons = if kind == "series" {
        let eps = remote::episodes(&imdb).await.unwrap_or_default();
        let mut by: BTreeMap<u32, Vec<_>> = BTreeMap::new();
        for e in &eps {
            by.entry(remote::season_of(e)).or_default().push(e);
        }
        by.into_iter()
            .map(|(season, mut list)| {
                list.sort_by_key(|e| e.number);
                let episodes: Vec<Value> = list.iter().map(|e| json!({ "episode": e.number, "title": e.title, "thumbnail": e.thumbnail, "overview": Value::Null })).collect();
                json!({ "season": season, "episodes": episodes })
            })
            .collect()
    } else {
        vec![]
    };
    v["seasons"] = Value::Array(seasons);
    Ok(Json(v))
}

#[derive(Deserialize)]
struct PlayBody {
    imdb: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    title: String,
    season: Option<u32>,
    episode: Option<u32>,
}

async fn play(Json(b): Json<PlayBody>) -> ApiResult<Json<Value>> {
    let imdb = b.imdb.trim().to_string();
    if !imdb.starts_with("tt") || imdb.len() > 16 || !imdb[2..].bytes().all(|c| c.is_ascii_digit()) {
        return Err(ApiError::bad_request("imdb must be an IMDb id (tt...)"));
    }
    let movie = b.kind != "series";
    let title = if b.title.trim().is_empty() { imdb.clone() } else { b.title.trim().to_string() };
    eprintln!("[phone] play on PC: {imdb} ({})", if movie { "movie" } else { "series" });
    let req = remote::find_best(title, imdb, movie, b.season, b.episode).await.map_err(|e| ApiError(StatusCode::NOT_FOUND, friendly(e)))?;
    bridge::on_ui(move |w, cx| remote::play_now(req, w, cx)).await.map_err(ApiError::unavailable)?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
struct OpenBody {
    page: String,
    manga_id: Option<String>,
    imdb: Option<String>,
    kind: Option<String>,
    title: Option<String>,
    mal: Option<String>,
}

fn route_for(b: OpenBody) -> Option<Route> {
    let some = |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    Some(match b.page.as_str() {
        "library" => match some(b.manga_id) {
            Some(manga_id) => Route::MangaDetail { manga_id, source_id: None },
            None => Route::Library,
        },
        "movies" => match some(b.imdb) {
            Some(imdb_id) => {
                let kind = if b.kind.as_deref() == Some("series") { "series" } else { "movie" }.to_string();
                Route::MovieDetail { imdb_id, kind, title: some(b.title).unwrap_or_default() }
            }
            None => Route::MoviesHome,
        },
        "anime" => match some(b.mal) {
            Some(mal_id) if crate::api_anime::mal_ok(&mal_id) => Route::AnimeDetail { mal_id, title: some(b.title).unwrap_or_default() },
            Some(_) => return None,
            None => Route::AnimeHome,
        },
        "studio" => Route::Studio,
        "settings" => Route::Settings { tab: None },
        "mobile" => Route::Mobile,
        _ => return None,
    })
}

async fn open(Json(b): Json<OpenBody>) -> ApiResult<Json<Value>> {
    let route = route_for(b).ok_or_else(|| ApiError::bad_request("unknown page"))?;
    bridge::on_ui(move |w, cx| mv_ui::navigate(route, w, cx)).await.map_err(ApiError::unavailable)?;
    Ok(Json(json!({})))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(page: &str) -> OpenBody {
        OpenBody { page: page.into(), manga_id: None, imdb: None, kind: None, title: None, mal: None }
    }

    #[test]
    fn pages_map_to_routes() {
        assert_eq!(route_for(body("library")), Some(Route::Library));
        assert_eq!(route_for(OpenBody { manga_id: Some("m1".into()), ..body("library") }), Some(Route::MangaDetail { manga_id: "m1".into(), source_id: None }));
        assert_eq!(
            route_for(OpenBody { imdb: Some("tt1".into()), kind: Some("series".into()), ..body("movies") }),
            Some(Route::MovieDetail { imdb_id: "tt1".into(), kind: "series".into(), title: String::new() })
        );
        assert_eq!(route_for(body("settings")), Some(Route::Settings { tab: None }));
        assert_eq!(route_for(body("anime")), Some(Route::AnimeHome));
        assert_eq!(route_for(OpenBody { mal: Some("21".into()), ..body("anime") }), Some(Route::AnimeDetail { mal_id: "21".into(), title: String::new() }));
        assert_eq!(route_for(OpenBody { mal: Some("../x".into()), ..body("anime") }), None);
        assert_eq!(route_for(body("nope")), None);
    }

    #[test]
    fn title_ids() {
        assert!(title_id_ok("tt0111161") && title_id_ok("kitsu:123"));
        for bad in ["", "../x", "tt1/../../y", "tt1?x=1", "tt1#", "a.b", "tt1%2F", &"t".repeat(65)] {
            assert!(!title_id_ok(bad), "{bad}");
        }
    }
}
