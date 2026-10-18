//! Data types shared with the UI. Field names match `src/shared/types.ts` in the Electron app, so
//! serialized values are interchangeable with the existing TypeScript side and the SQLite rows.

use serde::{Deserialize, Serialize};

/// One row of a search / popular / latest / by-tag listing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MangaSearchResult {
    pub source_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt_titles: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MangaDetails {
    pub source_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genres: Option<Vec<String>>,
}

/// One chapter in a manga's chapter list (ascending order, chapter 1 first).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChapterInfo {
    /// Opaque id passed back to `get_page_urls`.
    pub source_id: String,
    /// `None` for oneshots / extras without a number.
    pub number: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub language: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scanlation_group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_count: Option<u32>,
}

/// A browsable tag / genre (`get_tag_list`), usable with `get_manga_by_tag`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Tag {
    pub id: String,
    pub name: String,
}

/// Static description of a source for pickers and settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub lang: String,
    /// Adult-only source (hide unless the user enabled mature content).
    pub nsfw: bool,
    /// Source cannot work without a [`crate::WallSolver`] (browser-backed).
    pub requires_wall_solver: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}
