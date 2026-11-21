//! STORY (design 6.1): adapts the episode's read chapters into scenes, beats and lines with the
//! planner model, one call per chapter (split at scene-kind boundaries past the digest cap),
//! in order for continuity. Re-adapting keeps every scene the user edited.
//!
//! The flow of [`run_job`]: gather the inputs (each chapter's Studio Read, the brain's and the
//! bible's characters, the bible's places, the length budget), cut each chapter into parts
//! ([`chunk_pages`]), ask the model with `prompts/story.txt`, read its JSON leniently onto the
//! model types with fresh ids ([`map_reply`]), fit the scene lengths to the budget
//! ([`fit_budget`]), merge with what is on disk ([`merge`]) and save after **every** chapter, so a
//! later chapter that fails leaves the earlier ones adapted.
//!
//! Lines are checked against their scene's stakes (rule R3, [`review`]); the findings are kept in
//! the story file under `"issues"` (read them with [`issues`]).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write as _;

use anyhow::{anyhow, bail, Result};
use mv_ai::AiEffort;
use mv_recap::{Brain, ChapterRead};
use serde_json::Value;

use crate::llm::{self, Ai, BadReply};
use crate::merge::merge_unlocked;
use crate::model::{
    Beat, BibleIndex, ChapterPick, ColourTheme, Emotion, EmotionKind, Episode, Issue, Line, LineKind, Pace, Place, Scene, SceneKind, Severity, Shot,
    SourceRef, Status, Story, TimeOfDay, Tone,
};
use crate::pacing;
use crate::queue::{JobCtx, Tier};
use crate::source::{self, DIGEST_CAP};
use crate::store::{self, Store};

/// The system prompt of every adaptation call.
pub const PROMPT: &str = include_str!("prompts/story.txt");

/// The fixed colour themes a scene picks from (name, when it is used), from the measured colour
/// script of the reference episodes (research 25, plan R7). `ColourTheme.name` holds one of them.
pub const COLOUR_THEMES: [(&str, &str); 9] = [
    ("neutral daylight", "ordinary daytime, everyday life, open places"),
    ("warm home", "family, friends, safety, comfort; the brightest scenes"),
    ("dusk amber", "sunset, melancholy, the calm before trouble, goodbyes"),
    ("deep blue threat", "danger, dread, a monster close by, menace in the dark"),
    ("burning red", "rage, violence at its peak, blood, fire that destroys"),
    ("firelight hope", "fragile hope in the dark, warm light over cold shadows"),
    ("grey memory", "flashbacks and memories, almost no colour"),
    ("cold blue loss", "death, loss, the aftermath of a defeat"),
    ("eerie glow", "the supernatural, magic, a system window, a power awakening"),
];

/// Key of the story's issues in `Story.extra` (`"issues"` in `story.json`).
pub const ISSUES: &str = "issues";

/// Auto length of one chapter: 6 to 9 minutes (plan 3.1).
pub const CHAPTER_MIN_S: f32 = 360.;
/// Auto length of one chapter at most (seconds).
pub const CHAPTER_MAX_S: f32 = 540.;

/// Share of an anime episode adapted from manga panels: 45% comes from no panel (plan 3.1).
const FROM_PANELS: f32 = 0.55;
/// Air around a scene's spoken lines (as `pacing::scene_seconds`).
const AIR: f32 = 1.15;
/// Shortest and longest scene.
const SCENE_MIN_S: f32 = 2.;
const SCENE_MAX_S: f32 = 900.;

const NO_AI: &str = "No AI model is set up for the Anime Engine. Pick a model in the assistant or in the engine settings, then adapt again.";
const NO_SCENES: &str = "The AI's answer had no scenes in it. Try again or pick another model.";

/// Issue codes found while reading the model's answer (they cannot be recomputed later).
const MAPPING_CODES: [&str; 2] = ["unknown_page", PAGES_LEFT_OUT];

/// Issue code: the AI's answer stopped before the last pages of a part, and asking again for
/// them did not cover them either.
pub const PAGES_LEFT_OUT: &str = "pages_left_out";

/// Applies `f` to the project's id counters (`Series.next_ids`) under the one counters lock,
/// inside [`Store::update_series`], and answers the counters after the change. Every place that
/// hands out ids (Story and PlanScene jobs, the Story and Shots screens) goes through it, so two
/// of them never hand out the same id and none saves a stale copy of `project.json`.
///
/// `f` only changes the counters: read whatever it needs (the shot files, the story) before the
/// call. Lock order: counters, then `project.json`, then the global write lock.
pub fn bump_ids(store: &Store, f: impl FnOnce(&mut BTreeMap<String, u32>)) -> Result<BTreeMap<String, u32>> {
    store.bump_counters(f)
}

fn warn(msg: &str) {
    mv_core::logging::global().warn("anime", msg);
}

// ---------------------------------------------------------------- ids

/// Allocates scene, beat and line ids ("sc0001", "bt0001", "ln0001"), never below the project's
/// counters (`Series.next_ids`) nor below an id already in use.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ids {
    scene: u32,
    beat: u32,
    line: u32,
}

/// The number at the end of an id ("sc0012" -> 12; none -> 0).
fn number(id: &str) -> u32 {
    let digits: String = id.chars().rev().take_while(|c| c.is_ascii_digit()).collect::<Vec<_>>().into_iter().rev().collect();
    digits.parse().unwrap_or(0)
}

impl Ids {
    /// Counters from `next_ids`, raised past every id of `story`.
    pub fn new(counters: &BTreeMap<String, u32>, story: &Story) -> Ids {
        let get = |k: &str| counters.get(k).copied().unwrap_or(1).max(1);
        let mut ids = Ids { scene: get("scene"), beat: get("beat"), line: get("line") };
        ids.raise(story);
        ids
    }

    /// Never hands out an id `story` already uses.
    pub fn raise(&mut self, story: &Story) {
        for s in &story.scenes {
            self.scene = self.scene.max(number(&s.id).saturating_add(1));
            for b in &s.beats {
                self.beat = self.beat.max(number(&b.id).saturating_add(1));
            }
        }
        for l in &story.lines {
            self.line = self.line.max(number(&l.id).saturating_add(1));
        }
    }

    /// The next free scene id (`sc0001`).
    pub fn scene(&mut self) -> String {
        let n = self.scene;
        self.scene = n.saturating_add(1);
        format!("sc{n:04}")
    }

    /// The next free beat id (`bt0001`).
    pub fn beat(&mut self) -> String {
        let n = self.beat;
        self.beat = n.saturating_add(1);
        format!("bt{n:04}")
    }

    /// The next free line id (`ln0001`).
    pub fn line(&mut self) -> String {
        let n = self.line;
        self.line = n.saturating_add(1);
        format!("ln{n:04}")
    }

    /// Writes the counters into `next_ids` (never lowers one).
    pub fn store_in(&self, counters: &mut BTreeMap<String, u32>) {
        for (k, v) in [("scene", self.scene), ("beat", self.beat), ("line", self.line)] {
            let e = counters.entry(k.to_string()).or_insert(0);
            *e = (*e).max(v);
        }
    }
}

// ---------------------------------------------------------------- the cast

#[derive(Clone, Debug, Default)]
struct Person {
    id: String,
    name: String,
    aliases: Vec<String>,
    description: String,
}

/// Who can speak: the bible's characters (their ids), then the brain's characters the bible does
/// not have yet (id = the brain id, `safe_name`d, as the bible will name them).
#[derive(Clone, Debug, Default)]
pub struct Cast {
    people: Vec<Person>,
}

fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    !a.is_empty() && a.to_lowercase() == b.to_lowercase()
}

impl Cast {
    /// The cast the AI may name: the bible's characters, else the series brain's.
    pub fn new(bible: &BibleIndex, brain: &Brain) -> Cast {
        let mut people: Vec<Person> = bible
            .characters
            .iter()
            .filter(|c| !c.id.trim().is_empty())
            .map(|c| Person {
                id: c.id.clone(),
                name: if c.name.trim().is_empty() { c.id.clone() } else { c.name.trim().to_string() },
                aliases: c.aliases.clone(),
                description: c.description.clone(),
            })
            .collect();
        for b in &brain.characters {
            if b.name.trim().is_empty() {
                continue;
            }
            let id = mv_core::paths::safe_name(if b.id.trim().is_empty() { &b.name } else { &b.id });
            let in_bible = bible.characters.iter().any(|c| c.brain_id.as_deref() == Some(b.id.as_str()));
            if in_bible || people.iter().any(|p| p.id == id || same(&p.name, &b.name)) {
                continue;
            }
            people.push(Person { id, name: b.name.trim().to_string(), aliases: b.aliases.clone(), description: b.description.clone() });
        }
        Cast { people }
    }

    /// The character id of a name, alias or id (any case); `None` when nobody is called that.
    pub fn resolve(&self, name: &str) -> Option<String> {
        let n = name.trim();
        if n.is_empty() {
            return None;
        }
        self.people
            .iter()
            .find(|p| same(&p.name, n) || same(&p.id, n))
            .or_else(|| self.people.iter().find(|p| p.aliases.iter().any(|a| same(a, n))))
            .map(|p| p.id.clone())
    }

    /// The display name of a character id (the id itself when unknown).
    pub fn name_of(&self, id: &str) -> String {
        self.people.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_else(|| id.to_string())
    }

    /// Every character id.
    pub fn ids(&self) -> BTreeSet<String> {
        self.people.iter().map(|p| p.id.clone()).collect()
    }

    fn block(&self) -> String {
        if self.people.is_empty() {
            return "CHARACTERS: none known yet; use the names the manga gives.\n".into();
        }
        let mut out = String::from("CHARACTERS (write \"speaker\" exactly as one of these names):\n");
        for p in &self.people {
            let _ = write!(out, "- {}", p.name);
            let aliases: Vec<&str> = p.aliases.iter().map(|a| a.trim()).filter(|a| !a.is_empty()).collect();
            if !aliases.is_empty() {
                let _ = write!(out, " (also called {})", aliases.join(", "));
            }
            let desc = flat(&p.description);
            if !desc.is_empty() {
                let _ = write!(out, ": {}", clip(&desc, 240));
            }
            out.push('\n');
        }
        out
    }
}

fn flat(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

fn places_block(places: &[Place]) -> String {
    let names: Vec<String> = places.iter().map(|p| flat(&p.name)).filter(|n| !n.is_empty()).collect();
    if names.is_empty() {
        return String::new();
    }
    let mut out = String::from("PLACES ALREADY DESIGNED (use the name exactly when a scene is there):\n");
    for n in names {
        let _ = writeln!(out, "- {n}");
    }
    out
}

// ---------------------------------------------------------------- chunking

/// Scene kind of a manga page (`mv_recap::Scene.kind`) for the pacing table.
fn manga_kind(kind: &str) -> SceneKind {
    match kind.trim().to_lowercase().as_str() {
        "fight" | "action" => SceneKind::Action,
        "horror" => SceneKind::Dread,
        "flashback" => SceneKind::Flashback,
        _ => SceneKind::Dialogue,
    }
}

/// Splits a chapter into the page groups sent one call each (page indices, reading order).
///
/// The whole chapter when its digest fits in `cap` characters. Else runs of pages with the same
/// `scene.kind` are packed into parts of at most `cap`, cutting only where the kind changes; a
/// run larger than `cap` on its own is cut at page boundaries; a single page larger than `cap`
/// is a part of its own (its digest is cut at `cap`).
pub fn chunk_pages(read: &ChapterRead, cap: usize) -> Vec<Vec<usize>> {
    let mut pages: Vec<&mv_recap::PageRead> = read.pages.iter().collect();
    pages.sort_by_key(|p| p.index);
    pages.dedup_by_key(|p| p.index);
    let sized: Vec<(usize, usize, String)> =
        pages.iter().map(|p| (p.index, source::digest_page(p).chars().count(), p.scene.kind.trim().to_lowercase())).collect();
    if sized.is_empty() {
        return Vec::new();
    }
    let cap = cap.max(1);
    if sized.iter().map(|s| s.1).sum::<usize>() <= cap {
        return vec![sized.iter().map(|s| s.0).collect()];
    }
    let mut runs: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut last_kind: Option<&str> = None;
    for (index, size, kind) in &sized {
        let same_run = last_kind == Some(kind.as_str());
        match runs.last_mut() {
            Some(run) if same_run => run.push((*index, *size)),
            _ => runs.push(vec![(*index, *size)]),
        }
        last_kind = Some(kind.as_str());
    }
    let mut chunks: Vec<Vec<usize>> = Vec::new();
    let (mut cur, mut used): (Vec<usize>, usize) = (Vec::new(), 0);
    for run in runs {
        let size: usize = run.iter().map(|r| r.1).sum();
        if !cur.is_empty() && used + size > cap {
            chunks.push(std::mem::take(&mut cur));
            used = 0;
        }
        if size <= cap {
            cur.extend(run.iter().map(|r| r.0));
            used += size;
            continue;
        }
        for (index, size) in run {
            if !cur.is_empty() && used + size > cap {
                chunks.push(std::mem::take(&mut cur));
                used = 0;
            }
            cur.push(index);
            used += size;
        }
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    chunks
}

/// The digest of some pages of a chapter, cut at `cap` characters.
fn digest_of(read: &ChapterRead, pages: &[usize], cap: usize) -> String {
    let mut out = String::new();
    for i in pages {
        if let Some(p) = read.pages.iter().find(|p| p.index == *i) {
            out.push_str(&source::digest_page(p));
        }
    }
    if out.chars().count() > cap {
        out = out.chars().take(cap).collect();
    }
    out
}

// ---------------------------------------------------------------- the length budget

/// Screen time of a page's panels alone (pacing table seconds per panel, by the page's kind).
fn page_seconds(page: &mv_recap::PageRead) -> f32 {
    page.panels.len().max(1) as f32 * pacing::pace(manga_kind(&page.scene.kind)).secs_per_panel
}

/// Screen time of a chapter's panels alone.
fn panel_seconds(read: &ChapterRead) -> f32 {
    read.pages.iter().map(page_seconds).sum()
}

/// The share of a chapter's panel time on some of its pages (a part's share of the budget).
fn share_of(read: &ChapterRead, pages: &[usize]) -> f32 {
    let all = panel_seconds(read);
    if all <= 0. {
        return 1.;
    }
    read.pages.iter().filter(|p| pages.contains(&p.index)).map(page_seconds).sum::<f32>() / all
}

/// Auto screen time of a chapter: its panels at the pacing table's seconds per panel, plus the
/// anime's own beats (45% of an episode comes from no panel), between 6 and 9 minutes.
pub fn chapter_seconds(read: &ChapterRead) -> f32 {
    (panel_seconds(read) / FROM_PANELS).clamp(CHAPTER_MIN_S, CHAPTER_MAX_S)
}

/// Seconds for each chapter of the episode (same order as `episode.chapters`): the episode's
/// `target_minutes` shared out by each chapter's panel time when set, else [`chapter_seconds`].
/// An unread chapter (`None`) counts as an average one.
pub fn budgets(episode: &Episode, reads: &[Option<ChapterRead>]) -> Vec<f32> {
    let n = episode.chapters.len();
    let read = |i: usize| reads.get(i).and_then(Option::as_ref);
    let target = episode.target_minutes;
    if target.is_finite() && target > 0. {
        let weights: Vec<Option<f32>> = (0..n).map(|i| read(i).map(panel_seconds).filter(|w| *w > 0.)).collect();
        let known: Vec<f32> = weights.iter().flatten().copied().collect();
        let mean = if known.is_empty() { 1. } else { known.iter().sum::<f32>() / known.len() as f32 };
        let weights: Vec<f32> = weights.iter().map(|w| w.unwrap_or(mean)).collect();
        let sum: f32 = weights.iter().sum();
        let total = target * 60.;
        return weights.iter().map(|w| if sum > 0. { total * w / sum } else { total / n.max(1) as f32 }).collect();
    }
    let auto: Vec<Option<f32>> = (0..n).map(|i| read(i).map(chapter_seconds)).collect();
    let known: Vec<f32> = auto.iter().flatten().copied().collect();
    let mean = if known.is_empty() { (CHAPTER_MIN_S + CHAPTER_MAX_S) / 2. } else { known.iter().sum::<f32>() / known.len() as f32 };
    auto.iter().map(|a| a.unwrap_or(mean)).collect()
}

/// The lines of a scene (by its beats, then any other line naming it).
fn scene_lines<'a>(scene: &Scene, lines: &'a [Line]) -> Vec<&'a Line> {
    lines.iter().filter(|l| l.scene == scene.id || scene.beats.iter().any(|b| b.lines.contains(&l.id))).collect()
}

/// The shortest a scene may be: the time its lines take with 15% air, at least 2 s.
pub fn scene_floor(scene: &Scene, lines: &[Line]) -> f32 {
    let own: Vec<Line> = scene_lines(scene, lines).into_iter().cloned().collect();
    (pacing::spoken_seconds(&own, scene.kind) * AIR).max(SCENE_MIN_S)
}

/// Manga panels a scene adapts (a page without a panel counts once), at least 1.
fn panels_of(scene: &Scene, lines: &[Line]) -> usize {
    let mut seen: Vec<(String, usize, Option<usize>)> = Vec::new();
    let refs = scene.source.iter().chain(scene.beats.iter().flat_map(|b| b.source.iter())).chain(scene_lines(scene, lines).into_iter().filter_map(|l| l.source.as_ref()));
    for r in refs {
        let key = (r.chapter_id.clone(), r.page, r.panel);
        if !seen.contains(&key) {
            seen.push(key);
        }
    }
    seen.len().max(1)
}

/// Gives every scene its length: the AI's `target_s`, else one from its panels (pacing table);
/// never under the time its lines take ([`scene_floor`]). With `budget_s > 0` the lengths are
/// then scaled so the scenes share about `budget_s`, no scene going under its floor (when the
/// floors alone are more than the budget, every scene gets its floor). Each scene stays within
/// 2 s to 15 min, rounded to 0.1 s.
pub fn fit_budget(scenes: &mut [Scene], lines: &[Line], budget_s: f32) {
    let n = scenes.len();
    let floors: Vec<f32> = scenes.iter().map(|s| scene_floor(s, lines)).collect();
    let raw: Vec<f32> = scenes
        .iter()
        .zip(&floors)
        .map(|(s, floor)| {
            let t = s.target_s;
            let given = if t.is_finite() && t > 0. { t } else { pacing::scene_seconds(s.kind, panels_of(s, lines), floor / AIR) };
            given.max(*floor)
        })
        .collect();
    let mut out = raw.clone();
    if budget_s.is_finite() && budget_s > 0. && n > 0 {
        let mut fixed = vec![false; n];
        loop {
            let fixed_sum: f32 = (0..n).filter(|&i| fixed[i]).map(|i| floors[i]).sum();
            let free_raw: f32 = (0..n).filter(|&i| !fixed[i]).map(|i| raw[i]).sum();
            if free_raw <= 0. {
                out.clone_from(&floors);
                break;
            }
            let f = ((budget_s - fixed_sum) / free_raw).max(0.);
            let newly: Vec<usize> = (0..n).filter(|&i| !fixed[i] && raw[i] * f < floors[i]).collect();
            if newly.is_empty() {
                for i in 0..n {
                    out[i] = if fixed[i] { floors[i] } else { raw[i] * f };
                }
                break;
            }
            for i in newly {
                fixed[i] = true;
            }
        }
    }
    for ((s, t), floor) in scenes.iter_mut().zip(out).zip(&floors) {
        // Rounded to 0.1 s, never below the floor (rounding down must not leave it short).
        let t = (t.clamp(SCENE_MIN_S, SCENE_MAX_S) * 10.).round() / 10.;
        s.target_s = t.max(ceil_tenth(*floor)).min(SCENE_MAX_S);
    }
}

/// `x` rounded up to 0.1 (a hair of float noise does not round up).
fn ceil_tenth(x: f32) -> f32 {
    ((x * 10. - 1e-3).ceil() / 10.).max(0.)
}

/// Raises every scene whose length is the engine's own (not the user's: `/target_s` unlocked) to
/// at least the time its lines take ([`scene_floor`]); returns how many changed. Run after a
/// merge, where kept or added lines may need more time than the length that was fitted.
pub fn raise_to_floor(story: &mut Story) -> usize {
    let mut changed = 0;
    let lines = story.lines.clone();
    for s in &mut story.scenes {
        if crate::merge::is_locked("/target_s", &s.locked) {
            continue;
        }
        let floor = ceil_tenth(scene_floor(s, &lines)).min(SCENE_MAX_S);
        if s.target_s < floor {
            s.target_s = floor;
            changed += 1;
        }
    }
    changed
}

// ---------------------------------------------------------------- reading the answer

/// What one answer of the model became.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Adapted {
    pub scenes: Vec<Scene>,
    pub lines: Vec<Line>,
    /// What was dropped from the answer (a page the chapter does not have, ...).
    pub issues: Vec<Issue>,
}

/// What an answer is read against.
pub struct MapCtx<'a> {
    /// The chapter the answer adapts (every `SourceRef` gets it).
    pub chapter_id: &'a str,
    /// Its Studio Read (valid pages and panels).
    pub read: &'a ChapterRead,
    pub cast: &'a Cast,
    /// The bible's places (a scene's place name becomes its id).
    pub places: &'a [Place],
}

fn text(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// `text` lower-cased with spaces and dashes as underscores (enum words).
fn word(v: &Value, key: &str) -> String {
    text(v, key).to_lowercase().replace([' ', '-'], "_")
}

fn flag(v: &Value, key: &str) -> bool {
    match v.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(s.trim().to_lowercase().as_str(), "true" | "yes" | "1"),
        Some(Value::Number(n)) => n.as_f64().is_some_and(|x| x != 0.),
        _ => false,
    }
}

fn num(v: &Value, key: &str) -> Option<f64> {
    match v.get(key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().trim_end_matches(['s', 'S']).trim().parse().ok(),
        _ => None,
    }
    .filter(|x| x.is_finite())
}

fn time_of(w: &str) -> TimeOfDay {
    match w {
        "dusk" | "evening" | "sunset" | "twilight" => TimeOfDay::Dusk,
        "night" | "midnight" | "nighttime" | "night_time" => TimeOfDay::Night,
        "dawn" | "sunrise" | "early_morning" => TimeOfDay::Dawn,
        "indoor" | "indoors" | "interior" | "inside" => TimeOfDay::Indoor,
        _ => TimeOfDay::Day,
    }
}

fn scene_kind_of(w: &str, fight: bool) -> SceneKind {
    match w {
        "dialogue" | "talk" | "conversation" => SceneKind::Dialogue,
        "action" | "fight" | "battle" | "chase" => SceneKind::Action,
        "dread" | "horror" | "suspense" | "tension" => SceneKind::Dread,
        "narration" => SceneKind::Narration,
        "flashback" | "memory" => SceneKind::Flashback,
        "montage" => SceneKind::Montage,
        _ if fight => SceneKind::Action,
        _ => SceneKind::Dialogue,
    }
}

fn tone_of(w: &str, fight: bool) -> Tone {
    match w {
        "drama" | "dramatic" | "emotional" | "sad" => Tone::Drama,
        "romance" | "romantic" | "love" => Tone::Romance,
        "action" => Tone::Action,
        "fight" | "battle" | "combat" => Tone::Fight,
        "horror" | "dread" | "scary" | "terror" => Tone::Horror,
        "comedy" | "funny" | "humor" | "humour" | "comic" => Tone::Comedy,
        "calm" | "peaceful" | "slice_of_life" | "quiet" => Tone::Calm,
        "mystery" | "suspense" | "tense" | "mysterious" => Tone::Mystery,
        _ if fight => Tone::Fight,
        _ => Tone::Drama,
    }
}

fn emotion_of(w: &str) -> EmotionKind {
    match w {
        "joy" | "happy" | "happiness" | "excited" | "excitement" | "amused" | "glad" | "cheerful" => EmotionKind::Joy,
        "sadness" | "sad" | "sorrow" | "melancholy" => EmotionKind::Sadness,
        "anger" | "angry" | "rage" | "fury" | "furious" | "annoyed" => EmotionKind::Anger,
        "fear" | "scared" | "afraid" | "terror" | "terrified" | "panic" | "anxious" | "anxiety" | "nervous" => EmotionKind::Fear,
        "surprise" | "surprised" | "shock" | "shocked" | "astonished" => EmotionKind::Surprise,
        "disgust" | "disgusted" | "contempt" => EmotionKind::Disgust,
        "resolve" | "determined" | "determination" | "resolute" => EmotionKind::Resolve,
        "pride" | "proud" | "confident" | "confidence" | "smug" => EmotionKind::Pride,
        "grief" | "mourning" | "grieving" => EmotionKind::Grief,
        "despair" | "hopeless" | "hopelessness" => EmotionKind::Despair,
        "relief" | "relieved" => EmotionKind::Relief,
        "awe" | "wonder" | "amazed" | "amazement" => EmotionKind::Awe,
        "tender" | "tenderness" | "love" | "affection" | "caring" | "gentle" => EmotionKind::Tender,
        "menace" | "menacing" | "threatening" | "threat" | "sinister" => EmotionKind::Menace,
        _ => EmotionKind::Neutral,
    }
}

fn line_kind_of(w: &str, no_speaker: bool) -> LineKind {
    match w {
        "speech" | "dialogue" | "said" | "shout" | "spoken" => LineKind::Speech,
        "thought" | "inner" | "inner_voice" | "monologue" | "thinking" => LineKind::Thought,
        "narration" | "narrator" | "caption" => LineKind::Narration,
        "sfx" | "sound" | "sound_effect" | "effect" => LineKind::Sfx,
        _ if no_speaker => LineKind::Narration,
        _ => LineKind::Speech,
    }
}

fn pace_of(w: &str, kind: SceneKind) -> Pace {
    match w {
        "slow" => Pace::Slow,
        "fast" | "quick" => Pace::Fast,
        "normal" | "medium" => Pace::Normal,
        _ if kind == SceneKind::Action => Pace::Fast,
        _ => Pace::Normal,
    }
}

fn strength_of(v: &Value) -> u8 {
    let n = match v.get("strength") {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => match s.trim().to_lowercase().as_str() {
            "mild" | "low" | "weak" | "slight" => Some(1.),
            "medium" | "moderate" => Some(2.),
            "strong" | "high" | "intense" => Some(3.),
            other => other.parse().ok(),
        },
        _ => None,
    };
    n.filter(|x| x.is_finite() && *x > 0.).map(|x| x.round().clamp(1., 3.) as u8).unwrap_or(2)
}

/// The colour theme for what the model wrote: one of [`COLOUR_THEMES`] by name, else the one
/// its words point to, else the usual one for the scene's kind and tone.
pub fn colour_theme(raw: &str, kind: SceneKind, tone: Tone) -> String {
    let w = raw.trim().to_lowercase().replace(['_', '-'], " ");
    if let Some((name, _)) = COLOUR_THEMES.iter().find(|(n, _)| *n == w) {
        return name.to_string();
    }
    let has = |keys: &[&str]| keys.iter().any(|k| w.contains(k));
    let pick = if w.is_empty() {
        None
    } else if has(&["grey", "gray", "memory", "sepia", "flashback"]) {
        Some("grey memory")
    } else if has(&["loss", "death", "mourn", "icy", "grief"]) {
        Some("cold blue loss")
    } else if has(&["red", "rage", "blood", "burn", "crimson"]) {
        Some("burning red")
    } else if has(&["blue", "threat", "dread", "dark", "menace"]) {
        Some("deep blue threat")
    } else if has(&["fire", "hope", "candle", "torch"]) {
        Some("firelight hope")
    } else if has(&["amber", "dusk", "sunset", "orange", "gold"]) {
        Some("dusk amber")
    } else if has(&["home", "family", "warm", "cozy", "cosy"]) {
        Some("warm home")
    } else if has(&["glow", "magic", "eerie", "green", "purple", "system", "aura", "neon"]) {
        Some("eerie glow")
    } else if has(&["day", "white", "bright", "neutral", "sun"]) {
        Some("neutral daylight")
    } else {
        None
    };
    pick.unwrap_or_else(|| default_colour(kind, tone)).to_string()
}

fn default_colour(kind: SceneKind, tone: Tone) -> &'static str {
    match (kind, tone) {
        (SceneKind::Flashback, _) => "grey memory",
        (SceneKind::Dread, _) | (_, Tone::Horror | Tone::Mystery) => "deep blue threat",
        (SceneKind::Action, _) | (_, Tone::Fight | Tone::Action) => "burning red",
        (_, Tone::Romance) => "dusk amber",
        (_, Tone::Comedy) => "warm home",
        _ => "neutral daylight",
    }
}

/// The pages (index -> panel count and manga intensity) an answer may point at.
struct Pages {
    chapter: String,
    pages: BTreeMap<usize, (usize, u8)>,
}

impl Pages {
    fn new(chapter: &str, read: &ChapterRead) -> Pages {
        Pages { chapter: chapter.to_string(), pages: read.pages.iter().map(|p| (p.index, (p.panels.len(), p.scene.intensity))).collect() }
    }

    fn first(&self) -> usize {
        self.pages.keys().next().copied().unwrap_or(0)
    }

    /// A reference to a page this chapter has (a panel it does not have is dropped, the page
    /// kept); `None` and an issue for a page it does not have.
    fn check(&self, page: i64, panel: Option<i64>, scene: &str, issues: &mut Vec<Issue>) -> Option<SourceRef> {
        let known = usize::try_from(page).ok().and_then(|p| self.pages.get(&p).map(|v| (p, v.0)));
        let Some((page, panels)) = known else {
            let message = format!("The AI pointed at page {page}, which this chapter does not have; that reference was dropped.");
            if !issues.iter().any(|i| i.scene == scene && i.message == message) {
                issues.push(issue("unknown_page", Severity::Info, scene, message, None));
            }
            return None;
        };
        let panel = panel.and_then(|k| usize::try_from(k).ok()).filter(|k| *k < panels);
        Some(SourceRef { chapter_id: self.chapter.clone(), page, panel })
    }

    fn intensity(&self, refs: &[SourceRef]) -> Option<u8> {
        let v: Vec<u32> = refs.iter().filter_map(|r| self.pages.get(&r.page)).map(|p| u32::from(p.1.min(10))).collect();
        (!v.is_empty()).then(|| (v.iter().sum::<u32>() as f32 / v.len() as f32).round() as u8)
    }
}

/// `(page, panel)` of one reference written as `{"page":3,"panel":1}`, `3` or `"p3 k1"`.
fn one_ref(v: &Value) -> Option<(i64, Option<i64>)> {
    let int = |x: &Value| match x {
        Value::Number(n) => n.as_f64().filter(|f| f.is_finite()).map(|f| f.round() as i64),
        Value::String(s) => s.trim().trim_start_matches(['p', 'P', 'k', 'K']).parse().ok(),
        _ => None,
    };
    match v {
        Value::Object(o) => {
            let page = o.get("page").or_else(|| o.get("p")).and_then(int)?;
            Some((page, o.get("panel").or_else(|| o.get("k")).and_then(int)))
        }
        Value::Number(_) => int(v).map(|p| (p, None)),
        Value::String(s) => {
            let groups: Vec<i64> = s.split(|c: char| !c.is_ascii_digit()).filter(|g| !g.is_empty()).filter_map(|g| g.parse().ok()).collect();
            groups.first().map(|p| (*p, groups.get(1).copied()))
        }
        _ => None,
    }
}

fn refs(v: &Value, key: &str, pages: &Pages, scene: &str, issues: &mut Vec<Issue>) -> Vec<SourceRef> {
    let list: Vec<&Value> = match v.get(key) {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(x @ (Value::Object(_) | Value::Number(_) | Value::String(_))) => vec![x],
        _ => Vec::new(),
    };
    let mut out = Vec::new();
    for item in list {
        if let Some(r) = one_ref(item).and_then(|(p, k)| pages.check(p, k, scene, issues)) {
            out.push(r);
        }
    }
    dedupe(&mut out);
    out
}

fn dedupe(v: &mut Vec<SourceRef>) {
    let mut seen: Vec<SourceRef> = Vec::new();
    v.retain(|r| {
        if seen.contains(r) {
            false
        } else {
            seen.push(r.clone());
            true
        }
    });
}

fn issue(code: &str, severity: Severity, scene: &str, message: String, fix: Option<&str>) -> Issue {
    Issue { code: code.into(), severity, scene: scene.into(), shot: None, message, fix: fix.map(str::to_string) }
}

/// The scenes of an answer: `"scenes"`, or the first list of scene-like objects in it.
fn scenes_of(v: &Value) -> Option<&Vec<Value>> {
    if let Some(a) = v.get("scenes").and_then(Value::as_array) {
        return Some(a);
    }
    let looks = |a: &&Vec<Value>| a.iter().any(|s| s.get("beats").is_some() || s.get("lines").is_some());
    v.as_object()?.values().find_map(|x| match x {
        Value::Array(a) => Some(a).filter(looks),
        Value::Object(_) => x.get("scenes").and_then(Value::as_array),
        _ => None,
    })
}

struct LineCtx<'a> {
    scene: &'a str,
    kind: SceneKind,
    pages: &'a Pages,
    cast: &'a Cast,
}

fn map_line(l: &Value, cx: &LineCtx<'_>, issues: &mut Vec<Issue>, ids: &mut Ids) -> Option<Line> {
    let original = text(l, "original");
    let mut spoken = text(l, "spoken");
    if spoken.is_empty() {
        spoken = text(l, "text");
    }
    if spoken.is_empty() {
        spoken = original.clone();
    }
    if spoken.is_empty() {
        return None;
    }
    let raw_speaker = text(l, "speaker");
    let no_speaker = matches!(raw_speaker.to_lowercase().as_str(), "" | "null" | "none" | "narrator" | "?" | "unknown");
    let kind = line_kind_of(&word(l, "kind"), no_speaker);
    let speaker = if kind == LineKind::Sfx || no_speaker { None } else { Some(cx.cast.resolve(&raw_speaker).unwrap_or(raw_speaker)) };
    let keep = flag(l, "keep") && !original.is_empty();
    if keep {
        spoken = original.clone();
    }
    let source = l.get("page").and_then(|p| one_ref(p).map(|r| r.0)).and_then(|page| {
        let panel = l.get("panel").and_then(|k| one_ref(k).map(|r| r.0));
        cx.pages.check(page, panel, cx.scene, issues)
    });
    Some(Line {
        id: ids.line(),
        scene: cx.scene.to_string(),
        speaker,
        kind,
        original,
        spoken,
        keep_words: keep,
        emotion: Emotion { kind: emotion_of(&word(l, "emotion")), strength: strength_of(l) },
        pace: pace_of(&word(l, "pace"), cx.kind),
        source,
        ..Line::default()
    })
}

/// Reads one answer of the model onto the model types, with fresh ids from `ids`.
///
/// Lenient: unknown words fall back to defaults, a missing scene field gets one from the manga
/// (intensity) or later (length, [`fit_budget`]); a speaker becomes a character id when the cast
/// knows the name or alias (else the name stays); a place becomes a bible place id when one has
/// that name (else it is written into the summary for the Bible step). References to pages the
/// chapter does not have are dropped and reported. A scene with no source gets the page it
/// sits next to. Fails (with a message for the user) only when no scene can be read.
pub fn map_reply(v: &Value, cx: &MapCtx<'_>, ids: &mut Ids) -> Result<Adapted> {
    let Some(raw_scenes) = scenes_of(v) else { bail!(NO_SCENES) };
    let pages = Pages::new(cx.chapter_id, cx.read);
    let mut out = Adapted::default();
    let mut issues = Vec::new();
    for s in raw_scenes.iter().filter(|s| s.is_object()) {
        let fight_flag = flag(s, "fight");
        let tone = tone_of(&word(s, "tone"), fight_flag);
        let fight = fight_flag || tone == Tone::Fight;
        let kind = scene_kind_of(&word(s, "kind"), fight);
        let id = ids.scene();
        let colour = if s.get("colour").is_some() { text(s, "colour") } else { text(s, "color") };
        let mut scene = Scene {
            id: id.clone(),
            title: flat(&text(s, "title")),
            time_of_day: time_of(&word(s, "time")),
            kind,
            tone,
            fight,
            colour: ColourTheme { name: colour_theme(&colour, kind, tone), palette: None },
            target_s: num(s, "target_s").or_else(|| num(s, "seconds")).map(|t| t as f32).filter(|t| *t > 0.).unwrap_or(0.),
            summary: flat(&text(s, "summary")),
            ..Scene::default()
        };
        let place = flat(&text(s, "place"));
        if !place.is_empty() && !matches!(place.to_lowercase().as_str(), "null" | "none" | "unknown") {
            match cx.places.iter().find(|p| same(&p.name, &place) || same(&p.id, &place)) {
                Some(p) => scene.place = Some(p.id.clone()),
                None if !scene.summary.to_lowercase().contains(&place.to_lowercase()) => {
                    scene.summary = if scene.summary.is_empty() { format!("Place: {place}.") } else { format!("Place: {place}. {}", scene.summary) };
                }
                None => {}
            }
        }
        scene.source = refs(s, "source", &pages, &id, &mut issues);
        let line_cx = LineCtx { scene: &id, kind, pages: &pages, cast: cx.cast };
        let mut own_lines: Vec<Line> = Vec::new();
        for b in s.get("beats").and_then(Value::as_array).into_iter().flatten().filter(|b| b.is_object()) {
            let mut beat = Beat { id: ids.beat(), purpose: flat(&text(b, "purpose")), source: refs(b, "source", &pages, &id, &mut issues), lines: Vec::new(), hook: flag(b, "hook") };
            for l in b.get("lines").and_then(Value::as_array).into_iter().flatten() {
                if let Some(line) = map_line(l, &line_cx, &mut issues, ids) {
                    beat.lines.push(line.id.clone());
                    own_lines.push(line);
                }
            }
            if beat.lines.is_empty() && beat.purpose.is_empty() && beat.source.is_empty() {
                continue;
            }
            scene.beats.push(beat);
        }
        // A scene written with its lines straight under it: one beat.
        if scene.beats.is_empty() {
            let mut beat = Beat { id: ids.beat(), purpose: scene.summary.clone(), source: scene.source.clone(), lines: Vec::new(), hook: false };
            for l in s.get("lines").and_then(Value::as_array).into_iter().flatten() {
                if let Some(line) = map_line(l, &line_cx, &mut issues, ids) {
                    beat.lines.push(line.id.clone());
                    own_lines.push(line);
                }
            }
            if !beat.lines.is_empty() {
                scene.beats.push(beat);
            }
        }
        if scene.beats.is_empty() && scene.summary.is_empty() {
            continue;
        }
        if scene.source.is_empty() {
            let mut found: Vec<SourceRef> = scene.beats.iter().flat_map(|b| b.source.iter().cloned()).chain(own_lines.iter().filter_map(|l| l.source.clone())).collect();
            dedupe(&mut found);
            scene.source = found;
        }
        let intensity = num(s, "intensity").map(|x| x.round().clamp(0., 10.) as u8).or_else(|| pages.intensity(&scene.source));
        scene.intensity = intensity.unwrap_or(if fight { 7 } else { 4 });
        if scene.title.is_empty() {
            scene.title = format!("Scene {}", out.scenes.len() + 1);
        }
        out.scenes.push(scene);
        out.lines.extend(own_lines);
    }
    if out.scenes.is_empty() {
        bail!(NO_SCENES);
    }
    // A scene the anime added sits next to the pages around it.
    let n = out.scenes.len();
    for i in 0..n {
        if out.scenes[i].source.is_empty() {
            let near = (i + 1..n)
                .find_map(|j| out.scenes[j].source.first().cloned())
                .or_else(|| (0..i).rev().find_map(|j| out.scenes[j].source.last().cloned()))
                .unwrap_or(SourceRef { chapter_id: cx.chapter_id.to_string(), page: pages.first(), panel: None });
            out.scenes[i].source = vec![SourceRef { panel: None, ..near }];
        }
    }
    out.issues = issues;
    Ok(out)
}

/// The last pages of `pages` (reading order) that come after every page `adapted` points at
/// (its scenes', beats' and lines' sources in this chapter): where a cut-off answer stopped.
/// Pages without panels (a blank or credits page) do not count.
pub fn uncovered_tail(adapted: &Adapted, chapter_id: &str, pages: &[usize], read: &ChapterRead) -> Vec<usize> {
    let covered: BTreeSet<usize> = adapted
        .scenes
        .iter()
        .flat_map(|s| s.source.iter().chain(s.beats.iter().flat_map(|b| b.source.iter())))
        .chain(adapted.lines.iter().filter_map(|l| l.source.as_ref()))
        .filter(|r| r.chapter_id == chapter_id)
        .map(|r| r.page)
        .collect();
    let last = covered.iter().next_back().copied();
    let mut out: Vec<usize> = pages
        .iter()
        .copied()
        .filter(|p| last.is_none_or(|l| *p > l))
        .filter(|p| read.pages.iter().any(|x| x.index == *p && !x.panels.is_empty()))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// "page 4", "pages 40-60", "pages 3, 5 and 7" (numbers as the reader counts, from 1).
fn pages_words(pages: &[usize]) -> String {
    let mut p: Vec<usize> = pages.iter().map(|x| x + 1).collect();
    p.sort_unstable();
    p.dedup();
    match p.as_slice() {
        [] => "no pages".into(),
        [one] => format!("page {one}"),
        [first, .., last] if last - first + 1 == p.len() => format!("pages {first}-{last}"),
        [rest @ .., last] => format!("pages {} and {last}", rest.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ")),
    }
}

// ---------------------------------------------------------------- checks (rule R3)

/// Checks every line against its scene (rule R3) and the cast: a speaker the cast does not
/// know, a long line under threat (more than 8 words in a fight, 10 in dread), a cheerful spoken
/// line while someone is in danger. Lines the user edited are only checked for their speaker.
pub fn review(story: &Story, known: &BTreeSet<String>) -> Vec<Issue> {
    let mut out = Vec::new();
    for scene in &story.scenes {
        let mut unknown = BTreeSet::new();
        let danger = scene.fight || scene.kind == SceneKind::Dread || scene.tone == Tone::Horror || scene.intensity >= 8;
        let limit = if scene.fight || scene.kind == SceneKind::Action {
            Some((8, "about 4 words in a fight"))
        } else if scene.kind == SceneKind::Dread {
            Some((10, "about 6 words in dread"))
        } else {
            None
        };
        for line in scene_lines(scene, &story.lines) {
            if line.kind == LineKind::Sfx {
                continue;
            }
            if let Some(sp) = line.speaker.as_deref().filter(|s| !known.contains(*s)) {
                if unknown.insert(sp.to_string()) {
                    out.push(issue(
                        "unknown_speaker",
                        Severity::Warning,
                        &scene.id,
                        format!("\u{201c}{sp}\u{201d} speaks in this scene but is not in the cast."),
                        Some("Pick the speaker from the cast. If they are a real character, build the Bible: when it adds them, their lines are linked to them and get their voice."),
                    ));
                }
            }
            if line.edited || !matches!(line.kind, LineKind::Speech | LineKind::Thought) {
                continue;
            }
            let words = pacing::words(&line.spoken);
            if let Some((_, usual)) = limit.filter(|(max, _)| words > *max) {
                out.push(issue(
                    "long_line_under_threat",
                    Severity::Info,
                    &scene.id,
                    format!("A long line under threat ({words} words): \u{201c}{}\u{201d}", clip(&line.spoken, 80)),
                    Some(format!("Lines under threat are short, {usual}. Shorten it, or keep it if the moment allows.").as_str()),
                ));
            }
            if danger && line.kind == LineKind::Speech && line.emotion.kind == EmotionKind::Joy {
                out.push(issue(
                    "tone_mismatch",
                    Severity::Warning,
                    &scene.id,
                    format!("A cheerful line while someone is in danger: \u{201c}{}\u{201d}", clip(&line.spoken, 80)),
                    Some("Make it fit the danger, turn it into an inner thought, or keep it if the speaker is in no danger."),
                ));
            }
        }
    }
    out
}

/// The issues saved with the story (unknown speakers, long lines under threat, ...).
pub fn issues(story: &Story) -> Vec<Issue> {
    story.extra.get(ISSUES).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

fn set_issues(story: &mut Story, mut list: Vec<Issue>) {
    let mut seen = HashSet::new();
    list.retain(|i| seen.insert((i.code.clone(), i.scene.clone(), i.message.clone())));
    story.extra.insert(ISSUES.to_string(), serde_json::to_value(list).unwrap_or_else(|_| Value::Array(Vec::new())));
}

/// Fresh checks of `story`, plus the answer-reading issues (`extra`, then the old ones) of
/// scenes that are still there.
fn refresh_issues(story: &mut Story, cast: &Cast, extra: &[Issue], old: &[Issue]) {
    let scenes: HashSet<&str> = story.scenes.iter().map(|s| s.id.as_str()).collect();
    let mut list = review(story, &cast.ids());
    list.extend(extra.iter().chain(old).filter(|i| MAPPING_CODES.contains(&i.code.as_str()) && scenes.contains(i.scene.as_str())).cloned());
    set_issues(story, list);
}

/// Links every line's speaker to its character id now that the cast may know more people (the
/// Bible step added them): a name, alias or id the cast resolves becomes that character's id. A
/// speaker the user locked (`/speaker`) stays as it is. The saved checks are made again, so an
/// "unknown speaker" note goes once the speaker is known. Returns how many lines changed.
pub fn relink_speakers(story: &mut Story, cast: &Cast) -> usize {
    let mut changed = 0;
    for l in &mut story.lines {
        if crate::merge::is_locked("/speaker", &l.locked) {
            continue;
        }
        let Some(sp) = l.speaker.as_deref() else { continue };
        if let Some(id) = cast.resolve(sp).filter(|id| id != sp) {
            l.speaker = Some(id);
            changed += 1;
        }
    }
    let old = issues(story);
    refresh_issues(story, cast, &[], &old);
    changed
}

// ---------------------------------------------------------------- merging

/// A line holds the user's work: edited, locked, or its voice locked or approved.
fn line_kept(l: &Line) -> bool {
    l.edited || !l.locked.is_empty() || l.voice.locked || l.voice.status == Status::Approved
}

/// The scene holds the user's work: it was edited or has a lock, or one of its lines does
/// ([`merge`] keeps such a scene as it is).
pub fn is_kept(scene: &Scene, lines: &[Line]) -> bool {
    scene.edited || !scene.locked.is_empty() || scene_lines(scene, lines).into_iter().any(line_kept)
}

/// A scene's planned shots hold the user's work: a shot edited or locked, or one with pictures
/// (the director keeps such shots across a re-plan, so the story keeps their scene).
pub fn shots_hold_work(shots: &[Shot]) -> bool {
    shots.iter().any(|s| {
        s.edited
            || !s.locked.is_empty()
            || !s.frame.takes.is_empty()
            || s.frame.selected.is_some()
            || s.frame.locked
            || s.clip.as_ref().is_some_and(|c| !c.takes.is_empty() || c.selected.is_some())
    })
}

/// The scenes of an episode whose shots file holds the user's work ([`shots_hold_work`]).
fn worked_scenes(store: &Store, episode: &str) -> HashSet<String> {
    match store.all_shots(episode) {
        Ok(all) => all.into_iter().filter(|(_, shots)| shots_hold_work(shots)).map(|(id, _)| id).collect(),
        Err(e) => {
            warn(&format!("story: the planned shots could not be read, so every scene is kept: {e:#}"));
            // Unknown: keep every scene (losing work is worse than an old scene staying).
            store.story(episode).map(|s| s.scenes.into_iter().map(|s| s.id).collect()).unwrap_or_default()
        }
    }
}

/// How many scenes with planned shots "Adapt" would replace now: scenes from the manga that
/// hold none of the user's work, neither in the story nor in their shots, but have a plan.
/// Read-only, for the Story screen to ask before a re-run. (A new scene starting on the same
/// page takes the old one's id and so its plan; this counts the most that can be lost.)
pub fn replaced_with_shots(store: &Store, episode: &str) -> usize {
    let Ok(story) = store.story(episode) else { return 0 };
    let all = match store.all_shots(episode) {
        Ok(all) => all,
        Err(e) => {
            warn(&format!("story: the planned shots could not be read: {e:#}"));
            return 0;
        }
    };
    story
        .scenes
        .iter()
        .filter(|s| home(s).is_some() && !is_kept(s, &story.lines))
        .filter(|s| all.get(&s.id).is_some_and(|shots| !shots.is_empty() && !shots_hold_work(shots)))
        .count()
}

/// The first manga reference of a scene (its own, else its first beat's).
fn anchor(s: &Scene) -> Option<&SourceRef> {
    s.source.first().or_else(|| s.beats.iter().find_map(|b| b.source.first()))
}

fn home(s: &Scene) -> Option<String> {
    anchor(s).map(|r| r.chapter_id.clone())
}

type Key = (usize, usize, usize);

fn key_of(r: &SourceRef, order: &[String]) -> Key {
    let rank = order.iter().position(|c| *c == r.chapter_id).unwrap_or(order.len());
    (rank, r.page, r.panel.unwrap_or(0))
}

/// Lines in scene order, each scene's by its beats, then its lines outside any beat; lines of
/// no scene are dropped.
fn order_lines(scenes: &[Scene], pool: Vec<Line>) -> Vec<Line> {
    let mut pool: Vec<Option<Line>> = pool.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(pool.len());
    for s in scenes {
        for b in &s.beats {
            for id in &b.lines {
                if let Some(slot) = pool.iter_mut().find(|l| l.as_ref().is_some_and(|l| &l.id == id && l.scene == s.id)) {
                    out.extend(slot.take());
                }
            }
        }
        for slot in pool.iter_mut() {
            if slot.as_ref().is_some_and(|l| l.scene == s.id) {
                out.extend(slot.take());
            }
        }
    }
    out
}

/// Merges a new adaptation into the current one: edited scenes (or scenes holding an edited
/// line) stay as they are with their ids; the others are replaced by the new scenes covering
/// the same source pages; new scenes go in source order.
///
/// Only the chapters the new adaptation covers are touched: scenes of other chapters, and
/// scenes with no manga source (added by hand), stay. A scene "holds the user's work" when it or
/// one of its lines is edited or locked, or a line's voice is locked or approved
/// ([`is_kept`]). A new scene starting on the same page as a kept scene of its chapter is left
/// out (the user's version stands for it). A new scene starting on the same page as a replaced
/// scene takes that scene's id (so its planned shots stay attached). Other new ids that clash
/// with kept ones are renewed. Chapter order: as the scenes appear (old, then new); [`run_job`]
/// uses the episode's order.
pub fn merge(old: &Story, new: Story) -> Story {
    merge_keeping(old, new, &HashSet::new())
}

/// [`merge`], also keeping every scene in `worked` (scenes whose planned shots hold the user's
/// work: edits, locks or pictures, see [`shots_hold_work`]).
pub fn merge_keeping(old: &Story, new: Story, worked: &HashSet<String>) -> Story {
    let mut order: Vec<String> = Vec::new();
    for c in old.scenes.iter().chain(&new.scenes).filter_map(home) {
        if !order.contains(&c) {
            order.push(c);
        }
    }
    merge_in(old, new, &order, worked)
}

/// A new line joins `scene` under an id nobody else uses.
fn claim(mut line: Line, used: &mut HashSet<String>, ids: &mut Ids, scene: &str) -> Line {
    if line.id.is_empty() || used.contains(&line.id) {
        line.id = ids.line();
    }
    used.insert(line.id.clone());
    line.scene = scene.to_string();
    line
}

/// [`merge`] with the episode's chapter order. Scenes of a chapter no longer in `order` are
/// dropped unless they hold the user's work (in the story, or in their shots: `worked`).
fn merge_in(old: &Story, new: Story, order: &[String], worked: &HashSet<String>) -> Story {
    let covered: HashSet<String> = new.scenes.iter().filter_map(home).collect();
    let mut kept: Vec<(Key, Scene)> = Vec::new();
    // Replaced scenes by the page they start on: a new scene starting there takes the id.
    let mut heirs: BTreeMap<(String, usize), String> = BTreeMap::new();
    let mut last: Key = (0, 0, 0);
    for s in &old.scenes {
        let key = anchor(s).map(|r| key_of(r, order)).unwrap_or(last);
        last = key;
        let keep = is_kept(s, &old.lines)
            || worked.contains(&s.id)
            || match home(s) {
                Some(c) => !covered.contains(&c) && order.contains(&c),
                None => true,
            };
        if keep {
            kept.push((key, s.clone()));
        } else if let Some(r) = anchor(s).filter(|_| !s.id.is_empty()) {
            heirs.entry((r.chapter_id.clone(), r.page)).or_insert_with(|| s.id.clone());
        }
    }
    let heir_ids: HashSet<String> = heirs.values().cloned().collect();
    let taken: HashSet<(String, usize)> = kept.iter().filter_map(|(_, s)| anchor(s)).filter(|r| covered.contains(&r.chapter_id)).map(|r| (r.chapter_id.clone(), r.page)).collect();
    let mut fresh: Vec<(Key, Scene)> = Vec::new();
    let mut fresh_last: Key = (0, 0, 0);
    for s in new.scenes {
        if anchor(&s).is_some_and(|r| taken.contains(&(r.chapter_id.clone(), r.page))) {
            continue;
        }
        let key = anchor(&s).map(|r| key_of(r, order)).unwrap_or(fresh_last);
        fresh_last = key;
        fresh.push((key, s));
    }

    // Ids: the kept ones stand; a new one that is empty or already used is renewed.
    let kept_ids: HashSet<String> = kept.iter().map(|(_, s)| s.id.clone()).collect();
    let mut kept_lines: Vec<Line> = old.lines.iter().filter(|l| kept_ids.contains(&l.scene)).cloned().collect();
    for (_, s) in &kept {
        for b in &s.beats {
            for id in &b.lines {
                if !kept_lines.iter().any(|l| &l.id == id) {
                    if let Some(l) = old.lines.iter().find(|l| &l.id == id) {
                        let mut l = l.clone();
                        l.scene = s.id.clone();
                        kept_lines.push(l);
                    }
                }
            }
        }
    }
    let mut used_scenes = kept_ids;
    let mut used_beats: HashSet<String> = kept.iter().flat_map(|(_, s)| s.beats.iter().map(|b| b.id.clone())).collect();
    let mut used_lines: HashSet<String> = kept_lines.iter().map(|l| l.id.clone()).collect();
    let mut ids = Ids::new(&BTreeMap::new(), old);
    ids.raise(&Story { scenes: fresh.iter().map(|(_, s)| s.clone()).collect(), lines: new.lines.clone(), ..Story::default() });
    let mut pool = new.lines;
    let mut fresh_lines: Vec<Line> = Vec::new();
    for (_, s) in fresh.iter_mut() {
        let was = s.id.clone();
        let heir = anchor(s).and_then(|r| heirs.remove(&(r.chapter_id.clone(), r.page))).filter(|id| !used_scenes.contains(id));
        match heir {
            Some(id) => s.id = id,
            None if was.is_empty() || used_scenes.contains(&was) || heir_ids.contains(&was) => s.id = ids.scene(),
            None => {}
        }
        used_scenes.insert(s.id.clone());
        let (mine, rest): (Vec<Line>, Vec<Line>) = std::mem::take(&mut pool).into_iter().partition(|l| l.scene == was);
        pool = rest;
        let mut mine: Vec<Option<Line>> = mine.into_iter().map(Some).collect();
        let scene_id = s.id.clone();
        for b in s.beats.iter_mut() {
            if b.id.is_empty() || used_beats.contains(&b.id) {
                b.id = ids.beat();
            }
            used_beats.insert(b.id.clone());
            let mut refs = Vec::new();
            for id in &b.lines {
                if let Some(line) = mine.iter_mut().find(|l| l.as_ref().is_some_and(|l| &l.id == id)).and_then(Option::take) {
                    let line = claim(line, &mut used_lines, &mut ids, &scene_id);
                    refs.push(line.id.clone());
                    fresh_lines.push(line);
                }
            }
            b.lines = refs;
        }
        for line in mine.into_iter().flatten() {
            fresh_lines.push(claim(line, &mut used_lines, &mut ids, &scene_id));
        }
    }

    // Interleave: kept scenes keep their order, new ones go in by source position.
    let mut scenes = Vec::with_capacity(kept.len() + fresh.len());
    let (mut kept, mut fresh) = (kept.into_iter().peekable(), fresh.into_iter().peekable());
    loop {
        let take_kept = match (kept.peek(), fresh.peek()) {
            (Some(k), Some(f)) => k.0 <= f.0,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        let next = if take_kept { kept.next() } else { fresh.next() };
        if let Some((_, s)) = next {
            scenes.push(s);
        }
    }
    let lines = order_lines(&scenes, kept_lines.into_iter().chain(fresh_lines).collect());
    let mut extra = old.extra.clone();
    extra.extend(new.extra);
    Story {
        schema: old.schema.max(new.schema),
        episode: if new.episode.is_empty() { old.episode.clone() } else { new.episode },
        scenes,
        lines,
        model: if new.model.is_empty() { old.model.clone() } else { new.model },
        made_at: old.made_at.max(new.made_at),
        extra,
    }
}

/// "Adapt this scene again": puts the first scene of `fresh` in place of scene `scene_id`,
/// keeping its id and every field the user locked (`merge_unlocked`). An old line the new one
/// stands for (same panel and speaker) keeps its id and voice; an edited one stays exactly as
/// it is, and edited or locked lines the new scene has no place for are kept in the beat of
/// their page (else the last beat). Any other line of the old scene goes. Nothing changes when
/// the scene is gone or `fresh` has no scene.
pub fn replace_scene(old: &Story, scene_id: &str, fresh: Story) -> Story {
    let Some(at) = old.scenes.iter().position(|s| s.id == scene_id) else { return old.clone() };
    let Some(mut scene) = fresh.scenes.first().cloned() else { return old.clone() };
    let was = scene.id.clone();
    let current = &old.scenes[at];
    scene.id = current.id.clone();
    let old_lines: Vec<Line> = scene_lines(current, &old.lines).into_iter().cloned().collect();
    let mut used: Vec<bool> = vec![false; old_lines.len()];
    let mut new_lines: Vec<Line> = Vec::new();
    let mut ids = Ids::new(&BTreeMap::new(), old);
    let other_ids: HashSet<String> = old.lines.iter().filter(|l| !old_lines.iter().any(|o| o.id == l.id)).map(|l| l.id.clone()).collect();
    let mut beat_ids: HashSet<String> = old.scenes.iter().filter(|s| s.id != scene_id).flat_map(|s| s.beats.iter().map(|b| b.id.clone())).collect();
    for b in scene.beats.iter_mut() {
        if b.id.is_empty() || beat_ids.contains(&b.id) {
            b.id = ids.beat();
        }
        beat_ids.insert(b.id.clone());
        let mut refs = Vec::new();
        for id in &b.lines {
            let Some(line) = fresh.lines.iter().find(|l| &l.id == id && l.scene == was) else { continue };
            let mut line = line.clone();
            line.scene = scene.id.clone();
            let twin = old_lines.iter().enumerate().position(|(i, o)| !used[i] && o.source.is_some() && o.source == line.source && o.speaker == line.speaker);
            let line = match twin {
                Some(i) => {
                    used[i] = true;
                    let mine = &old_lines[i];
                    if mine.edited {
                        mine.clone()
                    } else {
                        line.id = mine.id.clone();
                        let mut cur = serde_json::to_value(mine).unwrap_or(Value::Null);
                        let ai = serde_json::to_value(&line).unwrap_or(Value::Null);
                        merge_unlocked(&mut cur, &ai, &mine.locked);
                        serde_json::from_value::<Line>(cur).unwrap_or(Line { voice: mine.voice.clone(), ..line })
                    }
                }
                None => {
                    if line.id.is_empty() || other_ids.contains(&line.id) || old_lines.iter().any(|o| o.id == line.id) || new_lines.iter().any(|n| n.id == line.id) {
                        line.id = ids.line();
                    }
                    line
                }
            };
            refs.push(line.id.clone());
            new_lines.push(line);
        }
        b.lines = refs;
    }
    // The user's lines the new scene has no place for.
    for (i, o) in old_lines.iter().enumerate() {
        if used[i] || !line_kept(o) {
            continue;
        }
        let page = o.source.as_ref().map(|r| r.page);
        let target = scene.beats.iter().position(|b| page.is_some_and(|p| b.source.iter().any(|r| r.page == p))).or(scene.beats.len().checked_sub(1));
        match target {
            Some(t) => scene.beats[t].lines.push(o.id.clone()),
            None => scene.beats.push(Beat { id: ids.beat(), purpose: String::new(), source: Vec::new(), lines: vec![o.id.clone()], hook: false }),
        }
        new_lines.push(o.clone());
    }
    let mut cur = serde_json::to_value(current).unwrap_or(Value::Null);
    let ai = serde_json::to_value(&scene).unwrap_or(Value::Null);
    merge_unlocked(&mut cur, &ai, &current.locked);
    let mut merged: Scene = serde_json::from_value(cur).unwrap_or(scene);
    if merged.beats.iter().all(|b| b.lines.is_empty()) && !new_lines.is_empty() {
        // A lock on the beats kept the old ones: the lines still need a home.
        let page_of = |l: &Line| l.source.as_ref().map(|r| r.page);
        for l in &new_lines {
            let t = merged.beats.iter().position(|b| page_of(l).is_some_and(|p| b.source.iter().any(|r| r.page == p))).or(merged.beats.len().checked_sub(1));
            match t {
                Some(t) => merged.beats[t].lines.push(l.id.clone()),
                None => merged.beats.push(Beat { id: ids.beat(), lines: vec![l.id.clone()], ..Beat::default() }),
            }
        }
    }
    let mut out = old.clone();
    out.scenes[at] = merged;
    let pool: Vec<Line> = old.lines.iter().filter(|l| l.scene != scene_id && !old_lines.iter().any(|o| o.id == l.id)).cloned().chain(new_lines).collect();
    out.lines = order_lines(&out.scenes, pool);
    if !fresh.model.is_empty() {
        out.model = fresh.model;
    }
    out.made_at = out.made_at.max(fresh.made_at);
    out
}

// ---------------------------------------------------------------- the job

/// The context of the scene before some pages: its title, summary and last three lines.
fn context_of(scene: &Scene, lines: &[Line], cast: &Cast) -> String {
    let mut out = format!("Title: {}\nSummary: {}\n", scene.title, scene.summary);
    let ordered = order_lines(std::slice::from_ref(scene), scene_lines(scene, lines).into_iter().cloned().collect());
    let said: Vec<&Line> = ordered.iter().filter(|l| l.kind != LineKind::Sfx).collect();
    let last = &said[said.len().saturating_sub(3)..];
    if !last.is_empty() {
        out.push_str("Last lines:\n");
        for l in last {
            let who = l.speaker.as_deref().map(|s| cast.name_of(s)).unwrap_or_else(|| "Narrator".into());
            let _ = writeln!(out, "  {who}: \"{}\"", flat(&l.spoken));
        }
    }
    out
}

/// One call's user message.
struct Part<'a> {
    series: &'a str,
    episode: &'a str,
    chapter: &'a str,
    index: usize,
    chapters: usize,
    part: usize,
    parts: usize,
    budget_s: f32,
    cast: &'a Cast,
    places: &'a [Place],
    context: Option<&'a str>,
    digest: &'a str,
    one_scene: bool,
    /// The answer for these pages stopped early: only its missing last pages are sent again.
    continuation: bool,
}

fn user_message(p: &Part<'_>) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "SERIES: {}", flat(p.series));
    if !p.episode.trim().is_empty() {
        let _ = writeln!(out, "EPISODE: {}", flat(p.episode));
    }
    let _ = write!(out, "CHAPTER: {} (chapter {} of {} in this episode)", flat(p.chapter), p.index + 1, p.chapters);
    if p.parts > 1 {
        let _ = write!(out, "; part {} of {}: the chapter is long, so it comes in parts. Adapt only the pages below; the other parts are adapted separately", p.part + 1, p.parts);
    }
    out.push('\n');
    let secs = p.budget_s.max(SCENE_MIN_S).round();
    let _ = writeln!(out, "LENGTH BUDGET: about {secs} seconds ({:.1} minutes) of screen time for the pages below. Your scenes' \"target_s\" add up to about this.", secs / 60.);
    out.push('\n');
    out.push_str(&p.cast.block());
    let places = places_block(p.places);
    if !places.is_empty() {
        out.push('\n');
        out.push_str(&places);
    }
    if let Some(c) = p.context.filter(|c| !c.trim().is_empty()) {
        out.push_str("\nTHE SCENE JUST BEFORE (continuity only; do not adapt it again):\n");
        out.push_str(c);
    }
    out.push_str("\nMANGA DIGEST:\n");
    out.push_str(p.digest);
    if !p.digest.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');
    if p.one_scene {
        out.push_str("Adapt these pages as exactly ONE scene (one item in \"scenes\"); the scenes around it stay as they are. Reply with the JSON object only.\n");
    } else if p.continuation {
        out.push_str("Your answer for this part stopped before its last pages. The pages before these are already adapted (the scene just before is above). Adapt only the pages of this digest, continuing from there. Reply with the JSON object only.\n");
    } else {
        out.push_str("Adapt every page of the digest. Reply with the JSON object only.\n");
    }
    out
}

/// Saves a reply the step could not use to `work/ai/<name>.txt`; the project-relative path.
fn save_raw(store: &Store, name: &str, raw: &str) -> Option<String> {
    let dir = store.work_dir().join("ai");
    let file = format!("{}.txt", mv_core::paths::safe_name(name));
    let written = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(&file), raw));
    match written {
        Ok(()) => Some(format!("work/ai/{file}")),
        Err(e) => {
            warn(&format!("could not keep the AI's answer ({file}): {e}"));
            None
        }
    }
}

/// One adaptation call: the answer's JSON, or an error a user can act on (an unreadable answer
/// is kept in `work/ai/` and the message says where).
async fn ask(cx: &JobCtx<'_>, ai: &Ai, user: String, raw_name: &str) -> Result<Value> {
    let say = |s: String| (cx.progress)(f32::NAN, &s);
    match ai.ask_json(PROMPT, user, Vec::new(), AiEffort::High, &say, cx.cancel).await {
        Ok(v) => Ok(v),
        Err(e) => {
            let Some(bad) = e.downcast_ref::<BadReply>() else { return Err(e) };
            match save_raw(cx.store, raw_name, &bad.raw) {
                Some(rel) => Err(anyhow!("{e} Its answer is kept in {rel}.")),
                None => Err(e),
            }
        }
    }
}

/// Reads an answer; one without scenes is kept in `work/ai/` like an unreadable one.
fn read_answer(cx: &JobCtx<'_>, v: &Value, map: &MapCtx<'_>, ids: &mut Ids, raw_name: &str) -> Result<Adapted> {
    map_reply(v, map, ids).map_err(|e| {
        let raw = serde_json::to_string_pretty(v).unwrap_or_default();
        match save_raw(cx.store, raw_name, &raw) {
            Some(rel) => anyhow!("{e} Its answer is kept in {rel}."),
            None => e,
        }
    })
}

fn chapter_label(pick: &ChapterPick) -> String {
    let l = flat(&pick.label);
    if !l.is_empty() {
        return l;
    }
    match pick.number {
        Some(n) if n.fract() == 0. => format!("Chapter {n:.0}"),
        Some(n) => format!("Chapter {n}"),
        None => format!("Chapter {}", pick.chapter_id),
    }
}

fn chapter_failed(label: &str, e: anyhow::Error, earlier: usize) -> anyhow::Error {
    if llm::is_cancelled(&e) {
        return e;
    }
    let kept = match earlier {
        0 => "",
        1 => " The chapter before it was adapted and kept.",
        _ => " The chapters before it were adapted and kept.",
    };
    anyhow!("Could not adapt {label}: {e:#}{kept}")
}

/// Raises the project's id counters past `story` (a failure is only logged: ids are also
/// derived from the story itself).
fn save_counters(store: &Store, ids: &mut Ids, story: &Story) {
    ids.raise(story);
    // Read, raise and save under the one counters lock, inside `project.json`'s own lock, so
    // neither a PlanScene job's shot counter nor a screen's change of the project is lost.
    if let Err(e) = bump_ids(store, |c| ids.store_in(c)) {
        warn(&format!("story: the id counters were not saved: {e:#}"));
    }
}

/// What every step of the job reads once.
struct Inputs {
    title: String,
    cast: Cast,
    places: Vec<Place>,
}

fn inputs(cx: &JobCtx<'_>, manga_id: &str, title: &str) -> Inputs {
    let bible = cx.store.bible().unwrap_or_else(|e| {
        warn(&format!("story: the bible could not be read, so only the manga's cast is used: {e:#}"));
        BibleIndex::default()
    });
    let brain = source::brain(manga_id, title);
    Inputs { title: title.to_string(), cast: Cast::new(&bible, &brain), places: bible.places }
}

/// The Story job: adapts every chapter of `episode` and saves `story.json`. A failed chapter
/// leaves the earlier chapters' scenes in place and names itself in the error.
pub async fn run_job(cx: &JobCtx<'_>, episode: &str) -> Result<()> {
    let store = cx.store;
    let series = store.series()?;
    let ep = store.episode(episode)?;
    if ep.chapters.is_empty() {
        bail!("This episode has no chapters yet. Add chapters in the Manga step, then adapt the story.");
    }
    let ai = (cx.run.ai)(Tier::Planner).ok_or_else(|| anyhow!(NO_AI))?;
    let manga_id = if series.manga_id.trim().is_empty() { ep.chapters[0].manga_id.clone() } else { series.manga_id.clone() };
    let title = if series.manga_title.trim().is_empty() { manga_id.clone() } else { series.manga_title.clone() };
    let input = inputs(cx, &manga_id, &title);
    (cx.progress)(0., "Reading the chapters…");
    let reads: Vec<Option<ChapterRead>> = ep.chapters.iter().map(|p| source::read(p, &input.title).filter(|r| !r.pages.is_empty())).collect();
    let budget = budgets(&ep, &reads);
    let order: Vec<String> = ep.chapters.iter().map(|c| c.chapter_id.clone()).collect();
    let mut ids = Ids::new(&series.next_ids, &store.story(episode)?);
    let n = ep.chapters.len();
    let mut context: Option<String> = None;
    let mut run_issues: Vec<Issue> = Vec::new();
    for (i, pick) in ep.chapters.iter().enumerate() {
        let label = chapter_label(pick);
        let Some(read) = reads[i].as_ref() else {
            let e = anyhow!("it has not been read yet. Read it in the Manga step, then adapt again.");
            return Err(chapter_failed(&label, e, i));
        };
        let chunks = chunk_pages(read, DIGEST_CAP);
        let map = MapCtx { chapter_id: &pick.chapter_id, read, cast: &input.cast, places: &input.places };
        let mut adapted = Adapted::default();
        for (k, pages) in chunks.iter().enumerate() {
            let line = if chunks.len() > 1 { format!("Adapting {label}, part {} of {}…", k + 1, chunks.len()) } else { format!("Adapting {label}…") };
            (cx.progress)((i as f32 + k as f32 / chunks.len() as f32) / n as f32, &line);
            let digest = digest_of(read, pages, DIGEST_CAP);
            let share = if chunks.len() > 1 { share_of(read, pages) } else { 1. };
            let user = user_message(&Part {
                series: &input.title,
                episode: &ep.title,
                chapter: &label,
                index: i,
                chapters: n,
                part: k,
                parts: chunks.len(),
                budget_s: budget[i] * share,
                cast: &input.cast,
                places: &input.places,
                context: context.as_deref(),
                digest: &digest,
                one_scene: false,
                continuation: false,
            });
            let raw_name = format!("story-{}-{}-{}", cx.job.id, pick.chapter_id, k + 1);
            let mut part = async {
                let v = ask(cx, &ai, user, &raw_name).await?;
                read_answer(cx, &v, &map, &mut ids, &raw_name)
            }
            .await
            .map_err(|e| chapter_failed(&label, e, i))?;
            // A cut-off answer: ask once more for just the pages it did not reach.
            let gap = uncovered_tail(&part, &pick.chapter_id, pages, read);
            if !gap.is_empty() {
                (cx.progress)(
                    (i as f32 + k as f32 / chunks.len() as f32) / n as f32,
                    &format!("The AI's answer for {label} stopped early; asking again for {}…", pages_words(&gap)),
                );
                let before = part.scenes.last().map(|s| context_of(s, &part.lines, &input.cast)).or_else(|| context.clone());
                let digest = digest_of(read, &gap, DIGEST_CAP);
                let user = user_message(&Part {
                    series: &input.title,
                    episode: &ep.title,
                    chapter: &label,
                    index: i,
                    chapters: n,
                    part: k,
                    parts: chunks.len(),
                    budget_s: budget[i] * share_of(read, &gap),
                    cast: &input.cast,
                    places: &input.places,
                    context: before.as_deref(),
                    digest: &digest,
                    one_scene: false,
                    continuation: true,
                });
                let raw_name = format!("{raw_name}-rest");
                let more = async {
                    let v = ask(cx, &ai, user, &raw_name).await?;
                    read_answer(cx, &v, &map, &mut ids, &raw_name)
                }
                .await;
                match more {
                    Ok(more) => {
                        part.scenes.extend(more.scenes);
                        part.lines.extend(more.lines);
                        part.issues.extend(more.issues);
                    }
                    Err(e) if llm::is_cancelled(&e) => return Err(e),
                    Err(e) => warn(&format!("story: asking again for the last pages of {label} failed: {e:#}")),
                }
                let still = uncovered_tail(&part, &pick.chapter_id, &gap, read);
                if let Some(last) = part.scenes.last().filter(|_| !still.is_empty()) {
                    part.issues.push(issue(
                        PAGES_LEFT_OUT,
                        Severity::Warning,
                        &last.id,
                        format!("The AI's answer stopped early, so {} of {label} {} not adapted.", pages_words(&still), if still.len() == 1 { "was" } else { "were" }),
                        Some("Adapt the story again, or pick a model that can write longer answers."),
                    ));
                }
            }
            if let Some(last) = part.scenes.last() {
                context = Some(context_of(last, &part.lines, &input.cast));
            }
            adapted.scenes.extend(part.scenes);
            adapted.lines.extend(part.lines);
            adapted.issues.extend(part.issues);
        }
        // Merge with what is on disk now (the user may have edited meanwhile) and save. A scene
        // whose planned shots hold the user's work (edits, locks, pictures) is kept too. The
        // shot files are read first; the merge itself runs inside the story file's own lock,
        // so a voice batch or a Sound edit saving the story at the same moment is kept.
        let worked = worked_scenes(store, episode);
        run_issues.extend(std::mem::take(&mut adapted.issues));
        let (cast, model, chapter_id, budget_i) = (&input.cast, ai.model.model.clone(), pick.chapter_id.as_str(), budget[i]);
        let merged = store
            .update_story(episode, |current| {
                let kept_s: f32 = current
                    .scenes
                    .iter()
                    .filter(|s| home(s).as_deref() == Some(chapter_id) && (is_kept(s, &current.lines) || worked.contains(&s.id)))
                    .map(|s| s.target_s.max(0.))
                    .sum();
                fit_budget(&mut adapted.scenes, &adapted.lines, (budget_i - kept_s).max(1.));
                let new = Story { episode: episode.to_string(), scenes: adapted.scenes, lines: adapted.lines, model, made_at: store::now_s(), ..Story::default() };
                let old = issues(current);
                let mut merged = merge_in(current, new, &order, &worked);
                merged.episode = episode.to_string();
                raise_to_floor(&mut merged);
                refresh_issues(&mut merged, cast, &run_issues, &old);
                *current = merged;
            })
            .map_err(|e| chapter_failed(&label, e, i))?;
        save_counters(store, &mut ids, &merged);
    }
    (cx.progress)(1., "Story adapted");
    Ok(())
}

/// "Adapt this scene again": re-adapts the manga pages of one scene and puts the answer in its
/// place ([`replace_scene`]): the scene keeps its id, every field the user locked and every line
/// the user edited. Queued as `JobKind::AdaptScene`.
pub async fn adapt_scene(cx: &JobCtx<'_>, episode: &str, scene_id: &str) -> Result<()> {
    let store = cx.store;
    let series = store.series()?;
    let ep = store.episode(episode)?;
    let story = store.story(episode)?;
    let at = story.scenes.iter().position(|s| s.id == scene_id).ok_or_else(|| anyhow!("That scene is no longer in the story. Reload the Story step and try again."))?;
    let scene = &story.scenes[at];
    let chapter = home(scene).ok_or_else(|| anyhow!("This scene has no manga pages to adapt from (it was written by hand)."))?;
    let pick = ep.chapters.iter().find(|c| c.chapter_id == chapter).ok_or_else(|| anyhow!("The chapter of this scene is no longer part of the episode."))?;
    let ai = (cx.run.ai)(Tier::Planner).ok_or_else(|| anyhow!(NO_AI))?;
    let manga_id = if series.manga_id.trim().is_empty() { pick.manga_id.clone() } else { series.manga_id.clone() };
    let title = if series.manga_title.trim().is_empty() { manga_id.clone() } else { series.manga_title.clone() };
    let input = inputs(cx, &manga_id, &title);
    let label = chapter_label(pick);
    let read = source::read(pick, &input.title)
        .filter(|r| !r.pages.is_empty())
        .ok_or_else(|| anyhow!("{label} has not been read yet. Read it in the Manga step, then adapt the scene again."))?;
    let mut pages: Vec<usize> = scene
        .source
        .iter()
        .chain(scene.beats.iter().flat_map(|b| b.source.iter()))
        .chain(scene_lines(scene, &story.lines).into_iter().filter_map(|l| l.source.as_ref()))
        .filter(|r| r.chapter_id == chapter)
        .map(|r| r.page)
        .collect();
    pages.sort_unstable();
    pages.dedup();
    let digest = digest_of(&read, &pages, DIGEST_CAP);
    let context = at.checked_sub(1).map(|j| context_of(&story.scenes[j], &story.lines, &input.cast));
    let index = ep.chapters.iter().position(|c| c.chapter_id == chapter).unwrap_or(0);
    let user = user_message(&Part {
        series: &input.title,
        episode: &ep.title,
        chapter: &label,
        index,
        chapters: ep.chapters.len(),
        part: 0,
        parts: 1,
        budget_s: scene.target_s,
        cast: &input.cast,
        places: &input.places,
        context: context.as_deref(),
        digest: &digest,
        one_scene: true,
        continuation: false,
    });
    (cx.progress)(0.1, &format!("Adapting \u{201c}{}\u{201d} again…", scene.title));
    let raw_name = format!("story-{}-{}", cx.job.id, scene_id);
    let map = MapCtx { chapter_id: &chapter, read: &read, cast: &input.cast, places: &input.places };
    let mut ids = Ids::new(&series.next_ids, &story);
    let fresh = async {
        let v = ask(cx, &ai, user, &raw_name).await?;
        read_answer(cx, &v, &map, &mut ids, &raw_name)
    }
    .await
    .map_err(|e| if llm::is_cancelled(&e) { e } else { anyhow!("Could not adapt the scene again: {e:#}") })?;
    let mut scenes: Vec<Scene> = fresh.scenes.into_iter().take(1).collect();
    let first = scenes.first().map(|s| s.id.clone()).unwrap_or_default();
    let lines: Vec<Line> = fresh.lines.into_iter().filter(|l| l.scene == first).collect();
    fit_budget(&mut scenes, &lines, scene.target_s);
    let fresh_story = Story { scenes, lines, model: ai.model.model.clone(), made_at: store::now_s(), ..Story::default() };
    // The answer's notes name the scene by the id it was read under; the scene keeps its own.
    let notes: Vec<Issue> = fresh
        .issues
        .into_iter()
        .map(|mut i| {
            i.scene = scene_id.to_string();
            i
        })
        .collect();
    // Put in place inside the story file's own lock, on the story as it is on disk now, so a
    // voice batch or a Sound edit saved meanwhile is kept.
    let cast = &input.cast;
    let merged = store.update_story(episode, |current| {
        let old = issues(current);
        let mut merged = replace_scene(current, scene_id, fresh_story);
        // The kept lines of the user may need more time than the new scene was fitted to.
        raise_to_floor(&mut merged);
        refresh_issues(&mut merged, cast, &notes, &old);
        if merged.episode.is_empty() {
            merged.episode = episode.to_string();
        }
        *current = merged;
    })?;
    save_counters(store, &mut ids, &merged);
    (cx.progress)(1., "Scene adapted again");
    Ok(())
}
