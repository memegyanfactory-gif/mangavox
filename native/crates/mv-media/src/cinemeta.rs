//! Cinemeta (Stremio's movie and series catalog) and IMDB suggestion search.
//! Mirrors `src/main/services/cinemeta-api.ts`.
//!
//! Unlike the TypeScript functions, which log and return empty results on failure, every method here
//! returns `Result` so callers can tell "no results" from "request failed". `unwrap_or_default()` gives
//! the TypeScript behaviour.

use std::time::Duration;

use serde_json::Value;

use crate::error::{MediaError, Result};
use crate::types::{AnimeEpisode, AnimeSearchResult};
use crate::util::{
    encode_uri_component, first_four_digits, str_field, value_to_f64, value_to_string,
};

pub const CINEMETA_BASE: &str = "https://v3-cinemeta.strem.io";
/// IMDB suggestion API. Fast and fuzzy, used before falling back to Cinemeta search.
pub const IMDB_SEARCH: &str = "https://v3.sg.media-imdb.com/suggestion";

const CATALOG_TIMEOUT: Duration = Duration::from_secs(15);
const IMDB_TIMEOUT: Duration = Duration::from_secs(10);
/// Cinemeta returns 100 items per page.
const PAGE_SIZE: u32 = 100;
const IMDB_MAX_RESULTS: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CinemetaCatalog {
    #[default]
    Top,
    Year,
    Popular,
    ImdbRating,
}

impl CinemetaCatalog {
    pub fn as_str(self) -> &'static str {
        match self {
            CinemetaCatalog::Top => "top",
            CinemetaCatalog::Year => "year",
            CinemetaCatalog::Popular => "popular",
            CinemetaCatalog::ImdbRating => "imdbRating",
        }
    }
}

/// Map a Cinemeta meta object (`mapMeta`).
fn map_meta(m: &Value) -> AnimeSearchResult {
    let id = str_field(m, "id").unwrap_or_default();
    // `m.year || m.releaseInfo`: a truthy year wins even if it has no 4-digit run.
    let year_src = value_to_string(m.get("year"))
        .filter(|s| !s.is_empty() && s != "0")
        .or_else(|| str_field(m, "releaseInfo"));
    let year = year_src
        .as_deref()
        .and_then(first_four_digits)
        .and_then(|s| s.parse::<i32>().ok());
    let is_movie = m.get("type").and_then(Value::as_str) == Some("movie");
    AnimeSearchResult {
        imdb_id: Some(str_field(m, "imdb_id").unwrap_or_else(|| id.clone())),
        id,
        title: str_field(m, "name").unwrap_or_else(|| "Unknown".to_string()),
        cover_url: str_field(m, "poster"),
        banner_url: str_field(m, "background"),
        year,
        genres: Some(string_array(m.get("genres"))),
        description: str_field(m, "description"),
        anime_type: Some(if is_movie { "Movie" } else { "TV" }.to_string()),
        score: value_to_f64(m.get("imdbRating")).filter(|&s| s != 0.0),
        status: str_field(m, "releaseInfo"),
        ..Default::default()
    }
}

/// Map an IMDB suggestion entry (`searchMovies`, first source).
fn map_suggestion(r: &Value) -> AnimeSearchResult {
    let id = str_field(r, "id").unwrap_or_default();
    AnimeSearchResult {
        imdb_id: Some(id.clone()),
        id,
        title: str_field(r, "l").unwrap_or_else(|| "Unknown".to_string()),
        cover_url: str_field(r.get("i").unwrap_or(&Value::Null), "imageUrl"),
        year: r.get("y").and_then(Value::as_i64).map(|y| y as i32),
        description: str_field(r, "s"),
        anime_type: Some("Movie".to_string()),
        genres: Some(Vec::new()),
        ..Default::default()
    }
}

/// Episodes from the `videos` array of a series meta object.
fn episodes_from_meta(meta: &Value, imdb_id: &str) -> Vec<AnimeEpisode> {
    meta.get("videos")
        .and_then(Value::as_array)
        .map(|videos| {
            videos
                .iter()
                .enumerate()
                .map(|(index, video)| {
                    let fallback_number = (index + 1) as u32;
                    let number = value_to_f64(video.get("episode"))
                        .filter(|&n| n != 0.0)
                        .map(|n| n as u32)
                        .unwrap_or(fallback_number);
                    AnimeEpisode {
                        id: str_field(video, "id")
                            .unwrap_or_else(|| format!("{imdb_id}:1:{}", index + 1)),
                        number,
                        title: str_field(video, "name").or_else(|| str_field(video, "title")),
                        air_date: str_field(video, "released"),
                        thumbnail: str_field(video, "thumbnail"),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Entries from an IMDB suggestion response: `tt` ids with movie-like `qid`, capped at 30.
fn map_suggestions(data: &Value) -> Vec<AnimeSearchResult> {
    data.get("d")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|r| {
                    let id_ok = str_field(r, "id").is_some_and(|id| id.starts_with("tt"));
                    let qid = r.get("qid").and_then(Value::as_str);
                    id_ok && matches!(qid, Some("movie" | "tvMovie" | "short"))
                })
                .take(IMDB_MAX_RESULTS)
                .map(map_suggestion)
                .collect()
        })
        .unwrap_or_default()
}

fn string_array(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Build the `/catalog/{type}/{id}` path with optional `genre` and `skip` extras.
fn catalog_path(kind: &str, catalog: CinemetaCatalog, page: u32, genre: Option<&str>) -> String {
    let skip = page.saturating_sub(1) * PAGE_SIZE;
    let mut parts = Vec::new();
    if let Some(g) = genre {
        parts.push(format!("genre={}", encode_uri_component(g)));
    }
    if skip > 0 {
        parts.push(format!("skip={skip}"));
    }
    let extras = if parts.is_empty() {
        String::new()
    } else {
        format!("/{}", parts.join("&"))
    };
    format!("/catalog/{kind}/{}{extras}.json", catalog.as_str())
}

/// Search-by-title path under a Cinemeta catalog.
fn search_path(kind: &str, query: &str) -> String {
    format!(
        "/catalog/{kind}/top/search={}.json",
        encode_uri_component(query.trim())
    )
}

/// Suggestion URL for the IMDB search (`{IMDB_SEARCH}/{first}/{q_with_underscores}.json`).
pub fn imdb_suggestion_url(base: &str, query: &str) -> String {
    let q = query.trim().to_lowercase();
    let first = q.chars().next().map(|c| c.to_string()).unwrap_or_default();
    let joined = q.split_whitespace().collect::<Vec<_>>().join("_");
    format!(
        "{base}/{}/{}.json",
        encode_uri_component(&first),
        encode_uri_component(&joined)
    )
}

pub struct CinemetaClient {
    http: reqwest::Client,
    base: String,
    imdb_base: String,
}

impl CinemetaClient {
    pub fn new() -> Result<Self> {
        Self::with_base_urls(CINEMETA_BASE, IMDB_SEARCH)
    }

    pub fn with_base_urls(base: impl Into<String>, imdb_base: impl Into<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!(
                "MangaVox/",
                env!("CARGO_PKG_VERSION"),
                " (mv-media)"
            ))
            .build()?;
        Ok(Self {
            http,
            base: base.into().trim_end_matches('/').to_string(),
            imdb_base: imdb_base.into().trim_end_matches('/').to_string(),
        })
    }

    async fn get_json(&self, url: &str, timeout: Duration) -> Result<Value> {
        let resp = self.http.get(url).timeout(timeout).send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(MediaError::Status {
                status: status.as_u16(),
                url: url.to_string(),
            });
        }
        let text = resp.text().await?;
        serde_json::from_str(&text).map_err(|source| MediaError::Json {
            url: url.to_string(),
            source,
        })
    }

    async fn metas(&self, url: &str) -> Result<Vec<AnimeSearchResult>> {
        let data = self.get_json(url, CATALOG_TIMEOUT).await?;
        Ok(data
            .get("metas")
            .and_then(Value::as_array)
            .map(|metas| metas.iter().map(map_meta).collect())
            .unwrap_or_default())
    }

    /// Movie catalog page (`/catalog/movie/{catalog}`), 100 items per page.
    pub async fn movies_catalog(
        &self,
        catalog: CinemetaCatalog,
        page: u32,
        genre: Option<&str>,
    ) -> Result<Vec<AnimeSearchResult>> {
        self.metas(&format!(
            "{}{}",
            self.base,
            catalog_path("movie", catalog, page, genre)
        ))
        .await
    }

    /// Series catalog page (`/catalog/series/{catalog}`), 100 items per page.
    pub async fn series_catalog(
        &self,
        catalog: CinemetaCatalog,
        page: u32,
        genre: Option<&str>,
    ) -> Result<Vec<AnimeSearchResult>> {
        self.metas(&format!(
            "{}{}",
            self.base,
            catalog_path("series", catalog, page, genre)
        ))
        .await
    }

    pub async fn search_series(&self, query: &str) -> Result<Vec<AnimeSearchResult>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        self.metas(&format!("{}{}", self.base, search_path("series", query)))
            .await
    }

    pub async fn series_details(&self, imdb_id: &str) -> Result<Option<AnimeSearchResult>> {
        self.meta_object("series", imdb_id)
            .await
            .map(|m| m.map(|m| map_meta(&m)))
    }

    /// Episodes from the `videos` array of a series meta object.
    pub async fn series_episodes(&self, imdb_id: &str) -> Result<Vec<AnimeEpisode>> {
        Ok(match self.meta_object("series", imdb_id).await? {
            Some(meta) => episodes_from_meta(&meta, imdb_id),
            None => Vec::new(),
        })
    }

    /// Movie search. Tries the IMDB suggestion API first and falls back to Cinemeta search if that request fails.
    pub async fn search_movies(&self, query: &str) -> Result<Vec<AnimeSearchResult>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let suggestion = self
            .get_json(&imdb_suggestion_url(&self.imdb_base, query), IMDB_TIMEOUT)
            .await;
        match suggestion {
            Ok(data) => Ok(map_suggestions(&data)),
            Err(_) => {
                self.metas(&format!("{}{}", self.base, search_path("movie", query)))
                    .await
            }
        }
    }

    pub async fn movie_details(&self, imdb_id: &str) -> Result<Option<AnimeSearchResult>> {
        self.meta_object("movie", imdb_id)
            .await
            .map(|m| m.map(|m| map_meta(&m)))
    }

    /// The `meta` object from `/meta/{kind}/{id}.json`, or `None` if the addon returned no meta.
    async fn meta_object(&self, kind: &str, imdb_id: &str) -> Result<Option<Value>> {
        let data = self
            .get_json(
                &format!("{}/meta/{kind}/{imdb_id}.json", self.base),
                CATALOG_TIMEOUT,
            )
            .await?;
        Ok(data.get("meta").filter(|m| m.is_object()).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let text = match name {
            "movie_top" => include_str!("../tests/fixtures/cinemeta_catalog_movie_top.json"),
            "movie_top_skip100" => {
                include_str!("../tests/fixtures/cinemeta_catalog_movie_top_skip100.json")
            }
            "series_top" => include_str!("../tests/fixtures/cinemeta_catalog_series_top.json"),
            "movie_search" => include_str!("../tests/fixtures/cinemeta_catalog_movie_search.json"),
            "series_search" => {
                include_str!("../tests/fixtures/cinemeta_catalog_series_search.json")
            }
            "meta_movie" => include_str!("../tests/fixtures/cinemeta_meta_movie.json"),
            "meta_series" => include_str!("../tests/fixtures/cinemeta_meta_series.json"),
            "imdb_suggestion" => include_str!("../tests/fixtures/imdb_suggestion_inception.json"),
            other => panic!("unknown fixture {other}"),
        };
        serde_json::from_str(text).unwrap()
    }

    fn metas(v: &Value) -> Vec<AnimeSearchResult> {
        v.get("metas")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .map(map_meta)
            .collect()
    }

    #[test]
    fn catalog_pages_map_to_search_results() {
        let top = metas(&fixture("movie_top"));
        assert!(top.len() >= 20);
        assert!(top
            .iter()
            .all(|m| m.imdb_id.as_deref().is_some_and(|id| id.starts_with("tt"))));
        assert!(top.iter().all(|m| m.anime_type.as_deref() == Some("Movie")));
        assert!(top
            .iter()
            .all(|m| m.year.is_some_and(|y| (1800..=2100).contains(&y))));

        let series = metas(&fixture("series_top"));
        assert!(series.iter().all(|m| m.anime_type.as_deref() == Some("TV")));

        let page2 = metas(&fixture("movie_top_skip100"));
        assert!(!page2.is_empty());
        assert_ne!(page2[0].id, top[0].id);
    }

    #[test]
    fn catalog_path_matches_typescript_urls() {
        assert_eq!(
            catalog_path("movie", CinemetaCatalog::Top, 1, None),
            "/catalog/movie/top.json"
        );
        assert_eq!(
            catalog_path("movie", CinemetaCatalog::Top, 2, None),
            "/catalog/movie/top/skip=100.json"
        );
        assert_eq!(
            catalog_path(
                "series",
                CinemetaCatalog::ImdbRating,
                1,
                Some("Sci-Fi & Fantasy")
            ),
            "/catalog/series/imdbRating/genre=Sci-Fi%20%26%20Fantasy.json"
        );
        assert_eq!(
            search_path("series", "  breaking bad "),
            "/catalog/series/top/search=breaking%20bad.json"
        );
    }

    #[test]
    fn movie_search_fixture_maps() {
        let results = metas(&fixture("movie_search"));
        assert!(results.iter().any(|m| m.title == "Inception"));
    }

    #[test]
    fn meta_movie_maps_details() {
        let meta = fixture("meta_movie");
        let m = map_meta(meta.get("meta").unwrap());
        assert_eq!(m.id, "tt1375666");
        assert_eq!(m.title, "Inception");
        assert_eq!(m.year, Some(2010));
        assert_eq!(m.score, Some(8.8));
        assert_eq!(m.anime_type.as_deref(), Some("Movie"));
        assert!(m
            .genres
            .as_ref()
            .is_some_and(|g| g.contains(&"Sci-Fi".to_string())));
    }

    #[test]
    fn meta_series_builds_episodes() {
        let meta = fixture("meta_series");
        let episodes = episodes_from_meta(meta.get("meta").unwrap(), "tt0903747");
        assert!(!episodes.is_empty());
        assert_eq!(episodes[0].id, "tt0903747:1:1");
        assert_eq!(episodes[0].number, 1);
        assert_eq!(episodes[0].title.as_deref(), Some("Pilot"));
        assert!(episodes[0].thumbnail.is_some());
    }

    #[test]
    fn imdb_suggestions_keep_movie_like_tt_ids() {
        let data = fixture("imdb_suggestion");
        let results = map_suggestions(&data);
        assert!(!results.is_empty());
        assert!(results.len() <= IMDB_MAX_RESULTS);
        assert!(results
            .iter()
            .all(|r| r.id.starts_with("tt") && r.anime_type.as_deref() == Some("Movie")));
        assert!(results
            .iter()
            .any(|r| r.title == "Inception" && r.year == Some(2010)));
    }

    #[test]
    fn imdb_suggestion_url_matches_typescript() {
        assert_eq!(
            imdb_suggestion_url(IMDB_SEARCH, "  The  Dark Knight "),
            "https://v3.sg.media-imdb.com/suggestion/t/the_dark_knight.json"
        );
    }
}
