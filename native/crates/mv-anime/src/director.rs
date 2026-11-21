//! SHOTS, the director (design 6.3): one planner call per scene, deterministic
//! post-processing (ids, every line in exactly one shot, timing floors, geometry), the
//! film-grammar checks, and up to [`REVISIONS`] revise calls kept only when they score better.
//!
//! The pure steps are public so the Shots screen and the tests use them directly:
//! [`prompt`] builds the planner call, [`read_reply`] turns the model's JSON into placed shots,
//! [`settle`] re-applies the line, timing and mouth rules after an edit or a merge, [`merge`]
//! keeps the user's work across a re-plan.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use anyhow::{anyhow, Result};
use mv_ai::{AiEffort, CancellationToken};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::checks;
use crate::geometry;
use crate::llm::{self, Ai, BadReply};
use crate::merge::{is_locked, merge_unlocked};
use crate::model::{
    Angle, BackgroundKind, Beat, BibleIndex, Depth, Emotion, EmotionKind, Focus, FocusTarget, Grain, Hand, Hold, Issue, Key, LightSetup, Line, LineKind, LookAt, Mark,
    MotionKind, MouthPolicy, Move, Origin, Pattern, Place, Pose, Role, Scene, SceneShots, Severity, Shot, ShotSize, SourceRef, Speed, Story, Transition, Visibility,
};
use crate::pacing;
use crate::patterns;
use crate::queue::{JobCtx, Tier};
use crate::store::Store;

/// Most revise calls per scene.
pub const REVISIONS: u8 = 2;
/// How long a bridge shot without its own length holds (1.5-2.5 s: it only carries the camera
/// across the line).
pub const BRIDGE_S: f32 = 2.0;

/// The planner's system prompt (role, staging rules, coordinates, the reply shape).
pub const SHOTS_PROMPT: &str = include_str!("prompts/shots.txt");
/// Appended to [`SHOTS_PROMPT`] for a revise call.
pub const REVISE_PROMPT: &str = include_str!("prompts/revise.txt");

const NO_AI: &str = "No AI model is set up for planning shots. Choose one in the engine settings (or set the assistant's model) and try again.";

/// Everything one scene's planning reads.
#[derive(Clone, Copy)]
pub struct SceneInput<'a> {
    pub scene: &'a Scene,
    /// The scene's lines in script order ([`scene_lines`]).
    pub lines: &'a [Line],
    pub bible: &'a BibleIndex,
    /// The scene opens in a place not seen just before ([`opens_new_place`]).
    pub new_place: bool,
}

impl SceneInput<'_> {
    fn place(&self) -> Option<&Place> {
        self.scene.place.as_deref().and_then(|p| self.bible.place(p))
    }
}

/// One scene's plan.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    /// Placed and settled, without ids (the job gives them).
    pub shots: Vec<Shot>,
    /// The checks of `shots`.
    pub issues: Vec<Issue>,
    /// What planning itself noticed (unknown ids dropped, a fallback used); codes start `plan-`.
    pub notes: Vec<Issue>,
    /// Revise calls made.
    pub revisions: u8,
    /// The AI gave no usable shots and the plan was built from the coverage patterns.
    pub fallback: bool,
    /// The AI's answer could not be read at all (its raw text): the plan is the basic one.
    pub unreadable: Option<String>,
}

// ------------------------------------------------------------------------- scene inputs

/// The scene's lines in script order: those that name the scene, plus any its beats list.
pub fn scene_lines(story: &Story, scene: &Scene) -> Vec<Line> {
    let in_beats: BTreeSet<&str> = scene.beats.iter().flat_map(|b| b.lines.iter().map(String::as_str)).collect();
    story.lines.iter().filter(|l| l.scene == scene.id || in_beats.contains(l.id.as_str())).cloned().collect()
}

/// Whether the scene opens in a place other than the scene before it (the first scene always does).
pub fn opens_new_place(story: &Story, scene_id: &str) -> bool {
    match story.scenes.iter().position(|s| s.id == scene_id) {
        Some(0) | None => true,
        Some(i) => story.scenes[i - 1].place != story.scenes[i].place,
    }
}

/// Character ids on screen in the scene: the speakers in order of their first line, then bible
/// characters the scene's text names. The hero (if any) comes first, then the threat.
pub fn cast(input: &SceneInput) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for sp in input.lines.iter().filter_map(|l| l.speaker.as_deref()).map(str::trim).filter(|s| !s.is_empty()) {
        if !ids.iter().any(|i| i == sp) {
            ids.push(sp.to_string());
        }
    }
    let s = input.scene;
    let mut text = format!("{} {}", s.title, s.summary);
    for b in &s.beats {
        text.push(' ');
        text.push_str(&b.purpose);
    }
    for l in input.lines {
        text.push(' ');
        text.push_str(&l.spoken);
        text.push(' ');
        text.push_str(&l.original);
    }
    let text = format!(" {} ", text.to_lowercase());
    for c in &input.bible.characters {
        if ids.contains(&c.id) {
            continue;
        }
        let named = std::iter::once(&c.name).chain(c.aliases.iter()).map(|n| n.trim().to_lowercase()).filter(|n| n.len() >= 2).any(|n| mentions(&text, &n));
        if named {
            ids.push(c.id.clone());
        }
    }
    let role = |id: &String| input.bible.character(id).map(|c| c.role).unwrap_or_default();
    // Stable: hero first, then threats, the rest in their order.
    let rank = |id: &String| match role(id) {
        Role::Hero => 0,
        Role::Threat => 1,
        _ => 2,
    };
    ids.sort_by_key(rank);
    ids
}

/// `text` (lower case, padded with spaces) names `name` as a whole word.
fn mentions(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(i, _)| {
        let before = text[..i].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let after = text[i + name.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        before && after
    })
}

// ------------------------------------------------------------------------- the prompt

/// The snake_case word of an enum value.
fn w<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

fn line_text(l: &Line) -> &str {
    if l.spoken.trim().is_empty() {
        l.original.trim()
    } else {
        l.spoken.trim()
    }
}

/// The scene, its lines, the cast, the place with its floor plan and stations, the pacing
/// targets, the patterns and the checks: the user message of the planner and revise calls.
pub fn bundle(input: &SceneInput) -> String {
    let s = input.scene;
    let mut o = String::new();
    let _ = writeln!(o, "SCENE {} \"{}\"", s.id, s.title);
    let _ = writeln!(
        o,
        "kind {} | tone {} | fight {} | intensity {}/10 | colour \"{}\" | time {} | target about {:.0} s",
        w(&s.kind),
        w(&s.tone),
        if s.fight { "yes" } else { "no" },
        s.intensity,
        s.colour.name,
        w(&s.time_of_day),
        s.target_s
    );
    if !s.summary.trim().is_empty() {
        let _ = writeln!(o, "summary: {}", s.summary.trim());
    }
    if !s.beats.is_empty() {
        let _ = writeln!(o, "beats:");
        for b in &s.beats {
            let _ = writeln!(o, "- {}: {}{} (lines: {})", b.id, b.purpose.trim(), if b.hook { " [hook]" } else { "" }, if b.lines.is_empty() { "none".into() } else { b.lines.join(", ") });
        }
    }
    if input.new_place {
        let _ = writeln!(o, "This scene OPENS IN A NEW PLACE: cut 1 or 2 must show the layout (extreme long, long or overhead) within 6 s, held about 4 s; consider the arrival funnel.");
    } else {
        let _ = writeln!(o, "This scene RETURNS to the place of the scene before: it may open on a detail, but show the layout within the first 3 cuts.");
    }

    let _ = writeln!(o, "\nLINES, in script order (id | speaker | kind | emotion | text | about how long it takes to say):");
    if input.lines.is_empty() {
        let _ = writeln!(o, "(none: a silent scene; tell it with pictures)");
    }
    for l in input.lines {
        let who = match &l.speaker {
            Some(id) => match input.bible.character(id) {
                Some(c) if !c.name.is_empty() => format!("{} ({id})", c.name),
                _ => id.clone(),
            },
            None => "narrator".into(),
        };
        let secs = if l.kind == LineKind::Sfx { "sound effect, not voiced".to_string() } else { format!("{:.1} s", pacing::line_seconds(line_text(l))) };
        let _ = writeln!(o, "{} | {who} | {} | {} {} | \"{}\" | {secs}", l.id, w(&l.kind), w(&l.emotion.kind), l.emotion.strength.clamp(1, 3), line_text(l).replace('"', "'"));
    }

    let ids = cast(input);
    let _ = writeln!(o, "\nCAST ON SCREEN (use these ids in marks; add others only if the scene needs them and they are listed here):");
    if ids.is_empty() {
        let _ = writeln!(o, "(nobody named; a place or object scene)");
    }
    for id in &ids {
        match input.bible.character(id) {
            Some(c) => {
                let mut line = format!("- {id} | {} | role {}", if c.name.is_empty() { id } else { &c.name }, w(&c.role));
                if !c.look.signature_colour.is_empty() {
                    let _ = write!(line, " | signature colour {}", c.look.signature_colour);
                }
                if c.look.height_m > 0. {
                    let _ = write!(line, " | {:.2} m tall", c.look.height_m);
                }
                let look: Vec<&str> = [c.look.hair.as_str(), c.look.default_outfit.as_str()].into_iter().map(str::trim).filter(|t| !t.is_empty()).collect();
                if !look.is_empty() {
                    let _ = write!(line, " | {}", look.join("; "));
                }
                let _ = writeln!(o, "{line}");
            }
            None => {
                let _ = writeln!(o, "- {id} | (not in the bible yet)");
            }
        }
    }
    let props: Vec<_> = input
        .bible
        .props
        .iter()
        .filter(|p| {
            let name = p.name.trim().to_lowercase();
            p.owner.as_ref().is_some_and(|ow| ids.contains(ow)) || (name.len() >= 2 && mentions(&format!(" {} {} ", s.summary.to_lowercase(), s.title.to_lowercase()), &name))
        })
        .collect();
    if !props.is_empty() {
        let _ = writeln!(o, "PROPS (ids for \"holding\" and \"focus\"):");
        for p in props {
            let _ = writeln!(o, "- {} | {} | owner {} | {}", p.id, p.name, p.owner.as_deref().unwrap_or("none"), if p.held { "held in a hand" } else { "not held" });
        }
    }

    match input.place() {
        Some(p) => {
            let _ = writeln!(
                o,
                "\nPLACE {} \"{}\" ({}), {:.1} x {:.1} m. Plan origin at the centre, x east (right), z south (down); north is the top. x runs {:.1}..{:.1}, z runs {:.1}..{:.1}.",
                p.id,
                p.name,
                w(&p.kind),
                p.plan.w_m,
                p.plan.d_m,
                -p.plan.w_m / 2.,
                p.plan.w_m / 2.,
                -p.plan.d_m / 2.,
                p.plan.d_m / 2.
            );
            if !p.description.trim().is_empty() {
                let _ = writeln!(o, "{}", p.description.trim());
            }
            if !p.plan.landmarks.is_empty() {
                let _ = writeln!(o, "Landmarks:");
                for l in &p.plan.landmarks {
                    let _ = writeln!(o, "- {} \"{}\" {} at ({:.1}, {:.1}), {:.1} x {:.1} m, {:.1} m high", l.id, l.name, w(&l.kind), l.pos[0], l.pos[1], l.size[0], l.size[1], l.h_m);
                }
            }
            if p.stations.is_empty() {
                let _ = writeln!(o, "No camera stations yet: give exact camera positions (station null).");
            } else {
                let _ = writeln!(o, "Camera stations:");
                for st in &p.stations {
                    let _ = writeln!(
                        o,
                        "- {} \"{}\" at ({:.1}, {:.1}), {:.1} m high, looking {:.0}°, pitch {:.0}°, {} mm{}",
                        st.id,
                        st.name,
                        st.pos[0],
                        st.pos[2],
                        st.pos[1],
                        st.yaw_deg,
                        st.pitch_deg,
                        st.lens_mm,
                        if st.overhead { ", OVERHEAD" } else { "" }
                    );
                }
            }
            let tod = w(&s.time_of_day);
            let state = p.states.iter().find(|st| st.id.eq_ignore_ascii_case(&tod) || st.name.to_lowercase().contains(&tod)).or_else(|| p.states.first());
            if let Some(st) = state {
                let _ = writeln!(o, "Light ({}): key light from {:.0}°, {} K{}. Every shot is lit from this side.", st.name, st.key_dir_deg, st.kelvin, if st.mood.is_empty() { String::new() } else { format!(", {}", st.mood) });
            }
        }
        None => {
            let _ = writeln!(
                o,
                "\nPLACE: none in the bible yet. Invent one simple floor plan of about 12 x 8 m centred on 0,0 (x -6..6, z -4..4), keep it for the whole scene, and give exact camera positions (station null)."
            );
        }
    }

    let row = pacing::pace(s.kind);
    let spoken: f32 = input.lines.iter().filter(|l| l.kind != LineKind::Sfx).map(|l| pacing::line_seconds(line_text(l))).sum();
    let target = if s.target_s > 0. { s.target_s } else { (spoken * 1.4).max(row.median_cut_s * 4.) };
    let cuts = (target / row.median_cut_s).round().clamp(3., 80.);
    let _ = writeln!(
        o,
        "\nPACING for a {} scene: median cut {:.1} s, mean cut {:.1}-{:.1} s, a single take at most about {:.0} s, a line starts {:.1} s after its cut. For about {:.0} s aim near {:.0} cuts. Full animation in {:.0}-{:.0}% of cuts; the rest held drawings, camera moves over stills and mouth flaps.",
        w(&s.kind),
        row.median_cut_s,
        row.mean_range_s.0,
        row.mean_range_s.1,
        row.long_take_s,
        row.line_lead_s,
        target,
        cuts,
        row.full_animation.0 * 100.,
        row.full_animation.1 * 100.
    );
    let _ = writeln!(o, "A shot's seconds are at least {:.1} (lead) + its lines' seconds + 0.35 between lines + 0.4 after the last.", row.line_lead_s);
    o.push('\n');
    o.push_str(&patterns::rules_text());
    o.push('\n');
    o.push_str(&checks::rules_text(s));
    o
}

/// The planner call: (system, user).
pub fn prompt(input: &SceneInput) -> (String, String) {
    let mut user = bundle(input);
    let _ = write!(user, "\nNow plan scene {}. Decide the floor plan, the marks and the camera side first, then write every shot. Reply with the JSON object only.", input.scene.id);
    (SHOTS_PROMPT.to_string(), user)
}

/// The revise call: (system, user) with the plan as the reply shape and the check's notes.
pub fn revise_prompt(input: &SceneInput, shots: &[Shot], issues: &[Issue]) -> (String, String) {
    let system = format!("{SHOTS_PROMPT}\n{REVISE_PROMPT}");
    let plan = json!({ "shots": shots.iter().enumerate().map(|(i, s)| to_reply(s, i + 1, input.place())).collect::<Vec<_>>() });
    let mut user = bundle(input);
    let _ = write!(
        user,
        "\nYOUR PLAN (cuts numbered from 1):\n{}\n\nTHE CHECK'S NOTES:\n{}\nRevise the plan. Reply with the whole revised JSON object only.",
        serde_json::to_string(&plan).unwrap_or_default(),
        checks::notes(issues)
    );
    (system, user)
}

/// A shot in the reply shape (for the revise call). `cut` is its 1-based number.
pub fn to_reply(s: &Shot, cut: usize, place: Option<&Place>) -> Value {
    let c = &s.camera;
    let pos = geometry::cam_pos(c, place);
    let look = match &c.look_at {
        LookAt::Character(id) => format!("char:{id}"),
        LookAt::Landmark(id) => format!("landmark:{id}"),
        LookAt::Point(p) => format!("point:{:.1},{:.1}", p[0], p[2]),
        LookAt::Angles { yaw_deg, pitch_deg } => format!("angles:{yaw_deg:.0},{pitch_deg:.0}"),
    };
    let focus = match &c.focus.target {
        FocusTarget::Character(id) => format!("char:{id}"),
        FocusTarget::Prop(id) => format!("prop:{id}"),
        FocusTarget::Background => "background".into(),
        _ => "auto".into(),
    };
    json!({
        "cut": cut,
        "tells": s.tells,
        "beat": s.beat,
        "origin": w(&s.origin),
        "source": s.source.iter().map(|r| json!({"page": r.page, "panel": r.panel})).collect::<Vec<_>>(),
        "pattern": s.pattern.map(|p| w(&p)),
        "size": w(&c.size),
        "angle": w(&c.angle),
        "station": c.station,
        "camera": {"x": round1(pos[0]), "z": round1(pos[2]), "height": round1(pos[1])},
        "look_at": look,
        "lens": c.lens_mm,
        "roll": c.roll_deg,
        "depth": w(&c.depth),
        "focus": focus,
        "key": w(&c.key),
        "grain": w(&c.grain),
        "glow": c.glow,
        "light": w(&c.light),
        "move": {"kind": w(&c.movement.kind), "speed": w(&c.movement.speed)},
        "marks": s.marks.iter().map(|m| json!({
            "char": m.character, "x": round1(m.pos[0]), "z": round1(m.pos[1]), "facing": m.facing_deg.round(),
            "pose": w(&m.pose), "visible": w(&m.visible), "emotion": w(&m.emotion.kind), "strength": m.emotion.strength,
            "action": m.action, "holding": m.holding.iter().map(|h| json!({"prop": h.prop, "hand": w(&h.hand)})).collect::<Vec<_>>(),
            "gaze": m.gaze,
        })).collect::<Vec<_>>(),
        "lines": s.lines,
        "mouth": w(&s.mouth),
        "motion": w(&s.motion),
        "background": w(&s.background),
        "hit": s.hit,
        "flash": s.flash,
        "bridge": s.bridge,
        "transition": w(&s.transition),
        "seconds": s.planned_s,
    })
}

fn round1(v: f32) -> f32 {
    if v.is_finite() {
        (v * 10.).round() / 10.
    } else {
        0.
    }
}

// ------------------------------------------------------------------------- reading the reply

fn norm(s: &str) -> String {
    s.trim().to_lowercase().replace([' ', '-'], "_")
}

/// A plain word into an enum; `None` for a word this build does not know.
fn word<T: DeserializeOwned + Serialize>(s: &str) -> Option<T> {
    let t: T = serde_json::from_value(Value::String(norm(s))).ok()?;
    (w(&t) != "unknown").then_some(t)
}

fn size_word(s: &str) -> Option<ShotSize> {
    let n = norm(s);
    let alias = match n.as_str() {
        "extreme_long" | "extreme_long_shot" | "establishing" | "ews" | "els_shot" | "extreme_wide" => Some(ShotSize::Els),
        "long" | "long_shot" | "wide" | "wide_shot" | "ws" => Some(ShotSize::Ls),
        "full" | "full_shot" | "full_figure" | "mls" | "medium_long" => Some(ShotSize::Fs),
        "medium" | "medium_shot" | "mid" => Some(ShotSize::Ms),
        "medium_close_up" | "medium_close" | "medium_closeup" => Some(ShotSize::Mcu),
        "close_up" | "close" | "closeup" => Some(ShotSize::Cu),
        "extreme_close_up" | "extreme_closeup" | "extreme_close" => Some(ShotSize::Ecu),
        "over_the_shoulder" | "over_shoulder" => Some(ShotSize::Ots),
        "two" | "twoshot" | "two_shot" => Some(ShotSize::TwoShot),
        "point_of_view" => Some(ShotSize::Pov),
        "top_down" | "top" | "birds_eye" | "bird_eye" | "bird's_eye" => Some(ShotSize::Overhead),
        _ => None,
    };
    alias.or_else(|| word(&n))
}

fn angle_word(s: &str) -> Option<Angle> {
    let n = norm(s);
    let alias = match n.as_str() {
        "eye_level" | "level" => Some(Angle::Eye),
        "low_angle" => Some(Angle::Low),
        "high_angle" => Some(Angle::High),
        "top_down" | "overhead" | "birds_eye" | "bird_eye" => Some(Angle::Top),
        "worms_eye" | "worm_eye" | "ground" | "ground_level" => Some(Angle::Worm),
        "tilted" | "dutch_angle" => Some(Angle::Dutch),
        _ => None,
    };
    alias.or_else(|| word(&n))
}

fn move_word(s: &str) -> Option<Move> {
    let n = norm(s);
    let alias = match n.as_str() {
        "fix" | "fixed" | "static" | "none" | "still" => Some(Move::Hold),
        "push" | "dolly_in" | "zoom_in" | "track_in" => Some(Move::PushIn),
        "pull" | "dolly_out" | "zoom_out" | "track_out" => Some(Move::PullOut),
        "rack" => Some(Move::RackFocus),
        "whip_pan" => Some(Move::Whip),
        _ => None,
    };
    alias.or_else(|| word(&n))
}

fn num(v: &Value) -> Option<f32> {
    let f = match v {
        Value::Number(n) => n.as_f64()? as f32,
        Value::String(s) => s.trim().trim_end_matches(['s', 'm']).trim().parse::<f32>().ok()?,
        _ => return None,
    };
    f.is_finite().then_some(f)
}

fn get_f(o: &Map<String, Value>, k: &str) -> Option<f32> {
    o.get(k).and_then(num)
}

fn get_s<'a>(o: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    o.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("null") && !s.eq_ignore_ascii_case("none"))
}

fn get_b(o: &Map<String, Value>, k: &str) -> bool {
    match o.get(k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.trim().eq_ignore_ascii_case("true") || s.trim().eq_ignore_ascii_case("yes"),
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.),
        _ => false,
    }
}

/// Splits "char:jinwoo" into ("char", "jinwoo"); a bare "jinwoo" has no kind.
fn split_ref(s: &str) -> (Option<String>, &str) {
    match s.split_once(':') {
        Some((k, rest)) if !k.trim().is_empty() && k.trim().chars().all(|c| c.is_ascii_alphabetic()) => (Some(k.trim().to_lowercase()), rest.trim()),
        _ => (None, s.trim()),
    }
}

/// Ids known to the scene, findable by id or (case-insensitive) name.
#[derive(Default)]
struct Ids {
    ids: BTreeSet<String>,
    by_name: BTreeMap<String, String>,
}

impl Ids {
    fn add(&mut self, id: &str, names: &[&str]) {
        if id.is_empty() {
            return;
        }
        self.ids.insert(id.to_string());
        for n in std::iter::once(id).chain(names.iter().copied()) {
            let k = n.trim().to_lowercase();
            if !k.is_empty() {
                self.by_name.entry(k).or_insert_with(|| id.to_string());
            }
        }
    }

    fn find(&self, s: &str) -> Option<String> {
        let s = s.trim();
        if self.ids.contains(s) {
            return Some(s.to_string());
        }
        self.by_name.get(&s.to_lowercase()).cloned()
    }
}

/// What a reply's ids are checked against.
struct Known<'a> {
    input: &'a SceneInput<'a>,
    chars: Ids,
    props: Ids,
    landmarks: Ids,
    stations: Ids,
    lines: Ids,
}

impl<'a> Known<'a> {
    fn new(input: &'a SceneInput<'a>) -> Known<'a> {
        let place = input.place();
        let mut chars = Ids::default();
        for c in &input.bible.characters {
            let mut names: Vec<&str> = vec![c.name.as_str()];
            names.extend(c.aliases.iter().map(String::as_str));
            chars.add(&c.id, &names);
        }
        for sp in input.lines.iter().filter_map(|l| l.speaker.as_deref()) {
            chars.add(sp, &[]);
        }
        let mut props = Ids::default();
        for p in &input.bible.props {
            props.add(&p.id, &[p.name.as_str()]);
        }
        let mut landmarks = Ids::default();
        let mut stations = Ids::default();
        if let Some(p) = place {
            for l in &p.plan.landmarks {
                landmarks.add(&l.id, &[l.name.as_str()]);
            }
            for s in &p.stations {
                stations.add(&s.id, &[s.name.as_str()]);
            }
        }
        let mut lines = Ids::default();
        for l in input.lines {
            lines.add(&l.id, &[]);
        }
        Known { input, chars, props, landmarks, stations, lines }
    }
}

/// A planning note (not a film-grammar check).
fn note(scene: &str, severity: Severity, code: &str, message: String, fix: Option<String>) -> Issue {
    Issue { code: code.into(), severity, scene: scene.to_string(), shot: None, message, fix }
}

/// Collects "unknown id" notes once per id.
struct Dropped {
    seen: BTreeSet<String>,
    notes: Vec<Issue>,
}

impl Dropped {
    fn add(&mut self, scene: &str, what: &str, id: &str) {
        if self.seen.insert(format!("{what}:{id}")) {
            self.notes.push(note(
                scene,
                Severity::Info,
                "plan-unknown",
                format!("The AI named a {what} \"{id}\" that this project does not have; it was left out of the plan."),
                Some(format!("Add the {what} in the Bible and plan the scene again if it belongs there.")),
            ));
        }
    }
}

fn read_ref_char(k: &Known, d: &mut Dropped, s: &str) -> Option<String> {
    let (_, id) = split_ref(s);
    let found = k.chars.find(id);
    if found.is_none() && !id.is_empty() {
        d.add(&k.input.scene.id, "character", id);
    }
    found
}

fn read_mark(o: &Map<String, Value>, k: &Known, d: &mut Dropped) -> Option<Mark> {
    let who = get_s(o, "char").or_else(|| get_s(o, "character")).or_else(|| get_s(o, "id"))?;
    let character = read_ref_char(k, d, who)?;
    let holding = o
        .get("holding")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|h| {
                    let (prop, hand) = match h {
                        Value::String(s) => (s.as_str(), None),
                        Value::Object(ho) => (get_s(ho, "prop")?, get_s(ho, "hand")),
                        _ => return None,
                    };
                    let (_, pid) = split_ref(prop);
                    match k.props.find(pid) {
                        Some(prop) => Some(Hold { prop, hand: hand.and_then(word::<Hand>).unwrap_or_default() }),
                        None => {
                            d.add(&k.input.scene.id, "prop", pid);
                            None
                        }
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let gaze = get_s(o, "gaze").and_then(|g| {
        let (kind, rest) = split_ref(g);
        match kind.as_deref() {
            _ if g.eq_ignore_ascii_case("camera") => Some("camera".to_string()),
            Some("landmark") => k.landmarks.find(rest).map(|id| format!("landmark:{id}")),
            _ => k.chars.find(rest).map(|id| format!("char:{id}")),
        }
    });
    Some(Mark {
        character,
        pos: [get_f(o, "x").unwrap_or(0.), get_f(o, "z").unwrap_or(0.)],
        facing_deg: get_f(o, "facing").map(|f| f.rem_euclid(360.)).unwrap_or(180.),
        pose: get_s(o, "pose").and_then(word::<Pose>).unwrap_or_default(),
        visible: get_s(o, "visible").and_then(word::<Visibility>).unwrap_or_default(),
        outfit: get_s(o, "outfit").map(str::to_string),
        emotion: Emotion {
            kind: get_s(o, "emotion").and_then(word::<EmotionKind>).unwrap_or_default(),
            strength: get_f(o, "strength").map(|s| s.round().clamp(1., 3.) as u8).unwrap_or(1),
        },
        action: get_s(o, "action").unwrap_or_default().to_string(),
        holding,
        gaze,
    })
}

fn read_look(s: &str, k: &Known) -> Option<LookAt> {
    let (kind, rest) = split_ref(s);
    match kind.as_deref() {
        Some("landmark") => k.landmarks.find(rest).map(LookAt::Landmark),
        Some("point") => {
            let mut it = rest.split([',', ' ']).filter(|t| !t.trim().is_empty()).filter_map(|t| t.trim().parse::<f32>().ok());
            let (x, z) = (it.next()?, it.next()?);
            (x.is_finite() && z.is_finite()).then_some(LookAt::Point([x, 1.5, z]))
        }
        Some("angles") => {
            let mut it = rest.split(',').filter_map(|t| t.trim().parse::<f32>().ok());
            let (yaw_deg, pitch_deg) = (it.next()?, it.next().unwrap_or(0.));
            Some(LookAt::Angles { yaw_deg, pitch_deg })
        }
        _ => k.chars.find(rest).map(LookAt::Character).or_else(|| k.landmarks.find(rest).map(LookAt::Landmark)),
    }
}

fn read_focus(s: &str, k: &Known) -> FocusTarget {
    let (kind, rest) = split_ref(s);
    match (kind.as_deref(), norm(s).as_str()) {
        (_, "background") => FocusTarget::Background,
        (_, "none") => FocusTarget::None,
        (Some("prop"), _) => k.props.find(rest).map(FocusTarget::Prop).unwrap_or_default(),
        (Some("char"), _) | (Some("character"), _) => k.chars.find(rest).map(FocusTarget::Character).unwrap_or_default(),
        _ => k.chars.find(rest).map(FocusTarget::Character).unwrap_or_default(),
    }
}

fn read_shot(o: &Map<String, Value>, k: &Known, d: &mut Dropped) -> Shot {
    let scene = k.input.scene;
    let size = get_s(o, "size").and_then(size_word).unwrap_or(ShotSize::Ms);
    let mut cam = patterns::default_camera(scene, size);
    if let Some(a) = get_s(o, "angle").and_then(angle_word) {
        cam.angle = a;
    }
    // A size that implies the angle.
    if size == ShotSize::Overhead {
        cam.angle = Angle::Top;
    }
    if let Some(st) = get_s(o, "station") {
        match k.stations.find(st) {
            Some(id) => cam.station = Some(id),
            None => d.add(&scene.id, "camera station", st),
        }
    }
    let camera_xz = o.get("camera").and_then(Value::as_object).and_then(|c| {
        let (x, z) = (get_f(c, "x")?, get_f(c, "z")?);
        let h = get_f(c, "height").or_else(|| get_f(c, "y")).unwrap_or_else(|| patterns::camera_height(cam.angle, size));
        Some([x, h, z])
    });
    if let Some(l) = get_s(o, "look_at").and_then(|s| read_look(s, k)) {
        cam.look_at = l;
    }
    if let Some(l) = get_f(o, "lens") {
        cam.lens_mm = patterns::snap_lens(l);
    }
    if let Some(r) = get_f(o, "roll") {
        cam.roll_deg = r.clamp(-45., 45.);
    }
    if let Some(v) = get_s(o, "depth").and_then(word::<Depth>) {
        cam.depth = v;
    }
    if let Some(v) = get_s(o, "key").and_then(word::<Key>) {
        cam.key = v;
    }
    if let Some(v) = get_s(o, "grain").and_then(word::<Grain>) {
        cam.grain = v;
    }
    if let Some(v) = get_s(o, "light").and_then(word::<LightSetup>) {
        cam.light = v;
    }
    if o.contains_key("glow") {
        cam.glow = get_b(o, "glow");
    }
    if let Some(f) = get_s(o, "focus") {
        cam.focus = Focus { target: read_focus(f, k), ..Focus::default() };
    }
    match o.get("move") {
        Some(Value::Object(m)) => {
            if let Some(kind) = get_s(m, "kind").and_then(move_word) {
                cam.movement.kind = kind;
            }
            if let Some(sp) = get_s(m, "speed").and_then(word::<Speed>) {
                cam.movement.speed = sp;
            }
            if let Some(a) = get_f(m, "amount").filter(|a| *a > 0.) {
                cam.movement.amount = a.clamp(0.25, 6.);
            }
        }
        Some(Value::String(s)) => {
            if let Some(kind) = move_word(s) {
                cam.movement.kind = kind;
            }
        }
        _ => {}
    }
    let mut marks: Vec<Mark> = Vec::new();
    for m in o.get("marks").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_object) {
        if let Some(mark) = read_mark(m, k, d) {
            if !marks.iter().any(|x| x.character == mark.character) {
                marks.push(mark);
            }
        }
    }
    // The reply template's example camera copied as is: exactly (0, 0), on top of someone. Left
    // out, so the camera is placed from the shot's size instead.
    if let Some(p) = camera_xz {
        let copied = p[0] == 0. && p[2] == 0. && marks.iter().any(|m| (m.pos[0] * m.pos[0] + m.pos[1] * m.pos[1]).sqrt() < 0.5);
        if !copied {
            cam.pos = Some(p);
        }
    }
    let mut lines: Vec<String> = Vec::new();
    for l in o.get("lines").and_then(Value::as_array).into_iter().flatten() {
        let id = match l {
            Value::String(s) => s.trim().to_string(),
            Value::Object(lo) => get_s(lo, "id").unwrap_or_default().to_string(),
            _ => continue,
        };
        match k.lines.find(&id) {
            Some(id) => lines.push(id),
            None if !id.is_empty() => d.add(&scene.id, "line", &id),
            None => {}
        }
    }
    let source: Vec<SourceRef> = o
        .get("source")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter_map(|r| {
            let page = get_f(r, "page")?.max(0.) as usize;
            let panel = get_f(r, "panel").map(|p| p.max(0.) as usize);
            let chapter_id = get_s(r, "chapter")
                .map(str::to_string)
                .or_else(|| scene.source.iter().find(|s| s.page == page).map(|s| s.chapter_id.clone()))
                .or_else(|| scene.source.first().map(|s| s.chapter_id.clone()))
                .unwrap_or_default();
            Some(SourceRef { chapter_id, page, panel })
        })
        .collect();
    let origin = match get_s(o, "origin").and_then(word::<Origin>) {
        Some(Origin::Anime) => Origin::Anime,
        _ if source.is_empty() => Origin::Anime,
        _ => Origin::Manga,
    };
    // A beat id of this scene, or a bare number counting its beats from 1 ("bt0012" from another
    // scene is not this scene's 12th beat).
    let beat = match o.get("beat") {
        Some(Value::Number(n)) => n.as_u64().and_then(|n| usize::try_from(n).ok()).and_then(|n| scene.beats.get(n.checked_sub(1)?)).map(|x| x.id.clone()),
        _ => get_s(o, "beat").and_then(|b| {
            scene.beats.iter().find(|x| x.id == b).map(|x| x.id.clone()).or_else(|| {
                let n: usize = b.trim().parse().ok()?;
                scene.beats.get(n.checked_sub(1)?).map(|x| x.id.clone())
            })
        }),
    };
    let mut shot = Shot {
        tells: crate::patterns::capitalised(get_s(o, "tells").unwrap_or_default()),
        beat,
        origin,
        source,
        pattern: get_s(o, "pattern").and_then(word::<Pattern>),
        camera: cam,
        marks,
        lines,
        mouth: get_s(o, "mouth").and_then(word::<MouthPolicy>).unwrap_or_default(),
        motion: get_s(o, "motion").and_then(word::<MotionKind>).unwrap_or_default(),
        hit: get_b(o, "hit"),
        flash: get_b(o, "flash"),
        bridge: get_b(o, "bridge"),
        transition: get_s(o, "transition").and_then(word::<Transition>).unwrap_or_default(),
        planned_s: get_f(o, "seconds").or_else(|| get_f(o, "duration")).unwrap_or(0.),
        ..Shot::default()
    };
    // A background the reply left out: layout shots need the room, the others a backdrop.
    shot.background = match get_s(o, "background").and_then(word::<BackgroundKind>) {
        Some(b) => b,
        None if checks::is_layout(&shot) => BackgroundKind::Matched,
        None => BackgroundKind::Backdrop,
    };
    shot
}

/// The model's reply as placed, settled shots (no ids yet) plus planning notes for every id it
/// named that the project does not have. A reply without a usable shot gives no shots.
pub fn read_reply(input: &SceneInput, v: &Value) -> (Vec<Shot>, Vec<Issue>) {
    let k = Known::new(input);
    let mut d = Dropped { seen: BTreeSet::new(), notes: Vec::new() };
    let items: Vec<&Value> = match v {
        Value::Array(a) => a.iter().collect(),
        _ => v.get("shots").or_else(|| v.get("cuts")).and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default(),
    };
    let mut shots: Vec<Shot> = items.into_iter().filter_map(Value::as_object).map(|o| read_shot(o, &k, &mut d)).collect();
    if shots.is_empty() {
        return (shots, d.notes);
    }
    let given: Vec<BTreeSet<String>> = shots.iter().map(|s| s.lines.iter().cloned().collect()).collect();
    place_shots(&mut shots, input);
    settle(&mut shots, input);
    let (mut shots, spilled) = spill(shots, &given, input);
    for s in &mut shots {
        s.suggested = suggestion(s);
    }
    let mut notes = d.notes;
    if spilled > 0 {
        notes.push(note(
            &input.scene.id,
            Severity::Info,
            "plan-extra-cuts",
            format!("The AI's plan left {} out, so {} added from the conversation pattern to carry them.", counted(spilled, "line"), if spilled > 2 { "cuts were" } else { "a cut was" }),
            Some("Check the added cuts, or plan the scene again.".into()),
        ));
    }
    (shots, notes)
}

/// Lines the plan did not place that would make a shot carry more than
/// [`checks::SHOT_MAX_LINES`] go into new cuts after it, two lines each, from the conversation
/// pattern (a cut-off answer must never become one shot several minutes long). The lines the AI
/// itself put in a shot stay there. Returns the shots and how many lines were moved.
fn spill(shots: Vec<Shot>, given: &[BTreeSet<String>], input: &SceneInput) -> (Vec<Shot>, usize) {
    let max = checks::SHOT_MAX_LINES;
    if shots.iter().all(|s| s.lines.len() <= max) {
        return (shots, 0);
    }
    let ids = talk_cast(input);
    let conv: Vec<Shot> = patterns::template(Pattern::Conversation, input.scene, &ids).into_iter().filter(frames_someone).collect();
    let talkers: Vec<Shot> = {
        let talk: Vec<usize> = patterns::def(Pattern::Conversation).map(|d| d.steps.iter().enumerate().filter(|(_, s)| s.talk).map(|(i, _)| i).collect()).unwrap_or_default();
        let all = patterns::template(Pattern::Conversation, input.scene, &ids);
        let picked: Vec<Shot> = all.into_iter().enumerate().filter(|(i, s)| talk.contains(i) && frames_someone(s)).map(|(_, s)| s).collect();
        if picked.is_empty() {
            conv
        } else {
            picked
        }
    };
    let mut out: Vec<Shot> = Vec::with_capacity(shots.len());
    let mut moved = 0;
    let mut cursor = 0;
    for (i, mut s) in shots.into_iter().enumerate() {
        let pinned = is_locked("/lines", &s.locked);
        let mine = given.get(i).cloned().unwrap_or_default();
        let added: Vec<String> = s.lines.iter().filter(|id| !mine.contains(*id)).cloned().collect();
        if pinned || s.lines.len() <= max || added.is_empty() {
            out.push(s);
            continue;
        }
        let room = max.saturating_sub(s.lines.len() - added.len());
        let extra: Vec<String> = added[room.min(added.len())..].to_vec();
        s.lines.retain(|id| !extra.contains(id));
        out.push(s);
        for pair in extra.chunks(2) {
            let mut n = talkers.get(cursor % talkers.len().max(1)).cloned().unwrap_or_default();
            cursor += 1;
            n.lines = pair.to_vec();
            n.tells = format!("{} (added to carry lines)", n.tells);
            place_shots(std::slice::from_mut(&mut n), input);
            out.push(n);
            moved += pair.len();
        }
    }
    if moved > 0 {
        settle(&mut out, input);
    }
    (out, moved)
}

/// The template shot frames somebody (a step whose characters are missing frames nobody).
fn frames_someone(s: &Shot) -> bool {
    s.marks.iter().any(|m| m.visible != Visibility::Offscreen)
}

/// The scene's characters for a pattern: the speakers first (the hero first among them), then
/// the rest.
fn talk_cast(input: &SceneInput) -> Vec<String> {
    let all = cast(input);
    let speaks = |id: &String| input.lines.iter().any(|l| l.speaker.as_ref() == Some(id));
    let mut ids: Vec<String> = all.iter().filter(|id| speaks(id)).cloned().collect();
    ids.extend(all.iter().filter(|id| !speaks(id)).cloned());
    ids
}

/// The shot as the director suggested it, without ids, cards or bookkeeping (for "Reset to Auto").
fn suggestion(s: &Shot) -> Value {
    let mut v = serde_json::to_value(s).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        for k in ["id", "frame", "clip", "suggested", "locked", "edited", "locked_s"] {
            o.remove(k);
        }
    }
    v
}

// ------------------------------------------------------------------------- placing

fn clamp_plan(place: Option<&Place>, x: f32, z: f32) -> [f32; 2] {
    match place.map(|p| (p.plan.w_m, p.plan.d_m)) {
        Some((w, d)) if w > 0. && d > 0. && w.is_finite() && d.is_finite() => [x.clamp(-w / 2., w / 2.), z.clamp(-d / 2., d / 2.)],
        _ => [x, z],
    }
}

/// Cameras on stations or exact points inside the place; marks inside the plan.
pub fn place_shots(shots: &mut [Shot], input: &SceneInput) {
    let place = input.place();
    for s in shots.iter_mut() {
        for m in &mut s.marks {
            m.pos = clamp_plan(place, m.pos[0], m.pos[1]);
            if !m.facing_deg.is_finite() {
                m.facing_deg = 180.;
            }
        }
        let c = &mut s.camera;
        let on_station = c.station.as_ref().is_some_and(|id| place.is_some_and(|p| p.stations.iter().any(|st| &st.id == id)));
        if on_station {
            // The station owns the position (the model's "None = the station's").
            c.pos = None;
        } else if let Some(p) = c.pos {
            let xz = clamp_plan(place, p[0], p[2]);
            c.pos = Some([xz[0], if p[1].is_finite() { p[1].clamp(0.1, 100.) } else { 1.5 }, xz[1]]);
            c.station = None;
            if let Some(pl) = place {
                let (yaw, _) = geometry::look_dir(c, &s.marks, place);
                c.station = geometry::nearest_station(pl, c.pos.unwrap_or_default(), yaw);
            }
        } else {
            c.station = None;
            let overhead_station = (c.size == ShotSize::Overhead || c.angle == Angle::Top).then(|| place.and_then(|p| p.stations.iter().find(|st| st.overhead))).flatten();
            match overhead_station {
                Some(st) => c.station = Some(st.id.clone()),
                None => {
                    // South of the subject at the size's distance (every such camera on one side).
                    let seen: Vec<&Mark> = s.marks.iter().filter(|m| m.visible != Visibility::Offscreen).collect();
                    let target = if seen.is_empty() {
                        [0., 0.]
                    } else {
                        let n = seen.len() as f32;
                        [seen.iter().map(|m| m.pos[0]).sum::<f32>() / n, seen.iter().map(|m| m.pos[1]).sum::<f32>() / n]
                    };
                    let h = patterns::camera_height(c.angle, c.size);
                    let xz = match c.size {
                        ShotSize::Overhead => target,
                        _ => clamp_plan(place, target[0], target[1] + patterns::camera_distance(c.size).max(0.5)),
                    };
                    c.pos = Some([xz[0], if c.size == ShotSize::Overhead { h.max(8.) } else { h }, xz[1]]);
                    if c.size == ShotSize::Overhead {
                        c.look_at = LookAt::Angles { yaw_deg: 0., pitch_deg: -90. };
                    }
                }
            }
        }
        // Nothing to look at: the first character seen, else the station's own direction.
        if matches!(c.look_at, LookAt::Angles { yaw_deg, pitch_deg } if yaw_deg == 0. && pitch_deg == 0.) {
            if let Some(m) = s.marks.iter().find(|m| m.visible != Visibility::Offscreen) {
                c.look_at = LookAt::Character(m.character.clone());
            } else if let Some(st) = c.station.as_ref().and_then(|id| place.and_then(|p| p.stations.iter().find(|x| &x.id == id))) {
                c.look_at = LookAt::Angles { yaw_deg: st.yaw_deg, pitch_deg: st.pitch_deg };
            }
        }
    }
}

// ------------------------------------------------------------------------- settling

/// Every line of the scene in exactly one shot: unknown ids dropped, duplicates keep the first
/// (a shot whose `/lines` the user locked keeps its lines first), a missing line goes to the
/// shot holding the line before it in the script (else the one after it), each shot's lines in
/// script order.
pub fn assign_lines(shots: &mut [Shot], lines: &[Line]) {
    if shots.is_empty() {
        return;
    }
    let order: BTreeMap<&str, usize> = lines.iter().enumerate().map(|(i, l)| (l.id.as_str(), i)).collect();
    let pinned = |s: &Shot| is_locked("/lines", &s.locked);
    let mut owned: BTreeSet<String> = BTreeSet::new();
    for s in shots.iter_mut().filter(|s| pinned(s)) {
        s.lines.retain(|id| order.contains_key(id.as_str()) && owned.insert(id.clone()));
    }
    for s in shots.iter_mut().filter(|s| !pinned(s)) {
        s.lines.retain(|id| order.contains_key(id.as_str()) && owned.insert(id.clone()));
    }
    // Where the plan itself put each line (script index -> shot), before any is filled in.
    let placed: BTreeMap<usize, usize> = shots.iter().enumerate().flat_map(|(si, s)| s.lines.iter().filter_map(|id| order.get(id.as_str()).map(|&k| (k, si))).collect::<Vec<_>>()).collect();
    for (k, line) in lines.iter().enumerate() {
        if owned.contains(&line.id) {
            continue;
        }
        let earlier = placed.range(..k).next_back().map(|(_, &si)| si);
        let later = || placed.range(k + 1..).next().map(|(_, &si)| si);
        // Nobody carries a line: share them out over the shots in order.
        let mut target = earlier.or_else(later).unwrap_or_else(|| (k * shots.len() / lines.len().max(1)).min(shots.len() - 1));
        // Prefer a shot the user has not pinned.
        if pinned(&shots[target]) {
            if let Some(free) = (target..shots.len()).chain((0..target).rev()).find(|&i| !pinned(&shots[i])) {
                target = free;
            }
        }
        shots[target].lines.push(line.id.clone());
        owned.insert(line.id.clone());
    }
    for s in shots.iter_mut() {
        s.lines.sort_by_key(|id| order.get(id.as_str()).copied().unwrap_or(usize::MAX));
    }
}

fn shot_lines<'a>(s: &Shot, lines: &'a [Line]) -> Vec<&'a Line> {
    s.lines.iter().filter_map(|id| lines.iter().find(|l| &l.id == id)).collect()
}

/// Default length of a shot the reply gave no seconds for.
fn default_secs(scene: &Scene, s: &Shot, first_in_new_place: bool) -> f32 {
    // A bridge only carries the camera across the line: short, whatever its size.
    if s.bridge {
        return BRIDGE_S;
    }
    if first_in_new_place {
        return pacing::NEW_PLACE_FIRST_S;
    }
    let m = pacing::pace(scene.kind).median_cut_s;
    m * match s.camera.size {
        ShotSize::Els | ShotSize::Ls | ShotSize::Overhead => 1.6,
        ShotSize::Insert => 0.7,
        ShotSize::Ecu => 0.6,
        ShotSize::Cu => 0.8,
        _ => 1.,
    }
}

/// Lines in exactly one shot, every shot at least as long as its lines take (and 0.4-20 s),
/// mouths that match who is seen, motion that matches the camera. Fields the user locked stay.
pub fn settle(shots: &mut [Shot], input: &SceneInput) {
    assign_lines(shots, input.lines);
    let kind = input.scene.kind;
    for (i, s) in shots.iter_mut().enumerate() {
        let ls: Vec<Line> = shot_lines(s, input.lines).into_iter().cloned().collect();
        if !is_locked("/planned_s", &s.locked) {
            let floor = pacing::spoken_seconds(&ls, kind);
            let mut secs = if s.planned_s.is_finite() && s.planned_s > 0. { s.planned_s } else { default_secs(input.scene, s, i == 0 && input.new_place) };
            secs = secs.clamp(pacing::CUT_MIN_S, pacing::CUT_MAX_S).max(floor);
            s.planned_s = (secs * 10.).ceil() / 10.;
        }
        if !is_locked("/mouth", &s.locked) {
            s.mouth = mouth_for(s, &ls);
        }
        if !is_locked("/motion", &s.locked) {
            s.motion = motion_for(s);
        }
        if s.source.is_empty() && !is_locked("/origin", &s.locked) {
            s.origin = Origin::Anime;
        }
    }
}

/// The mouth policy a shot's lines and marks allow.
fn mouth_for(s: &Shot, ls: &[Line]) -> MouthPolicy {
    let speakers: Vec<&str> = ls.iter().filter(|l| l.kind == LineKind::Speech && !line_text(l).is_empty()).filter_map(|l| l.speaker.as_deref()).collect();
    let anyone_speaks = ls.iter().any(|l| l.kind == LineKind::Speech && !line_text(l).is_empty());
    if !anyone_speaks {
        // Thoughts, narration, effects or silence: no mouth moves.
        return if matches!(s.mouth, MouthPolicy::Flap | MouthPolicy::Generate) { MouthPolicy::Closed } else { s.mouth };
    }
    let seen: Vec<&Mark> = s.marks.iter().filter(|m| speakers.contains(&m.character.as_str()) && m.visible != Visibility::Offscreen).collect();
    if seen.is_empty() {
        return MouthPolicy::Offscreen;
    }
    if seen.iter().all(|m| matches!(m.visible, Visibility::Back | Visibility::Ots)) {
        return MouthPolicy::Back;
    }
    match s.mouth {
        // The speaker is in the frame after all.
        MouthPolicy::Offscreen | MouthPolicy::Back => {
            if seen.iter().any(|m| m.visible == Visibility::Full) {
                MouthPolicy::Flap
            } else {
                MouthPolicy::Hidden
            }
        }
        // A speaker seen in full with the mouth "closed": the line would come from nowhere.
        MouthPolicy::Closed if seen.iter().any(|m| m.visible == Visibility::Full) => MouthPolicy::Flap,
        other => other,
    }
}

/// The motion kind the mouth and the camera move imply.
fn motion_for(s: &Shot) -> MotionKind {
    let moving = !matches!(s.camera.movement.kind, Move::Hold | Move::RackFocus);
    match s.motion {
        MotionKind::Hold if s.mouth == MouthPolicy::Flap => MotionKind::MouthOnly,
        MotionKind::Hold if moving => MotionKind::CameraOnly,
        MotionKind::MouthOnly if s.mouth != MouthPolicy::Flap => {
            if moving {
                MotionKind::CameraOnly
            } else {
                MotionKind::Hold
            }
        }
        MotionKind::CameraOnly if !moving => {
            if s.mouth == MouthPolicy::Flap {
                MotionKind::MouthOnly
            } else {
                MotionKind::Hold
            }
        }
        m => m,
    }
}

// ------------------------------------------------------------------------- the fallback plan

/// A plan built from the coverage patterns when the AI gave no usable shot: the arrival funnel
/// in a new place, then the conversation pattern carrying the lines two at a time, back to its
/// master every few cuts.
pub fn fallback_plan(input: &SceneInput) -> Vec<Shot> {
    // The speakers are the conversation's pair (the hero first among them), then the rest.
    let ids = talk_cast(input);
    let scene = input.scene;
    let mut shots = Vec::new();
    if input.new_place {
        shots.extend(patterns::template(Pattern::Arrival, scene, &ids));
    }
    // Steps that frame characters the scene does not have frame nobody: they are left out
    // (unless nobody is in the scene at all, when the place carries every shot).
    let all = patterns::template(Pattern::Conversation, scene, &ids);
    let keep: Vec<usize> = (0..all.len()).filter(|&i| ids.is_empty() || frames_someone(&all[i])).collect();
    let talk_steps: Vec<usize> = patterns::def(Pattern::Conversation).map(|d| d.steps.iter().enumerate().filter(|(_, s)| s.talk).map(|(i, _)| i).collect()).unwrap_or_default();
    let talkers: Vec<usize> = keep.iter().enumerate().filter(|(_, step)| talk_steps.contains(step)).map(|(i, _)| i).collect();
    let conv: Vec<Shot> = keep.iter().filter_map(|&i| all.get(i).cloned()).collect();
    let mut cursor = 0;
    let chunks: Vec<&[Line]> = input.lines.chunks(2).collect();
    for chunk in &chunks {
        // Silent steps (inserts) between the talking ones keep their place in the pattern.
        loop {
            let step = cursor % conv.len().max(1);
            cursor += 1;
            let mut s = conv.get(step).cloned().unwrap_or_default();
            if talkers.is_empty() || talkers.contains(&step) {
                s.lines = chunk.iter().map(|l| l.id.clone()).collect();
                shots.push(s);
                break;
            }
            shots.push(s);
        }
    }
    if shots.is_empty() {
        shots.extend(conv.into_iter().take(1));
    }
    for s in &mut shots {
        s.tells = format!("{} (basic plan)", s.tells);
    }
    place_shots(&mut shots, input);
    settle(&mut shots, input);
    for s in &mut shots {
        s.suggested = suggestion(s);
    }
    shots
}

// ------------------------------------------------------------------------- planning

/// Scenes with more lines than this are planned in parts, a run of beats at a time (one answer
/// for 120 lines would be cut off).
pub const SPLIT_LINES: usize = 40;

/// The scene's lines in planning parts: one part up to [`SPLIT_LINES`] lines; else runs of
/// lines in script order, cut where a beat ends once a part has three quarters of the limit,
/// and at the limit in any case.
fn line_parts(input: &SceneInput) -> Vec<BTreeSet<String>> {
    if input.lines.len() <= SPLIT_LINES {
        return vec![input.lines.iter().map(|l| l.id.clone()).collect()];
    }
    let beat_of = |id: &str| input.scene.beats.iter().position(|b| b.lines.iter().any(|x| x == id));
    let mut parts: Vec<BTreeSet<String>> = Vec::new();
    let mut cur: BTreeSet<String> = BTreeSet::new();
    let mut prev: Option<usize> = None;
    for l in input.lines {
        let beat = beat_of(&l.id);
        let new_beat = !cur.is_empty() && beat != prev;
        if cur.len() >= SPLIT_LINES || (new_beat && cur.len() >= SPLIT_LINES * 3 / 4) {
            parts.push(std::mem::take(&mut cur));
        }
        cur.insert(l.id.clone());
        prev = beat;
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

/// Plans one scene: the planner call, the reply read and settled, the checks, and up to
/// [`REVISIONS`] revise calls while any error or two warnings remain; a revision is kept only
/// when its score (error 3, warning 1) is lower. A reply with no usable shot, or one that cannot
/// be read at all, falls back to the coverage patterns (noted; an unreadable reply is kept in
/// `Plan::unreadable`). A scene with more than [`SPLIT_LINES`] lines is planned in parts, a run
/// of beats each, and checked whole. Fails only on a cancel or when the AI cannot be reached.
pub async fn plan_scene(ai: &Ai, input: &SceneInput<'_>, progress: &(dyn Fn(String) + Send + Sync), cancel: &CancellationToken) -> Result<Plan> {
    let parts = line_parts(input);
    if parts.len() <= 1 {
        return plan_part(ai, input, progress, cancel).await;
    }
    let scene = input.scene;
    let mut plan = Plan::default();
    let mut unreadable = 0;
    for (k, ids) in parts.iter().enumerate() {
        let lines: Vec<Line> = input.lines.iter().filter(|l| ids.contains(&l.id)).cloned().collect();
        // The part's beats: those carrying its lines, and the silent beats after them.
        let mut sub = scene.clone();
        let mut owner = 0usize;
        sub.beats = Vec::new();
        for b in &scene.beats {
            let mine: Vec<String> = b.lines.iter().filter(|id| ids.contains(*id)).cloned().collect();
            let silent = !b.lines.iter().any(|id| parts.iter().any(|g| g.contains(id)));
            if (silent && owner == k) || !mine.is_empty() {
                sub.beats.push(Beat { lines: mine, ..b.clone() });
            }
            if let Some(p) = b.lines.iter().rev().find_map(|id| parts.iter().position(|g| g.contains(id))) {
                owner = p;
            }
        }
        sub.target_s = scene.target_s * lines.len() as f32 / input.lines.len().max(1) as f32;
        let part_input = SceneInput { scene: &sub, lines: &lines, bible: input.bible, new_place: input.new_place && k == 0 };
        progress(format!("The scene is long: planning part {} of {}", k + 1, parts.len()));
        let p = plan_part(ai, &part_input, progress, cancel).await?;
        plan.shots.extend(p.shots);
        plan.notes.extend(p.notes);
        plan.revisions = plan.revisions.max(p.revisions);
        plan.fallback |= p.fallback;
        if p.unreadable.is_some() {
            unreadable += 1;
            plan.unreadable = p.unreadable;
        }
    }
    if unreadable < parts.len() {
        plan.unreadable = None;
    }
    settle(&mut plan.shots, input);
    plan.issues = checks::check_scene(scene, &plan.shots, input.lines, input.bible, input.new_place);
    plan.notes.push(note(
        &scene.id,
        Severity::Info,
        "plan-parts",
        format!("This scene has {} lines, so it was planned in {} parts.", input.lines.len(), parts.len()),
        Some("Check the cuts where one part ends and the next begins.".into()),
    ));
    Ok(plan)
}

/// [`plan_scene`] for one call's worth of lines.
async fn plan_part(ai: &Ai, input: &SceneInput<'_>, progress: &(dyn Fn(String) + Send + Sync), cancel: &CancellationToken) -> Result<Plan> {
    let scene = input.scene;
    let (system, user) = prompt(input);
    progress(format!("Planning the shots of \"{}\"", if scene.title.is_empty() { &scene.id } else { &scene.title }));
    let (v, unreadable) = match ai.ask_json(&system, user, Vec::new(), AiEffort::High, progress, cancel).await {
        Ok(v) => (Some(v), None),
        Err(e) if llm::is_cancelled(&e) => return Err(e),
        Err(e) => match e.downcast::<BadReply>() {
            Ok(bad) => (None, Some(bad.raw)),
            Err(e) => return Err(e),
        },
    };
    let (mut shots, mut notes) = match &v {
        Some(v) => read_reply(input, v),
        None => (Vec::new(), Vec::new()),
    };
    let mut fallback = false;
    if shots.is_empty() {
        fallback = true;
        let message = if unreadable.is_some() {
            "The AI's answer could not be read, so a basic plan was built from the coverage patterns."
        } else {
            "The AI's answer held no usable shot, so a basic plan was built from the coverage patterns."
        };
        notes.push(note(&scene.id, Severity::Warning, "plan-fallback", message.into(), Some("Plan the scene again, or edit the basic plan.".into())));
        shots = fallback_plan(input);
    }
    let mut issues = checks::check_scene(scene, &shots, input.lines, input.bible, input.new_place);
    let mut revisions = 0u8;
    while revisions < REVISIONS && checks::needs_revision(&issues) {
        revisions += 1;
        let (errors, warnings) = (issues.iter().filter(|i| i.severity == Severity::Error).count(), issues.iter().filter(|i| i.severity == Severity::Warning).count());
        progress(format!("The check sent the plan back ({}, {}); revision {revisions} of {REVISIONS}", counted(errors, "error"), counted(warnings, "warning")));
        let (system, user) = revise_prompt(input, &shots, &issues);
        let v = match ai.ask_json(&system, user, Vec::new(), AiEffort::High, progress, cancel).await {
            Ok(v) => v,
            Err(e) if llm::is_cancelled(&e) => return Err(e),
            Err(e) => {
                progress(format!("The revision could not be used ({e}); keeping the plan as it is."));
                break;
            }
        };
        let (new, new_notes) = read_reply(input, &v);
        if new.is_empty() {
            progress("The revision held no usable shot; keeping the plan as it is.".into());
            continue;
        }
        let new_issues = checks::check_scene(scene, &new, input.lines, input.bible, input.new_place);
        if checks::score(&new_issues) < checks::score(&issues) {
            progress(format!("Revision {revisions} is better (score {} → {}); keeping it.", checks::score(&issues), checks::score(&new_issues)));
            shots = new;
            issues = new_issues;
            notes = new_notes;
            fallback = false;
        } else {
            progress(format!("Revision {revisions} is not better; keeping the earlier plan."));
        }
    }
    Ok(Plan { shots, issues, notes, revisions, fallback, unreadable })
}

// ------------------------------------------------------------------------- merging

/// The user's work in a shot: an edit, a lock, or a picture.
fn has_work(s: &Shot) -> bool {
    s.edited || !s.locked.is_empty() || has_pictures(s)
}

fn has_pictures(s: &Shot) -> bool {
    !s.frame.takes.is_empty() || s.frame.selected.is_some() || s.frame.locked || s.clip.as_ref().is_some_and(|c| !c.takes.is_empty() || c.selected.is_some())
}

/// What a shot with pictures keeps through a re-plan: everything its picture shows. Only its
/// lines and timing (and what follows from them) move with the new plan.
const PICTURED: [&str; 12] = ["/tells", "/origin", "/source", "/pattern", "/camera", "/marks", "/background", "/route", "/hit", "/flash", "/bridge", "/transition"];

/// `old` with the AI's `new` written into every field the user did not lock (cards untouched).
/// A shot with pictures also keeps what its picture shows ([`PICTURED`]).
fn merged(old: &Shot, new: &Shot) -> Shot {
    let (Ok(mut cur), Ok(ai)) = (serde_json::to_value(old), serde_json::to_value(new)) else { return old.clone() };
    let mut locked = old.locked.clone();
    if has_pictures(old) {
        locked.extend(PICTURED.iter().map(|p| p.to_string()));
    }
    merge_unlocked(&mut cur, &ai, &locked);
    serde_json::from_value::<Shot>(cur).unwrap_or_else(|_| old.clone())
}

/// Merges a new plan into the current shots: edited, locked or pictured shots stay (by id);
/// new shots fill around them by line coverage; a shot with takes is never deleted.
pub fn merge(old: &[Shot], new: Vec<Shot>) -> Vec<Shot> {
    merge_report(old, new, "").0
}

/// [`merge`], plus an Info note for every pictured shot the new plan no longer covers (kept at
/// the end: "kept: has pictures").
///
/// A shot with the user's work is matched to the new shot sharing most of its lines (same
/// position when neither has lines) and takes the new shot's place with the AI's values in
/// every unlocked field, keeping its id, locks and pictures. An unmatched shot with edits or
/// locks stays at its old position; one with only pictures moves to the end. Shots without
/// any of the user's work are replaced.
pub fn merge_report(old: &[Shot], new: Vec<Shot>, scene: &str) -> (Vec<Shot>, Vec<Issue>) {
    let mut out = new;
    let mut used = vec![false; out.len()];
    let mut leftovers: Vec<(usize, &Shot)> = Vec::new();
    for (oi, k) in old.iter().enumerate().filter(|(_, s)| has_work(s)) {
        let theirs: BTreeSet<&str> = k.lines.iter().map(String::as_str).collect();
        let best = out
            .iter()
            .enumerate()
            .filter(|(i, _)| !used[*i])
            .map(|(i, n)| (n.lines.iter().filter(|l| theirs.contains(l.as_str())).count(), i))
            .filter(|(o, _)| *o > 0)
            .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
            .map(|(_, i)| i)
            .or_else(|| (k.lines.is_empty() && oi < out.len() && !used[oi] && out[oi].lines.is_empty()).then_some(oi));
        match best {
            Some(i) => {
                used[i] = true;
                out[i] = merged(k, &out[i]);
            }
            None => leftovers.push((oi, k)),
        }
    }
    let mut notes = Vec::new();
    let mut at_end = Vec::new();
    for (oi, k) in leftovers {
        if k.edited || !k.locked.is_empty() {
            let at = oi.min(out.len());
            out.insert(at, k.clone());
        } else {
            at_end.push(k.clone());
        }
    }
    let first_end = out.len();
    for (j, k) in at_end.iter().enumerate() {
        let what = if k.tells.trim().is_empty() { String::new() } else { format!(" (\u{201c}{}\u{201d})", clip_words(&k.tells, 40)) };
        notes.push(Issue {
            code: "plan-kept".into(),
            severity: Severity::Info,
            scene: scene.to_string(),
            shot: Some(k.id.clone()).filter(|id| !id.is_empty()),
            message: format!("Cut {}{what} was kept at the end of the scene: it has pictures, and the new plan no longer uses it.", first_end + j + 1),
            fix: Some("Move it where it belongs or delete it yourself.".into()),
        });
    }
    out.extend(at_end);
    (out, notes)
}

/// The first `max` characters of a text, with "…" when cut.
fn clip_words(s: &str, max: usize) -> String {
    let t: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= max {
        return t;
    }
    let cut: String = t.chars().take(max).collect();
    format!("{}…", cut.trim_end())
}

// ------------------------------------------------------------------------- the job

/// Gives every shot without an id the next "sh0001"-style id from the project's counter.
fn assign_ids(store: &Store, episode: &str, shots: &mut [Shot]) -> Result<()> {
    if shots.iter().all(|s| !s.id.is_empty()) {
        return Ok(());
    }
    // The shot files are read first (slow); then the counter is read, raised and saved under
    // the one counters lock inside `project.json`'s own lock (`story::bump_ids`): two PlanScene
    // jobs, or a job and the Shots screen, never hand out the same id, and nothing saves a
    // stale copy of the project file.
    let mut taken: BTreeSet<String> = store.all_shots(episode).unwrap_or_default().into_values().flatten().map(|s| s.id).collect();
    taken.extend(shots.iter().map(|s| s.id.clone()));
    store.bump_counters(|counters| {
        let mut n = counters.get("shot").copied().unwrap_or(0);
        for s in shots.iter_mut().filter(|s| s.id.is_empty()) {
            loop {
                n += 1;
                let id = format!("sh{n:04}");
                if taken.insert(id.clone()) {
                    s.id = id;
                    break;
                }
            }
        }
        counters.insert("shot".into(), n);
    })?;
    Ok(())
}

/// The PlanScene job: plans (or re-plans, keeping locks) one scene and saves its shots.
pub async fn run_job(cx: &JobCtx<'_>, episode: &str, scene: &str) -> Result<()> {
    (cx.progress)(0.02, "Reading the story and the bible");
    let story = cx.store.story(episode)?;
    let sc = story.scenes.iter().find(|s| s.id == scene).ok_or_else(|| anyhow!("This scene is no longer in the story. Adapt the story again or pick another scene."))?;
    let bible = cx.store.bible()?;
    let lines = scene_lines(&story, sc);
    let input = SceneInput { scene: sc, lines: &lines, bible: &bible, new_place: opens_new_place(&story, scene) };
    let ai = (cx.run.ai)(Tier::Planner).ok_or_else(|| anyhow!(NO_AI))?;
    let progress = cx.progress;
    let say = move |s: String| progress(0.4, &s);
    let plan = plan_scene(&ai, &input, &say, cx.cancel).await?;
    let current: SceneShots = cx.store.scene_shots(episode, scene)?;
    if let Some(raw) = &plan.unreadable {
        // Kept for the user (or a bug report).
        let dir = cx.store.work_dir().join("ai");
        let file = dir.join(format!("plan-{}-{}.txt", cx.job.id, mv_core::paths::safe_name(scene)));
        if let Err(io) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, raw)) {
            mv_core::logging::global().warn("anime", &format!("could not keep the AI's unreadable answer: {io}"));
        }
        // A scene that already has a plan keeps it rather than a basic one.
        if !current.shots.is_empty() {
            return Err(anyhow::Error::new(BadReply { raw: raw.clone() }));
        }
    }
    (cx.progress)(0.9, "Saving the shots");
    // Merged onto the scene's shots as they are on disk now, under that file's own lock, so a
    // Frame job recording its pictures (or a screen's edit) at the same moment is kept. Lock
    // order: the scene file, then the counters and `project.json` (`assign_ids`); nothing takes
    // them the other way round.
    let (plan_shots, revisions, model) = (plan.shots, plan.revisions, ai.model.model.clone());
    let mut out: Result<Vec<Issue>> = Ok(Vec::new());
    let current = cx.store.update_scene_shots_with(episode, scene, true, |current| {
        let (mut shots, notes) = merge_report(&current.shots, plan_shots, scene);
        settle(&mut shots, &input);
        if let Err(e) = assign_ids(cx.store, episode, &mut shots) {
            // Nothing changed: the file is saved as it was.
            out = Err(e);
            return;
        }
        current.episode = episode.to_string();
        current.scene = scene.to_string();
        current.shots = shots;
        current.planned_by = model;
        current.revisions = revisions;
        out = Ok(notes);
    })?;
    let mut notes = out?;
    notes.extend(plan.notes);

    // The episode's check results: every scene's checks now, plus the planning notes.
    let all = cx.store.all_shots(episode)?;
    let mut issues = checks::check_episode(&story, &all, &bible);
    let earlier = cx.store.checks(episode).unwrap_or_default();
    issues.extend(earlier.into_iter().filter(|i| i.code.starts_with("plan-") && i.scene != scene));
    let fresh: Vec<Issue> = notes.into_iter().map(|mut n| {
        n.scene = scene.to_string();
        n
    }).collect();
    issues.extend(fresh);
    if let Err(e) = cx.store.save_checks(episode, &issues) {
        mv_core::logging::global().warn("anime", &format!("could not save the check results: {e:#}"));
    }
    let mine = issues.iter().filter(|i| i.scene == scene && !i.code.starts_with("plan-"));
    let (errors, warnings) = mine.fold((0, 0), |(e, w), i| match i.severity {
        Severity::Error => (e + 1, w),
        Severity::Warning => (e, w + 1),
        _ => (e, w),
    });
    (cx.progress)(1.0, &format!("{} planned; {}, {}", counted(current.shots.len(), "shot"), counted(errors, "error"), counted(warnings, "warning")));
    Ok(())
}

/// "1 shot", "3 shots", "0 errors".
fn counted(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}
