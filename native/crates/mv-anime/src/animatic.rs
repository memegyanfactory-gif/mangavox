//! CUT, the animatic (design 8.2): the cut plan (stills, timing, moves, voice offsets), the
//! camera curve shared by the in-page preview and the export, and the MP4 render (per-shot
//! zoompan segments, concat, dialogue track summed in Rust, loudness, optional subtitles).
//!
//! Timing: a shot lasts `pacing::effective_s` (its locked length, else the longer of its
//! planned length and its voiced lines), minus the user's trims, snapped to whole frames at
//! 24 fps. Its first line starts the scene kind's measured lead after the cut (0.5 to 1.2 s),
//! the next ones 0.35 s apart.
//!
//! The camera never freezes (a hold creeps in 1.5%), every move is eased in and out, a push or
//! pull runs at the pace of one lasting at least 3 s, and a pan never slides faster than one
//! frame width in 7 s (plan 11, research 10 section 8). [`camera_at`] and [`zoompan_exprs`] are
//! the same curve, so the preview and the MP4 match frame for frame.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use mv_ai::CancellationToken;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::ffmpeg::{self, SegmentSpec, Subs, VideoCodec, AUDIO_RATE, FPS};
use crate::model::{FocusTarget, Line, LookAt, Move, Place, SceneKind, Shot, Speed, Status, Target, TakeKind, TakeMeta, TakeSource, SCHEMA};
use crate::pacing;
use crate::queue::JobCtx;
use crate::store::{self, Store};

/// The animatic's picture size.
pub const DEFAULT_SIZE: (u32, u32) = (1280, 720);
/// Each levelled line's gain in the sum: every line already sits at [`DIALOGUE_LUFS`] with its
/// peaks under -1.5 dBFS, and the soft limiter catches overlaps (the mux's loudnorm then only
/// trims the whole track to -16 LUFS).
pub const DIALOGUE_GAIN: f32 = 1.0;
/// The creep push of a held shot (anime never fully freezes).
pub const HOLD_CREEP: f32 = 0.015;
/// Shake: share of the frame width, decaying over the shot.
pub const SHAKE_AMP: f32 = 0.012;
/// A push or pull moves at the pace of one lasting at least this long.
pub const PUSH_MIN_S: f32 = 3.0;
/// A pan slides at most one frame width in this many seconds.
pub const PAN_FRAME_S: f32 = 7.0;
/// The placeholder card's colour.
const PLACEHOLDER_RGB: [u8; 3] = [0x1b, 0x1b, 0x1f];
const NO_SHOTS: &str = "This episode has no shots yet. Plan the shots in the Shots step first.";

/// `episodes/<ep_id>/cut.json` (derived + the user's trims).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CutPlan {
    pub schema: u32,
    pub shots: Vec<CutShot>,
    pub seconds: f32,
    pub timing_hash: String,
    pub subtitles: bool,
    pub burn_subtitles: bool,
    /// 1280 x 720.
    pub size: (u32, u32),
    /// The settings the last export was made with (written with `timing_hash` when a render
    /// finishes); `None` for an export made before they were recorded.
    pub exported_with: Option<ExportSettings>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// What changes an export's file without changing its timing: how the subtitles go in.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ExportSettings {
    /// "off", "file" (a subtitle track) or "burned" (drawn into the picture), as asked.
    pub subtitles: String,
}

/// The export settings a cut plan asks for now.
pub fn export_settings(plan: &CutPlan) -> ExportSettings {
    let subtitles = if plan.burn_subtitles {
        "burned"
    } else if plan.subtitles {
        "file"
    } else {
        "off"
    };
    ExportSettings { subtitles: subtitles.into() }
}

/// The last export (recorded in `saved`, the cut file) matches the cut as it would be made now
/// (`now`, from [`plan_cut`]): the same timing and the same export settings. An export whose
/// settings were not recorded is not reported up to date.
pub fn export_current(saved: &CutPlan, now: &CutPlan) -> bool {
    !saved.timing_hash.is_empty() && saved.timing_hash == now.timing_hash && saved.exported_with.as_ref() == Some(&export_settings(now))
}

/// One shot of the cut.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CutShot {
    pub shot: String,
    /// Selected frame take file; `None` = a placeholder card.
    pub still: Option<String>,
    pub start_s: f32,
    pub seconds: f32,
    pub trim_in: f32,
    pub trim_out: f32,
    pub mv: Move,
    pub speed: Speed,
    /// (voice take file, offset in the shot).
    pub lines: Vec<(String, f32)>,
    /// What is missing (no still, no voice).
    pub missing: Vec<String>,
    /// Share of the frame the picture grows or shrinks by over the shot (push, pull, crash zoom).
    pub zoom: f32,
    /// Frame widths (or heights) the picture slides over the shot (pans, tilts, trucks, whips).
    pub travel: f32,
    /// Screen x (0..1) of the character a push centres on, when the floor plan knows it.
    pub focus_x: Option<f32>,
    /// (start, end in the shot, text) of every spoken line, voiced or not: the subtitles.
    pub subs: Vec<(f32, f32, String)>,
}

/// Whole frames at 24 fps (at least one).
pub fn frames(seconds: f32) -> u32 {
    if seconds.is_finite() && seconds > 0. {
        ((seconds * FPS as f32).round() as u32).max(1)
    } else {
        1
    }
}

fn snap(seconds: f32) -> f32 {
    frames(seconds) as f32 / FPS as f32
}

fn clean(x: f32) -> f32 {
    if x.is_finite() && x > 0. {
        x
    } else {
        0.
    }
}

/// How much faster than usual a move of this speed goes.
pub fn speed_factor(s: Speed) -> f32 {
    match s {
        Speed::Creep => 0.5,
        Speed::Slow => 0.75,
        Speed::Fast => 1.5,
        Speed::Snap => 2.5,
        _ => 1.,
    }
}

/// `(zoom, travel)` of a move over a cut of `seconds`: `pacing::camera_extent` with the speed,
/// then the anime limits: a push or pull at the pace of a 3 s one at least, a pan at most one
/// frame width in 7 s.
pub fn move_extent(kind: SceneKind, mv: Move, seconds: f32, amount: f32, speed: Speed) -> (f32, f32) {
    let a = if amount.is_finite() && amount > 0. { amount } else { 1. } * speed_factor(speed);
    let (mut z, mut t) = pacing::camera_extent(kind, mv, seconds, a);
    match mv {
        Move::PushIn | Move::PullOut if seconds < PUSH_MIN_S => z *= clean(seconds) / PUSH_MIN_S,
        Move::PanLeft | Move::PanRight | Move::TiltUp | Move::TiltDown | Move::TruckLeft | Move::TruckRight | Move::BgSlide => {
            t = t.min(clean(seconds) / PAN_FRAME_S)
        }
        _ => {}
    }
    (z, t)
}

/// Screen x of the character the camera is about: the focus target, else the look-at
/// character, else the only mark.
fn focus_x(shot: &Shot, place: Option<&Place>) -> Option<f32> {
    let id = match &shot.camera.focus.target {
        FocusTarget::Character(id) => Some(id),
        _ => None,
    }
    .or(match &shot.camera.look_at {
        LookAt::Character(id) => Some(id),
        _ => None,
    })
    .or((shot.marks.len() == 1).then(|| &shot.marks[0].character))?;
    let d = crate::geometry::derive(shot, place);
    d.screen_x.get(id).copied().filter(|x| x.is_finite()).map(|x| x.clamp(0., 1.))
}

fn cut_path(store: &Store, ep: &str) -> PathBuf {
    store.path(&format!("episodes/{}/cut.json", mv_core::paths::safe_name(ep)))
}

/// Scenes in the episode's order, then the story's, then any other scene that has shots.
fn scene_order(order: &[String], story: &[String], shots: &std::collections::BTreeMap<String, Vec<Shot>>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in order.iter().chain(story.iter()).chain(shots.keys()) {
        if shots.contains_key(id) && !out.contains(id) {
            out.push(id.clone());
        }
    }
    out
}

/// One shot's place in the cut (start_s is set by the caller).
fn cut_shot(store: &Store, shot: &Shot, kind: SceneKind, place: Option<&Place>, lines: &HashMap<&str, &Line>, trims: (f32, f32)) -> CutShot {
    let mut missing = Vec::new();
    let still = shot
        .frame
        .selected
        .as_deref()
        .filter(|_| shot.frame.status != Status::Rejected)
        .and_then(|id| store.take(id))
        .map(|t| t.file)
        .filter(|f| store.path(f).is_file());
    if still.is_none() {
        missing.push("picture".to_string());
    }
    // (voice take file, seconds, text, speaker) of each line said in the shot, in order.
    let mut spoken: Vec<(Option<String>, f32, String, Option<String>)> = Vec::new();
    for id in &shot.lines {
        let Some(l) = lines.get(id.as_str()) else { continue };
        let Some(text) = crate::voices::said(l) else { continue };
        let who = l.speaker.clone().filter(|s| !s.trim().is_empty());
        let take = l.voice.selected.as_deref().and_then(|t| store.take(t)).filter(|t| store.path(&t.file).is_file());
        match take {
            Some(t) => {
                let secs = if t.seconds.is_finite() && t.seconds > 0. { t.seconds } else { pacing::line_seconds(text) };
                spoken.push((Some(t.file), secs, text.to_string(), who));
            }
            None => {
                missing.push(format!("voice {id}"));
                spoken.push((None, pacing::line_seconds(text), text.to_string(), who));
            }
        }
    }
    let secs: Vec<f32> = spoken.iter().map(|s| s.1).collect();
    let mut full = pacing::effective_s(shot, kind, &secs);
    if !(full.is_finite() && full > 0.) {
        full = pacing::pace(kind).median_cut_s;
    }
    let (ti, to) = (clean(trims.0), clean(trims.1));
    let seconds = snap((full - ti - to).max(pacing::CUT_MIN_S));
    let mut at = pacing::pace(kind).line_lead_s - ti;
    let (mut voice_lines, mut subs) = (Vec::new(), Vec::new());
    let mut prev: Option<Option<String>> = None;
    for (file, s, text, who) in spoken {
        if let Some(p) = &prev {
            at += pacing::line_gap(p.as_deref(), who.as_deref());
        }
        if let Some(f) = file {
            voice_lines.push((f, at));
        }
        subs.push((at, at + s, text));
        at += s;
        prev = Some(who);
    }
    let m = &shot.camera.movement;
    let (zoom, travel) = move_extent(kind, m.kind, seconds, m.amount, m.speed);
    let centred = matches!(m.kind, Move::PushIn | Move::PullOut | Move::CrashZoom | Move::Hold | Move::RackFocus);
    CutShot {
        shot: shot.id.clone(),
        still,
        start_s: 0.,
        seconds,
        trim_in: ti,
        trim_out: to,
        mv: m.kind,
        speed: m.speed,
        lines: voice_lines,
        missing,
        zoom,
        travel,
        focus_x: if centred { focus_x(shot, place) } else { None },
        subs,
    }
}

/// The cut plan of an episode from its shots, selected takes and timing.
pub fn plan_cut(store: &Store, ep: &str) -> Result<CutPlan> {
    let episode = store.episode(ep)?;
    let story = store.story(ep)?;
    let shots = store.all_shots(ep)?;
    let bible = store.bible()?;
    // The user's trims and switches; a damaged or newer file only loses them.
    let saved: CutPlan = store::read_json(&cut_path(store, ep)).unwrap_or_default();
    let lines: HashMap<&str, &Line> = story.lines.iter().map(|l| (l.id.as_str(), l)).collect();
    let trims: HashMap<&str, (f32, f32)> = saved.shots.iter().map(|s| (s.shot.as_str(), (s.trim_in, s.trim_out))).collect();
    let story_order: Vec<String> = story.scenes.iter().map(|s| s.id.clone()).collect();
    let mut cut = Vec::new();
    let mut at_frames: u64 = 0;
    for scene_id in scene_order(&episode.scene_order, &story_order, &shots) {
        let scene = story.scenes.iter().find(|s| s.id == scene_id);
        let kind = scene.map(|s| s.kind).unwrap_or_default();
        let place = scene.and_then(|s| s.place.as_deref()).and_then(|p| bible.place(p));
        for shot in shots.get(&scene_id).into_iter().flatten() {
            let mut cs = cut_shot(store, shot, kind, place, &lines, trims.get(shot.id.as_str()).copied().unwrap_or_default());
            cs.start_s = at_frames as f32 / FPS as f32;
            at_frames += frames(cs.seconds) as u64;
            cut.push(cs);
        }
    }
    let size = if saved.size.0 >= 16 && saved.size.1 >= 16 { (saved.size.0 & !1, saved.size.1 & !1) } else { DEFAULT_SIZE };
    let mut plan = CutPlan {
        schema: SCHEMA,
        shots: cut,
        seconds: at_frames as f32 / FPS as f32,
        timing_hash: String::new(),
        subtitles: saved.subtitles,
        burn_subtitles: saved.burn_subtitles,
        size,
        exported_with: saved.exported_with,
        extra: saved.extra,
    };
    plan.timing_hash = timing_hash(&plan);
    Ok(plan)
}

/// The timing channel (design 5.5): shot ids in order, lengths, selected stills and voice takes
/// with their offsets, moves, trims and the picture size. A changed caption, prompt or note
/// does not change it.
pub fn timing_hash(plan: &CutPlan) -> String {
    let r = |x: f32| (x as f64 * 1000.).round() / 1000.;
    let shots: Vec<Value> = plan
        .shots
        .iter()
        .map(|s| {
            json!({
                "shot": s.shot, "seconds": r(s.seconds), "still": s.still,
                "lines": s.lines.iter().map(|(f, o)| json!([f, r(*o)])).collect::<Vec<_>>(),
                "mv": s.mv, "speed": s.speed, "zoom": r(s.zoom), "travel": r(s.travel),
                "focus": s.focus_x.map(r), "trim": [r(s.trim_in), r(s.trim_out)],
            })
        })
        .collect();
    crate::hash::value_hash(&json!({ "v": 1, "size": [plan.size.0, plan.size.1], "shots": shots }))
}

// ------------------------------------------------------------------------- the camera curve

#[derive(Clone, Copy, Debug, PartialEq)]
enum Ease {
    /// Smoothstep: eased in and out.
    Smooth,
    /// Cubic ease-out: fast start, soft landing (crash zoom, whip).
    Out,
}

/// One move as numbers: zoom from `z0` to `z1`, the visible window's position in the room left
/// by the zoom (`u` across, `v` down; 0 = left/top edge, 1 = right/bottom) from 0 to 1 of the
/// eased progress, which runs over `dur` seconds of a `secs`-long shot.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Curve {
    z0: f32,
    z1: f32,
    u0: f32,
    u1: f32,
    v0: f32,
    v1: f32,
    ease: Ease,
    dur: f32,
    secs: f32,
    shake: bool,
}

/// The window position that puts screen x `fx` in the middle at zoom `z` (0.5 when unknown).
fn focus_u(fx: Option<f32>, z: f32) -> f32 {
    match fx {
        Some(f) if z > 1.0001 => ((f - 0.5 / z) / (1. - 1. / z)).clamp(0., 1.),
        _ => 0.5,
    }
}

fn curve(s: &CutShot) -> Curve {
    let secs = if s.seconds.is_finite() && s.seconds > 0. { s.seconds } else { 1. / FPS as f32 };
    let z = clean(s.zoom).max(HOLD_CREEP);
    let t = clean(s.travel);
    let base = Curve { z0: 1., z1: 1., u0: 0.5, u1: 0.5, v0: 0.5, v1: 0.5, ease: Ease::Smooth, dur: secs, secs, shake: false };
    let side = |u0: f32, u1: f32| Curve { z0: 1. + t, z1: 1. + t, u0, u1, ..base };
    let tilt = |v0: f32, v1: f32| Curve { z0: 1. + t, z1: 1. + t, v0, v1, ..base };
    match s.mv {
        Move::PushIn => {
            let u = focus_u(s.focus_x, 1. + z);
            Curve { z1: 1. + z, u0: u, u1: u, ..base }
        }
        Move::PullOut => {
            let u = focus_u(s.focus_x, 1. + z);
            Curve { z0: 1. + z, u0: u, u1: u, ..base }
        }
        Move::CrashZoom => {
            let u = focus_u(s.focus_x, 1. + z);
            Curve { z1: 1. + z, u0: u, u1: u, ease: Ease::Out, dur: secs.min(0.6), ..base }
        }
        // The camera turns left: the window travels from the right edge to the left one.
        Move::PanLeft | Move::TruckLeft => side(1., 0.),
        Move::PanRight | Move::TruckRight | Move::BgSlide => side(0., 1.),
        Move::TiltUp => tilt(1., 0.),
        Move::TiltDown => tilt(0., 1.),
        Move::Whip => Curve { ease: Ease::Out, dur: secs.min(0.4), ..side(0., 1.) },
        Move::Shake => Curve { z0: 1.04, z1: 1.04, shake: true, ..base },
        // Hold, rack focus (the still cannot refocus) and anything newer: the creep push.
        _ => {
            let u = focus_u(s.focus_x, 1. + HOLD_CREEP);
            Curve { z1: 1. + HOLD_CREEP, u0: u, u1: u, ..base }
        }
    }
}

fn ease(kind: Ease, p: f32) -> f32 {
    let p = p.clamp(0., 1.);
    match kind {
        Ease::Smooth => p * p * (3. - 2. * p),
        Ease::Out => 1. - (1. - p).powi(3),
    }
}

/// (zoom, x, y) of the shot's camera at `t` seconds: the same smoothstep curves the export
/// gives ffmpeg's zoompan, so the preview matches the MP4.
///
/// `zoom` ≥ 1 is how much the still is enlarged; `x`, `y` are the visible window's top-left
/// corner as shares of the still's width and height (0 ..= 1 − 1/zoom), exactly zoompan's
/// `x/iw`, `y/ih`. To draw it: scale the still by `zoom` and shift it by
/// (−x · zoom · view width, −y · zoom · view height).
pub fn camera_at(shot: &CutShot, t: f32) -> (f32, f32, f32) {
    let c = curve(shot);
    let t = if t.is_finite() { t.clamp(0., c.secs) } else { 0. };
    let e = ease(c.ease, t / c.dur);
    let zoom = c.z0 + (c.z1 - c.z0) * e;
    let room = 1. - 1. / zoom;
    let mut x = room * (c.u0 + (c.u1 - c.u0) * e);
    let y = room * (c.v0 + (c.v1 - c.v0) * e);
    if c.shake {
        let n = t * FPS as f32;
        x = (x + SHAKE_AMP * (n * 2.1).sin() * (-6. * t / c.secs).exp()).clamp(0., room);
    }
    (zoom, x, y)
}

/// zoompan's `z`, `x`, `y` for the shot: [`camera_at`] written as ffmpeg expressions of the
/// output frame number `on` (0-based, at 24 fps, so `t = on / 24`).
pub fn zoompan_exprs(shot: &CutShot) -> (String, String, String) {
    let c = curve(shot);
    let f = |x: f32| format!("{x:.6}");
    let p = format!("min(1,on/{})", f((c.dur * FPS as f32).max(1.)));
    let e = match c.ease {
        Ease::Smooth => format!("(({p})*({p})*(3-2*({p})))"),
        Ease::Out => format!("(1-pow(1-({p}),3))"),
    };
    let z = format!("{}+({})*{e}", f(c.z0), f(c.z1 - c.z0));
    let mut x = format!("(iw-iw/zoom)*({}+({})*{e})", f(c.u0), f(c.u1 - c.u0));
    if c.shake {
        x.push_str(&format!("+iw*{}*sin(on*2.1)*exp(-6*on/{})", f(SHAKE_AMP), f(c.secs * FPS as f32)));
    }
    let y = format!("(ih-ih/zoom)*({}+({})*{e})", f(c.v0), f(c.v1 - c.v0));
    (z, x, y)
}

/// The cache name of a shot's rendered segment: it changes only when what the segment is made
/// of changes (the still, the move curve, the length, the size, the encoder, the card text).
pub fn segment_key(ident: &str, frames: u32, size: (u32, u32), codec: VideoCodec, exprs: &(String, String, String), text: bool) -> String {
    let h = crate::hash::value_hash(&json!({
        "v": 1, "still": ident, "frames": frames, "size": [size.0, size.1], "codec": codec.name(),
        "z": exprs.0, "x": exprs.1, "y": exprs.2, "text": text,
    }));
    h[..32].to_string()
}

// ------------------------------------------------------------------------- the dialogue mix

/// The loudness every dialogue line is levelled to before the sum (LUFS, the animatic's
/// target): a whisper and a shout, Kokoro and Chatterbox, sit at one level.
pub const DIALOGUE_LUFS: f32 = -16.;
/// The highest peak a levelled line may reach (-1.5 dBFS): a line whose peaks would pass it is
/// levelled lower instead of clipped.
pub const DIALOGUE_PEAK: f32 = 0.841;
/// Where the mix's soft limiter starts bending (about -1 dBFS); below it nothing changes.
pub const LIMIT_KNEE: f32 = 0.89;

/// One K-weighting biquad (ITU-R BS.1770 at 48 kHz).
fn biquad(x: &[f32], b: [f64; 3], a: [f64; 2]) -> Vec<f32> {
    let (mut x1, mut x2, mut y1, mut y2) = (0f64, 0f64, 0f64, 0f64);
    x.iter()
        .map(|&s| {
            let s = s as f64;
            let y = b[0] * s + b[1] * x1 + b[2] * x2 - a[0] * y1 - a[1] * y2;
            (x2, x1, y2, y1) = (x1, s, y1, y);
            y as f32
        })
        .collect()
}

/// Integrated loudness (LUFS, ITU-R BS.1770-4: K-weighting, 400 ms blocks with 75% overlap,
/// absolute gate -70 LUFS, relative gate -10 LU) of 48 kHz mono samples; `None` for silence.
/// A clip shorter than one block is measured as one block.
pub fn loudness_lufs(samples: &[i16]) -> Option<f32> {
    if samples.is_empty() {
        return None;
    }
    let x: Vec<f32> = samples.iter().map(|&s| s as f32 / 32768.).collect();
    let shelf = biquad(&x, [1.535_124_859_586_97, -2.691_696_189_406_38, 1.198_392_810_852_85], [-1.690_659_293_182_41, 0.732_480_774_215_85]);
    let k = biquad(&shelf, [1., -2., 1.], [-1.990_047_454_833_98, 0.990_072_250_366_21]);
    let block = (AUDIO_RATE as usize * 4 / 10).min(k.len());
    let step = (AUDIO_RATE as usize / 10).max(1);
    let mut powers = Vec::new();
    let mut at = 0;
    while at + block <= k.len() {
        let ms = k[at..at + block].iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>() / block as f64;
        powers.push(ms);
        at += step;
    }
    let lufs = |p: f64| -0.691 + 10. * p.max(1e-20).log10();
    let loud: Vec<f64> = powers.into_iter().filter(|p| lufs(*p) > -70.).collect();
    if loud.is_empty() {
        return None;
    }
    let rel = lufs(loud.iter().sum::<f64>() / loud.len() as f64) - 10.;
    let gated: Vec<f64> = loud.into_iter().filter(|p| lufs(*p) > rel).collect();
    (!gated.is_empty()).then(|| lufs(gated.iter().sum::<f64>() / gated.len() as f64) as f32)
}

/// The gain that brings a line to [`DIALOGUE_LUFS`] without its peak passing [`DIALOGUE_PEAK`]
/// (1 for silence; never more than +24 dB).
pub fn level_gain(samples: &[i16]) -> f32 {
    let Some(l) = loudness_lufs(samples) else { return 1. };
    let want = 10f32.powf((DIALOGUE_LUFS - l) / 20.).min(15.85);
    let peak = samples.iter().map(|s| (*s as f32 / 32768.).abs()).fold(0f32, f32::max);
    if peak <= 0. {
        return 1.;
    }
    want.min(DIALOGUE_PEAK / peak)
}

/// A line levelled by [`level_gain`].
pub fn level_line(samples: &[i16]) -> Vec<i16> {
    let g = level_gain(samples);
    if (g - 1.).abs() < 1e-4 {
        return samples.to_vec();
    }
    samples.iter().map(|&s| (s as f32 * g).round().clamp(i16::MIN as f32, i16::MAX as f32) as i16).collect()
}

/// A soft limiter: unchanged up to [`LIMIT_KNEE`], then bent smoothly toward (never past) full
/// scale, so overlapping lines never clip hard.
pub fn soft_limit(s: f32) -> f32 {
    let a = s.abs();
    if a <= LIMIT_KNEE {
        return s;
    }
    let room = 1. - LIMIT_KNEE;
    (LIMIT_KNEE + room * ((a - LIMIT_KNEE) / room).tanh()).min(0.999).copysign(s)
}

/// Sum of `clips` (start sample, 16-bit samples) times `gain` over samples `from..from+len`,
/// through the [`soft_limit`]er. A clip starting before 0 loses its beginning; one running past
/// the end is cut.
pub fn mix_block(from: i64, len: usize, clips: &[(i64, Vec<i16>)], gain: f32) -> Vec<f32> {
    let mut out = vec![0f32; len];
    let end = from + len as i64;
    for (start, samples) in clips {
        let (start, n) = (*start, samples.len() as i64);
        if start >= end || start + n <= from {
            continue;
        }
        let a = start.max(from);
        let b = (start + n).min(end);
        for i in a..b {
            out[(i - from) as usize] += samples[(i - start) as usize] as f32 / 32768. * gain;
        }
    }
    for s in &mut out {
        *s = soft_limit(*s);
    }
    out
}

/// [`mix_block`] over the whole track.
pub fn sum_clips(total: usize, clips: &[(i64, Vec<i16>)], gain: f32) -> Vec<f32> {
    mix_block(0, total, clips, gain)
}

/// A normalised take (48 kHz mono 16-bit) as samples.
fn read_norm(path: &Path) -> Result<Vec<i16>> {
    let mut r = hound::WavReader::open(path).context("A prepared voice line could not be read.")?;
    let spec = r.spec();
    if spec.sample_rate != AUDIO_RATE || spec.channels != 1 || spec.bits_per_sample != 16 {
        bail!("A prepared voice line has the wrong format ({} Hz, {} channels). Delete the project's work folder and try again.", spec.sample_rate, spec.channels);
    }
    r.samples::<i16>().collect::<std::result::Result<Vec<_>, _>>().context("A prepared voice line could not be read.")
}

/// Writes the dialogue track, one second at a time.
fn write_mix(path: &Path, total: usize, clips: &[(i64, Vec<i16>)]) -> Result<()> {
    let spec = hound::WavSpec { channels: 1, sample_rate: AUDIO_RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let err = || "Could not write the dialogue track. Check that the data drive has free space.".to_string();
    let mut w = hound::WavWriter::create(path, spec).with_context(err)?;
    let block = AUDIO_RATE as usize;
    let mut at = 0usize;
    while at < total {
        let n = block.min(total - at);
        for s in mix_block(at as i64, n, clips, DIALOGUE_GAIN) {
            w.write_sample((s * 32767.).round() as i16).with_context(err)?;
        }
        at += n;
    }
    w.finalize().with_context(err)
}

/// A temp name beside `path` that no other mix (the preview's, the export's) uses at once.
fn unique_part(path: &Path) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!("{stem}.part-{}-{}.{ext}", std::process::id(), SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)))
}

/// A voice take normalised for the mix (`a/<take id>.wav`, shared by the preview and the export):
/// a cached copy is used only when it reads back whole; a new one is made under a name of its
/// own and then put in place, so two mixes at once never write into one file.
async fn normalised(exe: &Path, src: &Path, norm: &Path, cancel: &CancellationToken) -> Result<Vec<i16>> {
    if norm.is_file() {
        if let Ok(samples) = read_norm(norm) {
            return Ok(samples);
        }
    }
    let mut scratch = store::Scratch::default();
    let part = unique_part(norm);
    scratch.add(&part);
    ffmpeg::run(exe, &ffmpeg::normalise_args(src, &part), 0., |_| {}, cancel).await.context("Could not prepare a voice line for the mix.")?;
    let samples = read_norm(&part)?;
    if std::fs::rename(&part, norm).is_ok() {
        scratch.keep(&part);
    }
    // Not put in place (another mix holds it open): this mix still uses its own copy.
    Ok(samples)
}

async fn mix_into(store: &Store, plan: &CutPlan, exe: &Path, work: &Path, out_name: &str, progress: &(impl Fn(f32) + Send + Sync), cancel: &CancellationToken) -> Result<PathBuf> {
    let a_dir = work.join("a");
    store::make_dir(&a_dir)?;
    let placed: Vec<(f64, &str)> = plan.shots.iter().flat_map(|s| s.lines.iter().map(move |(f, o)| (s.start_s as f64 + *o as f64, f.as_str()))).collect();
    let mut clips: Vec<(i64, Vec<i16>)> = Vec::new();
    let mut cache: HashMap<PathBuf, usize> = HashMap::new();
    for (k, (start, file)) in placed.iter().enumerate() {
        let src = store.path(file);
        if !src.is_file() {
            continue;
        }
        let id = Path::new(file).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| format!("line{k}"));
        let norm = a_dir.join(format!("{}.wav", mv_core::paths::safe_name(&id)));
        let samples = match cache.get(&norm) {
            Some(i) => clips[*i].1.clone(),
            None => level_line(&normalised(exe, &src, &norm, cancel).await?),
        };
        cache.entry(norm).or_insert(clips.len());
        clips.push(((start * AUDIO_RATE as f64).round() as i64, samples));
        progress((k + 1) as f32 / placed.len().max(1) as f32);
    }
    // Exactly as long as the picture: whole frames of 1/24 s (2,000 samples each).
    let total = frames(plan.seconds) as usize * (AUDIO_RATE / FPS) as usize;
    let out = work.join(out_name);
    let mut scratch = store::Scratch::default();
    let part = unique_part(&out);
    scratch.add(&part);
    write_mix(&part, total, &clips)?;
    std::fs::rename(&part, &out).context("Could not keep the dialogue track. Close the preview if it is playing and try again.")?;
    scratch.keep(&part);
    Ok(out)
}

/// Mixes the plan's voices into `work/cut/preview-dialogue.wav` for the Cut page's preview (the
/// export mixes into a file of its own, so a preview never touches an export's track).
pub async fn mix_dialogue(store: &Store, plan: &CutPlan, progress: impl Fn(f32) + Send + Sync, cancel: &CancellationToken) -> Result<PathBuf> {
    let exe = ffmpeg::require()?;
    mix_into(store, plan, &exe, &store.work_dir().join("cut"), "preview-dialogue.wav", &progress, cancel).await
}

// ------------------------------------------------------------------------------ subtitles

fn srt_time(t: f32) -> String {
    let ms = (t.max(0.) as f64 * 1000.).round() as u64;
    format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

/// The plan's lines as SubRip text (times in the whole animatic).
pub fn srt(plan: &CutPlan) -> String {
    let mut items: Vec<(f32, f32, String)> = plan
        .shots
        .iter()
        .flat_map(|s| s.subs.iter().map(move |(a, b, t)| (s.start_s + a, s.start_s + b, t.split_whitespace().collect::<Vec<_>>().join(" "))))
        .map(|(a, b, t)| (a.max(0.), b.min(plan.seconds), t))
        .filter(|(a, b, t)| b - a >= 0.05 && !t.is_empty())
        .collect();
    items.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut out = String::new();
    for (i, (a, b, t)) in items.iter().enumerate() {
        out.push_str(&format!("{}\n{} --> {}\n{t}\n\n", i + 1, srt_time(*a), srt_time(*b)));
    }
    out
}

// ---------------------------------------------------------------------------------- render

fn font_file() -> Option<PathBuf> {
    let windir = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    Some(windir.join("Fonts").join("segoeui.ttf")).filter(|p| p.is_file())
}

fn blank_png(work: &Path) -> Result<PathBuf> {
    let p = work.join("blank.png");
    if !p.is_file() {
        image::RgbImage::from_pixel(64, 36, image::Rgb(PLACEHOLDER_RGB))
            .save(&p)
            .map_err(|e| anyhow!("Could not make the placeholder card ({e}). Check that the data drive has free space."))?;
    }
    Ok(p)
}

fn even(size: (u32, u32)) -> (u32, u32) {
    if size.0 >= 16 && size.1 >= 16 {
        (size.0 & !1, size.1 & !1)
    } else {
        DEFAULT_SIZE
    }
}

/// Renders the animatic to `out` (and `<out>.srt`); progress: segments 0–80%, concat 85%,
/// audio 90%, mux 100%.
pub async fn render(store: &Store, plan: &CutPlan, out: &Path, progress: impl Fn(f32, &str) + Send + Sync, cancel: &CancellationToken) -> Result<PathBuf> {
    if plan.shots.is_empty() {
        bail!(NO_SHOTS);
    }
    let exe = ffmpeg::require()?;
    let codec = ffmpeg::video_codec(&exe)?;
    let work = store.work_dir().join("cut");
    let seg_dir = work.join("seg");
    store::make_dir(&seg_dir)?;
    if let Some(dir) = out.parent() {
        store::make_dir(dir)?;
    }
    // Every `.part` file goes on any way out (error, cancel, the app closing the job).
    let mut scratch = store::Scratch::default();
    let size = even(plan.size);
    let font = font_file();
    let count = plan.shots.len();
    let total: u64 = plan.shots.iter().map(|s| frames(s.seconds) as u64).sum::<u64>().max(1);
    let mut done: u64 = 0;
    let mut list = String::new();
    let mut keys: std::collections::HashSet<String> = std::collections::HashSet::new();
    progress(0., "Moving the camera over each still");
    for (i, s) in plan.shots.iter().enumerate() {
        let n = frames(s.seconds);
        let label = format!("Cut {} of {count}", i + 1);
        let exprs = zoompan_exprs(s);
        let still = s.still.as_deref().map(|f| store.path(f)).filter(|p| p.is_file());
        let (input, text, ident) = match still {
            Some(p) => (p, None, s.still.clone().unwrap_or_default()),
            None => {
                let card = format!("Cut {} of {count}: no picture yet", i + 1);
                (blank_png(&work)?, font.clone().map(|f| (f, card.clone())), format!("placeholder: {card}"))
            }
        };
        let key = segment_key(&ident, n, size, codec, &exprs, text.is_some());
        keys.insert(key.clone());
        let rel = format!("seg/{key}.mp4");
        if !work.join(&rel).is_file() {
            let text = match text {
                Some((font, card)) => {
                    let tf = format!("seg/{key}.txt");
                    std::fs::write(work.join(&tf), card).context("Could not write the placeholder card's text.")?;
                    Some((font, tf))
                }
                None => None,
            };
            let part = seg_dir.join(format!("{key}.part.mp4"));
            scratch.add(&part);
            let args = ffmpeg::segment_args(&SegmentSpec { input, frames: n, size, exprs, text, codec, out: part.clone() });
            let base = done;
            let report = |f: f32| progress(0.8 * (base as f32 + f * n as f32) / total as f32, &label);
            ffmpeg::run_in(&exe, &args, Some(&work), s.seconds, report, cancel).await.with_context(|| format!("Could not make the video of cut {} of {count}.", i + 1))?;
            std::fs::rename(&part, work.join(&rel)).context("Could not keep a finished cut. Close other programs using the project folder and try again.")?;
            scratch.keep(&part);
        }
        done += n as u64;
        progress(0.8 * done as f32 / total as f32, &label);
        list.push_str(&format!("file '{rel}'\n"));
    }
    let list_file = work.join("list.txt");
    std::fs::write(&list_file, &list).context("Could not write the list of cuts.")?;
    let video = work.join("video.mp4");
    ffmpeg::run(&exe, &ffmpeg::concat_args(&list_file, &video), 0., |_| {}, cancel).await.context("Could not join the cuts into one video.")?;
    progress(0.85, "Mixing the voices");
    let dialogue = mix_into(store, plan, &exe, &work, "dialogue.wav", &|f| progress(0.85 + 0.05 * f, "Mixing the voices"), cancel).await?;
    progress(0.9, "Putting picture and sound together");
    // The subtitles wait in the work folder; the `.srt` beside the export appears only with it.
    let text = srt(plan);
    std::fs::write(work.join("subs.srt"), &text).context("Could not write the subtitles file.")?;
    let subs = if text.trim().is_empty() || !(plan.subtitles || plan.burn_subtitles) {
        Subs::None
    } else if plan.burn_subtitles && ffmpeg::has(&exe, "subtitles") {
        Subs::Burn("subs.srt".into())
    } else {
        Subs::Soft(work.join("subs.srt"))
    };
    let lang = store.series().map(|s| s.settings.language).unwrap_or_else(|_| "en".into());
    let stem = out.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "animatic".into());
    let part = out.with_file_name(format!("{stem}.part.mp4"));
    scratch.add(&part);
    let args = ffmpeg::mux_args(&video, &dialogue, &subs, codec, &lang, &part);
    ffmpeg::run_in(&exe, &args, Some(&work), plan.seconds, |f| progress(0.9 + 0.1 * f, "Putting picture and sound together"), cancel)
        .await
        .context("Could not put the picture and the voices together.")?;
    std::fs::rename(&part, out).context("Could not save the animatic. Close any player showing an earlier one and try again.")?;
    scratch.keep(&part);
    let srt_part = out.with_file_name(format!("{stem}.part.srt"));
    scratch.add(&srt_part);
    let kept = std::fs::write(&srt_part, &text).and_then(|_| std::fs::rename(&srt_part, out.with_extension("srt")));
    if let Err(e) = kept {
        // The video is made; only its side file is missing.
        mv_core::logging::global().warn("anime", &format!("the animatic's subtitles file was not kept: {e}"));
    }
    prune_segments(&seg_dir, &keys);
    progress(1., "Animatic ready");
    Ok(out.to_path_buf())
}

/// Removes cached segments (and their card texts and leftovers) that the current plan no
/// longer uses, so `work/cut/seg` does not grow by one file per changed cut forever.
fn prune_segments(seg_dir: &Path, keep: &std::collections::HashSet<String>) {
    let Ok(rd) = std::fs::read_dir(seg_dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let key = name.split('.').next().unwrap_or("");
        if !keep.contains(key) && e.path().is_file() {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// `yyyymmdd-hhmmss` (UTC) of a unix time.
pub fn stamp(unix_s: i64) -> String {
    let days = unix_s.div_euclid(86_400);
    let secs = unix_s.rem_euclid(86_400);
    // Days to a civil date (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", secs / 3600, secs / 60 % 60, secs % 60)
}

/// The Animatic job: plan, render to `exports/animatic-<stamp>.mp4`, store as a take.
pub async fn run_job(cx: &JobCtx<'_>, episode: &str) -> Result<()> {
    let store = cx.store;
    (cx.progress)(0., "Planning the cut");
    let plan = plan_cut(store, episode)?;
    if plan.shots.is_empty() {
        bail!(NO_SHOTS);
    }
    let ep_dir = format!("episodes/{}", mv_core::paths::safe_name(episode));
    // Named in the user's own time, so the file reads like the clock when it was made.
    let now = store::now_s();
    let when = Some(store::local_stamp(now)).filter(|s| !s.is_empty()).unwrap_or_else(|| stamp(now));
    let out = store.path(&format!("{ep_dir}/exports/animatic-{when}.mp4"));
    let made = render(store, &plan, &out, |p, s| (cx.progress)(p * 0.96, s), cx.cancel).await?;
    (cx.progress)(0.97, "Keeping the animatic in the history");
    // Hashed while streaming and linked, not copied: one copy of the video on the drive.
    store.add_take_file(
        &made,
        "mp4",
        TakeMeta {
            kind: TakeKind::Video,
            target: Target::Animatic { episode: episode.to_string() },
            request_hash: plan.timing_hash.clone(),
            source: TakeSource::Ffmpeg,
            w: plan.size.0,
            h: plan.size.1,
            seconds: plan.seconds,
            note: made.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            ..TakeMeta::default()
        },
    )?;
    // Read cut.json fresh: the user may have switched subtitles or trimmed a cut during the
    // render. Only what the render worked out is written; their choices stay.
    store.update_json::<CutPlan>(&format!("{ep_dir}/cut.json"), |saved| record_render(saved, &plan))?;
    (cx.progress)(1., "The animatic is ready");
    Ok(())
}

/// Writes what a finished render worked out (the shots, the length, the timing hash, the export
/// settings it was made with) into the cut file as it is now, keeping the user's own fields
/// there: subtitle switches, size, other fields and each shot's trims.
pub fn record_render(saved: &mut CutPlan, plan: &CutPlan) {
    let trims: HashMap<&str, (f32, f32)> = saved.shots.iter().map(|s| (s.shot.as_str(), (s.trim_in, s.trim_out))).collect();
    let shots: Vec<CutShot> = plan
        .shots
        .iter()
        .map(|s| {
            let mut s = s.clone();
            if let Some((ti, to)) = trims.get(s.shot.as_str()) {
                s.trim_in = *ti;
                s.trim_out = *to;
            }
            s
        })
        .collect();
    saved.shots = shots;
    saved.seconds = plan.seconds;
    saved.timing_hash = plan.timing_hash.clone();
    // The settings the render was made with (read when it started), not the switches as they are
    // now: a switch flipped during the render leaves this export out of date.
    saved.exported_with = Some(export_settings(plan));
    if saved.size.0 < 16 || saved.size.1 < 16 {
        saved.size = plan.size;
    }
}
