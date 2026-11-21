//! Step 4, Shots (design 9.7) [shots]: the user directs the film. W3-S3.
//!
//! Scenes on the left (each with its planning state, its issues and "outdated" when the script
//! changed after planning). The centre shows the selected scene: its shots as a strip of cards
//! (add, duplicate, delete, reorder, insert a coverage pattern), the camera map of the selected
//! shot (the place's floor plan, the characters' marks, the camera and its view cone, the
//! 180-degree line), and the film-grammar checks with a fix button wherever one can be made by
//! hand-free editing. The right column is the shot inspector (`inspector.rs`).
//!
//! Planning runs through the engine's queue (one PlanScene job per scene); a re-plan keeps every
//! field the user set. Edits apply to the screen's copy at once and are saved in the background
//! as read-modify-writes of the scene's shots file, after which the checks run again.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context as _;
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_anime::director::{self, SceneInput};
use mv_anime::merge::is_locked;
use mv_anime::queue::{self, JobKind, JobState, Queue};
use mv_anime::{checks, geometry, pacing, patterns, Angle, BibleIndex, Card, Emotion, EmotionKind, Issue, Line, LookAt, Mark, MouthPolicy, Move, Pattern, Place, Scene, SceneKind, Severity, Shot, ShotSize, Story, Store, Visibility};
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::{
    btn, confirm_dialog, empty_view, error_view, filter_chip, icon_button, icon_ph, icon_tag, menu_button, muted, ph, progress_bar, size, skeleton_block, skeleton_line, skeleton_row, spinner, tip,
    toast, ButtonKind, MenuItem, Ph, Toast, Tokens, Tone,
};
use serde_json::Value;

use super::inspector::{self, lock, mark_ptr, move_word, reset, size_word, Ask, Inspect, ShotFn, HEIGHT, POSITION};
use super::shared::{bible_notice, plain_reason, shot_secs, SAVE_FAILED};
use super::{Step, StepCtx};
use crate::map::{MapEvent, MapItem, MapScene, MapView, HINT};
use crate::page::Load;
use crate::style::{COLUMN_HEAD_H, POLL_MS};
use crate::widgets::{asks_first, column_body, column_head, confirm_bulk, count, empty_gallery, error_block, fmt_secs, hover_actions, info_line, is_narrow, label_caps, list_item, marker, outdated_tag, selectable, spec, workspace, LineTone};

/// Width of a card in the shot strip.
const CARD_W: f32 = 156.;
/// Height of the camera map.
const MAP_H: f32 = 380.;
/// Characters drawn on a shot card before "+n".
const CARD_FACES: usize = 4;
/// How far from the others a character put into a shot stands (metres).
const ADD_OFFSET_M: f32 = 1.;

/// One save at a time, so read-modify-writes of a scene's shots never interleave.
static SAVE: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------- edits (pure)

/// One edit of a scene's shots, applied to the screen's copy at once and to the file's copy when
/// saving. Every edit finds its shots by id, so it applies the same way to both.
type ShotsOp = Arc<dyn Fn(&mut Vec<Shot>) + Send + Sync>;

fn op(f: impl Fn(&mut Vec<Shot>) + Send + Sync + 'static) -> ShotsOp {
    Arc::new(f)
}

/// An edit of one shot.
fn on_shot(id: String, f: ShotFn) -> ShotsOp {
    op(move |shots| {
        if let Some(s) = shots.iter_mut().find(|s| s.id == id) {
            f(s)
        }
    })
}

/// Set fields and make them the user's, or put them back to Auto.
fn edit_fn(pointers: Vec<String>, set: Option<ShotFn>) -> ShotFn {
    Arc::new(move |s: &mut Shot| match &set {
        Some(f) => {
            f(s);
            for p in &pointers {
                lock(s, p);
            }
        }
        None => {
            for p in &pointers {
                reset(s, p);
            }
        }
    })
}

/// The shot as a suggestion (for "Reset to Auto"): without ids, cards or bookkeeping.
fn suggestion(s: &Shot) -> Value {
    let mut v = serde_json::to_value(s).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        for k in ["id", "frame", "clip", "suggested", "locked", "edited", "locked_s"] {
            o.remove(k);
        }
    }
    v
}

/// The number in a shot id ("sh0012" → 12).
fn id_number(id: &str) -> Option<u32> {
    id.strip_prefix("sh")?.parse().ok()
}

/// The next free "sh0001"-style id after the counter `n` (raised to it).
fn next_id(taken: &BTreeSet<String>, n: &mut u32) -> String {
    loop {
        *n += 1;
        let id = format!("sh{:04}", *n);
        if !taken.contains(&id) {
            return id;
        }
    }
}

fn move_shot(shots: &mut [Shot], id: &str, forward: bool) -> bool {
    let Some(i) = shots.iter().position(|s| s.id == id) else { return false };
    let j = if forward { i + 1 } else { i.wrapping_sub(1) };
    if j >= shots.len() {
        return false;
    }
    shots.swap(i, j);
    true
}

fn remove_shot(shots: &mut Vec<Shot>, id: &str) -> bool {
    let before = shots.len();
    shots.retain(|s| s.id != id);
    shots.len() != before
}

/// Puts a deleted shot back: before `before` while that shot is there, else after `after`, else
/// at the end. Its lines leave the shots they were shared out to. Nothing happens when it is
/// already there.
fn restore_shot(shots: &mut Vec<Shot>, shot: &Shot, before: Option<&str>, after: Option<&str>) {
    if shots.iter().any(|s| s.id == shot.id) {
        return;
    }
    for s in shots.iter_mut() {
        s.lines.retain(|l| !shot.lines.contains(l));
    }
    let at = before
        .and_then(|b| shots.iter().position(|s| s.id == b))
        .or_else(|| after.and_then(|a| shots.iter().position(|s| s.id == a)).map(|i| i + 1))
        .unwrap_or(shots.len());
    shots.insert(at, shot.clone());
}

/// Undo of a delete: the deleted shot back in its place, and every shot of the scene as it was
/// just before the delete (`scene_before`), so a neighbour that got longer when the shot went
/// is back to its own length. Pictures recorded since stay; shots added since stay, after them.
fn undo_delete(shots: &mut Vec<Shot>, shot: &Shot, before: Option<&str>, after: Option<&str>, scene_before: &[Shot]) {
    restore_shot(shots, shot, before, after);
    let mut out: Vec<Shot> = Vec::with_capacity(shots.len());
    for old in scene_before {
        let Some(now) = shots.iter().find(|s| s.id == old.id) else { continue };
        let mut back = old.clone();
        back.frame = now.frame.clone();
        back.clip = now.clip.clone();
        out.push(back);
    }
    out.extend(shots.iter().filter(|s| !scene_before.iter().any(|o| o.id == s.id)).cloned());
    *shots = out;
}

/// Puts `new` before the shot `before` (at the end when it is gone or `None`); shots already
/// there (same id) are not added twice.
fn insert_before(shots: &mut Vec<Shot>, before: Option<&str>, new: &[Shot]) {
    let fresh: Vec<Shot> = new.iter().filter(|n| !shots.iter().any(|s| s.id == n.id)).cloned().collect();
    let at = before.and_then(|b| shots.iter().position(|s| s.id == b)).unwrap_or(shots.len());
    for (k, s) in fresh.into_iter().enumerate() {
        shots.insert(at + k, s);
    }
}

/// Puts `new` after the shot `after` (at the end when it is gone or `None`).
fn insert_after(shots: &mut Vec<Shot>, after: Option<&str>, new: &[Shot]) {
    let next = after.and_then(|a| shots.iter().position(|s| s.id == a)).and_then(|i| shots.get(i + 1)).map(|s| s.id.clone());
    insert_before(shots, next.as_deref(), new);
}

/// Moves a line to the shot before or after the one carrying it; both shots' lines become the
/// user's.
fn move_line(shots: &mut [Shot], line: &str, forward: bool) -> bool {
    let Some(i) = shots.iter().position(|s| s.lines.iter().any(|l| l == line)) else { return false };
    let j = if forward { i + 1 } else { i.wrapping_sub(1) };
    if j >= shots.len() {
        return false;
    }
    shots[i].lines.retain(|l| l != line);
    if forward {
        shots[j].lines.insert(0, line.to_string());
    } else {
        shots[j].lines.push(line.to_string());
    }
    lock(&mut shots[i], "/lines");
    lock(&mut shots[j], "/lines");
    true
}

/// Wider (negative `by`) or closer framings along the size ladder.
const LADDER: [ShotSize; 8] = [ShotSize::Els, ShotSize::Ls, ShotSize::Fs, ShotSize::Cowboy, ShotSize::Ms, ShotSize::Mcu, ShotSize::Cu, ShotSize::Ecu];

fn step_size(size: ShotSize, by: i32) -> Option<ShotSize> {
    let i = LADDER.iter().position(|s| *s == size)? as i32;
    let j = (i + by).clamp(0, LADDER.len() as i32 - 1);
    (j != i).then(|| LADDER[j as usize])
}

/// Splits a shot's lines over it and a new shot right after it (one size closer, so the two
/// never make a jump cut).
fn split_shot(shots: &mut Vec<Shot>, id: &str, new_id: &str) -> bool {
    if shots.iter().any(|s| s.id == new_id) {
        return false;
    }
    let Some(i) = shots.iter().position(|s| s.id == id) else { return false };
    if shots[i].lines.len() < 2 {
        return false;
    }
    let half = shots[i].lines.len().div_ceil(2);
    let mut second = shots[i].clone();
    second.id = new_id.to_string();
    second.frame = Card::default();
    second.clip = None;
    second.lines = shots[i].lines.split_off(half);
    second.planned_s = 0.;
    second.locked_s = None;
    if let Some(s) = step_size(second.camera.size, 1).or_else(|| step_size(second.camera.size, -1)) {
        second.camera.size = s;
        lock(&mut second, "/camera/size");
    }
    if !second.tells.trim().is_empty() {
        second.tells = format!("{} (continued)", second.tells.trim());
    }
    lock(&mut shots[i], "/lines");
    lock(&mut second, "/lines");
    second.suggested = suggestion(&second);
    shots.insert(i + 1, second);
    true
}

/// Yaw (0 = up, clockwise) from one plan point toward another.
fn yaw_to(from: [f32; 2], to: [f32; 2]) -> f32 {
    let (dx, dz) = (to[0] - from[0], to[1] - from[1]);
    if dx.abs() < 1e-4 && dz.abs() < 1e-4 {
        return 180.;
    }
    dx.atan2(-dz).to_degrees().rem_euclid(360.)
}

/// The point `dist` metres from `target` in the direction of `from` (south when they meet).
fn pull_back(from: [f32; 2], target: [f32; 2], dist: f32) -> [f32; 2] {
    let (dx, dz) = (from[0] - target[0], from[1] - target[1]);
    let len = (dx * dx + dz * dz).sqrt();
    let (ux, uz) = if len.is_finite() && len > 1e-3 { (dx / len, dz / len) } else { (0., 1.) };
    [target[0] + ux * dist, target[1] + uz * dist]
}

/// The middle of the characters seen (all of them when nobody is seen).
fn centroid(marks: &[Mark]) -> [f32; 2] {
    let seen: Vec<&Mark> = marks.iter().filter(|m| m.visible != Visibility::Offscreen).collect();
    let pick: Vec<&Mark> = if seen.is_empty() { marks.iter().collect() } else { seen };
    if pick.is_empty() {
        return [0., 0.];
    }
    let n = pick.len() as f32;
    [pick.iter().map(|m| m.pos[0]).sum::<f32>() / n, pick.iter().map(|m| m.pos[1]).sum::<f32>() / n]
}

/// Inside the place, or a little beyond its walls (a camera may shoot in through a window).
fn clamp_plan(place: Option<&Place>, p: [f32; 2], beyond: f32) -> [f32; 2] {
    match place.map(|pl| (pl.plan.w_m, pl.plan.d_m)) {
        Some((w, d)) if w.is_finite() && d.is_finite() && w > 0. && d > 0. => [p[0].clamp(-w / 2. - beyond, w / 2. + beyond), p[1].clamp(-d / 2. - beyond, d / 2. + beyond)],
        _ => p,
    }
}

/// A shot the screen makes from the shots around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cover {
    /// A wide that shows where everyone is (a layout shot).
    Wide,
    /// Everyone full figure.
    Full,
    /// A plain medium shot (the "Add a shot" button).
    Medium,
    /// The floor from above: a bridge across the line.
    Overhead,
    /// The two who fight in one frame.
    Both,
}

/// A new shot of `cover`'s kind built on `base` (the shot it goes next to): the same people on
/// the same marks, every one in full, the camera on the same side of them.
fn cover_shot(cover: Cover, scene: &Scene, base: Option<&Shot>, cast: &[String], place: Option<&Place>) -> Shot {
    let size = match cover {
        Cover::Wide => ShotSize::Ls,
        Cover::Full | Cover::Both => ShotSize::Fs,
        Cover::Medium => ShotSize::Ms,
        Cover::Overhead => ShotSize::Overhead,
    };
    let mut marks: Vec<Mark> = base.map(|b| b.marks.clone()).unwrap_or_default();
    if marks.is_empty() {
        marks = patterns::template(Pattern::Arrival, scene, cast).get(1).map(|s| s.marks.clone()).unwrap_or_default();
    }
    if cover != Cover::Medium {
        for m in &mut marks {
            m.visible = Visibility::Full;
        }
    }
    let target = centroid(&marks);
    let mut camera = patterns::default_camera(scene, size);
    let overhead = (cover == Cover::Overhead).then(|| place.and_then(|p| p.stations.iter().find(|s| s.overhead))).flatten();
    match (cover, overhead) {
        (Cover::Overhead, Some(st)) => {
            camera.station = Some(st.id.clone());
            camera.pos = None;
            camera.look_at = LookAt::Angles { yaw_deg: st.yaw_deg, pitch_deg: st.pitch_deg };
            if st.lens_mm > 0 {
                camera.lens_mm = st.lens_mm;
            }
        }
        (Cover::Overhead, None) => {
            camera.pos = Some([target[0], patterns::camera_height(Angle::Top, size).max(8.), target[1]]);
            camera.look_at = LookAt::Angles { yaw_deg: 0., pitch_deg: -90. };
        }
        _ => {
            let from = base.map(|b| geometry::cam_pos(&b.camera, place)).map(|p| [p[0], p[2]]).unwrap_or([target[0], target[1] + 1.]);
            let xz = clamp_plan(place, pull_back(from, target, patterns::camera_distance(size).max(0.5)), crate::map::OUTSIDE_M);
            let at = [xz[0], patterns::camera_height(camera.angle, size), xz[1]];
            camera.pos = Some(at);
            camera.look_at = LookAt::Point([target[0], 1., target[1]]);
            let (yaw, _) = geometry::look_dir(&camera, &marks, place);
            camera.station = place.and_then(|p| geometry::nearest_station(p, at, yaw));
        }
    }
    let tells = match cover {
        Cover::Wide => "where everyone is: the place and who stands where",
        Cover::Full => "everyone seen whole before we go in close",
        Cover::Medium => "",
        Cover::Overhead => "the floor from above, so the move to the other side reads",
        Cover::Both => "both fighters in one frame, so the hit reads",
    };
    let mut shot = Shot {
        // A sentence, so it starts with a capital letter ("The floor from above…").
        tells: patterns::capitalised(tells),
        origin: mv_anime::Origin::Anime,
        camera,
        marks,
        background: if cover == Cover::Medium { base.map(|b| b.background).unwrap_or_default() } else { mv_anime::BackgroundKind::Matched },
        bridge: cover == Cover::Overhead,
        edited: true,
        ..Shot::default()
    };
    shot.suggested = suggestion(&shot);
    shot
}

/// A fix the screen can make for an issue of the checks.
#[derive(Clone, Debug, PartialEq)]
enum Fix {
    /// Put a new shot before this one (`None`: at the end).
    Cover { cover: Cover, before: Option<String> },
    Size { shot: String, size: ShotSize },
    Angle { shot: String, angle: Angle },
    /// Stop the camera move.
    Still(String),
    /// Mouth shapes drawn on the still instead of a generated mouth.
    Flap(String),
    /// Hold the first cut long enough.
    Hold(String),
    /// Every line of the scene in exactly one shot again.
    ShareLines,
    /// Split the shot's lines over two shots.
    Split(String),
    /// Only a new plan can fix it.
    Replan,
    /// The line itself must change, in the Story step.
    Story,
}

impl Fix {
    fn label(&self) -> String {
        match self {
            Fix::Cover { cover: Cover::Wide, .. } => "Add a wide shot".into(),
            Fix::Cover { cover: Cover::Full, .. } | Fix::Cover { cover: Cover::Medium, .. } => "Add a full shot first".into(),
            Fix::Cover { cover: Cover::Overhead, .. } => "Add a bridge shot".into(),
            Fix::Cover { cover: Cover::Both, .. } => "Add a shot of both".into(),
            Fix::Size { size, .. } => format!("Make it {}", size_word(*size).to_lowercase()),
            Fix::Angle { .. } => "Change the angle".into(),
            Fix::Still(_) => "Hold the camera still".into(),
            Fix::Flap(_) => "Use drawn mouth shapes".into(),
            Fix::Hold(_) => format!("Hold it {:.1} s", checks::FIRST_HOLD_S),
            Fix::ShareLines => "Share the lines out".into(),
            Fix::Split(_) => "Split this shot".into(),
            Fix::Replan => "Plan again".into(),
            Fix::Story => "Go to Story".into(),
        }
    }
}

/// The fix for an issue, when there is one to make.
fn fix_for(issue: &Issue, shots: &[Shot]) -> Option<Fix> {
    if issue.code.starts_with("plan-") {
        return None;
    }
    let at = issue.shot.as_ref().and_then(|id| shots.iter().position(|s| &s.id == id));
    let shot = at.map(|i| shots[i].id.clone());
    let fix = match (issue.code.as_str(), at, shot) {
        ("layout-early", _, _) => Fix::Cover { cover: Cover::Wide, before: shots.first().map(|s| s.id.clone()) },
        ("layout-return", Some(i), _) => {
            let end = shots.iter().enumerate().skip(i).find(|(_, s)| checks::is_layout(s)).map(|(j, _)| j).unwrap_or(shots.len());
            let mid = i + (end - i) / 2;
            Fix::Cover { cover: Cover::Wide, before: shots.get(mid).map(|s| s.id.clone()) }
        }
        ("axis", _, Some(id)) => Fix::Cover { cover: Cover::Overhead, before: Some(id) },
        ("threat-before-hit", _, Some(id)) => Fix::Cover { cover: Cover::Both, before: Some(id) },
        ("progression", _, Some(id)) => Fix::Cover { cover: Cover::Full, before: Some(id) },
        ("close-run", _, Some(id)) => Fix::Size { shot: id, size: ShotSize::Ms },
        ("jump-cut", Some(i), Some(id)) => {
            let c = &shots[i].camera;
            match step_size(c.size, -2).or_else(|| step_size(c.size, 2)) {
                Some(size) => Fix::Size { shot: id, size },
                None => Fix::Angle { shot: id, angle: if c.angle == Angle::Eye { Angle::High } else { Angle::Eye } },
            }
        }
        ("shake-talk", _, Some(id)) => Fix::Still(id),
        ("mouth", _, Some(id)) => Fix::Flap(id),
        ("first-hold", _, Some(id)) => Fix::Hold(id),
        ("lines-cover", _, _) => Fix::ShareLines,
        ("lines-fit", Some(i), Some(id)) if shots[i].lines.len() >= 2 => Fix::Split(id),
        ("line-length", _, _) => Fix::Story,
        _ => Fix::Replan,
    };
    Some(fix)
}

/// Why a planned scene no longer matches its script, if it does not.
fn outdated(lines: &[Line], shots: &[Shot], place: Option<&Place>, kind: SceneKind) -> Option<String> {
    if shots.is_empty() {
        return None;
    }
    let carried: BTreeSet<&str> = shots.iter().flat_map(|s| s.lines.iter().map(String::as_str)).collect();
    let script: BTreeSet<&str> = lines.iter().map(|l| l.id.as_str()).collect();
    let new = script.difference(&carried).count();
    let gone = carried.difference(&script).count();
    let place_changed = shots.iter().any(|s| s.camera.station.as_ref().is_some_and(|id| !place.is_some_and(|p| p.stations.iter().any(|st| &st.id == id))));
    let longer = shots.iter().any(|s| {
        if is_locked("/planned_s", &s.locked) || s.locked_s.is_some() || s.lines.is_empty() {
            return false;
        }
        let ls: Vec<Line> = s.lines.iter().filter_map(|id| lines.iter().find(|l| &l.id == id).cloned()).collect();
        !ls.is_empty() && pacing::spoken_seconds(&ls, kind) > s.planned_s + 0.05
    });
    let verb = |n: usize| if n == 1 { "is" } else { "are" };
    let mut why: Vec<String> = Vec::new();
    if new > 0 {
        why.push(format!("{} of the script {} in no shot", count(new, "line"), verb(new)));
    }
    if gone > 0 {
        why.push(format!("{} in the shots {} no longer in the script", count(gone, "line"), verb(gone)));
    }
    if place_changed {
        why.push("the scene's place changed".into());
    }
    if longer {
        why.push("lines got longer than their shots".into());
    }
    (!why.is_empty()).then(|| format!("The script changed after planning: {}.", why.join("; ")))
}

/// Scene ids in the episode's order (its own order first, then the script's).
fn scene_order(episode: &[String], scenes: &[Scene]) -> Vec<String> {
    let mut out: Vec<String> = episode.iter().filter(|e| scenes.iter().any(|s| &s.id == *e)).cloned().collect();
    for s in scenes {
        if !out.contains(&s.id) {
            out.push(s.id.clone());
        }
    }
    out
}


#[cfg(feature = "bible")]
fn bible_missing(bible: &BibleIndex) -> Vec<String> {
    mv_anime::bible::gate_missing(bible)
}

#[cfg(not(feature = "bible"))]
fn bible_missing(_bible: &BibleIndex) -> Vec<String> {
    Vec::new()
}

// ---------------------------------------------------------------- jobs (pure)

/// Where the latest planning job of a scene stands.
#[derive(Clone, Debug, PartialEq)]
enum Busy {
    Idle,
    Waiting { id: u64, paused: bool },
    Running { id: u64, progress: f32, line: String },
    Failed { id: u64, error: String },
}

impl Busy {
    fn active(&self) -> bool {
        matches!(self, Busy::Waiting { .. } | Busy::Running { .. })
    }

    /// Why the scene's shots cannot be changed now: "Planning…" only while the job really runs.
    fn note(&self) -> &'static str {
        match self {
            Busy::Waiting { paused: true, .. } => "Waiting to plan this scene (the queue is paused). Shots can be changed again once it is planned, or cancel it.",
            Busy::Waiting { .. } => "Waiting in the queue to plan this scene. Shots can be changed again once it is planned, or cancel it.",
            _ => "Planning this scene… Shots can be changed again when it finishes.",
        }
    }
}

/// What a job's state means for the screen: a waiting job of a paused queue is paused, and a
/// "running" one left over from a closed app (held until Resume) is not running either. Only a
/// job that really runs shows a spinner.
fn plan_state(q: &Queue, state: JobState) -> JobState {
    match state {
        JobState::Queued if q.paused || q.interrupted => JobState::Paused,
        JobState::Running if q.interrupted => JobState::Paused,
        s => s,
    }
}

fn job_busy(q: &Queue, project: &str, ep: &str, scene: &str) -> Busy {
    let mine = |k: &JobKind| matches!(k, JobKind::PlanScene { episode, scene: s } if episode == ep && s == scene);
    let Some(j) = q.jobs.iter().filter(|j| j.project == project && mine(&j.kind) && !(j.state == JobState::Failed && j.hidden)).max_by_key(|j| j.id) else { return Busy::Idle };
    match plan_state(q, j.state) {
        JobState::Queued => Busy::Waiting { id: j.id, paused: false },
        JobState::Paused => Busy::Waiting { id: j.id, paused: true },
        JobState::Running => Busy::Running { id: j.id, progress: j.progress, line: j.line.clone() },
        JobState::Failed => Busy::Failed { id: j.id, error: j.error.clone().filter(|e| !e.trim().is_empty()).unwrap_or_else(|| j.line.clone()) },
        _ => Busy::Idle,
    }
}

/// The AI waits for a usage limit to reset (the job's status line says so).
fn waits_for_limit(line: &str) -> bool {
    line.contains("usage limit") || line.contains("Waiting about")
}

// ---------------------------------------------------------------- data

#[derive(Clone)]
struct Data {
    story: Story,
    bible: Arc<BibleIndex>,
    approved: bool,
    missing: Vec<String>,
    /// Scene id -> its shots.
    shots: BTreeMap<String, Vec<Shot>>,
    /// The planner's notes (codes `plan-*`) from `checks.json`.
    notes: Vec<Issue>,
    /// Scene id -> the checks of its shots now, plus its notes.
    issues: BTreeMap<String, Vec<Issue>>,
    /// Scene ids in display order.
    order: Vec<String>,
    /// Every shot id of the episode.
    taken: BTreeSet<String>,
    /// The project's shot id counter.
    next_shot: u32,
}

impl Data {
    fn scene(&self, id: &str) -> Option<&Scene> {
        self.story.scenes.iter().find(|s| s.id == id)
    }

    /// 1-based number of a scene in display order.
    fn number(&self, id: &str) -> usize {
        self.order.iter().position(|s| s == id).map_or(0, |i| i + 1)
    }

    fn lines(&self, scene: &Scene) -> Vec<Line> {
        director::scene_lines(&self.story, scene)
    }

    fn place(&self, scene: &Scene) -> Option<&Place> {
        scene.place.as_deref().and_then(|p| self.bible.place(p))
    }

    fn shots_of(&self, id: &str) -> &[Shot] {
        self.shots.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    fn new_place(&self, id: &str) -> bool {
        director::opens_new_place(&self.story, id)
    }

    fn name(&self, id: &str) -> String {
        self.bible.character(id).map(|c| c.name.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| id.to_string())
    }

    fn hue(&self, id: &str) -> usize {
        self.bible.characters.iter().position(|c| c.id == id).unwrap_or_else(|| id.bytes().fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize)))
    }

    /// Characters of a scene, focal first (the director's order).
    fn cast(&self, scene: &Scene) -> Vec<String> {
        let lines = self.lines(scene);
        director::cast(&SceneInput { scene, lines: &lines, bible: &self.bible, new_place: false })
    }

    fn outdated(&self, scene: &Scene) -> Option<String> {
        outdated(&self.lines(scene), self.shots_of(&scene.id), self.place(scene), scene.kind)
    }

    fn recheck(&mut self, id: &str) {
        let Some(scene) = self.scene(id) else { return };
        let lines = self.lines(scene);
        let mut out = checks::check_scene(scene, self.shots_of(id), &lines, &self.bible, self.new_place(id));
        out.extend(self.notes.iter().filter(|n| n.scene == id).cloned());
        out.sort_by_key(|i| match i.severity {
            Severity::Error => 0,
            Severity::Warning => 1,
            _ => 2,
        });
        self.issues.insert(id.to_string(), out);
    }

    /// Applies an edit to the screen's copy (with the line, timing and mouth rules applied again
    /// when `settle`) and checks the scene again.
    fn change(&mut self, id: &str, edit: &ShotsOp, settle: bool) {
        let Some(scene) = self.scene(id).cloned() else { return };
        let lines = self.lines(&scene);
        let new_place = self.new_place(id);
        let bible = self.bible.clone();
        let list = self.shots.entry(id.to_string()).or_default();
        edit(list);
        if settle {
            director::settle(list, &SceneInput { scene: &scene, lines: &lines, bible: &bible, new_place });
        }
        self.taken.extend(list.iter().map(|s| s.id.clone()));
        self.recheck(id);
    }
}

/// Reads the script, the bible and every scene's shots. Blocking.
fn load(store: &Store, ep: &str) -> anyhow::Result<Data> {
    let series = store.series().context("The project could not be read")?;
    let episode = store.episode(ep).context("The episode could not be read")?;
    let story = store.story(ep).context("The script could not be read")?;
    let bible = store.bible().unwrap_or_else(|e| {
        mv_core::logging::global().warn("anime", &format!("shots step: the bible could not be read: {e:#}"));
        BibleIndex::default()
    });
    let shots = store.all_shots(ep).context("The shots could not be read")?;
    let notes: Vec<Issue> = store.checks(ep).unwrap_or_default().into_iter().filter(|i| i.code.starts_with("plan-")).collect();
    let approved = series.bible_approved.is_some();
    let missing = if approved { Vec::new() } else { bible_missing(&bible) };
    let order = scene_order(&episode.scene_order, &story.scenes);
    let taken = shots.values().flatten().map(|s| s.id.clone()).collect();
    let next_shot = series.next_ids.get("shot").copied().unwrap_or(0);
    let mut d = Data { story, bible: Arc::new(bible), approved, missing, shots, notes, issues: BTreeMap::new(), order, taken, next_shot };
    for id in d.order.clone() {
        d.recheck(&id);
    }
    Ok(d)
}

/// What a save of one scene needs besides the file.
struct SaveCtx {
    scene: Scene,
    lines: Vec<Line>,
    bible: Arc<BibleIndex>,
    new_place: bool,
}

/// Applies `ops` to the file's shots of the scene, settles them, saves, and runs the episode's
/// checks again. Blocking.
fn save_ops(store: &Store, ep: &str, cx: &SaveCtx, ops: &[ShotsOp]) -> anyhow::Result<Vec<Shot>> {
    let _guard = SAVE.lock().unwrap_or_else(|e| e.into_inner());
    let scene = cx.scene.id.as_str();
    let safe = mv_core::paths::safe_name(scene);
    // The other scenes' shot ids, read before any lock is taken (slow).
    let others: BTreeSet<String> = store.all_shots(ep).unwrap_or_default().into_iter().filter(|(k, _)| k != scene && *k != safe).flat_map(|(_, v)| v.into_iter().map(|s| s.id)).collect();
    // Read, settled, changed and written under the scene file's own lock, so a frame job
    // recording its pictures at the same moment never loses them nor this change. New ids are
    // handed out inside it under the counters lock (lock order: scene file, counters, project
    // file; nothing takes them the other way round); nothing else is saved in here.
    let rel = format!("episodes/{}/shots/{safe}.json", mv_core::paths::safe_name(ep));
    let mut failed: Option<anyhow::Error> = None;
    let file = store.update_json::<mv_anime::SceneShots>(&rel, |file| {
        if file.episode.is_empty() {
            file.episode = ep.to_string();
        }
        if file.scene.is_empty() {
            file.scene = scene.to_string();
        }
        let before = file.shots.clone();
        for o in ops {
            o(&mut file.shots);
        }
        director::settle(&mut file.shots, &SceneInput { scene: &cx.scene, lines: &cx.lines, bible: &cx.bible, new_place: cx.new_place });
        if let Err(e) = give_ids(store, &others, &mut file.shots) {
            // Saved as it was: a shot without its own id must never reach the disk.
            file.shots = before;
            failed = Some(e);
        }
    })?;
    if let Some(e) = failed {
        return Err(e);
    }
    if let Err(e) = refresh_checks(store, ep, &cx.bible) {
        mv_core::logging::global().warn("anime", &format!("shots step: the checks were not saved: {e:#}"));
    }
    Ok(file.shots)
}

/// Every shot gets an id no other shot of the episode has (`others`: the other scenes' ids). New
/// ids are handed out, and the project's counter raised, under the counters lock on a fresh read
/// of `project.json` (a PlanScene job hands them out the same way), so the two never give out
/// the same id and no stale copy of the project file is saved.
fn give_ids(store: &Store, others: &BTreeSet<String>, shots: &mut [Shot]) -> anyhow::Result<()> {
    // Nothing to hand out and the counter already covers every id: the project file is left
    // alone (every write makes the screens read again).
    let mut seen = others.clone();
    let mut top = 0;
    let mut fresh = false;
    for s in shots.iter() {
        fresh |= s.id.is_empty() || seen.contains(&s.id);
        seen.insert(s.id.clone());
        top = top.max(id_number(&s.id).unwrap_or(0));
    }
    let counter = store.series().ok().map(|s| s.next_ids.get("shot").copied().unwrap_or(0));
    if !fresh && counter.is_some_and(|c| c >= top) {
        return Ok(());
    }
    mv_anime::story::bump_ids(store, |counters| {
        let mut n = counters.get("shot").copied().unwrap_or(0);
        let mut taken = others.clone();
        for s in shots.iter_mut() {
            if s.id.is_empty() || taken.contains(&s.id) {
                s.id = next_id(&taken, &mut n);
            }
            taken.insert(s.id.clone());
            n = n.max(id_number(&s.id).unwrap_or(0));
        }
        counters.insert("shot".into(), n);
    })?;
    Ok(())
}

/// The episode's checks file: every scene's checks now plus the planner's notes. Blocking.
fn refresh_checks(store: &Store, ep: &str, bible: &BibleIndex) -> anyhow::Result<()> {
    let story = store.story(ep)?;
    let all = store.all_shots(ep)?;
    let mut issues = checks::check_episode(&story, &all, bible);
    issues.extend(store.checks(ep).unwrap_or_default().into_iter().filter(|i| i.code.starts_with("plan-")));
    store.save_checks(ep, &issues)
}

/// Sets a scene's place in the script (the user's choice: the Story step's place lock). Blocking.
fn set_place(store: &Store, ep: &str, scene: &str, place: &str) -> anyhow::Result<()> {
    let _guard = SAVE.lock().unwrap_or_else(|e| e.into_inner());
    // Checked first, so a scene gone meanwhile is said plainly (nothing is written then).
    if !store.story(ep)?.scenes.iter().any(|s| s.id == scene) {
        anyhow::bail!("The scene is no longer in the script");
    }
    // Changed under the story file's own lock, so a voice batch or a Sound edit landing at the
    // same moment is kept.
    store.update_story(ep, |story| {
        if let Some(s) = story.scenes.iter_mut().find(|s| s.id == scene) {
            s.place = Some(place.to_string());
            s.locked.insert("/place".into());
            s.edited = true;
        }
    })?;
    Ok(())
}

// ---------------------------------------------------------------- the view

/// A text box of the inspector, bound to one shot (and character) of one scene.
struct TextBox {
    scene: String,
    key: String,
    input: Entity<TextareaState>,
    /// What the user typed and has not been saved yet (kept on every change). While it is set,
    /// a redraw never puts the shot's saved words back into the box: GPUI moves the focus on
    /// mouse-down and redraws before the click lands, and that redraw used to wipe the words.
    typed: Option<String>,
    /// The shot's words last put into the box. A redraw compares with this (not with the box's
    /// text), so a box that holds the words slightly differently is never set again on every
    /// frame (each set redraws, which kept an idle screen busy).
    shown: String,
    _sub: Subscription,
}

/// Rows of an inspector text box: it grows from two lines up to six, then scrolls.
const TEXT_ROWS: (usize, usize) = (2, 6);

/// Puts `value` in a box with the caret and the view at its start, so the text reads from its
/// first word (not scrolled to its end).
fn show_from_start(s: &mut TextareaState, value: String, window: &mut Window, cx: &mut Context<TextareaState>) {
    s.set_value(value, window, cx);
    s.set_selected_range(0..0, cx);
    s.set_scroll_offset(point(px(0.), px(0.)), cx);
}

impl TextBox {
    /// The words typed, tidied (runs of spaces and line breaks become one space).
    fn text(&self, cx: &App) -> String {
        tidy(self.typed.as_deref().map(str::to_string).unwrap_or_else(|| self.input.read(cx).value().to_string()).as_str())
    }
}

/// Runs of spaces and line breaks become one space.
fn tidy(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Text {
    Tells,
    Action,
}

pub struct ShotsStep {
    ctx: StepCtx,
    store: Store,
    data: Load<Rc<Data>>,
    queue: Queue,
    seen: [u64; 2],
    scene: Option<String>,
    shot: Option<String>,
    /// The character whose acting the inspector shows.
    character: Option<String>,
    map: Entity<MapView>,
    tells: Option<TextBox>,
    action: Option<TextBox>,
    pending: Vec<(String, ShotsOp)>,
    saving: bool,
    stale: bool,
    /// The project's files changed: read them again when this view is next drawn (a step that
    /// is not on screen never re-reads).
    refresh: bool,
    loading: bool,
    again: bool,
    /// Narrow window: the inspector drawer is open.
    drawer: bool,
    strip: ScrollHandle,
}

pub fn open(ctx: StepCtx, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| {
        let map = cx.new(|cx| MapView::new(MapScene::default(), cx));
        cx.subscribe_in(&map, window, |this: &mut ShotsStep, _, ev: &MapEvent, window, cx| this.on_map(ev, window, cx)).detach();
        let mut s = ShotsStep {
            store: Store::open(&ctx.project),
            ctx,
            data: Load::Loading,
            queue: queue::snapshot(),
            seen: versions(),
            scene: None,
            shot: None,
            character: None,
            map,
            tells: None,
            action: None,
            pending: Vec::new(),
            saving: false,
            stale: false,
            refresh: false,
            loading: false,
            again: false,
            drawer: false,
            strip: ScrollHandle::new(),
        };
        s.reload(cx);
        s.poll(window, cx);
        s
    })
    .into()
}

fn versions() -> [u64; 2] {
    [mv_anime::store::version(), queue::version()]
}

fn eid(prefix: &str, key: &str, n: u64) -> ElementId {
    ElementId::NamedInteger(SharedString::from(format!("{prefix}-{key}")), n)
}

impl ShotsStep {
    fn ready(&self) -> Option<Rc<Data>> {
        match &self.data {
            Load::Ready(d) => Some(d.clone()),
            _ => None,
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            self.again = true;
            return;
        }
        self.loading = true;
        let (store, ep) = (self.store.clone(), self.ctx.episode.clone());
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || load(&store, &ep)).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match res {
                    // Edits made while it was reading are newer: the next save brings the file's copy.
                    Ok(_) if this.saving || !this.pending.is_empty() => this.stale = true,
                    Ok(d) => {
                        this.data = Load::Ready(Rc::new(d));
                        this.ensure_selection();
                        this.sync_map(cx);
                    }
                    Err(e) => {
                        if !matches!(this.data, Load::Ready(_)) {
                            this.data = Load::Failed("The shots could not be read. Check that the project's drive is connected, then try again.".into());
                        }
                        mv_core::logging::global().warn("anime", &format!("shots step: {e:#}"));
                    }
                }
                if std::mem::take(&mut this.again) {
                    this.reload(cx);
                }
                cx.notify();
                crate::redraw(cx);
            });
        })
        .detach();
    }

    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(POLL_MS)).await;
            if this.update(cx, |s, cx| s.tick(cx)).is_err() {
                break;
            }
        })
        .detach();
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let v = versions();
        if v == self.seen {
            return;
        }
        if v[1] != self.seen[1] {
            self.queue = queue::snapshot();
            self.sync_map(cx);
        }
        if v[0] != self.seen[0] {
            if self.saving || !self.pending.is_empty() {
                self.stale = true;
            } else {
                // Read again when drawn: right away while on screen, else when shown next.
                self.refresh = true;
            }
        }
        self.seen = v;
        cx.notify();
    }

    /// Keeps the selected scene and shot valid (the first ones when gone).
    fn ensure_selection(&mut self) {
        let Some(d) = self.ready() else { return };
        if !self.scene.as_ref().is_some_and(|s| d.scene(s).is_some()) {
            self.scene = d.order.first().cloned();
            self.shot = None;
        }
        let shots = self.scene.as_deref().map(|s| d.shots_of(s)).unwrap_or(&[]);
        if !self.shot.as_ref().is_some_and(|id| shots.iter().any(|s| &s.id == id)) {
            self.shot = shots.first().map(|s| s.id.clone());
            self.character = None;
        }
    }

    fn current_ids(&self) -> Option<(String, String)> {
        Some((self.scene.clone()?, self.shot.clone()?))
    }

    fn select_scene(&mut self, id: String, cx: &mut Context<Self>) {
        self.commit_all(cx);
        if self.scene.as_deref() != Some(id.as_str()) {
            self.scene = Some(id);
            self.shot = None;
            self.character = None;
            self.strip.set_offset(point(px(0.), px(0.)));
            self.ensure_selection();
            self.map.update(cx, |m, cx| m.select(None, cx));
            self.sync_map(cx);
        }
        cx.notify();
    }

    fn select_shot(&mut self, id: String, cx: &mut Context<Self>) {
        self.commit_all(cx);
        if self.shot.as_deref() != Some(id.as_str()) {
            self.shot = Some(id);
            self.character = None;
            self.map.update(cx, |m, cx| m.select(None, cx));
            self.sync_map(cx);
        }
        cx.notify();
    }

    fn scene_busy(&self, scene: &str) -> Busy {
        job_busy(&self.queue, &self.ctx.project, &self.ctx.episode, scene)
    }

    /// Planning jobs of this episode really running now.
    fn running_jobs(&self) -> usize {
        self.plan_jobs(|s| s == JobState::Running)
    }

    /// Planning jobs of this episode waiting their turn in a queue that runs.
    fn waiting_jobs(&self) -> usize {
        self.plan_jobs(|s| s == JobState::Queued)
    }

    /// Planning jobs of this episode held: paused, in a paused queue, or left from a closed app.
    fn paused_jobs(&self) -> usize {
        self.plan_jobs(|s| s == JobState::Paused)
    }

    fn plan_jobs(&self, state: impl Fn(JobState) -> bool) -> usize {
        self.queue.jobs.iter().filter(|j| j.project == self.ctx.project && state(plan_state(&self.queue, j.state)) && matches!(&j.kind, JobKind::PlanScene { episode, .. } if *episode == self.ctx.episode)).count()
    }

    /// Shows the selected shot on the camera map (read-only while its scene is being planned).
    fn sync_map(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.ready() else { return };
        let scene = self.scene.as_deref().and_then(|s| d.scene(s));
        let shot = scene.zip(self.shot.as_deref()).and_then(|(sc, id)| d.shots_of(&sc.id).iter().find(|s| s.id == id));
        let next = match (scene, shot) {
            (Some(sc), Some(sh)) => {
                let label = format!("{}.{}", d.number(&sc.id), d.shots_of(&sc.id).iter().position(|s| s.id == sh.id).map_or(0, |i| i + 1));
                MapScene::camera_map(d.place(sc), sh, &d.bible.characters, label)
            }
            _ => MapScene::default(),
        };
        let busy = scene.is_some_and(|s| self.scene_busy(&s.id).active());
        self.map.update(cx, |m, cx| {
            if *m.scene() != next {
                m.set_scene(next, cx);
            }
            m.set_read_only(busy, cx);
        });
    }

    // ------------------------------------------------------------ saving

    /// Applies an edit to the screen now and saves it in the background.
    fn apply(&mut self, scene: &str, edit: ShotsOp, cx: &mut Context<Self>) {
        let Load::Ready(rc) = &mut self.data else { return };
        Rc::make_mut(rc).change(scene, &edit, true);
        self.pending.push((scene.to_string(), edit));
        self.flush(cx);
        self.ensure_selection();
        self.sync_map(cx);
        cx.notify();
    }

    fn flush(&mut self, cx: &mut Context<Self>) {
        if self.saving || self.pending.is_empty() {
            return;
        }
        let Some(d) = self.ready() else { return };
        let scene_id = self.pending[0].0.clone();
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending).into_iter().partition(|(s, _)| *s == scene_id);
        self.pending = rest;
        let ops: Vec<ShotsOp> = mine.into_iter().map(|(_, o)| o).collect();
        let Some(scene) = d.scene(&scene_id).cloned() else {
            self.flush(cx);
            return;
        };
        let save = SaveCtx { lines: d.lines(&scene), new_place: d.new_place(&scene_id), bible: d.bible.clone(), scene };
        self.saving = true;
        let (store, ep) = (self.store.clone(), self.ctx.episode.clone());
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || save_ops(&store, &ep, &save, &ops)).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
                match res {
                    Ok(shots) => {
                        let more = this.pending.iter().any(|(s, _)| *s == scene_id);
                        if !more {
                            if let Load::Ready(rc) = &mut this.data {
                                let d = Rc::make_mut(rc);
                                d.taken.extend(shots.iter().map(|s| s.id.clone()));
                                d.shots.insert(scene_id.clone(), shots);
                                d.recheck(&scene_id);
                            }
                            this.ensure_selection();
                            this.sync_map(cx);
                        }
                    }
                    Err(e) => {
                        mv_core::logging::global().warn("anime", &format!("shots step: save failed: {e:#}"));
                        toast::show(cx, Toast::error(SAVE_FAILED));
                        this.pending.retain(|(s, _)| *s != scene_id);
                        this.stale = true;
                    }
                }
                this.flush(cx);
                if !this.saving && this.pending.is_empty() && std::mem::take(&mut this.stale) {
                    this.reload(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A new shot id, kept from every other shot of the episode.
    fn take_id(&mut self) -> Option<String> {
        let Load::Ready(rc) = &mut self.data else { return None };
        let d = Rc::make_mut(rc);
        let id = next_id(&d.taken, &mut d.next_shot);
        d.taken.insert(id.clone());
        Some(id)
    }

    // ------------------------------------------------------------ planning

    fn plan(&mut self, scenes: Vec<String>, cx: &mut Context<Self>) {
        let Some(d) = self.ready() else { return };
        if scenes.is_empty() {
            return;
        }
        self.commit_all(cx);
        crate::backends::ensure_runner(cx);
        for id in scenes {
            let n = d.number(&id);
            queue::enqueue(JobKind::PlanScene { episode: self.ctx.episode.clone(), scene: id }, &self.ctx.project, format!("Plan the shots of scene {n}"));
        }
        self.queue = queue::snapshot();
        self.sync_map(cx);
        cx.notify();
    }

    /// Scenes the main button plans: those without shots, else the outdated ones.
    fn to_plan(&self, d: &Data) -> (Vec<String>, bool) {
        let idle = |id: &String| !self.scene_busy(id).active();
        let unplanned: Vec<String> = d.order.iter().filter(|id| d.shots_of(id).is_empty() && idle(id)).cloned().collect();
        if !unplanned.is_empty() {
            return (unplanned, false);
        }
        let stale: Vec<String> = d.order.iter().filter(|id| idle(id) && d.scene(id).is_some_and(|s| d.outdated(s).is_some())).cloned().collect();
        (stale, true)
    }

    // ------------------------------------------------------------ shots

    fn add_shot(&mut self, cx: &mut Context<Self>) {
        self.add_cover(Cover::Medium, None, true, cx);
    }

    /// The selected scene, when its shots can be changed now (not while it is being planned).
    fn editable_scene(&self) -> Option<String> {
        self.scene.clone().filter(|s| !self.scene_busy(s).active())
    }

    /// Puts a covering shot before `before` (or after the selected one when `after_selected`).
    fn add_cover(&mut self, cover: Cover, before: Option<String>, after_selected: bool, cx: &mut Context<Self>) {
        let Some(scene_id) = self.editable_scene() else { return };
        self.commit_all(cx);
        let (scene, mut shot) = {
            let Some(d) = self.ready() else { return };
            let Some(scene) = d.scene(&scene_id).cloned() else { return };
            let shots = d.shots_of(&scene.id);
            let base = if after_selected {
                self.shot.as_deref().and_then(|id| shots.iter().find(|s| s.id == id)).or(shots.last())
            } else {
                before.as_deref().and_then(|b| shots.iter().find(|s| s.id == b)).or(shots.last())
            };
            let shot = cover_shot(cover, &scene, base, &d.cast(&scene), d.place(&scene));
            (scene, shot)
        };
        let Some(id) = self.take_id() else { return };
        shot.id = id.clone();
        let new = [shot];
        let edit = if after_selected {
            let after = self.shot.clone();
            op(move |list| insert_after(list, after.as_deref(), &new))
        } else {
            op(move |list| insert_before(list, before.as_deref(), &new))
        };
        self.apply(&scene.id, edit, cx);
        self.select_shot(id, cx);
    }

    fn duplicate(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(scene) = self.editable_scene() else { return };
        self.commit_all(cx);
        let Some(orig) = self.ready().and_then(|d| d.shots_of(&scene).iter().find(|s| s.id == id).cloned()) else { return };
        let Some(new_id) = self.take_id() else { return };
        let mut copy = orig;
        copy.id = new_id.clone();
        copy.frame = Card::default();
        copy.clip = None;
        copy.lines.clear();
        copy.locked.remove("/lines");
        copy.locked_s = None;
        copy.edited = true;
        let new = [copy];
        self.apply(&scene, op(move |list| insert_after(list, Some(&id), &new)), cx);
        self.select_shot(new_id, cx);
    }

    fn confirm_delete(&mut self, id: String, number: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(scene) = self.editable_scene() else { return };
        let Some(pictured) = self.ready().and_then(|d| d.shots_of(&scene).iter().find(|s| s.id == id).map(|s| !s.frame.takes.is_empty())) else { return };
        if !pictured {
            self.delete(scene, id, cx);
            return;
        }
        let weak = cx.entity().downgrade();
        confirm_dialog(
            format!("Delete shot {number}?"),
            "It has pictures. They stay in the project's history, but this shot and its camera are gone. Its lines move to the shots around it.",
            "Delete shot",
            true,
        )
        .on_confirm(move |_, cx| {
            let (scene, id) = (scene.clone(), id.clone());
            let _ = weak.update(cx, |this, cx| this.delete(scene, id, cx));
        })
        .open(window, cx);
    }

    fn delete(&mut self, scene: String, id: String, cx: &mut Context<Self>) {
        if self.scene_busy(&scene).active() {
            return;
        }
        self.commit_all(cx);
        // The shot as it is now and the shots around it, to put it back in its place.
        let Some((removed, next, before, after, scene_before)) = self.ready().and_then(|d| {
            let shots = d.shots_of(&scene);
            let i = shots.iter().position(|s| s.id == id)?;
            let near = |j: Option<usize>| j.and_then(|j| shots.get(j)).map(|s| s.id.clone());
            let next = near(Some(i + 1)).or_else(|| near(i.checked_sub(1)));
            Some((shots[i].clone(), next, near(Some(i + 1)), near(i.checked_sub(1)), Arc::new(shots.to_vec())))
        }) else {
            return;
        };
        let gone = id.clone();
        self.apply(&scene, op(move |list| {
            remove_shot(list, &gone);
        }), cx);
        if self.shot.as_deref() == Some(id.as_str()) {
            self.shot = next;
            self.ensure_selection();
            self.sync_map(cx);
        }
        let this = cx.entity().downgrade();
        toast::show(
            cx,
            Toast::info("Shot deleted.").icon(ph::TRASH).action("Undo", move |_, cx| {
                let (scene, shot, before, after, was) = (scene.clone(), removed.clone(), before.clone(), after.clone(), scene_before.clone());
                let _ = this.update(cx, |s, cx| s.restore_shot(scene, shot, before, after, was, cx));
            }),
        );
        cx.notify();
    }

    /// Puts a deleted shot back where it was (before the shot that followed it, else after the
    /// one before it), with its own lines again, and the scene's other shots as they were just
    /// before the delete (`was`).
    fn restore_shot(&mut self, scene: String, shot: Shot, before: Option<String>, after: Option<String>, was: Arc<Vec<Shot>>, cx: &mut Context<Self>) {
        if self.scene_busy(&scene).active() {
            toast::show(cx, Toast::info("The scene is being planned. Undo is not possible now."));
            return;
        }
        self.commit_all(cx);
        let id = shot.id.clone();
        self.apply(&scene, op(move |list| undo_delete(list, &shot, before.as_deref(), after.as_deref(), &was)), cx);
        if self.scene.as_deref() == Some(scene.as_str()) {
            self.select_shot(id, cx);
        }
    }

    fn move_shot(&mut self, id: String, forward: bool, cx: &mut Context<Self>) {
        let Some(scene) = self.editable_scene() else { return };
        self.commit_all(cx);
        self.apply(&scene, op(move |list| {
            move_shot(list, &id, forward);
        }), cx);
    }

    fn add_pattern(&mut self, pattern: Pattern, cx: &mut Context<Self>) {
        let Some(scene_id) = self.editable_scene() else { return };
        self.commit_all(cx);
        let mut new = {
            let Some(d) = self.ready() else { return };
            let Some(scene) = d.scene(&scene_id) else { return };
            let lines = d.lines(scene);
            let input = SceneInput { scene, lines: &lines, bible: &d.bible, new_place: d.new_place(&scene.id) };
            let mut new = patterns::template(pattern, scene, &director::cast(&input));
            director::place_shots(&mut new, &input);
            new
        };
        for s in new.iter_mut() {
            s.edited = true;
            s.suggested = suggestion(s);
        }
        for s in new.iter_mut() {
            match self.take_id() {
                Some(id) => s.id = id,
                None => return,
            }
        }
        let first = new.first().map(|s| s.id.clone());
        let after = self.shot.clone();
        self.apply(&scene_id, op(move |list| insert_after(list, after.as_deref(), &new)), cx);
        if let Some(id) = first {
            self.select_shot(id, cx);
        }
    }

    fn fix(&mut self, fix: Fix, cx: &mut Context<Self>) {
        let Some(scene) = self.editable_scene() else { return };
        self.commit_all(cx);
        let on =|id: String, pointers: &[&str], f: fn(&mut Shot)| on_shot(id, edit_fn(pointers.iter().map(|p| p.to_string()).collect(), Some(Arc::new(f))));
        match fix {
            Fix::Cover { cover, before } => {
                self.add_cover(cover, before, false, cx);
            }
            Fix::Size { shot, size } => {
                let lens = self.ready().and_then(|d| d.scene(&scene).map(|sc| patterns::default_camera(sc, size).lens_mm)).unwrap_or(35);
                self.apply(&scene, on_shot(shot.clone(), Arc::new(move |s: &mut Shot| {
                    s.camera.size = size;
                    lock(s, "/camera/size");
                    if !is_locked("/camera/lens_mm", &s.locked) {
                        s.camera.lens_mm = lens;
                    }
                })), cx);
                self.select_shot(shot, cx);
            }
            Fix::Angle { shot, angle } => {
                self.apply(&scene, on_shot(shot.clone(), edit_fn(vec!["/camera/angle".into()], Some(Arc::new(move |s: &mut Shot| s.camera.angle = angle)))), cx);
                self.select_shot(shot, cx);
            }
            Fix::Still(shot) => self.apply(&scene, on(shot, &["/camera/movement/kind"], |s| s.camera.movement.kind = Move::Hold), cx),
            Fix::Flap(shot) => self.apply(&scene, on(shot, &["/mouth"], |s| s.mouth = MouthPolicy::Flap), cx),
            Fix::Hold(shot) => self.apply(&scene, on(shot, &["/planned_s"], |s| s.planned_s = s.planned_s.max(checks::FIRST_HOLD_S)), cx),
            Fix::ShareLines => self.apply(&scene, op(|list| {
                for s in list.iter_mut() {
                    s.locked.remove("/lines");
                }
            }), cx),
            Fix::Split(shot) => {
                let Some(new_id) = self.take_id() else { return };
                let n = new_id.clone();
                self.apply(&scene, op(move |list| {
                    split_shot(list, &shot, &n);
                }), cx);
                self.select_shot(new_id, cx);
            }
            Fix::Replan => self.plan(vec![scene], cx),
            // The issue list opens the Story step itself (a plain button, outside this view's update).
            Fix::Story => {}
        }
    }

    // ------------------------------------------------------------ the map and the inspector

    /// Puts the camera on a station: its position, direction and (unless set by the user) lens.
    fn station_fn(place: &Place, id: &str) -> Option<ShotFn> {
        let st = place.stations.iter().find(|s| s.id == id)?.clone();
        Some(Arc::new(move |s: &mut Shot| {
            s.camera.station = Some(st.id.clone());
            s.camera.pos = None;
            s.camera.look_at = LookAt::Angles { yaw_deg: st.yaw_deg, pitch_deg: st.pitch_deg };
            if st.lens_mm > 0 && !is_locked("/camera/lens_mm", &s.locked) {
                s.camera.lens_mm = patterns::snap_lens(st.lens_mm as f32);
            }
            for p in POSITION {
                lock(s, p);
            }
            // The station sets the height too.
            s.locked.remove(HEIGHT);
        }))
    }

    fn on_map(&mut self, ev: &MapEvent, window: &mut Window, cx: &mut Context<Self>) {
        // A drag in progress changes nothing here: the map shows the item under the pointer
        // itself, and the drop (`done: true`) applies and saves the change once.
        if matches!(ev, MapEvent::Moved { done: false, .. } | MapEvent::Turned { done: false, .. }) {
            return;
        }
        let Some((scene_id, shot_id)) = self.current_ids() else { return };
        // Only what the edit needs is copied out, so applying it never copies the whole screen.
        let (place, pitch) = {
            let Some(d) = self.ready() else { return };
            let Some(scene) = d.scene(&scene_id) else { return };
            let place = d.place(scene).cloned();
            let Some(shot) = d.shots_of(&scene_id).iter().find(|s| s.id == shot_id) else { return };
            let (_, pitch) = geometry::look_dir(&shot.camera, &shot.marks, place.as_ref());
            (place, pitch)
        };
        let (f, done): (ShotFn, bool) = match ev.clone() {
            MapEvent::Selected(Some(MapItem::Mark(id))) => {
                self.drop_box(Text::Action, cx);
                self.character = Some(id);
                if is_narrow(window) {
                    self.drawer = true;
                }
                cx.notify();
                return;
            }
            MapEvent::Moved { item: MapItem::Camera, pos, done } => (
                Arc::new(move |s: &mut Shot| {
                    let h = geometry::cam_pos(&s.camera, place.as_ref())[1];
                    let at = [pos[0], h, pos[1]];
                    s.camera.pos = Some(at);
                    let (yaw, _) = geometry::look_dir(&s.camera, &s.marks, place.as_ref());
                    s.camera.station = place.as_ref().and_then(|pl| geometry::nearest_station(pl, at, yaw));
                    lock(s, "/camera/pos");
                    lock(s, "/camera/station");
                }),
                done,
            ),
            MapEvent::Turned { item: MapItem::Camera, yaw_deg, done } => (
                Arc::new(move |s: &mut Shot| {
                    s.camera.look_at = LookAt::Angles { yaw_deg, pitch_deg: pitch };
                    lock(s, "/camera/look_at");
                }),
                done,
            ),
            MapEvent::Moved { item: MapItem::Mark(id), pos, done } => {
                let p = mark_ptr(&id, "pos");
                (
                    Arc::new(move |s: &mut Shot| {
                        if let Some(m) = s.marks.iter_mut().find(|m| m.character == id) {
                            m.pos = pos;
                        }
                        lock(s, &p);
                    }),
                    done,
                )
            }
            MapEvent::Turned { item: MapItem::Mark(id), yaw_deg, done } => {
                let p = mark_ptr(&id, "facing_deg");
                (
                    Arc::new(move |s: &mut Shot| {
                        if let Some(m) = s.marks.iter_mut().find(|m| m.character == id) {
                            m.facing_deg = yaw_deg;
                        }
                        lock(s, &p);
                    }),
                    done,
                )
            }
            MapEvent::StationPicked(id) => match place.as_ref().and_then(|p| Self::station_fn(p, &id)) {
                Some(f) => (f, true),
                None => return,
            },
            MapEvent::DeleteRequested(MapItem::Mark(id)) => {
                let p = mark_ptr(&id, "visible");
                (
                    Arc::new(move |s: &mut Shot| {
                        if let Some(m) = s.marks.iter_mut().find(|m| m.character == id) {
                            m.visible = Visibility::Offscreen;
                        }
                        lock(s, &p);
                    }),
                    true,
                )
            }
            _ => return,
        };
        if !done || self.scene_busy(&scene_id).active() {
            return;
        }
        self.commit_all(cx);
        self.apply(&scene_id, on_shot(shot_id, f), cx);
    }

    fn ask(&mut self, ask: Ask, window: &mut Window, cx: &mut Context<Self>) {
        let Some((scene, shot)) = self.current_ids() else { return };
        // While the scene is being planned only looking around is allowed (the plan would
        // overwrite a change made now).
        let busy = self.scene_busy(&scene).active();
        if busy && !matches!(ask, Ask::Character(_) | Ask::Close) {
            return;
        }
        // Words typed in a box are saved before anything else changes.
        self.commit_all(cx);
        match ask {
            Ask::Edit { pointers, set } => self.apply(&scene, on_shot(shot, edit_fn(pointers, set)), cx),
            Ask::Character(id) => {
                self.drop_box(Text::Action, cx);
                self.character = Some(id.clone());
                self.map.update(cx, |m, cx| m.select(Some(MapItem::Mark(id)), cx));
                cx.notify();
            }
            Ask::AddCharacter(id) => {
                let place = {
                    let Some(d) = self.ready() else { return };
                    d.scene(&scene).and_then(|sc| d.place(sc)).cloned()
                };
                let who = id.clone();
                self.apply(
                    &scene,
                    on_shot(
                        shot,
                        Arc::new(move |s: &mut Shot| {
                            if s.marks.iter().any(|m| m.character == who) {
                                return;
                            }
                            let c = centroid(&s.marks);
                            let pos = clamp_plan(place.as_ref(), if s.marks.is_empty() { c } else { [c[0] + ADD_OFFSET_M, c[1]] }, 0.);
                            let cam = geometry::cam_pos(&s.camera, place.as_ref());
                            s.marks.push(Mark { character: who.clone(), pos, facing_deg: yaw_to(pos, [cam[0], cam[2]]), emotion: Emotion { kind: EmotionKind::Neutral, strength: 1 }, ..Mark::default() });
                            lock(s, &mark_ptr(&who, "pos"));
                        }),
                    ),
                    cx,
                );
                self.drop_box(Text::Action, cx);
                self.character = Some(id);
            }
            Ask::Station(id) => {
                let f = {
                    let Some(d) = self.ready() else { return };
                    d.scene(&scene).and_then(|sc| d.place(sc)).and_then(|p| Self::station_fn(p, &id))
                };
                let Some(f) = f else { return };
                self.apply(&scene, on_shot(shot, f), cx);
            }
            Ask::MoveLine { line, forward } => {
                self.apply(&scene, op(move |list| {
                    move_line(list, &line, forward);
                }), cx);
            }
            Ask::Close => {
                self.drawer = false;
                cx.notify();
            }
        }
        let _ = window;
    }

    // ------------------------------------------------------------ text boxes

    /// The inspector's text boxes for the selected shot and character, made when needed and kept
    /// in step with the data while they are not being typed in.
    fn ensure_inputs(&mut self, d: &Data, window: &mut Window, cx: &mut Context<Self>) {
        let scene = self.scene.clone().unwrap_or_default();
        let shot = self.shot.as_deref().and_then(|id| d.shots_of(&scene).iter().find(|s| s.id == id));
        let Some(shot) = shot else {
            self.drop_box(Text::Tells, cx);
            self.drop_box(Text::Action, cx);
            return;
        };
        let tells = shot.tells.clone();
        self.ensure_box(Text::Tells, &scene, shot.id.clone(), tells, "What this shot tells", window, cx);
        let who = self.character.clone().filter(|c| shot.marks.iter().any(|m| &m.character == c)).or_else(|| shot.marks.first().map(|m| m.character.clone()));
        match who.and_then(|c| shot.marks.iter().find(|m| m.character == c)) {
            Some(m) => {
                let key = format!("{}/{}", shot.id, m.character);
                let value = m.action.clone();
                self.ensure_box(Text::Action, &scene, key, value, "What they do", window, cx);
            }
            None => self.drop_box(Text::Action, cx),
        }
    }

    fn slot(&mut self, which: Text) -> &mut Option<TextBox> {
        match which {
            Text::Tells => &mut self.tells,
            Text::Action => &mut self.action,
        }
    }

    /// Takes a text box away, saving what was typed in it first: no path that replaces or
    /// closes a box can lose words that were not saved yet.
    fn drop_box(&mut self, which: Text, cx: &mut Context<Self>) {
        if let Some(b) = self.slot(which).take() {
            let text = b.text(cx);
            self.commit_text(which, &b.scene, &b.key, text, cx);
        }
    }

    fn ensure_box(&mut self, which: Text, scene: &str, key: String, value: String, placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(b) = self.slot(which).as_ref().filter(|b| b.key == key && b.scene == scene) {
            // Words typed and not saved yet are never replaced by the shot's (older) words; the
            // box is set only when the shot's words really changed since they were last shown.
            if b.typed.is_some() || b.shown == value {
                return;
            }
            let focused = b.input.read(cx).focus_handle(cx).contains_focused(window, cx);
            if !focused {
                let input = b.input.clone();
                let differs = input.read(cx).value().as_ref() != value.as_str();
                if differs {
                    let v = value.clone();
                    input.update(cx, |s, cx| show_from_start(s, v, window, cx));
                }
                if let Some(b) = self.slot(which).as_mut() {
                    b.shown = value;
                }
            }
            return;
        }
        // Another shot or character: the old box's words are saved before it goes.
        self.drop_box(which, cx);
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(TEXT_ROWS.0, TEXT_ROWS.1).submit_on_enter(true).placeholder(placeholder));
        let shown = value.clone();
        input.update(cx, |s, cx| show_from_start(s, value, window, cx));
        let (k, sc) = (key.clone(), scene.to_string());
        let sub = cx.subscribe_in(&input, window, move |this, input, ev: &InputEvent, _, cx| match ev {
            // Every key press keeps the latest words (`set_value` emits no change).
            InputEvent::Change => {
                let now = input.read(cx).value().to_string();
                if let Some(b) = this.slot(which).as_mut().filter(|b| b.key == k && b.scene == sc) {
                    b.typed = Some(now);
                }
            }
            InputEvent::PressEnter { shift: false, .. } | InputEvent::Blur => this.commit_box(which, cx),
            _ => {}
        });
        *self.slot(which) = Some(TextBox { scene: scene.to_string(), key, input, typed: None, shown, _sub: sub });
    }

    /// Saves one open box's words into its shot (when they changed) and forgets them as typed.
    fn commit_box(&mut self, which: Text, cx: &mut Context<Self>) {
        let open = self.slot(which).as_mut().map(|b| {
            let text = b.text(cx);
            b.typed = None;
            (b.scene.clone(), b.key.clone(), text)
        });
        if let Some((scene, key, text)) = open {
            self.commit_text(which, &scene, &key, text, cx);
        }
    }

    /// Saves a text box's words into its shot when they changed.
    fn commit_text(&mut self, which: Text, scene: &str, key: &str, text: String, cx: &mut Context<Self>) {
        let (shot_id, who) = match which {
            Text::Tells => (key.to_string(), None),
            Text::Action => match key.split_once('/') {
                Some((s, c)) => (s.to_string(), Some(c.to_string())),
                None => return,
            },
        };
        // What the shot holds now (copied out, so the edit below does not copy the whole screen).
        let now = {
            let Some(d) = self.ready() else { return };
            let Some(shot) = d.shots_of(scene).iter().find(|s| s.id == shot_id) else { return };
            match &who {
                None => Some(shot.tells.trim().to_string()),
                Some(c) => shot.marks.iter().find(|m| &m.character == c).map(|m| m.action.trim().to_string()),
            }
        };
        let scene = scene.to_string();
        match who {
            None => {
                if now.is_some_and(|t| t != text) {
                    self.apply(&scene, on_shot(shot_id, edit_fn(vec!["/tells".into()], Some(Arc::new(move |s: &mut Shot| s.tells = text.clone())))), cx);
                }
            }
            Some(c) => {
                if now.is_some_and(|a| a != text) {
                    let p = mark_ptr(&c, "action");
                    self.apply(
                        &scene,
                        on_shot(
                            shot_id,
                            edit_fn(
                                vec![p],
                                Some(Arc::new(move |s: &mut Shot| {
                                    if let Some(m) = s.marks.iter_mut().find(|m| m.character == c) {
                                        m.action = text.clone();
                                    }
                                })),
                            ),
                        ),
                        cx,
                    );
                }
            }
        }
    }

    /// Saves the open text boxes (before planning, leaving the shot, or any other change).
    fn commit_all(&mut self, cx: &mut Context<Self>) {
        for which in [Text::Tells, Text::Action] {
            self.commit_box(which, cx);
        }
    }
}

// ---------------------------------------------------------------- rendering

impl Render for ShotsStep {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.refresh) {
            if self.saving || !self.pending.is_empty() {
                self.stale = true;
            } else {
                self.reload(cx);
            }
        }
        let d = match &self.data {
            Load::Loading => return self.skeleton(window).into_any_element(),
            Load::Failed(e) => {
                let e = e.clone();
                return div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(error_view(e).retry(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.data = Load::Loading;
                        this.reload(cx);
                        cx.notify();
                    })))
                    .into_any_element();
            }
            Load::Ready(d) => d.clone(),
        };
        if d.story.scenes.is_empty() {
            return self.no_script().into_any_element();
        }
        // Only scenes still in the script count (a deleted or joined scene leaves its file).
        let planned_any = d.order.iter().any(|id| !d.shots_of(id).is_empty());
        // A failed attempt stays in view (each scene shows its reason and Try again).
        let failed = d.order.iter().any(|id| matches!(self.scene_busy(id), Busy::Failed { .. }));
        if !planned_any && !d.approved && self.running_jobs() + self.waiting_jobs() + self.paused_jobs() == 0 && !failed {
            return self.not_approved(&d, cx).into_any_element();
        }
        self.ensure_inputs(&d, window, cx);
        let narrow = is_narrow(window);
        let list = self.list(&d, cx);
        let center = self.center(&d, narrow, cx);
        let panel = if narrow && !self.drawer { None } else { self.inspector(&d, narrow, cx) };
        workspace(window, Some(list.into_any_element()), center, panel).into_any_element()
    }
}

/// The 40 px header of a column with a real title.
fn title_head(title: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(space::SM))
        .h(px(COLUMN_HEAD_H))
        .px(px(space::LG))
        .border_b_1()
        .border_color(Tokens::border())
        .child(div().min_w_0().truncate().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title.into()))
}

fn severity_icon(s: Severity) -> (Ph, Rgba) {
    match s {
        Severity::Error => (ph::WARNING_CIRCLE, Tokens::danger()),
        Severity::Warning => (ph::WARNING, Tokens::warning()),
        _ => (ph::INFO, Tokens::info()),
    }
}

impl ShotsStep {
    fn skeleton(&self, window: &Window) -> Div {
        let list = div().flex().flex_col().size_full().child(column_head("Scenes")).child(div().flex().flex_col().gap(px(space::XS)).p(px(space::MD)).children((0..6).map(|_| skeleton_row())));
        let center = div()
            .flex()
            .flex_col()
            .gap(px(space::LG))
            .size_full()
            .p(px(space::LG))
            .child(skeleton_line(px(240.)))
            .child(div().flex().gap(px(space::SM)).children((0..5).map(|_| skeleton_block(px(CARD_W), px(96.)))))
            .child(skeleton_block(relative(1.), px(MAP_H)));
        workspace(window, Some(list.into_any_element()), center, None)
    }

    /// No script yet: shots are planned from it.
    fn no_script(&self) -> Div {
        let go = self.ctx.go.clone();
        div().size_full().flex().items_center().justify_center().p(px(space::XXL)).child(
            empty_view(ph::VIDEO_CAMERA, "No script yet", "Shots are planned from the script: every scene becomes a list of cuts, each with its camera, lens and where everyone stands. Adapt the chapters in the Story step first.")
                .action(btn("sh-go-story", "Go to Story", ButtonKind::Primary).icon(ph::NOTE_PENCIL).on_click(move |_, w, cx| go(Step::Story, w, cx))),
        )
    }

    /// Nothing planned and the bible is not approved: say what is missing.
    fn not_approved(&self, d: &Data, cx: &mut Context<Self>) -> Div {
        let go = self.ctx.go.clone();
        let all = d.order.clone();
        let n = all.len();
        let left = match d.missing.len() {
            0 => "Approve it in the Bible step first.".to_string(),
            k => format!("{} still to do in the Bible step.", count(k, "thing")),
        };
        div().size_full().flex().items_center().justify_center().p(px(space::XXL)).child(
            empty_view(ph::VIDEO_CAMERA, "Approve the bible first", format!("Shots are staged on the places' floor plans with the approved cast. {left}"))
                .action(btn("sh-go-bible", "Go to Bible", ButtonKind::Primary).icon(ph::USERS).on_click(move |_, w, cx| go(Step::Bible, w, cx)))
                .secondary(
                    btn("sh-plan-anyway", format!("Plan {} anyway", count(n, "scene")), ButtonKind::Secondary)
                        .tooltip("Plans the scenes now. Shots may need planning again after the bible changes.")
                        .on_click(cx.listener(move |this, _: &ClickEvent, w, cx| this.confirm_plan(all.clone(), true, w, cx))),
                ),
        )
    }

    /// Plans `scenes`, asking first when that queues more than a handful of jobs.
    fn confirm_plan(&mut self, scenes: Vec<String>, unapproved: bool, window: &mut Window, cx: &mut Context<Self>) {
        let n = scenes.len();
        if n == 0 {
            return;
        }
        if !asks_first(n) {
            self.plan(scenes, cx);
            return;
        }
        let what = if unapproved {
            format!("The AI plans the shots of {} before the bible is approved, so some may need planning again once it is.", count(n, "scene"))
        } else {
            format!("The AI plans the shots of {}, one after another. Shots and fields you changed stay as they are.", count(n, "scene"))
        };
        let weak = cx.entity().downgrade();
        confirm_bulk(n, "plan_scene", format!("Plan {}?", count(n, "scene")), what, format!("Plan {}", count(n, "scene")), window, cx, move |_, cx| {
            let scenes = scenes.clone();
            let _ = weak.update(cx, |this, cx| this.plan(scenes, cx));
        });
    }

    // ------------------------------------------------------------ left: scenes

    fn list(&self, d: &Data, cx: &mut Context<Self>) -> Div {
        let head = column_head("Scenes").child(spec(count(d.order.len(), "scene")));
        let mut body = column_body("sh-scenes").gap(px(space::XXS));
        for id in &d.order {
            let Some(s) = d.scene(id) else { continue };
            let n = d.number(id);
            let selected = self.scene.as_deref() == Some(id.as_str());
            let shots = d.shots_of(id);
            let busy = self.scene_busy(id);
            let issues = d.issues.get(id).map(Vec::as_slice).unwrap_or(&[]);
            let errors = issues.iter().filter(|i| i.severity == Severity::Error).count();
            let warnings = issues.iter().filter(|i| i.severity == Severity::Warning).count();
            let stale = d.outdated(s).is_some();
            let total: f32 = shots.iter().map(shot_secs).sum();
            let place = d.place(s).map(|p| p.name.trim().to_string()).filter(|n| !n.is_empty());
            let meta = if shots.is_empty() { "Not planned yet".to_string() } else { format!("{} · {}", count(shots.len(), "shot"), fmt_secs(total)) };
            let meta = match place {
                Some(p) => format!("{p} · {meta}"),
                None => meta,
            };
            let mark = match &busy {
                Busy::Running { .. } => spinner(eid("sh-row-spin", id, 0), Tokens::accent(), icon_px::XS).into_any_element(),
                Busy::Waiting { .. } => icon_ph(ph::HOURGLASS, Tokens::text_3(), icon_px::XS).into_any_element(),
                Busy::Failed { .. } => icon_ph(ph::WARNING_CIRCLE, Tokens::danger(), icon_px::XS).into_any_element(),
                Busy::Idle if shots.is_empty() => icon_ph(ph::CIRCLE_DASHED, Tokens::text_3(), icon_px::XS).into_any_element(),
                Busy::Idle if stale => icon_ph(ph::ARROWS_CLOCKWISE, Tokens::warning(), icon_px::XS).into_any_element(),
                Busy::Idle if errors > 0 => icon_ph(ph::WARNING_CIRCLE, Tokens::danger(), icon_px::XS).into_any_element(),
                Busy::Idle if warnings > 0 => icon_ph(ph::WARNING, Tokens::warning(), icon_px::XS).into_any_element(),
                Busy::Idle => icon_ph(ph::CHECK, Tokens::success(), icon_px::XS).into_any_element(),
            };
            let tooltip = match &busy {
                Busy::Running { .. } => "Planning".to_string(),
                Busy::Waiting { paused: true, .. } => "Paused in the queue".to_string(),
                Busy::Waiting { .. } => "Waiting in the queue".to_string(),
                Busy::Failed { .. } => "Planning failed".to_string(),
                Busy::Idle if shots.is_empty() => "Not planned yet".to_string(),
                Busy::Idle if stale => "Outdated: the script changed after planning".to_string(),
                Busy::Idle if errors + warnings > 0 => format!("{} · {}", count(errors, "error"), count(warnings, "warning")),
                Busy::Idle => "Planned, no problems".to_string(),
            };
            let pick = id.clone();
            body = body.child(
                list_item(eid("sh-scene", id, 0), selected)
                    .child(div().flex_none().w(px(22.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(n.to_string()))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(if s.title.trim().is_empty() { format!("Scene {n}") } else { s.title.trim().to_string() }))
                            .child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(meta)),
                    )
                    .child(div().id(eid("sh-row-mark", id, 0)).flex_none().tooltip(tip(tooltip)).child(mark))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.commit_all(cx);
                        this.select_scene(pick.clone(), cx);
                    })),
            );
        }
        div().flex().flex_col().size_full().child(head).child(body).child(self.footer(d, cx))
    }

    fn footer(&self, d: &Data, cx: &mut Context<Self>) -> Div {
        // Over the scenes in the script only: a deleted or joined scene's shots file stays behind.
        let shots: usize = d.order.iter().map(|id| d.shots_of(id).len()).sum();
        let total: f32 = d.order.iter().flat_map(|id| d.shots_of(id)).map(shot_secs).sum();
        let planned = d.order.iter().filter(|id| !d.shots_of(id).is_empty()).count();
        let line = format!("{} of {} planned · {} · {}", planned, count(d.order.len(), "scene"), count(shots, "shot"), fmt_secs(total));
        let (running, waiting, paused) = (self.running_jobs(), self.waiting_jobs(), self.paused_jobs());
        let (todo, again) = self.to_plan(d);
        // Only a plan that really runs turns a spinner; one waiting or paused draws nothing that moves.
        let button = if running > 0 {
            btn("sh-plan-all", format!("Planning {}…", count(running + waiting, "scene")), ButtonKind::Primary).full_width().loading(true)
        } else if waiting > 0 {
            btn("sh-plan-all", format!("{} waiting in the queue", count(waiting, "scene")), ButtonKind::Primary).full_width().icon(ph::HOURGLASS).disabled("They start when the jobs before them are done")
        } else if todo.is_empty() && paused > 0 {
            btn("sh-plan-all", format!("{} paused in the queue", count(paused, "scene")), ButtonKind::Primary).full_width().icon(ph::PAUSE_CIRCLE).disabled("Resume them in the queue below, or cancel them there")
        } else if todo.is_empty() {
            let go = self.ctx.go.clone();
            btn("sh-plan-all", "Continue to Frames", ButtonKind::Primary).icon(ph::IMAGE).full_width().on_click(move |_, w, cx| go(Step::Frames, w, cx))
        } else {
            let label = match (again, planned, todo.len()) {
                (true, _, 1) => "Plan the outdated scene again".to_string(),
                (true, _, n) => format!("Plan {n} outdated scenes again"),
                (false, 0, _) => "Plan all shots".to_string(),
                (false, _, 1) => "Plan the last scene".to_string(),
                (false, _, n) => format!("Plan the other {n} scenes"),
            };
            btn("sh-plan-all", label, ButtonKind::Primary)
                .icon(ph::SPARKLE)
                .full_width()
                .tooltip("The AI breaks each scene into shots and stages them on the place's floor plan. Shots you changed stay as they are.")
                .on_click(cx.listener(move |this, _: &ClickEvent, w, cx| this.confirm_plan(todo.clone(), false, w, cx)))
        };
        div()
            .flex()
            .flex_none()
            .flex_col()
            .gap(px(space::SM))
            .p(px(space::MD))
            .border_t_1()
            .border_color(Tokens::border())
            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(line))
            .child(button)
    }

    // ------------------------------------------------------------ centre: the scene

    fn center(&self, d: &Data, narrow: bool, cx: &mut Context<Self>) -> Div {
        let Some(scene) = self.scene.as_deref().and_then(|s| d.scene(s)) else {
            return div().size_full().flex().items_center().justify_center().child(muted("Pick a scene on the left."));
        };
        let n = d.number(&scene.id);
        let shots = d.shots_of(&scene.id);
        let busy = self.scene_busy(&scene.id);
        let issues = d.issues.get(&scene.id).map(Vec::as_slice).unwrap_or(&[]);
        let errors = issues.iter().filter(|i| i.severity == Severity::Error).count();
        let warnings = issues.iter().filter(|i| i.severity == Severity::Warning).count();
        let stale = d.outdated(scene);
        let total: f32 = shots.iter().map(shot_secs).sum();
        let title = if scene.title.trim().is_empty() { format!("Scene {n}") } else { format!("Scene {n} · {}", scene.title.trim()) };
        let plan = {
            let label = if shots.is_empty() { "Plan this scene" } else { "Plan again" };
            let b = btn("sh-plan-scene", label, ButtonKind::Secondary).small().icon(ph::SPARKLE);
            if matches!(busy, Busy::Running { .. }) {
                b.loading(true)
            } else if let Busy::Waiting { paused, .. } = busy {
                b.disabled(if paused { "Paused in the queue" } else { "Waiting in the queue" })
            } else {
                let id = scene.id.clone();
                b.tooltip(if shots.is_empty() { "The AI breaks this scene into shots." } else { "Plans this scene again. Shots and fields you changed stay as they are." }).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.plan(vec![id.clone()], cx)))
            }
        };
        let head = title_head(title)
            .when(!shots.is_empty(), |h| h.child(spec(count(shots.len(), "shot"))).child(spec(fmt_secs(total))))
            .when(errors > 0, |h| h.child(icon_tag(ph::WARNING_CIRCLE, count(errors, "error"), Tone::Danger)))
            .when(warnings > 0, |h| h.child(icon_tag(ph::WARNING, count(warnings, "warning"), Tone::Warning)))
            .when(stale.is_some(), |h| h.child(outdated_tag()))
            .child(div().flex_1())
            .child(plan)
            .when(narrow && self.shot.is_some(), |h| {
                h.child(icon_button("sh-drawer", ph::SLIDERS_HORIZONTAL, "Shot settings").small().selected(self.drawer).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.drawer = !this.drawer;
                    cx.notify();
                })))
            });

        let mut body = div().id("sh-body").flex().flex_col().flex_1().min_h_0().gap(px(space::LG)).p(px(space::LG)).overflow_y_scroll();
        // What needs saying first.
        if !d.approved {
            body = body.child(bible_notice("sh-bible", d.missing.len(), "places and cast", self.ctx.go.clone()));
        }
        if d.place(scene).is_none() {
            body = body.child(self.no_place(d, scene, cx));
        }
        body = body.children(self.progress(&scene.id, &busy));
        if let Busy::Failed { id, error } = &busy {
            body = body.child(failure(*id, error.clone()));
        }
        if let Some(why) = &stale {
            body = body.child(info_line(ph::ARROWS_CLOCKWISE, LineTone::Warning, format!("{why} Plan the scene again to catch up; what you changed stays.")));
        }
        body = body.child(self.strip(d, scene, busy.active(), cx));
        if self.shot.as_ref().is_some_and(|id| shots.iter().any(|s| &s.id == id)) {
            body = body.child(self.map_view(d, scene, busy.active()));
        }
        if !shots.is_empty() {
            body = body.child(self.issues(d, scene, issues, busy.active(), cx));
        }
        div().flex().flex_col().size_full().child(head).child(body)
    }

    /// The scene has no place: pick one (it is the script's choice, saved as the user's).
    fn no_place(&self, d: &Data, scene: &Scene, cx: &mut Context<Self>) -> Div {
        let mut out = div().flex().flex_col().gap(px(space::SM)).child(info_line(ph::COMPASS, LineTone::Warning, "This scene has no place yet, so the camera map has no floor plan. Pick one, or add it in the Bible."));
        if d.bible.places.is_empty() {
            let go = self.ctx.go.clone();
            return out.child(div().child(btn("sh-place-bible", "Go to Bible", ButtonKind::Secondary).small().icon(ph::USERS).on_click(move |_, w, cx| go(Step::Bible, w, cx))));
        }
        let mut chips = div().flex().flex_wrap().gap(px(space::XS));
        for p in &d.bible.places {
            let (scene_id, place_id) = (scene.id.clone(), p.id.clone());
            let name = if p.name.trim().is_empty() { p.id.clone() } else { p.name.trim().to_string() };
            chips = chips.child(filter_chip(eid("sh-place", &p.id, 0), name, false).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.pick_place(scene_id.clone(), place_id.clone(), cx))));
        }
        out = out.child(chips);
        out
    }

    fn pick_place(&mut self, scene: String, place: String, cx: &mut Context<Self>) {
        let (store, ep) = (self.store.clone(), self.ctx.episode.clone());
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || set_place(&store, &ep, &scene, &place)).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if let Err(e) = res {
                    mv_core::logging::global().warn("anime", &format!("shots step: place not saved: {e:#}"));
                    toast::show(cx, Toast::error("The place could not be saved. Check that the project's drive is connected, then try again."));
                }
                this.reload(cx);
            });
        })
        .detach();
    }

    /// Progress of a waiting or running planning job, with Cancel (and Try now while the AI waits).
    fn progress(&self, key: &str, busy: &Busy) -> Option<Div> {
        match busy {
            Busy::Running { id, progress, line } => {
                let id = *id;
                let waiting = waits_for_limit(line);
                let line = if line.trim().is_empty() { "Planning the shots…".to_string() } else { line.clone() };
                Some(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(space::XS))
                        .child(progress_bar(eid("sh-progress", key, 0), *progress, !waiting))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::SM))
                                .child(div().flex_1().min_w_0().text_size(px(size::XS)).text_color(if waiting { Tokens::warning() } else { Tokens::text_2() }).whitespace_normal().child(line))
                                .when(waiting, |d| d.child(btn(eid("sh-now", key, id), "Try now", ButtonKind::Secondary).small().on_click(|_, _, _| mv_anime::llm::retry_now())))
                                .child(btn(eid("sh-cancel", key, id), "Cancel", ButtonKind::Ghost).small().on_click(move |_, _, _| queue::cancel(id))),
                        )
                        .child(muted("The shots, the map and the settings are read-only until planning finishes. Fields you set stay as they are.")),
                )
            }
            Busy::Waiting { id, paused } => {
                let (id, paused) = (*id, *paused);
                let row = div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .child(icon_ph(if paused { ph::PAUSE_CIRCLE } else { ph::HOURGLASS }, Tokens::text_2(), icon_px::SM))
                    // The one place to resume is the queue strip at the bottom of the tab.
                    .child(div().flex_1().text_size(px(size::XS)).text_color(Tokens::text_2()).child(if paused { "Waiting: the queue is paused. Resume it in the queue strip below." } else { "Waiting in the queue to be planned" }))
                    .child(btn(eid("sh-cancel", key, id), "Cancel", ButtonKind::Ghost).small().on_click(move |_, _, _| queue::cancel(id)));
                Some(div().flex().flex_col().gap(px(space::XS)).child(row).child(muted("The shots, the map and the settings are read-only until this scene is planned, or cancel it to change them now.")))
            }
            _ => None,
        }
    }

    // ------------------------------------------------------------ the shot strip

    fn strip(&self, d: &Data, scene: &Scene, busy: bool, cx: &mut Context<Self>) -> Div {
        let shots = d.shots_of(&scene.id);
        let n = d.number(&scene.id);
        let mut patterns_menu = Vec::new();
        for def in patterns::defs() {
            let weak = cx.entity().downgrade();
            let pattern = def.pattern;
            let mut name = def.name.replace('_', " ");
            if let Some(c) = name.get(..1).map(str::to_uppercase) {
                name.replace_range(..1, &c);
            }
            patterns_menu.push(MenuItem::new(format!("{name}: {}", def.when), move |_, cx| {
                let _ = weak.update(cx, |this, cx| this.add_pattern(pattern, cx));
            }));
        }
        let add = btn("sh-add", "Add a shot", ButtonKind::Ghost).small().icon(ph::PLUS);
        let add = if busy { add.disabled(self.scene_busy(&scene.id).note()) } else { add.tooltip("A medium shot after the selected one, with the same people").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.add_shot(cx))) };
        let head = div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .child(label_caps("Shots"))
            .child(div().flex_1())
            .child(add)
            .when(!busy, |h| h.child(menu_button("sh-pattern", ph::SQUARES_FOUR, "Add a ready-made run of shots after the selected one", patterns_menu).label("Add a pattern").small().min_width(320.)));
        if shots.is_empty() {
            return div().flex().flex_col().gap(px(space::SM)).child(head).child(empty_gallery(
                ph::VIDEO_CAMERA,
                "No shots yet",
                "Plan this scene and the AI breaks it into shots, each with its camera, lens and where everyone stands. Or add shots yourself.",
            ));
        }
        let issues = d.issues.get(&scene.id).map(Vec::as_slice).unwrap_or(&[]);
        let last = shots.len() - 1;
        let mut row = div().id("sh-strip").flex().gap(px(space::SM)).pb(px(space::XS)).overflow_x_scroll().track_scroll(&self.strip);
        for (i, s) in shots.iter().enumerate() {
            let number = format!("{n}.{}", i + 1);
            let selected = self.shot.as_deref() == Some(s.id.as_str());
            let worst = issues.iter().filter(|x| x.shot.as_deref() == Some(s.id.as_str())).map(|x| x.severity).min_by_key(|sev| match sev {
                Severity::Error => 0,
                Severity::Warning => 1,
                _ => 2,
            });
            let group = SharedString::from(format!("sh-card-{}", s.id));
            let seen: Vec<&Mark> = s.marks.iter().filter(|m| m.visible != Visibility::Offscreen).collect();
            let mut faces = div().flex().items_center().gap(px(space::XXS)).h(px(20.));
            if seen.is_empty() {
                faces = faces.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("Nobody"));
            }
            for m in seen.iter().take(CARD_FACES) {
                faces = faces.child(marker(d.hue(&m.character), &d.name(&m.character), 18.));
            }
            if seen.len() > CARD_FACES {
                faces = faces.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("+{}", seen.len() - CARD_FACES)));
            }
            let pictured = s.frame.selected.is_some();
            let top = div()
                .flex()
                .items_center()
                .gap(px(space::XS))
                .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(number.clone()))
                .child(div().flex_1())
                .when_some(worst, |d, sev| {
                    let (icon, color) = severity_icon(sev);
                    d.child(icon_ph(icon, color, icon_px::XS))
                })
                .when(pictured, |d| d.child(icon_ph(ph::IMAGE, Tokens::text_3(), icon_px::XS)))
                .when(!s.locked.is_empty(), |d| d.child(div().size(px(6.)).rounded_full().bg(Tokens::accent())));
            let id = s.id.clone();
            let actions = hover_actions(group.clone())
                .child({
                    let id = id.clone();
                    icon_button(eid("sh-left", &s.id, 0), ph::ARROW_LEFT, "Move earlier").small().when(i == 0, |b| b.disabled("This is the first shot")).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.move_shot(id.clone(), false, cx)))
                })
                .child({
                    let id = id.clone();
                    icon_button(eid("sh-right", &s.id, 0), ph::ARROW_RIGHT, "Move later").small().when(i == last, |b| b.disabled("This is the last shot")).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.move_shot(id.clone(), true, cx)))
                })
                .child({
                    let id = id.clone();
                    icon_button(eid("sh-copy", &s.id, 0), ph::COPY, "Duplicate").small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.duplicate(id.clone(), cx)))
                })
                .child({
                    let (id, number) = (id.clone(), number.clone());
                    icon_button(eid("sh-delete", &s.id, 0), ph::TRASH, "Delete this shot").small().on_click(cx.listener(move |this, _: &ClickEvent, w, cx| this.confirm_delete(id.clone(), number.clone(), w, cx)))
                });
            let mv = if s.camera.movement.kind == Move::Hold || s.camera.movement.kind == Move::Unknown { "Still".to_string() } else { move_word(s.camera.movement.kind).to_string() };
            row = row.child(
                selectable(eid("sh-card", &s.id, 0), selected)
                    .relative()
                    .group(group)
                    .flex_none()
                    .w(px(CARD_W))
                    .gap(px(space::XS))
                    .p(px(space::SM))
                    .child(top)
                    .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(size_word(s.camera.size)))
                    .child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{mv} · {:.1} s · {} mm", shot_secs(s), s.camera.lens_mm)))
                    .child(faces)
                    // While planning, a card can be looked at but not moved, copied or deleted.
                    .when(!busy, |c| c.child(actions))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select_shot(id.clone(), cx))),
            );
        }
        div().flex().flex_col().gap(px(space::SM)).child(head).child(row).when(busy, |c| c.child(muted(self.scene_busy(&scene.id).note())))
    }

    // ------------------------------------------------------------ the camera map

    fn map_view(&self, d: &Data, scene: &Scene, busy: bool) -> Div {
        let place = d.place(scene);
        let right = match place {
            Some(p) if p.plan.w_m > 0. && p.plan.d_m > 0. => format!("{} · {:.0} × {:.0} m", p.name.trim(), p.plan.w_m, p.plan.d_m),
            Some(p) => format!("{} · no floor plan yet", p.name.trim()),
            None => "No place".to_string(),
        };
        let help = if busy { "Read-only while the scene is being planned.".to_string() } else { format!("{HINT} Click a camera position to put the camera there.") };
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .child(div().flex().items_center().gap(px(space::SM)).child(label_caps("Camera map")).child(div().flex_1()).child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(right)))
            .child(div().h(px(MAP_H)).rounded(px(radius::LG)).overflow_hidden().child(self.map.clone()))
            .child(muted(help))
    }

    // ------------------------------------------------------------ issues

    fn issues(&self, d: &Data, scene: &Scene, issues: &[Issue], busy: bool, cx: &mut Context<Self>) -> Div {
        let shots = d.shots_of(&scene.id);
        let n = d.number(&scene.id);
        let mut out = div().flex().flex_col().gap(px(space::SM)).child(div().flex().items_center().gap(px(space::SM)).child(label_caps("Issues")).child(div().flex_1()).child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(if issues.is_empty() { String::new() } else { issues.len().to_string() })));
        if issues.is_empty() {
            return out.child(info_line(ph::CHECK_CIRCLE, LineTone::Success, "No problems found: the cuts follow the film-grammar checks."));
        }
        let mut replan = false;
        for (k, i) in issues.iter().enumerate() {
            let (icon, color) = severity_icon(i.severity);
            let fix = fix_for(i, shots);
            replan |= fix == Some(Fix::Replan);
            let at = i.shot.as_ref().and_then(|id| shots.iter().position(|s| &s.id == id));
            let mut side = div().flex().flex_none().flex_col().items_end().gap(px(space::XS));
            if let Some(ix) = at {
                let id = shots[ix].id.clone();
                side = side.child(btn(("sh-issue-shot", k), format!("Shot {n}.{}", ix + 1), ButtonKind::Ghost).small().tooltip("Select this shot").on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select_shot(id.clone(), cx))));
            }
            match fix {
                Some(Fix::Replan) | None => {}
                Some(Fix::Story) => {
                    let go = self.ctx.go.clone();
                    side = side.child(btn(("sh-fix", k), Fix::Story.label(), ButtonKind::Secondary).small().icon(ph::NOTE_PENCIL).on_click(move |_, w, cx| go(Step::Story, w, cx)));
                }
                Some(f) if busy => {
                    side = side.child(btn(("sh-fix", k), f.label(), ButtonKind::Secondary).small().icon(ph::MAGIC_WAND).disabled(self.scene_busy(&scene.id).note()));
                }
                Some(f) => {
                    side = side.child(btn(("sh-fix", k), f.label(), ButtonKind::Secondary).small().icon(ph::MAGIC_WAND).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.fix(f.clone(), cx))));
                }
            }
            let suggested = i.fix.as_deref().map(str::trim).filter(|f| !f.is_empty()).map(str::to_string);
            out = out.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(space::SM))
                    .p(px(space::MD))
                    .rounded(px(radius::LG))
                    .bg(Tokens::surface())
                    .border_1()
                    .border_color(Tokens::border())
                    .child(div().pt(px(2.)).child(icon_ph(icon, color, icon_px::SM)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(space::XXS))
                            .child(div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_1()).whitespace_normal().child(i.message.trim().to_string()))
                            .when_some(suggested, |d, f| d.child(div().text_size(px(size::XS)).line_height(px(16.)).text_color(Tokens::text_2()).whitespace_normal().child(f))),
                    )
                    .child(side),
            );
        }
        if replan {
            out = out.child(muted("The rest are fixed by planning the scene again: the planner runs these checks and revises its plan, keeping what you changed."));
        }
        out
    }

    // ------------------------------------------------------------ right: the inspector

    fn inspector(&self, d: &Data, narrow: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let scene = self.scene.as_deref().and_then(|s| d.scene(s))?;
        let shots = d.shots_of(&scene.id);
        let at = self.shot.as_deref().and_then(|id| shots.iter().position(|s| s.id == id))?;
        let shot = &shots[at];
        let lines = d.lines(scene);
        let cast = d.cast(scene);
        let number = format!("{}.{}", d.number(&scene.id), at + 1);
        let state = self.scene_busy(&scene.id);
        let busy = state.active();
        let weak = cx.entity().downgrade();
        let on: inspector::OnAsk = Rc::new(move |ask, window, cx| {
            let _ = weak.update(cx, |this, cx| this.ask(ask, window, cx));
        });
        let view = Inspect {
            shot,
            number: &number,
            place: d.place(scene),
            bible: &d.bible,
            lines: &lines,
            cast: &cast,
            character: self.character.as_deref(),
            // While planning, the words show as plain text (no box to type into).
            tells: self.tells.as_ref().filter(|_| !busy).map(|b| &b.input),
            action: self.action.as_ref().filter(|_| !busy).map(|b| &b.input),
            first: at == 0,
            last: at + 1 == shots.len(),
            narrow,
            busy,
            busy_note: state.note(),
        };
        Some(inspector::render(view, on).into_any_element())
    }
}

fn failure(id: u64, error: String) -> Div {
    // Only a reason a person can read is shown; the full error is in the queue's log.
    let text = match plain_reason(&error) {
        Some(why) => format!("Planning this scene stopped: {why} Try again."),
        None => "Planning this scene stopped. Try again; if it stops again, check the scene in the Story step.".to_string(),
    };
    error_block(text).child(
        div()
            .flex()
            .flex_none()
            .gap(px(space::XS))
            .child(btn(eid("sh-retry", "", id), "Try again", ButtonKind::Secondary).small().icon(ph::ARROW_CLOCKWISE).on_click(move |_, _, cx| {
                crate::backends::ensure_runner(cx);
                queue::retry(id);
            }))
            .child(icon_button(eid("sh-dismiss", "", id), ph::X, "Dismiss").small().on_click(move |_, _, _| queue::cancel(id))),
    )
}

#[cfg(test)]
mod tests {
    // Explicit imports: with test-support, `gpui_kit::*` carries a `test` macro that shadows `#[test]`.
    use super::{cover_shot, fix_for, id_number, insert_after, insert_before, job_busy, move_line, move_shot, next_id, outdated, pull_back, remove_shot, restore_shot, scene_order, split_shot, step_size, yaw_to, Busy, Cover, Fix};
    use mv_anime::queue::{Job, JobKind, JobState, Queue};
    use mv_anime::{Camera, FloorPlan, Issue, Line, Mark, Place, Scene, Severity, Shot, ShotSize, Station};
    use std::collections::BTreeSet;

    fn shot(id: &str, lines: &[&str]) -> Shot {
        Shot { id: id.into(), lines: lines.iter().map(|l| l.to_string()).collect(), planned_s: 3., ..Shot::default() }
    }

    fn ids(shots: &[Shot]) -> Vec<&str> {
        shots.iter().map(|s| s.id.as_str()).collect()
    }

    fn line(id: &str, text: &str) -> Line {
        Line { id: id.into(), scene: "sc1".into(), spoken: text.into(), ..Line::default() }
    }

    #[test]
    fn ids_count_up_past_the_taken_ones() {
        let taken: BTreeSet<String> = ["sh0002".to_string()].into_iter().collect();
        let mut n = 1;
        assert_eq!(next_id(&taken, &mut n), "sh0003");
        assert_eq!(n, 3);
        assert_eq!(id_number("sh0042"), Some(42));
        assert_eq!(id_number("x1"), None);
    }

    #[test]
    fn shots_move_insert_and_go() {
        let mut s = vec![shot("a", &[]), shot("b", &[]), shot("c", &[])];
        assert!(move_shot(&mut s, "a", true));
        assert_eq!(ids(&s), ["b", "a", "c"]);
        assert!(!move_shot(&mut s, "b", false));
        assert!(!move_shot(&mut s, "c", true));
        insert_before(&mut s, Some("a"), &[shot("x", &[])]);
        assert_eq!(ids(&s), ["b", "x", "a", "c"]);
        insert_before(&mut s, Some("a"), &[shot("x", &[])]);
        assert_eq!(s.len(), 4, "the same shot is never added twice");
        insert_after(&mut s, Some("c"), &[shot("y", &[])]);
        insert_after(&mut s, Some("b"), &[shot("z", &[])]);
        assert_eq!(ids(&s), ["b", "z", "x", "a", "c", "y"]);
        assert!(remove_shot(&mut s, "x"));
        assert!(!remove_shot(&mut s, "x"));
    }

    #[test]
    fn undo_puts_a_deleted_shot_back_in_its_place() {
        let gone = shot("b", &["l2"]);
        // Its line went to the next shot when it was deleted.
        let mut s = vec![shot("a", &["l1"]), shot("c", &["l2", "l3"])];
        restore_shot(&mut s, &gone, Some("c"), Some("a"));
        assert_eq!(ids(&s), ["a", "b", "c"]);
        assert_eq!(s[1].lines, ["l2"]);
        assert_eq!(s[2].lines, ["l3"]);
        restore_shot(&mut s, &gone, Some("c"), Some("a"));
        assert_eq!(s.len(), 3, "never twice");
        // The shot after it is gone too: it goes after the one before it.
        let mut t = vec![shot("a", &["l1"]), shot("x", &[])];
        restore_shot(&mut t, &gone, Some("c"), Some("a"));
        assert_eq!(ids(&t), ["a", "b", "x"]);
    }

    #[test]
    fn undo_puts_the_whole_scene_back() {
        let was = vec![shot("a", &["l1"]), shot("b", &["l2"]), shot("c", &["l3"])];
        let gone = was[1].clone();
        // After the delete the next shot took the line and got longer; a shot was added since.
        let mut c = shot("c", &["l2", "l3"]);
        c.planned_s = 5.8;
        let mut s = vec![shot("a", &["l1"]), c, shot("n", &[])];
        super::undo_delete(&mut s, &gone, Some("c"), Some("a"), &was);
        assert_eq!(ids(&s), ["a", "b", "c", "n"]);
        assert_eq!(s[2].lines, ["l3"]);
        assert_eq!(s[2].planned_s, was[2].planned_s, "the neighbour has its own length again");
    }

    #[test]
    fn a_line_moves_to_the_next_shot_and_both_become_the_users() {
        let mut s = vec![shot("a", &["l1", "l2"]), shot("b", &["l3"])];
        assert!(move_line(&mut s, "l2", true));
        assert_eq!(s[0].lines, ["l1"]);
        assert_eq!(s[1].lines, ["l2", "l3"]);
        assert!(s[0].locked.contains("/lines") && s[1].locked.contains("/lines"));
        assert!(!move_line(&mut s, "l1", false));
    }

    #[test]
    fn a_split_shares_the_lines_and_changes_the_size() {
        let mut s = vec![Shot { camera: Camera { size: ShotSize::Ms, ..Camera::default() }, ..shot("a", &["l1", "l2", "l3"]) }];
        assert!(split_shot(&mut s, "a", "b"));
        assert_eq!(ids(&s), ["a", "b"]);
        assert_eq!(s[0].lines, ["l1", "l2"]);
        assert_eq!(s[1].lines, ["l3"]);
        assert_eq!(s[1].camera.size, ShotSize::Mcu);
        assert!(s[1].frame.takes.is_empty());
        assert!(!split_shot(&mut s, "b", "c"), "one line cannot be split");
    }

    #[test]
    fn sizes_step_along_the_ladder() {
        assert_eq!(step_size(ShotSize::Cu, -2), Some(ShotSize::Ms));
        assert_eq!(step_size(ShotSize::Els, -2), None);
        assert_eq!(step_size(ShotSize::Insert, 1), None);
    }

    #[test]
    fn plan_geometry() {
        assert_eq!(pull_back([0., 5.], [0., 0.], 2.), [0., 2.]);
        assert_eq!(pull_back([0., 0.], [0., 0.], 3.), [0., 3.]);
        assert!((yaw_to([0., 0.], [1., 0.]) - 90.).abs() < 1e-3);
        assert!((yaw_to([0., 0.], [0., -1.])).abs() < 1e-3);
        assert!((yaw_to([0., 0.], [0., 1.]) - 180.).abs() < 1e-3);
    }

    #[test]
    fn a_wide_stays_on_the_same_side_and_a_bridge_uses_the_overhead_station() {
        let scene = Scene { id: "sc1".into(), ..Scene::default() };
        let base = Shot {
            camera: Camera { pos: Some([0., 1.5, 2.]), ..Camera::default() },
            marks: vec![Mark { character: "a".into(), pos: [-1., 0.], ..Mark::default() }, Mark { character: "b".into(), pos: [1., 0.], ..Mark::default() }],
            ..shot("x", &[])
        };
        let place = Place {
            plan: FloorPlan { w_m: 30., d_m: 30., ..FloorPlan::default() },
            stations: vec![Station { id: "top".into(), overhead: true, pos: [0., 9., 0.], pitch_deg: -90., lens_mm: 24, ..Station::default() }],
            ..Place::default()
        };
        let wide = cover_shot(Cover::Wide, &scene, Some(&base), &[], Some(&place));
        assert_eq!(wide.camera.size, ShotSize::Ls);
        let p = wide.camera.pos.unwrap_or_default();
        assert!(p[2] > 2., "pulled back south, the side the base camera is on");
        assert!(wide.edited && wide.marks.len() == 2);
        let bridge = cover_shot(Cover::Overhead, &scene, Some(&base), &[], Some(&place));
        assert!(bridge.bridge);
        assert_eq!(bridge.camera.station.as_deref(), Some("top"));
        assert_eq!(bridge.camera.lens_mm, 24);
    }

    #[test]
    fn issues_get_the_fix_that_matches() {
        let shots = vec![shot("a", &["l1", "l2"]), shot("b", &[]), shot("c", &[])];
        let issue = |code: &str, shot: Option<&str>| Issue { code: code.into(), severity: Severity::Warning, scene: "sc1".into(), shot: shot.map(str::to_string), message: String::new(), fix: None };
        assert_eq!(fix_for(&issue("axis", Some("b")), &shots), Some(Fix::Cover { cover: Cover::Overhead, before: Some("b".into()) }));
        assert_eq!(fix_for(&issue("layout-early", Some("a")), &shots), Some(Fix::Cover { cover: Cover::Wide, before: Some("a".into()) }));
        assert_eq!(fix_for(&issue("lines-fit", Some("a")), &shots), Some(Fix::Split("a".into())));
        assert_eq!(fix_for(&issue("lines-fit", Some("b")), &shots), Some(Fix::Replan));
        assert_eq!(fix_for(&issue("close-run", Some("c")), &shots), Some(Fix::Size { shot: "c".into(), size: ShotSize::Ms }));
        assert_eq!(fix_for(&issue("pace", None), &shots), Some(Fix::Replan));
        assert_eq!(fix_for(&issue("plan-kept", Some("a")), &shots), None);
        // A run of non-layout cuts from cut 1: the wide goes in its middle.
        assert_eq!(fix_for(&issue("layout-return", Some("a")), &shots), Some(Fix::Cover { cover: Cover::Wide, before: Some("b".into()) }));
    }

    #[test]
    fn a_changed_script_makes_the_scene_outdated() {
        let lines = vec![line("l1", "hello"), line("l2", "there")];
        let shots = vec![shot("a", &["l1", "l2"])];
        assert_eq!(outdated(&lines, &shots, None, mv_anime::SceneKind::Dialogue), None);
        assert_eq!(outdated(&lines, &[], None, mv_anime::SceneKind::Dialogue), None);
        let more = vec![line("l1", "hello"), line("l2", "there"), line("l3", "again")];
        assert!(outdated(&more, &shots, None, mv_anime::SceneKind::Dialogue).is_some_and(|w| w.contains("1 line of the script is in no shot")));
        let long = vec![line("l1", &"word ".repeat(40)), line("l2", "there")];
        assert!(outdated(&long, &shots, None, mv_anime::SceneKind::Dialogue).is_some_and(|w| w.contains("longer")));
        let on_station = vec![Shot { camera: Camera { station: Some("gone".into()), ..Camera::default() }, ..shot("a", &["l1", "l2"]) }];
        assert!(outdated(&lines, &on_station, Some(&Place::default()), mv_anime::SceneKind::Dialogue).is_some_and(|w| w.contains("place changed")));
    }

    #[test]
    fn scenes_follow_the_episode_order() {
        let scenes: Vec<Scene> = ["a", "b", "c"].iter().map(|id| Scene { id: id.to_string(), ..Scene::default() }).collect();
        assert_eq!(scene_order(&[], &scenes), ["a", "b", "c"]);
        assert_eq!(scene_order(&["c".into(), "x".into(), "a".into()], &scenes), ["c", "a", "b"]);
    }

    #[test]
    fn the_latest_planning_job_of_the_scene_counts() {
        let job = |id: u64, scene: &str, state: JobState| Job { id, project: "p".into(), kind: JobKind::PlanScene { episode: "ep01".into(), scene: scene.into() }, state, ..Job::default() };
        let q = Queue { jobs: vec![job(1, "s1", JobState::Failed), job(2, "s1", JobState::Running), job(3, "s2", JobState::Queued)], ..Queue::default() };
        assert!(matches!(job_busy(&q, "p", "ep01", "s1"), Busy::Running { id: 2, .. }));
        assert!(matches!(job_busy(&q, "p", "ep01", "s2"), Busy::Waiting { id: 3, paused: false }));
        assert_eq!(job_busy(&q, "p", "ep01", "s3"), Busy::Idle);
        assert_eq!(job_busy(&q, "other", "ep01", "s1"), Busy::Idle);
    }
}
