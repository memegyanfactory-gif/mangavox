//! Measured pacing (design 6.3 table, 8.1 shot timing): cut lengths per scene kind, line
//! timing and camera-move extents. Port of the salvage `pacing.rs` with the dread correction
//! (dread cuts are shorter than the salvage had them: median 2.8 s, mean 2.0-5.0 s, long take 10 s).

use crate::model::{Line, LineKind, Move, SceneKind, Shot};

/// Pause between two lines of one shot when the speaker changes (a turn in the conversation).
/// The shot's length budgets this gap for every pair of lines.
pub const LINE_GAP_S: f32 = 0.35;
/// Pause between two lines of the same speaker (one breath, not a turn).
pub const SAME_SPEAKER_GAP_S: f32 = 0.2;

/// The silence between a line by `prev` and the next by `next` (`None` = the narrator):
/// a short breath when the same voice goes on, a natural turn when another answers. Never more
/// than [`LINE_GAP_S`], so the lines always fit the shot's length.
pub fn line_gap(prev: Option<&str>, next: Option<&str>) -> f32 {
    if prev == next {
        SAME_SPEAKER_GAP_S
    } else {
        LINE_GAP_S
    }
}
/// Silence after the last line of a shot.
pub const TAIL_S: f32 = 0.4;
/// Default length of the first cut of a scene in a new place.
pub const NEW_PLACE_FIRST_S: f32 = 4.2;
/// Spoken words per second of a rough voice (the floor of a line's length).
pub const WORDS_PER_SEC: f32 = 2.5;
/// The shortest a spoken line is ever given.
pub const LINE_MIN_S: f32 = 0.6;
/// The shortest and the longest cut the director may plan.
pub const CUT_MIN_S: f32 = 0.4;
/// The longest cut the director may plan (seconds).
pub const CUT_MAX_S: f32 = 20.;

/// One row of the pacing table.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pacing {
    pub median_cut_s: f32,
    /// (low, high) of the scene's mean cut length.
    pub mean_range_s: (f32, f32),
    pub secs_per_panel: f32,
    pub long_take_s: f32,
    /// Silence before the first line.
    pub line_lead_s: f32,
    /// (low, high) share of fully animated shots.
    pub full_animation: (f32, f32),
}

/// How fast a camera moves over a still in a scene of this kind: (frame widths a second the
/// picture slides, share of the frame a second it grows or shrinks). Salvage numbers.
fn rates(kind: SceneKind) -> (f32, f32) {
    match kind {
        SceneKind::Action => (0.12, 0.05),
        SceneKind::Dread => (0.025, 0.01),
        SceneKind::Narration => (0.025, 0.008),
        SceneKind::Flashback => (0.03, 0.01),
        SceneKind::Montage => (0.04, 0.015),
        _ => (0.02, 0.008),
    }
}

/// The table row of a scene kind (an unknown kind paces like dialogue).
pub fn pace(kind: SceneKind) -> Pacing {
    let row = |median, lo, hi, panel, long, lead, flo, fhi| Pacing {
        median_cut_s: median,
        mean_range_s: (lo, hi),
        secs_per_panel: panel,
        long_take_s: long,
        line_lead_s: lead,
        full_animation: (flo, fhi),
    };
    match kind {
        SceneKind::Action => row(1.6, 1.0, 3.2, 2.0, 10., 0.5, 0.35, 0.85),
        SceneKind::Dread => row(2.8, 2.0, 5.0, 3.6, 10., 1.2, 0., 0.4),
        SceneKind::Narration => row(4.7, 4.0, 8.5, 5.0, 14., 0.6, 0., 0.2),
        SceneKind::Flashback => row(3.6, 3.0, 6.0, 3.6, 10., 0.9, 0., 0.3),
        SceneKind::Montage => row(2.5, 2.0, 4.5, 2.5, 8., 0.6, 0., 0.4),
        _ => row(3.8, 3.0, 6.0, 4.0, 16., 0.9, 0., 0.22),
    }
}

/// Spoken words of a text (tokens holding a letter or digit; "..." and "!?" are not words).
pub fn words(text: &str) -> usize {
    text.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric)).count()
}

/// max(0.6, words / 2.5).
pub fn line_seconds(text: &str) -> f32 {
    (words(text) as f32 / WORDS_PER_SEC).max(LINE_MIN_S)
}

/// The text a line is voiced with: what is spoken, else the manga's words.
fn said(line: &Line) -> Option<&str> {
    if line.kind == LineKind::Sfx {
        return None; // sound effects are never voiced
    }
    let spoken = line.spoken.trim();
    let text = if spoken.is_empty() { line.original.trim() } else { spoken };
    (!text.is_empty()).then_some(text)
}

/// lead + Σ line + 0.35·(n−1) + 0.4 over `secs`; 0 when nothing is said.
fn carried(kind: SceneKind, secs: impl Iterator<Item = f32>) -> f32 {
    let secs: Vec<f32> = secs.filter(|s| s.is_finite() && *s > 0.).collect();
    if secs.is_empty() {
        return 0.;
    }
    pace(kind).line_lead_s + secs.iter().sum::<f32>() + LINE_GAP_S * (secs.len() - 1) as f32 + TAIL_S
}

/// lead(kind) + Σ line_seconds + 0.35·(n−1) + 0.4: the dialogue floor of a shot. Sound effects
/// and empty lines take no time; a shot with nothing said has no floor (0).
pub fn spoken_seconds(lines: &[Line], kind: SceneKind) -> f32 {
    carried(kind, lines.iter().filter_map(said).map(line_seconds))
}

/// A scene's length from its panels, never shorter than its spoken lines (with 15% air),
/// between 2 s and 15 min.
pub fn scene_seconds(kind: SceneKind, panels: usize, spoken: f32) -> f32 {
    let by_panels = panels.max(1) as f32 * pace(kind).secs_per_panel;
    let spoken = if spoken.is_finite() { spoken.max(0.) } else { 0. };
    by_panels.max(spoken * 1.15).clamp(2., 900.)
}

/// `(zoom, travel)` of a camera move over `seconds`: the share of the frame the picture grows
/// or shrinks by (push in, pull out, crash zoom; a push is at least 8% so it reads as a move)
/// and the share of the frame it slides (pans, tilts, trucks, whips, background slides).
/// `amount` scales the usual speed (1.0; kept within 0.25..6). Hold, shake and rack focus: (0, 0).
pub fn camera_extent(kind: SceneKind, mv: Move, seconds: f32, amount: f32) -> (f32, f32) {
    let (travel, zoom) = rates(kind);
    let a = if amount.is_finite() && amount > 0. { amount.clamp(0.25, 6.) } else { 1. };
    let s = if seconds.is_finite() { seconds.clamp(CUT_MIN_S, CUT_MAX_S) } else { CUT_MIN_S };
    match mv {
        Move::PanLeft | Move::PanRight | Move::TiltUp | Move::TiltDown | Move::TruckLeft | Move::TruckRight | Move::BgSlide => {
            (0., (travel * s * a).clamp(0.03, 0.9))
        }
        Move::PushIn | Move::PullOut => ((zoom * s * a).clamp(0.08, 0.35), 0.),
        // A fast move toward the subject, most of it in the cut's first second.
        Move::CrashZoom => ((0.14 * a).clamp(0.08, 0.4), 0.),
        // A whip covers half a frame or more and settles.
        Move::Whip => (0., (0.5 * a).clamp(0.3, 1.2)),
        _ => (0., 0.),
    }
}

/// A shot's length: `locked_s`, else max(planned_s, lead + Σ voiced line seconds + gaps + tail).
/// `line_secs` are the selected voice takes' lengths of the shot's lines, in order. Core (not
/// `voices`) because the checks and the cut both use it.
pub fn effective_s(shot: &Shot, kind: SceneKind, line_secs: &[f32]) -> f32 {
    if let Some(locked) = shot.locked_s.filter(|s| s.is_finite() && *s > 0.) {
        return locked;
    }
    let planned = if shot.planned_s.is_finite() { shot.planned_s.max(0.) } else { 0. };
    planned.max(carried(kind, line_secs.iter().copied()))
}
