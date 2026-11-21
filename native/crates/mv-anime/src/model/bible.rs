//! The series bible: characters, places, props, the style and the voice cast (design 5.3,
//! `bible.rs`). One file per entity under `projects/<id>/bible/`.

use std::collections::BTreeSet;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use super::card::Card;
use super::enums::{CropFraming, LandmarkKind, PlaceKind, Role, Status, ViewAngle};
use super::story::SourceRef;

/// `bible/characters/<char_id>.json` (char_id = the brain id, safe_name'd).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Character {
    pub schema: u32,
    pub id: String,
    pub brain_id: Option<String>,
    pub name: String,
    pub aliases: Vec<String>,
    pub role: Role,
    pub description: String,
    pub age: String,
    pub look: Look,
    pub outfits: Vec<Outfit>,
    pub crops: Vec<RefCrop>,
    pub sheets: Vec<Sheet>,
    pub status: Status,
    pub locked: BTreeSet<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// How a character looks.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Look {
    pub hair: String,
    pub eyes: String,
    pub build: String,
    pub default_outfit: String,
    /// "#RRGGBB".
    pub signature_colour: String,
    /// Must-have features ("scar on left cheek").
    pub keep: Vec<String>,
    pub height_m: f32,
}

/// One outfit and the chapters it is worn in.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Outfit {
    pub id: String,
    pub name: String,
    pub description: String,
    pub chapters: Vec<String>,
}

/// A reference crop cut from a manga page.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RefCrop {
    /// Project-relative take file.
    pub file: String,
    /// Take id.
    pub take: String,
    pub source: SourceRef,
    pub angle: ViewAngle,
    pub framing: CropFraming,
    pub score: f32,
    pub approved: bool,
    /// How many signals picked it (2 = the LLM and the panel listing agree).
    pub agreed: u8,
}

/// One design sheet of a character or prop.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Sheet {
    pub kind: SheetKind,
    pub card: Card,
}

/// Which sheet.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum SheetKind {
    #[default]
    Master,
    Face,
    Turnaround,
    Expressions,
    /// Outfit id.
    Outfit(String),
    /// Prop id.
    Grip(String),
    ColourModel,
    Back,
    /// A kind written by a newer MangaVox (read whatever content it carries).
    Unknown,
}

/// [`SheetKind`] without `Unknown`, for reading.
#[derive(Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
enum SheetKindRepr {
    Master,
    Face,
    Turnaround,
    Expressions,
    Outfit(String),
    Grip(String),
    ColourModel,
    Back,
}

impl From<SheetKindRepr> for SheetKind {
    fn from(r: SheetKindRepr) -> Self {
        match r {
            SheetKindRepr::Master => SheetKind::Master,
            SheetKindRepr::Face => SheetKind::Face,
            SheetKindRepr::Turnaround => SheetKind::Turnaround,
            SheetKindRepr::Expressions => SheetKind::Expressions,
            SheetKindRepr::Outfit(s) => SheetKind::Outfit(s),
            SheetKindRepr::Grip(s) => SheetKind::Grip(s),
            SheetKindRepr::ColourModel => SheetKind::ColourModel,
            SheetKindRepr::Back => SheetKind::Back,
        }
    }
}

impl<'de> Deserialize<'de> for SheetKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        const KNOWN: &[&str] = &["master", "face", "turnaround", "expressions", "outfit", "grip", "colour_model", "back"];
        super::card::tagged_or_unknown::<D, SheetKindRepr, SheetKind>(d, KNOWN, SheetKind::Unknown)
    }
}

/// `bible/places/<place_id>.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Place {
    pub schema: u32,
    /// "pl_rooftop".
    pub id: String,
    pub name: String,
    pub kind: PlaceKind,
    pub description: String,
    pub plan: FloorPlan,
    pub stations: Vec<Station>,
    pub states: Vec<PlaceState>,
    /// Environment master.
    pub sheet: Card,
    pub plates: Vec<Plate>,
    pub crops: Vec<RefCrop>,
    pub status: Status,
    pub locked: BTreeSet<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A place's floor plan in metres: origin = plan centre, x right, z down (south).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FloorPlan {
    pub w_m: f32,
    pub d_m: f32,
    pub walls: Vec<Wall>,
    pub landmarks: Vec<Landmark>,
    pub north_deg: f32,
}

/// One wall segment on the plan.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Wall {
    pub a: [f32; 2],
    pub b: [f32; 2],
    pub h_m: f32,
}

/// A door, window, piece of furniture, ... on the plan.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Landmark {
    pub id: String,
    pub name: String,
    pub kind: LandmarkKind,
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub rot_deg: f32,
    pub h_m: f32,
}

/// A camera station: a fixed place the camera can stand.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Station {
    pub id: String,
    pub name: String,
    /// x, height, z.
    pub pos: [f32; 3],
    /// 0 = north (up), clockwise.
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub lens_mm: u16,
    pub overhead: bool,
}

/// A lighting state of a place ("day", "night").
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PlaceState {
    /// "day".
    pub id: String,
    pub name: String,
    pub key_dir_deg: f32,
    pub kelvin: u32,
    pub mood: String,
}

/// The empty-room picture from one station in one state.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Plate {
    pub station: String,
    pub state: String,
    pub card: Card,
}

/// `bible/props/<prop_id>.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Prop {
    pub schema: u32,
    pub id: String,
    pub name: String,
    pub owner: Option<String>,
    pub description: String,
    pub held: bool,
    pub states: Vec<String>,
    pub sheets: Vec<Sheet>,
    pub crops: Vec<RefCrop>,
    pub status: Status,
    pub locked: BTreeSet<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `bible/style.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Style {
    pub schema: u32,
    /// The paragraph used word for word in every picture prompt.
    pub line: String,
    pub negative: Vec<String>,
    /// The style frame.
    pub frame: Card,
    pub palettes: Vec<Palette>,
    pub status: Status,
    pub locked: BTreeSet<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A colour palette of seven swatches.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Palette {
    pub id: String,
    pub name: String,
    /// 7 swatches.
    pub swatches: Vec<Swatch>,
    pub brightness: f32,
    pub saturation: f32,
}

/// One swatch of a palette.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Swatch {
    /// dominant | shadow | light | accent | skin | mid | deep.
    pub role: String,
    pub hex: String,
}

/// `bible/cast.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct VoiceCast {
    pub schema: u32,
    pub narrator: CharVoice,
    pub voices: Vec<CharVoice>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One character's voice.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CharVoice {
    pub character: String,
    /// `mv_studio::tts::VOICES` id.
    pub voice_id: String,
    /// Project-relative clip for `cb_` cloning.
    pub reference: Option<String>,
    pub speed: f32,
    pub locked: bool,
    /// Take ids.
    pub auditions: Vec<String>,
}

/// The whole bible of a project, loaded at once (`Store::bible`), for the checks, the slot
/// picker and the prompt compiler.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BibleIndex {
    pub characters: Vec<Character>,
    pub places: Vec<Place>,
    pub props: Vec<Prop>,
    pub style: Style,
    pub cast: VoiceCast,
}

impl BibleIndex {
    /// The character with this id.
    pub fn character(&self, id: &str) -> Option<&Character> {
        self.characters.iter().find(|c| c.id == id)
    }

    /// The place with this id.
    pub fn place(&self, id: &str) -> Option<&Place> {
        self.places.iter().find(|p| p.id == id)
    }

    /// The prop with this id.
    pub fn prop(&self, id: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.id == id)
    }
}
