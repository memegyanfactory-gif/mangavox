//! The character intro: a short cinematic opening just before chapter 1 of a full recap (after the
//! hook of a one-video export, and first when Play plays the whole recap). The manga's main
//! characters the way a game's character select screen shows its cast: each drawn once for this
//! manga by Studio's image service on a key colour and cut out (without an image key, cut out of
//! their best panel), standing on a tilted card with their name and a short profile (a role, a
//! one-line bio, three traits and a fact or two, written by Studio's AI from what it learned
//! reading the manga, see `mv_recap::cast_profile`), in front of a background made for them from
//! the pages they appear in, over music in the manga's own style; then a Netflix-style ribbon
//! transition grows out of the last card and opens onto the recap's first frame. It shows no logo
//! and never the manga's cover.
//!
//! Each manga's intro is a pipeline of its own, every step shown on the job's activity line as it
//! happens ("Intro · Painting Mu Gung's background with Magnific (2 of 4)"): this side reads the
//! cast and the chapters, has the cast's profiles written (or reuses the kept ones) and writes the
//! music, then runs `python/intro_maker.py`, whose own steps (each character drawn and keyed, each
//! background painted, the cards, the transition) come back through a progress file. Every file it
//! uses or keeps is in that manga's own folders, what the renderer paid for goes into Studio's
//! credits meter, and the intro is kept and made again only when what it shows changes.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, bail, Context, Result};
use mv_recap::cast_profile::{brain_profile, make_profiles, CastStory, Profile, ProfileStep, Subject};
use serde_json::{json, Value};

use super::{probe, wait_for, Joined, Reporter, CANCELLED};
use crate::ytpack::magnific::{self, ImageModel};
use gpui_kit::BackgroundExecutor;

/// Characters introduced (the most seen first).
pub(crate) const MAX_CARDS: usize = 4;
/// Characters sent to the renderer, the most seen first (it shows the first [`MAX_CARDS`] it can),
/// each with their profile.
const MAX_CAST: usize = MAX_CARDS * 2;
/// Each card's time on screen: long enough to read its profile. Must match intro_maker.py.
const CARD_SEC: f64 = 2.5;
/// The ribbon transition after the last card, the moment in it when the camera passes through
/// the ribbons, and the recap's first frame settling in. Must match intro_transition.py.
const RIBBON_SEC: f64 = 1.2;
const PASS_SEC: f64 = 0.768;
const REVEAL_SEC: f64 = 1.4;
/// The longest intro: every card, then the transition.
const INTRO_SEC: f64 = MAX_CARDS as f64 * CARD_SEC + RIBBON_SEC + REVEAL_SEC;
/// The music's big moment: the camera passing through the ribbons after the last card.
const MUSIC_HIT: f64 = MAX_CARDS as f64 * CARD_SEC + PASS_SEC;
/// The music lasts the whole intro and rings on this much longer.
const MUSIC_TAIL: f64 = 0.5;
/// Bumped when the intro's makeup changes, so a kept one is made again.
const INTRO_VERSION: u64 = 8;
/// The renderer's share of the intro's bar (the steps before it are quick).
const RENDER_FROM: f32 = 0.05;
const RENDER_TO: f32 = 0.99;
/// How much older than its run a progress file may look (file times are coarse) before it counts
/// as left over from an earlier run.
const STALE_SLACK: Duration = Duration::from_secs(2);
/// The longest step shown (the activity line cuts the rest anyway).
const MAX_STEP: usize = 90;

/// What making an intro needs, gathered on the UI thread.
pub(crate) struct IntroJob {
    pub manga_id: String,
    pub manga_title: String,
    /// The made chapters, in order.
    pub chapters: Vec<crate::data::ChapterItem>,
    /// Every chapter's number by id, all the manga's chapters (see [`chapter_numbers`]): where each
    /// character is first seen, for their profile.
    pub numbers: HashMap<String, f64>,
    /// The model that writes the cast's profiles (none: they come from the character notes), its
    /// name for the activity line, and the AI registry it is reached through.
    pub model: Option<mv_recap::ModelChoice>,
    pub model_name: String,
    pub reg: Arc<mv_ai::Registry>,
    pub python: mv_studio::PythonInterpreter,
    pub layout: mv_studio::PythonLayout,
}

/// Every chapter's number by id (chapters without a number left out), for [`IntroJob::numbers`].
pub(crate) fn chapter_numbers(chapters: &[crate::data::ChapterItem]) -> HashMap<String, f64> {
    chapters.iter().filter_map(|c| Some((c.id.clone(), c.number?))).collect()
}

/// The model `(choice, display name)` that writes the cast's profiles: `None` when none is picked
/// (the picker leaves the model empty then).
pub(crate) fn profile_model(model: Option<(mv_recap::ModelChoice, String)>) -> Option<(mv_recap::ModelChoice, String)> {
    model.filter(|(m, _)| !m.model.trim().is_empty())
}

/// A made (or kept) intro, and what the result should say about it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MadeIntro {
    pub path: PathBuf,
    /// The renderer's notes (a character shown from their panel, …).
    pub notes: Vec<String>,
}

impl MadeIntro {
    /// What a result message adds (" · intro: …"): nothing when everything went as planned.
    pub(crate) fn note(&self) -> String {
        if self.notes.is_empty() {
            String::new()
        } else {
            format!(" · intro: {}", self.notes.join("; "))
        }
    }
}

/// One step of the intro on the job's activity line, "Intro · <step>", at `p` of the intro's bar.
fn step(r: &Reporter, p: f32, what: &str) {
    r.set(p, &format!("Intro · {what}"));
}

/// The cast the intro may show: the most seen characters with a name (names, aliases and
/// descriptions trimmed, only the face pictures that exist), at most [`MAX_CAST`] (the renderer
/// skips those it cannot show).
pub(crate) fn cast(brain: &mv_recap::Brain) -> Vec<mv_recap::Character> {
    let mut cast: Vec<&mv_recap::Character> = brain.characters.iter().filter(|c| !c.name.trim().is_empty()).collect();
    cast.sort_by(|a, b| b.appearances.cmp(&a.appearances));
    cast.into_iter()
        .take(MAX_CAST)
        .map(|c| {
            let mut c = c.clone();
            c.name = c.name.trim().to_string();
            c.aliases = c.aliases.iter().map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect();
            c.description = c.description.trim().to_string();
            c.faces.retain(|f| f.is_file());
            if c.id.trim().is_empty() {
                c.id = id_from_name(&c.name);
            }
            c
        })
        .collect()
}

/// A file-safe id for a character saved without one (an old brain), the same on every run, so its
/// kept drawing (`cast/<id>.png`) and background (`cast/<id>.bg.png`) are its own.
fn id_from_name(name: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    let slug: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let slug = slug.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    format!("{}-{:04x}", if slug.is_empty() { "character" } else { &slug }, h.finish() & 0xffff)
}

/// The profile a character's card shows (`profiles` by id): `null` without one.
fn profile_json(profiles: &HashMap<String, Profile>, id: &str) -> Value {
    profiles.get(id).filter(|p| !p.is_empty()).map_or(Value::Null, |p| json!(p))
}

/// The renderer's `cast`: each character's name, aliases, id (in the intro folder its kept drawing
/// is `cast/<id>.png` and its background `cast/<id>.bg.png`), description, face pictures (the
/// drawing's references) and profile (`{role, bio, traits, facts}` or `null`, from `profiles` by
/// id).
fn cast_json(cast: &[mv_recap::Character], profiles: &HashMap<String, Profile>) -> Value {
    // The traits too: a card without a profile shows them as its chips.
    Value::Array(cast.iter().map(|c| json!({"name": c.name, "aliases": c.aliases, "id": c.id, "description": c.description, "traits": c.traits, "faces": c.faces, "profile": profile_json(profiles, &c.id)})).collect())
}

/// What of the cast a kept intro depends on: the names and profiles the cards show, and the ids
/// that find each character's drawing and background (a new one changes the key through its
/// stamp). A new or edited profile makes the intro again.
fn cast_key(cast: &[mv_recap::Character], profiles: &HashMap<String, Profile>) -> Value {
    Value::Array(cast.iter().map(|c| json!([c.id, c.name, c.aliases, profile_json(profiles, &c.id)])).collect())
}

/// The manga's music style: the one its chapters' scores spend the most time in (Epic without any).
pub(crate) fn manga_style(chapters: &[crate::data::ChapterItem]) -> mv_score::Style {
    let mut time: std::collections::HashMap<mv_score::Style, f64> = Default::default();
    for ch in chapters {
        if let Some(score) = mv_score::load_score(&crate::page::chref(ch)) {
            for c in &score.cues {
                *time.entry(c.style).or_default() += (c.end - c.start).max(0.);
            }
        }
    }
    time.into_iter().max_by(|a, b| a.1.total_cmp(&b.1)).map(|(s, _)| s).unwrap_or(mv_score::Style::Epic)
}

/// The intro's music in `style`, at least `secs` long, its big moment at `hit`: the score
/// engine's own build-and-impact cue, rendered so its impact lands there.
fn intro_music(style: mv_score::Style, hit: f64, secs: f64, out: &Path) -> Result<()> {
    use mv_score::demo::{demo_score, render_demo};
    let at = demo_score(style, 10., 0.7, mv_score::SfxAmount::Off).cues.get(1).map(|c| c.start / 10.).filter(|f| *f > 0.1).unwrap_or(0.6);
    let len = (hit / at).max(secs);
    let (_, bus) = render_demo(style, len, 0.75, mv_score::SfxAmount::Off, &mv_score::orchestra::load(), -16.);
    mv_score::mix::write_wav(out, &bus)
}

/// Where the last intro is kept (made again only when what it shows changes), in the manga's
/// folder next to its joined video; its work folder is beside it.
fn kept_intro_path(manga_id: &str) -> PathBuf {
    super::joined_path(manga_id).with_file_name("full_recap_intro.mp4")
}

/// The manga's intro folder in its Studio folder (`studio/<id>/intro`, made when something is
/// written): the renderer keeps each character's drawing there (`cast/<id>.png`) and the
/// background made for them (`cast/<id>.bg.png`), so neither is paid for twice, and each
/// character's profile is kept beside them (`cast/<id>.info.json`, see `mv_recap::cast_profile`).
fn intro_dir(manga_id: &str) -> PathBuf {
    mv_recap::Brain::dir(manga_id).join("intro")
}

/// A file's (size, modified seconds), for telling when it changed.
fn stamp(p: &Path) -> Value {
    p.metadata().ok().map(|m| json!([m.len(), m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs())])).unwrap_or(Value::Null)
}

// ------------------------------------------------------------------ the renderer's progress

/// One reading of the renderer's progress file.
#[derive(Debug, Clone, PartialEq)]
struct Progress {
    /// How far the intro is, 0..1 (missing: where it was).
    p: Option<f32>,
    /// What it is doing ("Drawing Mu Gung with Magnific (2 of 4)").
    step: String,
}

/// The renderer's progress file at `path` (`{p, step, stage}`, replaced as it goes) when it is
/// whole and was written during this run (from `since` on): missing, half written or left over
/// from an earlier run reads as `None`, never as an error.
fn read_progress(path: &Path, since: SystemTime) -> Option<Progress> {
    let modified = path.metadata().and_then(|m| m.modified()).ok()?;
    if modified + STALE_SLACK < since {
        return None;
    }
    let v: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let step = v["step"].as_str().map(|s| one_line(s, MAX_STEP)).filter(|s| !s.is_empty())?;
    let p = v["p"].as_f64().filter(|p| p.is_finite()).map(|p| p.clamp(0., 1.) as f32);
    Some(Progress { p, step })
}

/// `s` on one line, at most `max` characters (then "…").
fn one_line(s: &str, max: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s,
    }
}

/// Where the renderer's progress `p` (0..1) sits on the intro's bar.
fn render_bar(p: f32) -> f32 {
    RENDER_FROM + (RENDER_TO - RENDER_FROM) * p.clamp(0., 1.)
}

// ------------------------------------------------------------------ the renderer's answer

/// The renderer's answer, its last JSON line: `{ok: true, path, seconds, credits, notes}` or
/// `{ok: false, message, credits, notes}`; `Null` when it printed none (it crashed).
fn answer(stdout: &str) -> Value {
    let last = stdout.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
    serde_json::from_str(last).unwrap_or(Value::Null)
}

/// Why the renderer made no intro (`None` when it made one): its message, else stderr's last line,
/// else its exit code.
fn failure(v: &Value, code: Option<i32>, stderr: &str) -> Option<String> {
    if v["ok"].as_bool() == Some(true) {
        return None;
    }
    let message = v["message"].as_str().map(str::trim).filter(|m| !m.is_empty()).map(str::to_string);
    Some(message.or_else(|| stderr.lines().rev().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)).unwrap_or_else(|| format!("intro_maker exited {code:?}")))
}

/// The images the renderer paid for (its `credits`: `[{model, credits}]`), each with its catalog
/// model; the ids Studio does not know come back apart (logged, not counted).
fn credit_entries(v: &Value) -> (Vec<(&'static ImageModel, f64)>, Vec<String>) {
    let (mut known, mut unknown) = (vec![], vec![]);
    for e in v["credits"].as_array().into_iter().flatten() {
        let id = e["model"].as_str().unwrap_or("").trim();
        let credits = e["credits"].as_f64().filter(|c| c.is_finite() && *c >= 0.).unwrap_or(0.);
        match magnific::CATALOG.iter().find(|m| m.id == id) {
            Some(m) => known.push((m, credits)),
            None => unknown.push(if id.is_empty() { e.to_string() } else { id.to_string() }),
        }
    }
    (known, unknown)
}

/// Counts what the renderer paid for in Studio's credits meter (Magnific's images; other services
/// bill their own accounts). Blocking.
fn record_credits(v: &Value) {
    let (known, unknown) = credit_entries(v);
    for (m, credits) in known {
        magnific::record(m, credits);
    }
    for id in unknown {
        eprintln!("[studio] intro: the renderer paid for an image of an unknown model ({id}); not counted in the credits");
    }
}

/// The renderer's notes, each on one line and said once.
fn notes_of(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for n in v["notes"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let n = one_line(n, 240);
        if !n.is_empty() && !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

// ------------------------------------------------------------------ what a kept intro was made from

/// The kept cast pictures in `dir` (`<intro folder>/cast`): each character's drawing (`<id>.png`)
/// and background (`<id>.bg.png`), (file name, stamp) by name: a new or redrawn character, or a
/// background painted again, makes the intro again.
fn picture_stamps(dir: &Path) -> Value {
    let mut found: Vec<(String, Value)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("png")))
        .map(|p| (p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), stamp(&p)))
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    json!(found)
}

/// The intro's key: what it shows (`base`) and the cast pictures in `cast_dir` as they are now.
fn intro_key(base: &Value, cast_dir: &Path) -> Value {
    let mut key = base.clone();
    key["pictures"] = picture_stamps(cast_dir);
    key
}

/// Writes what the intro at `out` was made from: its key, the renderer's notes and the joined
/// video's sources (the export preview plays it for those chapters).
fn save_record(out: &Path, key: &Value, notes: &[String], sources: &Value) {
    let _ = std::fs::write(out.with_extension("json"), json!({"key": key, "notes": notes, "sources": sources}).to_string());
}

/// The kept intro's record when the intro is there and was made from `key`.
fn kept_if_current(out: &Path, key: &Value) -> Option<Value> {
    let rec: Value = serde_json::from_slice(&std::fs::read(out.with_extension("json")).ok()?).ok()?;
    (out.is_file() && rec["key"] == *key).then_some(rec)
}

/// The intro kept at `out` when it was made for the joined video at `joined` as it is now.
fn kept_intro_at(out: &Path, joined: &Path) -> Option<PathBuf> {
    let rec: Value = serde_json::from_slice(&std::fs::read(out.with_extension("json")).ok()?).ok()?;
    let sources = super::joined_sources(joined);
    (out.is_file() && !sources.is_null() && rec["sources"] == sources).then(|| out.to_path_buf())
}

/// The manga's kept intro when it was made for the joined video at `joined` as it is now (the same
/// chapter videos): the export preview plays it before the recap (an export makes it again first
/// when anything else it shows has changed). Blocking.
pub(crate) fn kept_intro(manga_id: &str, joined: &Path) -> Option<PathBuf> {
    kept_intro_at(&kept_intro_path(manga_id), joined)
}

// ------------------------------------------------------------------ making it

/// The intro's work folder for one run (the renderer's arguments, the music, the progress file,
/// the renderer's frames): emptied when the run starts, removed in the background when it ends,
/// however it ends.
struct WorkDir(PathBuf);

impl WorkDir {
    async fn fresh(dir: PathBuf) -> Result<WorkDir> {
        let d = dir.clone();
        mv_ui::bg::blocking(move || {
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).with_context(|| format!("create {}", d.display()))
        })
        .await??;
        Ok(WorkDir(dir))
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let dir = self.0.clone();
        // Started on the background runtime at once; nothing waits for it.
        drop(mv_ui::bg::blocking(move || std::fs::remove_dir_all(dir)));
    }
}

/// Kills the renderer with everything it started (ffmpeg, the cut-out helper) when its run is
/// given up: `taskkill /T`, which finds the children through the renderer, so it runs while that
/// still lives.
struct KillTree(Option<u32>);

impl Drop for KillTree {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            let _ = mv_studio::python::quiet_command("taskkill").args(["/T", "/F", "/PID", &pid.to_string()]).status();
        }
    }
}

/// Runs `intro_maker.py` with `arg` to the end: (exit code, stdout, stderr). When the run is given
/// up (the job cancelled), the renderer and everything it started are killed.
async fn render(py: mv_studio::PythonInterpreter, script: PathBuf, arg: String, dir: PathBuf) -> Result<(Option<i32>, String, String)> {
    // On Windows not killed on drop (that would end it before `KillTree` finds its children);
    // elsewhere, without taskkill, it is.
    let child = mv_studio::bridge::build_command(&py, &script, &[arg], &dir).kill_on_drop(!cfg!(windows)).spawn().context("start Studio's Python for the intro")?;
    let mut tree = KillTree(child.id());
    if let Some(pid) = child.id() {
        mv_core::limits::adopt(pid);
    }
    let out = child.wait_with_output().await.context("run the intro renderer")?;
    tree.0 = None;
    Ok((out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned()))
}

/// The Python the intro renders with: it needs numpy and Pillow, which Studio's own Python (a
/// plain install: the video scripts don't need them) often lacks. The effects pack's Python has
/// them, then the voice environment may, then Studio's own. Blocking (it asks each one).
fn intro_python(studio: &mv_studio::PythonInterpreter, effects: Option<&Path>) -> Result<mv_studio::PythonInterpreter> {
    let plain = |p: &Path| mv_studio::PythonInterpreter { program: p.to_string_lossy().into_owned(), prefix_args: vec![], version: String::new() };
    let voice = mv_studio::python::venv_python(&mv_studio::python::mangavox_home());
    let mut candidates: Vec<mv_studio::PythonInterpreter> = effects.filter(|p| p.is_file()).map(plain).into_iter().collect();
    if voice.is_file() {
        candidates.push(plain(&voice));
    }
    candidates.push(studio.clone());
    candidates
        .into_iter()
        .find(|py| mv_studio::python::quiet_command(&py.program).args(&py.prefix_args).args(["-c", "import numpy, PIL"]).output().is_ok_and(|o| o.status.success()))
        .ok_or_else(|| anyhow!("the intro needs numpy and Pillow, and no Python Studio uses has them: install the effects pack in Studio's Setup"))
}

/// What a manga's intro is made of, read off the UI thread.
struct Gathered {
    /// The joined video's frame size and rate (the intro is made at them, to join it as it is).
    size: (u32, u32),
    fps: String,
    cast: Vec<mv_recap::Character>,
    /// The cast's profiles by id (made after gathering, see [`cast_profiles`]), and what the brain
    /// knows of the story they are written from: its chapter summaries and story notes.
    profiles: HashMap<String, Profile>,
    recaps: Vec<mv_recap::ChapterRecap>,
    notes: String,
    /// The read chapters, for the renderer (`[{read, pages}]`) and as files (whose stamps are in
    /// the intro's key).
    reads: Vec<Value>,
    read_files: Vec<PathBuf>,
    style: mv_score::Style,
    /// The recap's first chapter video (the transition opens onto its first frame).
    reveal: Option<PathBuf>,
    /// The layered-motion pack's Python (cut-outs), when it is installed.
    roto: Option<PathBuf>,
    magnific_key: bool,
}

/// Reads what the intro of manga `id` is made of: `chapters` (the ones in the joined video at
/// `joined`, whose first chapter video is `first`). Errs when there is nothing to show yet: no
/// named character, or none of the chapters read. Blocking.
fn gather(id: &str, title: &str, chapters: &[crate::data::ChapterItem], joined: &Path, first: Option<PathBuf>) -> Result<Gathered> {
    let brain = mv_recap::Brain::load(id, title);
    let cast = cast(&brain);
    if cast.is_empty() {
        bail!("no characters known yet: read the chapters first");
    }
    let (mut reads, mut read_files) = (vec![], vec![]);
    for c in chapters {
        let read = mv_recap::chapter_dir(&crate::page::chref(c)).join("read.json");
        if read.is_file() {
            reads.push(json!({"read": read, "pages": mv_recap::list_pages(&c.pages_dir)}));
            read_files.push(read);
        }
    }
    if reads.is_empty() {
        bail!("none of these chapters has been read yet");
    }
    let p = probe(joined)?;
    Ok(Gathered {
        size: p.size,
        fps: p.video.splitn(3, '/').nth(2).unwrap_or("30/1").to_string(),
        cast,
        profiles: HashMap::new(),
        recaps: brain.recaps,
        notes: brain.notes,
        reads,
        read_files,
        style: manga_style(chapters),
        reveal: first.filter(|f| f.is_file()),
        roto: mv_studio::vfx_pack::python(),
        magnific_key: magnific::key().is_some(),
    })
}

/// What the intro shows, but for its cast pictures (see [`intro_key`]): the frame, the music
/// style, the cast and their profiles, the reads, the recap's first video, whether characters can
/// be cut out and whether Magnific can draw them (a key added later makes the intro again).
/// Blocking (file stamps).
fn base_key(g: &Gathered) -> Value {
    json!({
        "v": INTRO_VERSION,
        "size": [g.size.0, g.size.1],
        "fps": g.fps,
        "style": g.style.key(),
        "cast": cast_key(&g.cast, &g.profiles),
        "reads": g.read_files.iter().map(|f| stamp(f)).collect::<Vec<_>>(),
        "reveal": [g.reveal, g.reveal.as_deref().map(stamp)],
        "roto": g.roto.is_some(),
        "magnific": g.magnific_key,
    })
}

/// The renderer's arguments (`intro_maker.py <args.json>`): the frame, the music style, the read
/// chapters, the cast with their profiles (drawn when it can be), Studio's folder (the image key),
/// the cut-out helper, the music, the recap's first video (the transition opens onto its first
/// frame), the manga's intro folder (`intro_dir`: the cast's kept drawings, backgrounds and
/// profiles), where to save the intro (`out`), its work folder and its progress file.
fn render_args(g: &Gathered, music: Option<&Path>, intro_dir: &Path, out: &Path, work_dir: &Path, progress: &Path) -> Value {
    json!({
        "width": g.size.0,
        "height": g.size.1,
        "fps": g.fps,
        "style": g.style.key(),
        "chapters": g.reads,
        "cast": cast_json(&g.cast, &g.profiles),
        "cast_draw": true,
        "studio_dir": mv_core::paths::studio(),
        "roto_python": g.roto,
        "roto_data": mv_core::paths::root(),
        "music": music,
        "reveal": g.reveal,
        "intro_dir": intro_dir,
        "out": out,
        "work_dir": work_dir,
        "progress_file": progress,
    })
}

/// What the activity line says of the cast's profiles ("Intro · <this>"), the model that writes
/// them named `model_name`.
fn profile_step(s: ProfileStep, model_name: &str) -> String {
    match s {
        ProfileStep::Kept => "Using the cast profiles written earlier".to_string(),
        ProfileStep::Writing(_) => format!("Writing the cast profiles with {model_name}"),
        ProfileStep::FromNotes => "No AI model set, so the profiles come from the character notes".to_string(),
    }
}

/// What the result says when the model could not write the cast's profiles (`why`).
fn profile_note(why: &str) -> String {
    format!("the cast profiles come from the character notes (the AI could not write them: {})", one_line(why, 160))
}

/// Has the profiles of `g`'s cast made (see `mv_recap::cast_profile`) and puts them in `g`: the
/// ones kept in `cast_dir` as they are, the rest written by `job`'s model in one call, else from
/// the character notes; the step on the activity line through `r`. Returns what the result says
/// when the model failed. Errs only when cancelled.
async fn cast_profiles(job: &IntroJob, g: &mut Gathered, cast_dir: &Path, r: &Reporter, ex: &BackgroundExecutor) -> Result<Option<String>> {
    let subject = |c: &mv_recap::Character| Subject { character: c.clone(), first_chapter: c.first_seen.as_ref().and_then(|(ch, _)| job.numbers.get(ch).copied()) };
    let story = CastStory { manga_title: job.manga_title.clone(), subjects: g.cast.iter().map(subject).collect(), recaps: std::mem::take(&mut g.recaps), notes: std::mem::take(&mut g.notes) };
    let model_name = match job.model_name.trim() {
        "" => job.model.as_ref().map(|m| m.model.clone()).unwrap_or_default(),
        name => name.to_string(),
    };
    let rr = r.clone();
    let on_step = move |s: ProfileStep| step(&rr, 0.01, &profile_step(s, &model_name));
    let ai = job.model.clone().map(|m| (job.reg.clone(), m));
    let token = mv_ai::CancellationToken::new();
    let task = mv_ui::bg::runtime().spawn(make_profiles(cast_dir.to_path_buf(), story, ai, token.clone(), on_step));
    let res = wait_for(task, r, ex, || {}).await;
    if r.cancelled() {
        token.cancel();
        bail!(CANCELLED);
    }
    // Making them never fails; should its task break, the character notes give them all.
    let (profiles, failed) = match res {
        Ok(made) => (made.by_id, made.failed),
        Err(e) => (g.cast.iter().map(|c| (c.id.clone(), brain_profile(&subject(c)))).collect(), Some(format!("{e:#}"))),
    };
    g.profiles = profiles;
    Ok(failed.map(|why| {
        step(r, 0.015, "The AI could not write the cast profiles, so they come from the character notes");
        profile_note(&why)
    }))
}

/// Makes (or reuses) the intro of `job` for the joined video `j`, at its size and frame rate,
/// opening onto the first chapter video's first frame: a pipeline of its own, each step shown
/// through `r` on the intro's own bar. Takes the export turn first (it waits for another export
/// to finish). Errs when there is nothing to show yet (no cast, no read chapter), when the
/// renderer fails, or when cancelled.
pub(crate) async fn make_intro(mut job: IntroJob, j: &Joined, r: &Reporter, ex: &BackgroundExecutor) -> Result<MadeIntro> {
    let rr = r.clone();
    mv_ui::bg::blocking(move || rr.take_turn()).await??;
    r.set(0., &format!("Intro for {} · Reading its cast and chapters", job.manga_title));
    // Only the chapters in the video (unrendered ones may have been left out).
    job.chapters.retain(|c| j.files.contains(&c.output_dir.join(crate::data::VIDEO_FILE)));
    let (id, title) = (job.manga_id.clone(), job.manga_title.clone());
    let out = kept_intro_path(&id);
    let work = WorkDir::fresh(out.with_extension("work")).await?;
    // The manga's intro folder, where the renderer keeps the cast's drawings and backgrounds and
    // the cast's profiles are kept.
    let folder = intro_dir(&id);
    let cast_dir = folder.join("cast");
    // Nothing is paid for before it is clear there is something to show.
    let mut g = {
        let (id, title, chapters, joined, first) = (id.clone(), title.clone(), job.chapters.clone(), j.path.clone(), j.files.first().cloned());
        mv_ui::bg::blocking(move || gather(&id, &title, &chapters, &joined, first)).await??
    };
    if r.cancelled() {
        bail!(CANCELLED);
    }
    // The cards' profiles (in the intro's key: a new or edited one makes it again).
    let profile_note = cast_profiles(&job, &mut g, &cast_dir, r, ex).await?;
    step(r, 0.02, "Checking for an intro made earlier");
    let (g, base, kept) = {
        let (out, cast_dir) = (out.clone(), cast_dir.clone());
        mv_ui::bg::blocking(move || {
            let base = base_key(&g);
            let kept = kept_if_current(&out, &intro_key(&base, &cast_dir));
            (g, base, kept)
        })
        .await?
    };
    if let Some(rec) = kept {
        r.set(1., "Intro · Ready (made earlier, nothing changed)");
        let mut notes = notes_of(&rec);
        notes.extend(profile_note);
        return Ok(MadeIntro { path: out, notes });
    }
    step(r, 0.03, "Writing its music");
    let music = {
        let (style, file) = (g.style, work.0.join("music.wav"));
        // The cards' music for the whole intro, its big moment as the camera passes through the
        // transition's ribbons (without it the cards play on their whooshes alone).
        mv_ui::bg::blocking(move || intro_music(style, MUSIC_HIT, INTRO_SEC + MUSIC_TAIL, &file).ok().map(|_| file)).await?
    };
    if r.cancelled() {
        bail!(CANCELLED);
    }
    let progress = work.0.join("progress.json");
    let made = out.with_extension("new.mp4");
    let spec = render_args(&g, music.as_deref(), &folder, &made, &work.0.join("render"), &progress);
    let args_file = work.0.join("args.json");
    std::fs::write(&args_file, serde_json::to_string(&spec)?).with_context(|| format!("write {}", args_file.display()))?;
    let script = job.layout.script("intro_maker.py").map_err(|e| anyhow!("{e}"))?;
    step(r, RENDER_FROM, "Starting the renderer");
    let py = {
        let (studio, effects) = (job.python.clone(), g.roto.clone());
        mv_ui::bg::blocking(move || intro_python(&studio, effects.as_deref())).await??
    };
    let (dir, arg) = (job.layout.dir.clone(), args_file.to_string_lossy().into_owned());
    let since = SystemTime::now();
    // The intro renders under the heavy-work cap the export turn holds low: lifted meanwhile.
    mv_core::limits::throttle(None);
    let task = mv_ui::bg::runtime().spawn(render(py, script, arg, dir));
    // The renderer's own steps, as it writes them; its bar never goes back.
    let mut at = RENDER_FROM;
    let res = wait_for(task, r, ex, || {
        if let Some(pg) = read_progress(&progress, since) {
            at = pg.p.map_or(at, |p| at.max(render_bar(p)));
            step(r, at, &pg.step);
        }
    })
    .await;
    mv_core::limits::throttle(Some(mv_core::limits::MIN_PERCENT));
    let (code, stdout, stderr) = res??;
    // What the renderer paid for counts, whether it made the intro or not.
    let v = answer(&stdout);
    let notes = notes_of(&v);
    let paid = v.clone();
    mv_ui::bg::blocking(move || record_credits(&paid)).await?;
    if let Some(why) = failure(&v, code, &stderr) {
        bail!("{why}");
    }
    // Kept with what it was made from: the cast pictures as they are now (this run may have drawn
    // or painted some).
    {
        let (made, out, notes, joined) = (made.clone(), out.clone(), notes.clone(), j.path.clone());
        mv_ui::bg::blocking(move || -> Result<()> {
            if !made.is_file() {
                bail!("the intro renderer saved no video");
            }
            let _ = std::fs::remove_file(&out);
            std::fs::rename(&made, &out).with_context(|| format!("save {}", out.display()))?;
            save_record(&out, &intro_key(&base, &cast_dir), &notes, &super::joined_sources(&joined));
            Ok(())
        })
        .await??;
    }
    if r.cancelled() {
        bail!(CANCELLED);
    }
    r.set(1., "Intro · Ready");
    // Why the profiles came from the notes is this run's news, not kept with the intro.
    let mut notes = notes;
    notes.extend(profile_note);
    Ok(MadeIntro { path: out, notes })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn character(id: &str, name: &str, n: u32) -> mv_recap::Character {
        mv_recap::Character { id: id.into(), name: name.into(), appearances: n, ..Default::default() }
    }

    fn profile(role: &str) -> Profile {
        Profile { role: role.into(), bio: "A sharp-eyed trainee monk who mocks the newcomer.".into(), traits: vec!["Smug".into(), "Skeptical".into(), "Proud".into()], facts: vec!["First seen in chapter 2".into()] }
    }

    fn gathered() -> Gathered {
        Gathered {
            size: (1920, 1080),
            fps: "30/1".into(),
            cast: vec![character("mu-gung-1a2b", "Mu Gung", 9)],
            profiles: HashMap::from([("mu-gung-1a2b".to_string(), profile("Smug trainee monk"))]),
            recaps: vec![],
            notes: String::new(),
            reads: vec![json!({"read": "read.json", "pages": ["001.jpg"]})],
            read_files: vec![PathBuf::from("read.json")],
            style: mv_score::Style::Epic,
            reveal: Some(PathBuf::from("ch1.mp4")),
            roto: None,
            magnific_key: true,
        }
    }

    /// The sorted keys of a JSON object.
    fn keys(v: &Value) -> Vec<&str> {
        let mut k: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        k.sort_unstable();
        k
    }

    /// `NAME = <number>` (an optional comment after it) at the start of a line of python/`file`.
    fn python_number(file: &str, name: &str) -> f64 {
        let path = format!("{}/../../../python/{file}", env!("CARGO_MANIFEST_DIR"));
        let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        src.lines()
            .find_map(|l| l.strip_prefix(&format!("{name} = ")))
            .and_then(|v| v.split('#').next())
            .and_then(|v| v.trim().parse::<f64>().ok())
            .unwrap_or_else(|| panic!("{file} has no number {name}"))
    }

    #[test]
    fn the_cast_is_the_most_seen_named_characters() {
        let mut brain = mv_recap::Brain::default();
        brain.characters = vec![character("k", "Kaida", 5), character("x", " ", 99), character("h", " Hachiro ", 40), character("a", "Antennar", 12)];
        let names: Vec<String> = cast(&brain).into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["Hachiro", "Antennar", "Kaida"]);
        // At most twice the cards: the renderer shows the first ones it can.
        brain.characters = (0..20).map(|i| character(&format!("c{i}"), &format!("C{i}"), i)).collect();
        assert_eq!(cast(&brain).len(), MAX_CAST);
    }

    #[test]
    fn the_renderer_gets_each_characters_id_description_and_faces() {
        let d = tempfile::tempdir().unwrap();
        let face = d.path().join("mu-gung-1a2b_0.jpg");
        std::fs::write(&face, b"jpg").unwrap();
        let mut brain = mv_recap::Brain::default();
        brain.characters = vec![
            mv_recap::Character {
                id: "mu-gung-1a2b".into(),
                name: "Mu Gung".into(),
                aliases: vec![" Gung ".into(), " ".into()],
                description: " A wandering monk with a staff. ".into(),
                traits: vec!["calm".into(), "stubborn".into()],
                faces: vec![face.clone(), d.path().join("gone.jpg")],
                appearances: 9,
                ..Default::default()
            },
            character("", "Hye Jung", 3),
        ];
        let c = cast(&brain);
        let v = cast_json(&c, &HashMap::new());
        assert_eq!(v[0], json!({"name": "Mu Gung", "aliases": ["Gung"], "id": "mu-gung-1a2b", "description": "A wandering monk with a staff.", "traits": ["calm", "stubborn"], "faces": [face], "profile": null}));
        // Saved without an id (an old brain): one made from the name, the same every time, so
        // each character's drawing and background are its own files.
        let id = v[1]["id"].as_str().unwrap();
        assert!(id.starts_with("hye-jung-"), "{id}");
        assert_eq!(cast(&brain)[1].id, id);
        assert_ne!(id_from_name("Hye Jung"), id_from_name("Hye-Jung!"));
        // The key holds what the cards show and what finds the pictures, not the face pictures.
        assert_eq!(cast_key(&c, &HashMap::new())[0], json!(["mu-gung-1a2b", "Mu Gung", ["Gung"], null]));
    }

    #[test]
    fn each_card_gets_its_characters_profile() {
        let g = gathered();
        let v = render_args(&g, None, Path::new("intro"), Path::new("o.mp4"), Path::new("w"), Path::new("p.json"));
        // Exactly the agreed shape: {role, bio, traits, facts}.
        assert_eq!(
            v["cast"][0]["profile"],
            json!({"role": "Smug trainee monk", "bio": "A sharp-eyed trainee monk who mocks the newcomer.", "traits": ["Smug", "Skeptical", "Proud"], "facts": ["First seen in chapter 2"]})
        );
        // Without one (or with nothing in it): null.
        let none = Gathered { profiles: HashMap::from([("mu-gung-1a2b".to_string(), Profile::default())]), ..gathered() };
        assert!(render_args(&none, None, Path::new("intro"), Path::new("o.mp4"), Path::new("w"), Path::new("p.json"))["cast"][0]["profile"].is_null());
        let other = Gathered { profiles: HashMap::from([("someone-else".to_string(), profile("Abbot"))]), ..gathered() };
        assert!(cast_json(&other.cast, &other.profiles)[0]["profile"].is_null());
    }

    #[test]
    fn a_new_or_edited_profile_makes_the_intro_again() {
        let key = base_key(&gathered());
        assert_eq!(base_key(&gathered()), key, "nothing changed: the same key");
        // Edited (by hand, or written again by the AI): a role, a bio, a trait, a fact.
        let edit = |f: &dyn Fn(&mut Profile)| {
            let mut g = gathered();
            f(g.profiles.get_mut("mu-gung-1a2b").unwrap());
            base_key(&g)
        };
        assert_ne!(edit(&|p| p.role = "Abbot's favourite".into()), key);
        assert_ne!(edit(&|p| p.bio.push_str(" Again.")), key);
        assert_ne!(edit(&|p| p.traits[2] = "Loyal".into()), key);
        assert_ne!(edit(&|p| p.facts.clear()), key);
        // A profile for a character who had none, and one taken away.
        let bare = Gathered { profiles: HashMap::new(), ..gathered() };
        assert_ne!(base_key(&bare), key);
        assert_eq!(base_key(&bare)["cast"][0], json!(["mu-gung-1a2b", "Mu Gung", [], null]));
    }

    #[test]
    fn the_activity_line_says_where_the_profiles_come_from() {
        assert_eq!(profile_step(ProfileStep::Writing(4), "Sonnet 5.5"), "Writing the cast profiles with Sonnet 5.5");
        assert_eq!(profile_step(ProfileStep::Kept, "Sonnet 5.5"), "Using the cast profiles written earlier");
        assert_eq!(profile_step(ProfileStep::FromNotes, ""), "No AI model set, so the profiles come from the character notes");
        assert_eq!(profile_note("the AI took longer\nthan 60 s"), "the cast profiles come from the character notes (the AI could not write them: the AI took longer than 60 s)");
        // A picked model writes them; the picker's empty model is no model.
        let m = |model: &str| Some((mv_recap::ModelChoice { provider: mv_ai::AiProviderId::ClaudeCode, model: model.into() }, "Opus".to_string()));
        assert_eq!(profile_model(m("claude-opus")).map(|x| x.1), Some("Opus".to_string()));
        assert_eq!(profile_model(m(" ")), None);
        assert_eq!(profile_model(None), None);
    }

    #[test]
    fn each_character_is_first_seen_in_a_numbered_chapter() {
        let ch = |id: &str, number: Option<f64>| crate::data::ChapterItem {
            id: id.into(),
            manga_id: "m".into(),
            manga_title: "M".into(),
            number,
            title: None,
            pages_dir: PathBuf::from("pages"),
            output_dir: PathBuf::from("out"),
            page_count: 1,
            has_text: true,
            video: None,
            audio_files: 0,
        };
        let numbers = chapter_numbers(&[ch("a", Some(1.)), ch("b", Some(4.5)), ch("c", None)]);
        assert_eq!(numbers, HashMap::from([("a".to_string(), 1.), ("b".to_string(), 4.5)]));
    }

    #[test]
    fn the_renderer_gets_the_cast_and_its_folder_and_nothing_of_a_logo() {
        let g = gathered();
        let folder = intro_dir("manga-a");
        let v = render_args(&g, Some(Path::new("music.wav")), &folder, Path::new("full_recap_intro.new.mp4"), Path::new("render"), Path::new("progress.json"));
        // Exactly these: no cover, plan, plate, Blender, world cache or samples (the logo ident's).
        assert_eq!(keys(&v), ["cast", "cast_draw", "chapters", "fps", "height", "intro_dir", "music", "out", "progress_file", "reveal", "roto_data", "roto_python", "studio_dir", "style", "width", "work_dir"]);
        assert_eq!(v["intro_dir"], json!(folder));
        assert_eq!((v["width"].as_u64(), v["height"].as_u64(), v["fps"].as_str(), v["style"].as_str()), (Some(1920), Some(1080), Some("30/1"), Some("epic")));
        assert_eq!((v["cast"][0]["id"].as_str(), v["cast_draw"].as_bool()), (Some("mu-gung-1a2b"), Some(true)));
        assert_eq!((v["music"].as_str(), v["reveal"].as_str()), (Some("music.wav"), Some("ch1.mp4")));
        assert!(v["roto_python"].is_null(), "no layered-motion pack: no cut-out helper");
        assert_eq!(v["chapters"], json!([{"read": "read.json", "pages": ["001.jpg"]}]));
        // Without music the cards play on their whooshes alone.
        assert!(render_args(&g, None, &folder, Path::new("o.mp4"), Path::new("w"), Path::new("p.json"))["music"].is_null());
        // A kept intro is checked against what the cards and the transition show and whether
        // Magnific can draw the cast; nothing of a cover, a world or a logo.
        let key = base_key(&g);
        assert_eq!(keys(&key), ["cast", "fps", "magnific", "reads", "reveal", "roto", "size", "style", "v"]);
        assert_eq!((key["v"].as_u64(), key["magnific"].as_bool()), (Some(INTRO_VERSION), Some(true)));
        assert_ne!(base_key(&Gathered { magnific_key: false, ..gathered() }), key);
    }

    #[test]
    fn the_progress_file_is_read_forgivingly() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("progress.json");
        let since = SystemTime::now();
        // Missing, or caught half written: nothing new.
        assert_eq!(read_progress(&f, since), None);
        std::fs::write(&f, r#"{"p": 0.4, "step": "Painting Mu Gu"#).unwrap();
        assert_eq!(read_progress(&f, since), None);
        std::fs::write(&f, r#"{"p": 0.4, "step": "Painting Mu Gung's background with Magnific\n (2 of 4)", "stage": "background"}"#).unwrap();
        assert_eq!(read_progress(&f, since), Some(Progress { p: Some(0.4), step: "Painting Mu Gung's background with Magnific (2 of 4)".into() }));
        // Out of range: clamped; missing: left where it was; no step: nothing to show.
        std::fs::write(&f, r#"{"p": 7, "step": "Saving the intro", "stage": "encode"}"#).unwrap();
        assert_eq!(read_progress(&f, since).unwrap().p, Some(1.0));
        std::fs::write(&f, r#"{"step": "Rendering the transition"}"#).unwrap();
        assert_eq!(read_progress(&f, since), Some(Progress { p: None, step: "Rendering the transition".into() }));
        std::fs::write(&f, r#"{"p": 0.5, "step": "  "}"#).unwrap();
        assert_eq!(read_progress(&f, since), None);
        // A long step is cut to fit the activity line.
        std::fs::write(&f, json!({"p": 0.1, "step": "x".repeat(300)}).to_string()).unwrap();
        assert_eq!(read_progress(&f, since).unwrap().step.chars().count(), MAX_STEP + 1);
        // Left over from an earlier run: not this run's progress.
        std::fs::write(&f, r#"{"p": 0.9, "step": "Saving the intro"}"#).unwrap();
        std::fs::File::options().write(true).open(&f).unwrap().set_modified(since - Duration::from_secs(3600)).unwrap();
        assert_eq!(read_progress(&f, since), None);
        // The renderer's share of the intro's bar.
        assert_eq!((render_bar(0.), render_bar(1.), render_bar(5.)), (RENDER_FROM, RENDER_TO, RENDER_TO));
        assert!(render_bar(0.5) > RENDER_FROM && render_bar(0.5) < RENDER_TO);
    }

    #[test]
    fn credits_are_counted_by_model_and_unknown_ones_left_out() {
        let v = answer(concat!(
            "drawing\n",
            r#"{"ok": false, "message": "Magnific refused to draw Mu Gung", "credits": [{"model": "nano-banana-2-1", "credits": 50.0}, {"model": "fal-nano-banana", "credits": 0}, {"model": "made-up-model", "credits": 12}, {"model": "nano-banana-2-1", "credits": -3}, "junk"], "notes": []}"#,
            "\n"
        ));
        let (known, unknown) = credit_entries(&v);
        let known: Vec<(&str, f64)> = known.iter().map(|(m, c)| (m.id, *c)).collect();
        assert_eq!(known, [("nano-banana-2-1", 50.0), ("fal-nano-banana", 0.0), ("nano-banana-2-1", 0.0)]);
        assert_eq!(unknown, ["made-up-model", "\"junk\""]);
        // Counted although the intro failed; the failure says why.
        assert_eq!(failure(&v, Some(0), "").as_deref(), Some("Magnific refused to draw Mu Gung"));
        // No answer (the renderer crashed): nothing to count.
        let (known, unknown) = credit_entries(&answer("Traceback"));
        assert!(known.is_empty() && unknown.is_empty());
    }

    #[test]
    fn the_renderers_answer_or_its_reason() {
        let ok = answer("rendering\n{\"ok\": true, \"path\": \"x.mp4\", \"seconds\": 8.1, \"credits\": [], \"notes\": [\" Hye Jung was shown\\nfrom her panel \", \"Hye Jung was shown from her panel\", \"\"]}\n\n");
        assert_eq!(failure(&ok, Some(0), ""), None);
        assert_eq!(notes_of(&ok), ["Hye Jung was shown from her panel"]);
        let e = failure(&answer(""), Some(1), "Traceback\nValueError: no character has a picture yet\n\n");
        assert_eq!(e.as_deref(), Some("ValueError: no character has a picture yet"));
        assert_eq!(failure(&answer("not json"), Some(3), "").as_deref(), Some("intro_maker exited Some(3)"));
    }

    #[test]
    fn the_result_says_how_the_intro_was_made() {
        let it = MadeIntro { path: "full_recap_intro.mp4".into(), notes: vec![] };
        assert_eq!(it.note(), "");
        let it = MadeIntro { notes: vec!["Hye Jung was shown from her panel".into(), "Mu Gung's background is a scene from his pages".into()], ..it };
        assert_eq!(it.note(), " · intro: Hye Jung was shown from her panel; Mu Gung's background is a scene from his pages");
    }

    #[test]
    fn a_new_drawing_or_background_makes_the_intro_again() {
        let d = tempfile::tempdir().unwrap();
        let cast_dir = d.path().join("cast");
        let base = json!({"v": INTRO_VERSION, "magnific": true});
        let none = intro_key(&base, &cast_dir);
        std::fs::create_dir_all(&cast_dir).unwrap();
        std::fs::write(cast_dir.join("mu-gung-1a2b.json"), b"{}").unwrap();
        assert_eq!(intro_key(&base, &cast_dir), none, "only the pictures count");
        std::fs::write(cast_dir.join("mu-gung-1a2b.png"), b"png").unwrap();
        let drawn = intro_key(&base, &cast_dir);
        assert_ne!(drawn, none);
        assert_eq!(intro_key(&base, &cast_dir), drawn, "nothing changed: the same key");
        // Its background, made from the pages Mu Gung appears in, then made again.
        std::fs::write(cast_dir.join("mu-gung-1a2b.bg.png"), b"bg png").unwrap();
        let painted = intro_key(&base, &cast_dir);
        assert_ne!(painted, drawn);
        std::fs::write(cast_dir.join("mu-gung-1a2b.bg.png"), b"painted again png").unwrap();
        let repainted = intro_key(&base, &cast_dir);
        assert_ne!(repainted, painted);
        // Redrawn.
        std::fs::write(cast_dir.join("mu-gung-1a2b.png"), b"redrawn png").unwrap();
        let redrawn = intro_key(&base, &cast_dir);
        assert_ne!(redrawn, repainted);
        // A Magnific key added later makes it again too.
        assert_ne!(intro_key(&json!({"v": INTRO_VERSION, "magnific": false}), &cast_dir), redrawn);
        // The kept intro is reused only for its own key; the export preview plays it for the same
        // chapter videos.
        let (out, joined) = (d.path().join("full_recap_intro.mp4"), d.path().join("full_recap.mp4"));
        std::fs::write(joined.with_extension("json"), json!({"v": 2, "sources": [["a.mp4", 1, 2]]}).to_string()).unwrap();
        save_record(&out, &redrawn, &["Hye Jung was shown from her panel".into()], &super::super::joined_sources(&joined));
        assert!(kept_if_current(&out, &redrawn).is_none(), "no video yet");
        assert_eq!(kept_intro_at(&out, &joined), None);
        std::fs::write(&out, b"mp4").unwrap();
        assert_eq!(kept_if_current(&out, &redrawn).map(|r| notes_of(&r)), Some(vec!["Hye Jung was shown from her panel".to_string()]));
        assert!(kept_if_current(&out, &drawn).is_none());
        assert_eq!(kept_intro_at(&out, &joined), Some(out.clone()));
        std::fs::write(joined.with_extension("json"), json!({"v": 2, "sources": [["a.mp4", 9, 2]]}).to_string()).unwrap();
        assert_eq!(kept_intro_at(&out, &joined), None);
    }

    #[test]
    fn every_file_of_an_intro_is_in_its_mangas_own_folders() {
        let (a, b) = (kept_intro_path("manga-a"), kept_intro_path("manga-b"));
        assert_eq!(a, mv_recap::media::manga_dir("manga-a").join("full_recap_intro.mp4"));
        assert_ne!(a.parent(), b.parent());
        // The work folder (arguments, music, progress file, frames) is beside the kept intro.
        assert_eq!(a.with_extension("work").parent(), a.parent());
        // The intro folder (the cast's drawings and backgrounds) is the manga's Studio folder's.
        assert_eq!(intro_dir("manga-a"), mv_recap::Brain::dir("manga-a").join("intro"));
        assert_ne!(intro_dir("manga-a"), intro_dir("manga-b"));
        assert!(intro_dir("abc/1 2").ends_with(Path::new("abc_1_2").join("intro")), "{}", intro_dir("abc/1 2").display());
    }

    #[test]
    fn intro_length_matches_the_renderer() {
        // At most 4 cast cards of 2.5 s (time to read each profile), then the ribbon transition
        // onto the recap's first frame, which settles while the music fades out: 12.6 s in all.
        let (m, t) = ("intro_maker.py", "intro_transition.py");
        let (card, cards) = (python_number(m, "CARD_SEC"), python_number(m, "MAX_CARDS"));
        let (ribbon, pass, reveal) = (python_number(t, "RIBBON_SEC"), python_number(t, "PASS_SEC"), python_number(t, "REVEAL_SEC"));
        assert_eq!((card, cards, ribbon, pass, reveal), (CARD_SEC, MAX_CARDS as f64, RIBBON_SEC, PASS_SEC, REVEAL_SEC));
        assert_eq!(CARD_SEC, 2.5);
        let long = cards * card + ribbon + reveal;
        assert!((long - INTRO_SEC).abs() < 1e-9 && (long - 12.6).abs() < 1e-9 && long <= 13.5, "{long}");
        // The music's big moment is the pass through the ribbons, after the last card and before
        // the recap.
        let hit = cards * card + pass;
        assert!((hit - MUSIC_HIT).abs() < 1e-9 && hit > cards * card && hit < long, "{hit}");
        let dir = format!("{}/../../../python/", env!("CARGO_MANIFEST_DIR"));
        for f in ["intro_maker.py", "intro_cast.py", "intro_transition.py", "intro_roto.py"] {
            assert!(Path::new(&format!("{dir}{f}")).is_file(), "{f}");
        }
        // The logo ident is gone: no world, no logo, no sting.
        for f in ["intro_ident.py", "intro_theme.py", "intro_depth.py", "brand_sting.py", "brand"] {
            assert!(!Path::new(&format!("{dir}{f}")).exists(), "{f} still ships");
        }
    }
}
