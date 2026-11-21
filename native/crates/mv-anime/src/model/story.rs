//! The episode's script: scenes, beats and lines (design 5.3, `story.rs`).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::card::VoiceCard;
use super::enums::{EmotionKind, LineKind, Pace, SceneKind, TimeOfDay, Tone};

/// A place in the manga: chapter, page index (position in `mv_recap::list_pages`), panel.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SourceRef {
    pub chapter_id: String,
    pub page: usize,
    pub panel: Option<usize>,
}

/// `episodes/<ep_id>/story.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Story {
    pub schema: u32,
    pub episode: String,
    pub scenes: Vec<Scene>,
    pub lines: Vec<Line>,
    pub model: String,
    pub made_at: i64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One scene.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Scene {
    /// "sc0003".
    pub id: String,
    pub title: String,
    /// Place id.
    pub place: Option<String>,
    pub time_of_day: TimeOfDay,
    pub kind: SceneKind,
    pub tone: Tone,
    pub fight: bool,
    pub colour: ColourTheme,
    /// 0-10.
    pub intensity: u8,
    pub target_s: f32,
    pub summary: String,
    pub beats: Vec<Beat>,
    pub source: Vec<SourceRef>,
    pub edited: bool,
    /// JSON pointers the user set, e.g. "/tone".
    pub locked: BTreeSet<String>,
}

/// One beat of a scene.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Beat {
    pub id: String,
    pub purpose: String,
    pub source: Vec<SourceRef>,
    /// Line ids, in order.
    pub lines: Vec<String>,
    pub hook: bool,
}

/// One line of the script.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Line {
    /// "ln0042".
    pub id: String,
    pub scene: String,
    /// Character id; `None` = narrator.
    pub speaker: Option<String>,
    pub kind: LineKind,
    pub original: String,
    pub spoken: String,
    pub keep_words: bool,
    pub emotion: Emotion,
    pub pace: Pace,
    pub source: Option<SourceRef>,
    pub voice: VoiceCard,
    pub edited: bool,
    pub locked: BTreeSet<String>,
}

/// An emotion and how strongly it shows.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Emotion {
    pub kind: EmotionKind,
    /// 1 mild, 2 medium, 3 strong.
    pub strength: u8,
}

/// A scene's colour theme.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ColourTheme {
    /// "deep blue threat".
    pub name: String,
    /// Style palette id.
    pub palette: Option<String>,
}
