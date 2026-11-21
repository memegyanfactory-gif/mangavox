//! Cards (anything generated: frames, sheets, plates, voices), what is sent to make them,
//! the takes they produced and the check results (design 5.3, `card.rs`).

use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::enums::{Severity, SiteSlot, Status, TakeKind};
use super::story::SourceRef;

/// A generated picture slot: its takes (history) and the spec that makes the next one.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Card {
    pub status: Status,
    /// Never change automatically.
    pub locked: bool,
    /// Take ids, oldest first. Never shrinks.
    pub takes: Vec<String>,
    pub selected: Option<String>,
    pub gen: GenSpec,
}

/// How the next picture of a card is made.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GenSpec {
    pub model: String,
    pub prompt_ai: String,
    pub prompt_user: Option<String>,
    /// Hash of `prompt_ai` when the user overrode it.
    pub prompt_user_base: Option<String>,
    pub refs: Vec<RefSlot>,
    pub refs_user: bool,
    pub aspect: String,
    pub res: String,
    pub variants: u8,
}

impl GenSpec {
    /// "Reset": the AI's prompt is sent again.
    pub fn reset_prompt(&mut self) {
        self.prompt_user = None;
        self.prompt_user_base = None;
    }

    /// The AI's prompt changed since the user pinned their own (design 5.3: `prompt_user_base`
    /// is the hash of the `prompt_ai` it replaced).
    pub fn prompt_moved(&self) -> bool {
        self.prompt_user.is_some() && self.prompt_user_base.as_deref().is_some_and(|b| b != crate::hash::sha256_hex(self.prompt_ai.as_bytes()))
    }
}

/// One reference picture sent with a request.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RefSlot {
    /// Project-relative.
    pub file: String,
    pub hash: String,
    pub role: RefRole,
    /// "owns the room and the camera".
    pub job: String,
    pub slot: SiteSlot,
    pub pinned: bool,
}

/// Reads an adjacently tagged enum (`{"kind": .., "id": ..}`) through its derived twin `R`; a
/// `kind` this build does not know reads as `unknown`, whatever content it carries. (serde's
/// `#[serde(other)]` cannot do this: it refuses a unit variant that comes with content.) A bare
/// word (`"face"`) is read as `{"kind": "face"}`.
pub(super) fn tagged_or_unknown<'de, D, R, T>(d: D, known: &[&str], unknown: T) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    R: DeserializeOwned + Into<T>,
{
    let mut v = Value::deserialize(d)?;
    if let Value::String(s) = &v {
        v = serde_json::json!({ "kind": s });
    }
    match serde_json::from_value::<R>(v.clone()) {
        Ok(r) => Ok(r.into()),
        Err(e) => match v.get("kind").and_then(Value::as_str) {
            Some(k) if known.contains(&k) => Err(D::Error::custom(e)),
            _ => Ok(unknown),
        },
    }
}

/// What a reference is for.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum RefRole {
    Plate,
    Layout,
    /// Character id.
    Identity(String),
    Style,
    /// Character id.
    Expression(String),
    /// Prop id.
    Prop(String),
    /// Prop id.
    Grip(String),
    /// Character id.
    ColourModel(String),
    PlaceView,
    Panel,
    #[default]
    User,
    /// A role written by a newer MangaVox.
    Unknown,
}

/// [`RefRole`] without `Unknown`, for reading.
#[derive(Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
enum RefRoleRepr {
    Plate,
    Layout,
    Identity(String),
    Style,
    Expression(String),
    Prop(String),
    Grip(String),
    ColourModel(String),
    PlaceView,
    Panel,
    User,
}

impl From<RefRoleRepr> for RefRole {
    fn from(r: RefRoleRepr) -> Self {
        match r {
            RefRoleRepr::Plate => RefRole::Plate,
            RefRoleRepr::Layout => RefRole::Layout,
            RefRoleRepr::Identity(s) => RefRole::Identity(s),
            RefRoleRepr::Style => RefRole::Style,
            RefRoleRepr::Expression(s) => RefRole::Expression(s),
            RefRoleRepr::Prop(s) => RefRole::Prop(s),
            RefRoleRepr::Grip(s) => RefRole::Grip(s),
            RefRoleRepr::ColourModel(s) => RefRole::ColourModel(s),
            RefRoleRepr::PlaceView => RefRole::PlaceView,
            RefRoleRepr::Panel => RefRole::Panel,
            RefRoleRepr::User => RefRole::User,
        }
    }
}

impl<'de> Deserialize<'de> for RefRole {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        const KNOWN: &[&str] = &["plate", "layout", "identity", "style", "expression", "prop", "grip", "colour_model", "place_view", "panel", "user"];
        tagged_or_unknown::<D, RefRoleRepr, RefRole>(d, KNOWN, RefRole::Unknown)
    }
}

/// A voice slot of a line.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct VoiceCard {
    pub status: Status,
    pub locked: bool,
    pub takes: Vec<String>,
    pub selected: Option<String>,
    pub voice_override: Option<String>,
    pub speed: f32,
}

/// Exactly what is sent to the picture model; its hash is the picture channel (design 5.5).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GenRequest {
    /// "nb21.frame@1".
    pub template: String,
    pub model: String,
    pub prompt: String,
    /// (content hash, role label, slot), in upload order.
    pub refs: Vec<(String, String, SiteSlot)>,
    pub aspect: String,
    pub res: String,
}

/// What a take or a job is for.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    ShotFrame {
        episode: String,
        shot: String,
    },
    Sheet {
        /// Character or prop id.
        owner: String,
        sheet: String,
    },
    PlaceSheet {
        place: String,
    },
    Plate {
        place: String,
        station: String,
        state: String,
    },
    StyleFrame,
    LineVoice {
        episode: String,
        line: String,
    },
    Audition {
        character: String,
    },
    Crop {
        owner: String,
    },
    #[default]
    Upload,
    Animatic {
        episode: String,
    },
    /// A target written by a newer MangaVox.
    #[serde(other)]
    Unknown,
}

/// `takes/<aa>/<sha256>.json`: one output file and how it was made.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Take {
    pub schema: u32,
    /// sha256 hex of the file's bytes.
    pub id: String,
    /// Project-relative.
    pub file: String,
    pub kind: TakeKind,
    pub target: Target,
    pub request: Option<GenRequest>,
    /// The channel hash at creation (design 5.5).
    pub request_hash: String,
    pub source: TakeSource,
    pub created_at: i64,
    pub w: u32,
    pub h: u32,
    pub seconds: f32,
    /// Variants of one Generate share it.
    pub batch: Option<String>,
    pub note: String,
}

/// Everything about a new take except what the store derives (id, file, schema, time):
/// the argument of `Store::add_take`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TakeMeta {
    pub kind: TakeKind,
    pub target: Target,
    pub request: Option<GenRequest>,
    pub request_hash: String,
    pub source: TakeSource,
    pub w: u32,
    pub h: u32,
    pub seconds: f32,
    pub batch: Option<String>,
    pub note: String,
}

/// Who made a take.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TakeSource {
    MagnificWeb {
        model: String,
        res: String,
        free: bool,
        site_id: String,
    },
    Voice {
        engine: String,
        voice: String,
    },
    Upload {
        name: String,
    },
    MangaCrop(SourceRef),
    #[default]
    Ffmpeg,
    /// A source written by a newer MangaVox.
    #[serde(other)]
    Unknown,
}

/// One check result (film grammar, story, missing data).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Issue {
    pub code: String,
    pub severity: Severity,
    pub scene: String,
    pub shot: Option<String>,
    pub message: String,
    /// What fixes it, for the revise call and the UI ("Add a wide or overhead shot before cut 5").
    pub fix: Option<String>,
}

/// A compiled picture prompt: the full text and its sections in the fixed order (design 6.6).
/// Lives in the core model (not `compile`) so the Generation panel builds without `frames`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Compiled {
    pub text: String,
    /// (section name, paragraph).
    pub sections: Vec<(String, String)>,
}
