//! Batch clip edits and the restyle planner behind the assistant's `video_edit_clips` and
//! `video_restyle`: pure, deterministic functions over a chapter's clip list.
//!
//! A restyle pass works at an energy level (1 calm .. 5 max). It retimes the clips (pacing:
//! a slightly faster narration and long clips split into jump cuts of new framings), gives
//! every clip a motion (varied, never the same move twice running), and every join a
//! transition (cuts inside a beat, quick moves on a new panel, bigger ones on a page turn).
//! Asking for the same energy again, or "more", goes one level up; at the top a pass varies the
//! motion differently, so a pass always changes what the video shows.
use std::collections::BTreeMap;

use crate::vedit::{self, Beat, ChapterEdit, Clip, Motion, TransKind, Transition, MAX_AMOUNT, MAX_SPEED, MIN_MS, MIN_SPEED};
use crate::vedit_manga::is_impact;

// ------------------------------------------------------------------------------------ selecting

/// Which clips a batch edit changes; every condition must hold.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filter {
    /// Clip indexes picked by number (`None`: every clip).
    pub only: Option<Vec<usize>>,
    /// Every `every`th of the matching clips (0 or 1: all), from the `phase`th (0-based).
    pub every: usize,
    pub phase: usize,
    pub longer_ms: Option<u64>,
    pub shorter_ms: Option<u64>,
    /// 0-based pages.
    pub pages: Option<Vec<usize>>,
    /// Only the first clip of each page (a page turn).
    pub page_changes: bool,
    /// Only action beats (an exclamation or a word of force in the caption).
    pub action: bool,
    pub motion: Option<Motion>,
}

fn page_of(beats: &[Beat], c: &Clip) -> Option<usize> {
    beats.get(c.src).map(|b| b.page)
}

fn text_of<'a>(beats: &'a [Beat], c: &'a Clip) -> &'a str {
    c.text.as_deref().or_else(|| beats.get(c.src).map(|b| b.text.as_str())).unwrap_or("")
}

/// Clip `i` turns to a new page.
fn page_turn(beats: &[Beat], clips: &[Clip], i: usize) -> bool {
    i == 0 || page_of(beats, &clips[i - 1]) != page_of(beats, &clips[i])
}

/// Clip `i` plays on from the clip before it (the same beat, split).
fn continues(clips: &[Clip], i: usize) -> bool {
    i > 0 && clips[i - 1].src == clips[i].src
}

/// The indexes `f` picks, in order.
pub fn select(beats: &[Beat], clips: &[Clip], f: &Filter) -> Vec<usize> {
    let hits: Vec<usize> = (0..clips.len())
        .filter(|i| f.only.as_ref().is_none_or(|o| o.contains(i)))
        .filter(|&i| {
            let d = vedit::clip_ms(beats, &clips[i]).unwrap_or(0);
            f.longer_ms.is_none_or(|m| d > m) && f.shorter_ms.is_none_or(|m| d < m)
        })
        .filter(|&i| f.pages.as_ref().is_none_or(|p| page_of(beats, &clips[i]).is_some_and(|pg| p.contains(&pg))))
        .filter(|&i| !f.page_changes || page_turn(beats, clips, i))
        .filter(|&i| !f.action || is_impact(text_of(beats, &clips[i])))
        .filter(|&i| f.motion.is_none_or(|m| clips[i].motion == m))
        .collect();
    let every = f.every.max(1);
    hits.into_iter().enumerate().filter(|(n, _)| *n >= f.phase && (n - f.phase) % every == 0).map(|(_, i)| i).collect()
}

// ------------------------------------------------------------------------------------ batch edits

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MotionPick {
    One(Motion),
    /// A different move per clip from [`VARIED`], never the same twice running.
    Varied,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TransPick {
    /// The look's transition.
    Look,
    Cut,
    One(TransKind),
    /// A different one per clip from [`VARIED_TRANS`].
    Varied,
}

/// What a batch edit sets (`None`: unchanged).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Change {
    pub motion: Option<MotionPick>,
    pub amount: Option<f32>,
    pub focus_from: Option<(f32, f32)>,
    pub focus_to: Option<(f32, f32)>,
    pub speed: Option<f32>,
    pub trans: Option<TransPick>,
    pub trans_ms: Option<u32>,
    /// Source narration trimmed off (ms).
    pub trim_in_ms: Option<u64>,
    pub trim_out_ms: Option<u64>,
    /// Caption text ("" or the narration's own: back to the narration).
    pub caption: Option<String>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        *self == Change::default()
    }
}

pub const VARIED: [Motion; 8] = [Motion::Zoom, Motion::PanRight, Motion::KenBurns, Motion::ZoomOut, Motion::Punch, Motion::PanLeft, Motion::PanDown, Motion::PanUp];
pub const VARIED_TRANS: [TransKind; 6] = [TransKind::WhipLeft, TransKind::Flash, TransKind::SlideUp, TransKind::Zoom, TransKind::WhipRight, TransKind::Fade];

fn pick_avoiding<T: Copy + PartialEq>(list: &[T], at: usize, prev: Option<T>) -> T {
    let m = list[at % list.len()];
    if Some(m) == prev && list.len() > 1 {
        list[(at + 1) % list.len()]
    } else {
        m
    }
}

/// Applies `ch` to the clips at `idxs`. Returns how many clips changed.
pub fn apply(beats: &[Beat], clips: &mut [Clip], idxs: &[usize], ch: &Change) -> Result<usize, String> {
    let before = clips.to_vec();
    for (n, &i) in idxs.iter().enumerate() {
        if i >= clips.len() {
            return Err(format!("There is no clip {} (the chapter has {})", i + 1, clips.len()));
        }
        let prev_motion = i.checked_sub(1).map(|p| clips[p].motion);
        let prev_trans = i.checked_sub(1).and_then(|p| clips[p].trans.map(|t| t.kind));
        let c = &mut clips[i];
        match ch.motion {
            Some(MotionPick::One(Motion::Auto)) => c.reset_motion(),
            Some(MotionPick::One(m)) => c.motion = m,
            Some(MotionPick::Varied) => c.motion = pick_avoiding(&VARIED, n + i, prev_motion),
            None => {}
        }
        if let Some(a) = ch.amount {
            c.amount = Some(a.clamp(0., MAX_AMOUNT));
        }
        if let Some(f) = ch.focus_from {
            c.focus_from = Some(clamp_focus(f));
        }
        if let Some(f) = ch.focus_to {
            c.focus_to = Some(clamp_focus(f));
        }
        if let Some(s) = ch.speed {
            c.speed = s.clamp(MIN_SPEED, MAX_SPEED);
        }
        let ms = ch.trans_ms.map(|m| m.clamp(40, 3000));
        match ch.trans {
            Some(TransPick::Look) => {
                c.cut = false;
                c.trans = None;
            }
            Some(TransPick::Cut) => {
                c.cut = true;
                c.trans = None;
            }
            Some(TransPick::One(k)) => {
                c.cut = false;
                c.trans = Some(Transition { kind: k, ms });
            }
            Some(TransPick::Varied) => {
                c.cut = false;
                c.trans = Some(Transition { kind: pick_avoiding(&VARIED_TRANS, n + i, prev_trans), ms });
            }
            None => {
                if let Some(ms) = ms {
                    c.cut = false;
                    c.trans = Some(Transition { kind: c.trans.map(|t| t.kind).unwrap_or(TransKind::Fade), ms: Some(ms) });
                }
            }
        }
        if let Some(t) = ch.trim_in_ms {
            c.trim_in_ms = t;
        }
        if let Some(t) = ch.trim_out_ms {
            c.trim_out_ms = t;
        }
        if ch.trim_in_ms.is_some() || ch.trim_out_ms.is_some() {
            let beat_ms = beats.get(c.src).map(|b| b.dur_ms).unwrap_or(0);
            if c.trim_in_ms + c.trim_out_ms + MIN_MS > beat_ms {
                return Err(format!("That trims away all of clip {} (its narration is {:.1} s)", i + 1, beat_ms as f64 / 1000.));
            }
        }
        if let Some(t) = &ch.caption {
            let own = beats.get(c.src).map(|b| b.text.as_str()) == Some(t.as_str());
            c.text = (!own && !t.trim().is_empty()).then(|| t.clone());
        }
        if vedit::clip_ms(beats, c).is_none() {
            return Err(format!("Clip {} would be shorter than {} ms", i + 1, MIN_MS));
        }
    }
    Ok(clips.iter().zip(&before).filter(|(a, b)| a != b).count())
}

fn clamp_focus((x, y): (f32, f32)) -> (f32, f32) {
    let c = |v: f32| if v.is_finite() { v.clamp(0., 100.) } else { 50. };
    (c(x), c(y))
}

// ------------------------------------------------------------------------------------ restyle

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Energy {
    /// 1 (calm) .. 5 (max). Asking 3 or more again at or below the current level goes up one.
    Level(u8),
    /// One level up (a first "more" lands on 3).
    More,
    /// One level down.
    Less,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Pacing {
    /// The energy level's.
    #[default]
    Auto,
    Keep,
    /// 1x, no extra cuts.
    Calm,
    Normal,
    /// A little faster narration, and clips over 4.5 s split into jump cuts.
    Fast,
    /// Faster still, clips over 3.2 s split.
    Faster,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum MotionStyle {
    Keep,
    Still,
    /// Slow pushes and pulls.
    Subtle,
    /// Slow Ken Burns drifts and pans.
    Cinematic,
    /// Every kind of move, harder as the energy rises.
    #[default]
    Varied,
    /// Punches, shakes and big moves.
    Aggressive,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum TransStyle {
    Keep,
    /// Back to the look's own.
    Look,
    Cuts,
    /// Fades and dips to black.
    Smooth,
    /// Picked by the energy level: cuts inside a beat, whips / flashes / zooms on new panels.
    #[default]
    Varied,
    /// Flashes, glitches, whips and zoom blurs.
    Flashy,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Restyle {
    pub energy: Energy,
    pub pacing: Pacing,
    pub motion: MotionStyle,
    pub transitions: TransStyle,
    pub beat_sync: bool,
}

impl Default for Restyle {
    fn default() -> Self {
        Restyle { energy: Energy::More, pacing: Pacing::Auto, motion: MotionStyle::Varied, transitions: TransStyle::Varied, beat_sync: false }
    }
}

impl Restyle {
    /// A named style: dynamic, cinematic, calm or hype.
    pub fn style(name: &str) -> Option<Restyle> {
        let r = Restyle::default();
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "dynamic" | "energetic" | "fast" | "punchy" => Restyle { energy: Energy::Level(3), ..r },
            "cinematic" | "film" | "movie" => Restyle { energy: Energy::Level(2), pacing: Pacing::Normal, motion: MotionStyle::Cinematic, transitions: TransStyle::Smooth, ..r },
            "calm" | "chill" | "slow" | "gentle" => Restyle { energy: Energy::Level(1), pacing: Pacing::Calm, motion: MotionStyle::Subtle, transitions: TransStyle::Smooth, ..r },
            "hype" | "trailer" | "max" | "insane" | "aggressive" => Restyle { energy: Energy::Level(5), pacing: Pacing::Faster, motion: MotionStyle::Aggressive, transitions: TransStyle::Flashy, ..r },
            _ => return None,
        })
    }
}

/// The level a pass works at, from the chapter's current one.
pub fn target_level(current: u8, ask: Energy) -> u8 {
    match ask {
        Energy::More => (current.max(2) + 1).min(5),
        Energy::Less => current.saturating_sub(1).max(1),
        Energy::Level(l) if l >= 3 && current >= l => (current + 1).min(5),
        Energy::Level(l) => l.clamp(1, 5),
    }
}

/// What an energy level does.
struct Level {
    /// Motion amount.
    amount: f32,
    /// Clips longer than this split into jump cuts.
    split_ms: Option<u64>,
    speed: f32,
    /// Transition lengths, times each kind's own.
    trans_scale: f32,
}

fn level(l: u8) -> Level {
    match l {
        0 | 1 => Level { amount: 0.04, split_ms: None, speed: 1., trans_scale: 1.4 },
        2 => Level { amount: 0.07, split_ms: None, speed: 1., trans_scale: 1.1 },
        3 => Level { amount: 0.11, split_ms: Some(6_000), speed: 1., trans_scale: 0.9 },
        4 => Level { amount: 0.15, split_ms: Some(4_500), speed: 1.06, trans_scale: 0.75 },
        _ => Level { amount: 0.2, split_ms: Some(3_200), speed: 1.12, trans_scale: 0.6 },
    }
}

/// Ken Burns paths (from, to; % of the panel) a pass rotates through.
const PATHS: [((f32, f32), (f32, f32)); 6] = [
    ((30., 35.), (70., 62.)),
    ((70., 30.), (32., 68.)),
    ((50., 22.), (50., 78.)),
    ((25., 60.), (75., 42.)),
    ((62., 70.), (40., 30.)),
    ((50., 50.), (66., 34.)),
];
/// Where punches and pushes aim at higher energy.
const AIMS: [(f32, f32); 5] = [(50., 42.), (36., 36.), (64., 46.), (50., 60.), (42., 52.)];

fn motions(style: MotionStyle, lv: u8) -> &'static [Motion] {
    use Motion::*;
    match style {
        MotionStyle::Keep => &[],
        MotionStyle::Still => &[Still],
        MotionStyle::Subtle => &[Zoom, ZoomOut, KenBurns, Zoom, Still],
        MotionStyle::Cinematic => &[KenBurns, Zoom, PanRight, ZoomOut, KenBurns, PanDown],
        MotionStyle::Varied if lv <= 2 => &[Zoom, PanRight, ZoomOut, KenBurns, PanLeft, PanDown],
        MotionStyle::Varied => &[Punch, PanRight, KenBurns, ZoomOut, Zoom, PanLeft, Punch, PanUp, KenBurns, PanDown],
        MotionStyle::Aggressive => &[Punch, PanLeft, Punch, ZoomOut, KenBurns, PanRight, Punch, PanUp],
    }
}

/// (into a page turn, into a new panel of the same page) transitions.
fn transitions(style: TransStyle, lv: u8) -> (&'static [TransKind], &'static [TransKind]) {
    use TransKind::*;
    match style {
        TransStyle::Smooth => (&[Black, Fade, Dissolve, Fade], &[Fade, Dissolve]),
        TransStyle::Flashy => (&[Flash, Glitch, WhipLeft, Zoom, WhipRight, Flash, Glitch, Burn], &[Flash, WhipDown, Glitch, WhipLeft]),
        _ => match lv {
            0..=2 => (&[Fade, SlideLeft, Black, SlideUp], &[Fade, Dissolve]),
            3 => (&[WhipLeft, Flash, SlideUp, Zoom, WhipRight, Fade], &[Dissolve, WhipLeft, Fade, WhipUp]),
            4 => (&[WhipLeft, Flash, Zoom, Glitch, WhipRight, Burn], &[WhipUp, Flash, WhipLeft, Dissolve]),
            _ => (&[Flash, Glitch, WhipLeft, Zoom, WhipRight, Flash, Glitch, Burn], &[Flash, WhipDown, WhipLeft, Glitch]),
        },
    }
}

/// What a restyle pass did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub level: u8,
    pub pass: u32,
    pub clips_before: usize,
    pub clips_after: usize,
    /// Extra clips made by splitting long ones.
    pub split: usize,
    /// Clips whose picture changed (motion, framing, speed or transition).
    pub changed: usize,
    pub speed: Option<f32>,
    /// Clips retimed to end on a music beat (`None`: not asked or no beat grid).
    pub beat_synced: Option<usize>,
    pub motions: BTreeMap<&'static str, usize>,
    pub transitions: BTreeMap<&'static str, usize>,
    pub seconds_before: f64,
    pub seconds_after: f64,
}

/// Splits clips longer than `over_ms` into equal jump cuts. Returns how many clips it added.
pub fn split_long(beats: &[Beat], clips: &mut Vec<Clip>, over_ms: u64) -> usize {
    let over_ms = over_ms.max(MIN_MS * 4);
    let mut added = 0;
    let mut i = 0;
    while i < clips.len() {
        let d = vedit::clip_ms(beats, &clips[i]).unwrap_or(0);
        if d > over_ms {
            let n = d.div_ceil(over_ms);
            let part = d / n;
            for _ in 1..n {
                if !vedit::split(beats, clips, i, part) {
                    break;
                }
                added += 1;
                i += 1;
            }
        }
        i += 1;
    }
    added
}

/// Retimes each clip (within `tolerance` of its speed) so it ends on the nearest beat of `grid`
/// (ms on the video's timeline). Returns how many clips moved onto a beat.
pub fn beat_sync(beats: &[Beat], clips: &mut [Clip], grid: &[u64], tolerance: f32) -> usize {
    if grid.is_empty() {
        return 0;
    }
    let mut t = 0u64;
    let mut n = 0;
    for c in clips.iter_mut() {
        let (Some(src), Some(d)) = (vedit::src_ms(beats, c), vedit::clip_ms(beats, c)) else { continue };
        let end = t + d;
        let i = grid.partition_point(|b| *b < end);
        let near = [i.checked_sub(1).and_then(|j| grid.get(j)), grid.get(i)].into_iter().flatten().copied().filter(|b| *b >= t + MIN_MS).min_by_key(|b| b.abs_diff(end));
        if let Some(b) = near {
            let s = src as f32 / (b - t) as f32;
            let base = c.speed();
            if b != end && (s / base - 1.).abs() <= tolerance && (MIN_SPEED..=MAX_SPEED).contains(&s) {
                c.speed = s;
                n += 1;
            } else if b == end {
                n += 1;
            }
        }
        t += vedit::clip_ms(beats, c).unwrap_or(0);
    }
    n
}

/// The beats (ms) of a score's cues: (start s, end s, bpm) each.
pub fn beat_grid(cues: &[(f64, f64, f32)]) -> Vec<u64> {
    let mut out = vec![];
    for &(start, end, bpm) in cues {
        if !(bpm.is_finite() && bpm >= 30. && end > start) {
            continue;
        }
        let step = 60. / bpm as f64;
        let mut t = start;
        while t < end {
            out.push((t * 1000.).round() as u64);
            t += step;
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn total(beats: &[Beat], clips: &[Clip]) -> f64 {
    (vedit::total_ms(&vedit::place(beats, clips)) as f64 / 100.).round() / 10.
}

/// One pass of `r` over the chapter's clips (`grid`: the music's beats, for `beat_sync`).
pub fn restyle(beats: &[Beat], e: &mut ChapterEdit, r: &Restyle, grid: &[u64]) -> Report {
    let before = e.clips(beats);
    let lv_n = target_level(e.energy, r.energy);
    let mut pass = e.passes;
    let mut out = plan(beats, &before, r, lv_n, pass, grid);
    // A pass always changes the picture: when this one lands where the clips already are,
    // vary the moves differently.
    if r.motion != MotionStyle::Keep {
        for _ in 0..4 {
            if out.0 != before {
                break;
            }
            pass += 1;
            out = plan(beats, &before, r, lv_n, pass, grid);
        }
    }
    let (clips, mut report) = out;
    report.level = lv_n;
    report.pass = pass + 1;
    report.clips_before = before.len();
    report.clips_after = clips.len();
    report.seconds_before = total(beats, &before);
    report.seconds_after = total(beats, &clips);
    for c in &clips {
        *report.motions.entry(c.motion.key()).or_default() += 1;
        if c.cut {
            *report.transitions.entry("cut").or_default() += 1;
        } else if let Some(t) = c.trans {
            *report.transitions.entry(t.kind.key()).or_default() += 1;
        }
    }
    // Clips that match none of the clips before the pass.
    report.changed = clips.iter().filter(|c| !before.contains(c)).count();
    e.energy = lv_n;
    e.passes = pass + 1;
    e.set_clips(beats, clips);
    report
}

fn plan(beats: &[Beat], before: &[Clip], r: &Restyle, lv_n: u8, pass: u32, grid: &[u64]) -> (Vec<Clip>, Report) {
    let lv = level(lv_n);
    let mut report = Report::default();
    let mut clips = before.to_vec();
    let p = pass as usize;

    // Pacing: speed, then jump cuts.
    let (speed, split_ms) = match r.pacing {
        Pacing::Keep => (None, None),
        Pacing::Auto => (Some(lv.speed), lv.split_ms),
        Pacing::Calm | Pacing::Normal => (Some(1.), None),
        Pacing::Fast => (Some(level(4).speed.max(lv.speed)), level(4).split_ms),
        Pacing::Faster => (Some(level(5).speed), level(5).split_ms),
    };
    if let Some(s) = speed {
        for c in &mut clips {
            c.speed = s;
        }
        report.speed = Some(s);
    }
    if let Some(over) = split_ms {
        report.split = split_long(beats, &mut clips, over);
    }

    // Motion.
    let list = motions(r.motion, lv_n);
    if !list.is_empty() {
        let shake_impacts = r.motion == MotionStyle::Aggressive || (lv_n >= 4 && r.motion == MotionStyle::Varied);
        let punch_impacts = lv_n == 3 && r.motion == MotionStyle::Varied;
        let amount = match r.motion {
            MotionStyle::Subtle => lv.amount * 0.6,
            MotionStyle::Cinematic => lv.amount.min(0.08),
            MotionStyle::Aggressive => lv.amount.max(0.18),
            _ => lv.amount,
        };
        let mut prev: Option<Motion> = None;
        for i in 0..clips.len() {
            let impact = is_impact(text_of(beats, &clips[i]));
            let m = if impact && shake_impacts {
                Motion::Shake
            } else if impact && punch_impacts {
                Motion::Punch
            } else {
                pick_avoiding(list, i + p * 3, prev)
            };
            let c = &mut clips[i];
            c.motion = m;
            c.focus_from = None;
            c.focus_to = None;
            c.amount = match m {
                Motion::Still | Motion::Auto => None,
                Motion::Shake => Some(amount.max(0.12)),
                Motion::Punch => Some((amount * 1.3).max(0.1)),
                _ => Some(amount),
            };
            match m {
                Motion::KenBurns => {
                    let (a, b) = PATHS[(i + p) % PATHS.len()];
                    c.focus_from = Some(a);
                    c.focus_to = Some(b);
                }
                Motion::Punch | Motion::Zoom if lv_n >= 3 => c.focus_to = Some(AIMS[(i + p) % AIMS.len()]),
                _ => {}
            }
            prev = Some(m);
        }
    }

    // Transitions.
    if r.transitions != TransStyle::Keep {
        let (turns, panels) = transitions(r.transitions, lv_n);
        let (mut nt, mut np) = (p, p);
        for i in 0..clips.len() {
            let turn = page_turn(beats, &clips, i);
            let cont = continues(&clips, i);
            let impact = is_impact(text_of(beats, &clips[i]));
            let c = &mut clips[i];
            c.cut = false;
            c.trans = None;
            if i == 0 {
                continue;
            }
            let kind = match r.transitions {
                TransStyle::Look => continue,
                TransStyle::Cuts => {
                    c.cut = true;
                    continue;
                }
                _ if cont && r.transitions != TransStyle::Smooth => {
                    c.cut = true;
                    continue;
                }
                _ if impact && lv_n >= 4 => TransKind::Flash,
                _ if turn => {
                    nt += 1;
                    turns[(nt - 1) % turns.len()]
                }
                _ => {
                    np += 1;
                    panels[(np - 1) % panels.len()]
                }
            };
            let ms = (kind.default_ms() as f32 * lv.trans_scale).round() as u32;
            c.trans = Some(Transition { kind, ms: Some(ms.clamp(120, 1500)) });
        }
    }

    if r.beat_sync {
        report.beat_synced = (!grid.is_empty()).then(|| beat_sync(beats, &mut clips, grid, 0.08));
    }
    (clips, report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn beats() -> Vec<Beat> {
        let b = |page: usize, text: &str, dur_ms: u64| Beat { page, image: None, text: text.into(), audio: PathBuf::from("a.wav"), offset_ms: 0, dur_ms };
        vec![
            b(0, "The city sleeps.", 4000),
            b(0, "A figure walks the roofs.", 7000),
            b(1, "He strikes!", 2000),
            b(1, "Silence again.", 3000),
            b(2, "Morning comes.", 9000),
            b(3, "The end.", 2500),
        ]
    }

    #[test]
    fn filters_pick_by_number_length_page_and_every_nth() {
        let b = beats();
        let clips: Vec<Clip> = (0..b.len()).map(Clip::of).collect();
        assert_eq!(select(&b, &clips, &Filter::default()), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(select(&b, &clips, &Filter { every: 3, ..Default::default() }), vec![0, 3]);
        assert_eq!(select(&b, &clips, &Filter { every: 2, phase: 1, ..Default::default() }), vec![1, 3, 5]);
        assert_eq!(select(&b, &clips, &Filter { longer_ms: Some(3500), ..Default::default() }), vec![0, 1, 4]);
        assert_eq!(select(&b, &clips, &Filter { shorter_ms: Some(3000), ..Default::default() }), vec![2, 5]);
        assert_eq!(select(&b, &clips, &Filter { pages: Some(vec![1]), ..Default::default() }), vec![2, 3]);
        assert_eq!(select(&b, &clips, &Filter { page_changes: true, ..Default::default() }), vec![0, 2, 4, 5]);
        assert_eq!(select(&b, &clips, &Filter { action: true, ..Default::default() }), vec![2]);
        assert_eq!(select(&b, &clips, &Filter { only: Some(vec![1, 2, 4]), longer_ms: Some(3000), ..Default::default() }), vec![1, 4]);
    }

    #[test]
    fn batch_changes_apply_to_every_selected_clip() {
        let b = beats();
        let mut clips: Vec<Clip> = (0..b.len()).map(Clip::of).collect();
        let ch = Change { motion: Some(MotionPick::Varied), amount: Some(0.2), speed: Some(1.25), trans: Some(TransPick::One(TransKind::Flash)), trans_ms: Some(250), ..Default::default() };
        let n = apply(&b, &mut clips, &[1, 2, 3], &ch).unwrap();
        assert_eq!(n, 3);
        assert_eq!(clips[0], Clip::of(0), "unselected clips stay");
        for c in &clips[1..4] {
            assert_eq!(c.amount, Some(0.2));
            assert_eq!(c.speed, 1.25);
            assert_eq!(c.trans, Some(Transition { kind: TransKind::Flash, ms: Some(250) }));
        }
        assert!(clips[1].motion != clips[2].motion && clips[2].motion != clips[3].motion, "varied never repeats");
        // A speed-up shortens the clip; the narration slice stays whole.
        assert_eq!(vedit::clip_ms(&b, &clips[1]), Some(5600));
        assert_eq!(vedit::src_ms(&b, &clips[1]), Some(7000));
        // Back to the look; out-of-range values clamp.
        apply(&b, &mut clips, &[1], &Change { motion: Some(MotionPick::One(Motion::Auto)), trans: Some(TransPick::Look), speed: Some(9.), ..Default::default() }).unwrap();
        assert_eq!((clips[1].motion, clips[1].amount, clips[1].trans, clips[1].speed), (Motion::Auto, None, None, MAX_SPEED));
        // Trimming a clip away fails.
        assert!(apply(&b, &mut clips, &[2], &Change { trim_in_ms: Some(1950), ..Default::default() }).is_err());
    }

    #[test]
    fn energy_levels_climb_on_repeat_and_more() {
        assert_eq!(target_level(0, Energy::More), 3);
        assert_eq!(target_level(3, Energy::More), 4);
        assert_eq!(target_level(5, Energy::More), 5);
        assert_eq!(target_level(0, Energy::Level(3)), 3);
        assert_eq!(target_level(3, Energy::Level(3)), 4, "asking high again goes higher");
        assert_eq!(target_level(4, Energy::Level(1)), 1, "calm is set as asked");
        assert_eq!(target_level(4, Energy::Less), 3);
    }

    #[test]
    fn restyle_twice_raises_energy_and_changes_the_clips() {
        let b = beats();
        let mut e = ChapterEdit::default();
        let dynamic = Restyle::style("dynamic").unwrap();

        let r1 = restyle(&b, &mut e, &dynamic, &[]);
        let first = e.clips(&b);
        assert_eq!((r1.level, e.energy), (3, 3));
        assert!(r1.changed > 0);
        // Level 3 splits the 7 s and 9 s clips into jump cuts.
        assert_eq!(r1.split, 2);
        assert_eq!(first.len(), 8);
        assert!(first.iter().all(|c| c.motion != Motion::Auto), "every clip moves");
        assert!(first.windows(2).all(|w| w[0].motion != w[1].motion), "never the same move twice running");
        assert!(first[0].trans.is_none() && !first[0].cut, "nothing transitions into the first clip");
        assert!(first[1..].iter().all(|c| c.cut || c.trans.is_some()), "every join is planned");
        // A split part cuts straight in from its own beat.
        let cont = first.windows(2).position(|w| w[0].src == w[1].src).unwrap() + 1;
        assert!(first[cont].cut);
        // The action beat punches in.
        assert_eq!(first.iter().find(|c| c.src == 2).unwrap().motion, Motion::Punch);

        // "More dynamic" again: a level up, faster, more cuts, different clips.
        let r2 = restyle(&b, &mut e, &dynamic, &[]);
        let second = e.clips(&b);
        assert_eq!((r2.level, e.energy, e.passes), (4, 4, 2));
        assert!(second != first && r2.changed > 0);
        assert!(r2.seconds_after < r1.seconds_after, "pacing tightens");
        assert!(second.iter().all(|c| c.speed > 1.));
        assert_eq!(second.iter().find(|c| c.src == 2).unwrap().motion, Motion::Shake, "action beats shake at level 4");

        // At the top it still changes something every time.
        let mut last = second;
        for want in [5, 5, 5] {
            let r = restyle(&b, &mut e, &Restyle { energy: Energy::More, ..dynamic }, &[]);
            let now = e.clips(&b);
            assert_eq!(r.level, want);
            assert!(now != last, "a pass is never a no-op");
            last = now;
        }
    }

    #[test]
    fn restyle_is_deterministic_and_calm_undoes_the_pace() {
        let b = beats();
        let (mut a, mut c) = (ChapterEdit::default(), ChapterEdit::default());
        let r = Restyle::default();
        restyle(&b, &mut a, &r, &[]);
        restyle(&b, &mut c, &r, &[]);
        assert_eq!(a.clips(&b), c.clips(&b));

        let calm = Restyle::style("calm").unwrap();
        let rep = restyle(&b, &mut a, &calm, &[]);
        assert_eq!(rep.level, 1);
        let clips = a.clips(&b);
        assert!(clips.iter().all(|c| c.speed == 1.));
        assert!(clips.iter().all(|c| matches!(c.motion, Motion::Zoom | Motion::ZoomOut | Motion::KenBurns | Motion::Still)));
        assert!(clips.iter().skip(1).all(|c| matches!(c.trans.map(|t| t.kind), Some(TransKind::Fade | TransKind::Dissolve | TransKind::Black))));
    }

    #[test]
    fn beat_sync_lands_cuts_on_the_music() {
        let b = beats();
        // 120 bpm: a beat every 500 ms.
        let grid = beat_grid(&[(0., 60., 120.)]);
        assert_eq!(&grid[..3], &[0, 500, 1000]);
        let mut clips: Vec<Clip> = (0..b.len()).map(Clip::of).collect();
        clips[0].speed = 1.05; // 3810 ms: the nearest beat is 4000.
        let n = beat_sync(&b, &mut clips, &grid, 0.08);
        assert_eq!(n, clips.len());
        let placed = vedit::place(&b, &clips);
        for p in &placed {
            assert!(p.end_ms() % 500 <= 1 || p.end_ms() % 500 >= 499, "clip {} ends at {}", p.index, p.end_ms());
        }
        // Too far from a beat for the tolerance: left alone.
        let mut one = vec![Clip::of(0)];
        assert_eq!(beat_sync(&b, &mut one, &[0, 2000, 9000], 0.05), 0);
        assert_eq!(one[0].speed, 1.);

        let mut e = ChapterEdit::default();
        let r = restyle(&b, &mut e, &Restyle { beat_sync: true, ..Restyle::default() }, &grid);
        assert_eq!(r.beat_synced, Some(r.clips_after));
        let r = restyle(&b, &mut e, &Restyle { beat_sync: true, ..Restyle::default() }, &[]);
        assert_eq!(r.beat_synced, None, "no beat grid: nothing synced");
    }
}
