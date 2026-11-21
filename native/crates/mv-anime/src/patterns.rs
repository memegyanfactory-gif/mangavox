//! Coverage patterns (arrival, conversation, threat reveal, ...) and the decision-table
//! defaults the director and the Shots screen's fixes use (design 6.3, 9.7).
//!
//! Every pattern is data: an ordered list of steps (size, angle, who is in frame and how they
//! are seen, what the shot tells, how long it holds), taken from two measured TV episodes
//! (Solo Leveling 1 and 2, reports 21, 22 and 25) and the professional's film (plan rules
//! R1-R8). The planner prompt reads them as text ([`rules_text`]); the director's fallback
//! plan and the Shots screen's "apply a pattern" build shots from them ([`template`]).
//!
//! Characters handed to a pattern are, in order: the focal character (the hero, the one whose
//! head we are in), the other one (the threat, or the one they talk to), then everyone else
//! (in a sudden death: the victim).

use std::fmt::Write as _;

use crate::model::{
    Angle, BackgroundKind, Camera, Depth, Emotion, Grain, Key, LightSetup, LookAt, Mark, MotionKind, MouthPolicy, Move, Movement, Origin, Pattern, Pose, Scene, SceneKind,
    Shot, ShotSize, Speed, Tone, Transition, Visibility,
};

/// The lenses a shot may use, widest first (millimetres, full-frame equivalent).
pub const LENSES: [u16; 6] = [14, 24, 35, 50, 85, 135];

/// Every pattern, in the order the prompt lists them.
pub const ALL: [Pattern; 7] = [Pattern::Arrival, Pattern::Conversation, Pattern::ThreatReveal, Pattern::Stunned, Pattern::Reveal, Pattern::SuddenDeath, Pattern::Chase];

/// One shot of a pattern.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Step {
    pub size: ShotSize,
    /// `None`: the decision table's angle for the size.
    pub angle: Option<Angle>,
    /// How the focal character is seen; `None`: not in the frame.
    pub focal: Option<Visibility>,
    /// How the other character is seen; `None`: not in the frame.
    pub other: Option<Visibility>,
    /// How everyone else is seen; `None`: not in the frame.
    pub rest: Option<Visibility>,
    pub pose: Pose,
    /// The one thing the shot tells.
    pub tells: &'static str,
    pub seconds: f32,
    pub motion: MotionKind,
    /// `None`: the decision table's move for the size.
    pub mv: Option<Move>,
    pub background: BackgroundKind,
    pub hit: bool,
    pub flash: bool,
    pub bridge: bool,
    /// The step carries dialogue when there is some.
    pub talk: bool,
}

/// A step with the usual values; the tables below change only what differs.
/// A generated sentence as shown: its first letter a capital ("the floor from above" -> "The
/// floor from above"); leading spaces dropped, the rest as written.
pub fn capitalised(s: &str) -> String {
    let s = s.trim_start();
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

const fn step(size: ShotSize, tells: &'static str, seconds: f32) -> Step {
    Step {
        size,
        angle: None,
        focal: None,
        other: None,
        rest: None,
        pose: Pose::Stand,
        tells,
        seconds,
        motion: MotionKind::Hold,
        mv: None,
        background: BackgroundKind::Backdrop,
        hit: false,
        flash: false,
        bridge: false,
        talk: false,
    }
}

const FULL: Option<Visibility> = Some(Visibility::Full);
const PART: Option<Visibility> = Some(Visibility::Partial);
const BACK: Option<Visibility> = Some(Visibility::Back);
const OTS: Option<Visibility> = Some(Visibility::Ots);

/// A pattern as data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PatternDef {
    pub pattern: Pattern,
    pub name: &'static str,
    /// When the director reaches for it.
    pub when: &'static str,
    /// Its measured shape in one sentence.
    pub shape: &'static str,
    /// Which characters it wants, in order.
    pub cast: &'static str,
    pub steps: &'static [Step],
}

const ARRIVAL: &[Step] = &[
    Step { angle: Some(Angle::High), motion: MotionKind::CameraOnly, mv: Some(Move::PanRight), background: BackgroundKind::Matched, ..step(ShotSize::Els, "where we are: the whole place from high and far, nobody readable yet", 4.5) },
    Step { angle: Some(Angle::Eye), focal: FULL, other: FULL, rest: FULL, background: BackgroundKind::Matched, ..step(ShotSize::Ls, "the way in: the people small in the place, the landmark ahead", 3.0) },
    Step { angle: Some(Angle::High), ..step(ShotSize::Insert, "the landmark the scene turns on, close: the gate, the door, the table", 1.8) },
    Step { focal: FULL, other: FULL, rest: FULL, motion: MotionKind::Partial, background: BackgroundKind::Matched, talk: true, ..step(ShotSize::Fs, "everyone at the landmark, each on their own side; the talk starts here", 5.0) },
];

const CONVERSATION: &[Step] = &[
    Step { focal: FULL, other: FULL, rest: FULL, motion: MotionKind::Partial, background: BackgroundKind::Matched, talk: true, ..step(ShotSize::Ls, "the master: both speakers full figure, each on their own side, the place behind", 6.0) },
    Step { focal: OTS, other: FULL, motion: MotionKind::MouthOnly, talk: true, ..step(ShotSize::Ots, "over the focal character's shoulder onto the other: the other speaks", 4.0) },
    Step { focal: FULL, other: OTS, motion: MotionKind::MouthOnly, talk: true, ..step(ShotSize::Ots, "the reverse: over the other's shoulder onto the focal character", 4.0) },
    Step { other: FULL, motion: MotionKind::MouthOnly, talk: true, ..step(ShotSize::Ms, "the other alone, chest up, looking room toward the focal character", 3.5) },
    Step { focal: PART, ..step(ShotSize::Insert, "a hand: the feeling carried without the face", 2.0) },
    Step { other: FULL, talk: true, ..step(ShotSize::Mcu, "the button: a reaction held, mouth closed while the line plays", 2.5) },
];

const THREAT_REVEAL: &[Step] = &[
    Step { angle: Some(Angle::Eye), focal: BACK, rest: BACK, motion: MotionKind::CameraOnly, mv: Some(Move::PushIn), background: BackgroundKind::Matched, ..step(ShotSize::Ls, "a slit of light: the group as silhouettes at the threshold", 3.0) },
    Step { angle: Some(Angle::High), focal: FULL, other: FULL, rest: FULL, motion: MotionKind::CameraOnly, mv: Some(Move::TiltDown), background: BackgroundKind::Matched, ..step(ShotSize::Els, "the whole space from high: where the threat waits and how small they are", 5.0) },
    Step { angle: Some(Angle::Low), focal: PART, ..step(ShotSize::Insert, "feet step in", 2.5) },
    Step { focal: FULL, rest: FULL, motion: MotionKind::Partial, ..step(ShotSize::Ms, "they look up", 2.5) },
    Step { angle: Some(Angle::Top), focal: FULL, other: FULL, rest: FULL, background: BackgroundKind::Matched, ..step(ShotSize::Overhead, "the floor as a map: who stands where", 2.5) },
    Step { angle: Some(Angle::Low), other: FULL, motion: MotionKind::CameraOnly, mv: Some(Move::TiltUp), background: BackgroundKind::Matched, ..step(ShotSize::Ls, "the threat from below, too big for the frame", 3.0) },
    Step { angle: Some(Angle::Low), other: PART, background: BackgroundKind::None, ..step(ShotSize::Cu, "the threat's face, in parts", 3.0) },
    Step { angle: Some(Angle::High), focal: FULL, other: PART, rest: FULL, motion: MotionKind::CameraOnly, mv: Some(Move::PullOut), background: BackgroundKind::Matched, talk: true, ..step(ShotSize::Els, "over the threat's shoulder: the group tiny below", 6.0) },
    Step { other: PART, background: BackgroundKind::None, ..step(ShotSize::Ecu, "its eye", 1.0) },
];

const STUNNED: &[Step] = &[
    Step { focal: PART, motion: MotionKind::CameraOnly, mv: Some(Move::PushIn), background: BackgroundKind::None, ..step(ShotSize::Mcu, "inside the head: the eyes cut off by the frame or hidden by hair, slow push, sound dipped", 3.5) },
    Step { other: FULL, rest: FULL, motion: MotionKind::Partial, ..step(ShotSize::Pov, "what they see, slowed, the sound muffled", 3.0) },
    Step { focal: PART, background: BackgroundKind::None, ..step(ShotSize::Ecu, "the eyes, pupils small, held still", 1.5) },
    Step { other: FULL, rest: FULL, motion: MotionKind::Partial, ..step(ShotSize::Pov, "what they see, again", 2.0) },
    Step { focal: FULL, other: FULL, rest: FULL, motion: MotionKind::Full, background: BackgroundKind::Matched, ..step(ShotSize::Els, "the hard cut out: a loud, clear wide", 3.0) },
];

const REVEAL: &[Step] = &[
    Step { focal: BACK, ..step(ShotSize::Ms, "promise: the hero's back, facing what is coming", 4.0) },
    Step { focal: PART, ..step(ShotSize::Insert, "promise: a hand, a detail; the full view withheld", 3.0) },
    Step { angle: Some(Angle::Low), other: PART, motion: MotionKind::CameraOnly, mv: Some(Move::PushIn), background: BackgroundKind::None, ..step(ShotSize::Cu, "promise: a part of what will be revealed", 3.0) },
    Step { flash: true, background: BackgroundKind::None, ..step(ShotSize::Insert, "trigger: a flash or a hit", 0.5) },
    Step { other: PART, background: BackgroundKind::None, ..step(ShotSize::Ecu, "parts: one detail of it", 2.5) },
    Step { angle: Some(Angle::Low), focal: FULL, other: FULL, rest: FULL, motion: MotionKind::CameraOnly, mv: Some(Move::TiltUp), background: BackgroundKind::Matched, talk: true, ..step(ShotSize::Els, "the full view, held (5 s, up to 12 s with lines)", 5.0) },
    Step { focal: FULL, motion: MotionKind::Partial, talk: true, ..step(ShotSize::Ms, "the body reacts", 2.5) },
];

const SUDDEN_DEATH: &[Step] = &[
    Step { rest: FULL, motion: MotionKind::Partial, talk: true, ..step(ShotSize::Ms, "the victim breaks the rule", 3.5) },
    Step { angle: Some(Angle::Low), other: PART, rest: PART, ..step(ShotSize::Insert, "feet passing the threat's foot", 1.0) },
    Step { focal: FULL, other: PART, rest: FULL, motion: MotionKind::Partial, background: BackgroundKind::Matched, talk: true, ..step(ShotSize::Fs, "the warning: hero and threat in one frame", 2.0) },
    Step { focal: PART, background: BackgroundKind::None, ..step(ShotSize::Ecu, "the witness snaps: eyes open", 0.5) },
    Step { motion: MotionKind::Full, background: BackgroundKind::None, hit: true, flash: true, ..step(ShotSize::Insert, "the impact as an abstraction: a spark, a white-out, red fog; no blade on screen", 0.4) },
    Step { rest: PART, background: BackgroundKind::Matched, ..step(ShotSize::Ls, "the result, held in silence", 6.0) },
    Step { angle: Some(Angle::High), rest: PART, ..step(ShotSize::Insert, "what is left: blood, a shoe", 3.0) },
    Step { focal: FULL, ..step(ShotSize::Ms, "the witnesses frozen", 1.5) },
    Step { angle: Some(Angle::Top), focal: FULL, other: FULL, rest: FULL, background: BackgroundKind::Matched, ..step(ShotSize::Overhead, "re-establish: the survivors from above", 4.0) },
];

const CHASE: &[Step] = &[
    Step { angle: Some(Angle::Low), motion: MotionKind::CameraOnly, mv: Some(Move::PanRight), background: BackgroundKind::Matched, ..step(ShotSize::Els, "where the chase runs", 3.0) },
    Step { other: FULL, pose: Pose::Run, motion: MotionKind::Full, ..step(ShotSize::Ms, "the one who flees", 1.5) },
    Step { other: FULL, pose: Pose::Run, motion: MotionKind::Full, background: BackgroundKind::Matched, ..step(ShotSize::Ls, "running at the lens: movement along the lens axis", 1.0) },
    Step { focal: FULL, pose: Pose::Run, motion: MotionKind::Full, ..step(ShotSize::Ms, "the pursuer", 1.5) },
    Step { angle: Some(Angle::Low), focal: BACK, other: FULL, pose: Pose::Run, motion: MotionKind::Full, background: BackgroundKind::Matched, ..step(ShotSize::Ls, "down the axis: pursuer and quarry in one frame", 1.5) },
    Step { motion: MotionKind::Full, background: BackgroundKind::None, hit: true, flash: true, ..step(ShotSize::Insert, "the catch: the impact", 0.6) },
    Step { angle: Some(Angle::Top), focal: FULL, other: FULL, rest: FULL, background: BackgroundKind::Matched, ..step(ShotSize::Overhead, "the aftermath from above", 3.5) },
];

/// Every pattern's definition.
pub fn defs() -> [PatternDef; 7] {
    [
        PatternDef {
            pattern: Pattern::Arrival,
            name: "arrival",
            when: "a scene opens in a new place",
            shape: "a funnel of 2-4 shots, each closer than the last; the top one held 4-7 s; the layout by cut 2 and within 6 s",
            cast: "anyone present",
            steps: ARRIVAL,
        },
        PatternDef {
            pattern: Pattern::Conversation,
            name: "conversation",
            when: "two or more people talk",
            shape: "a master that shows who is where, then over-the-shoulder pairs and chest-up singles; each person keeps their screen side; back to the master at least every 12 cuts or 30-35 s",
            cast: "the focal speaker, the one they talk to, then the others",
            steps: CONVERSATION,
        },
        PatternDef {
            pattern: Pattern::ThreatReveal,
            name: "threat_reveal",
            when: "a big space and the threat in it are shown for the first time",
            shape: "light at the threshold, the whole space from high, feet entering, the threat from below, its face in parts, the group tiny from over its shoulder",
            cast: "the hero, the threat, then the group",
            steps: THREAT_REVEAL,
        },
        PatternDef {
            pattern: Pattern::Stunned,
            name: "stunned",
            when: "a character is overwhelmed by what they see (inside their head)",
            shape: "eyes cropped or hidden, slow push, intercut with what they see (point of view), sound dipped 15-30 dB for 2-8 s; out by a hard cut onto a loud wide",
            cast: "the one who is stunned, what they see",
            steps: STUNNED,
        },
        PatternDef {
            pattern: Pattern::Reveal,
            name: "reveal",
            when: "a power, a weapon, a creature or a place is shown in full for the first time",
            shape: "promise 3-5 cuts over 12-26 s that withhold the full view; a trigger (flash or hit); 1-3 parts of 2-6 s; the full wide held 3.5-12 s (5 s by default); a body reaction 1.5-5 s",
            cast: "the hero, what is revealed",
            steps: REVEAL,
        },
        PatternDef {
            pattern: Pattern::SuddenDeath,
            name: "sudden_death",
            when: "someone is killed without warning",
            shape: "a warning shout 0.3-2 s before; one witness snap of 0.25-0.7 s; the impact as an abstraction under 1 s; the result held 4-10 s in silence; the first spoken reaction 5-18 s later; then re-establish from above",
            cast: "the witness (hero), the threat, then the victim",
            steps: SUDDEN_DEATH,
        },
        PatternDef {
            pattern: Pattern::Chase,
            name: "chase",
            when: "someone runs from or after someone",
            shape: "where it runs, the runners staged toward or away from the lens, both in one frame down the axis before the catch, the aftermath from above",
            cast: "the pursuer, the one who flees",
            steps: CHASE,
        },
    ]
}

/// One pattern's definition.
pub fn def(pattern: Pattern) -> Option<PatternDef> {
    defs().into_iter().find(|d| d.pattern == pattern)
}

/// The snake_case word of an enum value (its serde name).
fn word<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

fn seen(v: Option<Visibility>, who: &str) -> Option<String> {
    v.map(|v| match v {
        Visibility::Full => who.to_string(),
        Visibility::Partial => format!("part of {who}"),
        Visibility::Back => format!("{who} from behind"),
        Visibility::Ots => format!("{who}'s shoulder in the foreground"),
        _ => format!("{who} off screen"),
    })
}

/// Every pattern, and the smaller measured templates, as rules for the planner prompt.
pub fn rules_text() -> String {
    let mut o = String::new();
    let _ = writeln!(o, "COVERAGE PATTERNS (reusable shot sequences measured on real TV anime; name the one a shot belongs to in \"pattern\", or null):");
    for d in defs() {
        let _ = writeln!(o, "- {} ({}): {}. Characters: {}.", d.name, d.when, d.shape, d.cast);
        for (i, s) in d.steps.iter().enumerate() {
            let who: Vec<String> = [seen(s.focal, "focal"), seen(s.other, "other"), seen(s.rest, "the rest")].into_iter().flatten().collect();
            let angle = s.angle.map(|a| format!(" {}", word(&a))).unwrap_or_default();
            let flags = [(s.hit, " hit"), (s.flash, " flash"), (s.bridge, " bridge")].iter().filter(|(on, _)| *on).map(|(_, t)| *t).collect::<String>();
            let _ = writeln!(
                o,
                "    {}. {}{} {:.1} s: {}{}{}",
                i + 1,
                word(&s.size),
                angle,
                s.seconds,
                s.tells,
                if who.is_empty() { String::new() } else { format!(" [{}]", who.join(", ")) },
                flags
            );
        }
    }
    o.push_str(
        "Smaller templates (no pattern name; use null):\n\
- dread build: 20-60 s kept quiet; hold 4-12 s on architecture or the threat; hide the people (tiny, backs); return to the threat's face; show the threat in parts; a moment of false relief; then a hit.\n\
- cliffhanger (the last 30-45 s of an episode): the cost in a high or overhead wide held 2.7-9.4 s; the threat's eyes 1.8-6 s; the hero's eyes in extreme close-up or a symbol 3.6-5 s; silence; a last line; cut to black.\n\
- aftermath: smoke over the set; the threat; survivors rising; an overhead of the damage; survivors against a landmark wall.\n\
- connecting move: one tall or wide picture and a 3-9 s tilt or pan that carries the eye from one landmark to another instead of a cut.\n\
- awe: slow drifting hair and an object 0.5-2.7 s, the sound dropped away, an overhead of the crowd, then a small voice.\n",
    );
    o.push_str(
        "FRAMING PER FEELING (measured; use these instead of a plain face):\n\
- fear, shock: eyes only, held still 1-3.6 s; a face cropped to one eye; a hand over the mouth.\n\
- thinking under pressure: 7-11 cuts under a second, alternating eye crops and jaw crops.\n\
- resolve: a back view walking toward the threat; a fist insert; eyes in shadow with one glint.\n\
- pride, menace: a back view; eyes hidden, then revealed; lips only.\n\
- grief: the lower face only, tears and clenched teeth; a profile with hair over the eyes.\n\
- despair: a slumped body with the eyes hidden; a tiny figure from overhead.\n\
- relief: eyes closed, soft particles, the sound very quiet.\n\
- awe: slow drifting hair, the sound dropping away.\n",
    );
    o.push_str(
        "DEFAULT CAMERA PER SITUATION (decision table; change it when the beat asks):\n\
- establishing / a new place: extreme long or long, high, 24 mm, deep focus, a slow pan or a creeping push.\n\
- conversation: eye level, two-thirds of all cuts; the close shot is a chest-up medium close-up on 85 mm, not a tight face; about one in two dialogue close shots over a shoulder.\n\
- power, threat, resolve: low angle; vulnerability, a rule, a crowd: high angle; one overhead per main place and again after anything is destroyed.\n\
- fight exchange: long or full with both fighters, 24-35 mm, eye or high; the strike: low, 24 mm, a snap move and a shake on the hit; hit reaction: medium close-up, high, a short shake.\n\
- horror and dread: dark key, hard single source, long holds; a jump scare: a crash zoom on a close-up, under-light.\n\
- romance: long lens (85 mm), shallow focus, soft backlight, a creeping push.\n\
- grief: hold the shot; extreme long from high with empty space; a back view at a window.\n\
- flashback: medium or long, glow and grain, a creeping drift, washed-out colour.\n\
- tilted (dutch) frames: only in action or horror, never in conversation (about 3% of cuts).\n\
- stronger emotion: widen the lens and shorten the holds in kinetic scenes (fight, action, horror); lengthen the lens in intimate ones (romance, drama, grief).\n",
    );
    o
}

/// The lens of `LENSES` nearest to `mm` (a missing or absurd value: 35 mm).
pub fn snap_lens(mm: f32) -> u16 {
    if !mm.is_finite() || mm <= 0. {
        return 35;
    }
    LENSES.iter().copied().min_by(|a, b| (*a as f32 - mm).abs().total_cmp(&(*b as f32 - mm).abs())).unwrap_or(35)
}

/// `mm` moved `steps` places along [`LENSES`] (negative: wider).
fn step_lens(mm: u16, steps: i32) -> u16 {
    let i = LENSES.iter().position(|l| *l == snap_lens(mm as f32)).unwrap_or(2) as i32;
    LENSES[(i + steps).clamp(0, LENSES.len() as i32 - 1) as usize]
}

fn is_close(size: ShotSize) -> bool {
    matches!(size, ShotSize::Mcu | ShotSize::Cu | ShotSize::Ecu | ShotSize::Insert | ShotSize::Ots)
}

fn is_wide(size: ShotSize) -> bool {
    matches!(size, ShotSize::Els | ShotSize::Ls | ShotSize::Overhead)
}

/// Decision-table defaults (lens, angle, depth, light, move) for a shot size in a scene
/// (report 12 section 3, with its beat and intensity scalers): the starting point of every
/// field the planner leaves out.
pub fn default_camera(scene: &Scene, size: ShotSize) -> Camera {
    use ShotSize::*;
    let (lens, angle, depth, mv) = match size {
        Els => (24, Angle::High, Depth::Deep, Move::PanRight),
        Ls => (24, Angle::Eye, Depth::Deep, Move::Hold),
        Fs | Cowboy => (35, Angle::Eye, Depth::Normal, Move::Hold),
        Ms | TwoShot => (50, Angle::Eye, Depth::Normal, Move::Hold),
        Mcu | Cu => (85, Angle::Eye, Depth::Shallow, Move::Hold),
        Ecu => (85, Angle::Eye, Depth::Shallow, Move::Hold),
        Insert => (85, Angle::High, Depth::Shallow, Move::Hold),
        Ots => (85, Angle::Eye, Depth::Normal, Move::Hold),
        Pov => (24, Angle::Eye, Depth::Normal, Move::Hold),
        Overhead => (24, Angle::Top, Depth::Deep, Move::Hold),
        _ => (35, Angle::Eye, Depth::Normal, Move::Hold),
    };
    let mut c = Camera { size, angle, lens_mm: lens, depth, ..Camera::default() };
    let mut mv = mv;
    let mut speed = Speed::Slow;
    let kinetic = scene.fight || matches!(scene.kind, SceneKind::Action) || matches!(scene.tone, Tone::Fight | Tone::Action | Tone::Horror);
    let intimate = matches!(scene.tone, Tone::Romance | Tone::Drama | Tone::Calm);

    match scene.kind {
        SceneKind::Action => {
            speed = Speed::Fast;
            if matches!(size, Cowboy | Fs) {
                c.angle = Angle::Low; // the stand-off
            }
        }
        SceneKind::Dread => {
            c.key = Key::Dark;
            c.grain = Grain::Light;
            if is_wide(size) && size != Overhead {
                mv = Move::PushIn;
                speed = Speed::Creep;
            }
        }
        SceneKind::Narration => {
            if is_wide(size) && size != Overhead {
                mv = Move::PanRight;
            }
        }
        SceneKind::Flashback => {
            c.glow = true;
            c.grain = Grain::Light;
            if !is_close(size) {
                mv = Move::PushIn;
                speed = Speed::Creep;
            }
        }
        _ => {}
    }
    match scene.tone {
        Tone::Horror => {
            c.key = Key::Dark;
            c.grain = Grain::Light;
        }
        Tone::Romance if matches!(size, Mcu | Cu | Ecu) => {
            c.light = LightSetup::Backlit;
            c.depth = Depth::Shallow;
            if scene.intensity >= 5 {
                mv = Move::PushIn;
                speed = Speed::Creep;
            }
        }
        Tone::Comedy | Tone::Calm => c.key = Key::Bright,
        _ => {}
    }
    if matches!(scene.time_of_day, crate::model::TimeOfDay::Night) && c.key == Key::Normal {
        c.key = Key::Dark;
    }
    // Intensity scaler: stronger feeling widens the lens in kinetic scenes, lengthens it in
    // intimate ones (tighter shots only; a wide stays wide).
    if scene.intensity >= 8 && matches!(size, Ms | Mcu | Cu | Ecu | Cowboy) {
        if kinetic {
            c.lens_mm = step_lens(c.lens_mm, -1);
        } else if intimate {
            c.lens_mm = step_lens(c.lens_mm, 1);
        }
    }
    c.movement = Movement { kind: mv, speed, amount: 1. };
    c
}

/// Plan distance (metres) from the subject a camera of this size stands, and its height.
pub fn camera_distance(size: ShotSize) -> f32 {
    match size {
        ShotSize::Els => 14.,
        ShotSize::Ls => 7.,
        ShotSize::Fs => 4.5,
        ShotSize::Cowboy => 3.5,
        ShotSize::TwoShot => 3.,
        ShotSize::Ms => 2.5,
        ShotSize::Mcu => 1.8,
        ShotSize::Ots => 1.6,
        ShotSize::Cu => 1.3,
        ShotSize::Ecu => 0.8,
        ShotSize::Insert => 0.7,
        ShotSize::Pov | ShotSize::Overhead => 0.,
        _ => 3.,
    }
}

/// Camera height for an angle (metres).
pub fn camera_height(angle: Angle, size: ShotSize) -> f32 {
    match angle {
        Angle::Low => 0.6,
        Angle::Worm => 0.2,
        Angle::High => {
            if matches!(size, ShotSize::Els | ShotSize::Ls) {
                6.
            } else {
                3.
            }
        }
        Angle::Top => 12.,
        _ => 1.5,
    }
}

/// Where the template stands each character: the focal on the left of the line, the other on
/// the right, facing each other; the rest behind them in a shallow arc, facing the camera side.
fn home(i: usize) -> ([f32; 2], f32) {
    match i {
        0 => ([-1.2, 0.], 90.),
        1 => ([1.2, 0.], 270.),
        n => {
            let k = (n - 2) as f32;
            let side = if n % 2 == 0 { -1. } else { 1. };
            ([side * (0.6 + 0.6 * (k / 2.).floor()), -1.4 - 0.3 * (k / 2.).floor()], 180.)
        }
    }
}

/// The shots of a pattern for a scene and its characters (ids: focal, other, then the rest),
/// unsaved (no ids) and placed on a default line: the focal character on the left, the other on
/// the right, facing each other, every camera on the south side of that line.
pub fn template(pattern: Pattern, scene: &Scene, characters: &[String]) -> Vec<Shot> {
    let Some(d) = def(pattern) else { return Vec::new() };
    d.steps.iter().map(|s| from_step(s, pattern, scene, characters)).collect()
}

/// One shot from one pattern step.
pub fn from_step(s: &Step, pattern: Pattern, scene: &Scene, characters: &[String]) -> Shot {
    let mut marks = Vec::new();
    for (i, id) in characters.iter().enumerate() {
        let vis = match i {
            0 => s.focal,
            1 => s.other,
            _ => s.rest,
        };
        let Some(visible) = vis else { continue };
        let (pos, facing) = home(i);
        marks.push(Mark {
            character: id.clone(),
            pos,
            facing_deg: facing,
            pose: s.pose,
            visible,
            emotion: Emotion { kind: crate::model::EmotionKind::Neutral, strength: 1 },
            ..Mark::default()
        });
    }
    let mut camera = default_camera(scene, s.size);
    if let Some(a) = s.angle {
        camera.angle = a;
    }
    if let Some(m) = s.mv {
        camera.movement.kind = m;
    }
    // The subject: the first character seen whole (not over a shoulder), else the centre.
    let subject = marks.iter().find(|m| !matches!(m.visible, Visibility::Ots | Visibility::Offscreen));
    let target = subject.map(|m| m.pos).unwrap_or([0., 0.]);
    let h = camera_height(camera.angle, s.size);
    let dist = camera_distance(s.size);
    camera.pos = Some(match s.size {
        ShotSize::Overhead => [0., h.max(8.), 0.],
        ShotSize::Pov => {
            let looker = home(0).0;
            [looker[0], 1.6, looker[1]]
        }
        ShotSize::Ots => {
            // Behind the shoulder of the one seen from behind, on the south side of the line.
            let near = marks.iter().find(|m| m.visible == Visibility::Ots).map(|m| m.pos).unwrap_or([0., 0.]);
            [near[0] + (near[0] - target[0]).signum() * 0.5, 1.5, near[1] + 0.6]
        }
        _ => {
            // South of the subject, nudged toward the centre so the subject looks across the frame.
            let nudge = if target[0].abs() > 0.1 { -target[0].signum() * 0.25 * dist.min(3.) } else { 0. };
            [target[0] + nudge, h, target[1] + dist.max(0.5)]
        }
    });
    camera.look_at = match (s.size, subject) {
        (ShotSize::Overhead, _) => LookAt::Angles { yaw_deg: 0., pitch_deg: -90. },
        (_, Some(m)) => LookAt::Character(m.character.clone()),
        _ => LookAt::Point([target[0], 1., target[1]]),
    };
    let mouth = if s.talk && matches!(s.motion, MotionKind::MouthOnly) { MouthPolicy::Flap } else { MouthPolicy::Closed };
    Shot {
        tells: capitalised(s.tells),
        origin: Origin::Anime,
        pattern: Some(pattern),
        camera,
        marks,
        mouth,
        motion: s.motion,
        background: s.background,
        hit: s.hit,
        flash: s.flash,
        bridge: s.bridge,
        transition: Transition::Cut,
        planned_s: s.seconds,
        ..Shot::default()
    }
}
