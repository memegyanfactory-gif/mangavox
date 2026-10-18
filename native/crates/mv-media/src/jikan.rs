//! Jikan (MyAnimeList) v4 client. Mirrors `src/main/services/jikan-api.ts`.
//!
//! Behaviour kept from the TypeScript service:
//! - All Jikan requests go through one limiter (500 ms between request starts, about 2 req/s,
//!   under Jikan's 3 req/s cap). Use [`JikanClient::shared`] so every caller shares that limiter.
//! - A circuit breaker opens for 10 minutes after a network error, timeout or 5xx from Jikan.
//!   While open, calls fail fast with [`MediaError::CircuitOpen`].
//! - 429 responses are retried after 2 s. Invalid JSON is retried up to 3 times.
//! - The arm.haglund MAL to IMDB lookup is not Jikan, so it has no breaker and retries network errors.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{MediaError, Result};
use crate::types::{AnimeEpisode, AnimeSearchResult};
use crate::util::non_empty;

pub const BASE_URL: &str = "https://api.jikan.moe/v4";
const ARM_URL: &str = "https://arm.haglund.dev/api/v2/ids";

/// Minimum gap between Jikan request starts.
pub const MIN_INTERVAL: Duration = Duration::from_millis(500);
const JIKAN_TIMEOUT: Duration = Duration::from_secs(6);
const GENERIC_TIMEOUT: Duration = Duration::from_secs(15);
const BREAKER_OPEN_FOR: Duration = Duration::from_secs(10 * 60);
const RATE_LIMIT_RETRY: Duration = Duration::from_secs(2);
const ERROR_RETRY: Duration = Duration::from_millis(1500);
const MAX_RATE_LIMIT_RETRIES: u32 = 3;
const MAX_TRANSIENT_RETRIES: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScheduleDay {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl ScheduleDay {
    pub fn as_str(self) -> &'static str {
        match self {
            ScheduleDay::Monday => "monday",
            ScheduleDay::Tuesday => "tuesday",
            ScheduleDay::Wednesday => "wednesday",
            ScheduleDay::Thursday => "thursday",
            ScheduleDay::Friday => "friday",
            ScheduleDay::Saturday => "saturday",
            ScheduleDay::Sunday => "sunday",
        }
    }
}

/// A search result plus the broadcast string from the schedule endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduleEntry {
    #[serde(flatten)]
    anime: AnimeSearchResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    broadcast: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Genre {
    pub id: u32,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GenreShortcut {
    pub id: u32,
    pub name: &'static str,
}

pub const GENRE_SHORTCUTS: &[GenreShortcut] = &[
    GenreShortcut {
        id: 1,
        name: "Action",
    },
    GenreShortcut {
        id: 2,
        name: "Adventure",
    },
    GenreShortcut {
        id: 4,
        name: "Comedy",
    },
    GenreShortcut {
        id: 8,
        name: "Drama",
    },
    GenreShortcut {
        id: 10,
        name: "Fantasy",
    },
    GenreShortcut {
        id: 14,
        name: "Horror",
    },
    GenreShortcut {
        id: 22,
        name: "Romance",
    },
    GenreShortcut {
        id: 24,
        name: "Sci-Fi",
    },
    GenreShortcut {
        id: 36,
        name: "Slice of Life",
    },
    GenreShortcut {
        id: 30,
        name: "Sports",
    },
    GenreShortcut {
        id: 37,
        name: "Supernatural",
    },
    GenreShortcut {
        id: 7,
        name: "Mystery",
    },
    GenreShortcut {
        id: 41,
        name: "Suspense",
    },
    GenreShortcut {
        id: 62,
        name: "Isekai",
    },
    GenreShortcut {
        id: 25,
        name: "Shoujo",
    },
    GenreShortcut {
        id: 27,
        name: "Shounen",
    },
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnimeRelation {
    relation: String,
    entries: Vec<RelationEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelationEntry {
    mal_id: u64,
    #[serde(rename = "type")]
    kind: String,
    name: String,
}

// ── Raw Jikan payloads (only the fields the TypeScript mapper reads) ──

#[derive(Deserialize)]
struct Envelope<T> {
    data: Option<T>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct JikanAnime {
    mal_id: u64,
    title: Option<String>,
    title_english: Option<String>,
    images: Option<JikanImages>,
    year: Option<i32>,
    aired: Option<JikanAired>,
    status: Option<String>,
    genres: Vec<JikanNamed>,
    themes: Vec<JikanNamed>,
    synopsis: Option<String>,
    episodes: Option<u32>,
    #[serde(rename = "type")]
    anime_type: Option<String>,
    season: Option<String>,
    score: Option<f64>,
    broadcast: Option<JikanBroadcast>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct JikanImages {
    jpg: Option<JikanJpg>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct JikanJpg {
    image_url: Option<String>,
    large_image_url: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct JikanAired {
    prop: Option<JikanAiredProp>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct JikanAiredProp {
    from: Option<JikanDate>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct JikanDate {
    year: Option<i32>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct JikanNamed {
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct JikanBroadcast {
    string: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct JikanEpisode {
    mal_id: u32,
    title: Option<String>,
    title_japanese: Option<String>,
    aired: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct JikanRelation {
    relation: String,
    entry: Vec<JikanRelationEntry>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct JikanRelationEntry {
    mal_id: u64,
    #[serde(rename = "type")]
    kind: String,
    name: String,
}

#[derive(Deserialize)]
struct ArmIds {
    imdb: Option<String>,
}

/// Map a Jikan anime object to the shared search result (`mapAnime` in the TypeScript).
fn map_anime(item: &JikanAnime) -> AnimeSearchResult {
    let jpg = item.images.as_ref().and_then(|i| i.jpg.as_ref());
    let year_from_aired = item
        .aired
        .as_ref()
        .and_then(|a| a.prop.as_ref())
        .and_then(|p| p.from.as_ref())
        .and_then(|d| d.year);
    AnimeSearchResult {
        id: item.mal_id.to_string(),
        title: non_empty(item.title_english.clone())
            .or_else(|| non_empty(item.title.clone()))
            .unwrap_or_else(|| "Unknown".to_string()),
        cover_url: jpg.and_then(|j| {
            non_empty(j.large_image_url.clone()).or_else(|| non_empty(j.image_url.clone()))
        }),
        year: item
            .year
            .filter(|&y| y != 0)
            .or(year_from_aired.filter(|&y| y != 0)),
        status: item.status.clone(),
        genres: Some(
            item.genres
                .iter()
                .chain(item.themes.iter())
                .map(|g| g.name.clone())
                .collect(),
        ),
        description: item.synopsis.clone(),
        episode_count: item.episodes,
        anime_type: non_empty(item.anime_type.clone()),
        season: non_empty(item.season.clone()),
        score: item.score.filter(|&s| s != 0.0),
        ..Default::default()
    }
}

fn map_schedule_entry(item: &JikanAnime) -> ScheduleEntry {
    ScheduleEntry {
        anime: map_anime(item),
        broadcast: item
            .broadcast
            .as_ref()
            .and_then(|b| non_empty(b.string.clone())),
    }
}

fn dedupe_by_id<T>(items: Vec<T>, id: impl Fn(&T) -> &str) -> Vec<T> {
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(id(item).to_string()))
        .collect()
}

fn map_episode(mal_id: &str, ep: &JikanEpisode) -> AnimeEpisode {
    AnimeEpisode {
        id: format!("{}:{}", mal_id, ep.mal_id),
        number: ep.mal_id,
        title: non_empty(ep.title.clone()).or_else(|| non_empty(ep.title_japanese.clone())),
        air_date: ep.aired.clone(),
        thumbnail: None,
    }
}

/// Why a single HTTP attempt failed; decides whether and how it is retried.
enum Failure {
    RateLimited,
    /// Connect error, timeout, or body read error.
    Network,
    /// 5xx.
    Server,
    /// Body was not valid JSON for the requested type.
    BadBody,
    /// Any other non-2xx status.
    Fatal,
}

/// Opens after a Jikan failure and refuses calls until it expires.
#[derive(Default)]
struct Breaker {
    open_until: Mutex<Option<Instant>>,
}

impl Breaker {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Instant>> {
        self.open_until.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn is_available(&self) -> bool {
        match *self.lock() {
            Some(until) => Instant::now() >= until,
            None => true,
        }
    }

    fn trip(&self) {
        *self.lock() = Some(Instant::now() + BREAKER_OPEN_FOR);
    }
}

/// Reserves start times so consecutive requests are at least `min_interval` apart.
struct Limiter {
    min_interval: Duration,
    next_start: tokio::sync::Mutex<Instant>,
}

impl Limiter {
    fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            next_start: tokio::sync::Mutex::new(Instant::now()),
        }
    }

    async fn acquire(&self) {
        let start = {
            let mut next = self.next_start.lock().await;
            let now = Instant::now();
            let start = (*next).max(now);
            *next = start + self.min_interval;
            start
        };
        let delay = start.saturating_duration_since(Instant::now());
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
}

pub struct JikanClient {
    http: reqwest::Client,
    base: String,
    limiter: Limiter,
    breaker: Breaker,
    imdb_cache: Mutex<HashMap<String, Option<String>>>,
}

static SHARED: OnceLock<JikanClient> = OnceLock::new();

impl JikanClient {
    /// Client for the public Jikan API.
    pub fn new() -> Result<Self> {
        Self::with_base_url(BASE_URL)
    }

    /// Client for a custom base URL (for example a local mirror).
    pub fn with_base_url(base: impl Into<String>) -> Result<Self> {
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
            limiter: Limiter::new(MIN_INTERVAL),
            breaker: Breaker::default(),
            imdb_cache: Mutex::new(HashMap::new()),
        })
    }

    /// Process-wide client, so the rate limiter and breaker are shared like the TypeScript module state.
    pub fn shared() -> Result<&'static JikanClient> {
        if let Some(client) = SHARED.get() {
            return Ok(client);
        }
        let client = Self::new()?;
        Ok(SHARED.get_or_init(|| client))
    }

    /// False while the circuit breaker is open (Jikan failed in the last 10 minutes).
    pub fn is_available(&self) -> bool {
        self.breaker.is_available()
    }

    /// GET `url` and decode it. Jikan URLs (those under the base) are rate limited and use the breaker.
    async fn fetch_json<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        let jikan = url.starts_with(&self.base);
        if jikan {
            if !self.breaker.is_available() {
                return Err(MediaError::CircuitOpen);
            }
            self.limiter.acquire().await;
        }
        let timeout = if jikan {
            JIKAN_TIMEOUT
        } else {
            GENERIC_TIMEOUT
        };
        let mut rate_limit_retries = MAX_RATE_LIMIT_RETRIES;
        let mut transient_retries = MAX_TRANSIENT_RETRIES;
        loop {
            let (kind, err) = match self.attempt::<T>(url, timeout).await {
                Ok(value) => return Ok(value),
                Err(failure) => failure,
            };
            match kind {
                Failure::RateLimited if rate_limit_retries > 0 => {
                    rate_limit_retries -= 1;
                    tokio::time::sleep(RATE_LIMIT_RETRY).await;
                }
                Failure::Network | Failure::Server if jikan => {
                    self.breaker.trip();
                    return Err(err);
                }
                Failure::Network | Failure::Server | Failure::BadBody if transient_retries > 0 => {
                    transient_retries -= 1;
                    tokio::time::sleep(ERROR_RETRY).await;
                }
                _ => return Err(err),
            }
        }
    }

    async fn attempt<T: DeserializeOwned>(
        &self,
        url: &str,
        timeout: Duration,
    ) -> std::result::Result<T, (Failure, MediaError)> {
        let resp = self
            .http
            .get(url)
            .timeout(timeout)
            .send()
            .await
            .map_err(|e| (Failure::Network, MediaError::from(e)))?;
        let status = resp.status();
        let status_err = || MediaError::Status {
            status: status.as_u16(),
            url: url.to_string(),
        };
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err((Failure::RateLimited, status_err()));
        }
        if status.is_server_error() {
            return Err((Failure::Server, status_err()));
        }
        if !status.is_success() {
            return Err((Failure::Fatal, status_err()));
        }
        let text = resp
            .text()
            .await
            .map_err(|e| (Failure::Network, MediaError::from(e)))?;
        serde_json::from_str(&text).map_err(|source| {
            (
                Failure::BadBody,
                MediaError::Json {
                    url: url.to_string(),
                    source,
                },
            )
        })
    }

    async fn anime_list(&self, url: &str) -> Result<Vec<AnimeSearchResult>> {
        let env: Envelope<Vec<JikanAnime>> = self.fetch_json(url).await?;
        Ok(env.data.unwrap_or_default().iter().map(map_anime).collect())
    }

    /// Top anime by popularity. `kind`: "tv" | "movie" | None for all.
    pub async fn top_anime(&self, page: u32, kind: Option<&str>) -> Result<Vec<AnimeSearchResult>> {
        let type_param = kind.map(|t| format!("&type={t}")).unwrap_or_default();
        self.anime_list(&format!(
            "{}/top/anime?page={page}&limit=25{type_param}",
            self.base
        ))
        .await
    }

    /// Currently airing anime (`/seasons/now`).
    pub async fn airing_anime(&self, page: u32) -> Result<Vec<AnimeSearchResult>> {
        self.anime_list(&format!("{}/seasons/now?page={page}&limit=25", self.base))
            .await
    }

    /// Anime for a year and season ("winter" | "spring" | "summer" | "fall").
    /// `None` for the year, or for the season, means the current season.
    pub async fn season_anime(
        &self,
        year: Option<i32>,
        season: Option<&str>,
        page: u32,
    ) -> Result<Vec<AnimeSearchResult>> {
        let path = match (year, season) {
            (Some(y), Some(s)) => format!("{y}/{s}"),
            _ => "now".to_string(),
        };
        let items = self
            .anime_list(&format!(
                "{}/seasons/{path}?page={page}&limit=25",
                self.base
            ))
            .await?;
        Ok(dedupe_by_id(items, |a| a.id.as_str()))
    }

    /// Airing schedule for one weekday.
    pub async fn schedule_anime(&self, day: ScheduleDay, page: u32) -> Result<Vec<ScheduleEntry>> {
        let url = format!(
            "{}/schedules?filter={}&page={page}&limit=25",
            self.base,
            day.as_str()
        );
        let env: Envelope<Vec<JikanAnime>> = self.fetch_json(&url).await?;
        let items: Vec<ScheduleEntry> = env
            .data
            .unwrap_or_default()
            .iter()
            .map(map_schedule_entry)
            .collect();
        Ok(dedupe_by_id(items, |e| e.anime.id.as_str()))
    }

    /// Upcoming anime (`/seasons/upcoming`).
    pub async fn upcoming_anime(&self, page: u32) -> Result<Vec<AnimeSearchResult>> {
        self.anime_list(&format!(
            "{}/seasons/upcoming?page={page}&limit=25",
            self.base
        ))
        .await
    }

    /// Search by title, sorted by score. `kind`: "tv" | "movie" | None.
    pub async fn search_anime(
        &self,
        query: &str,
        page: u32,
        kind: Option<&str>,
    ) -> Result<Vec<AnimeSearchResult>> {
        let type_param = kind.map(|t| format!("&type={t}")).unwrap_or_default();
        let q = crate::util::encode_uri_component(query);
        self.anime_list(&format!(
            "{}/anime?q={q}&page={page}&limit=25&order_by=score&sort=desc{type_param}",
            self.base
        ))
        .await
    }

    /// Anime for one genre id, sorted by score.
    pub async fn anime_by_genre(
        &self,
        genre_id: u32,
        page: u32,
        kind: Option<&str>,
    ) -> Result<Vec<AnimeSearchResult>> {
        let type_param = kind.map(|t| format!("&type={t}")).unwrap_or_default();
        self.anime_list(&format!(
            "{}/anime?genres={genre_id}&page={page}&limit=25&order_by=score&sort=desc{type_param}",
            self.base
        ))
        .await
    }

    /// Full anime genre list.
    pub async fn genres(&self) -> Result<Vec<Genre>> {
        #[derive(Deserialize)]
        struct RawGenre {
            mal_id: u32,
            name: String,
        }
        let env: Envelope<Vec<RawGenre>> = self
            .fetch_json(&format!("{}/genres/anime", self.base))
            .await?;
        Ok(env
            .data
            .unwrap_or_default()
            .into_iter()
            .map(|g| Genre {
                id: g.mal_id,
                name: g.name,
            })
            .collect())
    }

    /// Full details for one MAL id (`/anime/{id}/full`).
    pub async fn anime_details(&self, mal_id: &str) -> Result<AnimeSearchResult> {
        let env: Envelope<JikanAnime> = self
            .fetch_json(&format!("{}/anime/{mal_id}/full", self.base))
            .await?;
        env.data
            .map(|item| map_anime(&item))
            .ok_or_else(|| MediaError::NotFound(format!("anime {mal_id}")))
    }

    /// Episode list for one MAL id, one page at a time.
    pub async fn anime_episodes(&self, mal_id: &str, page: u32) -> Result<Vec<AnimeEpisode>> {
        let env: Envelope<Vec<JikanEpisode>> = self
            .fetch_json(&format!(
                "{}/anime/{mal_id}/episodes?page={page}",
                self.base
            ))
            .await?;
        Ok(env
            .data
            .unwrap_or_default()
            .iter()
            .map(|ep| map_episode(mal_id, ep))
            .collect())
    }

    /// Related entries (sequels, prequels, side stories) for one MAL id.
    pub async fn anime_relations(&self, mal_id: &str) -> Result<Vec<AnimeRelation>> {
        let env: Envelope<Vec<JikanRelation>> = self
            .fetch_json(&format!("{}/anime/{mal_id}/relations", self.base))
            .await?;
        Ok(env
            .data
            .unwrap_or_default()
            .into_iter()
            .map(|r| AnimeRelation {
                relation: r.relation,
                entries: r
                    .entry
                    .into_iter()
                    .map(|e| RelationEntry {
                        mal_id: e.mal_id,
                        kind: e.kind,
                        name: e.name,
                    })
                    .collect(),
            })
            .collect())
    }

    /// MAL id to IMDB id via the arm.haglund mapping. Never fails: `None` on any error. Results (including misses) are cached.
    pub async fn imdb_id(&self, mal_id: &str) -> Option<String> {
        if let Some(hit) = self
            .imdb_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(mal_id)
        {
            return hit.clone();
        }
        let url = format!("{ARM_URL}?source=myanimelist&id={mal_id}");
        let imdb = self
            .fetch_json::<ArmIds>(&url)
            .await
            .ok()
            .and_then(|ids| non_empty(ids.imdb));
        self.imdb_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(mal_id.to_string(), imdb.clone());
        imdb
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_search_item_like_mapanime() {
        // Hand-written sample following the Jikan v4 anime object schema (network was unavailable when written).
        let raw = r#"{
            "mal_id": 16498, "title": "Shingeki no Kyojin", "title_english": "Attack on Titan",
            "images": {"jpg": {"image_url": "small.jpg", "large_image_url": "large.jpg"}},
            "year": null, "aired": {"prop": {"from": {"year": 2013}}},
            "status": "Finished Airing", "episodes": 25, "type": "TV", "season": "spring", "score": 8.53,
            "synopsis": "Humanity fights.", "genres": [{"name": "Action"}], "themes": [{"name": "Military"}]
        }"#;
        let item: JikanAnime = serde_json::from_str(raw).unwrap();
        let m = map_anime(&item);
        assert_eq!(m.id, "16498");
        assert_eq!(m.title, "Attack on Titan");
        assert_eq!(m.cover_url.as_deref(), Some("large.jpg"));
        assert_eq!(m.year, Some(2013));
        assert_eq!(
            m.genres,
            Some(vec!["Action".to_string(), "Military".to_string()])
        );
        assert_eq!(m.episode_count, Some(25));
        assert_eq!(m.anime_type.as_deref(), Some("TV"));
        assert_eq!(m.season.as_deref(), Some("spring"));
        assert_eq!(m.score, Some(8.53));
    }

    #[test]
    fn falls_back_to_unknown_title_and_drops_zero_score() {
        let item: JikanAnime =
            serde_json::from_str(r#"{"mal_id": 1, "score": 0, "title_english": ""}"#).unwrap();
        let m = map_anime(&item);
        assert_eq!(m.title, "Unknown");
        assert_eq!(m.score, None);
        assert_eq!(m.year, None);
    }

    #[test]
    fn schedule_entry_keeps_broadcast_and_dedupes() {
        let raw = r#"{"data": [
            {"mal_id": 5, "title": "A", "broadcast": {"string": "Mondays at 23:00 (JST)"}},
            {"mal_id": 5, "title": "A", "broadcast": {"string": "Mondays at 23:00 (JST)"}},
            {"mal_id": 6, "title": "B", "broadcast": {"string": ""}}
        ]}"#;
        let env: Envelope<Vec<JikanAnime>> = serde_json::from_str(raw).unwrap();
        let items: Vec<ScheduleEntry> = env.data.unwrap().iter().map(map_schedule_entry).collect();
        let items = dedupe_by_id(items, |e| e.anime.id.as_str());
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0].broadcast.as_deref(),
            Some("Mondays at 23:00 (JST)")
        );
        assert_eq!(items[1].broadcast, None);
    }

    #[test]
    fn episode_id_uses_requested_mal_id() {
        let ep: JikanEpisode = serde_json::from_str(r#"{"mal_id": 3, "title": "", "title_japanese": "第3話", "aired": "2013-04-21T00:00:00+00:00"}"#).unwrap();
        let mapped = map_episode("16498", &ep);
        assert_eq!(mapped.id, "16498:3");
        assert_eq!(mapped.number, 3);
        assert_eq!(mapped.title.as_deref(), Some("第3話"));
        assert_eq!(
            mapped.air_date.as_deref(),
            Some("2013-04-21T00:00:00+00:00")
        );
    }

    #[test]
    fn schedule_day_strings() {
        assert_eq!(ScheduleDay::Friday.as_str(), "friday");
        assert_eq!(GENRE_SHORTCUTS.len(), 16);
    }

    #[test]
    fn limiter_spaces_request_starts() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let limiter = Limiter::new(Duration::from_millis(50));
            let start = Instant::now();
            limiter.acquire().await;
            limiter.acquire().await;
            limiter.acquire().await;
            assert!(start.elapsed() >= Duration::from_millis(100));
        });
    }

    #[test]
    fn breaker_opens_after_trip() {
        let breaker = Breaker::default();
        assert!(breaker.is_available());
        breaker.trip();
        assert!(!breaker.is_available());
    }
}
