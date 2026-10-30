//! Movies and anime data and playback for the phone companion (ui-phone): the catalogs, caches and
//! stream lookup the Movies and Anime pages use, without a view. Everything async here runs on tokio.
use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::*;
use mv_core::cache::DataCache;
use mv_media::episode_art;
use mv_media::types::{AnimeEpisode, AnimeSearchResult, AnimeTorrentStream};
use mv_ui::nav::PlayRequest;

use crate::anime_home::{self, Feed};
use crate::common::{self, jikan, DAY, EPISODES_TTL};
use crate::movies_home::{self, catalog, catalog_key, cinemeta};
use crate::streams::{self, PlayBase};

pub use crate::movies_home::{background_of, kind_of, logo_of};
pub use crate::streams::NO_ADDONS;

/// Jikan pages of anime episodes hold 100.
pub const ANIME_EPISODE_PAGE: usize = 100;

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

/// One row of a Movies or Anime home tab.
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

/// The episode after S`season`E`episode` of a series: (season, episode, title).
pub async fn next_episode(imdb: &str, season: u32, episode: u32) -> Option<(u32, u32, Option<String>)> {
    let eps = episodes(imdb).await.ok()?;
    let find = |s: u32, e: u32| eps.iter().find(|x| season_of(x) == s && x.number == e).map(|x| (s, e, x.title.clone()));
    find(season, episode + 1).or_else(|| find(season + 1, 1))
}

// ---------------------------------------------------------------- anime

/// The rows of an Anime home tab ("home" | "airing" | "top" | "movies"), from the same Jikan
/// caches as the desktop Anime page. Rows that fail to load are left out.
pub async fn anime_home(tab: &str) -> Vec<Row> {
    let ids: &[&str] = match tab {
        "airing" => &["airing", "season", "upcoming"],
        "top" => &["top", "action", "isekai", "romance"],
        "movies" => &["movies"],
        _ => &["airing", "season", "top", "action", "movies", "isekai", "romance", "upcoming"],
    };
    let rows: Vec<(&'static str, &'static str, Feed)> = anime_home::ROWS.iter().copied().filter(|(id, ..)| ids.contains(id)).collect();
    let jobs = rows.into_iter().map(|(id, title, feed)| async move {
        let items = cached(feed.key(), DAY, feed.load(1)).await.ok()?;
        Some(Row { id, title: title.to_string(), top10: id == "top", items })
    });
    futures::future::join_all(jobs).await.into_iter().flatten().filter(|r| !r.items.is_empty()).collect()
}

/// Anime matching `q` (like the desktop Anime search).
pub async fn anime_search(q: &str) -> Result<Vec<AnimeSearchResult>, String> {
    jikan()?.search_anime(q, 1, None).await.map_err(|e| e.to_string())
}

/// An anime's details (shared cache with the desktop detail page).
pub async fn anime_details(mal: &str) -> Result<AnimeSearchResult, String> {
    let id = mal.to_string();
    cached(format!("jikan:anime:{mal}"), DAY, async move { jikan()?.anime_details(&id).await.map_err(|e| e.to_string()) }).await
}

/// One Jikan page of episodes (shared cache with the desktop detail page), with the stills the
/// desktop has found so far; missing stills are looked up in the background for next time.
pub async fn anime_episodes(mal: &str, page: u32) -> Result<Vec<AnimeEpisode>, String> {
    let id = mal.to_string();
    let mut eps = cached(format!("jikan:episodes:{mal}:{page}"), EPISODES_TTL, async move { jikan()?.anime_episodes(&id, page).await.map_err(|e| e.to_string()) }).await?;
    let art_key = format!("episode-art:{mal}");
    let known: HashMap<u32, String> = DataCache::shared().get(&art_key).map(|(m, _)| m).unwrap_or_default();
    for e in eps.iter_mut().filter(|e| e.thumbnail.is_none()) {
        e.thumbnail = known.get(&e.number).cloned();
    }
    let mut missing: Vec<AnimeEpisode> = eps.iter().filter(|e| e.thumbnail.is_none()).cloned().collect();
    if !missing.is_empty() {
        let id = mal.to_string();
        mv_ui::bg::runtime().spawn(async move {
            episode_art::fill_thumbnails(&id, &mut missing, None).await;
            let art: HashMap<u32, String> = missing.into_iter().filter_map(|e| Some((e.number, e.thumbnail?))).collect();
            if !art.is_empty() {
                let mut all: HashMap<u32, String> = DataCache::shared().get(&art_key).map(|(m, _)| m).unwrap_or_default();
                all.extend(art);
                DataCache::shared().put(&art_key, &all);
            }
        });
    }
    Ok(eps)
}

/// The episode after `episode` of an anime when it exists: (episode, title).
pub async fn anime_next(mal: &str, episode: u32) -> Option<(u32, Option<String>)> {
    let d = anime_details(mal).await.ok()?;
    if d.anime_type.as_deref() == Some("Movie") {
        return None;
    }
    let next = episode + 1;
    let page = (next - 1) / ANIME_EPISODE_PAGE as u32 + 1;
    let eps = anime_episodes(mal, page).await.unwrap_or_default();
    match eps.iter().find(|e| e.number == next) {
        Some(e) => Some((next, e.title.clone())),
        None => {
            let aired = d.episode_count.filter(|n| *n > 0).or_else(|| d.next_episode.map(|n| n.saturating_sub(1)));
            aired.is_some_and(|n| next <= n).then_some((next, None))
        }
    }
}

// ---------------------------------------------------------------- sources

/// Every link for `base` with the stream it came from, best first: cached links, else the
/// installed addons (cached after).
async fn links(base: PlayBase, movie: bool) -> Result<Vec<(PlayRequest, AnimeTorrentStream)>, String> {
    let kind = if movie { "movie" } else { "series" };
    let key = streams::stream_key(&base, kind);
    if let Some(((cached_imdb, list), _)) = DataCache::shared().get::<(Option<String>, Vec<AnimeTorrentStream>)>(&key).filter(|((_, l), _)| !l.is_empty()) {
        let mut b = base;
        b.imdb_id = b.imdb_id.or(cached_imdb);
        return Ok(list.into_iter().map(|s| (streams::request_for(&b, &s), s)).collect());
    }
    let (b, list) = streams::lookup(base, movie, kind).await?;
    DataCache::shared().put(&key, &(&b.imdb_id, &list));
    if list.is_empty() {
        return Err(streams::NOT_FOUND.to_string());
    }
    Ok(list.into_iter().map(|s| (streams::request_for(&b, &s), s)).collect())
}

/// Every link for a movie or series episode, best first (like the desktop Play button: cached
/// links first, then the installed addons). A series without a season and episode resumes the one
/// last started, else S1E1.
pub async fn sources(title: String, imdb: String, movie: bool, season: Option<u32>, episode: Option<u32>) -> Result<Vec<(PlayRequest, AnimeTorrentStream)>, String> {
    let kind = if movie { "movie" } else { "series" };
    let (s, e) = match (season, episode) {
        (Some(s), Some(e)) => (s, e),
        (Some(s), None) => (s, 1),
        _ => streams::last_played(&imdb).unwrap_or((1, 1)),
    };
    let label = if movie { title } else { format!("{title} S{s:02}E{e:02}") };
    let base = PlayBase { title: label, imdb_id: Some(imdb.clone()), mal_id: None, season: (!movie).then_some(s), episode: (!movie).then_some(e) };
    let _ = common::mark_watching(common::media_id(kind, &imdb)).await;
    links(base, movie).await
}

/// Every link for an anime episode (episode 1 when not given), best first, like the desktop
/// Anime pages' Play button.
pub async fn anime_sources(title: String, mal: String, episode: Option<u32>) -> Result<Vec<(PlayRequest, AnimeTorrentStream)>, String> {
    let d = anime_details(&mal).await.ok();
    let movie = d.as_ref().is_some_and(|d| d.anime_type.as_deref() == Some("Movie"));
    let name = d.map(|d| d.title).filter(|t| !t.is_empty()).unwrap_or(title);
    let episode = episode.unwrap_or(1).max(1);
    let base = PlayBase {
        title: if movie { name } else { format!("{name} - Episode {episode}") },
        imdb_id: None,
        mal_id: Some(mal.clone()),
        season: Some(1),
        episode: Some(episode),
    };
    let _ = common::mark_watching(common::media_id("anime", &mal)).await;
    links(base, movie).await
}

/// The best source for a title, for playing on the PC (see [`sources`]).
pub async fn find_best(title: String, imdb: String, movie: bool, season: Option<u32>, episode: Option<u32>) -> Result<PlayRequest, String> {
    sources(title, imdb, movie, season, episode).await?.into_iter().next().map(|(r, _)| r).ok_or_else(|| streams::NOT_FOUND.to_string())
}

/// The best source for an anime episode, for playing on the PC (see [`anime_sources`]).
pub async fn find_best_anime(title: String, mal: String, episode: Option<u32>) -> Result<PlayRequest, String> {
    anime_sources(title, mal, episode).await?.into_iter().next().map(|(r, _)| r).ok_or_else(|| streams::NOT_FOUND.to_string())
}

/// Remembers a series' season and episode for Continue Watching, as playing on the PC does.
pub fn remember(req: &PlayRequest) {
    streams::remember(req);
}

/// Opens the desktop player on `req` (UI thread), like choosing a source on the Movies pages.
pub fn play_now(req: PlayRequest, window: &mut Window, cx: &mut App) {
    streams::play_now(req, window, cx);
}
