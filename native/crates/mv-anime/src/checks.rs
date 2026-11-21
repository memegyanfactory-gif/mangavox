//! Film-grammar checks (design 6.4, pure): layout shots, the 180° line, eyelines, close-shot
//! runs, pacing, flashes, faces shown, line fit, ... Each `Issue` carries the `fix` text the
//! revise call and the UI both use.
//!
//! The thresholds are the numbers measured cut by cut on two TV episodes (plan section 3.2,
//! reports 21-25); they are constants here so the planner prompt ([`rules_text`]) states exactly
//! what the checks test. Cut numbers in messages count from 1 within the scene.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::geometry;
use crate::model::{
    Angle, BackgroundKind, BibleIndex, Issue, Line, LineKind, Mark, Move, MouthPolicy, Pattern, Place, Pose, Role, Scene, SceneKind, Severity, Shot, ShotSize, Story,
    Transition, Visibility,
};
use crate::pacing;

/// A scene in a new place shows its layout by this cut ...
pub const LAYOUT_BY_CUT: usize = 2;
/// ... starting at most this far into the scene.
pub const LAYOUT_BY_S: f32 = 6.;
/// A run of this many cuts without a layout shot is too long.
pub const LAYOUT_EVERY_CUTS: usize = 12;
/// Longest stretch without a layout shot (seconds), by scene kind.
pub const LAYOUT_EVERY_S: f32 = 30.;
/// Longest stretch without a layout shot in a dialogue or narration scene (seconds).
pub const LAYOUT_EVERY_TALK_S: f32 = 35.;
/// Longest stretch without a layout shot in an action or fight scene (seconds).
pub const LAYOUT_EVERY_ACTION_S: f32 = 13.;
/// Most close shots (MCU, CU, ECU) in a row ...
pub const CLOSE_RUN_MAX: usize = 3;
/// ... or in a panic burst where every cut is shorter than [`BURST_CUT_S`].
pub const CLOSE_BURST_MAX: usize = 7;
/// A cut shorter than this (seconds) belongs to a panic burst.
pub const BURST_CUT_S: f32 = 1.;
/// Threat and hero share a frame at most this many cuts before a hit.
pub const THREAT_WITHIN_CUTS: usize = 2;
/// How far over its target a scene's full-face share may go.
pub const FACE_SLACK: f32 = 0.1;
/// A face counts as shown when the character faces within this angle of the camera.
pub const FACE_ANGLE_DEG: f32 = 100.;
/// The episode's full-face share at most (with at least [`EPISODE_MIN_SHOTS`] shots).
pub const EPISODE_FACE_MAX: f32 = 0.45;
/// Shots an episode needs before its full-face share is judged.
pub const EPISODE_MIN_SHOTS: usize = 12;
/// Shares inside a scene (faces, pace, step-down) are judged from this many shots on.
pub const SHARE_MIN_SHOTS: usize = 4;
/// A scene's mean cut stays within its kind's range times these.
pub const PACE_LOW: f32 = 0.85;
/// A scene's mean cut stays above its kind's lowest pace times this ...
pub const PACE_HIGH: f32 = 1.15;
/// The first cut in a new place holds at least this long.
pub const FIRST_HOLD_S: f32 = 2.5;
/// At most this many flashes in any window of [`FLASH_WINDOW_S`] (broadcast safety).
pub const FLASH_MAX: usize = 3;
/// ... and below its kind's highest pace times this.
pub const FLASH_WINDOW_S: f32 = 1.;
/// One shot carries at most this much speech and this many lines.
pub const SHOT_MAX_S: f32 = pacing::CUT_MAX_S;
/// The window (seconds) in which flashes are counted.
pub const SHOT_MAX_LINES: usize = 5;
/// Close-to-close cuts at most this share of all cuts between shots (measured 11%).
pub const CLOSE_TO_CLOSE_MAX: f32 = 0.25;
/// Fight scenes: at least this share of shots cowboy or wider.
pub const FIGHT_WIDE_MIN: f32 = 1. / 3.;
/// Episode: share of screen time with a moving camera.
pub const CAMERA_SHARE: (f32, f32) = (0.35, 0.65);
/// Episode: background need by share of cuts (matched room, simple backdrop, none) ...
pub const BACKGROUND_MIX: [(BackgroundKind, f32); 3] = [(BackgroundKind::Matched, 0.25), (BackgroundKind::Backdrop, 0.40), (BackgroundKind::None, 0.30)];
/// ... reported when one is further off than this.
pub const BACKGROUND_SLACK: f32 = 0.15;
/// Longest line under threat (words): fight scenes, dread scenes.
pub const FIGHT_LINE_WORDS: usize = 8;
/// Longest line under threat (words) in a dread scene.
pub const DREAD_LINE_WORDS: usize = 10;

/// error = 3, warning = 1, info = 0.
pub fn score(issues: &[Issue]) -> usize {
    issues
        .iter()
        .map(|i| match i.severity {
            Severity::Error => 3,
            Severity::Warning => 1,
            _ => 0,
        })
        .sum()
}

/// A plan goes back to the director for any error or two warnings (design 6.3).
pub fn needs_revision(issues: &[Issue]) -> bool {
    issues.iter().any(|i| i.severity == Severity::Error) || issues.iter().filter(|i| i.severity == Severity::Warning).count() >= 2
}

/// The issues as notes the revise call reads: "cut 4: ... Fix: ...". Info is left out.
pub fn notes(issues: &[Issue]) -> String {
    let mut o = String::new();
    for i in issues.iter().filter(|i| i.severity != Severity::Info) {
        let sev = if i.severity == Severity::Error { "MUST FIX" } else { "should fix" };
        let _ = write!(o, "- [{sev}] ({}) {}", i.code, i.message);
        if let Some(f) = &i.fix {
            let _ = write!(o, " Fix: {f}");
        }
        o.push('\n');
    }
    o
}

/// Target share of cuts showing a full face, by scene intensity (talk 70%, the first death 50%,
/// maiming 35%, a death scene 20-25%).
pub fn face_target(intensity: u8) -> f32 {
    match intensity {
        0..=4 => 0.70,
        5..=7 => 0.50,
        8 => 0.35,
        _ => 0.25,
    }
}

/// Close shot: MCU, CU, ECU.
pub fn is_close(size: ShotSize) -> bool {
    matches!(size, ShotSize::Mcu | ShotSize::Cu | ShotSize::Ecu)
}

fn in_frame(m: &Mark) -> bool {
    m.visible != Visibility::Offscreen && !m.character.is_empty()
}

/// Layout shot: extreme long, long or overhead; or full shot on the matched room with at least
/// two people in it.
pub fn is_layout(shot: &Shot) -> bool {
    matches!(shot.camera.size, ShotSize::Els | ShotSize::Ls | ShotSize::Overhead)
        || shot.camera.angle == Angle::Top
        || (shot.camera.size == ShotSize::Fs && shot.background == BackgroundKind::Matched && shot.marks.iter().filter(|m| in_frame(m)).count() >= 2)
}

/// Cowboy or wider.
fn is_wide_enough(size: ShotSize) -> bool {
    matches!(size, ShotSize::Els | ShotSize::Ls | ShotSize::Fs | ShotSize::Cowboy | ShotSize::Overhead)
}

/// A shot after which the camera may stand on the other side of a pair.
fn is_bridge(shot: &Shot) -> bool {
    shot.bridge || shot.camera.size == ShotSize::Overhead || shot.camera.angle == Angle::Top || shot.camera.size == ShotSize::Ecu
}

fn norm_deg(a: f32) -> f32 {
    if a.is_finite() {
        a.rem_euclid(360.)
    } else {
        0.
    }
}

fn angle_diff(a: f32, b: f32) -> f32 {
    let d = norm_deg(a - b);
    if d > 180. {
        360. - d
    } else {
        d
    }
}

/// Yaw (0 = north, clockwise) of a plan direction.
fn yaw_of(dx: f32, dz: f32) -> f32 {
    norm_deg(dx.atan2(-dz).to_degrees())
}

fn faces_camera(m: &Mark, cam: [f32; 3]) -> bool {
    if m.gaze.as_deref() == Some("camera") {
        return true;
    }
    let (dx, dz) = (cam[0] - m.pos[0], cam[2] - m.pos[1]);
    let d = (dx * dx + dz * dz).sqrt();
    if !d.is_finite() || d < 0.05 {
        return true;
    }
    angle_diff(m.facing_deg, yaw_of(dx, dz)) <= FACE_ANGLE_DEG
}

/// Face shown: a character seen whole (visible Full), facing within ±100° of the camera, in a
/// medium, close, two-shot or over-the-shoulder framing.
pub fn face_shown(shot: &Shot, place: Option<&Place>) -> bool {
    if !matches!(shot.camera.size, ShotSize::Ms | ShotSize::Mcu | ShotSize::Cu | ShotSize::Ecu | ShotSize::TwoShot | ShotSize::Ots) {
        return false;
    }
    let cam = geometry::cam_pos(&shot.camera, place);
    shot.marks.iter().any(|m| m.visible == Visibility::Full && !m.character.is_empty() && faces_camera(m, cam))
}

/// -1 (screen left), 1 (screen right) or 0 (toward or away from the camera) for where a
/// character looks in the frame.
fn screen_look(m: &Mark, cam_yaw: f32) -> i8 {
    let s = (m.facing_deg - cam_yaw).to_radians().sin();
    if !s.is_finite() || s.abs() < 0.25 {
        0
    } else if s > 0. {
        1
    } else {
        -1
    }
}

/// One cut with its place in the scene's time.
struct Cut<'a> {
    shot: &'a Shot,
    /// 1-based.
    n: usize,
    start: f32,
    secs: f32,
}

struct Ctx<'a> {
    scene: &'a Scene,
    place: Option<&'a Place>,
    bible: &'a BibleIndex,
    lines: BTreeMap<&'a str, &'a Line>,
    out: Vec<Issue>,
}

impl Ctx<'_> {
    fn push(&mut self, code: &str, severity: Severity, shot: Option<&Shot>, message: String, fix: Option<String>) {
        self.out.push(Issue {
            code: code.to_string(),
            severity,
            scene: self.scene.id.clone(),
            shot: shot.map(|s| s.id.clone()).filter(|id| !id.is_empty()),
            message,
            fix,
        });
    }

    fn name(&self, id: &str) -> String {
        self.bible.character(id).map(|c| c.name.clone()).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| id.to_string())
    }

    fn role(&self, id: &str) -> Role {
        self.bible.character(id).map(|c| c.role).unwrap_or_default()
    }

    fn shot_lines(&self, shot: &Shot) -> Vec<Line> {
        shot.lines.iter().filter_map(|id| self.lines.get(id.as_str()).map(|l| (*l).clone())).collect()
    }
}

/// Seconds a line takes when voiced (estimated: no voice takes here); sound effects take none.
fn line_secs(l: &Line) -> Option<f32> {
    if l.kind == LineKind::Sfx {
        return None;
    }
    let t = if l.spoken.trim().is_empty() { l.original.trim() } else { l.spoken.trim() };
    (!t.is_empty()).then(|| pacing::line_seconds(t))
}

fn cut_range(a: usize, b: usize) -> String {
    if a == b {
        format!("cut {a}")
    } else {
        format!("cuts {a}-{b}")
    }
}

/// The checks of one scene's shots. `new_place`: the scene opens in a place not seen just before.
pub fn check_scene(scene: &Scene, shots: &[Shot], lines: &[Line], bible: &BibleIndex, new_place: bool) -> Vec<Issue> {
    if shots.is_empty() {
        return Vec::new();
    }
    let place = scene.place.as_deref().and_then(|p| bible.place(p));
    let mine: Vec<&Line> = {
        let own: Vec<&Line> = lines.iter().filter(|l| l.scene == scene.id).collect();
        if own.is_empty() {
            lines.iter().collect()
        } else {
            own
        }
    };
    let mut cx = Ctx { scene, place, bible, lines: lines.iter().map(|l| (l.id.as_str(), l)).collect(), out: Vec::new() };
    let mut t = 0.;
    let cuts: Vec<Cut> = shots
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let secs: Vec<f32> = cx.shot_lines(s).iter().filter_map(line_secs).collect();
            let d = pacing::effective_s(s, scene.kind, &secs);
            let c = Cut { shot: s, n: i + 1, start: t, secs: d };
            t += d;
            c
        })
        .collect();

    layout_early(&mut cx, &cuts, new_place);
    layout_return(&mut cx, &cuts);
    close_run(&mut cx, &cuts);
    threat_before_hit(&mut cx, &cuts);
    axis(&mut cx, &cuts);
    eyeline(&mut cx, &cuts);
    along_lens(&mut cx, &cuts);
    faces(&mut cx, &cuts);
    pace(&mut cx, &cuts);
    first_hold(&mut cx, &cuts, new_place);
    flash_rate(&mut cx, &cuts);
    jump_cut(&mut cx, &cuts);
    lines_fit(&mut cx, &cuts);
    lines_cover(&mut cx, &cuts, &mine);
    shake_talk(&mut cx, &cuts);
    step_down(&mut cx, &cuts);
    progression(&mut cx, &cuts);
    fight_wide(&mut cx, &cuts);
    mouth(&mut cx, &cuts);
    line_length(&mut cx, &cuts, &mine);
    cx.out
}

fn layout_early(cx: &mut Ctx, cuts: &[Cut], new_place: bool) {
    if !new_place {
        return;
    }
    let ok = cuts.iter().take(LAYOUT_BY_CUT).any(|c| is_layout(c.shot) && c.start <= LAYOUT_BY_S + 1e-3);
    if !ok {
        let place = cx.place.map(|p| p.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| "the new place".into());
        cx.push(
            "layout-early",
            Severity::Error,
            cuts.first().map(|c| c.shot),
            format!("The scene opens in {place} but neither of its first two cuts shows where everyone is (a wide, long or overhead shot starting within {LAYOUT_BY_S:.0} s)."),
            Some(format!("Make cut 1 or cut 2 an extreme long, long or overhead shot of {place} that starts within {LAYOUT_BY_S:.0} s (or open with the arrival pattern).")),
        );
    }
}

fn layout_limit_s(scene: &Scene) -> f32 {
    if scene.fight || scene.kind == SceneKind::Action {
        LAYOUT_EVERY_ACTION_S
    } else if matches!(scene.kind, SceneKind::Dialogue | SceneKind::Narration | SceneKind::Flashback) {
        LAYOUT_EVERY_TALK_S
    } else {
        LAYOUT_EVERY_S
    }
}

fn layout_return(cx: &mut Ctx, cuts: &[Cut]) {
    let limit = layout_limit_s(cx.scene);
    let mut runs: Vec<(usize, usize)> = Vec::new(); // index ranges [a, b) of non-layout cuts
    let mut start = 0;
    for (i, c) in cuts.iter().enumerate() {
        if is_layout(c.shot) {
            if i > start {
                runs.push((start, i));
            }
            start = i + 1;
        }
    }
    if start < cuts.len() {
        runs.push((start, cuts.len()));
    }
    for (a, b) in runs {
        let run = &cuts[a..b];
        // Fog, effects or an inner state for the whole run replace the room on purpose.
        if run.iter().all(|c| c.shot.background == BackgroundKind::None) {
            continue;
        }
        let secs: f32 = run.iter().map(|c| c.secs).sum();
        let too_many = run.len() >= LAYOUT_EVERY_CUTS;
        if too_many || secs > limit + 1e-3 {
            let (first, last) = (run[0].n, run[run.len() - 1].n);
            // Where a wide would fix it: within the limits from the run's start.
            let mut at = first;
            let mut acc = 0.;
            for c in run {
                acc += c.secs;
                if acc > limit || c.n - first + 1 >= LAYOUT_EVERY_CUTS {
                    break;
                }
                at = c.n;
            }
            cx.push(
                "layout-return",
                Severity::Warning,
                Some(run[0].shot),
                format!(
                    "{} ({} cuts, {secs:.0} s) never show where everyone is; real episodes come back to a wide at least every {LAYOUT_EVERY_CUTS} cuts or {limit:.0} s in this kind of scene.",
                    cut_range(first, last),
                    run.len()
                ),
                Some(format!("Add a wide or overhead shot of the place around cut {at}, or turn one of these cuts into one.")),
            );
        }
    }
}

fn close_run(cx: &mut Ctx, cuts: &[Cut]) {
    let mut i = 0;
    while i < cuts.len() {
        if !is_close(cuts[i].shot.camera.size) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < cuts.len() && is_close(cuts[j].shot.camera.size) {
            j += 1;
        }
        let run = &cuts[i..j];
        let burst = run.iter().all(|c| c.secs < BURST_CUT_S);
        let cap = if burst { CLOSE_BURST_MAX } else { CLOSE_RUN_MAX };
        if run.len() > cap {
            let (a, b) = (run[0].n, run[run.len() - 1].n);
            cx.push(
                "close-run",
                Severity::Warning,
                Some(run[cap].shot),
                format!("{} are {} close shots in a row; real episodes break a run after {CLOSE_RUN_MAX} (or {CLOSE_BURST_MAX} in a panic burst of cuts under {BURST_CUT_S:.0} s).", cut_range(a, b), run.len()),
                Some(format!("Change cut {} to a medium or wider shot, or an insert (a hand, the feet, an object).", run[cap].n)),
            );
        }
        i = j;
    }
}

fn threat_before_hit(cx: &mut Ctx, cuts: &[Cut]) {
    if !cx.scene.fight {
        return;
    }
    let all: BTreeSet<&str> = cuts.iter().flat_map(|c| c.shot.marks.iter().filter(|m| in_frame(m)).map(|m| m.character.as_str())).collect();
    let threats: Vec<&str> = all.iter().copied().filter(|id| cx.role(id) == Role::Threat).collect();
    let heroes: Vec<&str> = all.iter().copied().filter(|id| cx.role(id) == Role::Hero).collect();
    if threats.is_empty() || heroes.is_empty() {
        return; // nobody to put together
    }
    let together = |s: &Shot| {
        let ids: BTreeSet<&str> = s.marks.iter().filter(|m| in_frame(m)).map(|m| m.character.as_str()).collect();
        threats.iter().any(|t| ids.contains(t)) && heroes.iter().any(|h| ids.contains(h))
    };
    for (i, c) in cuts.iter().enumerate().filter(|(_, c)| c.shot.hit) {
        let ok = cuts[i.saturating_sub(THREAT_WITHIN_CUTS)..i].iter().any(|p| together(p.shot));
        if !ok {
            let (t, h) = (cx.name(threats[0]), cx.name(heroes[0]));
            cx.push(
                "threat-before-hit",
                Severity::Warning,
                Some(c.shot),
                format!("The hit on cut {} comes without {t} and {h} in one frame in the {THREAT_WITHIN_CUTS} cuts before it, so the viewer cannot tell who strikes whom.", c.n),
                Some(format!("Put {t} and {h} together in a wide or full shot right before cut {}.", c.n)),
            );
        }
    }
}

fn axis(cx: &mut Ctx, cuts: &[Cut]) {
    let mut sides: BTreeMap<(String, String), (i8, usize)> = BTreeMap::new();
    for c in cuts {
        if is_bridge(c.shot) {
            sides.clear();
            continue;
        }
        let cam = geometry::cam_pos(&c.shot.camera, cx.place);
        let mut vis: Vec<&Mark> = c.shot.marks.iter().filter(|m| in_frame(m)).collect();
        vis.sort_by(|a, b| a.character.cmp(&b.character));
        vis.dedup_by(|a, b| a.character == b.character);
        let mut flagged = false;
        for (i, a) in vis.iter().enumerate() {
            for b in &vis[i + 1..] {
                let s = geometry::side_of_line(a.pos, b.pos, [cam[0], cam[2]]);
                if s == 0 {
                    continue;
                }
                let key = (a.character.clone(), b.character.clone());
                match sides.get(&key).copied() {
                    Some((was, n)) if was != s => {
                        if !flagged {
                            flagged = true;
                            let (an, bn) = (cx.name(&a.character), cx.name(&b.character));
                            cx.push(
                                "axis",
                                Severity::Error,
                                Some(c.shot),
                                format!("Cut {} crosses the line between {an} and {bn}: the camera is on the other side than in cut {n}, so they swap sides of the screen.", c.n),
                                Some(format!(
                                    "Keep the camera on the same side of {an} and {bn} as in cut {n}, or put a bridge shot (head-on, overhead or extreme close-up) before cut {}.",
                                    c.n
                                )),
                            );
                        }
                        sides.insert(key, (s, c.n));
                    }
                    Some(_) => {}
                    None => {
                        sides.insert(key, (s, c.n));
                    }
                }
            }
        }
    }
}

/// The one character seen in a single (nobody else in frame), if the shot is one.
fn single(shot: &Shot) -> Option<&Mark> {
    let vis: Vec<&Mark> = shot.marks.iter().filter(|m| in_frame(m)).collect();
    match vis.as_slice() {
        [m] if m.visible != Visibility::Ots => Some(m),
        _ => None,
    }
}

fn eyeline(cx: &mut Ctx, cuts: &[Cut]) {
    for w in cuts.windows(2) {
        let (p, c) = (&w[0], &w[1]);
        let (Some(a), Some(b)) = (single(p.shot), single(c.shot)) else { continue };
        if a.character == b.character {
            continue;
        }
        // Only a shot and its reverse: neither looks at something else.
        let looks_at_other = |m: &Mark, other: &str| m.gaze.as_deref().is_none_or(|g| g == format!("char:{other}"));
        if !looks_at_other(a, &b.character) || !looks_at_other(b, &a.character) {
            continue;
        }
        let ya = geometry::look_dir(&p.shot.camera, &p.shot.marks, cx.place).0;
        let yb = geometry::look_dir(&c.shot.camera, &c.shot.marks, cx.place).0;
        let (la, lb) = (screen_look(a, ya), screen_look(b, yb));
        if la != 0 && la == lb {
            let side = if la > 0 { "right" } else { "left" };
            let other = if la > 0 { "left" } else { "right" };
            let (an, bn) = (cx.name(&a.character), cx.name(&b.character));
            cx.push(
                "eyeline",
                Severity::Warning,
                Some(c.shot),
                format!("Cuts {} and {}: {an} and {bn} both look screen {side}, so their eyes never meet.", p.n, c.n),
                Some(format!("In cut {}, have {bn} look screen {other} toward {an} (or move the camera to the other side of {bn}).", c.n)),
            );
        }
    }
}

fn along_lens(cx: &mut Ctx, cuts: &[Cut]) {
    for c in cuts {
        if c.shot.camera.size == ShotSize::Pov {
            continue;
        }
        let yaw = geometry::look_dir(&c.shot.camera, &c.shot.marks, cx.place).0;
        let across = c.shot.marks.iter().find(|m| in_frame(m) && m.pose == Pose::Run && (m.facing_deg - yaw).to_radians().sin().abs() > 0.7);
        if let Some(m) = across {
            let name = cx.name(&m.character);
            cx.push(
                "along-lens",
                Severity::Info,
                Some(c.shot),
                format!("Cut {}: {name} runs across the frame; real episodes stage most runs and falls toward or away from the camera.", c.n),
                Some(format!("Turn cut {} so {name} runs toward or away from the lens (it cannot break screen direction).", c.n)),
            );
        }
    }
}

fn faces(cx: &mut Ctx, cuts: &[Cut]) {
    if cuts.len() < SHARE_MIN_SHOTS {
        return;
    }
    let shown: Vec<usize> = cuts.iter().filter(|c| face_shown(c.shot, cx.place)).map(|c| c.n).collect();
    let share = shown.len() as f32 / cuts.len() as f32;
    let target = face_target(cx.scene.intensity);
    if share > target + FACE_SLACK + 1e-4 {
        let allowed = ((target + FACE_SLACK) * cuts.len() as f32).floor() as usize;
        let over = shown.len().saturating_sub(allowed).max(1);
        let list = shown.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ");
        cx.push(
            "faces",
            Severity::Warning,
            None,
            format!(
                "{} of the scene's {} cuts show a full face ({:.0}%); at intensity {} real episodes show about {:.0}%, fewer as the stakes rise.",
                shown.len(),
                cuts.len(),
                share * 100.,
                cx.scene.intensity,
                target * 100.
            ),
            Some(format!("Carry {over} of these beats with a back, a profile, hands, the eyes cut off or a wide instead of a full face (cuts {list}).")),
        );
    }
}

fn pace(cx: &mut Ctx, cuts: &[Cut]) {
    if cuts.len() < SHARE_MIN_SHOTS {
        return;
    }
    let mean = cuts.iter().map(|c| c.secs).sum::<f32>() / cuts.len() as f32;
    let row = pacing::pace(cx.scene.kind);
    let (lo, hi) = (row.mean_range_s.0 * PACE_LOW, row.mean_range_s.1 * PACE_HIGH);
    let kind = serde_json::to_value(cx.scene.kind).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    if mean < lo - 1e-3 {
        cx.push(
            "pace",
            Severity::Warning,
            None,
            format!("Its cuts average {mean:.1} s; a {kind} scene averages {:.1}-{:.1} s, so it feels rushed.", row.mean_range_s.0, row.mean_range_s.1),
            Some("Merge neighbouring cuts into longer takes that carry several lines, or hold the wides longer.".into()),
        );
    } else if mean > hi + 1e-3 {
        cx.push(
            "pace",
            Severity::Warning,
            None,
            format!("Its cuts average {mean:.1} s; a {kind} scene averages {:.1}-{:.1} s, so it drags.", row.mean_range_s.0, row.mean_range_s.1),
            Some("Split the long takes with reactions, inserts and over-the-shoulder shots.".into()),
        );
    }
}

fn first_hold(cx: &mut Ctx, cuts: &[Cut], new_place: bool) {
    let Some(c) = cuts.first() else { return };
    if new_place && c.secs + 1e-3 < FIRST_HOLD_S {
        cx.push(
            "first-hold",
            Severity::Info,
            Some(c.shot),
            format!("The first cut in the new place lasts {:.1} s; establishing shots are held (about {:.1} s).", c.secs, pacing::NEW_PLACE_FIRST_S),
            Some(format!("Hold cut 1 for at least {FIRST_HOLD_S:.1} s.")),
        );
    }
}

/// Flash moments of the scene: (time, cut).
fn flashes(cuts: &[Cut]) -> Vec<(f32, usize)> {
    let mut ev = Vec::new();
    for c in cuts {
        if c.shot.flash {
            ev.push((c.start, c.n));
        }
        if c.shot.transition == Transition::White {
            ev.push((c.start + c.secs, c.n));
        }
    }
    ev.sort_by(|a, b| a.0.total_cmp(&b.0));
    ev
}

fn flash_rate(cx: &mut Ctx, cuts: &[Cut]) {
    let ev = flashes(cuts);
    for (i, (at, n)) in ev.iter().enumerate() {
        let within = ev[i..].iter().take_while(|(b, _)| *b < at + FLASH_WINDOW_S - 1e-4).count();
        if within > FLASH_MAX {
            let shot = cuts.iter().find(|c| c.n == *n).map(|c| c.shot);
            cx.push(
                "flash-rate",
                Severity::Error,
                shot,
                format!("{within} flashes within one second from cut {n}; at most {FLASH_MAX} a second is safe for viewers (broadcast guideline)."),
                Some(format!("Drop some flashes or white transitions near cut {n}, or space those cuts out.")),
            );
            break;
        }
    }
}

/// Who a shot is of: everyone seen, but not a shoulder in the foreground.
fn subject(shot: &Shot) -> BTreeSet<&str> {
    shot.marks.iter().filter(|m| matches!(m.visible, Visibility::Full | Visibility::Partial | Visibility::Back) && !m.character.is_empty()).map(|m| m.character.as_str()).collect()
}

fn jump_cut(cx: &mut Ctx, cuts: &[Cut]) {
    for w in cuts.windows(2) {
        let (p, c) = (&w[0], &w[1]);
        let (sp, sc) = (subject(p.shot), subject(c.shot));
        let burst = p.secs < BURST_CUT_S && c.secs < BURST_CUT_S;
        if p.shot.transition == Transition::Cut && !sp.is_empty() && sp == sc && p.shot.camera.size == c.shot.camera.size && p.shot.camera.angle == c.shot.camera.angle && !burst {
            cx.push(
                "jump-cut",
                Severity::Error,
                Some(c.shot),
                format!("Cuts {} and {} show the same people at the same size and angle: a jump cut.", p.n, c.n),
                Some(format!("Change cut {} by two sizes or a clearly different angle, or cut away to something else between them.", c.n)),
            );
        }
    }
}

fn lines_fit(cx: &mut Ctx, cuts: &[Cut]) {
    for c in cuts {
        let lines = cx.shot_lines(c.shot);
        let need = pacing::spoken_seconds(&lines, cx.scene.kind);
        if need > SHOT_MAX_S + 1e-3 || c.shot.lines.len() > SHOT_MAX_LINES {
            cx.push(
                "lines-fit",
                Severity::Error,
                Some(c.shot),
                format!("Cut {} carries {} lines that take {need:.0} s to say; one cut holds at most {SHOT_MAX_LINES} lines and {SHOT_MAX_S:.0} s.", c.n, c.shot.lines.len()),
                Some(format!("Split the lines of cut {} over two or three cuts (a reverse, a listener, an insert).", c.n)),
            );
        }
    }
}

fn lines_cover(cx: &mut Ctx, cuts: &[Cut], mine: &[&Line]) {
    let mut count: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for c in cuts {
        for id in &c.shot.lines {
            count.entry(id.as_str()).or_default().push(c.n);
        }
    }
    let missing: Vec<&Line> = mine.iter().copied().filter(|l| !count.contains_key(l.id.as_str())).collect();
    if !missing.is_empty() {
        let shown: Vec<String> = missing.iter().take(3).map(|l| quote(l)).collect();
        let more = if missing.len() > 3 { format!(" and {} more", missing.len() - 3) } else { String::new() };
        let what = if missing.len() == 1 { "1 of the scene's lines is".to_string() } else { format!("{} of the scene's lines are", missing.len()) };
        cx.push(
            "lines-cover",
            Severity::Error,
            None,
            format!("{what} in no cut ({}{more}), so {} never be heard.", shown.join(", "), if missing.len() == 1 { "it would" } else { "they would" }),
            Some("Put every line of the scene in exactly one cut.".into()),
        );
    }
    for (id, ns) in count.iter().filter(|(_, ns)| ns.len() > 1) {
        let shot = cuts.iter().find(|c| c.n == ns[1]).map(|c| c.shot);
        let said = cx.lines.get(id).map(|l| format!("The line {}", quote(l))).unwrap_or_else(|| "A line".into());
        cx.push(
            "lines-cover",
            Severity::Error,
            shot,
            format!("{said} is in more than one cut ({}), so it would be said twice.", ns.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ")),
            Some(format!("Keep it in cut {} only.", ns[0])),
        );
    }
}

/// A line as the user knows it: its first words in quotes ("“We have to go back…”").
fn quote(l: &Line) -> String {
    let t = if l.spoken.trim().is_empty() { l.original.trim() } else { l.spoken.trim() };
    let t: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
    let short: String = t.chars().take(40).collect();
    let short = if short.chars().count() < t.chars().count() { format!("{}…", short.trim_end()) } else { short };
    format!("\u{201c}{short}\u{201d}")
}

fn shake_talk(cx: &mut Ctx, cuts: &[Cut]) {
    if !matches!(cx.scene.kind, SceneKind::Dialogue | SceneKind::Narration | SceneKind::Flashback) {
        return;
    }
    for c in cuts.iter().filter(|c| c.shot.camera.movement.kind == Move::Shake) {
        cx.push(
            "shake-talk",
            Severity::Warning,
            Some(c.shot),
            format!("Cut {} shakes the camera in a talk scene; shake belongs to a hit, a landing, an explosion or a scare.", c.n),
            Some(format!("Hold cut {} still, or use a slow push.", c.n)),
        );
    }
}

fn step_down(cx: &mut Ctx, cuts: &[Cut]) {
    if cuts.len() < SHARE_MIN_SHOTS + 1 {
        return;
    }
    let pairs = cuts.len() - 1;
    let cc = cuts.windows(2).filter(|w| is_close(w[0].shot.camera.size) && is_close(w[1].shot.camera.size)).count();
    let share = cc as f32 / pairs as f32;
    if share > CLOSE_TO_CLOSE_MAX + 1e-4 {
        cx.push(
            "step-down",
            Severity::Info,
            None,
            format!("{cc} of the {pairs} cuts go from one close shot straight to another ({:.0}%; real episodes about 11%).", share * 100.),
            Some("Step down in size: after a wide, go to a medium; save close-to-close cuts for a shock or a locked two-person exchange.".into()),
        );
    }
}

fn progression(cx: &mut Ctx, cuts: &[Cut]) {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for c in cuts {
        for m in c.shot.marks.iter().filter(|m| in_frame(m)) {
            if !seen.insert(m.character.clone()) {
                continue;
            }
            let deliberate = matches!(c.shot.pattern, Some(Pattern::Stunned) | Some(Pattern::Reveal));
            if c.shot.camera.size == ShotSize::Ecu && !deliberate {
                let name = cx.name(&m.character);
                cx.push(
                    "progression",
                    Severity::Warning,
                    Some(c.shot),
                    format!("Cut {} is the first sight of {name} in this scene, and it is an extreme close-up.", c.n),
                    Some(format!("Show {name} whole first (where they stand, what they face), then go in.")),
                );
            }
        }
    }
}

fn fight_wide(cx: &mut Ctx, cuts: &[Cut]) {
    if !cx.scene.fight || cuts.len() < 3 {
        return;
    }
    let wide = cuts.iter().filter(|c| is_wide_enough(c.shot.camera.size)).count();
    if (wide as f32) < FIGHT_WIDE_MIN * cuts.len() as f32 - 1e-4 {
        cx.push(
            "fight-wide",
            Severity::Warning,
            None,
            format!("Only {wide} of the fight's {} cuts are cowboy or wider; about a third must show attacker and target together.", cuts.len()),
            Some("Turn some close coverage into full or long shots with both fighters in frame.".into()),
        );
    }
}

fn mouth(cx: &mut Ctx, cuts: &[Cut]) {
    for c in cuts.iter().filter(|c| c.shot.mouth == MouthPolicy::Generate && is_close(c.shot.camera.size)) {
        let lines = cx.shot_lines(c.shot);
        let speaker_seen = lines.iter().filter(|l| l.kind == LineKind::Speech).filter_map(|l| l.speaker.as_deref()).any(|sp| c.shot.marks.iter().any(|m| m.character == sp && m.visible == Visibility::Full));
        if speaker_seen {
            cx.push(
                "mouth",
                Severity::Warning,
                Some(c.shot),
                format!("Cut {} asks the picture model to draw a talking mouth in a close shot; it cannot keep it in sync.", c.n),
                Some(format!("Set the mouth of cut {} to flap (drawn mouth shapes) or stage the line on a listener, a back or a hand.", c.n)),
            );
        }
    }
}

fn line_length(cx: &mut Ctx, cuts: &[Cut], mine: &[&Line]) {
    let limit = match (cx.scene.fight, cx.scene.kind == SceneKind::Dread) {
        (true, _) => FIGHT_LINE_WORDS,
        (false, true) => DREAD_LINE_WORDS,
        _ => return,
    };
    for l in mine.iter().filter(|l| matches!(l.kind, LineKind::Speech | LineKind::Thought)) {
        let text = if l.spoken.trim().is_empty() { &l.original } else { &l.spoken };
        let n = pacing::words(text);
        if n > limit {
            let shot = cuts.iter().find(|c| c.shot.lines.iter().any(|id| id == &l.id)).map(|c| c.shot);
            cx.push(
                "line-length",
                Severity::Info,
                shot,
                format!("The line {} has {n} words; under threat real episodes keep lines to about {limit} words.", quote(l)),
                Some("Shorten it in the Story step.".into()),
            );
        }
    }
}

/// Episode-wide checks (faces share, camera share, background mix) plus every scene's, and (in
/// a build with frames) the "crowded" note of every shot showing more characters than one
/// picture can keep faces for.
pub fn check_episode(story: &Story, shots: &BTreeMap<String, Vec<Shot>>, bible: &BibleIndex) -> Vec<Issue> {
    let mut out = Vec::new();
    let mut prev_place: Option<&Option<String>> = None;
    let mut all: Vec<(&Shot, Option<&Place>, f32)> = Vec::new();
    for scene in &story.scenes {
        let new_place = prev_place.is_none_or(|p| *p != scene.place);
        prev_place = Some(&scene.place);
        let Some(list) = shots.get(&scene.id).filter(|l| !l.is_empty()) else { continue };
        let lines: Vec<Line> = story.lines.iter().filter(|l| l.scene == scene.id).cloned().collect();
        out.extend(check_scene(scene, list, &lines, bible, new_place));
        // A shot with more faces than one picture can keep gets its "crowded: second pass" note
        // (design 6.6), saved with the other results so the Shots and Frames steps show it.
        #[cfg(feature = "frames")]
        out.extend(list.iter().filter_map(|s| crate::slots::crowded_issue(s, scene, bible)));
        let place = scene.place.as_deref().and_then(|p| bible.place(p));
        let by_id: BTreeMap<&str, &Line> = lines.iter().map(|l| (l.id.as_str(), l)).collect();
        for s in list {
            let secs: Vec<f32> = s.lines.iter().filter_map(|id| by_id.get(id.as_str())).filter_map(|l| line_secs(l)).collect();
            all.push((s, place, pacing::effective_s(s, scene.kind, &secs)));
        }
    }
    if all.len() < EPISODE_MIN_SHOTS {
        return out;
    }
    let ep = |code: &str, severity: Severity, message: String, fix: &str| Issue { code: code.into(), severity, scene: String::new(), shot: None, message, fix: Some(fix.to_string()) };
    let n = all.len() as f32;
    let faces = all.iter().filter(|(s, p, _)| face_shown(s, *p)).count() as f32 / n;
    if faces > EPISODE_FACE_MAX + 1e-4 {
        out.push(ep(
            "faces",
            Severity::Warning,
            format!("{:.0}% of the episode's cuts show a full face; real episodes show about a third (at most {:.0}%).", faces * 100., EPISODE_FACE_MAX * 100.),
            "Carry more beats with backs, profiles, hands, feet, objects and wides with small figures.",
        ));
    }
    let total: f32 = all.iter().map(|(_, _, d)| d).sum();
    if total > 0. {
        // (+ 0: a sum of nothing is minus zero, which would read "-0%".)
        let moving = (all.iter().filter(|(s, _, _)| !matches!(s.camera.movement.kind, Move::Hold | Move::RackFocus)).map(|(_, _, d)| d).sum::<f32>() + 0.) / total;
        if moving < CAMERA_SHARE.0 - 1e-4 || moving > CAMERA_SHARE.1 + 1e-4 {
            out.push(ep(
                "camera-share",
                Severity::Info,
                format!("{:.0}% of the episode's screen time has a moving camera; real episodes have {:.0}-{:.0}%.", moving * 100., CAMERA_SHARE.0 * 100., CAMERA_SHARE.1 * 100.),
                if moving < CAMERA_SHARE.0 { "Add slow pans and pushes over held drawings (wides and emotional holds)." } else { "Lock off more cuts; a camera move needs a reason (reveal, follow, tighten, unsettle)." },
            ));
        }
    }
    let share = |k: BackgroundKind| all.iter().filter(|(s, _, _)| s.background == k).count() as f32 / n;
    let off: Vec<String> = BACKGROUND_MIX.iter().filter(|(k, t)| (share(*k) - t).abs() > BACKGROUND_SLACK).map(|(k, t)| {
        let name = serde_json::to_value(k).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        format!("{name} {:.0}% (about {:.0}%)", share(*k) * 100., t * 100.)
    }).collect();
    if !off.is_empty() {
        out.push(ep(
            "background-mix",
            Severity::Info,
            format!("Backgrounds: {}. Real episodes match the room in about a quarter of cuts and use a simple backdrop or none for the rest.", off.join(", ")),
            "Close shots rarely need the matched room: give them a plain wall, sky, darkness or effects.",
        ));
    }
    out
}

/// The checks as rules for the planner prompt, with the same numbers the checks test.
pub fn rules_text(scene: &Scene) -> String {
    let limit = layout_limit_s(scene);
    let mut o = String::new();
    let _ = writeln!(o, "THE CHECKS YOUR PLAN MUST PASS (measured on real TV anime; a plan that fails them comes back to you):");
    let _ = writeln!(o, "- layout-early: in a new place, cut 1 or 2 is a layout shot (extreme long, long or overhead; or a full shot on the matched room with 2+ people) starting within {LAYOUT_BY_S:.0} s.");
    let _ = writeln!(o, "- layout-return: never {LAYOUT_EVERY_CUTS} cuts or {limit:.0} s in a row without a layout shot (unless every one of those cuts has background none: fog, effects, an inner state).");
    let _ = writeln!(o, "- close-run: at most {CLOSE_RUN_MAX} close shots (mcu, cu, ecu) in a row; up to {CLOSE_BURST_MAX} only in a panic burst where each cut is under {BURST_CUT_S:.0} s. Break a run with a medium or wider shot or an insert.");
    let _ = writeln!(o, "- threat-before-hit: in a fight, the threat and the hero share one frame within the {THREAT_WITHIN_CUTS} cuts before every hit.");
    let _ = writeln!(o, "- axis: each pair keeps its screen sides; the camera stays on one side of the line between them. Cross only after a bridge shot (head-on, \"bridge\": true), an overhead or an extreme close-up.");
    let _ = writeln!(o, "- eyeline: in a shot and its reverse, the two look opposite ways across the screen (one left, one right).");
    let _ = writeln!(o, "- along-lens: stage runs and falls toward or away from the lens, not across it.");
    let _ = writeln!(
        o,
        "- faces: a full face (both eyes, facing the camera) in at most {:.0}% of this scene's cuts (intensity {}: talk 70%, first death 50%, maiming 35%, death 25%); the episode about a third, never above {:.0}%.",
        (face_target(scene.intensity) + FACE_SLACK) * 100.,
        scene.intensity,
        EPISODE_FACE_MAX * 100.
    );
    let row = pacing::pace(scene.kind);
    let _ = writeln!(o, "- pace: the scene's mean cut stays within {:.1}-{:.1} s.", row.mean_range_s.0 * PACE_LOW, row.mean_range_s.1 * PACE_HIGH);
    let _ = writeln!(o, "- first-hold: the first cut in a new place holds at least {FIRST_HOLD_S:.1} s (about {:.1} s).", pacing::NEW_PLACE_FIRST_S);
    let _ = writeln!(o, "- flash-rate: at most {FLASH_MAX} flashes (\"flash\": true or a white transition) in any second.");
    let _ = writeln!(o, "- jump-cut: never cut between two shots of the same people at the same size and angle.");
    let _ = writeln!(o, "- lines-fit: a cut carries at most {SHOT_MAX_LINES} lines and {SHOT_MAX_S:.0} s of speech; lines-cover: every line id in exactly one cut.");
    let _ = writeln!(o, "- shake-talk: no shake in dialogue, narration or flashback.");
    let _ = writeln!(o, "- step-down: close-to-close cuts at most {:.0}% of cuts (real episodes 11%); after a wide go to a medium.", CLOSE_TO_CLOSE_MAX * 100.);
    let _ = writeln!(o, "- progression: a character's first appearance in the scene is not an extreme close-up (except in the stunned or reveal patterns).");
    let _ = writeln!(o, "- fight-wide: in a fight at least a third of the cuts are cowboy or wider.");
    let _ = writeln!(o, "- mouth: never \"generate\" a talking mouth in a close shot; use flap, closed, hidden, back or offscreen.");
    let _ = writeln!(o, "- episode: moving camera {:.0}-{:.0}% of screen time; backgrounds about matched 25%, backdrop 40%, none 30%.", CAMERA_SHARE.0 * 100., CAMERA_SHARE.1 * 100.);
    o
}
