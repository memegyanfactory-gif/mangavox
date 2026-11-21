//! A project (one series = one manga) and its episodes (design 5.3, `series.rs`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// `projects/<series_id>/project.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Series {
    pub schema: u32,
    /// `mv_core::paths::safe_name(manga_id)`.
    pub id: String,
    pub manga_id: String,
    pub manga_title: String,
    pub created_at: i64,
    pub episodes: Vec<EpisodeRef>,
    pub settings: ProjectSettings,
    /// Id counters: "scene", "shot", "line", "beat", "place", "prop".
    pub next_ids: BTreeMap<String, u32>,
    /// Gate 1: the bible was approved (hash of what was approved).
    pub bible_approved: Option<Approval>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One episode in the project's list.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EpisodeRef {
    /// "ep01".
    pub id: String,
    pub title: String,
}

/// Per-project choices.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProjectSettings {
    /// "16:9".
    pub aspect: String,
    /// "en".
    pub language: String,
    /// "nano-banana-2-1".
    pub picture_model: String,
    /// "1K".
    pub picture_res: String,
    /// Pictures per Generate.
    pub variants: u8,
    /// `None` = the assistant's model.
    pub planner: Option<ModelPick>,
    /// `None` = the assistant's model.
    pub helper: Option<ModelPick>,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        ProjectSettings {
            aspect: "16:9".into(),
            language: "en".into(),
            picture_model: "nano-banana-2-1".into(),
            picture_res: "1K".into(),
            variants: 2,
            planner: None,
            helper: None,
        }
    }
}

/// A provider + model picked in the engine settings.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ModelPick {
    pub provider: String,
    pub model: String,
}

/// A gate passed by the user.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Approval {
    pub at: i64,
    pub hash: String,
}

/// One manga chapter of an episode. Dirs are stored relative to `mv_core::paths::root()` when
/// they are inside it, absolute otherwise (see `source`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ChapterPick {
    pub chapter_id: String,
    pub manga_id: String,
    pub label: String,
    pub number: Option<f64>,
    pub pages_dir: String,
    pub output_dir: String,
}

/// `episodes/<ep_id>/episode.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Episode {
    pub schema: u32,
    pub id: String,
    pub title: String,
    pub chapters: Vec<ChapterPick>,
    /// Minutes; 0 = auto.
    pub target_minutes: f32,
    pub scene_order: Vec<String>,
    /// Gate 2: the animatic was approved (with its timing hash).
    pub animatic_approved: Option<Approval>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
