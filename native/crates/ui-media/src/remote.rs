//! Movies data and desktop playback for the phone companion (ui-phone): the catalogs, caches and
//! stream lookup the Movies pages use, without a view. Everything async here runs on tokio.
use std::time::Duration;

use gpui_kit::*;
use mv_core::cache::DataCache;
use mv_media::types::{AnimeEpisode, AnimeSearchResult, AnimeTorrentStream};
use mv_ui::nav::PlayRequest;

use crate::common::{self, DAY, EPISODES_TTL};
use crate::movies_home::{self, catalog, catalog_key, cinemeta};
use crate::streams::{self, PlayBase};

pub use crate::movies_home::{background_of, kind_of, logo_of};

/// A fresh cached value, else fetch and cache; a failed fetch falls back to the stale value.
async fn cached<T>(key: String, ttl: Duration, fut: impl std::future::Future<Output = Result<T, String>> + Send + 'static) -> Result<T, String>
where
    T: serde::Serialize + serde::de::DeserializeOwned + Send + 'static,
{
    let hit = DataCache::shared().get::<T>(&key);
    match hit {
        Some((v, age)) if age < ttl => Ok(v),
        stale => match common::cached(key, fut).await {
            Ok(v) => Ok(v),
            Err(e) => stale.map(|(v, _)| v).ok_or(e),
        },
    }
}

/// One row of a Movies home tab.
pub struct Row {
    pub id: &'static str,
    pub title: String,
    pub top10: bool,
    pub items: Vec<AnimeSearchResult>,
}

/// The rows of a home tab ("home" | "movies" | "series"), from the same cache as the desktop page.
/// Rows that fail to load are left out.
pub async fn home(tab: &str) -> Vec<Row> {
    let jobs = movies_home::home_rows(tab).into_iter().map(|(id, title, top10, movie, cat, genre)| async move {
        let items = cached(catalog_key(movie, cat, genre), DAY, catalog(movie, cat, genre, 1)).await.ok()?;
        Some(Row { id, title, top10, items })
    });
    futures::future::join_all(jobs).await.into_iter().flatten().filter(|r| !r.items.is_empty()).collect()
}

/// Movies and series matching `q` (like the desktop search).
pub async fn search(q: &str) -> Result<Vec<AnimeSearchResult>, String> {
    let c = cinemeta()?;
    let (movies, series) = futures::join!(c.search_movies(q), c.search_series(q));
    if let (Err(e), Err(_)) = (&movies, &series) {
        return Err(e.to_string());
    }
    let mut all = movies.unwrap_or_default();
    let extra: Vec<_> = series.unwrap_or_default().into_iter().filter(|s| !all.iter().any(|m| m.id == s.id)).collect();
    all.extend(extra);
    Ok(all)
}

/// Full details of a title (`kind` "movie" | "series").
pub async fn details(imdb: &str, kind: &str) -> Result<AnimeSearchResult, String> {
    // The desktop detail page caches (details, similar) under this key.
    if let Some(((d, _), age)) = DataCache::shared().get::<(AnimeSearchResult, Vec<AnimeSearchResult>)>(&format!("cinemeta:meta:{kind}:{imdb}")) {
        if age < DAY {
            return Ok(d);
        }
    }
    let (id, series) = (imdb.to_string(), kind == "series");
    cached(format!("phone:meta:{kind}:{imdb}"), DAY, async move {
        let c = cinemeta()?;
        let d = if series { c.series_details(&id).await } else { c.movie_details(&id).await };
        d.map_err(|e| e.to_string())?.ok_or_else(|| "Title not found.".to_string())
    })
    .await
}

/// A series' episodes (shared cache with the desktop detail page).
pub async fn episodes(imdb: &str) -> Result<Vec<AnimeEpisode>, String> {
    let id = imdb.to_string();
    cached(format!("cinemeta:episodes:{imdb}"), EPISODES_TTL, async move { cinemeta()?.series_episodes(&id).await.map_err(|e| e.to_string()) }).await
}

/// Season of a Cinemeta episode (`tt..:<season>:<episode>`).
pub fn season_of(ep: &AnimeEpisode) -> u32 {
    ep.id.split(':').nth(1).and_then(|s| s.parse().ok()).unwrap_or(1)
}

/// Finds the best source for a title like the desktop Play button (cached links first, then the
/// installed addons) and returns what the player should open. A series without a season and
/// episode resumes the one last started, else S1E1.
pub async fn find_best(title: String, imdb: String, movie: bool, season: Option<u32>, episode: Option<u32>) -> Result<PlayRequest, String> {
    let kind = if movie { "movie" } else { "series" };
    let (s, e) = match (season, episode) {
        (Some(s), Some(e)) => (s, e),
        (Some(s), None) => (s, 1),
        _ => streams::last_played(&imdb).unwrap_or((1, 1)),
    };
    let label = if movie { title } else { format!("{title} S{s:02}E{e:02}") };
    let base = PlayBase { title: label, imdb_id: Some(imdb.clone()), mal_id: None, season: (!movie).then_some(s), episode: (!movie).then_some(e) };
    let _ = common::mark_watching(common::media_id(kind, &imdb)).await;
    let key = streams::stream_key(&base, kind);
    if let Some(((cached_imdb, list), _)) = DataCache::shared().get::<(Option<String>, Vec<AnimeTorrentStream>)>(&key).filter(|((_, l), _)| !l.is_empty()) {
        let mut b = base;
        b.imdb_id = b.imdb_id.or(cached_imdb);
        return Ok(streams::request_for(&b, &list[0]));
    }
    let (b, list) = streams::lookup(base, movie, kind).await?;
    DataCache::shared().put(&key, &(&b.imdb_id, &list));
    list.first().map(|s| streams::request_for(&b, s)).ok_or_else(|| streams::NOT_FOUND.to_string())
}

/// Opens the desktop player on `req` (UI thread), like choosing a source on the Movies pages.
pub fn play_now(req: PlayRequest, window: &mut Window, cx: &mut App) {
    streams::play_now(req, window, cx);
}
