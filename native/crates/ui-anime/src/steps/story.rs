//! Step 2, Story (design 9.5) [story]: the episode's script. One main action adapts the read
//! chapters into scenes, beats and lines through the engine queue (`JobKind::Story`); the scene
//! list sits on the left, the selected scene's beats and lines in the centre, and its settings
//! (kind, tone, fight, colour theme, length, place, time) on the right with Auto chips.
//!
//! Everything is editable in place. Every edit marks what it changed as the user's (`edited` and
//! a JSON pointer in `locked`, design 5.4), so adapting again keeps it. An edit is applied to the
//! screen at once and saved in the background as a read-modify-write of `story.json` (by ids,
//! so a job that saved meanwhile is never overwritten), one save at a time. "Adapt this scene
//! again" is `JobKind::AdaptScene`; earlier versions of the script come from the store's history
//! snapshots (design 5.6). W3-S1.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context as _;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_anime::queue::{self, JobKind, JobState, Queue};
use mv_anime::story::{self as engine, Ids, COLOUR_THEMES};
use mv_anime::{source, Beat, Emotion, EmotionKind, Episode, Issue, Line, LineKind, Scene, SceneKind, Severity, Story, Store, TimeOfDay, Tone as Mood};
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::{btn, confirm_dialog, empty_view, error_view, icon_button, icon_ph, menu_button, muted, overflow_menu, ph, progress_bar, size, skeleton_line, skeleton_row, spinner, tag, tip, toast, ButtonKind, MenuItem, Toast, Tokens, Tone};

use super::shared::{plain_reason, SAVE_FAILED};
use super::{Step, StepCtx};
use crate::page::Load;
use crate::style::{COLUMN_HEAD_H, POLL_MS};
use crate::widgets::{ago, auto_chips, column_body, column_head, count, error_block, fmt_secs, info_line, inspector_row, is_narrow, label_caps, list_item, now_secs, section, spec, workspace, LineTone};

/// Whole-scene lock (the root pointer): adapting again never changes the scene.
const SCENE_LOCK: &str = "";
/// Pointer locked by structural edits (lines moved, added, removed; beats split or merged).
const BEATS: &str = "/beats";
/// Width of the speaker and emotion columns of a line row.
const SPEAKER_W: f32 = 132.;
const EMOTION_W: f32 = 132.;
/// The length field in the scene settings.
const LENGTH_W: f32 = 72.;
/// A line's words never get narrower than this: in a narrow script they go under the speaker.
const LINE_TEXT_MIN_W: f32 = 220.;

/// One save at a time, so read-modify-writes of `story.json` never interleave.
static SAVE: Mutex<()> = Mutex::new(());

const TONES: [(Mood, &str); 8] = [
    (Mood::Drama, "Drama"),
    (Mood::Romance, "Romance"),
    (Mood::Action, "Action"),
    (Mood::Fight, "Fight"),
    (Mood::Horror, "Horror"),
    (Mood::Comedy, "Comedy"),
    (Mood::Calm, "Calm"),
    (Mood::Mystery, "Mystery"),
];

const KINDS: [(SceneKind, &str); 6] = [
    (SceneKind::Dialogue, "Dialogue"),
    (SceneKind::Action, "Action"),
    (SceneKind::Dread, "Dread"),
    (SceneKind::Narration, "Narration"),
    (SceneKind::Flashback, "Flashback"),
    (SceneKind::Montage, "Montage"),
];

const TIMES: [(TimeOfDay, &str); 5] = [(TimeOfDay::Day, "Day"), (TimeOfDay::Dusk, "Dusk"), (TimeOfDay::Night, "Night"), (TimeOfDay::Dawn, "Dawn"), (TimeOfDay::Indoor, "Indoors")];

const EMOTIONS: [(EmotionKind, &str); 15] = [
    (EmotionKind::Neutral, "Neutral"),
    (EmotionKind::Joy, "Joy"),
    (EmotionKind::Sadness, "Sadness"),
    (EmotionKind::Anger, "Anger"),
    (EmotionKind::Fear, "Fear"),
    (EmotionKind::Surprise, "Surprise"),
    (EmotionKind::Disgust, "Disgust"),
    (EmotionKind::Resolve, "Resolve"),
    (EmotionKind::Pride, "Pride"),
    (EmotionKind::Grief, "Grief"),
    (EmotionKind::Despair, "Despair"),
    (EmotionKind::Relief, "Relief"),
    (EmotionKind::Awe, "Awe"),
    (EmotionKind::Tender, "Tender"),
    (EmotionKind::Menace, "Menace"),
];

const LINE_KINDS: [(LineKind, &str); 4] = [(LineKind::Speech, "Spoken aloud"), (LineKind::Thought, "Thought"), (LineKind::Narration, "Narration"), (LineKind::Sfx, "Sound effect")];

const STRENGTHS: [&str; 3] = ["Mild", "Medium", "Strong"];

fn word<T: PartialEq + Copy>(table: &[(T, &'static str)], v: T) -> &'static str {
    table.iter().find(|(k, _)| *k == v).map(|(_, w)| *w).unwrap_or("Other")
}

fn eid(prefix: &str, key: &str, n: u64) -> ElementId {
    ElementId::NamedInteger(SharedString::from(format!("{prefix}-{key}")), n)
}

// ---------------------------------------------------------------- edits (pure)

/// One edit of the script, applied to the screen's copy at once and to the file's copy when
/// saving. Every edit finds its scene and lines by id.
type Op = Arc<dyn Fn(&mut Story) + Send + Sync>;

fn op(f: impl Fn(&mut Story) + Send + Sync + 'static) -> Op {
    Arc::new(f)
}

/// An edit whose "did anything change" answer is not needed (nothing to do is fine).
fn done<T>(_: T) {}

/// Where the story keeps the AI's own choice of each scene setting the user changed (so Auto can
/// give it back): `{"<scene id>": {"<pointer>": value}}` in the story's extra fields.
const AI_CHOICES: &str = "ai_choices";
/// The scene settings with an Auto chip.
const SETTINGS: [&str; 7] = ["/kind", "/tone", "/fight", "/colour/name", "/time_of_day", "/place", "/target_s"];

fn ai_choices(story: &mut Story) -> Option<&mut serde_json::Map<String, serde_json::Value>> {
    let slot = story.extra.entry(AI_CHOICES.to_string()).or_insert_with(|| serde_json::Value::Object(Default::default()));
    if !slot.is_object() {
        *slot = serde_json::Value::Object(Default::default());
    }
    slot.as_object_mut()
}

/// Sets a scene field and marks it as the user's. The first time a setting becomes the user's,
/// the AI's value is kept so Auto can bring it back.
fn edit_scene(story: &mut Story, id: &str, pointer: &str, f: impl FnOnce(&mut Scene)) -> bool {
    let Some(si) = story.scenes.iter().position(|s| s.id == id) else { return false };
    if SETTINGS.contains(&pointer) && !story.scenes[si].locked.contains(pointer) {
        let ai = serde_json::to_value(&story.scenes[si]).ok().and_then(|v| v.pointer(pointer).cloned());
        if let (Some(ai), Some(all)) = (ai, ai_choices(story)) {
            let per = all.entry(id.to_string()).or_insert_with(|| serde_json::Value::Object(Default::default()));
            if let Some(m) = per.as_object_mut() {
                m.entry(pointer.to_string()).or_insert(ai);
            }
        }
    }
    let s = &mut story.scenes[si];
    f(s);
    s.locked.insert(pointer.to_string());
    s.edited = true;
    true
}

/// "Auto" again: the user's pick and its mark go, the AI's own choice comes back (a place the
/// AI never chose is cleared, "not set"), and adapting again may change it.
fn reset_scene(story: &mut Story, id: &str, pointer: &str) -> bool {
    let Some(si) = story.scenes.iter().position(|s| s.id == id) else { return false };
    let ai = match ai_choices(story).filter(|_| SETTINGS.contains(&pointer)) {
        Some(all) => {
            let kept = all.get_mut(id).and_then(|m| m.as_object_mut()).and_then(|m| m.remove(pointer));
            if all.get(id).and_then(|m| m.as_object()).is_some_and(|m| m.is_empty()) {
                all.remove(id);
            }
            kept
        }
        None => None,
    };
    if story.extra.get(AI_CHOICES).and_then(|v| v.as_object()).is_some_and(|m| m.is_empty()) {
        story.extra.remove(AI_CHOICES);
    }
    let s = &mut story.scenes[si];
    match ai {
        Some(v) => {
            if let Ok(mut cur) = serde_json::to_value(&*s) {
                if let Some(slot) = cur.pointer_mut(pointer) {
                    *slot = v;
                    if let Ok(back) = serde_json::from_value::<Scene>(cur) {
                        *s = back;
                    }
                }
            }
        }
        None if pointer == "/place" => s.place = None,
        None => {}
    }
    s.locked.remove(pointer);
    if pointer == SCENE_LOCK {
        s.locked.remove("/");
    }
    s.edited = !s.locked.is_empty();
    true
}

/// Sets a line field and marks it (and what it changed) as the user's.
fn edit_line(story: &mut Story, id: &str, pointers: &[&str], f: impl FnOnce(&mut Line)) -> bool {
    let Some(l) = story.lines.iter_mut().find(|l| l.id == id) else { return false };
    f(l);
    for p in pointers {
        l.locked.insert(p.to_string());
    }
    l.edited = true;
    true
}

/// The scene was restructured by the user.
fn touch(scene: &mut Scene) {
    scene.edited = true;
    scene.locked.insert(BEATS.to_string());
}

/// (scene index, beat index, position in the beat) of a line.
fn find_line(story: &Story, line: &str) -> Option<(usize, usize, usize)> {
    story.scenes.iter().enumerate().find_map(|(si, s)| s.beats.iter().enumerate().find_map(|(bi, b)| b.lines.iter().position(|l| l == line).map(|k| (si, bi, k))))
}

/// Moves a line one place up or down; past the edge of its beat it goes into the beat before or
/// after (never out of its scene).
fn move_line(story: &mut Story, line: &str, up: bool) -> bool {
    let Some((si, bi, k)) = find_line(story, line) else { return false };
    let scene = &mut story.scenes[si];
    let len = scene.beats[bi].lines.len();
    if up && k > 0 {
        scene.beats[bi].lines.swap(k, k - 1);
    } else if !up && k + 1 < len {
        scene.beats[bi].lines.swap(k, k + 1);
    } else if up && bi > 0 {
        let id = scene.beats[bi].lines.remove(k);
        scene.beats[bi - 1].lines.push(id);
    } else if !up && bi + 1 < scene.beats.len() {
        let id = scene.beats[bi].lines.remove(k);
        scene.beats[bi + 1].lines.insert(0, id);
    } else {
        return false;
    }
    touch(scene);
    sort_lines(story);
    true
}

/// Where a removed line was, to put it back.
#[derive(Clone, Debug, PartialEq)]
struct Removed {
    scene: String,
    beat: String,
    at: usize,
    line: Line,
}

fn remove_line(story: &mut Story, line: &str) -> Option<Removed> {
    let at = story.lines.iter().position(|l| l.id == line)?;
    let removed = story.lines.remove(at);
    let mut out = Removed { scene: removed.scene.clone(), beat: String::new(), at: 0, line: removed };
    if let Some((si, bi, k)) = find_line(story, line) {
        let scene = &mut story.scenes[si];
        scene.beats[bi].lines.remove(k);
        out.scene = scene.id.clone();
        out.beat = scene.beats[bi].id.clone();
        out.at = k;
        touch(scene);
    }
    Some(out)
}

/// Puts a line into a beat at `at` (the scene's last beat when that beat is gone; a new beat
/// when the scene has none). Nothing happens when the scene is gone or the line is there.
fn insert_line(story: &mut Story, scene: &str, beat: &str, at: usize, line: Line, new_beat: &str) -> bool {
    if story.lines.iter().any(|l| l.id == line.id) {
        return false;
    }
    let Some(s) = story.scenes.iter_mut().find(|s| s.id == scene) else { return false };
    if s.beats.is_empty() {
        s.beats.push(Beat { id: new_beat.to_string(), ..Beat::default() });
    }
    let bi = s.beats.iter().position(|b| b.id == beat).unwrap_or(s.beats.len() - 1);
    let b = &mut s.beats[bi];
    b.lines.insert(at.min(b.lines.len()), line.id.clone());
    touch(s);
    let mut line = line;
    line.scene = scene.to_string();
    story.lines.push(line);
    sort_lines(story);
    true
}

/// Splits a scene before beat `at`: that beat and the ones after it become a new scene right
/// after it, with the same settings and a share of the length.
fn split_scene(story: &mut Story, scene: &str, at: usize, new_id: &str) -> bool {
    let Some(si) = story.scenes.iter().position(|s| s.id == scene) else { return false };
    let beats = story.scenes[si].beats.len();
    if at == 0 || at >= beats || story.scenes.iter().any(|x| x.id == new_id) {
        return false;
    }
    let s = &mut story.scenes[si];
    let moved: Vec<Beat> = s.beats.split_off(at);
    let lines_in = |beats: &[Beat]| beats.iter().map(|b| b.lines.len()).sum::<usize>();
    let (stay, go) = (lines_in(&s.beats), lines_in(&moved));
    let share = if stay + go == 0 { 0.5 } else { go as f32 / (stay + go) as f32 };
    let total = s.target_s.max(0.);
    let mut next = Scene { id: new_id.to_string(), title: format!("{} (continued)", s.title.trim()), beats: moved, source: Vec::new(), ..s.clone() };
    next.target_s = ((total * share) * 10.).round() / 10.;
    next.target_s = next.target_s.max(2.);
    s.target_s = (total - next.target_s).max(2.);
    touch(s);
    touch(&mut next);
    let ids: HashSet<String> = next.beats.iter().flat_map(|b| b.lines.iter().cloned()).collect();
    for l in story.lines.iter_mut().filter(|l| ids.contains(&l.id)) {
        l.scene = new_id.to_string();
    }
    story.scenes.insert(si + 1, next);
    sort_lines(story);
    true
}

/// Joins the scene after this one into it.
fn merge_next(story: &mut Story, scene: &str) -> bool {
    let Some(si) = story.scenes.iter().position(|s| s.id == scene) else { return false };
    if si + 1 >= story.scenes.len() {
        return false;
    }
    let next = story.scenes.remove(si + 1);
    for l in story.lines.iter_mut().filter(|l| l.scene == next.id) {
        l.scene = scene.to_string();
    }
    let s = &mut story.scenes[si];
    s.beats.extend(next.beats);
    s.source.extend(next.source);
    s.target_s = s.target_s.max(0.) + next.target_s.max(0.);
    if !next.summary.trim().is_empty() {
        s.summary = format!("{} {}", s.summary.trim(), next.summary.trim()).trim().to_string();
    }
    touch(s);
    sort_lines(story);
    true
}

fn move_scene(story: &mut Story, scene: &str, up: bool) -> bool {
    let Some(si) = story.scenes.iter().position(|s| s.id == scene) else { return false };
    let to = if up { si.checked_sub(1) } else { (si + 1 < story.scenes.len()).then_some(si + 1) };
    let Some(to) = to else { return false };
    story.scenes.swap(si, to);
    sort_lines(story);
    true
}

fn delete_scene(story: &mut Story, scene: &str) -> bool {
    let before = story.scenes.len();
    story.scenes.retain(|s| s.id != scene);
    story.lines.retain(|l| l.scene != scene);
    story.scenes.len() != before
}

/// Lines in script order: scene by scene, each scene's by its beats, then its lines outside any
/// beat; lines of no scene keep their place at the end.
fn sort_lines(story: &mut Story) {
    let mut pool: Vec<Option<Line>> = std::mem::take(&mut story.lines).into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(pool.len());
    for s in &story.scenes {
        for b in &s.beats {
            for id in &b.lines {
                if let Some(slot) = pool.iter_mut().find(|l| l.as_ref().is_some_and(|l| &l.id == id)) {
                    if let Some(mut l) = slot.take() {
                        l.scene = s.id.clone();
                        out.push(l);
                    }
                }
            }
        }
        for slot in pool.iter_mut() {
            if slot.as_ref().is_some_and(|l| l.scene == s.id) {
                out.extend(slot.take());
            }
        }
    }
    out.extend(pool.into_iter().flatten());
    story.lines = out;
}

/// The checks of the adaptation step (rule R3), run again after an edit; issues found while
/// reading the AI's answer (pages it named that do not exist) are kept for scenes still there.
fn refresh_issues(story: &mut Story, known: &BTreeSet<String>) {
    let scenes: HashSet<String> = story.scenes.iter().map(|s| s.id.clone()).collect();
    let kept: Vec<Issue> = engine::issues(story).into_iter().filter(|i| i.code == "unknown_page" && scenes.contains(&i.scene)).collect();
    let mut list = engine::review(story, known);
    list.extend(kept);
    let mut seen = HashSet::new();
    list.retain(|i| seen.insert((i.code.clone(), i.scene.clone(), i.message.clone())));
    story.extra.insert(engine::ISSUES.to_string(), serde_json::to_value(list).unwrap_or_else(|_| serde_json::Value::Array(Vec::new())));
}

/// The scene is shorter than its lines need, as the screen shows both (whole seconds).
fn too_short(length_s: f32, need_s: f32) -> bool {
    let whole = |t: f32| if t.is_finite() { t.max(0.).round() } else { 0. };
    whole(need_s) > whole(length_s)
}

/// What "Adapt again" keeps and writes again; `replaced` = scenes with planned shots that would
/// be written again, when known.
fn adapt_again_text(replaced: Option<usize>) -> String {
    let base = "Scenes you edited, locked, planned or drew stay as they are. The other scenes are written again from the manga.";
    match replaced {
        Some(0) | None => base.to_string(),
        Some(1) => format!("{base} 1 scene with planned shots is written again, so its shots need planning again."),
        Some(n) => format!("{base} {n} scenes with planned shots are written again, so their shots need planning again."),
    }
}

/// A length typed by the user: "2:30", "150", "150 s", "2 min". Between 2 s and 15 min.
fn parse_length(text: &str) -> Option<f32> {
    let t = text.trim().to_lowercase();
    let secs = if let Some((m, s)) = t.split_once(':') {
        let (m, s) = (m.trim().parse::<f32>().ok()?, s.trim().parse::<f32>().ok()?);
        if !(0. ..60.).contains(&s) {
            return None;
        }
        m * 60. + s
    } else if let Some(m) = t.strip_suffix("min").or_else(|| t.strip_suffix('m')) {
        m.trim().parse::<f32>().ok()? * 60.
    } else {
        t.trim_end_matches('s').trim().parse::<f32>().ok()?
    };
    (secs.is_finite() && (2. ..=900.).contains(&secs)).then(|| (secs * 10.).round() / 10.)
}

// ---------------------------------------------------------------- jobs (pure)

/// Where the latest job of a kind stands for this screen.
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
}

fn job_busy(q: &Queue, project: &str, want: impl Fn(&JobKind) -> bool) -> Busy {
    let Some(j) = q.jobs.iter().filter(|j| j.project == project && want(&j.kind) && !(j.state == JobState::Failed && j.hidden)).max_by_key(|j| j.id) else { return Busy::Idle };
    // A waiting job of a paused queue is paused, and a "running" one left from a closed app is
    // held until Resume: only a job that really runs shows a spinner.
    match j.state {
        JobState::Queued if q.paused || q.interrupted => Busy::Waiting { id: j.id, paused: true },
        JobState::Running if q.interrupted => Busy::Waiting { id: j.id, paused: true },
        JobState::Queued => Busy::Waiting { id: j.id, paused: false },
        JobState::Paused => Busy::Waiting { id: j.id, paused: true },
        JobState::Running => Busy::Running { id: j.id, progress: j.progress, line: j.line.clone() },
        JobState::Failed => Busy::Failed { id: j.id, error: j.error.clone().filter(|e| !e.trim().is_empty()).unwrap_or_else(|| j.line.clone()) },
        JobState::Done => Busy::Idle,
    }
}

fn is_story_job(k: &JobKind, ep: &str) -> bool {
    matches!(k, JobKind::Story { episode } if episode == ep)
}

fn is_scene_job(k: &JobKind, ep: &str, scene: &str) -> bool {
    matches!(k, JobKind::AdaptScene { episode, scene: s } if episode == ep && s == scene)
}

/// The AI waits for a usage limit to reset (the job's status line says so).
fn waits_for_limit(line: &str) -> bool {
    line.contains("usage limit") || line.contains("Waiting about")
}

// ---------------------------------------------------------------- data

/// A saved earlier version of the script.
#[derive(Clone, Debug)]
struct Version {
    ms: i64,
    scenes: usize,
    lines: usize,
    /// What changed after it, in words ("line edited in scene 3").
    change: String,
}

/// Two versions tell the same script (the checks' findings aside).
fn same_script(a: &Story, b: &Story) -> bool {
    a.scenes == b.scenes && a.lines == b.lines
}

/// What changed from `before` to `after`, in a few words, lower case ("scene 2 locked").
fn what_changed(before: &Story, after: &Story) -> String {
    if before.scenes.is_empty() {
        return "the first adaptation".into();
    }
    let number = |id: &str| after.scenes.iter().position(|s| s.id == id).or_else(|| before.scenes.iter().position(|s| s.id == id)).map_or(0, |i| i + 1);
    let added = after.scenes.iter().filter(|s| !before.scenes.iter().any(|b| b.id == s.id)).count();
    let removed: Vec<&Scene> = before.scenes.iter().filter(|s| !after.scenes.iter().any(|a| a.id == s.id)).collect();
    if after.made_at != before.made_at && after.made_at > 0 {
        // Going back in time: an earlier version was restored.
        if after.made_at < before.made_at {
            return "an earlier version was put back".into();
        }
        // Every scene but one is as it was: that one scene was adapted again.
        if before.scenes.len() == after.scenes.len() {
            let lines = |s: &Story, id: &str| s.lines.iter().filter(|l| l.scene == id).cloned().collect::<Vec<Line>>();
            let differ: Vec<usize> = before.scenes.iter().zip(&after.scenes).enumerate().filter(|(_, (b, a))| b != a || lines(before, &b.id) != lines(after, &a.id)).map(|(i, _)| i).collect();
            if let [i] = differ.as_slice() {
                return format!("scene {} was adapted again", i + 1);
            }
        }
        return "the AI adapted the chapters again".into();
    }
    match (added, removed.len()) {
        (0, 0) => {}
        (1, 0) => return "a scene added".into(),
        (n, 0) => return format!("{n} scenes added"),
        (0, 1) => {
            let gone = removed[0];
            // Its lines live on in another scene: the two scenes were joined, not one deleted.
            let by_lines = before.lines.iter().filter(|l| l.scene == gone.id).find_map(|l| after.lines.iter().find(|a| a.id == l.id && a.scene != gone.id)).map(|a| a.scene.clone());
            let by_beats = || after.scenes.iter().find(|s| s.beats.iter().any(|b| !b.id.is_empty() && gone.beats.iter().any(|g| g.id == b.id))).map(|s| s.id.clone());
            let joined_into = by_lines.or_else(by_beats);
            let name = |s: &Scene, n: usize| {
                let t = s.title.trim();
                if t.is_empty() {
                    format!("scene {n}")
                } else {
                    format!("scene \u{201c}{t}\u{201d}")
                }
            };
            if let Some(into) = joined_into {
                let kept = after.scenes.iter().find(|s| s.id == into);
                return match kept {
                    Some(k) => format!("{} joined with {}", name(k, number(&k.id)), name(gone, number(&gone.id))),
                    None => format!("{} joined with the scene before it", name(gone, number(&gone.id))),
                };
            }
            return format!("{} deleted", name(gone, number(&gone.id)));
        }
        (0, n) => return format!("{n} scenes deleted"),
        _ => return "scenes split or joined".into(),
    }
    let order = |s: &Story| s.scenes.iter().map(|x| x.id.clone()).collect::<Vec<_>>();
    if order(before) != order(after) {
        return "scenes moved".into();
    }
    for b in &before.scenes {
        let Some(a) = after.scenes.iter().find(|x| x.id == b.id) else { continue };
        let n = number(&b.id);
        let lock = |s: &Scene| s.locked.contains(SCENE_LOCK) || s.locked.contains("/");
        if lock(b) != lock(a) {
            return format!("scene {n} {}", if lock(a) { "locked" } else { "unlocked" });
        }
        if b.title != a.title {
            return format!("scene {n} renamed");
        }
        if b.summary != a.summary {
            return format!("scene {n}'s summary edited");
        }
        let setting = [
            (b.kind != a.kind, "kind"),
            (b.tone != a.tone, "tone"),
            (b.fight != a.fight, "fight"),
            (b.colour != a.colour, "colour"),
            (b.time_of_day != a.time_of_day, "time of day"),
            (b.place != a.place, "place"),
            (b.target_s != a.target_s, "length"),
        ]
        .into_iter()
        .find_map(|(changed, word)| changed.then_some(word));
        if let Some(word) = setting {
            return format!("scene {n}'s {word} changed");
        }
        let lines_of = |s: &Story| s.lines.iter().filter(|l| l.scene == b.id).cloned().collect::<Vec<Line>>();
        let (lb, la) = (lines_of(before), lines_of(after));
        if la.len() > lb.len() {
            return format!("a line added to scene {n}");
        }
        if la.len() < lb.len() {
            return format!("a line removed from scene {n}");
        }
        if lb.iter().zip(&la).any(|(x, y)| x.spoken != y.spoken) {
            return format!("a line edited in scene {n}");
        }
        if lb != la {
            return format!("a line's speaker or feeling changed in scene {n}");
        }
        if b.beats != a.beats {
            return format!("scene {n}'s beats changed");
        }
    }
    "a small change".into()
}

/// When a version was kept: relative within the hour, else its date and time.
fn version_time(ms: i64, now: i64) -> String {
    let secs = ms / 1000;
    if now - secs < 3600 {
        ago(secs, now)
    } else {
        mv_anime::store::local_when(secs)
    }
}

#[derive(Clone)]
struct Data {
    story: Story,
    episode: Episode,
    /// (character id, name), by name.
    cast: Vec<(String, String)>,
    known: BTreeSet<String>,
    /// (place id, name).
    places: Vec<(String, String)>,
    /// (chapter label, fully read).
    reads: Vec<(String, bool)>,
    next_ids: BTreeMap<String, u32>,
}

impl Data {
    fn name_of(&self, id: &str) -> String {
        self.cast.iter().find(|(c, _)| c == id).map(|(_, n)| n.clone()).unwrap_or_else(|| "Someone not in the cast".to_string())
    }

    fn ids(&self) -> Ids {
        Ids::new(&self.next_ids, &self.story)
    }
}

fn story_rel(ep: &str) -> String {
    format!("episodes/{}/story.json", mv_core::paths::safe_name(ep))
}

/// Reads the script and what it refers to. Blocking.
fn load(store: &Store, ep: &str) -> anyhow::Result<Data> {
    let series = store.series().context("The project could not be read")?;
    let episode = store.episode(ep).context("The episode could not be read")?;

    let story = store.story(ep).context("The script could not be read")?;
    let bible = store.bible().unwrap_or_else(|e| {
        mv_core::logging::global().warn("anime", &format!("story step: the bible could not be read: {e:#}"));
        Default::default()
    });
    let manga_id = if series.manga_id.trim().is_empty() { episode.chapters.first().map(|c| c.manga_id.clone()).unwrap_or_default() } else { series.manga_id.clone() };
    let brain = source::brain(&manga_id, &series.manga_title);
    let people = engine::Cast::new(&bible, &brain);
    let known = people.ids();
    let mut cast: Vec<(String, String)> = known.iter().map(|id| (id.clone(), people.name_of(id))).collect();
    cast.sort_by_key(|(_, n)| n.to_lowercase());
    let places = bible.places.iter().map(|p| (p.id.clone(), if p.name.trim().is_empty() { p.id.clone() } else { p.name.clone() })).collect();
    let reads = episode
        .chapters
        .iter()
        .map(|c| {
            let (read, pages) = source::read_progress(c, &series.manga_title);
            let label = if c.label.trim().is_empty() { c.chapter_id.clone() } else { c.label.clone() };
            (label, pages > 0 && read >= pages)
        })
        .collect();
    Ok(Data { story, episode, cast, known, places, reads, next_ids: series.next_ids })
}

/// The script's earlier versions, newest first, each with what changed after it. A version that
/// tells the same script as the one after it is left out (restoring it would change nothing);
/// one that cannot be read is left out too. Blocking.
fn load_versions(store: &Store, ep: &str) -> Vec<Version> {
    let rel = story_rel(ep);
    let dir = store.dir.join("history").join(rel.replace('/', "__"));
    let mut newer: Option<Story> = store.story(ep).ok();
    let mut out = Vec::new();
    for ms in store.history(&rel) {
        let Some(old) = std::fs::read(dir.join(format!("{ms}.json"))).ok().and_then(|b| serde_json::from_slice::<Story>(&b).ok()) else { continue };
        if newer.as_ref().is_some_and(|n| same_script(n, &old)) {
            continue;
        }
        let change = match &newer {
            Some(n) => what_changed(&old, n),
            None => "the latest change".into(),
        };
        out.push(Version { ms, scenes: old.scenes.len(), lines: old.lines.len(), change });
        newer = Some(old);
    }
    out
}

/// Applies `ops` to the file's script and saves it, with the checks run again and the project's
/// id counters raised. Blocking.
fn save_ops(store: &Store, ep: &str, ops: &[Op], known: &BTreeSet<String>) -> anyhow::Result<Story> {
    let _guard = SAVE.lock().unwrap_or_else(|e| e.into_inner());
    // Read, changed and written under the story file's own lock, so a voice batch finishing (or
    // a Sound edit) at the same moment keeps its change and this one is kept too.
    let story = store.update_story(ep, |story| {
        for o in ops {
            o(story);
        }
        refresh_issues(story, known);
    })?;
    // The project's id counters follow, raised under the counters lock on a fresh read of the
    // project file (written only when they move, since every write makes the screens read again).
    let needed = |counters: &BTreeMap<String, u32>| {
        let mut raised = counters.clone();
        Ids::new(counters, &story).store_in(&mut raised);
        raised != *counters
    };
    if store.series().map(|s| needed(&s.next_ids)).unwrap_or(true) {
        if let Err(e) = engine::bump_ids(store, |c| Ids::new(c, &story).store_in(c)) {
            mv_core::logging::global().warn("anime", &format!("story step: id counters not saved: {e:#}"));
        }
    }
    Ok(story)
}

// ---------------------------------------------------------------- the view

/// What the open text box edits.
#[derive(Clone, Debug, PartialEq)]
enum Field {
    Title(String),
    Summary(String),
    Purpose { scene: String, beat: String },
    Spoken(String),
}

struct Editing {
    field: Field,
    input: Entity<TextareaState>,
    /// A line just added: left empty, it goes away again.
    fresh: bool,
    multi: bool,
    _sub: Subscription,
}

/// The selected scene's length box.
struct LengthBox {
    scene: String,
    /// The length the box shows (it follows the scene while not typed in).
    shown: f32,
    input: Entity<InputState>,
    _sub: Subscription,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Panel {
    Scene,
    Versions,
}

pub struct StoryStep {
    ctx: StepCtx,
    store: Store,
    data: Load<Data>,
    queue: Queue,
    seen: [u64; 2],
    selected: Option<String>,
    editing: Option<Editing>,
    length: Option<LengthBox>,
    panel: Panel,
    /// Narrow window: the scene settings drawer is open.
    drawer: bool,
    /// The window was narrow at the last render.
    was_narrow: bool,
    versions: Option<Load<Vec<Version>>>,
    pending: Vec<Op>,
    saving: bool,
    stale: bool,
    /// The project's files changed: read them again when this view is next drawn (a step that
    /// is not on screen never re-reads).
    refresh: bool,
    loading: bool,
    again: bool,
    scroll: ScrollHandle,
}

pub fn open(ctx: StepCtx, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| {
        let mut s = StoryStep {
            store: Store::open(&ctx.project),
            ctx,
            data: Load::Loading,
            queue: queue::snapshot(),
            seen: versions(),
            selected: None,
            editing: None,
            length: None,
            panel: Panel::Scene,
            drawer: false,
            was_narrow: false,
            versions: None,
            pending: Vec::new(),
            saving: false,
            stale: false,
            refresh: false,
            loading: false,
            again: false,
            scroll: ScrollHandle::new(),
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

impl StoryStep {
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
                        let still = this.selected.as_ref().is_some_and(|id| d.story.scenes.iter().any(|s| &s.id == id));
                        if !still {
                            this.selected = d.story.scenes.first().map(|s| s.id.clone());
                        }
                        this.data = Load::Ready(d);
                    }
                    Err(e) => {
                        if !matches!(this.data, Load::Ready(_)) {
                            this.data = Load::Failed("The script could not be read. Check that the project's drive is connected, then try again.".into());
                        }
                        mv_core::logging::global().warn("anime", &format!("story step: {e:#}"));
                    }
                }
                if std::mem::take(&mut this.again) {
                    this.reload(cx);
                }
                if this.panel == Panel::Versions {
                    this.load_versions(cx);
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

    fn load_versions(&mut self, cx: &mut Context<Self>) {
        if matches!(self.versions, Some(Load::Loading)) {
            return;
        }
        if self.versions.is_none() {
            self.versions = Some(Load::Loading);
        }
        let (store, ep) = (self.store.clone(), self.ctx.episode.clone());
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || load_versions(&store, &ep)).await;
            let _ = this.update(cx, |this, cx| {
                this.versions = Some(match res {
                    Ok(v) => Load::Ready(v),
                    Err(e) => {
                        mv_core::logging::global().warn("anime", &format!("story step: earlier versions not listed: {e:#}"));
                        Load::Failed("The earlier versions could not be listed. Try again in a moment.".into())
                    }
                });
                cx.notify();
            });
        })
        .detach();
    }

    // ------------------------------------------------------------ saving

    /// Applies an edit to the screen now and saves it in the background.
    fn apply(&mut self, edit: Op, cx: &mut Context<Self>) {
        let Load::Ready(d) = &mut self.data else { return };
        edit(&mut d.story);
        refresh_issues(&mut d.story, &d.known);
        self.pending.push(edit);
        self.flush(cx);
        cx.notify();
    }

    fn flush(&mut self, cx: &mut Context<Self>) {
        if self.saving || self.pending.is_empty() {
            return;
        }
        let Load::Ready(d) = &self.data else { return };
        let ops = std::mem::take(&mut self.pending);
        self.saving = true;
        let (store, ep, known) = (self.store.clone(), self.ctx.episode.clone(), d.known.clone());
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || save_ops(&store, &ep, &ops, &known)).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
                match res {
                    Ok(story) => {
                        if this.pending.is_empty() {
                            if let Load::Ready(d) = &mut this.data {
                                d.story = story;
                            }
                        }
                    }
                    Err(e) => {
                        mv_core::logging::global().warn("anime", &format!("story step: save failed: {e:#}"));
                        toast::show(cx, Toast::error(SAVE_FAILED));
                        this.pending.clear();
                        this.stale = true;
                    }
                }
                this.flush(cx);
                if !this.saving && this.pending.is_empty() && std::mem::take(&mut this.stale) {
                    this.reload(cx);
                }
                if this.panel == Panel::Versions {
                    this.load_versions(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ------------------------------------------------------------ jobs

    fn adapt_all(&mut self, cx: &mut Context<Self>) {
        self.commit(cx);
        let title = match &self.data {
            Load::Ready(d) if !d.episode.title.trim().is_empty() => d.episode.title.clone(),
            _ => self.ctx.episode.clone(),
        };
        crate::backends::ensure_runner(cx);
        queue::enqueue(JobKind::Story { episode: self.ctx.episode.clone() }, &self.ctx.project, format!("Adapt {title}"));
        self.queue = queue::snapshot();
        cx.notify();
    }

    /// "Adapt again" on an episode that already has scenes: says first what stays and what is
    /// written again, then adapts.
    fn confirm_adapt_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit(cx);
        let Load::Ready(d) = &self.data else { return };
        if d.story.scenes.is_empty() {
            self.adapt_all(cx);
            return;
        }
        // How many planned scenes would be written again is read from disk off the UI thread;
        // the question opens when it is known.
        let (store, ep) = (self.store.clone(), self.ctx.episode.clone());
        cx.spawn_in(window, async move |this, cx| {
            let replaced = mv_ui::bg::blocking(move || engine::replaced_with_shots(&store, &ep)).await.ok();
            let _ = this.update_in(cx, |_, window, cx| {
                let weak = cx.entity().downgrade();
                confirm_dialog("Adapt the chapters again?", adapt_again_text(replaced), "Adapt again", false)
                    .cancel_label("Keep the script")
                    .on_confirm(move |_, cx| {
                        let _ = weak.update(cx, |s, cx| s.adapt_all(cx));
                    })
                    .open(window, cx);
            });
        })
        .detach();
    }

    fn adapt_scene(&mut self, scene: String, number: usize, cx: &mut Context<Self>) {
        self.commit(cx);
        crate::backends::ensure_runner(cx);
        queue::enqueue(JobKind::AdaptScene { episode: self.ctx.episode.clone(), scene }, &self.ctx.project, format!("Adapt scene {number} again"));
        self.queue = queue::snapshot();
        cx.notify();
    }

    fn story_busy(&self) -> Busy {
        let ep = self.ctx.episode.clone();
        job_busy(&self.queue, &self.ctx.project, |k| is_story_job(k, &ep))
    }

    fn scene_busy(&self, scene: &str) -> Busy {
        let ep = self.ctx.episode.clone();
        job_busy(&self.queue, &self.ctx.project, |k| is_scene_job(k, &ep, scene))
    }

    // ------------------------------------------------------------ editing text

    fn edit(&mut self, field: Field, current: String, fresh: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.commit(cx);
        let multi = matches!(field, Field::Summary(_));
        let placeholder = match field {
            Field::Title(_) => "Scene title",
            Field::Summary(_) => "What happens in this scene",
            Field::Purpose { .. } => "What this beat does",
            Field::Spoken(_) => "What is said",
        };
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, if multi { 8 } else { 4 }).submit_on_enter(!multi).placeholder(placeholder));
        input.update(cx, |s, cx| {
            s.set_value(current, window, cx);
            s.focus(window, cx);
        });
        let sub = cx.subscribe_in(&input, window, move |this, _, ev: &InputEvent, _, cx| {
            let done = match ev {
                InputEvent::Blur => true,
                InputEvent::PressEnter { shift, .. } => !multi && !*shift,
                _ => false,
            };
            if done {
                this.commit(cx);
            }
        });
        self.editing = Some(Editing { field, input, fresh, multi, _sub: sub });
        cx.notify();
    }

    /// Saves the open text box, if any.
    fn commit(&mut self, cx: &mut Context<Self>) {
        let Some(e) = self.editing.take() else { return };
        let raw = e.input.read(cx).value().to_string();
        let text = if e.multi { raw.trim().to_string() } else { raw.split_whitespace().collect::<Vec<_>>().join(" ") };
        cx.notify();
        let Load::Ready(d) = &self.data else { return };
        match e.field {
            Field::Title(id) => {
                let same = d.story.scenes.iter().find(|s| s.id == id).is_some_and(|s| s.title.trim() == text);
                if !same && !text.is_empty() {
                    self.apply(op(move |s| done(edit_scene(s, &id, "/title", |sc| sc.title = text.clone()))), cx);
                }
            }
            Field::Summary(id) => {
                if d.story.scenes.iter().find(|s| s.id == id).is_some_and(|s| s.summary.trim() != text) {
                    self.apply(op(move |s| done(edit_scene(s, &id, "/summary", |sc| sc.summary = text.clone()))), cx);
                }
            }
            Field::Purpose { scene, beat } => {
                let current = d.story.scenes.iter().find(|s| s.id == scene).and_then(|s| s.beats.iter().find(|b| b.id == beat)).map(|b| b.purpose.trim().to_string());
                if current.is_some_and(|c| c != text) {
                    self.apply(
                        op(move |s| {
                            let Some(sc) = s.scenes.iter_mut().find(|x| x.id == scene) else { return };
                            if let Some(b) = sc.beats.iter_mut().find(|b| b.id == beat) {
                                b.purpose = text.clone();
                                touch(sc);
                            }
                        }),
                        cx,
                    );
                }
            }
            Field::Spoken(id) => {
                if e.fresh && text.is_empty() {
                    self.apply(op(move |s| done(remove_line(s, &id))), cx);
                    return;
                }
                if d.story.lines.iter().find(|l| l.id == id).is_some_and(|l| l.spoken.trim() != text) {
                    self.apply(
                        op(move |s| {
                            edit_line(s, &id, &["/spoken"], |l| {
                                l.spoken = text.clone();
                                // Changed words are no longer the manga's.
                                if l.keep_words && l.spoken.trim() != l.original.trim() {
                                    l.keep_words = false;
                                }
                            });
                        }),
                        cx,
                    );
                }
            }
        }
    }

    fn is_editing(&self, f: &Field) -> Option<Entity<TextareaState>> {
        self.editing.as_ref().filter(|e| &e.field == f).map(|e| e.input.clone())
    }

    /// The length box of the selected scene. While it is not typed in it follows the scene's
    /// length (after a join, an adaptation or Auto), so it never holds an old length.
    fn ensure_length(&mut self, scene: &Scene, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(b) = self.length.as_mut().filter(|b| b.scene == scene.id) {
            let focused = b.input.read(cx).focus_handle(cx).contains_focused(window, cx);
            if !focused && (b.shown - scene.target_s).abs() >= 0.05 {
                b.shown = scene.target_s;
                let value = fmt_secs(scene.target_s);
                b.input.update(cx, |s, cx| s.set_value(value, window, cx));
            }
            return b.input.clone();
        }
        // Another scene: what was typed in the old box is saved first.
        self.drop_length(cx);
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("2:30"));
        let value = fmt_secs(scene.target_s);
        input.update(cx, |s, cx| s.set_value(value, window, cx));
        let sub = cx.subscribe_in(&input, window, move |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                this.commit_length(Some(window), cx);
            }
        });
        self.length = Some(LengthBox { scene: scene.id.clone(), shown: scene.target_s, input: input.clone(), _sub: sub });
        input
    }

    /// Saves a length typed in the box, when it was changed and differs from the scene's.
    /// With a window, a length that cannot be read is put back and explained.
    fn commit_length(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        let Some(b) = &self.length else { return };
        let (id, input) = (b.scene.clone(), b.input.clone());
        let text = input.read(cx).value().to_string();
        // Untouched: the box still shows the length it was given.
        if text.trim() == fmt_secs(b.shown) {
            return;
        }
        let current = match &self.data {
            Load::Ready(d) => d.story.scenes.iter().find(|s| s.id == id).map(|s| s.target_s),
            _ => None,
        };
        let Some(current) = current else { return };
        match parse_length(&text) {
            Some(v) if (v - current).abs() >= 0.05 => {
                self.apply(op(move |s| done(edit_scene(s, &id, "/target_s", |sc| sc.target_s = v))), cx);
            }
            Some(_) => {}
            None => {
                if let Some(window) = window {
                    toast::show(cx, Toast::warning("Type a length like 2:30 or 90, between 2 seconds and 15 minutes."));
                    input.update(cx, |s, cx| s.set_value(fmt_secs(current), window, cx));
                }
            }
        }
    }

    /// Closes the length box, saving a length typed in it first.
    fn drop_length(&mut self, cx: &mut Context<Self>) {
        self.commit_length(None, cx);
        self.length = None;
    }

    // ------------------------------------------------------------ scene and line actions

    fn pick<T: Clone + Send + Sync + 'static>(&mut self, scene: &str, pointer: &'static str, v: Option<T>, set: fn(&mut Scene, T), cx: &mut Context<Self>) {
        let id = scene.to_string();
        let edit = match v {
            None => op(move |s| done(reset_scene(s, &id, pointer))),
            Some(v) => op(move |s| done(edit_scene(s, &id, pointer, |sc| set(sc, v.clone())))),
        };
        self.apply(edit, cx);
        if pointer == "/target_s" {
            self.length = None;
        }
    }

    fn set_lock(&mut self, scene: String, on: bool, cx: &mut Context<Self>) {
        let edit = if on { op(move |s| done(edit_scene(s, &scene, SCENE_LOCK, |_| {}))) } else { op(move |s| done(reset_scene(s, &scene, SCENE_LOCK))) };
        self.apply(edit, cx);
    }

    fn add_scene(&mut self, cx: &mut Context<Self>) {
        self.commit(cx);
        self.drop_length(cx);
        let Load::Ready(d) = &self.data else { return };
        let mut ids = d.ids();
        let (id, beat) = (ids.scene(), ids.beat());
        let after = self.selected.clone();
        let scene = Scene {
            id: id.clone(),
            title: "New scene".into(),
            target_s: 10.,
            beats: vec![Beat { id: beat, ..Beat::default() }],
            edited: true,
            locked: [BEATS.to_string(), "/title".to_string()].into_iter().collect(),
            ..Scene::default()
        };
        self.apply(
            op(move |s| {
                if s.scenes.iter().any(|x| x.id == scene.id) {
                    return;
                }
                let at = after.as_ref().and_then(|a| s.scenes.iter().position(|x| &x.id == a)).map_or(s.scenes.len(), |i| i + 1);
                s.scenes.insert(at, scene.clone());
            }),
            cx,
        );
        self.selected = Some(id);
    }

    fn add_line(&mut self, scene: String, beat: String, window: &mut Window, cx: &mut Context<Self>) {
        self.commit(cx);
        let Load::Ready(d) = &self.data else { return };
        let mut ids = d.ids();
        let (lid, bid) = (ids.line(), ids.beat());
        let at = d.story.scenes.iter().find(|s| s.id == scene).and_then(|s| s.beats.iter().find(|b| b.id == beat)).map_or(usize::MAX, |b| b.lines.len());
        let line = Line {
            id: lid.clone(),
            scene: scene.clone(),
            kind: LineKind::Narration,
            emotion: Emotion { kind: EmotionKind::Neutral, strength: 1 },
            edited: true,
            locked: ["/spoken".to_string()].into_iter().collect(),
            ..Line::default()
        };
        self.apply(op(move |s| done(insert_line(s, &scene, &beat, at, line.clone(), &bid))), cx);
        self.edit(Field::Spoken(lid), String::new(), true, window, cx);
    }

    fn add_beat(&mut self, scene: String, cx: &mut Context<Self>) {
        self.commit(cx);
        let Load::Ready(d) = &self.data else { return };
        let bid = d.ids().beat();
        self.apply(
            op(move |s| {
                if let Some(sc) = s.scenes.iter_mut().find(|x| x.id == scene) {
                    if !sc.beats.iter().any(|b| b.id == bid) {
                        sc.beats.push(Beat { id: bid.clone(), ..Beat::default() });
                        touch(sc);
                    }
                }
            }),
            cx,
        );
    }

    fn delete_beat(&mut self, scene: String, beat: String, cx: &mut Context<Self>) {
        self.apply(
            op(move |s| {
                if let Some(sc) = s.scenes.iter_mut().find(|x| x.id == scene) {
                    // Its lines join the beat before it (or after it), never vanish.
                    let Some(bi) = sc.beats.iter().position(|b| b.id == beat) else { return };
                    if sc.beats.len() < 2 {
                        return;
                    }
                    let b = sc.beats.remove(bi);
                    let to = bi.saturating_sub(1).min(sc.beats.len() - 1);
                    if bi == 0 {
                        let mut lines = b.lines;
                        lines.extend(std::mem::take(&mut sc.beats[0].lines));
                        sc.beats[0].lines = lines;
                    } else {
                        sc.beats[to].lines.extend(b.lines);
                    }
                    touch(sc);
                }
                sort_lines(s);
            }),
            cx,
        );
    }

    fn delete_line(&mut self, line: String, cx: &mut Context<Self>) {
        let Load::Ready(d) = &self.data else { return };
        let mut copy = d.story.clone();
        let Some(removed) = remove_line(&mut copy, &line) else { return };
        let bid = d.ids().beat();
        self.apply(op(move |s| done(remove_line(s, &line))), cx);
        let this = cx.entity().downgrade();
        toast::show(
            cx,
            Toast::info("Line removed.").icon(ph::TRASH).action("Undo", move |_, cx| {
                let r = removed.clone();
                let bid = bid.clone();
                let _ = this.update(cx, |s, cx| s.apply(op(move |st| done(insert_line(st, &r.scene, &r.beat, r.at, r.line.clone(), &bid))), cx));
            }),
        );
    }

    fn confirm_delete_scene(&mut self, scene: String, number: usize, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        let body = format!(
            "Its beats and lines leave the script. Earlier versions keep the script as it was before each of your last {} changes, so you can bring it back from there for a while.",
            mv_anime::store::HISTORY_KEEP
        );
        confirm_dialog(format!("Delete scene {number}?"), body, "Delete scene", true)
            .cancel_label("Keep it")
            .on_confirm(move |_, cx| {
                let id = scene.clone();
                let _ = this.update(cx, |s, cx| {
                    s.apply(op(move |st| done(delete_scene(st, &id))), cx);
                    if let Load::Ready(d) = &s.data {
                        if !s.selected.as_ref().is_some_and(|sel| d.story.scenes.iter().any(|x| &x.id == sel)) {
                            s.selected = d.story.scenes.first().map(|x| x.id.clone());
                        }
                    }
                });
            })
            .open(window, cx);
    }

    fn confirm_restore(&mut self, v: Version, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        let when = version_time(v.ms, now_secs());
        confirm_dialog(
            "Restore this version?",
            format!(
                "The script goes back to how it was before {} ({when}: {}, {}). The version you have now is kept as an earlier version, so you can come back to it.",
                v.change,
                count(v.scenes, "scene"),
                count(v.lines, "line")
            ),
            "Restore",
            false,
        )
        .on_confirm(move |_, cx| {
            let ms = v.ms;
            let _ = this.update(cx, |s, cx| s.restore(ms, cx));
        })
        .open(window, cx);
    }

    fn restore(&mut self, ms: i64, cx: &mut Context<Self>) {
        self.commit(cx);
        let (store, ep) = (self.store.clone(), self.ctx.episode.clone());
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || {
                let _guard = SAVE.lock().unwrap_or_else(|e| e.into_inner());
                store.restore(&story_rel(&ep), ms)
            })
            .await
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(()) => {
                        toast::show(cx, Toast::success("The earlier version is back."));
                        this.length = None;
                        this.reload(cx);
                    }
                    Err(e) => {
                        mv_core::logging::global().warn("anime", &format!("story step: restore failed: {e:#}"));
                        toast::show(cx, Toast::error("That version could not be brought back. Check that the project's drive is connected, then try again."));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn show_versions(&mut self, on: bool, cx: &mut Context<Self>) {
        self.panel = if on { Panel::Versions } else { Panel::Scene };
        // In a narrow window the drawer opens for the versions and closes with them; a wide
        // window shows the panel anyway.
        self.drawer = on;
        if on {
            self.load_versions(cx);
        }
        cx.notify();
    }
}

// ---------------------------------------------------------------- render

impl Render for StoryStep {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.refresh) {
            if self.saving || !self.pending.is_empty() {
                self.stale = true;
            } else {
                self.reload(cx);
            }
        }
        let d = match &self.data {
            Load::Loading => return self.skeleton(window, None, cx).into_any_element(),
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
        let busy = self.story_busy();
        if d.story.scenes.is_empty() {
            return self.empty(&d, &busy, window, cx).into_any_element();
        }
        let narrow = is_narrow(window);
        // Becoming narrow closes the drawer: it opens over the script only when asked.
        if narrow && !self.was_narrow {
            self.drawer = false;
        }
        self.was_narrow = narrow;
        let list = self.list(&d, &busy, cx);
        let at = self.selected.as_ref().and_then(|id| d.story.scenes.iter().position(|s| &s.id == id)).unwrap_or(0);
        let scene = d.story.scenes[at].clone();
        let center = self.scene_view(&d, at, &scene, narrow, window, cx);
        let panel = (!narrow || self.drawer).then(|| match self.panel {
            Panel::Scene => self.inspector(&d, &scene, narrow, window, cx).into_any_element(),
            Panel::Versions => self.versions_view(cx).into_any_element(),
        });
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

/// A text box in the screen's frame (focused: strong border).
fn editor(input: &Entity<TextareaState>, window: &Window, cx: &App) -> Div {
    let focused = input.read(cx).focus_handle(cx).contains_focused(window, cx);
    div()
        .flex_1()
        .min_w_0()
        .px(px(space::SM))
        .py(px(space::XS))
        .rounded(px(radius::MD))
        .bg(Tokens::surface())
        .border_1()
        .border_color(if focused { Tokens::text_1() } else { Tokens::border_strong() })
        .child(Textarea::new(input).appearance(false).bordered(false))
}

/// Clickable text that becomes a text box (placeholder in `text_3` when empty).
fn editable(id: ElementId, text: &str, placeholder: &str, size_px: f32, color: Rgba) -> Stateful<Div> {
    let empty = text.trim().is_empty();
    div()
        .id(id)
        .flex_1()
        .min_w_0()
        .px(px(space::XS))
        .rounded(px(radius::SM))
        .cursor_text()
        .hover(|s| s.bg(Tokens::hover()))
        .text_size(px(size_px))
        .line_height(px(size_px * 1.45))
        .text_color(if empty { Tokens::text_3() } else { color })
        .whitespace_normal()
        .child(if empty { placeholder.to_string() } else { text.trim().to_string() })
        .tooltip(tip("Click to change"))
}

fn issue_line(i: &Issue) -> Div {
    let (icon, tone) = match i.severity {
        Severity::Error => (ph::WARNING_CIRCLE, LineTone::Danger),
        Severity::Warning => (ph::WARNING, LineTone::Warning),
        _ => (ph::INFO, LineTone::Info),
    };
    let text = match i.fix.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
        Some(fix) => format!("{} {fix}", i.message.trim()),
        None => i.message.trim().to_string(),
    };
    info_line(icon, tone, text)
}

impl StoryStep {
    fn skeleton(&self, window: &Window, top: Option<Div>, _cx: &mut Context<Self>) -> Div {
        let list = div().flex().flex_col().size_full().child(column_head("Scenes")).child(div().flex().flex_col().gap(px(space::XS)).p(px(space::MD)).children((0..6).map(|_| skeleton_row())));
        let center = div()
            .flex()
            .flex_col()
            .gap(px(space::LG))
            .size_full()
            .p(px(space::LG))
            .children(top)
            .child(skeleton_line(px(240.)))
            .children((0..5).map(|_| div().flex().gap(px(space::MD)).child(skeleton_line(px(SPEAKER_W))).child(skeleton_line(relative(0.6)))));
        workspace(window, Some(list.into_any_element()), center, None)
    }

    /// No scenes yet: one action, or what has to happen first.
    fn empty(&self, d: &Data, busy: &Busy, window: &Window, cx: &mut Context<Self>) -> Div {
        if let Busy::Waiting { paused, .. } = busy {
            // Waiting or paused: a still notice with Resume / Cancel, nothing that moves.
            let title = if *paused { "Adapting is paused" } else { "Waiting to adapt the chapters" };
            let top = div()
                .flex()
                .flex_col()
                .gap(px(space::SM))
                .w_full()
                .max_w(px(560.))
                .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title))
                .children(self.progress("all", busy));
            return div().size_full().flex().flex_col().items_center().justify_center().p(px(space::XXL)).child(top);
        }
        if busy.active() {
            // Working: the list fills in when the first chapter is adapted.
            let top = div()
                .flex()
                .flex_col()
                .gap(px(space::SM))
                .max_w(px(560.))
                .child(div().flex().items_center().gap(px(space::SM)).child(spinner("st-skel-spin", Tokens::accent(), icon_px::SM)).child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Adapting the chapters")))
                .children(self.progress("all", busy));
            return self.skeleton(window, Some(top), cx);
        }
        let go = self.ctx.go.clone();
        let read = d.reads.iter().filter(|(_, r)| *r).count();
        let view = if d.episode.chapters.is_empty() {
            empty_view(ph::NOTE_PENCIL, "No chapters to adapt", "Add the chapters this episode tells in the Manga step first.").action(btn("st-go-manga", "Go to Manga", ButtonKind::Primary).icon(ph::BOOK_OPEN_TEXT).on_click(move |_, w, cx| go(Step::Manga, w, cx)))
        } else if read == 0 {
            empty_view(ph::NOTE_PENCIL, "Read the chapters first", "The script is adapted from Studio's Read of each chapter. Read them in the Manga step, then come back.")
                .action(btn("st-go-manga", "Go to Manga", ButtonKind::Primary).icon(ph::BOOK_OPEN_TEXT).on_click(move |_, w, cx| go(Step::Manga, w, cx)))
        } else {
            let unread: Vec<&str> = d.reads.iter().filter(|(_, r)| !*r).map(|(l, _)| l.as_str()).collect();
            let body = if unread.is_empty() {
                "The AI adapts the read chapters into scenes, beats and lines. You can change everything after.".to_string()
            } else {
                format!("The AI adapts the read chapters into scenes, beats and lines. You can change everything after. {} is not fully read yet, so the adaptation stops there.", unread[0])
            };
            empty_view(ph::NOTE_PENCIL, "No script yet", body).action(btn("st-adapt", "Adapt the chapters", ButtonKind::Primary).icon(ph::SPARKLE).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.adapt_all(cx))))
        };
        let failed = match busy {
            Busy::Failed { id, error } => Some(self.failure(*id, error.clone())),
            _ => None,
        };
        div().size_full().flex().flex_col().items_center().justify_center().gap(px(space::LG)).p(px(space::XXL)).child(view).children(failed.map(|f| div().w_full().max_w(px(560.)).child(f)))
    }

    fn failure(&self, id: u64, error: String) -> Div {
        // Only a reason a person can read is shown; the full error is in the queue's log.
        let text = match plain_reason(&error) {
            Some(why) => format!("Adapting stopped: {why} Try again."),
            None => "Adapting stopped. Try again in a moment.".to_string(),
        };
        error_block(text).child(
            div()
                .flex()
                .flex_none()
                .gap(px(space::XS))
                .child(btn(eid("st-retry", "", id), "Try again", ButtonKind::Secondary).small().icon(ph::ARROW_CLOCKWISE).on_click(move |_, _, cx| {
                    crate::backends::ensure_runner(cx);
                    queue::retry(id);
                }))
                .child(icon_button(eid("st-dismiss", "", id), ph::X, "Dismiss").small().on_click(move |_, _, _| queue::cancel(id))),
        )
    }

    /// Progress of a running or waiting job, with Cancel (and Try now while the AI waits).
    fn progress(&self, key: &str, busy: &Busy) -> Option<Div> {
        match busy {
            Busy::Running { id, progress, line } => {
                let id = *id;
                let waiting = waits_for_limit(line);
                let line = if line.trim().is_empty() { "Adapting…".to_string() } else { line.clone() };
                Some(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(space::XS))
                        .child(progress_bar(eid("st-progress", key, 0), *progress, !waiting))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::SM))
                                .child(div().flex_1().min_w_0().text_size(px(size::XS)).text_color(if waiting { Tokens::warning() } else { Tokens::text_2() }).whitespace_normal().child(line))
                                .when(waiting, |d| d.child(btn(eid("st-now", key, id), "Try now", ButtonKind::Secondary).small().on_click(|_, _, _| mv_anime::llm::retry_now())))
                                .child(btn(eid("st-cancel", key, id), "Cancel", ButtonKind::Ghost).small().on_click(move |_, _, _| queue::cancel(id))),
                        ),
                )
            }
            Busy::Waiting { id, paused } => {
                let id = *id;
                let paused = *paused;
                Some(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .child(icon_ph(if paused { ph::PAUSE_CIRCLE } else { ph::HOURGLASS }, Tokens::text_2(), icon_px::SM))
                        // The one place to resume is the queue strip at the bottom of the tab.
                        .child(div().flex_1().text_size(px(size::XS)).text_color(Tokens::text_2()).child(if paused { "Waiting: the queue is paused. Resume it in the queue strip below." } else { "Waiting in the queue" }))
                        .child(btn(eid("st-cancel", key, id), "Cancel", ButtonKind::Ghost).small().on_click(move |_, _, _| queue::cancel(id))),
                )
            }
            _ => None,
        }
    }

    // ------------------------------------------------------------ left: scenes

    fn list(&self, d: &Data, busy: &Busy, cx: &mut Context<Self>) -> Div {
        let issues = engine::issues(&d.story);
        let head = column_head("Scenes")
            .child(spec(count(d.story.scenes.len(), "scene")))
            .child(icon_button("st-add-scene", ph::PLUS, "Add a scene after the selected one").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.add_scene(cx))))
            .child(icon_button("st-versions", ph::CLOCK_COUNTER_CLOCKWISE, "Earlier versions of the script").small().selected(self.panel == Panel::Versions).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                let on = this.panel != Panel::Versions;
                this.show_versions(on, cx);
            })));
        let mut body = column_body("st-scenes").gap(px(space::XXS));
        for (i, s) in d.story.scenes.iter().enumerate() {
            let selected = self.selected.as_deref() == Some(s.id.as_str()) || (self.selected.is_none() && i == 0);
            let warn: Vec<String> = issues.iter().filter(|x| x.scene == s.id && x.severity != Severity::Info).map(|x| x.message.trim().to_string()).collect();
            let scene_busy = self.scene_busy(&s.id);
            let working = matches!(scene_busy, Busy::Running { .. });
            let queued = matches!(scene_busy, Busy::Waiting { .. });
            let locked = s.locked.contains(SCENE_LOCK) || s.locked.contains("/");
            let id = s.id.clone();
            let mut meta = div().flex().items_center().gap(px(space::XS)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{} · {}", word(&TONES, s.tone), fmt_secs(s.target_s)));
            if s.fight {
                meta = meta.child(icon_ph(ph::SWORD, Tokens::text_3(), icon_px::XS));
            }
            let marks = div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(space::XS))
                .when(working, |d| d.child(spinner(eid("st-row-spin", &s.id, 0), Tokens::accent(), icon_px::XS)))
                .when(queued, |d| d.child(div().id(eid("st-row-wait", &s.id, 0)).tooltip(tip("Waiting in the queue to be adapted again")).child(icon_ph(ph::HOURGLASS, Tokens::text_3(), icon_px::XS))))
                .when(!warn.is_empty(), |d| {
                    let why = if warn.len() == 1 { warn[0].clone() } else { format!("{} to look at: {}", count(warn.len(), "thing"), warn.join(" · ")) };
                    d.child(div().id(eid("st-row-warn", &s.id, 0)).tooltip(tip(why)).child(icon_ph(ph::WARNING, Tokens::warning(), icon_px::XS)))
                })
                .when(locked, |d| d.child(div().id(eid("st-row-lock", &s.id, 0)).tooltip(tip("Locked: adapting again keeps this scene")).child(icon_ph(ph::LOCK_SIMPLE, Tokens::text_3(), icon_px::XS))))
                .when(s.edited && !locked, |d| d.child(div().id(eid("st-row-mine", &s.id, 0)).tooltip(tip("Changed by you; adapting again keeps it")).child(div().size(px(6.)).rounded_full().bg(Tokens::accent()))));
            body = body.child(
                list_item(eid("st-scene", &s.id, 0), selected)
                    .child(div().flex_none().w(px(22.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{}", i + 1)))
                        .child(div().flex().flex_col().flex_1().min_w_0().gap(px(space::XXS)).child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(if s.title.trim().is_empty() { format!("Scene {}", i + 1) } else { s.title.trim().to_string() })).child(meta))
                    .child(marks)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if this.selected.as_deref() != Some(id.as_str()) {
                            this.commit(cx);
                            // A length typed but not yet saved is saved before the box goes.
                            this.drop_length(cx);
                            this.selected = Some(id.clone());
                            this.scroll.set_offset(point(px(0.), px(0.)));
                        }
                        cx.notify();
                    })),
            );
        }
        div().flex().flex_col().size_full().child(head).child(body).child(self.footer(d, busy, cx))
    }

    fn footer(&self, d: &Data, busy: &Busy, cx: &mut Context<Self>) -> Div {
        let total: f32 = d.story.scenes.iter().map(|s| s.target_s.max(0.)).sum();
        let aim = d.episode.target_minutes;
        let line = if aim > 0. { format!("Total {} · aim {} min", fmt_secs(total), (aim * 10.).round() / 10.) } else { format!("Total {}", fmt_secs(total)) };
        let unread = d.reads.iter().find(|(_, r)| !*r).map(|(l, _)| l.clone());
        // A script exists: the next step is the primary action, adapting again the second one.
        let buttons = if matches!(busy, Busy::Running { .. }) {
            div().child(btn("st-adapt", "Adapting…", ButtonKind::Primary).full_width().loading(true))
        } else if let Busy::Waiting { paused, .. } = busy {
            // Waiting or paused: nothing turns (the queue below says why and has Resume).
            div().child(btn("st-adapt", if *paused { "Adapting is paused" } else { "Waiting to adapt" }, ButtonKind::Primary).icon(if *paused { ph::PAUSE_CIRCLE } else { ph::HOURGLASS }).full_width().disabled(if *paused { "Resume it in the queue below, or cancel it there" } else { "It starts when the jobs before it are done" }))
        } else {
            let go = self.ctx.go.clone();
            div()
                .flex()
                .flex_col()
                .gap(px(space::SM))
                .child(btn("st-next", "Continue to Bible", ButtonKind::Primary).icon(Step::Bible.icon()).full_width().on_click(move |_, w, cx| go(Step::Bible, w, cx)))
                .child(btn("st-adapt", "Adapt again", ButtonKind::Secondary).icon(ph::SPARKLE).full_width().tooltip("Adapts the chapters again. Scenes you changed or locked stay as they are.").on_click(cx.listener(|this, _: &ClickEvent, w, cx| this.confirm_adapt_again(w, cx))))
        };
        let failed = match busy {
            Busy::Failed { id, error } => Some(self.failure(*id, error.clone())),
            _ => None,
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
            .when_some(unread.filter(|_| !busy.active()), |f, l| f.child(info_line(ph::INFO, LineTone::Warning, format!("{l} is not fully read yet; adapting stops there."))))
            .children(self.progress("all", busy))
            .children(failed)
            .child(buttons)
    }

    // ------------------------------------------------------------ centre: the scene

    fn scene_view(&self, d: &Data, at: usize, scene: &Scene, narrow: bool, window: &Window, cx: &mut Context<Self>) -> Div {
        let number = at + 1;
        let sid = scene.id.clone();
        let locked = scene.locked.contains(SCENE_LOCK) || scene.locked.contains("/");
        let busy = self.scene_busy(&scene.id);
        let has_source = scene.source.first().or_else(|| scene.beats.iter().find_map(|b| b.source.first())).is_some();
        let adapt = {
            let b = btn("st-adapt-scene", "Adapt this scene again", ButtonKind::Secondary).small().icon(ph::ARROWS_CLOCKWISE);
            if locked {
                b.disabled("Unlock the scene to adapt it again")
            } else if !has_source {
                b.disabled("This scene was written by hand: it has no manga pages to adapt")
            } else if matches!(busy, Busy::Running { .. }) {
                b.loading(true)
            } else if let Busy::Waiting { paused, .. } = &busy {
                b.disabled(if *paused { "Paused in the queue" } else { "Waiting in the queue" })
            } else {
                let id = sid.clone();
                b.tooltip("Adapts this scene's pages again. What you changed stays.").on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.adapt_scene(id.clone(), number, cx)))
            }
        };
        let lock = {
            let id = sid.clone();
            icon_button("st-lock", if locked { ph::LOCK_SIMPLE } else { ph::LOCK_SIMPLE_OPEN }, if locked { "Unlock: adapting again may change this scene" } else { "Lock: adapting again keeps this scene exactly as it is" })
                .small()
                .selected(locked)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_lock(id.clone(), !locked, cx)))
        };
        let w = cx.entity().downgrade();
        let last = d.story.scenes.len().saturating_sub(1);
        let mut more = Vec::new();
        {
            let (w, id) = (w.clone(), sid.clone());
            more.push(
                MenuItem::new("Move up", move |_, cx| {
                    let id = id.clone();
                    let _ = w.update(cx, |s, cx| s.apply(op(move |st| done(move_scene(st, &id, true))), cx));
                })
                .icon(ph::ARROW_UP)
                .disabled(at == 0),
            );
        }
        {
            let (w, id) = (w.clone(), sid.clone());
            more.push(
                MenuItem::new("Move down", move |_, cx| {
                    let id = id.clone();
                    let _ = w.update(cx, |s, cx| s.apply(op(move |st| done(move_scene(st, &id, false))), cx));
                })
                .icon(ph::ARROW_DOWN)
                .disabled(at >= last),
            );
        }
        {
            let (w, id) = (w.clone(), sid.clone());
            more.push(
                MenuItem::new("Join with the next scene", move |_, cx| {
                    let id = id.clone();
                    let _ = w.update(cx, |s, cx| s.apply(op(move |st| done(merge_next(st, &id))), cx));
                })
                .icon(ph::ROWS)
                .disabled(at >= last),
            );
        }
        {
            let (w, id) = (w.clone(), sid.clone());
            more.push(MenuItem::new("Add a beat", move |_, cx| {
                let id = id.clone();
                let _ = w.update(cx, |s, cx| s.add_beat(id, cx));
            })
            .icon(ph::PLUS));
        }
        more.push(MenuItem::separator());
        {
            let (w, id) = (w.clone(), sid.clone());
            more.push(
                MenuItem::new("Delete scene", move |win, cx| {
                    let id = id.clone();
                    let _ = w.update(cx, |s, cx| s.confirm_delete_scene(id, number, win, cx));
                })
                .icon(ph::TRASH)
                .danger(),
            );
        }
        let head = title_head(format!("Scene {number} of {}", d.story.scenes.len()))
            .child(spec(fmt_secs(scene.target_s)))
            .when(locked, |h| h.child(crate::widgets::locked_chip()))
            .child(div().flex_1())
            .child(adapt)
            .child(lock)
            .when(narrow, |h| {
                h.child(icon_button("st-settings", ph::SLIDERS_HORIZONTAL, "Scene settings").small().selected(self.drawer).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.drawer = !this.drawer;
                    this.panel = Panel::Scene;
                    cx.notify();
                })))
            })
            .child(overflow_menu("st-scene-more", more).small());

        let mut body = div().id("st-body").flex().flex_col().flex_1().min_h_0().gap(px(space::LG)).p(px(space::LG)).overflow_y_scroll().track_scroll(&self.scroll);
        // Title and summary.
        let title_field = Field::Title(sid.clone());
        let title = match self.is_editing(&title_field) {
            Some(input) => editor(&input, window, cx).into_any_element(),
            None => {
                let current = scene.title.clone();
                editable(eid("st-title", &sid, 0), &scene.title, "Name this scene", size::LG, Tokens::text_1())
                    .font_weight(FontWeight::SEMIBOLD)
                    .on_click(cx.listener(move |this, _: &ClickEvent, w, cx| this.edit(title_field.clone(), current.clone(), false, w, cx)))
                    .into_any_element()
            }
        };
        let summary_field = Field::Summary(sid.clone());
        let summary = match self.is_editing(&summary_field) {
            Some(input) => editor(&input, window, cx).into_any_element(),
            None => {
                let current = scene.summary.clone();
                editable(eid("st-summary", &sid, 0), &scene.summary, "Add what happens in this scene", size::SM, Tokens::text_2())
                    .on_click(cx.listener(move |this, _: &ClickEvent, w, cx| this.edit(summary_field.clone(), current.clone(), false, w, cx)))
                    .into_any_element()
            }
        };
        body = body.child(div().flex().flex_col().gap(px(space::XS)).child(div().flex().child(title)).child(div().flex().child(summary)));
        if locked {
            body = body.child(info_line(ph::LOCK_SIMPLE, LineTone::Neutral, "Locked: adapting again keeps this scene exactly as it is. You can still change it here."));
        }
        if let Some(p) = self.progress(&sid, &busy) {
            body = body.child(p);
        }
        if let Busy::Failed { id, error } = &busy {
            body = body.child(self.failure(*id, error.clone()));
        }
        // Beats and their lines.
        let by_id: HashMap<&str, &Line> = d.story.lines.iter().map(|l| (l.id.as_str(), l)).collect();
        let mut placed: HashSet<&str> = HashSet::new();
        let beats = scene.beats.len();
        for (bi, b) in scene.beats.iter().enumerate() {
            let lines: Vec<&Line> = b.lines.iter().filter_map(|id| by_id.get(id.as_str()).copied()).collect();
            placed.extend(lines.iter().map(|l| l.id.as_str()));
            body = body.child(self.beat_view(d, &sid, bi, beats, b, &lines, window, cx));
        }
        let loose: Vec<&Line> = d.story.lines.iter().filter(|l| l.scene == sid && !placed.contains(l.id.as_str())).collect();
        if !loose.is_empty() {
            let mut group = section("Not in a beat");
            for l in loose {
                group = group.child(self.line_row(d, l, window, cx));
            }
            body = body.child(group);
        }
        {
            let id = sid.clone();
            body = body.child(div().child(btn("st-add-beat", "Add a beat", ButtonKind::Ghost).small().icon(ph::PLUS).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.add_beat(id.clone(), cx)))));
        }
        div().flex().flex_col().size_full().child(head).child(body)
    }

    #[allow(clippy::too_many_arguments)]
    fn beat_view(&self, d: &Data, scene: &str, bi: usize, beats: usize, b: &Beat, lines: &[&Line], window: &Window, cx: &mut Context<Self>) -> Div {
        let field = Field::Purpose { scene: scene.to_string(), beat: b.id.clone() };
        let purpose = match self.is_editing(&field) {
            Some(input) => editor(&input, window, cx).into_any_element(),
            None => {
                let current = b.purpose.clone();
                editable(eid("st-purpose", &b.id, 0), &b.purpose, "What this beat does", size::SM, Tokens::text_1())
                    .font_weight(FontWeight::MEDIUM)
                    .on_click(cx.listener(move |this, _: &ClickEvent, w, cx| this.edit(field.clone(), current.clone(), false, w, cx)))
                    .into_any_element()
            }
        };
        let w = cx.entity().downgrade();
        let mut menu = Vec::new();
        {
            let (w, s, bt) = (w.clone(), scene.to_string(), b.id.clone());
            menu.push(MenuItem::new("Add a line", move |win, cx| {
                let (s, bt) = (s.clone(), bt.clone());
                let _ = w.update(cx, |this, cx| this.add_line(s, bt, win, cx));
            })
            .icon(ph::PLUS));
        }
        if bi > 0 {
            let (w, s) = (w.clone(), scene.to_string());
            menu.push(MenuItem::new("Start a new scene here", move |_, cx| {
                let s = s.clone();
                let _ = w.update(cx, |this, cx| {
                    let Load::Ready(d) = &this.data else { return };
                    let new_id = d.ids().scene();
                    let nid = new_id.clone();
                    this.apply(op(move |st| done(split_scene(st, &s, bi, &nid))), cx);
                    this.selected = Some(new_id);
                    this.length = None;
                });
            })
            .icon(ph::SCISSORS));
        }
        if beats > 1 {
            let (w, s, bt) = (w.clone(), scene.to_string(), b.id.clone());
            menu.push(MenuItem::separator());
            menu.push(
                MenuItem::new("Remove this beat (its lines stay)", move |_, cx| {
                    let (s, bt) = (s.clone(), bt.clone());
                    let _ = w.update(cx, |this, cx| this.delete_beat(s, bt, cx));
                })
                .icon(ph::TRASH)
                .danger(),
            );
        }
        let head = div()
            .flex()
            .items_start()
            .gap(px(space::SM))
            .child(div().flex_none().pt(px(3.)).child(label_caps(&format!("Beat {}", bi + 1))))
            .child(div().flex().flex_1().min_w_0().child(purpose))
            .when(b.hook, |d| d.child(tag("Hook", Tone::Accent)))
            .child(overflow_menu(eid("st-beat-more", &b.id, 0), menu).small());
        let mut rows = div().flex().flex_col().gap(px(space::XXS));
        if lines.is_empty() {
            rows = rows.child(div().px(px(space::SM)).child(muted("No lines in this beat yet.")));
        }
        for l in lines {
            rows = rows.child(self.line_row(d, l, window, cx));
        }
        let (s, bt) = (scene.to_string(), b.id.clone());
        rows = rows.child(div().pl(px(space::SM)).child(btn(eid("st-add-line", &b.id, 0), "Add a line", ButtonKind::Ghost).small().icon(ph::PLUS).on_click(cx.listener(move |this, _: &ClickEvent, w, cx| this.add_line(s.clone(), bt.clone(), w, cx)))));
        div().flex().flex_col().gap(px(space::SM)).child(head).child(rows)
    }

    fn line_row(&self, d: &Data, l: &Line, window: &Window, cx: &mut Context<Self>) -> Div {
        let w = cx.entity().downgrade();
        let lid = l.id.clone();
        let group = SharedString::from(format!("st-line-{lid}"));
        // Who says it, and how.
        let who = match (&l.speaker, l.kind) {
            (Some(id), _) => d.name_of(id),
            (None, LineKind::Sfx) => "Sound".to_string(),
            (None, _) => "Narration".to_string(),
        };
        let icon = match l.kind {
            LineKind::Thought => ph::CHAT_CIRCLE_DOTS,
            LineKind::Narration => ph::BOOK_OPEN,
            LineKind::Sfx => ph::SPEAKER_HIGH,
            _ => ph::CHAT_CIRCLE,
        };
        let mut items = vec![MenuItem::heading("Who")];
        {
            let (w, id) = (w.clone(), lid.clone());
            items.push(
                MenuItem::new("Narration", move |_, cx| {
                    let id = id.clone();
                    let _ = w.update(cx, |s, cx| s.apply(op(move |st| done(edit_line(st, &id, &["/speaker", "/kind"], |l| {
                        l.speaker = None;
                        l.kind = LineKind::Narration;
                    }))), cx));
                })
                .checked(l.speaker.is_none() && l.kind == LineKind::Narration),
            );
        }
        let mut speakers: Vec<(String, String)> = d.cast.clone();
        if let Some(id) = l.speaker.as_ref().filter(|id| !d.cast.iter().any(|(c, _)| c == *id)) {
            speakers.insert(0, (id.clone(), "Someone not in the cast".to_string()));
        }
        for (cid, name) in speakers {
            let (w, id, pick) = (w.clone(), lid.clone(), cid.clone());
            items.push(
                MenuItem::new(name, move |_, cx| {
                    let (id, pick) = (id.clone(), pick.clone());
                    let _ = w.update(cx, |s, cx| s.apply(op(move |st| done(edit_line(st, &id, &["/speaker", "/kind"], |l| {
                        l.speaker = Some(pick.clone());
                        if matches!(l.kind, LineKind::Narration | LineKind::Sfx) {
                            l.kind = LineKind::Speech;
                        }
                    }))), cx));
                })
                .checked(l.speaker.as_deref() == Some(cid.as_str())),
            );
        }
        items.push(MenuItem::separator());
        items.push(MenuItem::heading("How"));
        for (k, label) in LINE_KINDS {
            let (w, id) = (w.clone(), lid.clone());
            items.push(
                MenuItem::new(label, move |_, cx| {
                    let id = id.clone();
                    let _ = w.update(cx, |s, cx| s.apply(op(move |st| done(edit_line(st, &id, &["/kind"], |l| {
                        l.kind = k;
                        if k == LineKind::Narration {
                            l.speaker = None;
                        }
                    }))), cx));
                })
                .checked(l.kind == k),
            );
        }
        let speaker = div().flex_none().w(px(SPEAKER_W)).overflow_hidden().child(menu_button(eid("st-who", &lid, 0), icon, "Who says it, and how", items).label(who).small().anchor(Anchor::TopLeft).min_width(220.));

        // The emotion and its strength.
        let mut feel = Vec::new();
        for (k, label) in EMOTIONS {
            let (w, id) = (w.clone(), lid.clone());
            feel.push(
                MenuItem::new(label, move |_, cx| {
                    let id = id.clone();
                    let _ = w.update(cx, |s, cx| s.apply(op(move |st| done(edit_line(st, &id, &["/emotion/kind"], |l| {
                        l.emotion.kind = k;
                        l.emotion.strength = l.emotion.strength.clamp(1, 3);
                    }))), cx));
                })
                .checked(l.emotion.kind == k),
            );
        }
        let strength = l.emotion.strength.clamp(1, 3);
        let mut dots = div().flex().flex_none().items_center();
        for k in 1..=3u8 {
            let (w, id) = (w.clone(), lid.clone());
            dots = dots.child(
                div()
                    .id(eid("st-str", &lid, k as u64))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(14.))
                    .cursor_pointer()
                    .tooltip(tip(STRENGTHS[k as usize - 1]))
                    .child(div().size(px(6.)).rounded_full().bg(if k <= strength { Tokens::accent() } else { Tokens::track() }))
                    .on_click(move |_, _, cx| {
                        let id = id.clone();
                        let _ = w.update(cx, |s, cx| s.apply(op(move |st| done(edit_line(st, &id, &["/emotion/strength"], |l| l.emotion.strength = k))), cx));
                    }),
            );
        }
        let emotion = div().flex().flex_none().w(px(EMOTION_W)).items_center().gap(px(space::XXS)).overflow_hidden().child(menu_button(eid("st-feel", &lid, 0), ph::MASK_HAPPY, "How it feels", feel).label(word(&EMOTIONS, l.emotion.kind)).small().anchor(Anchor::TopLeft).min_width(180.)).child(dots);

        // The words.
        let field = Field::Spoken(lid.clone());
        let editing = self.is_editing(&field).is_some();
        let words = match self.is_editing(&field) {
            Some(input) => editor(&input, window, cx).into_any_element(),
            None => {
                let current = l.spoken.clone();
                let quoted = if l.spoken.trim().is_empty() || l.kind == LineKind::Sfx { l.spoken.clone() } else { format!("\u{201c}{}\u{201d}", l.spoken.trim()) };
                editable(eid("st-words", &lid, 0), &quoted, "Write the line", size::SM, if l.kind == LineKind::Sfx { Tokens::text_2() } else { Tokens::text_1() })
                    .on_click(cx.listener(move |this, _: &ClickEvent, w, cx| this.edit(field.clone(), current.clone(), false, w, cx)))
                    .into_any_element()
            }
        };
        let original = (!l.original.trim().is_empty() && l.original.trim() != l.spoken.trim()).then(|| div().px(px(space::XS)).text_size(px(size::XS)).text_color(Tokens::text_3()).whitespace_normal().child(format!("Manga: \u{201c}{}\u{201d}", l.original.trim())));
        let marks = div()
            .flex()
            .flex_wrap()
            .gap(px(space::XS))
            .px(px(space::XS))
            .when(l.keep_words, |d| d.child(tag("Manga's words", Tone::Neutral)))
            .when(l.kind == LineKind::Sfx, |d| d.child(tag("Not voiced", Tone::Neutral)))
            .when(l.edited, |d| d.child(div().id(eid("st-mine", &lid, 0)).flex().items_center().gap(px(space::XXS)).tooltip(tip("Changed by you; adapting again keeps it")).child(div().size(px(6.)).rounded_full().bg(Tokens::accent())).child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("Yours"))));
        // The words take the free width; below a readable width they move under who and how.
        let text = div().flex().flex_col().flex_1().min_w(px(LINE_TEXT_MIN_W)).gap(px(space::XXS)).child(div().flex().child(words)).children(original).child(marks);

        // Hover actions: a small floating toolbar, so they never take width from the words.
        let keep = l.keep_words;
        let has_original = !l.original.trim().is_empty();
        let actions = div()
            .absolute()
            .top(px(space::XXS))
            .right(px(space::XS))
            .flex()
            .gap(px(space::XXS))
            .p(px(space::XXS))
            .rounded(px(radius::MD))
            .bg(Tokens::popover())
            .border_1()
            .border_color(Tokens::border())
            .shadow_sm()
            .invisible()
            .group_hover(group.clone(), |s| s.visible())
            .child({
                let id = lid.clone();
                icon_button(eid("st-up", &lid, 0), ph::ARROW_UP, "Move up").small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let id = id.clone();
                    this.apply(op(move |st| done(move_line(st, &id, true))), cx);
                }))
            })
            .child({
                let id = lid.clone();
                icon_button(eid("st-down", &lid, 0), ph::ARROW_DOWN, "Move down").small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let id = id.clone();
                    this.apply(op(move |st| done(move_line(st, &id, false))), cx);
                }))
            })
            .when(has_original, |d| {
                let id = lid.clone();
                d.child(icon_button(eid("st-keep", &lid, 0), ph::BOOK_OPEN_TEXT, if keep { "Keep the manga's words: on" } else { "Keep the manga's words: use the line as the manga has it" }).small().selected(keep).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let id = id.clone();
                    this.apply(op(move |st| done(edit_line(st, &id, &["/keep_words", "/spoken"], |l| {
                        l.keep_words = !keep;
                        if l.keep_words {
                            l.spoken = l.original.trim().to_string();
                        }
                    }))), cx);
                })))
            })
            .child({
                let id = lid.clone();
                icon_button(eid("st-del", &lid, 0), ph::TRASH, "Remove this line").small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.delete_line(id.clone(), cx)))
            });
        div()
            .group(group)
            .relative()
            .flex()
            .flex_wrap()
            .items_start()
            .gap(px(space::SM))
            .px(px(space::SM))
            .py(px(space::XS))
            .rounded(px(radius::MD))
            .hover(|s| s.bg(Tokens::hover()))
            .child(div().flex().flex_none().items_start().gap(px(space::SM)).child(speaker).child(emotion))
            .child(text)
            // While the line is typed in, its toolbar stays away so it never covers the box.
            .when(!editing, |d| d.child(actions))
    }

    // ------------------------------------------------------------ right: scene settings

    fn inspector(&mut self, d: &Data, scene: &Scene, narrow: bool, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let sid = scene.id.clone();
        let is = |p: &str| scene.locked.contains(p);
        let w = cx.entity().downgrade();
        let head = column_head("Scene").when(narrow, |h| {
            h.child(icon_button("st-close", ph::X, "Close").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.drawer = false;
                cx.notify();
            })))
        });
        macro_rules! picker {
            ($id:expr, $options:expr, $value:expr, $pointer:expr, $set:expr) => {{
                let (w, s) = (w.clone(), sid.clone());
                auto_chips($id, $options, $value, is($pointer)).on_pick(move |v, _, cx| {
                    let _ = w.update(cx, |this, cx| this.pick(&s, $pointer, v, $set, cx));
                })
            }};
        }
        let kind = picker!("st-kind", KINDS, scene.kind, "/kind", |sc: &mut Scene, v| sc.kind = v);
        let tone = picker!("st-tone", TONES, scene.tone, "/tone", |sc: &mut Scene, v| sc.tone = v);
        let fight = picker!("st-fight", [(true, "Fight"), (false, "No fight")], scene.fight, "/fight", |sc: &mut Scene, v| sc.fight = v);
        let colour_ix = COLOUR_THEMES.iter().position(|(n, _)| n.eq_ignore_ascii_case(scene.colour.name.trim())).unwrap_or(usize::MAX);
        let colour = picker!("st-colour", COLOUR_THEMES.iter().enumerate().map(|(i, (n, _))| (i, *n)), colour_ix, "/colour/name", |sc: &mut Scene, v: usize| {
            if let Some((n, _)) = COLOUR_THEMES.get(v) {
                sc.colour.name = n.to_string();
            }
        });
        let time = picker!("st-time", TIMES, scene.time_of_day, "/time_of_day", |sc: &mut Scene, v| sc.time_of_day = v);
        let place_ix = scene.place.as_ref().and_then(|p| d.places.iter().position(|(id, _)| id == p)).unwrap_or(usize::MAX);
        let places: Vec<(usize, String)> = d.places.iter().enumerate().map(|(i, (_, n))| (i, n.clone())).collect();
        let place_ids: Vec<String> = d.places.iter().map(|(id, _)| id.clone()).collect();
        let place = {
            let (w, s) = (w.clone(), sid.clone());
            auto_chips("st-place", places, place_ix, is("/place")).unset("not set").on_pick(move |v: Option<usize>, _, cx| {
                let v = v.and_then(|i| place_ids.get(i).cloned());
                let reset = v.is_none();
                let _ = w.update(cx, |this, cx| {
                    if reset {
                        this.pick::<Option<String>>(&s, "/place", None, |sc, v| sc.place = v, cx);
                    } else {
                        this.pick(&s, "/place", Some(v), |sc, v| sc.place = v, cx);
                    }
                });
            })
        };
        // Length: Auto, or the user's own.
        let length_input = self.ensure_length(scene, window, cx);
        let floor = engine::scene_floor(scene, &d.story.lines);
        let length = {
            let (w, s) = (w.clone(), sid.clone());
            div()
                .flex()
                .flex_col()
                .gap(px(space::XS))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .child(auto_chips("st-length", Vec::<(u8, &str)>::new(), 0u8, is("/target_s")).on_pick(move |_: Option<u8>, _, cx| {
                            let _ = w.update(cx, |this, cx| this.pick::<f32>(&s, "/target_s", None, |sc, v| sc.target_s = v, cx));
                        }))
                        .child(div().w(px(LENGTH_W)).child(Input::new(&length_input))),
                )
                // Compared at the precision shown (whole seconds), so "0:59" never warns of "0:59".
                .when(too_short(scene.target_s, floor), |c| c.child(info_line(ph::WARNING, LineTone::Warning, format!("Its lines need about {}; the scene is shorter.", fmt_secs(floor)))))
        };
        let mut body = column_body("st-inspector")
            .child(inspector_row("Kind", kind))
            .child(inspector_row("Tone", tone))
            .child(inspector_row("Fight", fight))
            .child(inspector_row("Colour", colour))
            .child(inspector_row("Length", length))
            .child(inspector_row("Time", time))
            .child(inspector_row(
                "Place",
                if d.places.is_empty() {
                    muted(match &scene.place {
                        Some(p) => format!("{p}. Places get their own pages in the Bible step."),
                        None => "No places yet. The Bible step finds them in the manga.".to_string(),
                    })
                    .into_any_element()
                } else {
                    place.into_any_element()
                },
            ))
            .child(muted("Auto: the AI chooses from the manga. Pick one and it stays yours when you adapt again."));
        let issues: Vec<Issue> = engine::issues(&d.story).into_iter().filter(|i| i.scene == sid).collect();
        if !issues.is_empty() {
            let mut check = section("Check");
            for i in &issues {
                check = check.child(issue_line(i));
            }
            body = body.child(check);
        }
        body = body.child(div().pt(px(space::SM)).child(btn("st-open-versions", "Earlier versions", ButtonKind::Ghost).small().icon(ph::CLOCK_COUNTER_CLOCKWISE).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.show_versions(true, cx)))));
        div().flex().flex_col().size_full().child(head).child(body)
    }

    fn versions_view(&self, cx: &mut Context<Self>) -> Div {
        let head = column_head("Earlier versions").child(icon_button("st-versions-close", ph::X, "Back to the scene").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.show_versions(false, cx))));
        let mut body = column_body("st-versions-list").gap(px(space::XXS));
        match &self.versions {
            None | Some(Load::Loading) => {
                body = body.children((0..4).map(|_| skeleton_row()));
            }
            Some(Load::Failed(e)) => body = body.child(error_block(e.clone())),
            Some(Load::Ready(list)) if list.is_empty() => {
                body = body.child(muted("No earlier versions yet. Each time you or the AI change the script, the version before is kept here."));
            }
            Some(Load::Ready(list)) => {
                let now = now_secs();
                body = body.child(div().pb(px(space::XS)).child(muted("The script as it was before each change, newest first.")));
                for (i, v) in list.iter().enumerate() {
                    let pick = v.clone();
                    body = body.child(
                        div()
                            .id(("st-version", i))
                            .flex()
                            .flex_none()
                            .items_start()
                            .gap(px(space::SM))
                            .p(px(space::SM))
                            .rounded(px(radius::MD))
                            .hover(|s| s.bg(Tokens::hover()))
                            .child(div().flex_none().pt(px(2.)).child(icon_ph(ph::CLOCK_COUNTER_CLOCKWISE, Tokens::text_3(), icon_px::SM)))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(px(space::XXS))
                                    .child(div().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(version_time(v.ms, now)))
                                    .child(div().text_size(px(size::XS)).line_height(px(16.)).text_color(Tokens::text_2()).whitespace_normal().child(format!("Before {}", v.change)))
                                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{} · {}", count(v.scenes, "scene"), count(v.lines, "line")))),
                            )
                            .child(div().flex_none().child(btn(("st-restore", i), "Restore", ButtonKind::Secondary).small().on_click(cx.listener(move |this, _: &ClickEvent, w, cx| this.confirm_restore(pick.clone(), w, cx))))),
                    );
                }
            }
        }
        div().flex().flex_col().size_full().child(head).child(body)
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: with test-support, `gpui_kit::*` carries a `test` macro that shadows `#[test]`.
    use super::{
        adapt_again_text, delete_scene, edit_line, edit_scene, insert_line, job_busy, merge_next, move_line, move_scene, parse_length, refresh_issues, remove_line, reset_scene, sort_lines, split_scene, too_short, what_changed, Busy, SCENE_LOCK,
    };
    use mv_anime::queue::{Job, JobKind, JobState, Queue};
    use mv_anime::{Beat, Line, LineKind, Scene, Story};
    use std::collections::BTreeSet;

    #[test]
    fn earlier_versions_say_what_really_happened() {
        let scene = |id: &str, title: &str| Scene { id: id.into(), title: title.into(), ..Scene::default() };
        let before = Story { made_at: 10, scenes: vec![scene("a", "One"), scene("b", "Two")], ..Story::default() };
        let one = Story { made_at: 20, scenes: vec![scene("a", "One"), scene("b", "Two again")], ..Story::default() };
        assert_eq!(what_changed(&before, &one), "scene 2 was adapted again");
        let all = Story { made_at: 20, scenes: vec![scene("a", "1"), scene("b", "2")], ..Story::default() };
        assert_eq!(what_changed(&before, &all), "the AI adapted the chapters again");
        assert_eq!(what_changed(&one, &before), "an earlier version was put back");
        let gone = Story { made_at: 10, scenes: vec![scene("a", "One")], ..Story::default() };
        let untitled = Story { made_at: 10, scenes: vec![scene("a", "One"), scene("b", " ")], ..Story::default() };
        assert_eq!(what_changed(&untitled, &gone), "scene 2 deleted");
        assert!(adapt_again_text(Some(2)).contains("2 scenes with planned shots"));
        assert!(!adapt_again_text(Some(0)).contains("planned shots are"));
        // Both shown as 0:59: no warning; 1:00 needed: a warning.
        assert!(!too_short(59.2, 58.7));
        assert!(too_short(59., 60.));
    }

    fn line(id: &str, scene: &str) -> Line {
        Line { id: id.into(), scene: scene.into(), spoken: format!("words of {id}"), kind: LineKind::Speech, ..Line::default() }
    }

    fn beat(id: &str, lines: &[&str]) -> Beat {
        Beat { id: id.into(), lines: lines.iter().map(|s| s.to_string()).collect(), ..Beat::default() }
    }

    /// sc1: b1 [l1 l2], b2 [l3]; sc2: b3 [l4].
    fn story() -> Story {
        Story {
            episode: "ep01".into(),
            scenes: vec![
                Scene { id: "sc1".into(), title: "Hall".into(), target_s: 30., beats: vec![beat("b1", &["l1", "l2"]), beat("b2", &["l3"])], ..Scene::default() },
                Scene { id: "sc2".into(), title: "Gate".into(), target_s: 20., beats: vec![beat("b3", &["l4"])], ..Scene::default() },
            ],
            lines: vec![line("l1", "sc1"), line("l2", "sc1"), line("l3", "sc1"), line("l4", "sc2")],
            ..Story::default()
        }
    }

    fn order(s: &Story) -> Vec<&str> {
        s.lines.iter().map(|l| l.id.as_str()).collect()
    }

    #[test]
    fn edits_mark_what_the_user_set() {
        let mut s = story();
        assert!(edit_scene(&mut s, "sc1", "/tone", |sc| sc.fight = true));
        assert!(s.scenes[0].edited && s.scenes[0].locked.contains("/tone"));
        assert!(reset_scene(&mut s, "sc1", "/tone"));
        assert!(!s.scenes[0].edited && s.scenes[0].locked.is_empty());
        assert!(edit_scene(&mut s, "sc2", SCENE_LOCK, |_| {}));
        assert!(s.scenes[1].locked.contains(""));
        assert!(reset_scene(&mut s, "sc2", SCENE_LOCK));
        assert!(s.scenes[1].locked.is_empty());
        assert!(edit_line(&mut s, "l2", &["/spoken"], |l| l.spoken = "Get back!".into()));
        let l = &s.lines[1];
        assert!(l.edited && l.locked.contains("/spoken") && l.spoken == "Get back!");
        assert!(!edit_scene(&mut s, "nope", "/tone", |_| {}));
    }

    #[test]
    fn versions_say_what_changed() {
        let before = story();
        let mut after = before.clone();
        assert!(super::same_script(&before, &after));
        assert!(edit_line(&mut after, "l2", &["/spoken"], |l| l.spoken = "Get back!".into()));
        assert!(!super::same_script(&before, &after));
        assert_eq!(super::what_changed(&before, &after), "a line edited in scene 1");
        let mut locked = before.clone();
        assert!(edit_scene(&mut locked, "sc2", SCENE_LOCK, |_| {}));
        assert_eq!(super::what_changed(&before, &locked), "scene 2 locked");
        let mut gone = before.clone();
        assert!(delete_scene(&mut gone, "sc2"));
        assert_eq!(super::what_changed(&before, &gone), "scene \u{201c}Gate\u{201d} deleted");
        assert_eq!(super::what_changed(&Story::default(), &before), "the first adaptation");
        // A join is told as a join, not as the second scene deleted.
        let mut joined = before.clone();
        assert!(super::merge_next(&mut joined, "sc1"));
        let label = super::what_changed(&before, &joined);
        assert!(label.contains("joined with") && label.contains("Gate"), "{label}");
    }

    #[test]
    fn auto_gives_the_ai_choice_back() {
        let mut s = story();
        s.scenes[0].place = Some("rooftop".into());
        // The user picks another place: it is theirs, and the AI's is remembered.
        assert!(edit_scene(&mut s, "sc1", "/place", |sc| sc.place = Some("street".into())));
        assert_eq!(s.scenes[0].place.as_deref(), Some("street"));
        // A second pick keeps the AI's first choice, not the user's earlier pick.
        assert!(edit_scene(&mut s, "sc1", "/place", |sc| sc.place = Some("store".into())));
        assert!(reset_scene(&mut s, "sc1", "/place"));
        assert_eq!(s.scenes[0].place.as_deref(), Some("rooftop"), "Auto is the AI's place again");
        assert!(!s.scenes[0].locked.contains("/place") && !s.scenes[0].edited);
        assert!(!s.extra.contains_key(super::AI_CHOICES), "nothing left to remember");
        // A place the AI never chose: Auto clears the user's pick ("not set").
        s.scenes[1].locked.insert("/place".into());
        s.scenes[1].place = Some("street".into());
        assert!(reset_scene(&mut s, "sc2", "/place"));
        assert_eq!(s.scenes[1].place, None);
        // Other settings come back the same way.
        let before = s.scenes[0].tone;
        assert!(edit_scene(&mut s, "sc1", "/tone", |sc| sc.tone = mv_anime::Tone::Horror));
        assert!(reset_scene(&mut s, "sc1", "/tone"));
        assert_eq!(s.scenes[0].tone, before);
    }

    #[test]
    fn lines_move_within_and_across_beats() {
        let mut s = story();
        assert!(move_line(&mut s, "l2", true));
        assert_eq!(s.scenes[0].beats[0].lines, ["l2", "l1"]);
        assert!(s.scenes[0].edited && s.scenes[0].locked.contains("/beats"));
        assert!(move_line(&mut s, "l1", false));
        assert_eq!(s.scenes[0].beats[0].lines, ["l2"]);
        assert_eq!(s.scenes[0].beats[1].lines, ["l1", "l3"]);
        assert_eq!(order(&s), ["l2", "l1", "l3", "l4"]);
        // Never out of its scene.
        assert!(!move_line(&mut s, "l3", false));
        assert!(!move_line(&mut s, "l4", true));
    }

    #[test]
    fn a_removed_line_comes_back_where_it_was() {
        let mut s = story();
        let r = remove_line(&mut s, "l2").unwrap_or_else(|| panic!("removed"));
        assert_eq!((r.scene.as_str(), r.beat.as_str(), r.at), ("sc1", "b1", 1));
        assert_eq!(order(&s), ["l1", "l3", "l4"]);
        assert!(insert_line(&mut s, &r.scene, &r.beat, r.at, r.line.clone(), "bx"));
        assert_eq!(s.scenes[0].beats[0].lines, ["l1", "l2"]);
        assert_eq!(order(&s), ["l1", "l2", "l3", "l4"]);
        // Twice does nothing; a scene that is gone takes nothing.
        assert!(!insert_line(&mut s, &r.scene, &r.beat, r.at, r.line.clone(), "bx"));
        assert!(!insert_line(&mut s, "gone", "b1", 0, line("l9", "gone"), "bx"));
    }

    #[test]
    fn scenes_split_join_move_and_go() {
        let mut s = story();
        assert!(split_scene(&mut s, "sc1", 1, "sc9"));
        assert_eq!(s.scenes.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["sc1", "sc9", "sc2"]);
        assert_eq!(s.scenes[1].beats[0].id, "b2");
        assert_eq!(s.lines.iter().find(|l| l.id == "l3").map(|l| l.scene.as_str()), Some("sc9"));
        assert!((s.scenes[0].target_s + s.scenes[1].target_s - 30.).abs() < 0.2);
        assert!(!split_scene(&mut s, "sc1", 0, "sc8"));
        assert!(!split_scene(&mut s, "sc2", 1, "sc8"));
        assert!(merge_next(&mut s, "sc1"));
        assert_eq!(s.scenes.len(), 2);
        assert_eq!(s.scenes[0].beats.len(), 2);
        assert_eq!(s.lines.iter().find(|l| l.id == "l3").map(|l| l.scene.as_str()), Some("sc1"));
        assert!(!merge_next(&mut s, "sc2"));
        assert!(move_scene(&mut s, "sc2", true));
        assert_eq!(s.scenes[0].id, "sc2");
        assert_eq!(order(&s), ["l4", "l1", "l2", "l3"]);
        assert!(!move_scene(&mut s, "sc2", true));
        assert!(delete_scene(&mut s, "sc2"));
        assert_eq!(order(&s), ["l1", "l2", "l3"]);
    }

    #[test]
    fn line_order_follows_the_beats() {
        let mut s = story();
        s.lines.reverse();
        s.lines.push(line("lx", "elsewhere"));
        sort_lines(&mut s);
        assert_eq!(order(&s), ["l1", "l2", "l3", "l4", "lx"]);
    }

    #[test]
    fn issues_are_checked_again_after_an_edit() {
        let mut s = story();
        s.lines[0].speaker = Some("stranger".into());
        s.extra.insert("issues".into(), serde_json::json!([
            {"code": "unknown_page", "severity": "info", "scene": "sc1", "message": "page 99"},
            {"code": "unknown_page", "severity": "info", "scene": "gone", "message": "page 98"},
            {"code": "tone_mismatch", "severity": "warning", "scene": "sc1", "message": "old"}
        ]));
        let known: BTreeSet<String> = ["jinwoo".to_string()].into_iter().collect();
        refresh_issues(&mut s, &known);
        let codes: Vec<String> = mv_anime::story::issues(&s).into_iter().map(|i| format!("{}:{}", i.code, i.scene)).collect();
        assert_eq!(codes, ["unknown_speaker:sc1", "unknown_page:sc1"]);
    }

    #[test]
    fn lengths_read_the_way_people_type_them() {
        assert_eq!(parse_length("2:30"), Some(150.));
        assert_eq!(parse_length(" 90 "), Some(90.));
        assert_eq!(parse_length("45s"), Some(45.));
        assert_eq!(parse_length("2 min"), Some(120.));
        assert_eq!(parse_length("1:75"), None);
        assert_eq!(parse_length("1"), None);
        assert_eq!(parse_length("20:00"), None);
        assert_eq!(parse_length("soon"), None);
    }

    #[test]
    fn the_latest_job_of_the_kind_counts() {
        let job = |id: u64, kind: JobKind, state: JobState| Job { id, project: "p".into(), kind, state, line: "Adapting Chapter 2…".into(), progress: 0.5, ..Job::default() };
        let story = |ep: &str| JobKind::Story { episode: ep.into() };
        let mut q = Queue { jobs: vec![job(1, story("ep01"), JobState::Failed), job(2, story("ep01"), JobState::Running), job(3, story("ep02"), JobState::Queued)], ..Queue::default() };
        let want = |k: &JobKind| super::is_story_job(k, "ep01");
        assert_eq!(job_busy(&q, "p", want), Busy::Running { id: 2, progress: 0.5, line: "Adapting Chapter 2…".into() });
        assert_eq!(job_busy(&q, "other", want), Busy::Idle);
        q.jobs[1].state = JobState::Done;
        assert_eq!(job_busy(&q, "p", want), Busy::Idle);
        q.jobs.push(Job { error: Some("No AI model".into()), ..job(4, story("ep01"), JobState::Failed) });
        assert_eq!(job_busy(&q, "p", want), Busy::Failed { id: 4, error: "No AI model".into() });
        q.jobs[3].hidden = true;
        assert_eq!(job_busy(&q, "p", want), Busy::Idle);
        let scene = JobKind::AdaptScene { episode: "ep01".into(), scene: "sc3".into() };
        assert!(super::is_scene_job(&scene, "ep01", "sc3"));
        assert!(!super::is_scene_job(&scene, "ep01", "sc4"));
        assert!(super::waits_for_limit("Claude hit a usage limit (x). Waiting about 12 min, then asking again; nothing is skipped."));
    }
}
