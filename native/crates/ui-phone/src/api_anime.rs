//! Anime routes (API.md "Anime"): home rows, search, details with paged episodes from the same
//! Jikan caches as the desktop Anime pages, and "play on the PC".
use axum::extract::{Path as UrlPath, Query};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use mv_media::types::{AnimeEpisode, AnimeSearchResult};
use serde::Deserialize;
use serde_json::{json, Value};
use ui_media::remote;

use crate::api_media::friendly;
use crate::bridge;
use crate::server::{ApiError, ApiResult};

pub fn routes() -> Router {
    Router::new()
        .route("/api/anime", get(home))
        .route("/api/anime/search", get(search))
        .route("/api/anime/play", post(play))
        .route("/api/anime/{mal}", get(detail))
        .route("/api/anime/{mal}/episodes", get(episodes))
}

/// A MAL id: digits only (it goes into Jikan URLs and cache keys).
pub(crate) fn mal_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= 10 && id.bytes().all(|b| b.is_ascii_digit())
}

pub(crate) fn item(r: &AnimeSearchResult) -> Value {
    json!({
        "id": r.id,
        "mal": r.id,
        "kind": "anime",
        "title": r.title,
        "year": r.year,
        "description": r.description,
        "genres": r.genres.clone().unwrap_or_default(),
        "score": r.score,
        "poster": r.cover_url,
        "background": r.banner_url.clone().or_else(|| r.cover_url.clone()),
        "episodes": r.episode_count,
        "format": r.anime_type,
        "status": r.status,
    })
}

fn episode(e: &AnimeEpisode) -> Value {
    json!({ "episode": e.number, "title": e.title, "thumbnail": e.thumbnail, "aired": e.air_date, "filler": false })
}

fn next_page(eps: &[AnimeEpisode], page: u32) -> Option<u32> {
    (eps.len() >= remote::ANIME_EPISODE_PAGE).then_some(page + 1)
}

#[derive(Deserialize)]
struct TabQuery {
    #[serde(default)]
    tab: String,
}

async fn home(Query(q): Query<TabQuery>) -> Json<Value> {
    let rows = remote::anime_home(&q.tab).await;
    // Billboard: the first row's first title with a synopsis (the desktop carousel's first slide).
    let hero = rows.first().and_then(|r| r.items.iter().find(|i| i.description.is_some())).map(item);
    let error = rows.is_empty().then_some("Anime could not be loaded. Check the PC's internet connection.");
    let rows: Vec<Value> = rows.iter().map(|r| json!({ "id": r.id, "title": r.title, "top10": r.top10, "items": r.items.iter().map(item).collect::<Vec<_>>() })).collect();
    Json(json!({ "hero": hero, "rows": rows, "error": error }))
}

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
}

async fn search(Query(q): Query<SearchQuery>) -> ApiResult<Json<Value>> {
    let q: String = q.q.trim().chars().take(200).collect();
    if q.is_empty() {
        return Ok(Json(json!([])));
    }
    let found = remote::anime_search(&q).await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(Value::Array(found.iter().map(item).collect())))
}

async fn detail(UrlPath(mal): UrlPath<String>) -> ApiResult<Json<Value>> {
    if !mal_ok(&mal) {
        return Err(ApiError::bad_request("not a MAL id"));
    }
    let d = remote::anime_details(&mal).await.map_err(ApiError::not_found)?;
    let eps = remote::anime_episodes(&mal, 1).await.unwrap_or_default();
    let mut v = item(&d);
    v["episodes_list"] = Value::Array(eps.iter().map(episode).collect());
    v["next_page"] = json!(next_page(&eps, 1));
    Ok(Json(v))
}

#[derive(Deserialize)]
struct PageQuery {
    page: Option<u32>,
}

async fn episodes(UrlPath(mal): UrlPath<String>, Query(q): Query<PageQuery>) -> ApiResult<Json<Value>> {
    if !mal_ok(&mal) {
        return Err(ApiError::bad_request("not a MAL id"));
    }
    let page = q.page.unwrap_or(1).clamp(1, 1000);
    let eps = remote::anime_episodes(&mal, page).await.map_err(|e| ApiError(StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(json!({ "episodes": eps.iter().map(episode).collect::<Vec<_>>(), "next_page": next_page(&eps, page) })))
}

#[derive(Deserialize)]
struct PlayBody {
    mal: String,
    #[serde(default)]
    title: String,
    episode: Option<u32>,
}

async fn play(Json(b): Json<PlayBody>) -> ApiResult<Json<Value>> {
    let mal = b.mal.trim().to_string();
    if !mal_ok(&mal) {
        return Err(ApiError::bad_request("mal must be a MyAnimeList id"));
    }
    let title: String = b.title.trim().chars().take(200).collect();
    eprintln!("[phone] play anime on PC: mal {mal} episode {:?}", b.episode);
    let req = remote::find_best_anime(title, mal, b.episode.map(|e| e.clamp(1, 100_000))).await.map_err(|e| ApiError(StatusCode::NOT_FOUND, friendly(e)))?;
    bridge::on_ui(move |w, cx| remote::play_now(req, w, cx)).await.map_err(ApiError::unavailable)?;
    Ok(Json(json!({})))
}
