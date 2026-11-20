//! Anime: a chapter of a downloaded manga made into a TV anime the way a studio makes one, with
//! an AI as the director. What the director knows was measured on real episodes and studied from
//! film and anime craft (`native/docs/design/anime.md`).
//!
//! 1. **Read** — the chapter's real pages are cut into panels (`anime_comp.py panels`) and a
//!    vision model reads every panel: who is in it, wearing what, where, what is said.
//! 2. **Cast** ([`board`]) — the characters and places are gathered from those readings, so the
//!    manga's own people, looks and outfits are kept (settei).
//! 3. **Scenes** ([`board`]) — the panels are grouped into scenes, each with a kind (dialogue,
//!    action, dread, narration, flashback, montage), a place, a lighting state and one colour
//!    key, and a length worked out from the measured pacing ([`pacing`]).
//! 4. **Storyboard** (絵コンテ, [`board`]) — scene by scene the director writes the cuts from a
//!    library of shot recipes ([`recipes`]): size, angle, lens, composition, camera move, length,
//!    lines, how much of it is animated, effects, transition; and the background plates each
//!    scene reuses. The board is then checked like an episode director would check it
//!    ([`checks`]) and the director revises what failed ([`director`]).
//! 5. **Animatic** — every cut is played at once from the manga panel it adapts, with its camera
//!    move and voices, so the episode can be watched before anything is drawn.
//! 6. **Assets and cuts** — design sheets, plates and effects are painted; each cut's characters
//!    are drawn on a chroma screen, animated as much as the cut needs, keyed and composited
//!    (撮影) with the camera, light and grain; the cuts are edited into the episode.
//!
//! This module holds the data, the prompts and the checks; `ui-creative` runs the models and the
//! scripts.
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::project::{lenient_text, lenient_u32, CastMember, Line, PanelChar, Project};

pub mod board;
pub mod checks;
pub mod director;
pub mod pacing;
pub mod recipes;

pub use pacing::SCENE_KINDS;

/// A number the AI may send as text ("3.5", "3.5s") or not at all (0).
pub(crate) fn lenient_f32<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<f32, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.) as f32,
        Value::String(s) => {
            let t: String = s.trim().chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
            t.parse().unwrap_or(0.)
        }
        _ => 0.,
    })
}

/// Panel numbers the AI may send as numbers, text or one number.
pub(crate) fn lenient_list<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<usize>, D::Error> {
    let v = Value::deserialize(d)?;
    let one = |x: &Value| match x {
        Value::Number(n) => n.as_f64().filter(|f| *f >= 0.).map(|f| f as usize),
        Value::String(s) => s.trim().trim_start_matches('#').parse().ok(),
        _ => None,
    };
    Ok(match &v {
        Value::Array(a) => a.iter().filter_map(one).collect(),
        x => one(x).into_iter().collect(),
    })
}

/// [x, y] the AI may send as a list, or junk (none).
pub(crate) fn lenient_point<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Option<[f32; 2]>, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(v.as_array().filter(|a| a.len() >= 2).and_then(|a| Some([a[0].as_f64()? as f32, a[1].as_f64()? as f32])))
}

/// A yes / no the AI may send as text or a number.
pub(crate) fn lenient_bool<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<bool, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(match v {
        Value::Bool(b) => b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.),
        Value::String(s) => matches!(s.trim().to_lowercase().as_str(), "true" | "yes" | "y" | "1"),
        _ => false,
    })
}

/// Names the AI may send as a list of text, or as one text with commas.
pub(crate) fn lenient_names<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<String>, D::Error> {
    let v = Value::deserialize(d)?;
    let clean = |s: &str| s.trim().to_string();
    Ok(match &v {
        Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(clean).or_else(|| x.get("name").and_then(Value::as_str).map(clean))).filter(|s| !s.is_empty()).collect(),
        Value::String(s) => s.split([',', ';']).map(clean).filter(|s| !s.is_empty()).collect(),
        _ => vec![],
    })
}

// ------------------------------------------------------------------ data

/// A panel of the manga as the director's assistant read it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SourcePanel {
    /// The panel image, relative to the project (`source/chNNN/pXXX_YY.jpg`).
    pub file: String,
    /// The page it is on (1-based).
    #[serde(deserialize_with = "lenient_u32")]
    pub page: u32,
    /// Its size in pixels (a tall panel is tilted over, a wide one panned).
    pub w: u32,
    pub h: u32,
    #[serde(deserialize_with = "lenient_text")]
    pub description: String,
    pub characters: Vec<PanelChar>,
    #[serde(deserialize_with = "lenient_text")]
    pub location: String,
    pub lines: Vec<Line>,
    #[serde(deserialize_with = "lenient_text")]
    pub sfx: String,
    #[serde(deserialize_with = "lenient_text")]
    pub mood: String,
    /// Not story (a credits page, an ad, a translator's note, a blank): left out of the board.
    pub skip: bool,
    /// The panel has been read by the vision model.
    pub read: bool,
}

impl SourcePanel {
    /// Story the board adapts.
    pub fn story(&self) -> bool {
        self.read && !self.skip
    }
}

/// A background plate: one painting of a place from one view in one light.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Plate {
    #[serde(deserialize_with = "lenient_text")]
    pub id: String,
    #[serde(deserialize_with = "lenient_text")]
    pub location: String,
    /// What the camera sees ("the long stone corridor from the gate, looking in").
    #[serde(deserialize_with = "lenient_text")]
    pub view: String,
    /// Time of day and light ("cold blue torchlight from the left").
    #[serde(deserialize_with = "lenient_text")]
    pub light: String,
    pub image: Option<String>,
}

/// An effects element of the bank: painted (and, for a video model, animated) on black, or
/// on a chroma screen when it is opaque (thick smoke, rubble).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FxElement {
    #[serde(deserialize_with = "lenient_text")]
    pub id: String,
    #[serde(deserialize_with = "lenient_text")]
    pub what: String,
    /// "screen" (light: fire, aura, sparks, magic) or "over" (opaque: smoke, debris; keyed).
    #[serde(deserialize_with = "lenient_text")]
    pub blend: String,
    pub image: Option<String>,
    pub clip: Option<String>,
}

/// A prop characters rest on (a bench, a crate, a plank stack). As in TV anime: designed once on a
/// prop sheet, left out of the background plate, and drawn on the characters' cel layer together
/// with them in every cut of that plate, so whoever sits on it touches it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Prop {
    #[serde(deserialize_with = "lenient_text")]
    pub id: String,
    /// "plank stack".
    #[serde(deserialize_with = "lenient_text")]
    pub what: String,
    /// The plate it belongs to (and is left out of).
    #[serde(deserialize_with = "lenient_text")]
    pub plate: String,
    /// Its prop sheet.
    pub image: Option<String>,
}

/// The props the boards' cuts rest on, one per plate and thing: new ones added, unused undrawn
/// ones dropped. Returns whether anything changed.
pub fn sync_props(p: &mut Project) -> bool {
    let mut want: Vec<(String, String)> = vec![];
    for c in p.chapters.iter().flat_map(|ch| ch.anime.cuts.iter()) {
        if let Some(what) = c.touched().filter(|_| c.has_cast() && !c.plate.is_empty()) {
            if !want.iter().any(|(pl, w)| *pl == c.plate && *w == what) {
                want.push((c.plate.clone(), what));
            }
        }
    }
    let before = p.props.clone();
    p.props.retain(|x| x.image.is_some() || want.iter().any(|(pl, w)| *pl == x.plate && *w == x.what));
    for (plate, what) in want {
        if !p.props.iter().any(|x| x.plate == plate && x.what == what) {
            // A plate painted before it was known shows it: painted again without it.
            if let Some(pl) = p.plates.iter_mut().find(|x| x.id == plate) {
                pl.image = None;
            }
            p.props.push(Prop { id: crate::project::slug(&format!("{plate} {what}")), what, plate, image: None });
        }
    }
    p.props != before
}

/// A prop's design sheet: the one object, on a plain light background, from a few sides.
pub fn prop_sheet(p: &Project, x: &Prop, refs: &[String]) -> String {
    let place = p.plates.iter().find(|pl| pl.id == x.plate);
    let mut s = format!(
        "Anime prop design sheet (設定) for a TV anime: {} {}. The single object only, drawn three times on a plain flat light-grey background: front view, three-quarter view and side view, the same size and scale, clean line art and flat cel colours, {}. No people, no hands, no ground scenery, no text, no labels.",
        x.what,
        place.map(|pl| format!("from {}", pl.location)).unwrap_or_default(),
        ANIME_LOOK,
    );
    if let Some(pl) = place {
        let _ = write!(s, " Its light and wear match the place: {}.", pl.light);
    }
    if !refs.is_empty() {
        let _ = write!(s, "\nReference images: {}. Keep the manga's design of it.", refs.join("; "));
    }
    s
}

/// An effect placed in a cut.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FxUse {
    #[serde(deserialize_with = "lenient_text")]
    pub id: String,
    #[serde(deserialize_with = "lenient_f32")]
    pub x: f32,
    #[serde(deserialize_with = "lenient_f32")]
    pub y: f32,
    #[serde(deserialize_with = "lenient_f32")]
    pub scale: f32,
    /// In front of the characters (else behind them, over the plate).
    pub front: bool,
    #[serde(deserialize_with = "lenient_f32")]
    pub opacity: f32,
}

impl Default for FxUse {
    fn default() -> Self {
        FxUse { id: String::new(), x: 0.5, y: 0.5, scale: 0.6, front: true, opacity: 1.0 }
    }
}

/// A scene: a stretch of the chapter in one place, time and lighting state with one dramatic
/// purpose. It sets the pacing, the colour key and how much is animated for all its cuts.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Scene {
    #[serde(deserialize_with = "lenient_u32")]
    pub number: u32,
    #[serde(deserialize_with = "lenient_text")]
    pub title: String,
    /// One of [`SCENE_KINDS`].
    #[serde(deserialize_with = "lenient_text")]
    pub kind: String,
    #[serde(deserialize_with = "lenient_text")]
    pub location: String,
    /// The lighting state ("overcast noon", "blue torchlight; then the altar's fire").
    #[serde(deserialize_with = "lenient_text")]
    pub light: String,
    /// The colour key, one of [`GRADES`].
    #[serde(deserialize_with = "lenient_text")]
    pub grade: String,
    /// The panels it adapts: indices into the chapter's panels, `from` to `to` (exclusive).
    pub from: usize,
    pub to: usize,
    #[serde(deserialize_with = "lenient_text")]
    pub summary: String,
    #[serde(deserialize_with = "lenient_names")]
    pub characters: Vec<String>,
    /// How it is told: what is kept, cut and added.
    #[serde(deserialize_with = "lenient_text")]
    pub plan: String,
    /// The music under it ("none", "low dread drone", "driving battle theme") and the room tone.
    #[serde(deserialize_with = "lenient_text")]
    pub music: String,
    #[serde(deserialize_with = "lenient_text")]
    pub ambience: String,
    /// What it should run to, from the measured pacing.
    #[serde(deserialize_with = "lenient_f32")]
    pub seconds: f32,
    /// The staging map, written before any cut: where everyone and everything stands in the
    /// place (and where they move), the objective or threat, which way things travel, and the
    /// side of the line the camera stays on. Every cut is framed from it.
    #[serde(deserialize_with = "lenient_text")]
    pub blocking: String,
}

/// Shot sizes, widest first.
pub const SIZES: [&str; 14] = ["establishing", "extreme long", "long", "group", "full", "cowboy", "two-shot", "medium", "over-the-shoulder", "medium close-up", "close-up", "extreme close-up", "insert", "pov"];
/// Camera heights and angles (any may also be tilted: "low dutch").
pub const ANGLES: [&str; 7] = ["eye level", "low", "high", "bird's eye", "worm's eye", "ground level", "dutch"];
/// Lens feel.
pub const LENSES: [&str; 4] = ["wide", "normal", "tele", "fisheye"];
/// Camera moves the compositor knows.
pub const MOVES: [&str; 10] = ["fix", "push_in", "track_in", "pull_out", "pan_left", "pan_right", "tilt_up", "tilt_down", "whip", "shake"];
/// How much of a cut is animated, cheapest first: a frozen drawing; a still drawing under a
/// moving camera; mouth flaps and blinks; two or three key poses; continuous animation.
pub const MOTIONS: [&str; 5] = ["hold", "camera", "lip", "pose", "full"];
/// The colour script's grades (anime_comp.py GRADES).
pub const GRADES: [&str; 11] = ["neutral", "daylight", "warm_home", "sunset", "night", "hospital", "cold_dungeon", "system_blue", "blood_red", "horror", "flashback"];
/// Transitions the edit knows.
pub const TRANSITIONS: [&str; 5] = ["cut", "dissolve", "black", "white", "smash"];

fn squash(s: &str) -> String {
    s.trim().to_lowercase().replace(['-', '_'], " ")
}

/// A shot size in our words ("ECU", "wide shot", "over the shoulder" are understood).
pub fn norm_size(s: &str) -> &'static str {
    let t = squash(s);
    let has = |w: &str| t.contains(w);
    let word = |w: &str| t.split(|c: char| !c.is_alphanumeric()).any(|x| x == w);
    if has("establish") {
        "establishing"
    } else if has("extreme close") || word("ecu") || word("xcu") || has("big close") {
        "extreme close-up"
    } else if has("medium close") || word("mcu") {
        "medium close-up"
    } else if has("extreme long") || has("extreme wide") || word("els") || word("ews") {
        "extreme long"
    } else if has("over the shoulder") || has("over shoulder") || word("ots") {
        "over-the-shoulder"
    } else if has("two shot") || has("2 shot") || has("twoshot") {
        "two-shot"
    } else if has("insert") || has("detail") || has("cutaway") {
        "insert"
    } else if has("close") || word("cu") {
        "close-up"
    } else if has("group") || has("crowd") {
        "group"
    } else if has("cowboy") || has("american") || has("knee") {
        "cowboy"
    } else if has("pov") || has("point of view") {
        "pov"
    } else if has("full") || word("fs") {
        "full"
    } else if has("long") || has("wide") || word("ls") || word("ws") {
        "long"
    } else {
        "medium"
    }
}

/// How wide a size is, 0 the widest: two cuts of one subject need different ranks (or angles).
pub fn size_rank(size: &str) -> u8 {
    match size {
        "establishing" | "extreme long" => 0,
        "long" | "group" => 1,
        "full" => 2,
        "cowboy" | "two-shot" => 3,
        "medium" | "over-the-shoulder" | "pov" => 4,
        "medium close-up" => 5,
        "close-up" => 6,
        "extreme close-up" => 7,
        _ => 8,
    }
}

/// A camera angle in our words; a tilted frame keeps its height ("low dutch").
pub fn norm_angle(s: &str) -> String {
    let t = squash(s);
    let has = |w: &str| t.contains(w);
    let base = if has("bird") || has("top") || has("overhead") || has("directly above") {
        "bird's eye"
    } else if has("worm") {
        "worm's eye"
    } else if has("ground") || has("floor level") {
        "ground level"
    } else if has("low") || has("from below") {
        "low"
    } else if has("high") || has("from above") {
        "high"
    } else {
        "eye level"
    };
    let dutch = has("dutch") || has("canted") || has("tilted") || has("oblique");
    match (base, dutch) {
        ("eye level", true) => "dutch".into(),
        (b, true) => format!("{b} dutch"),
        (b, false) => b.into(),
    }
}

pub fn norm_lens(s: &str) -> &'static str {
    let t = squash(s);
    if t.contains("fish") {
        "fisheye"
    } else if t.contains("tele") || t.contains("long lens") || t.contains("compress") {
        "tele"
    } else if t.contains("wide") {
        "wide"
    } else {
        "normal"
    }
}

/// A camera move in the compositor's words ("slow push", "T.U", "pan up" are understood).
pub fn norm_camera(s: &str) -> &'static str {
    let t = squash(s).replace('.', "");
    let has = |w: &str| t.contains(w);
    let word = |w: &str| t.split_whitespace().any(|x| x == w);
    if let Some(m) = MOVES.iter().find(|m| m.replace('_', " ") == t) {
        return m;
    }
    if has("whip") || has("swish") {
        "whip"
    } else if has("shake") || has("handheld") || has("hand held") {
        "shake"
    } else if has("track in") || has("crash") || has("qtu") || has("rush in") || has("dolly zoom") {
        "track_in"
    } else if has("tilt up") || has("pan up") || has("crane up") || has("boom up") {
        "tilt_up"
    } else if has("tilt down") || has("pan down") || has("crane down") || has("boom down") {
        "tilt_down"
    } else if has("tilt") {
        "tilt_up"
    } else if has("pan left") || has("truck left") || has("track left") {
        "pan_left"
    } else if has("pan") || has("truck") || has("follow") || has("track right") {
        "pan_right"
    } else if has("pull") || word("out") || word("back") || t == "tb" {
        "pull_out"
    } else if has("push") || has("zoom") || has("dolly") || word("in") || t == "tu" {
        "push_in"
    } else {
        "fix"
    }
}

/// How much is animated, in our words; without a word, by the camera.
pub fn norm_motion(s: &str, camera: &str) -> &'static str {
    let t = squash(s);
    let has = |w: &str| t.contains(w);
    if has("full") || has("sakuga") || has("continuous") {
        "full"
    } else if has("pose") || has("limited") || has("layered") {
        "pose"
    } else if has("lip") || has("mouth") || has("flap") || has("blink") {
        "lip"
    } else if has("camera") {
        "camera"
    } else if has("hold") || has("still") || has("frozen") || has("freeze") || has("tome") {
        if camera == "fix" {
            "hold"
        } else {
            "camera"
        }
    } else if camera == "fix" {
        "hold"
    } else {
        "camera"
    }
}

/// A grade from the list; anything else is neutral.
pub fn norm_grade(s: &str) -> &'static str {
    let t = s.trim().to_lowercase().replace([' ', '-'], "_");
    GRADES.iter().find(|g| **g == t).copied().unwrap_or("neutral")
}

pub fn norm_transition(s: &str) -> &'static str {
    let t = squash(s).replace('.', "");
    if t.contains("dissolve") || t.contains("overlap") || t == "ol" || t.contains("cross") {
        "dissolve"
    } else if t.contains("black") || t.contains("fade out") || t.contains("fade in") {
        "black"
    } else if t.contains("white") {
        "white"
    } else if t.contains("smash") {
        "smash"
    } else {
        "cut"
    }
}

/// A scene kind from the list; anything else is dialogue.
pub fn norm_kind(s: &str) -> &'static str {
    let t = squash(s);
    if t.contains("fight") || t.contains("chase") || t.contains("battle") {
        return "action";
    }
    if t.contains("horror") || t.contains("suspense") || t.contains("tension") {
        return "dread";
    }
    if t.contains("memory") {
        return "flashback";
    }
    if t.contains("exposition") {
        return "narration";
    }
    SCENE_KINDS.iter().find(|k| t.contains(**k)).copied().unwrap_or("dialogue")
}

/// One cut of the storyboard and what has been made for it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Cut {
    #[serde(deserialize_with = "lenient_u32")]
    pub number: u32,
    /// The scene it belongs to (its number).
    #[serde(deserialize_with = "lenient_u32")]
    pub scene: u32,
    /// The shot recipe it follows ([`recipes::RECIPES`]), when it follows one.
    #[serde(deserialize_with = "lenient_text")]
    pub recipe: String,
    #[serde(deserialize_with = "lenient_f32")]
    pub seconds: f32,
    /// The source panels (indices into the chapter's panels, 0-based) this cut adapts; empty
    /// for a cut the director added (an establishing shot, a reaction).
    #[serde(deserialize_with = "lenient_list")]
    pub from: Vec<usize>,
    /// One of [`SIZES`].
    #[serde(alias = "shot", deserialize_with = "lenient_text")]
    pub size: String,
    /// From [`ANGLES`] ("low", "high dutch").
    #[serde(deserialize_with = "lenient_text")]
    pub angle: String,
    /// One of [`LENSES`].
    #[serde(deserialize_with = "lenient_text")]
    pub lens: String,
    /// One of [`MOVES`].
    #[serde(deserialize_with = "lenient_text")]
    pub camera: String,
    /// How far the move goes against the usual for its scene (1; 2 is twice as far).
    #[serde(deserialize_with = "lenient_f32")]
    pub amount: f32,
    #[serde(deserialize_with = "lenient_f32")]
    pub shake: f32,
    /// How the frame is built: where the subject sits, what is in front, the depth, the light.
    #[serde(deserialize_with = "lenient_text")]
    pub composition: String,
    /// What we see: who is where, their acting.
    #[serde(deserialize_with = "lenient_text")]
    pub picture: String,
    /// What moves over the cut.
    #[serde(deserialize_with = "lenient_text")]
    pub action: String,
    /// One of [`MOTIONS`]: how much of it is animated.
    #[serde(deserialize_with = "lenient_text")]
    pub motion: String,
    pub characters: Vec<PanelChar>,
    #[serde(deserialize_with = "lenient_text")]
    pub location: String,
    /// The plate id it sits on.
    #[serde(deserialize_with = "lenient_text")]
    pub plate: String,
    /// The plate alone is the picture (an establishing shot of an empty place).
    #[serde(deserialize_with = "lenient_bool")]
    pub plate_only: bool,
    pub fx: Vec<FxUse>,
    pub lines: Vec<Line>,
    /// A speaker's mouth is seen moving (else the lines are heard over backs, a wide shot, the
    /// listener: no mouth to animate).
    #[serde(deserialize_with = "lenient_bool")]
    pub mouth: bool,
    #[serde(deserialize_with = "lenient_text")]
    pub sfx: String,
    /// One of [`TRANSITIONS`] (into this cut).
    #[serde(deserialize_with = "lenient_text")]
    pub transition: String,
    /// One of [`GRADES`]: the scene's, unless this cut is a memory or another world.
    #[serde(deserialize_with = "lenient_text")]
    pub grade: String,
    /// Photography extras: floating dust, light rays from (x, y), speed lines, an impact flash.
    #[serde(deserialize_with = "lenient_u32")]
    pub dust: u32,
    #[serde(deserialize_with = "lenient_point")]
    pub rays: Option<[f32; 2]>,
    #[serde(deserialize_with = "lenient_f32")]
    pub speed: f32,
    #[serde(deserialize_with = "lenient_f32")]
    pub flash: f32,
    /// The user's notes for this cut (sent with every prompt when it is made again).
    #[serde(deserialize_with = "lenient_text")]
    pub notes: String,
    // What was made (relative paths); never taken from the AI.
    /// The animatic's version: the manga panel under the cut's camera move.
    pub rough: Option<String>,
    /// The characters drawn on the chroma screen (lifted from `layout`).
    pub layer: Option<String>,
    /// The cut's own background: the place painted from this cut's camera (its layout BG).
    pub bg: Option<String>,
    /// The layout: the characters and their props drawn into `bg`, lit and shadowed by it.
    pub layout: Option<String>,
    /// The composited key frame (plate + characters) a `full` cut is animated from.
    pub key: Option<String>,
    /// The animation: the character layer animated (`layered`) or the whole frame (`full`).
    pub anim: Option<String>,
    pub voice: Option<String>,
    /// The finished cut.
    pub comp: Option<String>,
}

/// Words that put someone's weight on something.
const CONTACT: &[&str] = &["sit", "sits", "sitting", "seated", "sat", "lean", "leans", "leaning", "lying", "lies", "lie", "kneel", "kneels", "kneeling", "knelt", "perched", "slumped", "slumps", "sprawled", "propped", "straddling", "crouched", "squatting", "resting", "rests"];

fn words(t: &str) -> Vec<String> {
    t.to_lowercase().split(|ch: char| !ch.is_alphabetic() && ch != '\'').filter(|w| !w.is_empty()).map(str::to_string).collect()
}

impl Cut {
    /// How the cut is produced, from how much of it is animated: a drawing under the camera
    /// ("still"), characters animated on their own layer ("layered") or the whole frame
    /// animated ("full").
    pub fn render_kind(&self) -> &'static str {
        match self.motion.as_str() {
            "full" => "full",
            "lip" | "pose" => "layered",
            _ => "still",
        }
    }

    pub fn has_cast(&self) -> bool {
        !self.plate_only && self.characters.iter().any(|c| !c.name.trim().is_empty())
    }

    /// Someone in the cut sits, leans, lies or kneels on something in the place.
    pub fn touches_set(&self) -> bool {
        self.touched().is_some() || self.texts().iter().any(|t| words(t).iter().any(|w| CONTACT.contains(&w.as_str())))
    }

    /// What someone in the cut rests on ("plank stack" from "sitting on the plank stack, elbows on
    /// knees"), when the board names it. Drawn alone on the screen and laid over a plate that has
    /// it, they would float in front of it (the first painted test sat two characters in the air
    /// before the planks): so it is left out of the plate and drawn on their cel layer with them.
    pub fn touched(&self) -> Option<String> {
        // ("in" is left out: "leaning in toward him", "sitting back in surprise".)
        const ON: &[&str] = &["on", "onto", "against", "atop", "upon", "across"];
        const SKIP: &[&str] = &["the", "a", "an", "his", "her", "their", "its", "some", "top", "of", "edge", "end", "side", "corner"];
        // Not a prop: a body part, a pronoun, a feeling, or the place itself (walls, steps, a
        // counter, a truck stay in the background, as anime keeps them).
        const NOT: &[&str] = &[
            "ground", "floor", "knees", "knee", "lap", "hands", "hand", "back", "side", "front", "shock", "silence", "thought", "despair", "pain", "air", "seiza", "place", "hip", "hips", "shoulder", "shoulders", "arm", "arms", "elbow", "elbows", "chest", "head", "legs", "leg", "heels", "feet", "foot", "it", "him", "her", "them", "me", "us", "toward", "towards", "surprise", "wall", "walls", "building", "truck", "car", "bus", "van", "counter", "door", "doorway", "window", "pillar", "railing", "fence", "gate", "stairs", "steps", "step", "road", "street", "tree", "altar", "throne",
        ];
        for t in self.texts() {
            let w = words(&t);
            for (i, x) in w.iter().enumerate() {
                if !CONTACT.contains(&x.as_str()) {
                    continue;
                }
                let Some(at) = w.iter().skip(i + 1).take(3).position(|y| ON.contains(&y.as_str())).map(|k| i + 2 + k) else { continue };
                let mut thing: Vec<&str> = vec![];
                for y in w.iter().skip(at) {
                    if thing.is_empty() && SKIP.contains(&y.as_str()) {
                        continue;
                    }
                    if ["with", "and", "while", "as", "looking", "facing", "his", "her", "their", "near", "next", "beside", "behind"].contains(&y.as_str()) || thing.len() == 3 {
                        break;
                    }
                    thing.push(y);
                }
                let thing = thing.join(" ");
                // Its last word names the thing ("plank stack", "rusty bench").
                if !thing.is_empty() && !thing.split(' ').any(|x| NOT.contains(&x)) {
                    return Some(thing);
                }
            }
        }
        None
    }

    /// The board's words about the cut's people and picture, clause by clause.
    fn texts(&self) -> Vec<String> {
        self.characters.iter().map(|x| x.pose.clone()).chain([self.action.clone(), self.picture.clone(), self.composition.clone()]).flat_map(|t| t.split([',', '.', ';', '(', ')']).map(str::to_string).collect::<Vec<_>>()).filter(|t| !t.trim().is_empty()).collect()
    }

    /// The cut has a line said aloud (not a thought, not narration).
    pub fn spoken(&self) -> bool {
        self.lines.iter().any(|l| !l.text.trim().is_empty() && said_aloud(l))
    }

    /// Drops everything made for the cut (a new board or a redo).
    pub fn clear_made(&mut self) {
        self.rough = None;
        self.layer = None;
        self.key = None;
        self.anim = None;
        self.voice = None;
        self.comp = None;
    }
}

/// A line a mouth moves for: said aloud by a character.
pub fn said_aloud(l: &Line) -> bool {
    let who = l.speaker.trim().to_lowercase();
    !matches!(l.kind.trim().to_lowercase().as_str(), "thought" | "narration" | "inner" | "monologue") && !matches!(who.as_str(), "narrator" | "system" | "")
}

/// Something the episode director's check found in the board.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Issue {
    /// A short code ("jump-cut", "axis", "scene-pace").
    pub code: String,
    /// "error" (breaks film grammar or cannot be made) or "warning" (taste, pacing, cost).
    pub severity: String,
    /// The scene's number (0: the whole chapter).
    pub scene: u32,
    /// The cut's number (0: the whole scene).
    pub cut: u32,
    /// What is wrong and what to do, written for the director.
    pub message: String,
}

/// A chapter's anime.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AnimeChapter {
    pub panels: Vec<SourcePanel>,
    /// The cast and places have been gathered from these panels.
    pub cast_done: bool,
    /// The director's treatment: how the chapter is told as an episode.
    #[serde(deserialize_with = "lenient_text")]
    pub treatment: String,
    pub scenes: Vec<Scene>,
    pub cuts: Vec<Cut>,
    /// What the last check of the board still found.
    pub issues: Vec<Issue>,
    /// The animatic: the episode from the manga panels, before anything is drawn.
    pub animatic: Option<String>,
    /// The user watched the animatic and said to draw it.
    pub approved: bool,
    pub video: Option<String>,
}

impl AnimeChapter {
    pub fn read(&self) -> bool {
        !self.panels.is_empty() && self.panels.iter().all(|p| p.read)
    }

    pub fn seconds(&self) -> f32 {
        self.cuts.iter().map(|c| c.seconds.max(0.)).sum::<f32>() + 0.
    }

    pub fn scene(&self, number: u32) -> Option<&Scene> {
        self.scenes.iter().find(|s| s.number == number)
    }

    /// The kind of the scene a cut is in (dialogue when it has none).
    pub fn kind_of(&self, c: &Cut) -> &str {
        self.scene(c.scene).map(|s| s.kind.as_str()).filter(|k| !k.is_empty()).unwrap_or("dialogue")
    }

    /// Every scene has its cuts.
    pub fn boarded(&self) -> bool {
        !self.scenes.is_empty() && self.scenes.iter().all(|s| self.cuts.iter().any(|c| c.scene == s.number))
    }

    /// Gives the cuts their numbers again, in scene order.
    pub fn renumber(&mut self) {
        let order: Vec<u32> = self.scenes.iter().map(|s| s.number).collect();
        self.cuts.sort_by_key(|c| order.iter().position(|n| *n == c.scene).unwrap_or(usize::MAX));
        for (i, c) in self.cuts.iter_mut().enumerate() {
            c.number = i as u32 + 1;
        }
    }
}

// ------------------------------------------------------------------ look

/// What every anime picture is drawn as.
pub const ANIME_LOOK: &str = "a frame of a modern Japanese TV anime (2D cel animation, as by a top studio): clean confident lineart of even weight, flat cel shading with one crisp shadow tone and a sharp highlight, expressive anime faces and eyes, painted anime background art, modern digital anime photography (soft glow, light diffusion, gentle bloom on lights); NOT 3D render, NOT photorealistic, NOT a manga page, no screentone, no halftone";

/// The chroma screen a character is drawn on: magenta when green is part of their look (it
/// would be keyed out), else green.
pub fn screen_for(cast: &[&CastMember]) -> (&'static str, &'static str) {
    let green = cast.iter().any(|c| {
        let t = format!("{} {} {}", c.appearance, c.signature, c.outfits.iter().map(|o| o.look.as_str()).collect::<Vec<_>>().join(" ")).to_lowercase();
        t.contains("green") || t.contains("emerald") || t.contains("jade") || t.contains("lime") || t.contains("teal")
    });
    if green {
        ("magenta", "#FF00FF")
    } else {
        ("chroma green", "#00B140")
    }
}

fn look_of(c: &CastMember, outfit: &str) -> String {
    let o = c.outfits.iter().find(|o| o.name.eq_ignore_ascii_case(outfit.trim())).or(c.outfits.first());
    let mut s = format!("{}: {}", c.name, c.appearance);
    if let Some(o) = o {
        let _ = write!(s, "; wearing {}", o.look);
    } else if !outfit.trim().is_empty() {
        // An outfit the sheet does not list: the manga's own words for it.
        let _ = write!(s, "; wearing {}", outfit.trim());
    }
    if !c.signature.is_empty() {
        let _ = write!(s, "; {}", c.signature);
    }
    s
}

/// Ready-made words for each camera move, for the video model.
pub fn move_words(m: &str) -> &'static str {
    match m {
        "push_in" => "the camera slowly pushes in",
        "track_in" => "the camera tracks in fast toward the subject",
        "pull_out" => "the camera slowly pulls back",
        "pan_left" => "the camera pans left",
        "pan_right" => "the camera pans right",
        "tilt_up" => "the camera tilts up",
        "tilt_down" => "the camera tilts down",
        "whip" => "a fast whip pan that settles on the subject",
        "shake" => "the camera shakes with the impact",
        _ => "locked-off camera, no camera movement",
    }
}

/// What a lens does to a drawing, for the artist.
pub fn lens_words(l: &str) -> &'static str {
    match l {
        "wide" => "wide-angle lens: strong perspective, what is near the lens large, deep space, converging lines",
        "tele" => "long lens: flattened depth, the background close and soft, the subject isolated",
        "fisheye" => "fisheye lens: bulging distortion, curved lines",
        _ => "natural perspective",
    }
}

/// The shot in one line for the artist: size, angle, lens and how the frame is built.
fn shot_line(c: &Cut) -> String {
    let mut s = format!("{} shot, {} angle, {}.", if c.size.is_empty() { "medium" } else { &c.size }, if c.angle.is_empty() { "eye level" } else { &c.angle }, lens_words(&c.lens));
    let comp = if c.composition.trim().is_empty() { recipes::recipe(&c.recipe).map(|r| r.composition.to_string()).unwrap_or_default() } else { c.composition.trim().to_string() };
    if !comp.is_empty() {
        let _ = write!(s, " Composition: {}.", comp.trim_end_matches('.'));
    }
    s
}

// ------------------------------------------------------------------ 1. read

/// Panels per vision call.
pub const READ_BATCH: usize = 6;

/// The vision prompt for a batch of panels (attached in order). `first` is the index of the
/// first one in the chapter.
pub fn read_panels(p: &Project, chapter: u32, first: usize, count: usize, earlier: &str) -> (String, String) {
    let system = "You are the assistant director of a TV anime, reading the manga it adapts panel by panel so the director can storyboard it. You are shown panels cut from the real pages, in reading order. For EACH image say exactly what is in it. Answer with ONE JSON object, no other text:\n{\"panels\": [{\"index\": 0, \"skip\": false, \"description\": \"what is drawn: framing (close-up, wide, seen from above...), who is where, their pose, acting and expression, the setting, the light and colours, any action or effect\", \"characters\": [{\"name\": \"cast name\", \"expression\": \"\", \"outfit\": \"what they wear here, with colours\", \"pose\": \"\"}], \"location\": \"where this is\", \"lines\": [{\"speaker\": \"cast name, or Narrator\", \"text\": \"the words in the balloon, in English\", \"kind\": \"speech|thought|shout|whisper|narration\"}], \"sfx\": \"sound effects drawn, as sounds\", \"mood\": \"the feeling of the beat\"}]}\nRules: one entry per image, index = its position in this batch from 0. Name characters with the cast names given when you recognise them (by face, hair, outfit, or the names in the text). Someone not in the cast gets the name the text calls them, else a short fixed handle from their look (\"Scarred hunter\", \"Girl in red coat\") that you use again every time they appear. Transcribe every balloon and caption in reading order, translated to natural English if it is not English; a spiky or dark balloon of thought, or text with no tail, is kind \"thought\"; a caption box is \"narration\" by \"Narrator\"; a system window or screen text is \"narration\" by \"System\". A panel that is only a balloon or a sound effect still gets its lines. skip = true for anything that is not story (a series logo or title card, credits, an ad, a translator's note, a blank or black panel with no text, a 'to be continued').".to_string();
    let mut user = format!("Manga: {}. Chapter {chapter}. Panels {} to {} of the chapter are attached in order.\n", p.adapts.as_deref().unwrap_or(&p.title), first + 1, first + count);
    if !p.cast.is_empty() {
        user.push_str("\nKnown cast (name: look):\n");
        for c in p.cast.iter().take(40) {
            let _ = writeln!(user, "- {}: {}", c.name, c.appearance.chars().take(160).collect::<String>());
        }
    }
    if !p.locations.is_empty() {
        let _ = write!(user, "\nKnown places: {}\n", p.locations.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(", "));
    }
    if let Some(ch) = p.chapter(chapter).filter(|c| !c.summary.trim().is_empty()) {
        let _ = write!(user, "\nWhat happens in this chapter: {}\n", ch.summary.trim());
    }
    if !earlier.is_empty() {
        let _ = write!(user, "\nThe panels just before these (keep the same names and handles):\n{earlier}\n");
    }
    (system, user)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ReadAnswer {
    panels: Vec<ReadPanel>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ReadPanel {
    #[serde(deserialize_with = "lenient_u32")]
    index: u32,
    #[serde(deserialize_with = "lenient_bool")]
    skip: bool,
    #[serde(deserialize_with = "lenient_text")]
    description: String,
    characters: Vec<PanelChar>,
    #[serde(deserialize_with = "lenient_text")]
    location: String,
    lines: Vec<Line>,
    #[serde(deserialize_with = "lenient_text")]
    sfx: String,
    #[serde(deserialize_with = "lenient_text")]
    mood: String,
}

/// Stores a vision answer into `panels[first..first + count]`. Every panel of the batch counts
/// as read (one the answer left out stays without a description, and the board skips it).
pub fn apply_read(p: &Project, panels: &mut [SourcePanel], first: usize, count: usize, v: Value) -> anyhow::Result<usize> {
    let a: ReadAnswer = serde_json::from_value(v)?;
    if a.panels.is_empty() {
        anyhow::bail!("the AI described no panels");
    }
    let mut got = 0;
    for (k, r) in a.panels.into_iter().enumerate() {
        // The index the AI gave, else its place in the answer.
        let i = if (r.index as usize) < count { r.index as usize } else { k };
        let Some(sp) = panels.get_mut(first + i).filter(|_| i < count) else { continue };
        sp.skip = r.skip;
        sp.description = r.description;
        sp.characters = r
            .characters
            .into_iter()
            .filter(|c| !c.name.trim().is_empty())
            .map(|mut c| {
                if let Some(m) = p.character(&c.name) {
                    c.name = m.name.clone();
                }
                c
            })
            .collect();
        sp.location = r.location;
        sp.lines = r
            .lines
            .into_iter()
            .filter(|l| !l.text.trim().is_empty())
            .map(|mut l| {
                if let Some(m) = p.character(&l.speaker) {
                    l.speaker = m.name.clone();
                }
                l
            })
            .collect();
        sp.sfx = r.sfx;
        sp.mood = r.mood;
        got += 1;
    }
    for sp in panels.iter_mut().skip(first).take(count) {
        sp.read = true;
        // A panel the answer said nothing about has nothing to adapt.
        if sp.description.trim().is_empty() && sp.lines.is_empty() {
            sp.skip = true;
        }
    }
    Ok(got)
}

/// One line per read panel, for the next batch and the board.
pub fn panel_digest(panels: &[SourcePanel], from: usize, to: usize) -> String {
    let mut s = String::new();
    for (i, sp) in panels.iter().enumerate().take(to).skip(from) {
        if !sp.story() {
            continue;
        }
        let who: Vec<String> = sp.characters.iter().map(|c| if c.expression.is_empty() { c.name.clone() } else { format!("{} ({})", c.name, c.expression) }).collect();
        let shape = if sp.w > 0 && sp.h > sp.w * 2 {
            " tall"
        } else if sp.h > 0 && sp.w > sp.h * 2 {
            " wide"
        } else {
            ""
        };
        let _ = write!(s, "#{} [p{}{shape}] {}", i + 1, sp.page, sp.description.trim());
        if !who.is_empty() {
            let _ = write!(s, " | who: {}", who.join(", "));
        }
        if !sp.location.is_empty() {
            let _ = write!(s, " | at: {}", sp.location);
        }
        for l in &sp.lines {
            let _ = write!(s, " | {} ({}): \"{}\"", if l.speaker.is_empty() { "?" } else { &l.speaker }, if l.kind.is_empty() { "speech" } else { &l.kind }, l.text.trim());
        }
        if !sp.sfx.is_empty() {
            let _ = write!(s, " | sfx: {}", sp.sfx);
        }
        if !sp.mood.is_empty() {
            let _ = write!(s, " | mood: {}", sp.mood);
        }
        s.push('\n');
    }
    s
}

// ------------------------------------------------------------------ assets and cuts

/// A background plate: painted background art, no people.
pub fn plate(p: &Project, pl: &Plate, refs: &[String]) -> String {
    let look = p.location(&pl.location).map(|l| l.look.clone()).unwrap_or_default();
    let mut s = format!(
        "Anime background art (a painted background, 美術) for a TV anime, 16:9: {}{}. View: {}. Light: {}. World: {}. {}. Painted like a professional anime background artist: detailed, atmospheric perspective, consistent light and cast shadows, clear depth. Composed wide with room around the subject so the camera can push in and pan. NO people, NO characters, NO creatures, no text, no watermark.",
        pl.location,
        if look.is_empty() { String::new() } else { format!(" — {look}") },
        if pl.view.is_empty() { "a wide establishing view" } else { &pl.view },
        if pl.light.is_empty() { "natural" } else { &pl.light },
        p.bible.world,
        ANIME_LOOK,
    );
    if !refs.is_empty() {
        let _ = write!(s, "\nReference images: {}. Keep the manga's architecture, props and layout of this place, redrawn as colour anime background art without any person in it.", refs.join("; "));
    }
    let props: Vec<String> = p.props.iter().filter(|x| x.plate == pl.id).map(|x| x.what.clone()).collect();
    if !props.is_empty() {
        let _ = write!(s, "\nLeave OUT the {}: characters sit or lean on {}, so {} drawn on the characters' own layer over this background. Keep the open ground where {} would stand clear and level.", props.join(" and "), if props.len() == 1 { "it" } else { "them" }, if props.len() == 1 { "it is" } else { "they are" }, if props.len() == 1 { "it" } else { "they" });
    }
    s
}

/// An effects element on pure black (screen) or on the chroma screen (opaque elements).
pub fn fx(f: &FxElement) -> String {
    if f.blend == "over" {
        format!("A single anime effects element, 2D hand-drawn anime effects animation style (sakuga effects): {}. Isolated in the centre on a perfectly flat, uniform chroma green (#00B140) background, nothing else, no shadow on the background, no text.", f.what)
    } else {
        format!("A single glowing anime effects element, 2D hand-drawn anime effects animation style (sakuga effects, bright cel-shaded shapes with a glowing core): {}. Isolated in the centre on a pure solid black background (#000000), nothing else, no ground, no reflections, no text.", f.what)
    }
}

/// The characters of a cut drawn alone on a chroma screen, over the plate's light.
pub fn layer(p: &Project, c: &Cut, aspect: &str, refs: &[String]) -> String {
    let cast: Vec<&CastMember> = c.characters.iter().filter_map(|x| p.character(&x.name)).collect();
    let (screen, hex) = screen_for(&cast);
    let mut s = format!("{}, {aspect} frame. {} {}", ANIME_LOOK, shot_line(c), c.picture);
    for x in &c.characters {
        let fixed = p.character(&x.name).map(|m| look_of(m, &x.outfit)).unwrap_or_else(|| x.name.clone());
        let place = match (x.side.trim(), x.facing.trim()) {
            ("", "") => String::new(),
            (side, "") => format!(" On the {side} of the frame."),
            ("", facing) => format!(" Facing {facing}."),
            (side, facing) => format!(" On the {side} of the frame, facing {facing}."),
        };
        let _ = write!(s, "\n  • {fixed}. Expression: {}. Pose: {}.{place}", x.expression, x.pose);
    }
    let light = p.plates.iter().find(|x| x.id == c.plate).map(|x| x.light.clone()).unwrap_or_default();
    let _ = write!(
        s,
        "\nDraw ONLY the characters (and anything they hold or wear) on a perfectly flat, uniform {screen} chroma-key background ({hex}): no floor, no scenery, no props on the ground, no cast shadow on the background, no gradient, no vignette, no text, no speech balloons. Place and size them in frame exactly as this shot needs, so they line up with the background plate. Light them as the plate is lit{}; clean crisp edges against the {screen}.",
        if light.is_empty() { String::new() } else { format!(" ({light})") }
    );
    let props: Vec<&Prop> = p.props.iter().filter(|x| x.plate == c.plate).collect();
    if !props.is_empty() {
        let touched = c.touched();
        let names: Vec<String> = props.iter().map(|x| x.what.clone()).collect();
        let _ = write!(
            s,
            "\nALSO draw on this layer, on the same {screen} screen, the {} (they are not in the background plate), exactly as on {} prop sheet{}: placed where this shot shows {} in the place, at the plate's scale and perspective, standing on the plate's ground.{}",
            names.join(" and "),
            if names.len() == 1 { "its" } else { "their" },
            if names.len() == 1 { "" } else { "s" },
            if names.len() == 1 { "it" } else { "them" },
            // "sitting, legs dangling" names no thing: in this place it is the prop.
            touched.or_else(|| c.touches_set().then(|| names.join(" and "))).map(|t| format!(" Whoever sits, leans or lies on the {t} rests on it: their weight on it, hands and feet touching it, one drawing with them.")).unwrap_or_default()
        );
    }
    if !refs.is_empty() {
        let _ = write!(s, "\nReference images: {}. Every character must match their model sheet exactly (face, hair, eyes, outfit, colours).", refs.join("; "));
    }
    if !c.notes.trim().is_empty() {
        let _ = write!(s, "\nDirector's notes, follow them: {}", c.notes.trim());
    }
    s
}

/// Where the camera stands for a cut, in the words a layout artist uses: the horizon's height in
/// the frame (it is the camera's eye level), how far the lines converge, and how high the lens is.
pub fn perspective(c: &Cut) -> String {
    let (horizon, height) = match c.angle.as_str() {
        "low" => ("low in the frame, about a quarter up from the bottom", "the lens below the characters' waists, looking up"),
        "worm's eye" | "ground level" => ("at or below the bottom edge", "the lens at the ground, looking up"),
        "high" => ("high in the frame, about three quarters up", "the lens above their heads, looking down"),
        "bird's eye" => ("above the top edge (looking straight down)", "the lens high overhead"),
        "dutch" => ("tilted about 15 degrees, crossing the middle", "the lens at eye level, rolled"),
        _ => ("across the middle, at the characters' eye level", "the lens at the characters' eye level"),
    };
    let lens = match c.lens.as_str() {
        "wide" => "a wide lens: strong convergence, near things large, depth stretched",
        "tele" => "a long lens: almost parallel lines, depth compressed, the background close and large",
        "fisheye" => "a fisheye: curved lines, strong distortion at the edges",
        _ => "a normal lens: natural convergence",
    };
    format!("Camera: {height}; the horizon line {horizon}; {lens}. Every line of the place converges to vanishing points on that horizon.")
}

/// The cut's own background: the plate's place painted again from this cut's camera (a real
/// layout BG, not a crop of a wide picture), empty and ready for what will be drawn into it.
pub fn cut_bg(p: &Project, c: &Cut, aspect: &str, refs: &[String]) -> String {
    let pl = p.plates.iter().find(|x| x.id == c.plate);
    let props: Vec<String> = p.props.iter().filter(|x| x.plate == c.plate).map(|x| x.what.clone()).collect();
    let mut s = format!(
        "Anime background art (美術) for ONE cut of a TV anime, {aspect} frame: {}{}. {} {} Re-draw the place of the background plate from THIS camera position and framing (as a background artist paints each cut from its layout), not a crop: the same architecture, props, materials, colours and light, seen from here. {} Painted background only: NO people, NO characters, NO creatures, no text.",
        pl.map(|x| x.location.as_str()).unwrap_or("the place"),
        pl.filter(|x| !x.light.is_empty()).map(|x| format!(", {}", x.light)).unwrap_or_default(),
        shot_line(c),
        perspective(c),
        ANIME_LOOK,
    );
    if !props.is_empty() {
        let _ = write!(s, " Leave out the {}: {} drawn later with the characters; keep the ground where {} stand{} clear and level.", props.join(" and "), if props.len() == 1 { "it is" } else { "they are" }, if props.len() == 1 { "it would" } else { "they would" }, "");
    }
    if !refs.is_empty() {
        let _ = write!(s, "\nReference images: {}.", refs.join("; "));
    }
    s
}

/// The cut's layout frame: the characters and their props drawn INTO the cut's background, in
/// its perspective and light, with their shadows (Image 1 is that background).
pub fn layout_frame(p: &Project, c: &Cut, aspect: &str, refs: &[String]) -> String {
    let light = p.plates.iter().find(|x| x.id == c.plate).map(|x| x.light.clone()).filter(|l| !l.is_empty()).unwrap_or_else(|| "the background's light".into());
    let mut s = format!("{}, {aspect} frame. {} {}", ANIME_LOOK, shot_line(c), c.picture);
    for x in &c.characters {
        let fixed = p.character(&x.name).map(|m| look_of(m, &x.outfit)).unwrap_or_else(|| x.name.clone());
        let place = match (x.side.trim(), x.facing.trim()) {
            ("", "") => String::new(),
            (side, "") => format!(" On the {side} of the frame."),
            ("", facing) => format!(" Facing {facing}."),
            (side, facing) => format!(" On the {side} of the frame, facing {facing}."),
        };
        let _ = write!(s, "\n  • {fixed}. Expression: {}. Pose: {}.{place}", x.expression, x.pose);
    }
    let props: Vec<String> = p.props.iter().filter(|x| x.plate == c.plate).map(|x| x.what.clone()).collect();
    if !props.is_empty() {
        let touched = c.touched().or_else(|| c.touches_set().then(|| props.join(" and ")));
        let _ = write!(s, "\nProps: draw the {} where this shot shows {} in the place, exactly as on {} prop sheet{}.{}", props.join(" and "), if props.len() == 1 { "it" } else { "them" }, if props.len() == 1 { "its" } else { "their" }, if props.len() == 1 { "" } else { "s" }, touched.map(|t| format!(" Whoever sits, leans or lies on the {t} rests on it: weight on it, hands and feet touching it.")).unwrap_or_default());
    }
    let _ = write!(
        s,
        "\nIMAGE 1 IS THIS CUT'S BACKGROUND: keep it exactly as it is (the same place, framing, horizon and perspective, every object where it is) and draw the characters and props INTO it.\n\
PERSPECTIVE: {} Everyone and every prop stands on that background's ground plane: feet and the props' bases on the ground at the right depth, each sized by distance, their eye lines and the props' edges converging to the same vanishing points as the background.\n\
LIGHT: the characters are lit by the scene's light ({light}), from the same direction and in the same colour as the background, with a rim of light where they stand against something bright, and darker where the place is in shade.\n\
SHADOWS: each character and prop casts a soft shadow on the ground, away from the light, and a small dark contact shadow where feet, seats and props touch the ground.\n\
No text, no speech balloons, no watermark.",
        perspective(c)
    );
    if !refs.is_empty() {
        let _ = write!(s, "\nReference images: {}. Every character must match their model sheet exactly (face, hair, eyes, outfit, colours).", refs.join("; "));
    }
    if !c.notes.trim().is_empty() {
        let _ = write!(s, "\nDirector's notes, follow them: {}", c.notes.trim());
    }
    s
}

/// The cel lifted from the layout (Image 1): the characters and props kept exactly, everything
/// else a flat chroma screen.
pub fn cel(p: &Project, c: &Cut) -> String {
    let cast: Vec<&CastMember> = c.characters.iter().filter_map(|x| p.character(&x.name)).collect();
    let (screen, hex) = screen_for(&cast);
    let props: Vec<String> = p.props.iter().filter(|x| x.plate == c.plate).map(|x| x.what.clone()).collect();
    let keep = if props.is_empty() { "every person in the frame, those in the background too (and what they hold or wear)".to_string() } else { format!("every person in the frame, those in the background too (and what they hold or wear), and the {}", props.join(" and ")) };
    format!("Edit Image 1. Keep {keep} EXACTLY as they are: the same position, size, pose, line art, colours and lighting, pixel for pixel. Replace everything else, the whole background, the ground and every shadow on it, with a perfectly flat, uniform {screen} ({hex}). No floor, no cast shadows, no gradient, no vignette, no text. Clean crisp edges against the {screen}.")
}

/// A whole frame drawn at once (a cut with no one from the cast in it that is not just the
/// plate: an insert of an object, a crowd, a monster).
pub fn frame(p: &Project, c: &Cut, aspect: &str, refs: &[String]) -> String {
    let mut s = format!("{}, {aspect} frame. {} {}", ANIME_LOOK, shot_line(c), c.picture);
    if c.has_cast() {
        for x in &c.characters {
            let fixed = p.character(&x.name).map(|m| look_of(m, &x.outfit)).unwrap_or_else(|| x.name.clone());
            let _ = write!(s, "\n  • {fixed}. Expression: {}. Pose: {}.", x.expression, x.pose);
        }
    }
    if let Some(pl) = p.plates.iter().find(|x| x.id == c.plate) {
        let _ = write!(s, "\nSetting: {} — {} — {}.", pl.location, pl.view, pl.light);
        let props: Vec<String> = p.props.iter().filter(|x| x.plate == pl.id).map(|x| x.what.clone()).collect();
        if !props.is_empty() {
            let _ = write!(s, "\nThe place's {} stand where they belong in it, as on their prop sheets.", props.join(", "));
        }
    }
    s.push_str("\nNo text, no speech balloons, no watermark.");
    if !refs.is_empty() {
        let _ = write!(s, "\nReference images: {}.", refs.join("; "));
    }
    if !c.notes.trim().is_empty() {
        let _ = write!(s, "\nDirector's notes, follow them: {}", c.notes.trim());
    }
    s
}

fn mouths(c: &Cut) -> String {
    let speakers: Vec<&str> = c.lines.iter().filter(|l| said_aloud(l)).map(|l| l.speaker.as_str()).filter(|s| c.characters.iter().any(|x| x.name.eq_ignore_ascii_case(s))).collect();
    if speakers.is_empty() || !c.mouth {
        " Nobody's mouth moves: mouths stay closed.".into()
    } else {
        format!(" {} speaks: the mouth opens and closes in simple anime lip flaps.", speakers.join(" and "))
    }
}

/// What the video model is asked for a cut: a `layered` cut animates the characters alone on
/// their screen with a locked camera (the camera moves in compositing); a `full` cut animates
/// the whole key frame with the camera move.
pub fn motion(c: &Cut) -> String {
    let act = if c.action.trim().is_empty() { "small natural movements: breathing, a blink, hair and clothes stir" } else { c.action.trim() };
    let notes = if c.notes.trim().is_empty() { String::new() } else { format!(" Director's notes: {}", c.notes.trim()) };
    if c.render_kind() == "layered" {
        let how = if c.motion == "lip" { "Only the mouth and eyes move (lip flaps, a blink); the body is held still" } else { "Limited animation: a few held poses with short moves between them" };
        format!("2D anime cel animation of the character(s) on a flat chroma-key screen. Locked-off camera: the camera does not move, zoom or pan. The background stays one perfectly flat, even colour with no shadows, no floor, no objects, no light changes. {how}: {act}.{} Keep their faces, hair, outfits, colours and line art exactly as in the first frame; anime timing on threes; no text, no new characters.{notes}", mouths(c))
    } else {
        format!("TV anime scene, 2D cel animation; keep the exact art style, character designs, colours and background of the first frame. {} ({} shot, {} angle). Action: {act}.{} Dynamic anime motion with weight and impact, smears on fast moves; no morphing, no new characters, no text.{notes}", capitalise(move_words(&c.camera)), c.size, c.angle, mouths(c))
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

// ------------------------------------------------------------------ photography

fn frame_size(aspect: &str, rough: bool) -> (u32, u32) {
    match (aspect == "9:16", rough) {
        (true, false) => (1080, 1920),
        (false, false) => (1920, 1080),
        (true, true) => (720, 1280),
        (false, true) => (1280, 720),
    }
}

/// The camera of a cut for the compositor: the move, how far it travels and zooms (from the
/// measured speeds of its scene kind) and the shake.
fn camera_json(c: &Cut, kind: &str) -> Value {
    let (travel, zoom) = pacing::camera_extent(kind, &c.camera, c.seconds, c.amount);
    serde_json::json!({ "move": c.camera, "travel": travel, "zoom": zoom, "shake": if c.camera == "shake" { c.shake.max(0.6) } else { c.shake } })
}

/// What a finished cut is put together from. Paths are absolute.
#[derive(Default)]
pub struct CompParts<'a> {
    pub plate: Option<&'a std::path::Path>,
    /// The cut's layout frame, when the layer was lifted from it: its shadows are carried back
    /// under the layer (or it stands in for the plate when the plate drifted from it).
    pub layout: Option<&'a std::path::Path>,
    /// The character layer, and whether it is an animation (else one drawing).
    pub layer: Option<(&'a std::path::Path, bool)>,
    /// The whole frame animated by the video model (a `full` cut).
    pub base: Option<&'a std::path::Path>,
    pub fx: &'a [(std::path::PathBuf, &'a FxUse, &'a FxElement)],
}

/// The compositor's job for a cut (`anime_comp.py comp` / `still`) in a scene of `kind`.
pub fn comp_job(c: &Cut, kind: &str, out: &std::path::Path, parts: CompParts<'_>, aspect: &str, finish: bool) -> Value {
    let (w, h) = frame_size(aspect, false);
    let seed = c.number.max(1) * 7 + 1;
    // Drawn animation is held three frames a drawing (two in action); one drawing needs no step.
    let step = if kind == "action" { 2 } else { 3 };
    let chars: Vec<Value> = parts
        .layer
        // Lifted from a lit layout, a layer keeps its own light and the layout's shadows.
        .map(|(f, animated)| if parts.layout.is_some() { serde_json::json!({ "path": f, "key": "green", "step": if animated { step } else { 1 }, "shadow": false, "match": 0.08 }) } else { serde_json::json!({ "path": f, "key": "green", "step": if animated { step } else { 1 }, "shadow": size_rank(&c.size) <= 3 }) })
        .into_iter()
        .collect();
    let fxs: Vec<Value> = parts.fx.iter().map(|(f, u, e)| serde_json::json!({ "path": f, "blend": if e.blend == "over" { "over" } else { "screen" }, "x": u.x, "y": u.y, "scale": u.scale, "front": u.front, "opacity": u.opacity })).collect();
    let fin = if finish {
        serde_json::json!({ "bloom": 0.35, "diffusion": 0.15, "grain": 0.03, "ca": 1.0, "vignette": 0.25, "dust": c.dust, "rays": c.rays, "speed": c.speed, "flash": c.flash })
    } else {
        serde_json::json!({ "bloom": 0, "diffusion": 0, "grain": 0, "ca": 0, "vignette": 0 })
    };
    let mut camera = camera_json(c, kind);
    if parts.base.is_some() {
        // A full cut's camera was moved by the video model: only the shake is added here.
        camera["move"] = "fix".into();
        camera["travel"] = 0.0.into();
        camera["zoom"] = 0.0.into();
    }
    serde_json::json!({
        "out": out, "width": w, "height": h, "fps": 24, "seconds": c.seconds.max(pacing::CUT_MIN), "seed": seed,
        "plate": parts.plate.map(|p| serde_json::json!({ "path": p })),
        "layout": parts.layout.map(|p| serde_json::json!({ "path": p })),
        "base": parts.base.map(|b| serde_json::json!({ "path": b, "step": if c.camera == "fix" { step } else { 1 } })),
        "chars": chars, "fx": fxs, "camera": camera,
        "grade": if finish { c.grade.as_str() } else { "neutral" },
        "finish": fin,
    })
}

/// The animatic's job for a cut: the manga panel it adapts (`panel`, with its size) under the
/// cut's camera move, in its colour key, with a slate; a cut the director added shows a card.
pub fn rough_job(c: &Cut, kind: &str, out: &std::path::Path, panel: Option<(&std::path::Path, u32, u32)>, aspect: &str) -> Value {
    let (w, h) = frame_size(aspect, true);
    let slate = format!("C{:03}  {:.1}s  {} / {}  {}  {}", c.number, c.seconds, c.size, c.angle, c.camera.replace('_', " "), c.motion);
    let mut job = serde_json::json!({
        "out": out, "width": w, "height": h, "fps": 24, "seconds": c.seconds.max(pacing::CUT_MIN), "seed": c.number.max(1),
        "camera": camera_json(c, kind), "grade": c.grade,
        "finish": { "bloom": 0, "diffusion": 0, "grain": 0, "ca": 0, "vignette": 0.15, "flash": c.flash, "speed": c.speed },
        "slate": slate,
    });
    match panel {
        Some((file, pw, ph)) => {
            // A panel much taller than the frame is travelled down (or up) by a tilt, as the
            // reader scrolls it; any other is shown whole over a blurred copy of itself.
            let tall = pw > 0 && (ph as f32 / pw as f32) > (h as f32 / w as f32) * 1.6;
            let fit = if tall && matches!(c.camera.as_str(), "tilt_up" | "tilt_down") { "scan" } else { "contain" };
            job["base"] = serde_json::json!({ "path": file, "step": 1, "fit": fit });
        }
        None => {
            job["card"] = serde_json::json!({ "title": format!("Cut {} · added by the director", c.number), "text": c.picture });
        }
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{Chapter, Outfit};

    pub(crate) fn proj() -> Project {
        let mut p = Project { adapts: Some("Hunter".into()), ..Default::default() };
        p.cast = vec![
            CastMember { name: "Jin Woo".into(), appearance: "black hair, grey eyes".into(), outfits: vec![Outfit { name: "hoodie".into(), look: "blue hoodie".into() }], ..Default::default() },
            CastMember { name: "Mira".into(), appearance: "long green hair".into(), ..Default::default() },
        ];
        p.chapters = vec![Chapter { number: 1, ..Default::default() }];
        p.normalize();
        p
    }

    #[test]
    fn the_ai_s_words_become_ours() {
        assert_eq!(norm_size("ECU"), "extreme close-up");
        assert_eq!(norm_size("Medium Close-Up"), "medium close-up");
        assert_eq!(norm_size("wide shot"), "long");
        assert_eq!(norm_size("extreme wide shot"), "extreme long");
        assert_eq!(norm_size("OTS on Song"), "over-the-shoulder");
        assert_eq!(norm_size("two shot"), "two-shot");
        assert_eq!(norm_size("establishing shot"), "establishing");
        assert_eq!(norm_size("close up"), "close-up");
        assert_eq!(norm_size(""), "medium");
        assert!(SIZES.iter().all(|s| norm_size(s) == *s), "every size reads as itself");
        assert_eq!(norm_angle("low angle, dutch tilt"), "low dutch");
        assert_eq!(norm_angle("canted"), "dutch");
        assert_eq!(norm_angle("top-down"), "bird's eye");
        assert_eq!(norm_angle(""), "eye level");
        assert_eq!(norm_lens("telephoto, compressed"), "tele");
        assert_eq!(norm_lens("wide-angle"), "wide");
        assert_eq!((norm_camera("slow push in"), norm_camera("T.U"), norm_camera("pan up"), norm_camera("static"), norm_camera("pull back")), ("push_in", "push_in", "tilt_up", "fix", "pull_out"));
        assert!(MOVES.iter().all(|m| norm_camera(m) == *m), "every move reads as itself");
        assert_eq!((norm_motion("", "fix"), norm_motion("", "pan_left"), norm_motion("still", "push_in"), norm_motion("lip flaps", "fix"), norm_motion("layered", "fix"), norm_motion("FULL", "fix")), ("hold", "camera", "camera", "lip", "pose", "full"));
        assert_eq!((norm_grade("Cold Dungeon"), norm_grade("teal")), ("cold_dungeon", "neutral"));
        assert_eq!((norm_transition("O.L"), norm_transition("fade to black"), norm_transition("hard cut")), ("dissolve", "black", "cut"));
        assert_eq!((norm_kind("fight scene"), norm_kind("suspense"), norm_kind("Dialogue"), norm_kind("?")), ("action", "dread", "dialogue", "dialogue"));
        assert!(size_rank("long") < size_rank("medium") && size_rank("medium") < size_rank("close-up"));
    }

    #[test]
    fn reading_fills_the_batch_and_names_the_cast() {
        let p = proj();
        let mut panels = vec![SourcePanel::default(); 4];
        let v = serde_json::json!({ "panels": [
            { "index": 0, "description": "a gate opens", "characters": [{ "name": "jin woo" }, { "name": " " }], "lines": [{ "speaker": "Jin", "text": "Again...", "kind": "thought" }, { "speaker": "x", "text": "  " }] },
            { "index": "1", "skip": "yes" }
        ]});
        assert_eq!(apply_read(&p, &mut panels, 1, 2, v).unwrap(), 2);
        assert!(!panels[0].read && panels[1].read && panels[2].read && !panels[3].read);
        assert_eq!(panels[1].characters.len(), 1);
        assert_eq!(panels[1].characters[0].name, "Jin Woo");
        assert_eq!(panels[1].lines.len(), 1);
        assert_eq!(panels[1].lines[0].speaker, "Jin Woo");
        assert!(panels[2].skip);
        let d = panel_digest(&panels, 0, 4);
        assert!(d.contains("#2 [p0] a gate opens") && d.contains("Jin Woo (thought): \"Again...\"") && !d.contains("#3"));
        assert!(apply_read(&p, &mut panels, 0, 1, serde_json::json!({ "panels": [] })).is_err());
        // A tall panel is marked for the director.
        panels[1].w = 700;
        panels[1].h = 2000;
        assert!(panel_digest(&panels, 0, 4).contains("[p0 tall]"));
    }

    #[test]
    fn prompts_carry_the_shot_screen_and_moves() {
        let mut p = proj();
        p.plates = vec![Plate { id: "street".into(), location: "Street".into(), view: "wide".into(), light: "noon sun".into(), image: None }];
        let mut c = Cut {
            number: 1,
            size: "medium".into(),
            angle: "low".into(),
            lens: "wide".into(),
            composition: "Mira large on the left third".into(),
            picture: "Mira glares".into(),
            plate: "street".into(),
            motion: "lip".into(),
            mouth: true,
            camera: "push_in".into(),
            seconds: 5.,
            characters: vec![PanelChar { name: "Mira".into(), side: "left".into(), facing: "right".into(), ..Default::default() }],
            lines: vec![Line { speaker: "Mira".into(), text: "Go".into(), kind: "speech".into() }],
            ..Default::default()
        };
        let l = layer(&p, &c, "16:9", &["Image 1: Mira model sheet".into()]);
        // Green hair: drawn on magenta, never on green.
        assert!(l.contains("magenta") && l.contains("#FF00FF") && l.contains("noon sun") && l.contains("Image 1: Mira model sheet"));
        assert!(l.contains("medium shot, low angle, wide-angle lens") && l.contains("Composition: Mira large on the left third.") && l.contains("On the left of the frame, facing right."));
        assert_eq!(c.render_kind(), "layered");
        assert!(!c.touches_set() && c.touched().is_none());
        // Sitting on a prop: it is found, left out of the plate and drawn on the layer with her.
        let mut sat = c.clone();
        sat.characters[0].pose = "sitting on the plank stack, elbows on knees".into();
        assert_eq!(sat.touched().as_deref(), Some("plank stack"));
        assert!(sat.touches_set() && sat.render_kind() == "layered");
        let mut q = p.clone();
        q.chapters = vec![crate::project::Chapter { number: 1, ..Default::default() }];
        q.chapters[0].anime.cuts = vec![sat.clone()];
        assert!(sync_props(&mut q) && !sync_props(&mut q), "added once");
        assert_eq!((q.props[0].what.as_str(), q.props[0].plate.as_str()), ("plank stack", "street"));
        let pl = q.plates.iter().find(|x| x.id == "street").cloned().unwrap_or(Plate { id: "street".into(), ..Default::default() });
        assert!(plate(&q, &pl, &[]).contains("Leave OUT the plank stack"));
        let l = layer(&q, &sat, "16:9", &[]);
        assert!(l.contains("ALSO draw on this layer") && l.contains("rests on it"), "{l}");
        assert!(prop_sheet(&q, &q.props[0], &[]).contains("plank stack"));
        // On the ground, on her knees, in shock: nothing to draw.
        for pose in ["kneeling on the ground", "sitting in shock", "leaning on his knees", "situation tense, standing", "leaning in toward him", "leaning back in surprise", "one hand resting on it", "leaning on the truck side", "seated at the base of the altar steps"] {
            sat.characters[0].pose = pose.into();
            assert_eq!(sat.touched(), None, "{pose}");
        }
        // A bench a few words on.
        sat.characters[0].pose = "slumped against a rusty bench, eyes closed".into();
        assert_eq!(sat.touched().as_deref(), Some("rusty bench"));
        let m = motion(&c);
        assert!(m.contains("Locked-off camera") && m.contains("Mira speaks") && m.contains("Only the mouth and eyes move"));
        // Heard over her back: no mouth to move.
        c.mouth = false;
        assert!(motion(&c).contains("mouths stay closed"));
        c.motion = "full".into();
        assert!(motion(&c).starts_with("TV anime scene") && motion(&c).contains("The camera slowly pushes in"));
        c.characters[0].name = "Jin Woo".into();
        let cast: Vec<&CastMember> = vec![p.character("Jin Woo").unwrap()];
        assert_eq!(screen_for(&cast).0, "chroma green");
        assert!(plate(&p, &p.plates[0], &[]).contains("NO people"));
        assert!(fx(&FxElement { what: "fire".into(), blend: "screen".into(), ..Default::default() }).contains("pure solid black"));
        // A full cut: the video model moved the camera, the compositor only finishes it.
        let job = comp_job(&c, "dialogue", std::path::Path::new("o.mp4"), CompParts { base: Some(std::path::Path::new("clip.mp4")), ..Default::default() }, "16:9", true);
        assert_eq!(job["camera"]["move"], "fix");
        assert_eq!(job["width"], 1920);
        // A still under a slow push: at least 8%, so the whole frame visibly moves.
        c.motion = "camera".into();
        let job = comp_job(&c, "dialogue", std::path::Path::new("o.mp4"), CompParts { plate: Some(std::path::Path::new("p.png")), layer: Some((std::path::Path::new("l.png"), false)), ..Default::default() }, "16:9", true);
        assert_eq!(job["camera"]["move"], "push_in");
        assert!((job["camera"]["zoom"].as_f64().unwrap() - 0.08).abs() < 1e-6);
        assert_eq!(job["chars"][0]["step"], 1);
    }

    #[test]
    fn the_animatic_shows_the_panel_or_a_card() {
        let mut c = Cut { number: 4, size: "full".into(), angle: "low".into(), camera: "tilt_up".into(), motion: "camera".into(), seconds: 5., picture: "The statue towers".into(), grade: "cold_dungeon".into(), ..Default::default() };
        // A tall webtoon panel under a tilt is travelled, like a reader scrolling it.
        let job = rough_job(&c, "dread", std::path::Path::new("r.mp4"), Some((std::path::Path::new("p.jpg"), 720, 3000)), "16:9");
        assert_eq!((job["base"]["fit"].as_str(), job["width"].as_u64(), job["grade"].as_str()), (Some("scan"), Some(1280), Some("cold_dungeon")));
        assert!(job["slate"].as_str().unwrap().starts_with("C004  5.0s  full / low  tilt up"));
        c.camera = "fix".into();
        assert_eq!(rough_job(&c, "dread", std::path::Path::new("r.mp4"), Some((std::path::Path::new("p.jpg"), 720, 3000)), "16:9")["base"]["fit"], "contain");
        let card = rough_job(&c, "dread", std::path::Path::new("r.mp4"), None, "16:9");
        assert!(card.get("base").is_none() && card["card"]["text"] == "The statue towers");
    }
}
