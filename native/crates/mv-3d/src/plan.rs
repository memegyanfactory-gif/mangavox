//! The episode a chapter becomes: what the story needs (assets), the scenes in order, and for
//! each scene its blocking, render and review state. Saved as `episode.json` in the episode
//! folder after every step, so a run can stop and continue at any time.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::library::Category;
use crate::util::{read_json, write_json};

/// Something the story needs: one location, character, prop ...
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AssetNeed {
    /// Short key the scenes refer to (`gym`, `ganghyeok`).
    pub key: String,
    pub name: String,
    pub category: Category,
    pub description: String,
    pub tags: Vec<String>,
    /// 0-based pages it appears on.
    pub pages: Vec<usize>,
    /// An everyday thing any library may have (a chair, a car), worth looking for online.
    pub generic: bool,
    /// Library asset the AI said this is.
    pub reuse: Option<String>,
    /// The library asset it resolved to, once built or found.
    pub asset_id: Option<String>,
    /// Interior or exterior (locations).
    pub interior: Option<bool>,
    /// Where it sits on the world map, -1..1 (locations).
    pub map_pos: Option<[f32; 2]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SceneStatus {
    #[default]
    Planned,
    /// Building the scene's assets.
    Assets,
    /// The AI is blocking the scene (placements, shots, actions).
    Blocking,
    /// Blender is assembling, making the previz and rendering.
    Rendering,
    /// Rendered; waiting for the user to approve it or ask for a redo.
    Review,
    Approved,
    Failed,
}

impl SceneStatus {
    pub fn label(self) -> &'static str {
        match self {
            SceneStatus::Planned => "Planned",
            SceneStatus::Assets => "Building assets",
            SceneStatus::Blocking => "Blocking",
            SceneStatus::Rendering => "Rendering",
            SceneStatus::Review => "Ready for review",
            SceneStatus::Approved => "Approved",
            SceneStatus::Failed => "Failed",
        }
    }

    pub fn busy(self) -> bool {
        matches!(self, SceneStatus::Assets | SceneStatus::Blocking | SceneStatus::Rendering)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Placement {
    pub actor: String,
    pub pos: [f32; 2],
    /// Degrees, a point `[x, y]` or another actor's key.
    pub facing: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Action {
    pub actor: String,
    #[serde(rename = "type")]
    pub kind: String,
    /// A point `[x, y]`, another actor's key, or null.
    pub target: Value,
    /// Seconds from the start of the shot.
    pub start: f32,
    pub duration: f32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShotLine {
    pub speaker: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shot {
    /// The manga panel this shot stages: (0-based page, 0-based panel).
    pub panel: Option<[usize; 2]>,
    pub description: String,
    /// establishing | wide | full | medium | medium_close | close | extreme_close | two_shot | over_shoulder
    pub framing: String,
    pub subject: String,
    pub secondary: Option<String>,
    /// front | side | back | left | right | low | high | top
    pub angle: String,
    /// static | push_in | pull_out | pan_left | pan_right | orbit | shake
    #[serde(rename = "move")]
    pub camera_move: String,
    pub duration: f32,
    pub actions: Vec<Action>,
    pub line: Option<ShotLine>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Blocking {
    pub placements: Vec<Placement>,
    pub shots: Vec<Shot>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SceneRender {
    /// The scene's clip (absent when ffmpeg is missing; the frames are still there).
    pub clip: Option<PathBuf>,
    /// One previz still per shot.
    pub previz: Vec<PathBuf>,
    /// A few final frames to show without playing the clip.
    pub stills: Vec<PathBuf>,
    pub frames_dir: PathBuf,
    /// The assembled scene, to open in Blender.
    pub blend: PathBuf,
    pub seconds: f32,
    /// Frame range of each shot.
    pub shots: Vec<[u32; 2]>,
    pub rendered_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScenePlan {
    pub index: usize,
    pub title: String,
    /// First and last 0-based page.
    pub pages: [usize; 2],
    /// Key of the location need.
    pub location: String,
    /// day | morning | dawn | dusk | evening | night | interior
    pub time_of_day: String,
    pub mood: String,
    pub characters: Vec<String>,
    pub props: Vec<String>,
    /// What happens, beat by beat.
    pub summary: String,
    pub status: SceneStatus,
    /// What the user asked to change on the last redo.
    pub note: Option<String>,
    pub blocking: Option<Blocking>,
    pub render: Option<SceneRender>,
    pub error: Option<String>,
    /// How many times it was rendered.
    pub takes: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Episode {
    pub manga_id: String,
    pub manga_title: String,
    pub chapter_id: String,
    pub label: String,
    pub title: String,
    pub logline: String,
    pub assets: Vec<AssetNeed>,
    pub scenes: Vec<ScenePlan>,
    /// The model that planned it (`provider/model`).
    pub model: String,
    /// The world map asset of this manga.
    pub world_map: Option<String>,
    /// The joined episode video once every scene is approved.
    pub video: Option<PathBuf>,
    pub created: i64,
    pub updated: i64,
}

impl Episode {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("episode.json")
    }

    pub fn load(dir: &Path) -> Option<Episode> {
        read_json(&Episode::path(dir))
    }

    pub fn save(&mut self, dir: &Path) -> Result<()> {
        self.updated = crate::util::now();
        write_json(&Episode::path(dir), self)
    }

    pub fn need(&self, key: &str) -> Option<&AssetNeed> {
        self.assets.iter().find(|a| a.key == key)
    }

    /// Keys of every asset a scene needs: its location, cast and props.
    pub fn scene_needs(&self, i: usize) -> Vec<String> {
        let Some(s) = self.scenes.get(i) else { return vec![] };
        let mut v = vec![];
        if !s.location.is_empty() {
            v.push(s.location.clone());
        }
        v.extend(s.characters.iter().cloned());
        v.extend(s.props.iter().cloned());
        v.retain(|k| self.need(k).is_some());
        v.dedup();
        v
    }

    pub fn all_approved(&self) -> bool {
        !self.scenes.is_empty() && self.scenes.iter().all(|s| s.status == SceneStatus::Approved)
    }
}
