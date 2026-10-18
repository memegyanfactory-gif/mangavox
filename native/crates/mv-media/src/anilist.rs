//! AniList GraphQL client. Used as the fallback when Jikan (api.jikan.moe) is unreachable.
//!
//! Results use the same shapes as the Jikan client: `id` is the MAL id, so routes keyed by MAL id keep
//! working. Entries without a MAL id are dropped from lists.
//!
//! - Requests are spaced 700 ms apart (about 85/min, under AniList's 90/min cap). 429s are retried
//!   after `Retry-After` (capped), up to 3 times.
//! - Media fetched by MAL id is cached for 10 minutes, so details + episodes cost one request.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use regex::Regex;
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::error::{MediaError, Result};
use crate::jikan::Limiter;
use crate::types::{AnimeEpisode, AnimeSearchResult};
use crate::util::non_empty;

pub const ANILIST_URL: &str = "https://graphql.anilist.co";

const MIN_INTERVAL: Duration = Duration::from_millis(700);
const TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RATE_LIMIT_RETRIES: u32 = 3;
const MAX_RETRY_AFTER: Duration = Duration::from_secs(10);
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const PAGE_SIZE: u32 = 25;
/// Jikan returns 100 episodes per page; synthesized episode pages match that.
pub const EPISODES_PER_PAGE: u32 = 100;

const MEDIA_FIELDS: &str =
    "id idMal title { romaji english } synonyms format status episodes duration \
season seasonYear startDate { year } averageScore meanScore genres bannerImage \
coverImage { extraLarge large color } description(asHtml: false) trailer { id site } \
studios(isMain: true) { nodes { name } } nextAiringEpisode { episode airingAt }";

// ── Raw AniList payloads ──

#[derive(Deserialize, Default, Clone)]
#[serde(default, rename_all = "camelCase")]
struct AniMedia {
    id: u64,
    id_mal: Option<u64>,
    title: AniTitle,
    synonyms: Vec<String>,
    format: Option<String>,
    status: Option<String>,
    episodes: Option<u32>,
    duration: Option<u32>,
    season: Option<String>,
    season_year: Option<i32>,
    start_date: Option<AniDate>,
    average_score: Option<f64>,
    mean_score: Option<f64>,
    genres: Vec<String>,
    banner_image: Option<String>,
    cover_image: Option<AniCover>,
    description: Option<String>,
    trailer: Option<AniTrailer>,
    studios: Option<AniStudios>,
    next_airing_episode: Option<AniNextEpisode>,
    streaming_episodes: Vec<AniStreamingEpisode>,
    characters: Option<AniCharacters>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct AniCharacters {
    edges: Vec<AniCharacterEdge>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default, rename_all = "camelCase")]
struct AniCharacterEdge {
    node: Option<AniPerson>,
    voice_actors: Vec<AniPerson>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct AniPerson {
    name: AniPersonName,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct AniPersonName {
    full: Option<String>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct AniTitle {
    romaji: Option<String>,
    english: Option<String>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct AniDate {
    year: Option<i32>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default, rename_all = "camelCase")]
struct AniCover {
    extra_large: Option<String>,
    large: Option<String>,
    color: Option<String>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct AniTrailer {
    id: Option<String>,
    site: Option<String>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct AniStudios {
    nodes: Vec<AniNamed>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct AniNamed {
    name: String,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default, rename_all = "camelCase")]
struct AniNextEpisode {
    episode: u32,
    airing_at: i64,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct AniStreamingEpisode {
    title: Option<String>,
    thumbnail: Option<String>,
}

#[derive(Deserialize)]
struct PageData {
    #[serde(rename = "Page")]
    page: Option<PageMedia>,
}

#[derive(Deserialize)]
struct PageMedia {
    #[serde(default)]
    media: Vec<AniMedia>,
}

#[derive(Deserialize)]
struct MediaData {
    #[serde(rename = "Media")]
    media: Option<AniMedia>,
}

#[derive(Deserialize)]
struct GqlResponse<T> {
    data: Option<T>,
}

// ── Mapping ──

/// AniList `format` to Jikan's `type` spelling.
fn map_format(format: &str) -> Option<&'static str> {
    Some(match format {
        "TV" | "TV_SHORT" => "TV",
        "MOVIE" => "Movie",
        "OVA" => "OVA",
        "ONA" => "ONA",
        "SPECIAL" => "Special",
        "MUSIC" => "Music",
        _ => return None,
    })
}

/// Jikan-style `kind` filter ("tv", "movie", ...) to an AniList `MediaFormat`.
fn format_filter(kind: &str) -> Option<&'static str> {
    Some(match kind.to_ascii_lowercase().as_str() {
        "tv" => "TV",
        "movie" => "MOVIE",
        "ova" => "OVA",
        "ona" => "ONA",
        "special" => "SPECIAL",
        "music" => "MUSIC",
        _ => return None,
    })
}

/// AniList status to Jikan's human text.
fn map_status(status: &str) -> &'static str {
    match status {
        "FINISHED" => "Finished Airing",
        "RELEASING" => "Currently Airing",
        "NOT_YET_RELEASED" => "Not yet aired",
        "CANCELLED" => "Cancelled",
        "HIATUS" => "On Hiatus",
        _ => "Unknown",
    }
}

/// A MAL genre id (as used by the Discover page) to an AniList filter: `(is_tag, name)`.
fn mal_genre(id: u32) -> Option<(bool, &'static str)> {
    Some(match id {
        1 => (false, "Action"),
        2 => (false, "Adventure"),
        4 => (false, "Comedy"),
        7 => (false, "Mystery"),
        8 => (false, "Drama"),
        9 => (false, "Ecchi"),
        10 => (false, "Fantasy"),
        14 => (false, "Horror"),
        18 => (false, "Mecha"),
        19 => (false, "Music"),
        22 => (false, "Romance"),
        24 => (false, "Sci-Fi"),
        30 => (false, "Sports"),
        36 => (false, "Slice of Life"),
        37 => (false, "Supernatural"),
        40 => (false, "Psychological"),
        41 => (false, "Thriller"),
        23 => (true, "School"),
        25 => (true, "Shoujo"),
        27 => (true, "Shounen"),
        29 => (true, "Space"),
        38 => (true, "Military"),
        42 => (true, "Seinen"),
        43 => (true, "Josei"),
        62 => (true, "Isekai"),
        _ => return None,
    })
}

/// Strip AniList's HTML (`<br>`, `<i>`, ...) and decode the common entities.
fn strip_html(s: &str) -> String {
    static BR: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    static BLANK: OnceLock<Regex> = OnceLock::new();
    let br = BR.get_or_init(|| Regex::new(r"(?i)<br\s*/?>\n?").unwrap());
    let tag = TAG.get_or_init(|| Regex::new(r"<[^>]*>").unwrap());
    let blank = BLANK.get_or_init(|| Regex::new(r"\n[ \t]*(\n[ \t]*)+").unwrap());
    let s = s.replace('\r', "");
    let s = br.replace_all(&s, "\n");
    let s = tag.replace_all(&s, "");
    let s = s
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&mdash;", "—");
    blank.replace_all(&s, "\n\n").trim().to_string()
}

/// Map an AniList media object. `None` when it has no MAL id.
fn map_media(m: &AniMedia) -> Option<AnimeSearchResult> {
    let mal = m.id_mal.filter(|&id| id != 0)?;
    let english = non_empty(m.title.english.clone());
    let romaji = non_empty(m.title.romaji.clone());
    let title = english
        .clone()
        .or_else(|| romaji.clone())
        .unwrap_or_else(|| "Unknown".to_string());
    let mut alt_titles: Vec<String> = romaji
        .into_iter()
        .chain(m.synonyms.iter().cloned())
        .filter(|t| !t.is_empty() && *t != title)
        .collect();
    alt_titles.dedup();
    let cover = m.cover_image.as_ref();
    let trailer_url = m.trailer.as_ref().and_then(|t| {
        let id = t.id.as_deref()?.trim();
        if id.is_empty() {
            return None;
        }
        match t.site.as_deref() {
            Some("youtube") => Some(format!("https://www.youtube.com/watch?v={id}")),
            Some("dailymotion") => Some(format!("https://www.dailymotion.com/video/{id}")),
            _ => None,
        }
    });
    let studios: Vec<String> = m
        .studios
        .iter()
        .flat_map(|s| s.nodes.iter())
        .map(|n| n.name.clone())
        .filter(|n| !n.is_empty())
        .collect();
    // Main characters' Japanese voice actors, or the character name when no actor is listed.
    let mut cast: Vec<String> = Vec::new();
    for edge in m.characters.iter().flat_map(|c| c.edges.iter()) {
        let name = edge
            .voice_actors
            .first()
            .or(edge.node.as_ref())
            .and_then(|p| non_empty(p.name.full.clone()));
        if let Some(name) = name.filter(|n| !cast.contains(n)) {
            cast.push(name);
        }
    }
    Some(AnimeSearchResult {
        id: mal.to_string(),
        title,
        alt_titles: (!alt_titles.is_empty()).then_some(alt_titles),
        cover_url: cover
            .and_then(|c| non_empty(c.extra_large.clone()).or_else(|| non_empty(c.large.clone()))),
        banner_url: non_empty(m.banner_image.clone()),
        year: m
            .season_year
            .or_else(|| m.start_date.as_ref().and_then(|d| d.year))
            .filter(|&y| y != 0),
        status: m.status.as_deref().map(|s| map_status(s).to_string()),
        genres: Some(m.genres.clone()),
        description: m
            .description
            .as_deref()
            .map(strip_html)
            .filter(|d| !d.is_empty()),
        episode_count: m.episodes.filter(|&n| n != 0),
        anime_type: m.format.as_deref().and_then(map_format).map(str::to_string),
        season: m.season.as_deref().map(str::to_ascii_lowercase),
        score: m
            .average_score
            .or(m.mean_score)
            .filter(|&s| s != 0.0)
            .map(|s| s / 10.0),
        anilist_id: Some(m.id),
        color: cover.and_then(|c| non_empty(c.color.clone())),
        trailer_url,
        studios: (!studios.is_empty()).then_some(studios),
        cast: (!cast.is_empty()).then_some(cast),
        duration_minutes: m.duration.filter(|&d| d != 0),
        next_episode: m.next_airing_episode.as_ref().map(|n| n.episode),
        next_episode_at: m.next_airing_episode.as_ref().map(|n| n.airing_at),
        ..Default::default()
    })
}

fn map_list(media: &[AniMedia]) -> Vec<AnimeSearchResult> {
    let mut seen = std::collections::HashSet::new();
    media
        .iter()
        .filter_map(map_media)
        .filter(|a| seen.insert(a.id.clone()))
        .collect()
}

/// Synthesize one page of episodes: AniList has no per-episode list, only a count plus
/// (sometimes) streaming episode titles and thumbnails.
fn synth_episodes(mal_id: &str, m: &AniMedia, page: u32) -> Vec<AnimeEpisode> {
    static EP: OnceLock<Regex> = OnceLock::new();
    let ep_re =
        EP.get_or_init(|| Regex::new(r"(?i)^\s*episode\s+(\d+)\s*(?:[-:–]\s*(.*))?$").unwrap());
    let mut streaming: HashMap<u32, (Option<String>, Option<String>)> = HashMap::new();
    for (index, se) in m.streaming_episodes.iter().enumerate() {
        let raw = se.title.clone().unwrap_or_default();
        let (number, title) = match ep_re.captures(&raw) {
            Some(c) => (
                c[1].parse().unwrap_or(index as u32 + 1),
                c.get(2).map(|t| t.as_str().trim().to_string()),
            ),
            None => (index as u32 + 1, Some(raw.trim().to_string())),
        };
        streaming
            .entry(number)
            .or_insert((non_empty(title), non_empty(se.thumbnail.clone())));
    }
    // Airing shows: only episodes that already aired. Otherwise the announced count.
    let total = match (&m.next_airing_episode, m.episodes) {
        (Some(next), _) => next.episode.saturating_sub(1),
        (None, Some(n)) if n > 0 => n,
        _ if m.status.as_deref() == Some("NOT_YET_RELEASED") => 0,
        _ => streaming.keys().copied().max().unwrap_or(0),
    };
    let start = page.max(1).saturating_sub(1) * EPISODES_PER_PAGE + 1;
    let end = total.min(page.max(1) * EPISODES_PER_PAGE);
    (start..=end)
        .map(|n| {
            let (title, thumbnail) = streaming.get(&n).cloned().unwrap_or_default();
            AnimeEpisode {
                id: format!("{mal_id}:{n}"),
                number: n,
                title,
                air_date: None,
                thumbnail,
            }
        })
        .collect()
}

/// `(year, season)` for "now", using MAL's quarters (Jan-Mar winter, Apr-Jun spring, ...).
fn current_season() -> (i32, &'static str) {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0) as i64;
    let (year, month) = civil_year_month(days);
    let season = match month {
        1..=3 => "WINTER",
        4..=6 => "SPRING",
        7..=9 => "SUMMER",
        _ => "FALL",
    };
    (year, season)
}

/// Days since 1970-01-01 to `(year, month)` (Howard Hinnant's civil_from_days).
fn civil_year_month(days: i64) -> (i32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year as i32, month)
}

/// Filters for a `Page { media(...) }` query. Unset fields are sent as null, which AniList ignores.
#[derive(Default)]
struct ListQuery<'a> {
    sort: &'a str,
    kind: Option<&'a str>,
    status: Option<&'a str>,
    season: Option<&'a str>,
    season_year: Option<i32>,
    search: Option<&'a str>,
    genre: Option<&'a str>,
    tag: Option<&'a str>,
}

pub struct AniListClient {
    http: reqwest::Client,
    url: String,
    limiter: Limiter,
    media_cache: Mutex<HashMap<String, (Instant, AniMedia)>>,
}

static SHARED: OnceLock<AniListClient> = OnceLock::new();

impl AniListClient {
    pub fn new() -> Result<Self> {
        Self::with_url(ANILIST_URL)
    }

    pub fn with_url(url: impl Into<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!(
                "MangaVox/",
                env!("CARGO_PKG_VERSION"),
                " (mv-media)"
            ))
            .connect_timeout(CONNECT_TIMEOUT)
            .build()?;
        Ok(Self {
            http,
            url: url.into(),
            limiter: Limiter::new(MIN_INTERVAL),
            media_cache: Mutex::new(HashMap::new()),
        })
    }

    /// Process-wide client, so every caller shares one rate limiter and cache.
    pub fn shared() -> Result<&'static AniListClient> {
        if let Some(client) = SHARED.get() {
            return Ok(client);
        }
        let client = Self::new()?;
        Ok(SHARED.get_or_init(|| client))
    }

    async fn query<T: serde::de::DeserializeOwned>(
        &self,
        query: &str,
        variables: Value,
    ) -> Result<T> {
        let body = json!({ "query": query, "variables": variables });
        let mut retries = MAX_RATE_LIMIT_RETRIES;
        loop {
            self.limiter.acquire().await;
            let resp = self
                .http
                .post(&self.url)
                .header("Accept", "application/json")
                .json(&body)
                .timeout(TIMEOUT)
                .send()
                .await?;
            let status = resp.status();
            if status == StatusCode::TOO_MANY_REQUESTS && retries > 0 {
                retries -= 1;
                let wait = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .map(Duration::from_secs)
                    .unwrap_or(Duration::from_secs(2))
                    .min(MAX_RETRY_AFTER);
                tokio::time::sleep(wait).await;
                continue;
            }
            if status == StatusCode::NOT_FOUND {
                return Err(MediaError::NotFound("AniList media".to_string()));
            }
            if !status.is_success() {
                return Err(MediaError::Status {
                    status: status.as_u16(),
                    url: self.url.clone(),
                });
            }
            let text = resp.text().await?;
            let parsed: GqlResponse<T> =
                serde_json::from_str(&text).map_err(|source| MediaError::Json {
                    url: self.url.clone(),
                    source,
                })?;
            return parsed
                .data
                .ok_or_else(|| MediaError::NotFound("AniList returned no data".to_string()));
        }
    }

    async fn list(&self, q: ListQuery<'_>, page: u32) -> Result<Vec<AnimeSearchResult>> {
        let query = format!(
            "query($page: Int, $perPage: Int, $sort: [MediaSort], $format: MediaFormat, \
             $status: MediaStatus, $season: MediaSeason, $seasonYear: Int, $search: String, \
             $genre: String, $tag: String) {{ Page(page: $page, perPage: $perPage) {{ \
             media(type: ANIME, isAdult: false, sort: $sort, format: $format, status: $status, \
             season: $season, seasonYear: $seasonYear, search: $search, genre: $genre, tag: $tag) \
             {{ {MEDIA_FIELDS} }} }} }}"
        );
        let mut vars = Map::new();
        vars.insert("page".into(), json!(page.max(1)));
        vars.insert("perPage".into(), json!(PAGE_SIZE));
        vars.insert("sort".into(), json!([q.sort]));
        let optional = [
            ("format", q.kind.and_then(format_filter).map(Value::from)),
            ("status", q.status.map(Value::from)),
            ("season", q.season.map(Value::from)),
            ("seasonYear", q.season_year.map(Value::from)),
            ("search", q.search.map(Value::from)),
            ("genre", q.genre.map(Value::from)),
            ("tag", q.tag.map(Value::from)),
        ];
        for (key, value) in optional {
            if let Some(v) = value {
                vars.insert(key.into(), v);
            }
        }
        let data: PageData = self.query(&query, Value::Object(vars)).await?;
        Ok(data.page.map(|p| map_list(&p.media)).unwrap_or_default())
    }

    /// Top anime by score. `kind`: "tv" | "movie" | None.
    pub async fn top_anime(&self, page: u32, kind: Option<&str>) -> Result<Vec<AnimeSearchResult>> {
        self.list(
            ListQuery {
                sort: "SCORE_DESC",
                kind,
                ..Default::default()
            },
            page,
        )
        .await
    }

    /// Currently airing anime, most popular first.
    pub async fn airing_anime(&self, page: u32) -> Result<Vec<AnimeSearchResult>> {
        self.list(
            ListQuery {
                sort: "POPULARITY_DESC",
                status: Some("RELEASING"),
                ..Default::default()
            },
            page,
        )
        .await
    }

    /// Anime for a year and season ("winter" | "spring" | "summer" | "fall"); `None` means now.
    pub async fn season_anime(
        &self,
        year: Option<i32>,
        season: Option<&str>,
        page: u32,
    ) -> Result<Vec<AnimeSearchResult>> {
        let (year, season) = match (year, season) {
            (Some(y), Some(s)) => (y, s.to_ascii_uppercase()),
            _ => {
                let (y, s) = current_season();
                (y, s.to_string())
            }
        };
        self.list(
            ListQuery {
                sort: "POPULARITY_DESC",
                season: Some(&season),
                season_year: Some(year),
                ..Default::default()
            },
            page,
        )
        .await
    }

    /// Upcoming anime, most popular first.
    pub async fn upcoming_anime(&self, page: u32) -> Result<Vec<AnimeSearchResult>> {
        self.list(
            ListQuery {
                sort: "POPULARITY_DESC",
                status: Some("NOT_YET_RELEASED"),
                ..Default::default()
            },
            page,
        )
        .await
    }

    /// Search by title, best match first. `kind`: "tv" | "movie" | None.
    pub async fn search_anime(
        &self,
        query: &str,
        page: u32,
        kind: Option<&str>,
    ) -> Result<Vec<AnimeSearchResult>> {
        self.list(
            ListQuery {
                sort: "SEARCH_MATCH",
                search: Some(query),
                kind,
                ..Default::default()
            },
            page,
        )
        .await
    }

    /// Anime for one MAL genre id, sorted by score.
    pub async fn anime_by_genre(
        &self,
        mal_genre_id: u32,
        page: u32,
        kind: Option<&str>,
    ) -> Result<Vec<AnimeSearchResult>> {
        let (is_tag, name) = mal_genre(mal_genre_id).ok_or_else(|| {
            MediaError::NotFound(format!("AniList genre for MAL id {mal_genre_id}"))
        })?;
        let mut q = ListQuery {
            sort: "SCORE_DESC",
            kind,
            ..Default::default()
        };
        if is_tag {
            q.tag = Some(name);
        } else {
            q.genre = Some(name);
        }
        self.list(q, page).await
    }

    /// Raw media for a MAL id, cached for 10 minutes.
    async fn media_by_mal(&self, mal_id: &str) -> Result<AniMedia> {
        if let Some((at, media)) = self.cache().get(mal_id) {
            if at.elapsed() < CACHE_TTL {
                return Ok(media.clone());
            }
        }
        let id: u64 = mal_id
            .trim()
            .parse()
            .map_err(|_| MediaError::NotFound(format!("anime {mal_id}")))?;
        let query = format!(
            "query($id: Int) {{ Media(idMal: $id, type: ANIME) {{ {MEDIA_FIELDS} \
             streamingEpisodes {{ title thumbnail }} \
             characters(role: MAIN, sort: [ROLE, RELEVANCE], perPage: 8) {{ edges {{ \
             node {{ name {{ full }} }} voiceActors(language: JAPANESE) {{ name {{ full }} }} }} }} }} }}"
        );
        let data: MediaData = self.query(&query, json!({ "id": id })).await?;
        let media = data
            .media
            .ok_or_else(|| MediaError::NotFound(format!("anime {mal_id}")))?;
        self.cache()
            .insert(mal_id.to_string(), (Instant::now(), media.clone()));
        Ok(media)
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, HashMap<String, (Instant, AniMedia)>> {
        self.media_cache.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Full details for one MAL id.
    pub async fn anime_details(&self, mal_id: &str) -> Result<AnimeSearchResult> {
        let media = self.media_by_mal(mal_id).await?;
        map_media(&media).ok_or_else(|| MediaError::NotFound(format!("anime {mal_id}")))
    }

    /// Synthesized episode list for one MAL id, 100 per page like Jikan.
    pub async fn anime_episodes(&self, mal_id: &str, page: u32) -> Result<Vec<AnimeEpisode>> {
        let media = self.media_by_mal(mal_id).await?;
        Ok(synth_episodes(mal_id, &media, page))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"data": {"Media": {
        "id": 16498, "idMal": 16498,
        "title": {"romaji": "Shingeki no Kyojin", "english": "Attack on Titan"},
        "synonyms": ["AoT", "SnK"], "format": "TV", "status": "FINISHED", "episodes": 25,
        "duration": 24, "season": "SPRING", "seasonYear": 2013, "startDate": {"year": 2013},
        "averageScore": 85, "genres": ["Action", "Drama"],
        "bannerImage": "https://img/banner.jpg",
        "coverImage": {"extraLarge": "https://img/xl.jpg", "large": "https://img/l.jpg", "color": "#f1a143"},
        "description": "Titans came.<br><br>\r\nEren &amp; Mikasa <i>watch</i>.<br>\n(Source: X)",
        "trailer": {"id": "LHtdKWJdif4\t", "site": "youtube"},
        "studios": {"nodes": [{"name": "WIT STUDIO"}]},
        "nextAiringEpisode": null,
        "characters": {"edges": [
            {"node": {"name": {"full": "Eren Yeager"}}, "voiceActors": [{"name": {"full": "Yuki Kaji"}}]},
            {"node": {"name": {"full": "Mikasa Ackerman"}}, "voiceActors": []}
        ]},
        "streamingEpisodes": [
            {"title": "Episode 2 - That Day", "thumbnail": "https://img/2.jpg"},
            {"title": "Episode 1 - To You, 2,000 Years in the Future", "thumbnail": "https://img/1.jpg"}
        ]
    }}}"##;

    fn fixture_media() -> AniMedia {
        let r: GqlResponse<MediaData> = serde_json::from_str(FIXTURE).unwrap();
        r.data.unwrap().media.unwrap()
    }

    #[test]
    fn maps_media_like_jikan() {
        let m = map_media(&fixture_media()).unwrap();
        assert_eq!(m.id, "16498");
        assert_eq!(m.title, "Attack on Titan");
        assert_eq!(
            m.alt_titles,
            Some(vec![
                "Shingeki no Kyojin".into(),
                "AoT".into(),
                "SnK".into()
            ])
        );
        assert_eq!(m.cover_url.as_deref(), Some("https://img/xl.jpg"));
        assert_eq!(m.banner_url.as_deref(), Some("https://img/banner.jpg"));
        assert_eq!(m.year, Some(2013));
        assert_eq!(m.status.as_deref(), Some("Finished Airing"));
        assert_eq!(m.genres, Some(vec!["Action".into(), "Drama".into()]));
        assert_eq!(
            m.description.as_deref(),
            Some("Titans came.\n\nEren & Mikasa watch.\n(Source: X)")
        );
        assert_eq!(m.episode_count, Some(25));
        assert_eq!(m.anime_type.as_deref(), Some("TV"));
        assert_eq!(m.season.as_deref(), Some("spring"));
        assert_eq!(m.score, Some(8.5));
        assert_eq!(m.anilist_id, Some(16498));
        assert_eq!(m.color.as_deref(), Some("#f1a143"));
        assert_eq!(
            m.trailer_url.as_deref(),
            Some("https://www.youtube.com/watch?v=LHtdKWJdif4")
        );
        assert_eq!(m.studios, Some(vec!["WIT STUDIO".into()]));
        assert_eq!(m.duration_minutes, Some(24));
        assert_eq!(m.next_episode, None);
        assert_eq!(
            m.cast,
            Some(vec!["Yuki Kaji".into(), "Mikasa Ackerman".into()])
        );
    }

    #[test]
    fn lists_drop_entries_without_mal_id_and_dedupe() {
        let raw = r#"{"data": {"Page": {"media": [
            {"id": 1, "idMal": 10, "title": {"romaji": "Romaji Only", "english": null}, "format": "MOVIE"},
            {"id": 2, "idMal": null, "title": {"romaji": "No Mal"}},
            {"id": 3, "idMal": 10, "title": {"romaji": "Dup"}},
            {"id": 4, "idMal": 11, "title": {}, "format": "SPECIAL", "averageScore": null, "meanScore": 70}
        ]}}}"#;
        let r: GqlResponse<PageData> = serde_json::from_str(raw).unwrap();
        let list = map_list(&r.data.unwrap().page.unwrap().media);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].title, "Romaji Only");
        assert_eq!(list[0].alt_titles, None);
        assert_eq!(list[0].anime_type.as_deref(), Some("Movie"));
        assert_eq!(list[1].title, "Unknown");
        assert_eq!(list[1].anime_type.as_deref(), Some("Special"));
        assert_eq!(list[1].score, Some(7.0));
    }

    #[test]
    fn synthesizes_episodes_with_streaming_titles() {
        let eps = synth_episodes("16498", &fixture_media(), 1);
        assert_eq!(eps.len(), 25);
        assert_eq!(eps[0].id, "16498:1");
        assert_eq!(
            eps[0].title.as_deref(),
            Some("To You, 2,000 Years in the Future")
        );
        assert_eq!(eps[1].thumbnail.as_deref(), Some("https://img/2.jpg"));
        assert_eq!(eps[2].title, None);
        assert!(synth_episodes("16498", &fixture_media(), 2).is_empty());
    }

    #[test]
    fn airing_shows_count_aired_episodes_and_page_by_100() {
        let mut m = fixture_media();
        m.episodes = None;
        m.streaming_episodes.clear();
        m.next_airing_episode = Some(AniNextEpisode {
            episode: 151,
            airing_at: 0,
        });
        let p1 = synth_episodes("1", &m, 1);
        let p2 = synth_episodes("1", &m, 2);
        assert_eq!(p1.len(), 100);
        assert_eq!(p2.len(), 50);
        assert_eq!(p2[0].number, 101);
        assert_eq!(p2.last().unwrap().number, 150);
    }

    #[test]
    fn genre_and_format_mapping() {
        assert_eq!(mal_genre(41), Some((false, "Thriller")));
        assert_eq!(mal_genre(62), Some((true, "Isekai")));
        assert_eq!(mal_genre(9999), None);
        for id in [1, 2, 4, 8, 22, 10, 24, 41, 14, 7] {
            assert!(mal_genre(id).is_some(), "discover genre {id} unmapped");
        }
        assert_eq!(format_filter("tv"), Some("TV"));
        assert_eq!(map_format("TV_SHORT"), Some("TV"));
        assert_eq!(map_status("RELEASING"), "Currently Airing");
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_year_month(0), (1970, 1));
        // 2026-10-19
        assert_eq!(civil_year_month(20_745), (2026, 10));
        // 2024-02-29
        assert_eq!(civil_year_month(19_782), (2024, 2));
    }

    /// Live check against graphql.anilist.co and arm.haglund.dev.
    /// Run: `cargo test -p mv-media -- --ignored anilist_live --nocapture`
    #[tokio::test]
    #[ignore]
    async fn anilist_live() {
        let c = AniListClient::new().unwrap();
        let top = c.top_anime(1, None).await.unwrap();
        let airing = c.airing_anime(1).await.unwrap();
        let season = c.season_anime(None, None, 1).await.unwrap();
        let search = c.search_anime("frieren", 1, None).await.unwrap();
        let genre = c.anime_by_genre(41, 1, None).await.unwrap();
        for (name, list) in [
            ("top", &top),
            ("airing", &airing),
            ("season", &season),
            ("search", &search),
            ("genre", &genre),
        ] {
            println!(
                "{name}: {} items, first = {:?}",
                list.len(),
                list.first().map(|a| (&a.id, &a.title))
            );
            assert!(!list.is_empty(), "{name} empty");
            assert!(list.iter().all(|a| a.id.parse::<u64>().is_ok()));
        }
        let details = c.anime_details("16498").await.unwrap();
        println!(
            "details: {} score={:?} trailer={:?} banner={:?} cast={:?}",
            details.title, details.score, details.trailer_url, details.banner_url, details.cast
        );
        assert!(details.cast.is_some());
        assert_eq!(details.title, "Attack on Titan");
        let eps = c.anime_episodes("16498", 1).await.unwrap();
        println!("episodes: {} first = {:?}", eps.len(), eps.first());
        assert_eq!(eps.len(), 25);
        let airing_id = &airing[0].id;
        let airing_eps = c.anime_episodes(airing_id, 1).await.unwrap();
        println!("airing {airing_id} episodes: {}", airing_eps.len());

        let jikan = crate::jikan::JikanClient::new().unwrap();
        let imdb = jikan.imdb_id("16498").await;
        println!("imdb 16498 = {imdb:?}");
        assert_eq!(imdb.as_deref(), Some("tt2560140"));
        for a in airing.iter().take(5) {
            println!(
                "imdb {} ({}) = {:?}",
                a.id,
                a.title,
                jikan.imdb_id(&a.id).await
            );
        }
    }
}
