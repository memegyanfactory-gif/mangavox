//! Shared media types, mirroring `src/shared/anime-addon-interface.ts`.
//! Fields serialize as camelCase so JSON output matches the Electron IPC shapes.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CatalogType {
    Anime,
    Movie,
    Series,
}

impl CatalogType {
    pub fn as_str(self) -> &'static str {
        match self {
            CatalogType::Anime => "anime",
            CatalogType::Movie => "movie",
            CatalogType::Series => "series",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CatalogSource {
    Addon,
    Tmdb,
    Cinemeta,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnimeAddonCatalog {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: CatalogType,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimeSearchResult {
    /// MAL id for Jikan results; IMDB id (tt...) or addon id for Cinemeta/Stremio results.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<u64>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt_titles: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banner_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genres: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_count: Option<u32>,
    /// "TV" | "Movie" | "OVA" | "ONA" | "Special" | "Music"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anime_type: Option<String>,
    /// "spring" | "summer" | "fall" | "winter"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_source: Option<CatalogSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_source_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimeEpisode {
    pub id: String,
    pub number: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub air_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimeTorrentStream {
    /// Magnet info hash, a magnet URI with trackers, or a full HTTP URL for direct streams.
    pub info_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_idx: Option<u32>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seeders: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    /// Bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimeAddonManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub catalogs: Vec<AnimeAddonCatalog>,
    /// Normalized base URL (no trailing slash, no `/manifest.json`).
    pub endpoint: String,
    #[serde(default)]
    pub resources: Vec<String>,
    #[serde(default)]
    pub types: Vec<String>,
    #[serde(default)]
    pub id_prefixes: Vec<String>,
}

/// A subtitle track from a Stremio addon's `subtitles` resource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StremioSubtitle {
    /// Subtitle id reported by the addon, or the language code when absent.
    pub title: String,
    /// Lowercased language code, `"unknown"` when absent.
    pub lang: String,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkipType {
    Op,
    Ed,
    Recap,
    MixedOp,
    MixedEd,
}

impl SkipType {
    pub const ALL: [SkipType; 5] = [
        SkipType::Op,
        SkipType::Ed,
        SkipType::Recap,
        SkipType::MixedOp,
        SkipType::MixedEd,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SkipSegment {
    #[serde(rename = "type")]
    pub kind: SkipType,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SkipTimesResult {
    pub found: bool,
    pub segments: Vec<SkipSegment>,
}
