//! Shots: what the camera sees, where everyone stands, and the still of each cut (design 5.3,
//! `shot.rs`). One file per scene: `episodes/<ep_id>/shots/<scene_id>.json`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::card::Card;
use super::enums::{
    Angle, BackgroundKind, Depth, Grain, Hand, Key, LightSetup, MotionKind, MotionRoute, MouthPolicy, Move, Origin, Pattern, Pose, ShotSize, Speed, Transition,
    Visibility,
};
use super::story::{Emotion, SourceRef};

/// `episodes/<ep_id>/shots/<scene_id>.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SceneShots {
    pub schema: u32,
    pub episode: String,
    pub scene: String,
    pub shots: Vec<Shot>,
    pub planned_by: String,
    pub revisions: u8,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One cut.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Shot {
    /// "sh0012".
    pub id: String,
    /// The one thing it tells.
    pub tells: String,
    pub beat: Option<String>,
    pub origin: Origin,
    pub source: Vec<SourceRef>,
    pub pattern: Option<Pattern>,
    pub camera: Camera,
    pub marks: Vec<Mark>,
    /// Line ids, in order.
    pub lines: Vec<String>,
    pub mouth: MouthPolicy,
    pub motion: MotionKind,
    pub background: BackgroundKind,
    pub route: MotionRoute,
    pub hit: bool,
    pub flash: bool,
    pub bridge: bool,
    pub transition: Transition,
    pub planned_s: f32,
    /// From voices or the user.
    pub locked_s: Option<f32>,
    /// The still.
    pub frame: Card,
    /// Beta 2 (Animate); always `None` now.
    pub clip: Option<Card>,
    /// The director's last raw suggestion for this shot (for "Reset to Auto").
    pub suggested: Value,
    pub edited: bool,
    pub locked: BTreeSet<String>,
}

/// The camera of a shot.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Camera {
    pub size: ShotSize,
    pub angle: Angle,
    pub station: Option<String>,
    /// `None` = the station's.
    pub pos: Option<[f32; 3]>,
    pub look_at: LookAt,
    pub roll_deg: f32,
    /// 14 | 24 | 35 | 50 | 85 | 135.
    pub lens_mm: u16,
    pub depth: Depth,
    pub focus: Focus,
    pub key: Key,
    pub grain: Grain,
    pub glow: bool,
    pub light: LightSetup,
    pub movement: Movement,
}

/// Where the camera looks.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum LookAt {
    /// Character id.
    Character(String),
    /// Landmark id.
    Landmark(String),
    /// x, height, z in plan metres.
    Point([f32; 3]),
    /// A number saved as `null` (it was not finite) reads as 0, never as a damaged file.
    Angles {
        #[serde(default)]
        yaw_deg: f32,
        #[serde(default)]
        pitch_deg: f32,
    },
}

impl Default for LookAt {
    fn default() -> Self {
        LookAt::Angles { yaw_deg: 0., pitch_deg: 0. }
    }
}

/// Focus and an optional rack to a second target.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Focus {
    pub target: FocusTarget,
    pub rack_to: Option<FocusTarget>,
    pub rack_at_s: f32,
}

/// What is in focus.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum FocusTarget {
    #[default]
    Auto,
    /// Character id.
    Character(String),
    /// Prop id.
    Prop(String),
    Background,
    None,
}

/// The camera's move.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Movement {
    pub kind: Move,
    pub speed: Speed,
    /// 1.0 = usual.
    pub amount: f32,
}

/// Where a character stands in a shot and what they do.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Mark {
    /// Character id.
    pub character: String,
    /// Plan metres (x, z).
    pub pos: [f32; 2],
    pub facing_deg: f32,
    pub pose: Pose,
    pub visible: Visibility,
    pub outfit: Option<String>,
    pub emotion: Emotion,
    pub action: String,
    pub holding: Vec<Hold>,
    /// "char:<id>" | "landmark:<id>" | "camera".
    pub gaze: Option<String>,
}

/// A prop held in a hand.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Hold {
    /// Prop id.
    pub prop: String,
    pub hand: Hand,
}

/// Computed by `geometry::derive`; a cache only, never stored as truth.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Derived {
    pub hfov_deg: f32,
    pub horizon_y: f32,
    pub perspective: String,
    /// Character id -> screen x (0..1).
    pub screen_x: BTreeMap<String, f32>,
    /// "a|b" character pair -> side of the line (-1, 0, 1).
    pub sides: BTreeMap<String, i8>,
    /// Character id -> landmark ids behind them.
    pub behind: BTreeMap<String, Vec<String>>,
    pub station: Option<String>,
}
