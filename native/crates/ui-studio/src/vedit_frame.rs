//! One frame of a chapter's picture, worked out the way the renderer (`python/video_generator.py`)
//! draws it, so the Video editor's Program monitor shows what the render will: which shot is on
//! screen and how far into it, the panel's camera (zoom and window: `clip_camera` / `edit_move`),
//! where the panel sits (`panel_margins`), which transition joins two shots and how far it is
//! (`plan_transitions`, `clip_frames`), the action punch (`_flash`, `_shake`, the IMPACT_GAP
//! rule) and the side-by-side duo panels. `tests/fixtures/frame_transforms.json` pins the camera
//! of both sides to the same numbers.
//!
//! Pure functions only; `vedit_view` draws from them.
use std::f64::consts::PI;
use std::path::Path;

use crate::look::{Effects, Preset};
use crate::vedit::{Beat, ChapterEdit, Clip, Motion, Placed, TransKind};

pub const FPS: f64 = 30.;
const FG_PUSH: f64 = 0.05;
const FG_PAN_ZOOM: f64 = 1.04;
const FG_MARGIN: f64 = 0.05;
pub const IMPACT_SEC: f64 = 0.4;
const IMPACT_GAP: i64 = 4;
const PUNCH_SEC: f64 = 0.25;
const LETTERBOX_ASPECT: f64 = 2.39;
const SAME_PAGE: (&str, f64) = ("fade", 0.35);
const PAGE_TRANSITIONS: [&str; 4] = ["smoothleft", "fade", "smoothup", "fadeblack"];
/// The look's own page-change transition length (`transition_duration` of every render).
const BASE_TRANSITION: f64 = 0.5;

/// Python's `round()` (halves to even).
fn round_py(x: f64) -> f64 {
    x.round_ties_even()
}

/// Python's `round(x, d)` / an `f"{x:.dd}"` in an ffmpeg expression.
fn round_to(x: f64, d: i32) -> f64 {
    let m = 10f64.powi(d);
    (x * m).round_ties_even() / m
}

/// A value the editor saves as f32 and `vedit::render_extra` hands the renderer at 3 decimals.
fn sent(v: f32) -> f64 {
    (v as f64 * 1000.).round() / 1000.
}

// ------------------------------------------------------------------------------------ the look

/// What a look does to the picture (the renderer's `LOOKS` and `apply_intensity`).
#[derive(Clone, Debug, PartialEq)]
pub struct LookParams {
    pub preset: Preset,
    pub fx: Effects,
    /// The edit's intensities (key -> multiplier 0..2).
    pub intensity: std::collections::BTreeMap<String, f32>,
}

impl LookParams {
    /// The chapter's look: its own pick over the Studio-wide `studio` one.
    pub fn of(e: &ChapterEdit, studio: (Preset, Effects)) -> LookParams {
        let (preset, fx) = match &e.look {
            Some(l) => {
                let p = Preset::parse(&l.preset).unwrap_or_default();
                let mut fx = p.effects();
                for eff in crate::look::Effect::ALL {
                    if let Some(on) = l.effects.get(eff.key()) {
                        set(&mut fx, eff, *on);
                    }
                }
                (p, fx)
            }
            None => studio,
        };
        LookParams { preset, fx, intensity: e.intensity.clone() }
    }

    /// An intensity multiplier (1 = the look's own).
    pub fn k(&self, key: &str) -> f64 {
        self.intensity.get(key).map(|v| sent(v.clamp(0., 2.))).unwrap_or(1.)
    }

    /// The look's push, scaled by the motion intensity.
    pub fn push(&self) -> f64 {
        let base = match self.preset {
            Preset::Cinematic => 0.06,
            Preset::Dynamic => 0.08,
            _ => FG_PUSH,
        };
        round_to(base * self.k("motion"), 4)
    }

    /// Moves timed to the beat (a snap on a short beat).
    pub fn ramp(&self) -> bool {
        self.preset == Preset::Dynamic
    }

    pub fn duo(&self) -> bool {
        matches!(self.preset, Preset::Recap | Preset::Dynamic)
    }

    /// The page-change transitions the look rotates through.
    pub fn transitions(&self) -> &'static [&'static str] {
        match self.preset {
            Preset::Clean => &PAGE_TRANSITIONS,
            Preset::Cinematic => &["fade", "fadeblack", "smoothleft", "fade"],
            Preset::Recap => &["smoothleft", "smoothup", "fade", "smoothright"],
            Preset::Dynamic => &["smoothleft", "burn", "smoothup", "zoomblur", "circleopen", "smoothright", "burn", "slidedown", "zoomblur"],
        }
    }
}

fn set(fx: &mut Effects, e: crate::look::Effect, on: bool) {
    use crate::look::Effect::*;
    *match e {
        Grade => &mut fx.grade,
        Glow => &mut fx.glow,
        Vignette => &mut fx.vignette,
        Grain => &mut fx.grain,
        Impact => &mut fx.impact,
        Leaks => &mut fx.leaks,
        Chroma => &mut fx.chroma,
        Parallax => &mut fx.parallax,
        Particles => &mut fx.particles,
        Letterbox => &mut fx.letterbox,
    } = on;
}

// ------------------------------------------------------------------------------------ the camera

/// A move the look picks by itself (`FG_MOTIONS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Auto {
    Still,
    Push,
    Pull,
    PanLeft,
    PanRight,
    PanUp,
    PanDown,
}

impl Auto {
    #[cfg(test)]
    pub fn parse(s: &str) -> Option<Auto> {
        Some(match s {
            "still" => Auto::Still,
            "push" => Auto::Push,
            "pull" => Auto::Pull,
            "pan_left" => Auto::PanLeft,
            "pan_right" => Auto::PanRight,
            "pan_up" => Auto::PanUp,
            "pan_down" => Auto::PanDown,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cam {
    Auto(Auto),
    /// The editor's move (`edit_move`): from / to in 0..1 of the panel, amount the zoom share.
    Edit { kind: Motion, amount: Option<f64>, from: Option<(f64, f64)>, to: Option<(f64, f64)> },
}

/// Everything the camera of one single-panel shot depends on (`clip_camera`'s `clip`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipCam {
    pub cam: Cam,
    pub impact: bool,
    pub ramp: bool,
    /// The look's push.
    pub push: f64,
    pub motion_k: f64,
    /// The action punch's zoom.
    pub punch: f64,
}

/// The panel's visible rectangle: 1/`zoom` of it, at `x` / `y` of the slack the zoom leaves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub zoom: f64,
    pub x: f64,
    pub y: f64,
}

/// The eased 0..1 progress at frame `on` of a move over `frames` (`ramp_ease`).
pub fn ease(on: f64, frames: u32, ramp: bool) -> f64 {
    let last = frames.saturating_sub(1).max(1) as f64;
    let cosine = 0.5 - 0.5 * (PI * on / last).cos();
    if !ramp {
        return cosine;
    }
    if (frames as f64) < 2. * FPS {
        let n = round_py(frames as f64 * 0.5).max(2.);
        return 1. - (1. - (on / n).min(1.)).powi(3);
    }
    0.65 * (on / last).min(1.) + 0.35 * cosine
}

/// The camera at frame `on` of a shot `frames` long (`clip_camera` evaluated).
pub fn camera(c: &ClipCam, frames: u32, on: f64) -> View {
    let mut k = (frames as f64 / (3. * FPS)).min(1.);
    if c.ramp && (frames as f64) < 2. * FPS {
        k = k.max(0.7);
    }
    let push = round_to(c.push * k, 4);
    let e = ease(on, frames, c.ramp);
    let window = |a: Auto| match a {
        Auto::PanDown => (0.5, e),
        Auto::PanUp => (0.5, 1. - e),
        Auto::PanRight => (e, 0.5),
        Auto::PanLeft => (1. - e, 0.5),
        _ => (0.5, 0.5),
    };
    let (mut zoom, (mut x, mut y)) = match c.cam {
        Cam::Auto(a) => {
            let pan_zoom = round_to(1. + (FG_PAN_ZOOM - 1.) * k * c.motion_k, 4);
            let z = match a {
                Auto::Push => 1. + push * e,
                Auto::Pull => round_to(1. + push, 4) - push * e,
                Auto::Still => 1.,
                _ => pan_zoom,
            };
            (z, window(a))
        }
        Cam::Edit { kind, amount, from, to } => {
            let aim = |f: Option<(f64, f64)>| f.map(|(x, y)| (round_to(x, 4), round_to(y, 4))).unwrap_or((0.5, 0.5));
            match kind {
                Motion::Zoom => (1. + round_to(amount.unwrap_or(push), 4) * e, aim(to.or(from))),
                Motion::ZoomOut => {
                    let a = amount.unwrap_or(push);
                    (round_to(1. + a, 4) - round_to(a, 4) * e, aim(from.or(to)))
                }
                Motion::PanLeft | Motion::PanRight | Motion::PanUp | Motion::PanDown => {
                    let a = amount.unwrap_or(FG_PAN_ZOOM - 1.).max(0.02);
                    let w = match kind {
                        Motion::PanLeft => Auto::PanLeft,
                        Motion::PanRight => Auto::PanRight,
                        Motion::PanUp => Auto::PanUp,
                        _ => Auto::PanDown,
                    };
                    (round_to(1. + a, 4), window(w))
                }
                Motion::KenBurns => {
                    let a = amount.unwrap_or(0.08);
                    let (p0, p1) = (from.unwrap_or((0.35, 0.4)), to.unwrap_or((0.65, 0.6)));
                    let x = round_to(p0.0, 4) + round_to(p1.0 - p0.0, 4) * e;
                    let y = round_to(p0.1, 4) + round_to(p1.1 - p0.1, 4) * e;
                    (round_to(1. + a / 2., 4) + round_to(a / 2., 4) * e, (x, y))
                }
                Motion::Punch => {
                    let a = amount.unwrap_or(0.15);
                    let n = round_py(PUNCH_SEC * FPS).max(2.);
                    (1. + round_to(a, 4) * (1. - (1. - (on / n).min(1.)).powi(3)) + 0.02 * e, aim(to.or(from)))
                }
                _ => (1., (0.5, 0.5)),
            }
        }
    };
    if c.impact && c.cam != Cam::Auto(Auto::Still) {
        let n = round_py(IMPACT_SEC * FPS * 0.6).max(2.);
        zoom = 1. + c.punch * (1. - (on / n).min(1.)).powi(3) + push * e;
        (x, y) = (0.5, 0.5);
    }
    View { zoom, x, y }
}

/// The white flash over an action beat `t` seconds in (`_flash`: brightness added).
pub fn flash(t: f64) -> f64 {
    0.35 * (1. - t / IMPACT_SEC).max(0.).powi(2)
}

/// The camera jolt of an action beat `t` seconds in, for a frame `h` pixels high (`_shake`).
pub fn jolt(t: f64, punch: f64, h: f64) -> (f64, f64) {
    let a = round_py(h * 0.1 * punch);
    let decay = (-t * round_to(3.5 / IMPACT_SEC, 2)).exp();
    (a * (2. * PI * t * 13.).sin() * decay, a * (2. * PI * t * 11.).cos() * decay)
}

// ------------------------------------------------------------------------------------ layout

/// The frame a render makes (1080p; the preview scales it).
pub fn render_size(reels: bool) -> (f64, f64) {
    if reels {
        (1080., 1920.)
    } else {
        (1920., 1080.)
    }
}

/// Letterbox bar height (`letterbox_bar`).
pub fn letterbox_bar(w: f64, h: f64) -> f64 {
    if w <= h {
        return 0.;
    }
    (round_py((h - w / LETTERBOX_ASPECT) / 4.) * 2.).max(0.)
}

/// The space around a single panel on the blurred fill (`panel_margins`).
fn margins(w: f64, h: f64, inset: f64) -> (f64, f64) {
    let (mx, mut my) = (round_py(w * FG_MARGIN), round_py(h * FG_MARGIN));
    if inset > 0. {
        my = my.max(inset + round_py(h * 0.025));
    }
    (mx, my)
}

/// Where a panel of `aspect` (w / h) sits in a `w` x `h` frame: (x, y, width, height)
/// (`_prepare_clip_images`, the blurred fill).
pub fn panel_box(aspect: f64, w: f64, h: f64, letterbox: bool) -> (f64, f64, f64, f64) {
    let inset = if letterbox { letterbox_bar(w, h) } else { 0. };
    let (mx, my) = margins(w, h, inset);
    let (bw, bh) = (w - 2. * mx, h - 2. * my);
    let a = aspect.max(0.01);
    let scale = (bw / a).min(bh);
    let pw = ((a * scale).floor() / 2.).floor().max(1.) * 2.;
    let ph = (scale.floor() / 2.).floor().max(1.) * 2.;
    ((((w - pw) / 4.).floor()) * 2., (((h - ph) / 4.).floor()) * 2., pw, ph)
}

/// The panel box's aspect (`box_aspect`).
fn box_aspect(w: f64, h: f64, letterbox: bool) -> f64 {
    let inset = if letterbox { letterbox_bar(w, h) } else { 0. };
    let (mx, my) = margins(w, h, inset);
    (w - 2. * mx) / (h - 2. * my)
}

/// The look's move for a panel of `aspect` (`pick_motion`).
fn pick_motion(aspect: f64, box_aspect: f64, turn: usize, prev: Option<Auto>) -> Auto {
    use Auto::*;
    let options: &[Auto] = if aspect < box_aspect * 0.75 {
        &[Push, PanDown, Pull, PanUp]
    } else if aspect > box_aspect * 1.33 {
        &[Push, PanRight, Pull, PanLeft]
    } else {
        &[Push, Pull, PanRight, Push, Pull, PanDown]
    };
    let m = options[turn % options.len()];
    if Some(m) == prev {
        options[(turn + 1) % options.len()]
    } else {
        m
    }
}

// ------------------------------------------------------------------------------------ the plan

/// The renderer's name and default length of an editor transition (`EDIT_TRANSITIONS`).
pub fn edit_transition(k: TransKind) -> &'static str {
    match k {
        TransKind::Fade => "fade",
        TransKind::Dissolve => "dissolve",
        TransKind::Black => "fadeblack",
        TransKind::Flash => "fadewhite",
        TransKind::WhipLeft => "whipleft",
        TransKind::WhipRight => "whipright",
        TransKind::WhipUp => "whipup",
        TransKind::WhipDown => "whipdown",
        TransKind::SlideLeft => "slideleft",
        TransKind::SlideRight => "slideright",
        TransKind::SlideUp => "slideup",
        TransKind::SlideDown => "slidedown",
        TransKind::Wipe => "wipeleft",
        TransKind::Circle => "circleopen",
        TransKind::Zoom => "zoomblur",
        TransKind::Burn => "burn",
        TransKind::Glitch => "glitch",
    }
}

/// One shot on the rendered timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct Shot {
    /// Index in the placed clips.
    pub index: usize,
    pub start: f64,
    /// Frames the shot renders (its length plus the next transition's tail: `clip_frames`).
    pub frames: u32,
    pub cam: ClipCam,
    /// Two tall panels side by side (drawn by the duo layout, not the camera).
    pub duo: bool,
    pub impact: bool,
    /// The transition into this shot (as planned: name, seconds); `None` for the first.
    pub trans: Option<(String, f64)>,
}

/// The editor clip's camera as the renderer gets it (`apply_edit_clips` + the motion pick).
pub fn edit_cam(c: &Clip) -> Option<Cam> {
    let to01 = |f: Option<(f32, f32)>| f.map(|(x, y)| (sent(x.clamp(0., 100.)) / 100., sent(y.clamp(0., 100.)) / 100.));
    match c.motion {
        Motion::Auto | Motion::Shake => None,
        Motion::Still => Some(Cam::Auto(Auto::Still)),
        kind => Some(Cam::Edit { kind, amount: c.amount.map(|a| sent(a.clamp(0., 0.5))), from: to01(c.focus_from), to: to01(c.focus_to) }),
    }
}

/// The chapter's shots as the renderer plans them (`generate_video` step 1-2). `aspect` reads
/// an image's width / height; `cuts`: the edit's hard cuts between every clip.
pub fn plan(beats: &[Beat], placed: &[Placed], look: &LookParams, cuts: bool, reels: bool, aspect: &dyn Fn(&Path) -> Option<f32>) -> Vec<Shot> {
    let (w, h) = render_size(reels);
    let box_a = box_aspect(w, h, look.fx.letterbox);
    let duo_on = look.duo() && !reels;
    let (mut turn, mut prev) = (0usize, None);
    let mut last_impact = -IMPACT_GAP;
    let mut shots: Vec<Shot> = vec![];
    for (i, p) in placed.iter().enumerate() {
        let b = beats.get(p.clip.src);
        let a = b.and_then(|b| b.image.as_deref()).and_then(aspect).map(f64::from).unwrap_or(1.);
        let own = p.clip.motion.own_move();
        let duo = duo_on && !own && crate::vedit_manga::duo_aspect(a as f32);
        let cam = if duo {
            Cam::Auto(Auto::Still)
        } else {
            match edit_cam(&p.clip) {
                Some(c) => c,
                None => {
                    let mut m = pick_motion(a, box_a, turn, prev);
                    turn += 1;
                    prev = Some(m);
                    if p.clip.motion == Motion::Shake && m == Auto::Still {
                        m = Auto::Push;
                    }
                    Cam::Auto(m)
                }
            }
        };
        let text = p.clip.text.as_deref().or(b.map(|b| b.text.as_str())).unwrap_or("");
        let impact = if p.clip.motion != Motion::Auto {
            p.clip.motion == Motion::Shake
        } else {
            look.fx.impact && i as i64 - last_impact >= IMPACT_GAP && crate::vedit_manga::is_impact(text)
        };
        if impact {
            last_impact = i as i64;
        }
        let punch = match (p.clip.motion, p.clip.amount) {
            (Motion::Shake, Some(a)) => sent(a.clamp(0., 0.5)),
            _ => 0.12,
        };
        shots.push(Shot {
            index: p.index,
            start: p.start_ms as f64 / 1000.,
            frames: 0,
            cam: ClipCam { cam, impact, ramp: look.ramp(), push: look.push(), motion_k: look.k("motion"), punch },
            duo,
            impact,
            trans: None,
        });
    }
    // Joins (`plan_transitions`): the editor's pick, else a hard cut, a quick dissolve into an
    // action beat, a short fade within a page and the look's rotation on a page turn.
    let names = look.transitions();
    let frame = 1. / FPS;
    let mut rot = 0;
    for k in 1..shots.len() {
        let (pc, cc) = (&placed[k - 1].clip, &placed[k].clip);
        let page = |c: &Clip| beats.get(c.src).map(|b| b.page);
        let (name, mut dur) = if let (false, Some(t)) = (cc.cut, cc.trans) {
            (edit_transition(t.kind).to_string(), round_to(t.ms.unwrap_or(t.kind.default_ms()) as f64 / 1000., 3))
        } else if cuts || cc.cut || (shots[k].duo && shots[k - 1].duo) {
            ("fade".to_string(), frame)
        } else if shots[k].impact {
            ("fade".to_string(), 0.12)
        } else if page(pc) == page(cc) {
            (SAME_PAGE.0.to_string(), SAME_PAGE.1.min(BASE_TRANSITION))
        } else {
            rot += 1;
            (names[(rot - 1) % names.len()].to_string(), BASE_TRANSITION)
        };
        let (dp, dc) = (placed[k - 1].dur_ms as f64 / 1000., placed[k].dur_ms as f64 / 1000.);
        dur = frame.max(dur.min(0.45 * dp).min(0.45 * dc));
        shots[k].trans = Some((name, round_to(dur, 3)));
    }
    // Frames each shot renders (`clip_frames`).
    let n = shots.len();
    for k in 0..n {
        let first = round_py(shots[k].start * FPS);
        shots[k].frames = if k + 1 < n {
            let tail = shots[k + 1].trans.as_ref().map(|t| t.1).unwrap_or(0.);
            (round_py(shots[k + 1].start * FPS) - first + (tail * FPS).ceil() + 1.) as u32
        } else {
            ((placed[k].dur_ms as f64 / 1000. * FPS).ceil() + 1.) as u32
        };
    }
    shots
}

/// What is on screen at `t` seconds: the shot and its frame, and while a transition runs, the
/// shot it leaves (and its frame) and how far the transition is (0..1).
#[derive(Clone, Debug, PartialEq)]
pub struct At {
    pub shot: usize,
    pub on: f64,
    pub from: Option<(usize, f64)>,
    pub trans: Option<(String, f64)>,
}

pub fn at(shots: &[Shot], t: f64) -> Option<At> {
    let k = shots.partition_point(|s| s.start <= t).checked_sub(1)?;
    let s = &shots[k];
    let on = ((t - s.start) * FPS).floor().max(0.);
    let mut out = At { shot: k, on, from: None, trans: None };
    if let (Some((name, dur)), Some(prev)) = (&s.trans, k.checked_sub(1)) {
        if t - s.start < *dur && *dur > 1.5 / FPS {
            out.from = Some((prev, ((t - shots[prev].start) * FPS).floor()));
            out.trans = Some((name.clone(), ((t - s.start) / dur).clamp(0., 1.)));
        }
    }
    Some(out)
}

/// How a transition the preview draws mixes two pictures: the incoming one's offset (fractions
/// of the frame, the outgoing one moves the other way for a slide), its opacity, and a colour
/// dip (white or black, its opacity) between them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mix {
    /// The outgoing picture's offset (fractions of the frame).
    pub out_dx: f64,
    pub out_dy: f64,
    /// The incoming one's offset and opacity.
    pub in_dx: f64,
    pub in_dy: f64,
    pub in_alpha: f64,
    /// The incoming one is revealed from this side only (a wipe): share of the frame shown.
    pub reveal: Option<(Side, f64)>,
    /// (white, opacity) over both.
    pub dip: Option<(bool, f64)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

pub fn mix(name: &str, p: f64) -> Mix {
    let base = Mix { out_dx: 0., out_dy: 0., in_dx: 0., in_dy: 0., in_alpha: 1., reveal: None, dip: None };
    match name {
        "fadeblack" | "fadewhite" => Mix { in_alpha: if p < 0.5 { 0. } else { 1. }, dip: Some((name == "fadewhite", 1. - (2. * p - 1.).abs())), ..base },
        "slideleft" | "whipleft" => Mix { out_dx: -p, in_dx: 1. - p, ..base },
        "slideright" | "whipright" => Mix { out_dx: p, in_dx: p - 1., ..base },
        "slideup" | "whipup" => Mix { out_dy: -p, in_dy: 1. - p, ..base },
        "slidedown" | "whipdown" => Mix { out_dy: p, in_dy: p - 1., ..base },
        "smoothleft" | "wipeleft" => Mix { reveal: Some((Side::Right, p)), ..base },
        "smoothright" | "wiperight" => Mix { reveal: Some((Side::Left, p)), ..base },
        "smoothup" | "wipeup" => Mix { reveal: Some((Side::Bottom, p)), ..base },
        "smoothdown" | "wipedown" => Mix { reveal: Some((Side::Top, p)), ..base },
        // Dissolves, and the ones the clips draw over a dissolve (burn, zoom blur, glitch).
        _ => Mix { in_alpha: p, ..base },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::path::PathBuf;

    const FIXTURE: &str = include_str!("../../../../tests/fixtures/frame_transforms.json");

    fn cam_of(c: &Value) -> ClipCam {
        let f = |k: &str, d: f64| c.get(k).and_then(Value::as_f64).unwrap_or(d);
        let pt = |v: &Value| v.as_array().map(|a| (a[0].as_f64().unwrap(), a[1].as_f64().unwrap()));
        let motion = c["motion"].as_str().unwrap();
        let cam = if motion == "edit" {
            let m = &c["move"];
            Cam::Edit { kind: Motion::parse(m["kind"].as_str().unwrap()).unwrap(), amount: m["amount"].as_f64(), from: pt(&m["from"]), to: pt(&m["to"]) }
        } else {
            Cam::Auto(Auto::parse(motion).unwrap())
        };
        ClipCam { cam, impact: c.get("impact").and_then(Value::as_bool).unwrap_or(false), ramp: c.get("ramp").and_then(Value::as_bool).unwrap_or(false), push: f("push", FG_PUSH), motion_k: f("motion_k", 1.), punch: f("punch", 0.12) }
    }

    /// The preview's camera gives the renderer's numbers (the fixture `tests/test_frame_transforms.py`
    /// checks the renderer's ffmpeg expressions against) at the start, middle and end of each move.
    #[test]
    fn the_preview_camera_matches_the_renderer() {
        let v: Value = serde_json::from_str(FIXTURE).unwrap();
        let cases = v["cases"].as_array().unwrap();
        assert!(cases.len() >= 10);
        for case in cases {
            let cam = cam_of(&case["clip"]);
            let frames = case["frames"].as_u64().unwrap() as u32;
            for row in case["expect"].as_array().unwrap() {
                let r: Vec<f64> = row.as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
                let got = camera(&cam, frames, r[0]);
                let name = case["name"].as_str().unwrap();
                assert!((got.zoom - r[1]).abs() < 1e-4, "{name} frame {}: zoom {} != {}", r[0], got.zoom, r[1]);
                assert!((got.x - r[2]).abs() < 1e-4, "{name} frame {}: x {} != {}", r[0], got.x, r[2]);
                assert!((got.y - r[3]).abs() < 1e-4, "{name} frame {}: y {} != {}", r[0], got.y, r[3]);
            }
        }
    }

    fn beats() -> Vec<Beat> {
        let b = |page: usize, text: &str, dur_ms: u64| Beat { page, image: Some(PathBuf::from(format!("p{page}.jpg"))), text: text.into(), audio: PathBuf::from("a.wav"), offset_ms: 0, dur_ms };
        vec![b(0, "Calm.", 3000), b(0, "Still calm.", 2000), b(1, "He strikes!", 2400), b(2, "After.", 3000)]
    }

    fn clean() -> LookParams {
        LookParams { preset: Preset::Clean, fx: Preset::Clean.effects(), intensity: Default::default() }
    }

    #[test]
    fn the_editor_clip_reaches_the_camera_as_the_renderer_gets_it() {
        let mut c = Clip::of(0);
        c.motion = Motion::KenBurns;
        c.amount = Some(0.11);
        c.focus_from = Some((30., 35.));
        c.focus_to = Some((70., 62.));
        assert_eq!(edit_cam(&c), Some(Cam::Edit { kind: Motion::KenBurns, amount: Some(0.11), from: Some((0.3, 0.35)), to: Some((0.7, 0.62)) }));
        c.motion = Motion::Shake;
        assert_eq!(edit_cam(&c), None, "a shake is the look's move with the punch on top");
    }

    #[test]
    fn shots_plan_like_the_renderer() {
        let b = beats();
        let mut clips: Vec<Clip> = (0..b.len()).map(Clip::of).collect();
        clips[3].trans = Some(crate::vedit::Transition { kind: TransKind::Flash, ms: Some(250) });
        clips[1].motion = Motion::PanUp;
        let placed = crate::vedit::place(&b, &clips);
        let look = LookParams { fx: Effects { impact: true, ..Effects::default() }, ..clean() };
        let shots = plan(&b, &placed, &look, false, false, &|_| Some(0.7));
        // Same page: a short fade; the action beat: a quick dissolve; the editor's flash wins.
        assert_eq!(shots[1].trans, Some(("fade".into(), 0.35)));
        assert_eq!(shots[2].trans, Some(("fade".into(), 0.12)));
        assert_eq!(shots[3].trans, Some(("fadewhite".into(), 0.25)));
        assert!(shots[2].impact && !shots[0].impact);
        // Tall panels in a clean look move up and down; the editor's pan is its own.
        assert_eq!(shots[0].cam.cam, Cam::Auto(Auto::Push));
        assert!(matches!(shots[1].cam.cam, Cam::Edit { kind: Motion::PanUp, .. }));
        assert_eq!(shots[2].cam.cam, Cam::Auto(Auto::PanDown));
        // Each shot renders up to the next start plus that transition's tail and a spare frame.
        assert_eq!(shots[0].frames, 90 + 11 + 1);
        assert_eq!(shots[3].frames, 91);
        // Mid-transition into shot 3 both pictures are there.
        let a = at(&shots, 7.4 + 0.1).unwrap();
        assert_eq!((a.shot, a.from.map(|f| f.0)), (3, Some(2)));
        assert!((a.trans.unwrap().1 - 0.4).abs() < 1e-6);
        assert!(at(&shots, 7.4 + 0.3).unwrap().from.is_none());
        // A duo look pairs the tall panels (no camera), but not the editor's own move.
        let duo = LookParams { preset: Preset::Recap, fx: Preset::Recap.effects(), intensity: Default::default() };
        let shots = plan(&b, &placed, &duo, false, false, &|_| Some(0.7));
        assert_eq!(shots.iter().map(|s| s.duo).collect::<Vec<_>>(), vec![true, false, true, true]);
        assert_eq!(shots[3].trans, Some(("fadewhite".into(), 0.25)), "the editor's transition wins over the duo cut");
    }

    #[test]
    fn panels_sit_where_the_renderer_puts_them() {
        // A square panel in 1080p: 5% margins, centred, even sizes.
        assert_eq!(panel_box(1., 1920., 1080., false), (474., 54., 972., 972.));
        // The cinematic letterbox keeps it between the bars.
        assert_eq!(letterbox_bar(1920., 1080.), 138.);
        let (_, y, _, h) = panel_box(1., 1920., 1080., true);
        assert!(y >= 138. && y + h <= 1080. - 138.);
        assert!((flash(0.) - 0.35).abs() < 1e-9 && flash(0.4) == 0.);
        assert_eq!(mix("slideleft", 0.25).in_dx, 0.75);
        assert_eq!(mix("fadeblack", 0.5).dip, Some((false, 1.)));
    }
}
