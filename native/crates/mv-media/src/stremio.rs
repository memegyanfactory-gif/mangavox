//! Stremio addon protocol client: manifest, catalog, search, meta, streams and subtitles.
//! Mirrors `src/main/addons/stremio-adapter.ts` and the pure helpers in `addon-loader.ts`.
//!
//! Every method returns `Result`. The TypeScript search and stream calls swallow errors and return
//! empty lists; use `unwrap_or_default()` to get that behaviour.

use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde_json::Value;

use crate::error::{MediaError, Result};
use crate::types::{
    AnimeAddonCatalog, AnimeAddonManifest, AnimeEpisode, AnimeSearchResult, AnimeTorrentStream,
    CatalogType, StremioSubtitle,
};
use crate::util::{
    encode_uri_component, parse_leading_int, str_field, value_to_f64, value_to_string,
};

const TIMEOUT: Duration = Duration::from_secs(10);
const PAGE_SIZE: u32 = 100;

/// Strip trailing slashes (`normalizeEndpoint`).
pub fn normalize_endpoint(url: &str) -> String {
    url.trim_end_matches('/').to_string()
}

/// Resolve an install URL to `(manifest_url, endpoint)`. Accepts either a base URL or a `.../manifest.json` URL.
pub fn manifest_url(addon_url: &str) -> (String, String) {
    let base = normalize_endpoint(addon_url);
    match base.strip_suffix("/manifest.json") {
        Some(stripped) => (base.clone(), stripped.to_string()),
        None => (format!("{base}/manifest.json"), base),
    }
}

/// `{endpoint}/catalog/{type}/{id}[/skip={n}].json`
pub fn catalog_url(endpoint: &str, kind: &str, catalog_id: &str, page: u32) -> String {
    let skip = page.saturating_sub(1) * PAGE_SIZE;
    let suffix = if skip > 0 {
        format!("/skip={skip}")
    } else {
        String::new()
    };
    format!(
        "{}/catalog/{kind}/{catalog_id}{suffix}.json",
        normalize_endpoint(endpoint)
    )
}

/// `{endpoint}/catalog/{type}/{catalogId}/search={query}.json`. `catalog_id` defaults to `top`.
pub fn search_url(endpoint: &str, kind: &str, query: &str, catalog_id: Option<&str>) -> String {
    format!(
        "{}/catalog/{kind}/{}/search={}.json",
        normalize_endpoint(endpoint),
        catalog_id.unwrap_or("top"),
        encode_uri_component(query)
    )
}

/// `{endpoint}/meta/{type}/{id}.json`
pub fn meta_url(endpoint: &str, kind: &str, id: &str) -> String {
    format!("{}/meta/{kind}/{id}.json", normalize_endpoint(endpoint))
}

/// `{endpoint}/stream/{type}/{id}.json`
pub fn stream_url(endpoint: &str, kind: &str, id: &str) -> String {
    format!("{}/stream/{kind}/{id}.json", normalize_endpoint(endpoint))
}

/// `{endpoint}/subtitles/{type}/{id}.json`
pub fn subtitles_url(endpoint: &str, kind: &str, id: &str) -> String {
    format!(
        "{}/subtitles/{kind}/{id}.json",
        normalize_endpoint(endpoint)
    )
}

/// Result of [`StremioClient::fetch_meta`]: the series/movie info plus its video list.
#[derive(Debug, Clone, PartialEq)]
pub struct StremioMeta {
    pub info: AnimeSearchResult,
    pub episodes: Vec<AnimeEpisode>,
}

pub struct StremioClient {
    http: reqwest::Client,
}

impl StremioClient {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!(
                "MangaVox/",
                env!("CARGO_PKG_VERSION"),
                " (mv-media)"
            ))
            .build()?;
        Ok(Self { http })
    }

    async fn get_json(&self, url: &str) -> Result<Value> {
        let resp = self.http.get(url).timeout(TIMEOUT).send().await?;
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

    /// Fetch and validate an addon manifest. Fails when `id` or `name` is missing.
    pub async fn fetch_manifest(&self, addon_url: &str) -> Result<AnimeAddonManifest> {
        let (url, endpoint) = manifest_url(addon_url);
        let raw = self.get_json(&url).await?;
        parse_manifest(&raw, endpoint)
    }

    /// One page of a catalog (`/catalog/{type}/{id}`), 100 items per page.
    pub async fn fetch_catalog(
        &self,
        endpoint: &str,
        catalog_id: &str,
        kind: &str,
        page: u32,
    ) -> Result<Vec<AnimeSearchResult>> {
        let raw = self
            .get_json(&catalog_url(endpoint, kind, catalog_id, page))
            .await?;
        Ok(map_items(&raw, |m| catalog_item(m, kind)))
    }

    /// Search one catalog of an addon. The result is `Ok(empty)` when the addon returns no `metas`.
    pub async fn search_addon(
        &self,
        endpoint: &str,
        query: &str,
        kind: &str,
        catalog_id: Option<&str>,
    ) -> Result<Vec<AnimeSearchResult>> {
        let raw = self
            .get_json(&search_url(endpoint, kind, query, catalog_id))
            .await?;
        Ok(map_items(&raw, search_item))
    }

    /// Meta for one id: info plus episodes built from `meta.videos`.
    pub async fn fetch_meta(&self, endpoint: &str, id: &str, kind: &str) -> Result<StremioMeta> {
        let raw = self.get_json(&meta_url(endpoint, kind, id)).await?;
        Ok(parse_meta(&raw, id))
    }

    /// Torrent or direct streams for one resource id.
    pub async fn fetch_streams(
        &self,
        endpoint: &str,
        id: &str,
        kind: &str,
    ) -> Result<Vec<AnimeTorrentStream>> {
        let raw = self.get_json(&stream_url(endpoint, kind, id)).await?;
        Ok(parse_streams(&raw))
    }

    /// Subtitle tracks from an addon's `subtitles` resource. Entries without a URL are dropped.
    pub async fn fetch_subtitles(
        &self,
        endpoint: &str,
        id: &str,
        kind: &str,
    ) -> Result<Vec<StremioSubtitle>> {
        let raw = self.get_json(&subtitles_url(endpoint, kind, id)).await?;
        Ok(parse_subtitles(&raw))
    }
}

// ── Pure parsing (no I/O) ──

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

/// Truthy string coercion (`v || fallback`): non-empty, and numbers other than 0.
fn truthy_string(v: Option<&Value>) -> Option<String> {
    value_to_string(v).filter(|s| !s.is_empty() && s != "0")
}

/// `m.year ? parseInt(m.year) : undefined` for catalog and meta items.
fn parse_year(v: Option<&Value>) -> Option<i32> {
    truthy_string(v)
        .and_then(|s| parse_leading_int(&s))
        .and_then(|y| i32::try_from(y).ok())
}

fn parse_manifest(raw: &Value, endpoint: String) -> Result<AnimeAddonManifest> {
    let id = str_field(raw, "id");
    let name = str_field(raw, "name");
    let (Some(id), Some(name)) = (id, name) else {
        return Err(MediaError::InvalidManifest(
            "missing id or name".to_string(),
        ));
    };

    let catalogs = raw
        .get("catalogs")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|c| {
                    let kind = match c.get("type").and_then(Value::as_str)? {
                        "anime" => CatalogType::Anime,
                        "movie" => CatalogType::Movie,
                        "series" => CatalogType::Series,
                        _ => return None,
                    };
                    let cid = str_field(c, "id")?;
                    Some(AnimeAddonCatalog {
                        name: str_field(c, "name").unwrap_or_else(|| cid.clone()),
                        id: cid,
                        kind,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    // Resources may be plain strings or `{ "name": ... }` objects.
    let resources = raw
        .get("resources")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|r| match r {
                    Value::String(s) => Some(s.clone()),
                    other => str_field(other, "name"),
                })
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    Ok(AnimeAddonManifest {
        id,
        name,
        version: str_field(raw, "version").unwrap_or_else(|| "0.0.0".to_string()),
        description: str_field(raw, "description"),
        icon: str_field(raw, "logo").or_else(|| str_field(raw, "icon")),
        catalogs,
        endpoint,
        resources,
        types: string_array(raw.get("types")),
        id_prefixes: string_array(raw.get("idPrefixes")),
    })
}

/// Map a catalog/meta preview item (`fetchStremioCatalog`).
fn catalog_item(m: &Value, kind: &str) -> AnimeSearchResult {
    let id = str_field(m, "id")
        .or_else(|| str_field(m, "imdb_id"))
        .unwrap_or_default();
    let is_imdb = id.starts_with("tt");
    AnimeSearchResult {
        imdb_id: is_imdb.then(|| id.clone()),
        title: str_field(m, "name")
            .or_else(|| str_field(m, "title"))
            .unwrap_or_else(|| "Unknown".to_string()),
        cover_url: str_field(m, "poster").or_else(|| str_field(m, "logo")),
        banner_url: str_field(m, "background"),
        year: parse_year(m.get("year")),
        status: str_field(m, "status"),
        genres: Some(string_array(m.get("genres"))),
        description: str_field(m, "description"),
        anime_type: match kind {
            "movie" => Some("Movie".to_string()),
            "series" => Some("TV".to_string()),
            _ => None,
        },
        id,
        ..Default::default()
    }
}

/// Map a search result item (`searchStremioAddon`): same as a catalog item but without banner, type or IMDB id.
fn search_item(m: &Value) -> AnimeSearchResult {
    AnimeSearchResult {
        id: str_field(m, "id")
            .or_else(|| str_field(m, "imdb_id"))
            .unwrap_or_default(),
        title: str_field(m, "name")
            .or_else(|| str_field(m, "title"))
            .unwrap_or_else(|| "Unknown".to_string()),
        cover_url: str_field(m, "poster").or_else(|| str_field(m, "logo")),
        year: parse_year(m.get("year")),
        status: str_field(m, "status"),
        genres: Some(string_array(m.get("genres"))),
        description: str_field(m, "description"),
        ..Default::default()
    }
}

fn map_items(raw: &Value, f: impl Fn(&Value) -> AnimeSearchResult) -> Vec<AnimeSearchResult> {
    raw.get("metas")
        .and_then(Value::as_array)
        .map(|metas| metas.iter().map(f).collect())
        .unwrap_or_default()
}

fn parse_meta(raw: &Value, requested_id: &str) -> StremioMeta {
    let empty = Value::Object(Default::default());
    let meta = raw.get("meta").filter(|m| m.is_object()).unwrap_or(&empty);
    let videos = meta.get("videos").and_then(Value::as_array);

    let info = AnimeSearchResult {
        id: str_field(meta, "id").unwrap_or_else(|| requested_id.to_string()),
        title: str_field(meta, "name")
            .or_else(|| str_field(meta, "title"))
            .unwrap_or_else(|| "Unknown".to_string()),
        cover_url: str_field(meta, "poster").or_else(|| str_field(meta, "logo")),
        banner_url: str_field(meta, "background"),
        year: parse_year(meta.get("year")),
        status: str_field(meta, "status"),
        genres: Some(string_array(meta.get("genres"))),
        description: str_field(meta, "description"),
        episode_count: videos.map(|v| v.len() as u32),
        ..Default::default()
    };

    let episodes = videos
        .map(|items| {
            items
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let fallback_number = i + 1;
                    let season = truthy_string(v.get("season")).unwrap_or_else(|| "1".to_string());
                    let episode_text = truthy_string(v.get("episode"))
                        .unwrap_or_else(|| fallback_number.to_string());
                    let number = value_to_f64(v.get("episode"))
                        .filter(|&n| n != 0.0)
                        .map(|n| n as u32)
                        .unwrap_or(fallback_number as u32);
                    AnimeEpisode {
                        id: str_field(v, "id")
                            .unwrap_or_else(|| format!("{requested_id}:{season}:{episode_text}")),
                        number,
                        title: str_field(v, "title").or_else(|| str_field(v, "name")),
                        air_date: str_field(v, "released"),
                        thumbnail: str_field(v, "thumbnail"),
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    StremioMeta { info, episodes }
}

fn parse_streams(raw: &Value) -> Vec<AnimeTorrentStream> {
    let Some(items) = raw.get("streams").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|s| {
            let info_hash_field = str_field(s, "infoHash");
            let mut hash = info_hash_field
                .clone()
                .or_else(|| str_field(s, "url"))
                .unwrap_or_default();
            // Addons ship extra `tracker:` entries in `sources`. Bake them into the magnet so the swarm stays reachable.
            if let Some(info_hash) = &info_hash_field {
                let trackers: Vec<&str> = s
                    .get("sources")
                    .and_then(Value::as_array)
                    .map(|srcs| {
                        srcs.iter()
                            .filter_map(Value::as_str)
                            .filter_map(|src| src.strip_prefix("tracker:"))
                            .filter(|t| !t.is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
                if !trackers.is_empty() {
                    let tr: String = trackers
                        .iter()
                        .map(|t| format!("&tr={}", encode_uri_component(t)))
                        .collect();
                    hash = format!("magnet:?xt=urn:btih:{info_hash}{tr}");
                }
            }
            if hash.is_empty() {
                return None;
            }
            let title = str_field(s, "title")
                .or_else(|| str_field(s, "name"))
                .unwrap_or_else(|| "Unknown stream".to_string());
            let parsed = parse_torrentio_title(&title);
            Some(AnimeTorrentStream {
                info_hash: hash,
                file_idx: value_to_f64(s.get("fileIdx")).map(|n| n as u32),
                seeders: parsed
                    .seeders
                    .or_else(|| value_to_f64(s.get("seeders")).map(|n| n as u64)),
                quality: parsed.quality,
                size: parsed
                    .size_bytes
                    .or_else(|| value_to_f64(s.get("size")).map(|n| n.round() as u64)),
                source: parsed
                    .source
                    .or_else(|| str_field(s, "source"))
                    .or_else(|| str_field(s, "addon")),
                addon: str_field(s, "name").and_then(|n| n.lines().next().map(|l| l.trim().to_string())).filter(|n| !n.is_empty()),
                title,
            })
        })
        .collect()
}

fn parse_subtitles(raw: &Value) -> Vec<StremioSubtitle> {
    let Some(items) = raw.get("subtitles").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|s| {
            let url = str_field(s, "url")?;
            let lang = str_field(s, "lang")
                .unwrap_or_else(|| "unknown".to_string())
                .to_lowercase();
            let title = value_to_string(s.get("id"))
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| lang.clone());
            Some(StremioSubtitle { title, lang, url })
        })
        .collect()
}

#[derive(Debug, Default, PartialEq)]
struct ParsedTitle {
    quality: Option<String>,
    seeders: Option<u64>,
    size_bytes: Option<u64>,
    source: Option<String>,
}

static QUALITY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(2160p|1080p|720p|480p|4K)").unwrap());
static SEEDERS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"👤\s*([0-9]+)").unwrap());
static SIZE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)📝\s*([0-9.]+)\s*(GB|MB|TB)").unwrap());
static SOURCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("\u{2699}\u{FE0F}\\s*(\\S+)").unwrap());

/// Parse Torrentio-style titles: `"Name S01E01 2023 1080p WEB-DL\n👤 33 📝 8.84 GB ⚙️ ThePirateBay"`.
fn parse_torrentio_title(title: &str) -> ParsedTitle {
    let quality = QUALITY_RE.captures(title).map(|c| c[1].to_string());
    let seeders = SEEDERS_RE
        .captures(title)
        .and_then(|c| c[1].parse::<u64>().ok());
    let size_bytes = SIZE_RE.captures(title).and_then(|c| {
        let num = parse_float_prefix(&c[1])?;
        let multiplier = match c[2].to_ascii_uppercase().as_str() {
            "TB" => 1e12,
            "GB" => 1e9,
            _ => 1e6,
        };
        Some((num * multiplier).round() as u64)
    });
    let source = SOURCE_RE.captures(title).map(|c| c[1].to_string());
    ParsedTitle {
        quality,
        seeders,
        size_bytes,
        source,
    }
}

/// `parseFloat` on a string made of digits and dots: keeps the longest valid prefix, `None` if none.
fn parse_float_prefix(s: &str) -> Option<f64> {
    let head = s.split('.').take(2).collect::<Vec<_>>().join(".");
    head.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// True for addons that look like anime sources (`isAnimeAddon`).
pub fn is_anime_addon(manifest: &AnimeAddonManifest) -> bool {
    let is_anime_type = manifest.types.iter().any(|t| t == "anime");
    let has_anime_catalog = manifest
        .catalogs
        .iter()
        .any(|c| c.kind == CatalogType::Anime);
    let lc_id = manifest.id.to_lowercase();
    let lc_name = manifest.name.to_lowercase();
    let name_matches = ["anime", "kitsu", "animeo", "hanime"]
        .iter()
        .any(|k| lc_id.contains(k) || lc_name.contains(k));
    is_anime_type || has_anime_catalog || name_matches
}

/// First addon whose `idPrefixes` match the start of `id` (`findAddonForId`).
pub fn find_addon_for_id<'a>(
    addons: &'a [AnimeAddonManifest],
    id: &str,
) -> Option<&'a AnimeAddonManifest> {
    addons.iter().find(|addon| {
        addon
            .id_prefixes
            .iter()
            .any(|prefix| id.starts_with(prefix.as_str()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn manifest_url_normalizes_endpoint() {
        assert_eq!(
            manifest_url("https://x.example/addon/"),
            (
                "https://x.example/addon/manifest.json".to_string(),
                "https://x.example/addon".to_string()
            )
        );
        assert_eq!(
            manifest_url("https://x.example/addon/manifest.json"),
            (
                "https://x.example/addon/manifest.json".to_string(),
                "https://x.example/addon".to_string()
            )
        );
    }

    #[test]
    fn catalog_and_meta_urls_match_typescript() {
        let ep = "https://x.example/addon/";
        assert_eq!(
            catalog_url(ep, "movie", "top", 1),
            "https://x.example/addon/catalog/movie/top.json"
        );
        assert_eq!(
            catalog_url(ep, "movie", "top", 3),
            "https://x.example/addon/catalog/movie/top/skip=200.json"
        );
        assert_eq!(
            search_url(ep, "series", "a b&c", None),
            "https://x.example/addon/catalog/series/top/search=a%20b%26c.json"
        );
        assert_eq!(
            search_url(ep, "series", "q", Some("trending")),
            "https://x.example/addon/catalog/series/trending/search=q.json"
        );
        assert_eq!(
            meta_url(ep, "series", "tt1:1:2"),
            "https://x.example/addon/meta/series/tt1:1:2.json"
        );
        assert_eq!(
            stream_url(ep, "movie", "tt1375666"),
            "https://x.example/addon/stream/movie/tt1375666.json"
        );
        assert_eq!(
            subtitles_url(ep, "movie", "tt1375666"),
            "https://x.example/addon/subtitles/movie/tt1375666.json"
        );
    }

    #[test]
    fn parses_manifest_resources_and_catalog_filters() {
        let raw = json!({
            "id": "com.example", "name": "Example", "version": "1.2.3", "logo": "l.png",
            "catalogs": [
                {"type": "anime", "id": "popular", "name": ""},
                {"type": "tv", "id": "skipped"},
                {"type": "movie", "id": "m"}
            ],
            "resources": ["catalog", {"name": "stream"}, {"x": 1}],
            "types": ["anime", 5], "idPrefixes": ["kitsu:"]
        });
        let m = parse_manifest(&raw, "https://x.example".into()).unwrap();
        assert_eq!(m.version, "1.2.3");
        assert_eq!(m.icon.as_deref(), Some("l.png"));
        assert_eq!(m.catalogs.len(), 2);
        assert_eq!(m.catalogs[0].name, "popular");
        assert_eq!(m.catalogs[1].kind, CatalogType::Movie);
        assert_eq!(
            m.resources,
            vec!["catalog".to_string(), "stream".to_string()]
        );
        assert_eq!(m.types, vec!["anime".to_string()]);
        assert!(is_anime_addon(&m));
        assert!(parse_manifest(&json!({"id": "x"}), String::new()).is_err());
    }

    #[test]
    fn finds_addon_by_id_prefix() {
        let mk = |id: &str, prefix: &str| AnimeAddonManifest {
            id: id.into(),
            name: id.into(),
            version: "1".into(),
            description: None,
            icon: None,
            catalogs: vec![],
            endpoint: String::new(),
            resources: vec![],
            types: vec![],
            id_prefixes: vec![prefix.into()],
        };
        let addons = vec![mk("a", "tt"), mk("b", "kitsu:")];
        assert_eq!(
            find_addon_for_id(&addons, "kitsu:1:2").map(|a| a.id.as_str()),
            Some("b")
        );
        assert_eq!(
            find_addon_for_id(&addons, "tt1").map(|a| a.id.as_str()),
            Some("a")
        );
        assert!(find_addon_for_id(&addons, "zz").is_none());
    }

    #[test]
    fn catalog_items_map_like_fetch_stremio_catalog() {
        let m = json!({
            "id": "tt1375666", "name": "Inception", "poster": "p", "background": "b",
            "year": "2010", "genres": ["Sci-Fi"], "description": "d"
        });
        let item = catalog_item(&m, "movie");
        assert_eq!(item.id, "tt1375666");
        assert_eq!(item.imdb_id.as_deref(), Some("tt1375666"));
        assert_eq!(item.year, Some(2010));
        assert_eq!(item.banner_url.as_deref(), Some("b"));
        assert_eq!(item.anime_type.as_deref(), Some("Movie"));

        let s = search_item(&m);
        assert_eq!(s.banner_url, None);
        assert_eq!(s.anime_type, None);
        assert_eq!(s.imdb_id, None);
        assert_eq!(s.cover_url.as_deref(), Some("p"));
    }

    #[test]
    fn parses_torrentio_style_title() {
        let t = parse_torrentio_title(
            "Show S01E01 2023 1080p WEB-DL\n👤 33 📝 8.84 GB ⚙️ ThePirateBay",
        );
        assert_eq!(t.quality.as_deref(), Some("1080p"));
        assert_eq!(t.seeders, Some(33));
        assert_eq!(t.size_bytes, Some(8_840_000_000));
        assert_eq!(t.source.as_deref(), Some("ThePirateBay"));

        let small = parse_torrentio_title("x 📝 500 MB");
        assert_eq!(small.size_bytes, Some(500_000_000));
        assert_eq!(small.quality, None);
        assert_eq!(parse_float_prefix("1.2.3"), Some(1.2));
        assert_eq!(parse_float_prefix("."), None);
    }

    #[test]
    fn streams_bake_sources_into_magnet_and_drop_empty_hashes() {
        let raw = json!({"streams": [
            {"infoHash": "abcdef0123456789abcdef0123456789abcdef01", "fileIdx": 2,
             "title": "[SubsPlease] One Piece 1100 1080p\n👤 7 📝 1.5 GB ⚙️ Nyaa",
             "sources": ["dht:xyz", "tracker:udp://tracker.example:80/announce"]},
            {"url": "https://cdn.example/video.m3u8", "title": "Direct 720p"},
            {"title": "no hash"}
        ]});
        let streams = parse_streams(&raw);
        assert_eq!(streams.len(), 2);
        assert_eq!(
            streams[0].info_hash,
            "magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01&tr=udp%3A%2F%2Ftracker.example%3A80%2Fannounce"
        );
        assert_eq!(streams[0].file_idx, Some(2));
        assert_eq!(streams[0].seeders, Some(7));
        assert_eq!(streams[0].size, Some(1_500_000_000));
        assert_eq!(streams[0].source.as_deref(), Some("Nyaa"));
        assert_eq!(streams[1].info_hash, "https://cdn.example/video.m3u8");
        assert_eq!(streams[1].quality.as_deref(), Some("720p"));
    }

    #[test]
    fn subtitles_default_language_and_skip_missing_urls() {
        let raw = json!({"subtitles": [
            {"id": 3728010, "lang": "ENG", "url": "https://s/1.srt"},
            {"lang": "fre", "url": "https://s/2.srt"},
            {"lang": "spa"}
        ]});
        let subs = parse_subtitles(&raw);
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0].lang, "eng");
        assert_eq!(subs[0].title, "3728010");
        assert_eq!(subs[1].title, "fre");
    }

    #[test]
    fn meta_builds_episodes_with_fallbacks() {
        let raw = json!({"meta": {"id": "tt1", "name": "Show", "videos": [
            {"id": "tt1:1:1", "season": 1, "episode": 1, "title": "Pilot", "released": "2008-01-21"},
            {"season": 1, "name": "No episode number"}
        ]}});
        let meta = parse_meta(&raw, "tt1");
        assert_eq!(meta.info.episode_count, Some(2));
        assert_eq!(meta.episodes[0].title.as_deref(), Some("Pilot"));
        assert_eq!(meta.episodes[1].id, "tt1:1:2");
        assert_eq!(meta.episodes[1].number, 2);
        assert_eq!(meta.episodes[1].title.as_deref(), Some("No episode number"));
    }
}

#[cfg(test)]
mod fixture_tests {
    use super::*;

    #[test]
    fn recorded_cinemeta_manifest_parses() {
        let raw: Value =
            serde_json::from_str(include_str!("../tests/fixtures/cinemeta_manifest.json")).unwrap();
        let m = parse_manifest(&raw, "https://v3-cinemeta.strem.io".into()).unwrap();
        assert_eq!(m.id, "com.linvo.cinemeta");
        assert!(m.resources.iter().any(|r| r == "catalog"));
        assert!(m
            .catalogs
            .iter()
            .any(|c| c.kind == CatalogType::Movie && c.id == "top"));
        assert!(m
            .catalogs
            .iter()
            .any(|c| c.kind == CatalogType::Series && c.id == "top"));
        assert!(!is_anime_addon(&m));
    }

    #[test]
    fn recorded_opensubtitles_manifest_declares_subtitles() {
        let raw: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/opensubtitles_manifest.json"
        ))
        .unwrap();
        let m = parse_manifest(&raw, "https://opensubtitles-v3.strem.io".into()).unwrap();
        assert_eq!(m.resources, vec!["subtitles".to_string()]);
        assert_eq!(m.id_prefixes, vec!["tt".to_string()]);
    }

    #[test]
    fn recorded_opensubtitles_response_parses() {
        let raw: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/opensubtitles_subtitles_movie_inception.json"
        ))
        .unwrap();
        let subs = parse_subtitles(&raw);
        assert!(!subs.is_empty());
        assert!(subs.iter().all(|s| s.url.starts_with("https://")));
        assert!(subs.iter().all(|s| s.lang == s.lang.to_lowercase()));
    }

    #[test]
    fn recorded_cinemeta_movie_meta_parses_via_catalog_item_shape() {
        let raw: Value =
            serde_json::from_str(include_str!("../tests/fixtures/cinemeta_meta_movie.json"))
                .unwrap();
        let meta = parse_meta(&raw, "tt1375666");
        assert_eq!(meta.info.title, "Inception");
        assert_eq!(meta.info.year, Some(2010));
        assert_eq!(meta.info.episode_count, Some(0)); // Cinemeta movies ship an empty `videos` array
        assert!(meta.episodes.is_empty());
    }
}
