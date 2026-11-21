//! The picture prompt compiler (design 6.6, plan 10): one short paragraph per section in a fixed
//! order (Style, Pictures, Camera, Action, Props, Colour, Keep, Avoid), built only from the
//! shot's numbers and the bible through the geometry functions and the phrase banks in [`bank`],
//! never from text a model wrote for the prompt. The text sent is the user's override when there
//! is one ([`crate::frames::final_prompt`]), else this.
//!
//! What image models follow (research 12 §2, 08 §2): what the lens *does* rather than a
//! millimetre number, a stated horizon line, a stated screen side with a costume cue, and one
//! stated job per reference picture, named by its position ("Image 2").

use std::collections::BTreeSet;

pub use crate::model::Compiled;
use crate::geometry;
use crate::model::{
    Angle, BackgroundKind, BibleIndex, Depth, Derived, EmotionKind, FocusTarget, Hand, Key, LightSetup, LookAt, Mark, Move, MouthPolicy, Place, RefRole, RefSlot, Scene, SheetKind,
    Shot, ShotSize, Station, TimeOfDay, Visibility,
};
use crate::slots::{self, file_stem, join_and};

/// Section names, in the order they are written.
pub const SECTIONS: [&str; 8] = ["Style", "Pictures", "Camera", "Action", "Props", "Colour", "Keep", "Avoid"];

/// The style paragraph when the bible has none yet (the bible step's default look).
pub const STYLE_FALLBACK: &str =
    "Thin dark-brown lines, two-tone hard cel shading, painted backgrounds without outlines, glow on light sources, soft gradient from the key-light side.";

/// The closing line of every frame prompt (design 6.6).
pub const CLOSING: &str = "No text, no speech bubbles, no captions, no extra characters, no watermark.";

/// Fills `{key}` placeholders.
pub(crate) fn fill(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

/// Phrase banks: the words each camera, staging and light field becomes. Plain data, easy to
/// tune; placeholders in braces are filled by the compiler.
pub mod bank {
    use crate::model::{Angle, Depth, EmotionKind, Key, LightSetup, Pose, ShotSize, TimeOfDay};

    /// Shot size. `{subject}`, `{place}`, `{thing}`, `{a}`, `{b}`.
    pub const SIZE: &[(ShotSize, &str)] = &[
        (ShotSize::Els, "extreme long shot: {subject} a tiny figure in a vast {place}"),
        (ShotSize::Ls, "long shot: {subject} small in the frame, full figure, with plenty of {place} around"),
        (ShotSize::Fs, "full shot: {subject} head to toe in the frame"),
        (ShotSize::Cowboy, "cowboy shot: {subject} framed from the head to mid-thigh, hands at the hips visible"),
        (ShotSize::Ms, "medium shot: {subject} from the waist up"),
        (ShotSize::Mcu, "medium close-up (bust shot): {subject} from mid-chest up"),
        (ShotSize::Cu, "close-up of {subject}'s face and head, cropped at the neck"),
        (ShotSize::Ecu, "extreme close-up of {subject}'s eyes, framed from below the lips to above the eyebrows"),
        (ShotSize::Insert, "insert close-up of {thing}, filling the frame"),
        (ShotSize::Ots, "over-the-shoulder shot past {a}'s blurred shoulder and back of the head, {b} facing the camera"),
        (ShotSize::TwoShot, "two-shot: {a} and {b} both in the frame from the waist up"),
        (ShotSize::Pov, "first-person point of view of {subject}: the camera sees what they see"),
        (ShotSize::Overhead, "overhead shot looking straight down on {place}"),
    ];
    /// Used when a size needs two characters and the shot has fewer.
    pub const SIZE_FALLBACK: &str = "medium shot: {subject} from the waist up";
    /// A close framing (MCU, CU, ECU) with nobody on screen.
    pub const EMPTY_CLOSE: &str = "insert close-up of a detail of {place}, filling the frame, nobody in it";
    /// A full, cowboy, medium, over-the-shoulder or two-shot framing with nobody on screen.
    pub const EMPTY_VIEW: &str = "medium view of {place} with nobody in the frame";
    /// A point of view whose looker is off screen and nobody else is seen.
    pub const EMPTY_POV: &str = "first-person point of view: {place} as someone standing in it sees it, nobody in the frame";

    /// Camera angle. `{subject}`, `{roll}`.
    pub const ANGLE: &[(Angle, &str)] = &[
        (Angle::Eye, "eye-level shot"),
        (Angle::Low, "low-angle shot looking up at {subject}"),
        (Angle::High, "high-angle shot looking down at {subject}"),
        (Angle::Top, "top-down bird's-eye view looking straight down"),
        (Angle::Worm, "worm's-eye view from the ground looking steeply up at {subject}, verticals converging toward the top"),
        (Angle::Dutch, "Dutch angle: the camera rolled {roll} degrees, the horizon tilted"),
    ];
    /// Roll of a Dutch angle without its own roll.
    pub const DUTCH_DEFAULT_DEG: f32 = 15.;

    /// What a lens does, by the longest focal length (mm) of each band. `{mm}`.
    pub const LENS: &[(u16, &str)] = &[
        (18, "ultra-wide {mm} mm lens: camera very close, exaggerated perspective, the foreground looms large and the background is tiny and far"),
        (28, "wide {mm} mm lens: camera close, space stretched, strong depth and converging lines"),
        (40, "{mm} mm lens: natural space with a little depth"),
        (65, "normal {mm} mm lens: true proportions, perspective as the eye sees it"),
        (110, "long {mm} mm lens: camera far away, background compressed and close behind"),
        (u16::MAX, "telephoto {mm} mm lens from far away: flat compressed perspective, the background stacked right behind the subject"),
    ];

    /// Depth of field. `{target}`.
    pub const DEPTH: &[(Depth, &str)] = &[
        (Depth::Deep, "deep focus: everything from foreground to background sharp"),
        (Depth::Normal, "focus on {target}, the background slightly soft but readable"),
        (Depth::Shallow, "shallow depth of field: focus on {target}, background softly out of focus"),
    ];
    /// A focus pull is done later in the compositor: the still keeps every layer sharp.
    pub const RACK: &str = "deep focus: every layer sharp";

    /// Exposure key.
    pub const KEY: &[(Key, &str)] = &[
        (Key::Dark, "low-key lighting, deep shadows, most of the frame in darkness"),
        (Key::Normal, "balanced natural exposure"),
        (Key::Bright, "high-key lighting, bright and airy, soft light shadows"),
    ];

    /// Background kinds that do not use a plate.
    pub const BACKDROP: &str = "background: a simple backdrop in {place}'s colours (a plain wall, the floor or the sky), softly blurred, no detailed room";
    /// A simple backdrop when the place has no name.
    pub const BACKDROP_PLAIN: &str = "background: a simple plain backdrop, softly blurred, no detailed room";
    /// No room at all behind the characters.
    pub const NO_BACKGROUND: &str = "background: no room, only darkness, mist or speed lines behind the characters";
    /// The setting line when there is no plate or backdrop.
    pub const SETTING: &str = "setting: {place}";

    /// Body pose.
    pub const POSE: &[(Pose, &str)] = &[
        (Pose::Stand, "standing"),
        (Pose::Sit, "sitting"),
        (Pose::Kneel, "kneeling"),
        (Pose::Lie, "lying down"),
        (Pose::Run, "running"),
        (Pose::Fight, "in a fighting stance"),
    ];

    /// Facing relative to the camera, by the largest angle (degrees) away from facing it. `{side}`.
    pub const FACING: &[(f32, &str)] = &[
        (30., "facing the camera"),
        (70., "in three-quarter view, turned toward screen {side}"),
        (115., "in profile, facing screen {side}"),
        (150., "turned away in three-quarter back view, toward screen {side}"),
        (180., "with the back to the camera"),
    ];
    /// A character with the back to the camera.
    pub const BACK: &str = "with the back to the camera";
    /// A character partly cut off by the frame edge.
    pub const PARTIAL: &str = "partly cut off by the frame edge";
    /// The foreground character of an over-the-shoulder shot.
    pub const OTS: &str = "in the foreground, seen from behind over the shoulder, softly out of focus";

    /// Screen thirds: left below the first value, right above the second.
    pub const THIRDS: (f32, f32) = (0.36, 0.64);
    /// Words for the left screen third.
    pub const LEFT: &str = "on the left third of the frame";
    /// Words for the centre of the screen.
    pub const CENTRE: &str = "in the centre of the frame";
    /// Words for the right screen third.
    pub const RIGHT: &str = "on the right third of the frame";

    /// Faces by emotion and strength (mild, medium, strong).
    pub const EMOTION: &[(EmotionKind, [&str; 3])] = &[
        (EmotionKind::Neutral, ["a calm, neutral face", "a calm, neutral face", "a blank, unreadable face"]),
        (EmotionKind::Joy, ["a slight smile", "a warm open smile, eyes bright", "beaming with joy, laughing, eyes crinkled"]),
        (EmotionKind::Sadness, ["a faint sad look, eyes lowered", "sad, brows raised in the middle, mouth turned down", "devastated, tears running down the cheeks"]),
        (EmotionKind::Anger, ["slightly annoyed, a small frown", "angry, brows drawn together, jaw tight", "furious, teeth bared, eyes blazing"]),
        (EmotionKind::Fear, ["uneasy, eyes darting", "afraid, eyes wide, shoulders raised", "terrified, eyes wide open, pupils small, mouth open"]),
        (EmotionKind::Surprise, ["mildly surprised, brows raised", "surprised, eyes wide, mouth slightly open", "shocked, eyes huge, mouth wide open"]),
        (EmotionKind::Disgust, ["faintly disgusted, lip curled", "disgusted, nose wrinkled, leaning away", "revolted, face twisted, recoiling"]),
        (EmotionKind::Resolve, ["focused, a steady gaze", "determined, jaw set, eyes narrowed", "fierce resolve, eyes burning, jaw clenched"]),
        (EmotionKind::Pride, ["quietly confident, chin up", "proud, chest out, a slight smirk", "triumphant, head high, a wide confident grin"]),
        (EmotionKind::Grief, ["quietly mournful, eyes downcast", "grieving, eyes wet, lips pressed together", "overwhelmed by grief, sobbing, face crumpled"]),
        (EmotionKind::Despair, ["weary, eyes dull", "hopeless, shoulders slumped, empty eyes", "broken, hollow eyes, head hanging"]),
        (EmotionKind::Relief, ["a small relieved breath", "relieved, shoulders dropping, a soft smile", "flooded with relief, eyes closed, a long exhale"]),
        (EmotionKind::Awe, ["curious, eyes widening", "in awe, mouth slightly open, eyes wide", "awestruck, frozen, eyes huge and shining"]),
        (EmotionKind::Tender, ["a gentle look", "tender, a soft smile, warm eyes", "deeply tender, eyes glistening, a soft loving smile"]),
        (EmotionKind::Menace, ["a cold stare", "menacing, eyes narrowed, a thin smile", "terrifying menace, eyes shadowed, a cruel grin"]),
    ];

    /// One word per emotion (expression sheet jobs).
    pub const EMOTION_ADJ: &[(EmotionKind, &str)] = &[
        (EmotionKind::Neutral, "calm"),
        (EmotionKind::Joy, "happy"),
        (EmotionKind::Sadness, "sad"),
        (EmotionKind::Anger, "angry"),
        (EmotionKind::Fear, "afraid"),
        (EmotionKind::Surprise, "surprised"),
        (EmotionKind::Disgust, "disgusted"),
        (EmotionKind::Resolve, "determined"),
        (EmotionKind::Pride, "proud"),
        (EmotionKind::Grief, "grieving"),
        (EmotionKind::Despair, "despairing"),
        (EmotionKind::Relief, "relieved"),
        (EmotionKind::Awe, "awed"),
        (EmotionKind::Tender, "tender"),
        (EmotionKind::Menace, "menacing"),
    ];

    /// Light setups (Auto = the place state decides).
    pub const LIGHT: &[(LightSetup, &str)] = &[
        (LightSetup::Backlit, "strong backlight, a bright rim outlining the hair and shoulders"),
        (LightSetup::Window, "soft window light"),
        (LightSetup::UnderLight, "lit from below, shadows cast upward, eyes lit from beneath"),
        (LightSetup::Silhouette, "backlit silhouette: the characters are flat dark shapes against the brightest part of the background"),
        (LightSetup::Top, "hard top light, eyes in shadow"),
        (LightSetup::Rim, "strong rim light outlining the hair and shoulders"),
    ];

    /// Time of day.
    pub const TIME: &[(TimeOfDay, &str)] = &[
        (TimeOfDay::Day, "clear daylight"),
        (TimeOfDay::Dusk, "golden low sun, long warm shadows"),
        (TimeOfDay::Night, "moonlit night, cool blue light, deep navy shadows"),
        (TimeOfDay::Dawn, "early dawn, soft warm light low on the horizon, blue-violet shadows"),
        (TimeOfDay::Indoor, "indoor light"),
    ];
    /// Characters at night or dusk.
    pub const NIGHT_COLOURS: &str = "characters lit in their night colours: skin and clothes bluer and darker";

    /// Key light colour by the highest colour temperature (K) of each band.
    pub const KELVIN: &[(u32, &str)] = &[
        (2400, "deep orange firelight"),
        (3500, "warm amber"),
        (4800, "warm white"),
        (6000, "neutral daylight"),
        (7500, "cool daylight"),
        (u32::MAX, "cold blue"),
    ];

    /// Where the key light comes from on screen, by its angle to the camera's view.
    pub const FROM_BEHIND: &str = "from behind the characters, toward the camera";
    /// Key light behind the characters.
    pub const FROM_FRONT: &str = "from the front, behind the camera";
    /// Key light from the camera's side.
    pub const FROM_LEFT: &str = "from screen left";
    /// Key light from screen left.
    pub const FROM_RIGHT: &str = "from screen right";
    /// Key light from screen right.
    pub const FROM_BELOW: &str = "from below";
    /// Key light from below (under-light).
    pub const FROM_ABOVE: &str = "from directly above";

    /// Palette swatch roles, in the order they are named.
    pub const SWATCHES: &[(&str, &str)] = &[
        ("dominant", "main colour"),
        ("mid", "mid-tones"),
        ("light", "lights"),
        ("shadow", "shadows"),
        ("deep", "deepest darks"),
        ("skin", "skin"),
    ];
}

/// One word for an emotion ("angry").
pub fn emotion_adjective(kind: EmotionKind) -> &'static str {
    bank::EMOTION_ADJ.iter().find(|(k, _)| *k == kind).map(|(_, w)| *w).unwrap_or("calm")
}

/// The face words of an emotion at a strength (0 = unset reads as medium).
pub fn emotion_words(kind: EmotionKind, strength: u8) -> &'static str {
    let i = match strength {
        0 | 2 => 1,
        1 => 0,
        _ => 2,
    };
    bank::EMOTION.iter().find(|(k, _)| *k == kind).map(|(_, w)| w[i]).unwrap_or("a calm, neutral face")
}

fn lookup<K: PartialEq + Copy, V: Copy>(table: &[(K, V)], k: K) -> Option<V> {
    table.iter().find(|(x, _)| *x == k).map(|(_, v)| *v)
}

/// What a lens does, in words.
pub fn lens_phrase(lens_mm: u16) -> String {
    let mm = if lens_mm == 0 { geometry::DEFAULT_LENS_MM } else { lens_mm };
    let t = bank::LENS.iter().find(|(max, _)| mm <= *max).map(|(_, t)| *t).unwrap_or(bank::LENS[bank::LENS.len() - 1].1);
    fill(t, &[("mm", &mm.to_string())])
}

/// A colour word for "#RRGGBB" ("dark blue"); `None` when it is not a hex colour.
pub fn colour_name(hex: &str) -> Option<String> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).map(|b| b as f32 / 255.).unwrap_or(0.);
    let (r, g, b) = (v(0), v(2), v(4));
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.;
    let d = max - min;
    if l < 0.1 {
        return Some("black".into());
    }
    if l > 0.93 {
        return Some("white".into());
    }
    let s = if d == 0. { 0. } else { d / (1. - (2. * l - 1.).abs()) };
    if s < 0.15 {
        return Some(if l < 0.35 { "dark grey" } else if l > 0.7 { "light grey" } else { "grey" }.into());
    }
    let hue = if d == 0. {
        0.
    } else if max == r {
        60. * ((g - b) / d).rem_euclid(6.)
    } else if max == g {
        60. * ((b - r) / d + 2.)
    } else {
        60. * ((r - g) / d + 4.)
    };
    let base = match hue {
        h if !(15. ..345.).contains(&h) => "red",
        h if h < 40. => "orange",
        h if h < 65. => "yellow",
        h if h < 160. => "green",
        h if h < 195. => "teal",
        h if h < 250. => "blue",
        h if h < 290. => "purple",
        _ => "pink",
    };
    let word = match (base, l) {
        ("red", l) if l > 0.75 => "pink".to_string(),
        ("orange", l) if l < 0.35 => "brown".to_string(),
        (b, l) if l < 0.3 => format!("dark {b}"),
        (b, l) if l > 0.75 => format!("light {b}"),
        (b, _) => b.to_string(),
    };
    Some(word)
}

/// Capitalises the first letter.
fn cap(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Sentences from parts: each capitalised and ending with a full stop, joined by spaces.
fn sentences(parts: &[String]) -> String {
    parts
        .iter()
        .map(|p| p.trim().trim_end_matches(['.', ',', ';']).trim())
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut s = cap(p);
            if !s.ends_with(['!', '?']) {
                s.push('.');
            }
            s
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A short clean phrase from a stored text field (one line, no final full stop, capped).
fn clean(text: &str, max_chars: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let one = one.trim();
    let cut = if one.chars().count() <= max_chars {
        one.to_string()
    } else {
        let cut: String = one.chars().take(max_chars).collect();
        match cut.rfind(' ') {
            Some(i) if i > max_chars / 2 => cut[..i].to_string(),
            _ => cut,
        }
    };
    // After the cut: a cut right after "dark trousers;" must not leave the ";" before the
    // caller's ", " ("dark trousers;, blue signature colour").
    cut.trim_end_matches(['.', ';', ',', ':', ' ']).to_string()
}

/// An angle in (-180, 180].
fn signed(a: f32) -> f32 {
    let r = a.rem_euclid(360.);
    let r = if r.is_finite() { r } else { 0. };
    if r > 180. {
        r - 360.
    } else {
        r
    }
}

/// Everything the sections share.
struct Ctx<'a> {
    shot: &'a Shot,
    scene: &'a Scene,
    bible: &'a BibleIndex,
    derived: &'a Derived,
    slots: &'a [RefSlot],
    place: Option<&'a Place>,
    /// The camera's view direction on the plan (yaw degrees).
    cam_yaw: f32,
    /// Visible characters, left to right.
    cast: Vec<&'a Mark>,
}

impl<'a> Ctx<'a> {
    fn name(&self, id: &str) -> String {
        match self.bible.character(id) {
            Some(c) if !c.name.trim().is_empty() => c.name.trim().to_string(),
            _ => id.replace('_', " "),
        }
    }

    fn prop_name(&self, id: &str) -> String {
        match self.bible.prop(id) {
            Some(p) if !p.name.trim().is_empty() => p.name.trim().to_string(),
            _ => id.replace('_', " "),
        }
    }

    fn place_name(&self) -> String {
        match self.place {
            Some(p) if !p.name.trim().is_empty() => p.name.trim().to_string(),
            Some(p) => p.id.replace('_', " "),
            None => "the place".into(),
        }
    }

    /// The shot's main subject: the focus character, the one looked at, else the first on screen.
    fn subject(&self) -> Option<String> {
        let in_shot = |id: &str| self.shot.marks.iter().any(|m| m.character == id);
        if let FocusTarget::Character(id) = &self.shot.camera.focus.target {
            if in_shot(id) {
                return Some(self.name(id));
            }
        }
        if let LookAt::Character(id) = &self.shot.camera.look_at {
            if in_shot(id) {
                return Some(self.name(id));
            }
        }
        self.cast.iter().find(|m| m.visible != Visibility::Ots).or(self.cast.first()).map(|m| self.name(&m.character))
    }

    /// Angle of a mark's facing away from facing the camera: 0 = facing it, ±180 = back to it;
    /// negative = turned toward screen right.
    fn facing_rel(&self, m: &Mark) -> f32 {
        signed(m.facing_deg - (self.cam_yaw + 180.))
    }

    /// 1-based image number of the first slot matching.
    fn image_of(&self, f: impl Fn(&RefSlot) -> bool) -> Option<usize> {
        self.slots.iter().position(f).map(|i| i + 1)
    }
}

/// The frame prompt of a shot; `slots` in upload order ("Image N" numbering).
pub fn frame_prompt(shot: &Shot, scene: &Scene, bible: &BibleIndex, derived: &Derived, slots: &[RefSlot]) -> Compiled {
    let place = scene.place.as_deref().and_then(|id| bible.place(id));
    let (cam_yaw, _) = geometry::look_dir(&shot.camera, &shot.marks, place);
    let cx = Ctx { shot, scene, bible, derived, slots, place, cam_yaw, cast: slots::on_screen(shot, derived) };
    let paragraphs = [style(&cx), pictures(&cx), camera(&cx), action(&cx), props(&cx), colour(&cx), keep(&cx), avoid(&cx)];
    let sections: Vec<(String, String)> = SECTIONS.iter().zip(paragraphs).filter(|(_, p)| !p.trim().is_empty()).map(|(n, p)| (n.to_string(), p)).collect();
    let text = sections.iter().map(|(_, p)| p.as_str()).collect::<Vec<_>>().join("\n\n");
    Compiled { text, sections }
}

/// 1. The style line, word for word, bound to the style frame.
fn style(cx: &Ctx<'_>) -> String {
    let line = cx.bible.style.line.trim();
    let line = if line.is_empty() { STYLE_FALLBACK } else { line };
    let mut out = clean(line, 2000);
    out.push('.');
    if let Some(n) = cx.image_of(|r| r.role == RefRole::Style) {
        out.push_str(&format!(" In the exact style of Image {n}."));
    }
    out
}

/// 2. What each picture is and what it may be used for, in upload order.
fn pictures(cx: &Ctx<'_>) -> String {
    cx.slots
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let (what, use_) = describe(cx, r);
            format!("Image {} is {what}: {use_}.", i + 1)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// (what the picture is, what it may be used for).
fn describe(cx: &Ctx<'_>, r: &RefSlot) -> (String, String) {
    let stem = file_stem(&r.file);
    let place = cx.place_name();
    match &r.role {
        RefRole::Plate => {
            let plate = cx.place.and_then(|p| p.plates.iter().find(|pl| pl.card.selected.as_deref() == Some(stem)).map(|pl| (p, pl)));
            if let Some((p, pl)) = plate {
                let station = p.stations.iter().find(|s| s.id == pl.station).map(|s| if s.name.trim().is_empty() { s.id.clone() } else { s.name.trim().to_string() }).unwrap_or_else(|| pl.station.clone());
                let state = p.states.iter().find(|s| s.id == pl.state).map(|s| if s.name.trim().is_empty() { s.id.clone() } else { s.name.trim().to_string() }).unwrap_or_else(|| pl.state.clone());
                (format!("the background plate of {place} from the camera position \"{station}\" in {state} light"), "keep the room, the layout and the camera exactly".into())
            } else if cx.place.is_some_and(|p| p.sheet.selected.as_deref() == Some(stem)) {
                (format!("the environment sheet of {place}"), "keep its architecture, furniture, materials and colours, and place the camera as described below".into())
            } else {
                ("the background".into(), "keep the room, the layout and the camera exactly".into())
            }
        }
        RefRole::Identity(id) => {
            let n = cx.name(id);
            let sheet = cx.bible.character(id).and_then(|c| c.sheets.iter().find(|s| s.card.selected.as_deref() == Some(stem)));
            let crop = cx.bible.character(id).is_some_and(|c| c.crops.iter().any(|cr| cr.file == r.file));
            match sheet.map(|s| &s.kind) {
                Some(SheetKind::Face) => (format!("{n}'s face sheet"), format!("use it only for {n}'s face and hair")),
                Some(SheetKind::Back) => (format!("{n}'s back view"), format!("use it only for how {n} looks from behind")),
                Some(SheetKind::Turnaround) => (format!("{n}'s turnaround sheet"), format!("use it only for {n}'s body, hair and outfit, not for the pose")),
                Some(SheetKind::Outfit(o)) => {
                    let outfit = cx.bible.character(id).and_then(|c| c.outfits.iter().find(|x| &x.id == o)).map(|x| clean(if x.name.trim().is_empty() { &x.description } else { &x.name }, 60)).filter(|s| !s.is_empty());
                    let what = match outfit {
                        Some(o) => format!("{n}'s outfit sheet ({o})"),
                        None => format!("{n}'s outfit sheet"),
                    };
                    (what, format!("use it only for {n}'s body and outfit, not for the pose"))
                }
                Some(_) => (format!("{n}'s character sheet"), format!("use it only for {n}'s look, not for the pose")),
                None if crop => (format!("a drawing of {n} from the manga"), format!("use it only for {n}'s face, hair and outfit, not for its style")),
                None => (format!("{n}'s reference"), format!("use it only for {n}'s look, not for the pose")),
            }
        }
        RefRole::Style => ("the style frame".into(), "use it only for line, shading and colour style, not for its characters or place".into()),
        RefRole::Expression(id) => {
            let n = cx.name(id);
            let feel = cx.shot.marks.iter().find(|m| &m.character == id).map(|m| emotion_adjective(m.emotion.kind)).unwrap_or("calm");
            (format!("{n}'s expression sheet"), format!("use only the {feel} face from it"))
        }
        RefRole::Grip(p) => {
            let holder = cx
                .shot
                .marks
                .iter()
                .find(|m| m.holding.iter().any(|h| &h.prop == p))
                .map(|m| cx.name(&m.character))
                .or_else(|| cx.bible.prop(p).and_then(|x| x.owner.clone()).map(|o| cx.name(&o)))
                .unwrap_or_else(|| "the character".into());
            (format!("how {holder} holds the {}", cx.prop_name(p)), "use it only for the grip and the hand".into())
        }
        RefRole::Prop(p) => {
            let n = cx.prop_name(p);
            (format!("the {n}"), "use it only for its shape, colours and size".into())
        }
        RefRole::ColourModel(id) => {
            let n = cx.name(id);
            (format!("{n}'s colour model for this light"), format!("use it only for {n}'s colours"))
        }
        RefRole::PlaceView if cx.shot.background == BackgroundKind::Backdrop => {
            (place.clone(), "use only its colours and materials for a simple backdrop, not its layout".into())
        }
        RefRole::PlaceView => (format!("another view of {place}"), "use it only to keep the room the same, not for the camera".into()),
        RefRole::Panel => ("the manga panel this shot adapts".into(), "use it only for the pose and the composition, not for its style, lines or text".into()),
        RefRole::Layout => ("the layout".into(), "keep its camera, perspective, horizon and every object's position".into()),
        RefRole::User | RefRole::Unknown => {
            let job = clean(&r.job, 120);
            if job.is_empty() {
                ("an extra reference".into(), "use it as a loose guide only".into())
            } else {
                ("an extra reference".into(), format!("use it for {job}"))
            }
        }
    }
}

/// 3. Shot size, angle, lens, camera height and horizon, focus, exposure, background.
fn camera(cx: &Ctx<'_>) -> String {
    let c = &cx.shot.camera;
    let subject = cx.subject();
    let place = cx.place_name();
    let subj = subject.clone().unwrap_or_else(|| place.clone());
    let names: Vec<String> = cx.cast.iter().map(|m| cx.name(&m.character)).collect();

    let thing = match &c.focus.target {
        FocusTarget::Prop(p) => format!("the {}", cx.prop_name(p)),
        _ => match cx.cast.iter().flat_map(|m| m.holding.iter()).next() {
            Some(h) => format!("the {}", cx.prop_name(&h.prop)),
            None => match &subject {
                Some(s) => format!("{s}'s hands"),
                None => "the detail that matters".into(),
            },
        },
    };
    let (a, b) = match c.size {
        ShotSize::Ots => {
            let fg = cx.cast.iter().position(|m| matches!(m.visible, Visibility::Ots | Visibility::Back)).unwrap_or(0);
            let other = (0..names.len()).find(|i| *i != fg);
            (names.get(fg).cloned(), other.and_then(|i| names.get(i).cloned()))
        }
        _ => (names.first().cloned(), names.get(1).cloned()),
    };
    let size_t = lookup(bank::SIZE, c.size).unwrap_or(bank::SIZE_FALLBACK);
    let needs_two = matches!(c.size, ShotSize::Ots | ShotSize::TwoShot);
    let size_t = if needs_two && b.is_none() { bank::SIZE_FALLBACK } else { size_t };
    // Nobody on screen: a framing made for a person frames the place instead (never "a
    // close-up of the hospital room's face").
    let size_t = match c.size {
        _ if !cx.cast.is_empty() => size_t,
        ShotSize::Mcu | ShotSize::Cu | ShotSize::Ecu => bank::EMPTY_CLOSE,
        ShotSize::Pov => bank::EMPTY_POV,
        ShotSize::Fs | ShotSize::Cowboy | ShotSize::Ms | ShotSize::Ots | ShotSize::TwoShot => bank::EMPTY_VIEW,
        _ => size_t,
    };
    let size = fill(size_t, &[("subject", &subj), ("place", &place), ("thing", &thing), ("a", a.as_deref().unwrap_or("")), ("b", b.as_deref().unwrap_or(""))]);

    // The camera as it really stands: on a station its height and pitch are the station's, and
    // the angle words follow them; when the background is that station's plate (painted with
    // the station's lens), the station's lens is the one described.
    let (_, pitch) = geometry::look_dir(c, &cx.shot.marks, cx.place);
    let placed = c.pos.is_some() || c.station.as_deref().is_some_and(|id| cx.place.is_some_and(|p| p.stations.iter().any(|s| s.id == id)));
    let angle_kind = if placed { geometry::real_angle(c.angle, geometry::cam_pos(c, cx.place)[1], pitch) } else { c.angle };
    let lens_mm = plate_station(cx).map(|s| s.lens_mm).filter(|l| *l > 0).unwrap_or(c.lens_mm);
    let mut derived = cx.derived.clone();
    if lens_mm != c.lens_mm {
        derived.horizon_y = geometry::horizon_y(pitch, lens_mm as f32);
    }
    let words = geometry::wording_in(&derived, c, cx.place);
    let roll = if c.roll_deg.is_finite() && c.roll_deg.abs() >= 1. { c.roll_deg.abs() } else { bank::DUTCH_DEFAULT_DEG };
    let angle = lookup(bank::ANGLE, angle_kind).map(|t| fill(t, &[("subject", &subj), ("roll", &format!("{roll:.0}"))])).unwrap_or_default();

    let mut parts = vec![size, angle, lens_phrase(lens_mm), format!("{}, {}", words.horizon, words.perspective)];
    if c.angle != Angle::Dutch && !words.roll.is_empty() {
        parts.push(format!("the frame {}, the horizon at an angle", words.roll));
    }

    // Focus: a pull is made later, so the still keeps every layer sharp.
    let target = match &c.focus.target {
        FocusTarget::Character(id) => Some(format!("{}'s eyes", cx.name(id))),
        FocusTarget::Prop(p) => Some(format!("the {}", cx.prop_name(p))),
        FocusTarget::Background => Some("the background".into()),
        FocusTarget::None => None,
        _ => subject.as_ref().map(|s| format!("{s}'s eyes")),
    };
    let rack = c.focus.rack_to.is_some() || c.movement.kind == Move::RackFocus;
    let focus = if rack {
        bank::RACK.to_string()
    } else {
        match (c.depth, target) {
            (Depth::Deep, _) => lookup(bank::DEPTH, Depth::Deep).unwrap_or_default().to_string(),
            (d, Some(t)) => lookup(bank::DEPTH, d).map(|x| fill(x, &[("target", &t)])).unwrap_or_default(),
            (_, None) => String::new(),
        }
    };
    parts.push(focus);
    parts.push(lookup(bank::KEY, c.key).unwrap_or_default().to_string());

    let has_plate = cx.slots.iter().any(|r| r.role == RefRole::Plate || r.role == RefRole::Layout);
    let bg = match cx.shot.background {
        BackgroundKind::Matched if !has_plate && cx.place.is_some() => fill(bank::SETTING, &[("place", &place)]),
        BackgroundKind::Backdrop if cx.place.is_some() => fill(bank::BACKDROP, &[("place", &place)]),
        BackgroundKind::Backdrop => bank::BACKDROP_PLAIN.into(),
        BackgroundKind::None => bank::NO_BACKGROUND.into(),
        _ => String::new(),
    };
    parts.push(bg);
    sentences(&parts)
}

/// The shot's own station, when the background reference is that station's plate.
fn plate_station<'a>(cx: &Ctx<'a>) -> Option<&'a Station> {
    let place = cx.place?;
    let r = cx.slots.iter().find(|r| r.role == RefRole::Plate)?;
    let stem = file_stem(&r.file);
    let plate = place.plates.iter().find(|pl| pl.card.selected.as_deref() == Some(stem))?;
    let here = slots::station_for(place, cx.shot, cx.derived)?;
    (here.id == plate.station).then_some(here)
}

/// The costume cue of a mark: its outfit and the character's signature colour.
fn costume(cx: &Ctx<'_>, m: &Mark) -> Option<String> {
    let c = cx.bible.character(&m.character)?;
    let outfit = m
        .outfit
        .as_deref()
        .and_then(|o| c.outfits.iter().find(|x| x.id == o))
        .map(|x| if x.name.trim().is_empty() { x.description.clone() } else { x.name.clone() })
        .unwrap_or_else(|| c.look.default_outfit.clone());
    let outfit = clean(&outfit, 60);
    let colour = colour_name(&c.look.signature_colour).map(|w| format!("{w} signature colour"));
    let parts: Vec<String> = [Some(outfit).filter(|o| !o.is_empty()), colour].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// 4. Who does what, left to right, with screen side and a costume cue; what is behind them.
fn action(cx: &Ctx<'_>) -> String {
    if cx.cast.is_empty() {
        return "No characters in this shot.".into();
    }
    let mut out = Vec::new();
    for m in &cx.cast {
        let name = cx.name(&m.character);
        let head = match costume(cx, m) {
            Some(c) => format!("{name} ({c})"),
            None => name.clone(),
        };
        let mut bits: Vec<String> = Vec::new();
        if let Some(x) = cx.derived.screen_x.get(&m.character).copied().filter(|x| x.is_finite()) {
            let (l, r) = bank::THIRDS;
            bits.push(if x < l { bank::LEFT } else if x > r { bank::RIGHT } else { bank::CENTRE }.into());
        }
        match m.visible {
            Visibility::Ots => bits.push(bank::OTS.into()),
            Visibility::Back => bits.push(bank::BACK.into()),
            v => {
                let rel = cx.facing_rel(m);
                let side = if rel < 0. { "right" } else { "left" };
                let t = bank::FACING.iter().find(|(max, _)| rel.abs() <= *max).map(|(_, t)| *t).unwrap_or(bank::BACK);
                bits.push(fill(t, &[("side", side)]));
                if v == Visibility::Partial {
                    bits.push(bank::PARTIAL.into());
                }
            }
        }
        if let Some(p) = lookup(bank::POSE, m.pose) {
            bits.push(p.into());
        }
        let act = clean(&m.action, 160);
        if !act.is_empty() {
            bits.push(act);
        }
        match m.gaze.as_deref().map(str::trim) {
            Some("camera") => bits.push("looking straight into the camera".into()),
            Some(g) if g.starts_with("char:") => bits.push(format!("looking at {}", cx.name(&g[5..]))),
            Some(g) if g.starts_with("landmark:") => {
                let id = &g[9..];
                let n = cx.place.and_then(|p| p.plan.landmarks.iter().find(|l| l.id == id)).map(|l| l.name.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| id.replace('_', " "));
                bits.push(format!("looking at the {n}"));
            }
            _ => {}
        }
        if !matches!(m.visible, Visibility::Back | Visibility::Ots) {
            bits.push(format!("expression: {}", emotion_words(m.emotion.kind, m.emotion.strength)));
        }
        out.push(format!("{head} {}", bits.join(", ")));
    }
    // What stands behind each character (matched rooms only).
    if cx.shot.background == BackgroundKind::Matched {
        if let Some(place) = cx.place {
            for m in &cx.cast {
                let Some(ids) = cx.derived.behind.get(&m.character) else { continue };
                let names: Vec<String> = ids
                    .iter()
                    .take(2)
                    .map(|id| place.plan.landmarks.iter().find(|l| &l.id == id).map(|l| l.name.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| id.replace('_', " ")))
                    .map(|n| format!("the {n}"))
                    .collect();
                if !names.is_empty() {
                    out.push(format!("behind {}: {}", cx.name(&m.character), join_and(&names)));
                }
            }
        }
    }
    if matches!(cx.shot.mouth, MouthPolicy::Closed | MouthPolicy::Flap) && cx.cast.iter().any(|m| !matches!(m.visible, Visibility::Back | Visibility::Ots)) {
        out.push("mouths closed".into());
    }
    if slots::is_crowded(cx.shot) {
        let with: BTreeSet<&str> = cx.slots.iter().filter_map(|r| if let RefRole::Identity(id) = &r.role { Some(id.as_str()) } else { None }).collect();
        let named: Vec<String> = cx.cast.iter().filter(|m| with.contains(m.character.as_str())).map(|m| cx.name(&m.character)).collect();
        if !named.is_empty() {
            out.push(format!("only {} have reference pictures; keep the others simple and further back", join_and(&named)));
        }
    }
    sentences(&out)
}

/// 5. Props and which hand, with the screen side of that hand when the body shows it.
fn props(cx: &Ctx<'_>) -> String {
    let mut out = Vec::new();
    for m in &cx.cast {
        let name = cx.name(&m.character);
        let rel = if matches!(m.visible, Visibility::Back | Visibility::Ots) { 180. } else { cx.facing_rel(m).abs() };
        for h in &m.holding {
            let prop = cx.prop_name(&h.prop);
            let line = match h.hand {
                Hand::Both => format!("{name} holds the {prop} in both hands, fingers wrapped around it"),
                hand => {
                    let right = hand != Hand::Left;
                    let word = if right { "right" } else { "left" };
                    // Facing the camera, a right hand is on screen left; from behind, on screen right.
                    let screen = if rel <= 60. {
                        Some(if right { "left" } else { "right" })
                    } else if rel >= 120. {
                        Some(if right { "right" } else { "left" })
                    } else {
                        None
                    };
                    match screen {
                        Some(s) => format!("{name} holds the {prop} in the {word} hand (on screen {s}), fingers wrapped around it"),
                        None => format!("{name} holds the {prop} in the {word} hand, fingers wrapped around it"),
                    }
                }
            };
            out.push(line);
        }
    }
    sentences(&out)
}

/// The key light's direction on screen.
fn key_direction(cx: &Ctx<'_>) -> Option<&'static str> {
    match cx.shot.camera.light {
        LightSetup::Backlit | LightSetup::Silhouette | LightSetup::Rim => return Some(bank::FROM_BEHIND),
        LightSetup::UnderLight => return Some(bank::FROM_BELOW),
        LightSetup::Top => return Some(bank::FROM_ABOVE),
        _ => {}
    }
    let state = slots::state_for(cx.place?, cx.scene.time_of_day)?;
    // `key_dir_deg` is the plan direction the key light comes from (yaw, 0 = north).
    let rel = signed(state.key_dir_deg - cx.cam_yaw);
    Some(if rel.abs() <= 45. {
        bank::FROM_BEHIND
    } else if rel.abs() >= 135. {
        bank::FROM_FRONT
    } else if rel > 0. {
        bank::FROM_RIGHT
    } else {
        bank::FROM_LEFT
    })
}

/// 6. Colour key, palette, time of day, light setup, key light colour and direction.
fn colour(cx: &Ctx<'_>) -> String {
    let mut out = Vec::new();
    let theme = clean(&cx.scene.colour.name, 80);
    if !theme.is_empty() {
        out.push(format!("colour key: {theme}"));
    }
    let palette = cx.scene.colour.palette.as_deref().and_then(|id| cx.bible.style.palettes.iter().find(|p| p.id == id));
    if let Some(p) = palette {
        let hex = |role: &str| p.swatches.iter().find(|s| s.role.eq_ignore_ascii_case(role)).map(|s| s.hex.trim().to_string()).filter(|h| !h.is_empty());
        let named = |h: String| match colour_name(&h) {
            Some(w) => format!("{h} ({w})"),
            None => h,
        };
        let mut items: Vec<String> = bank::SWATCHES.iter().filter_map(|(role, word)| hex(role).map(|h| format!("{} for the {word}", named(h)))).collect();
        if let Some(a) = hex("accent") {
            items.push(format!("a single {} accent", named(a)));
        }
        if !items.is_empty() {
            out.push(format!("colour palette limited to {}", join_and(&items)));
        }
        if p.brightness > 0. && p.brightness < 0.35 {
            out.push("dark overall".into());
        } else if p.brightness > 0.65 {
            out.push("bright overall".into());
        }
        if p.saturation > 0. && p.saturation < 0.35 {
            out.push("muted, desaturated colours".into());
        } else if p.saturation > 0.65 {
            out.push("vivid, saturated colours".into());
        }
    }
    if let Some(t) = lookup(bank::TIME, cx.scene.time_of_day) {
        out.push(t.into());
    }
    if let Some(l) = lookup(bank::LIGHT, cx.shot.camera.light) {
        out.push(l.into());
    }
    let state = cx.place.and_then(|p| slots::state_for(p, cx.scene.time_of_day));
    let kelvin = state.map(|s| s.kelvin).filter(|k| *k > 0);
    let warmth = kelvin.and_then(|k| bank::KELVIN.iter().find(|(max, _)| k <= *max).map(|(_, w)| format!("{w} ({k} K)")));
    match (warmth, key_direction(cx)) {
        (Some(w), Some(d)) => out.push(format!("key light: {w}, {d}")),
        (Some(w), None) => out.push(format!("key light: {w}")),
        (None, Some(d)) => out.push(format!("key light {d}")),
        (None, None) => {}
    }
    if (matches!(cx.scene.time_of_day, TimeOfDay::Night | TimeOfDay::Dusk) || cx.shot.camera.key == Key::Dark) && !cx.cast.is_empty() {
        out.push(bank::NIGHT_COLOURS.into());
    }
    sentences(&out)
}

/// 7. Each character's must-have features (at most 5).
fn keep(cx: &Ctx<'_>) -> String {
    let mut out = Vec::new();
    for m in &cx.cast {
        let Some(c) = cx.bible.character(&m.character) else { continue };
        let mut items: Vec<String> = c.look.keep.iter().map(|k| clean(k, 80)).filter(|k| !k.is_empty()).take(5).collect();
        if items.is_empty() {
            items = [&c.look.hair, &c.look.eyes].iter().map(|k| clean(k, 60)).filter(|k| !k.is_empty()).collect();
        }
        if !items.is_empty() {
            out.push(format!("keep {}'s {}", cx.name(&c.id), join_and(&items)));
        }
    }
    sentences(&out)
}

/// 8. The closing line: frame shape, no text or extras, the style's own negatives; the style
/// repeated when a failed style check moved the style frame to Image 1.
fn avoid(cx: &Ctx<'_>) -> String {
    let aspect = cx.shot.frame.gen.aspect.trim();
    let aspect = if aspect.is_empty() { "16:9" } else { aspect };
    let mut out = format!("{aspect} frame. {CLOSING}");
    let negatives: Vec<String> = cx.bible.style.negative.iter().map(|n| clean(n, 60)).filter(|n| !n.is_empty()).collect();
    if !negatives.is_empty() {
        out.push_str(&format!(" Avoid {}.", negatives.join(", ")));
    }
    if cx.slots.first().is_some_and(|r| r.role == RefRole::Style) {
        out.push_str(" Once more: in the exact style of Image 1.");
    }
    out
}
