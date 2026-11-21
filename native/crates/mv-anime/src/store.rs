//! On-disk layout under `<data>/anime/` (design 5.1, 5.2): paths, atomic JSON writes with
//! history snapshots, corrupt-file recovery, the content-addressed take store and the project
//! listing. Every save bumps [`version`] so pages know to reload.
//!
//! Paths inside JSON are project-relative with forward slashes; manga pages and panels are
//! stored as `SourceRef`s, never as absolute paths.
//!
//! Times: every `*_at` field of the model is unix **seconds** ([`now_s`]); history snapshots
//! are named by unix **milliseconds**.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;

use crate::model::{BibleIndex, Character, Episode, Issue, Place, Prop, SceneShots, Series, Shot, Story, Style, Take, TakeMeta, VoiceCast, SCHEMA};

/// Snapshots kept per entity file in `history/`.
pub const HISTORY_KEEP: usize = 50;
/// Top-level fields that are worked out from the rest of a file (the checks' findings): a save
/// that changes only these is not a new version worth keeping.
const DERIVED: [&str; 1] = ["issues"];

/// What a file written by a newer app answers when this build would have to change it.
pub const NEWER: &str = "Made with a newer MangaVox; update to edit";

static VERSION: AtomicU64 = AtomicU64::new(0);
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
/// One writer at a time: the history snapshot and the replace of one file must not interleave
/// with another save of the same file.
static WRITE_LOCK: Mutex<()> = Mutex::new(());
static WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Project folders being removed while their jobs wind down: a write never makes them again.
static CLOSING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// What a write into a removed project answers.
pub const REMOVED: &str = "This project was removed, so nothing more is saved in it.";

/// One lock per file, held across a whole read-change-write ([`update_file`]) and every plain
/// save of that file, so a job and a screen never lose each other's change. Lock order: a
/// file's lock first, then `WRITE_LOCK`.
fn file_lock(path: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let mut m = LOCKS.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|e| e.into_inner());
    m.entry(path.to_path_buf()).or_default().clone()
}

/// Marks a project folder as being removed (`on`) or not. While marked, nothing recreates it
/// once it is gone (renamed or deleted); writes into it fail with [`REMOVED`].
pub(crate) fn set_closing(dir: &Path, on: bool) {
    let mut c = CLOSING.lock().unwrap_or_else(|e| e.into_inner());
    c.retain(|d| d != dir);
    if on {
        c.push(dir.to_path_buf());
    }
}

/// Refuses a write under a project folder that was removed while its jobs wound down.
fn refuse_removed(path: &Path) -> Result<()> {
    let Some((proj, _)) = project_of(path) else { return Ok(()) };
    if proj.is_dir() {
        return Ok(());
    }
    if CLOSING.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|d| *d == proj) {
        bail!(REMOVED);
    }
    Ok(())
}

/// `create_dir_all` for a folder inside a project, except into a project that was removed
/// meanwhile (see [`set_closing`]).
pub(crate) fn make_dir(dir: &Path) -> Result<()> {
    refuse_removed(&dir.join("x"))?;
    std::fs::create_dir_all(dir).context("Could not create a folder for the project's files. Check that the data drive is connected and has free space.")
}

/// Files removed when this is dropped (temp and `.part` files on every way out: error, cancel,
/// success). [`Scratch::keep`] takes one off the list.
#[derive(Default)]
pub(crate) struct Scratch(Vec<PathBuf>);

impl Scratch {
    pub(crate) fn add(&mut self, p: &Path) {
        self.0.push(p.to_path_buf());
    }

    pub(crate) fn keep(&mut self, p: &Path) {
        self.0.retain(|x| x != p);
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Reads `path` fresh, applies `f` and writes it back atomically, all under the file's own
/// lock. A `schema` field is stamped to this build's; a file from a newer MangaVox is refused.
/// Answers the value as saved. `f` only changes the value: it must not save that same file
/// (it would wait for its own lock), and should be quick (other savers of the file wait).
pub fn update_file<T: DeserializeOwned + Serialize + Default>(path: &Path, f: impl FnOnce(&mut T)) -> Result<T> {
    let lock = file_lock(path);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut v: T = read_json(path)?;
    f(&mut v);
    let mut value = serde_json::to_value(&v).with_context(|| format!("Could not prepare {} for saving.", display_name(path)))?;
    let stamp = match value.as_object_mut() {
        Some(m) => match m.get("schema").map(Value::as_u64) {
            Some(Some(s)) if s > SCHEMA as u64 => bail!("{NEWER} ({}).", display_name(path)),
            Some(_) if m.get("schema").and_then(Value::as_u64) != Some(SCHEMA as u64) => {
                m.insert("schema".into(), Value::from(SCHEMA));
                true
            }
            _ => false,
        },
        None => false,
    };
    if stamp {
        // Back through the type, so the file keeps its usual field order.
        if let Ok(stamped) = serde_json::from_value::<T>(value.clone()) {
            v = stamped;
        } else {
            write_bytes(path, &serde_json::to_vec_pretty(&value)?)?;
            return Ok(v);
        }
    }
    let bytes = serde_json::to_vec_pretty(&v).with_context(|| format!("Could not prepare {} for saving.", display_name(path)))?;
    write_bytes(path, &bytes)?;
    Ok(v)
}

/// `<data>/anime` (`mv_core::paths::root().join("anime")`).
pub fn root() -> PathBuf {
    mv_core::paths::root().join("anime")
}

/// `<data>/anime/projects`.
pub fn projects_dir() -> PathBuf {
    root().join("projects")
}

/// Bumped on every save through this module.
pub fn version() -> u64 {
    VERSION.load(Ordering::SeqCst)
}

fn bump() {
    VERSION.fetch_add(1, Ordering::SeqCst);
}

/// Unix seconds now (the unit of every `*_at` field).
pub fn now_s() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Unix milliseconds now (history snapshot names).
pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// A file with this `schema` was written by a newer MangaVox: show it, do not edit it.
pub fn is_newer(schema: u32) -> bool {
    schema > SCHEMA
}

/// Warnings for the user collected since the last call (a damaged file put back from its
/// history, ...), oldest first. Each is a full sentence; the page shows them once.
pub fn take_warnings() -> Vec<String> {
    std::mem::take(&mut *WARNINGS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// A project file that was found damaged and put back (or started empty), for the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recovery {
    /// The project's id (its folder name).
    pub project: String,
    /// What it was, in plain words ("the story of episode 1", "the voices"); never a path.
    pub what: String,
    /// How old the copy put back is ("8 minutes ago"); `None` when it had to start empty.
    pub restored: Option<String>,
}

static RECOVERIES: Mutex<Vec<Recovery>> = Mutex::new(Vec::new());

/// Every project file found damaged and put back since the last call, oldest first; each is
/// handed out once.
pub fn take_recoveries() -> Vec<Recovery> {
    std::mem::take(&mut *RECOVERIES.lock().unwrap_or_else(|e| e.into_inner()))
}

fn recovered(proj: &Path, rel: &str, restored: Option<String>) {
    let project = proj.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut r = RECOVERIES.lock().unwrap_or_else(|e| e.into_inner());
    r.push(Recovery { project, what: plain_what(rel), restored });
    if r.len() > 50 {
        let extra = r.len() - 50;
        r.drain(..extra);
    }
}

/// A project file in the user's words: "the story of episode 1", "the shots of a scene".
pub fn plain_what(rel: &str) -> String {
    let parts: Vec<&str> = rel.split('/').collect();
    let episode = |id: &str| {
        let digits: String = id.chars().filter(char::is_ascii_digit).collect();
        match digits.parse::<u32>() {
            Ok(n) => format!("episode {n}"),
            Err(_) => "an episode".to_string(),
        }
    };
    match parts.as_slice() {
        ["project.json"] => "the project's settings".into(),
        ["episodes", ep, "story.json"] => format!("the story of {}", episode(ep)),
        ["episodes", ep, "episode.json"] => format!("the details of {}", episode(ep)),
        ["episodes", ep, "cut.json"] => format!("the cut of {}", episode(ep)),
        ["episodes", ep, "checks.json"] => format!("the checks of {}", episode(ep)),
        ["episodes", _, "shots", _] => "the shots of a scene".into(),
        ["bible", "cast.json"] => "the voices".into(),
        ["bible", "style.json"] => "the bible's style".into(),
        ["bible", "characters", _] => "a character in the bible".into(),
        ["bible", "places", _] => "a place in the bible".into(),
        ["bible", "props", _] => "a prop in the bible".into(),
        ["bible", ..] => "the bible".into(),
        ["takes", ..] => "a picture or sound's details".into(),
        _ => "a file of the project".into(),
    }
}

fn warn(msg: String) {
    mv_core::logging::global().warn("anime", &msg);
    let mut w = WARNINGS.lock().unwrap_or_else(|e| e.into_inner());
    w.push(msg);
    // Nobody is reading them (no page open): keep only the newest few.
    if w.len() > 20 {
        let extra = w.len() - 20;
        w.drain(..extra);
    }
}

/// Atomic pretty-JSON write: temp file, `sync_all`, rename. Snapshots the replaced file into
/// the project's `history/` when its bytes differ, then bumps [`version`].
pub fn write_json<T: Serialize>(path: &Path, v: &T) -> Result<()> {
    let lock = file_lock(path);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let bytes = serde_json::to_vec_pretty(v).with_context(|| format!("Could not prepare {} for saving.", display_name(path)))?;
    write_bytes(path, &bytes)
}

/// Missing file: `T::default()`. A corrupt file is set aside as `<name>.corrupt-<unix_ms>` and
/// the newest history snapshot is restored (logged as a warning).
///
/// A file written by a newer MangaVox that this build cannot read is left untouched and
/// answers an error saying so. A file that cannot be read at all (locked, drive gone) answers
/// an error too: it is never mistaken for a missing one.
pub fn read_json<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
        Err(e) => bail!("Could not open {}: {e}. Check that the data drive is connected and try again.", display_name(path)),
    };
    match parse::<T>(&bytes) {
        Ok(v) => Ok(v),
        Err(Bad::Newer) => bail!("{NEWER} ({}).", display_name(path)),
        Err(Bad::Broken(why)) => recover(path, &why),
    }
}

enum Bad {
    Newer,
    Broken(String),
}

/// Strict first; then, for valid JSON, a second try with `null` object fields dropped (they
/// read as defaults: an `Option` is `None` anyway, and a number saved as `null` because it was
/// not finite reads as 0 instead of failing the whole file) and `null` array or tuple elements
/// read as 0 (a position `[NaN, 1.0]` was saved as `[null, 1.0]`).
fn parse<T: DeserializeOwned>(bytes: &[u8]) -> std::result::Result<T, Bad> {
    let strict = match serde_json::from_slice::<T>(bytes) {
        Ok(v) => return Ok(v),
        Err(e) => e.to_string(),
    };
    let mut value: Value = serde_json::from_slice(bytes).map_err(|e| Bad::Broken(e.to_string()))?;
    if value.get("schema").and_then(Value::as_u64).is_some_and(|s| s > SCHEMA as u64) {
        // A newer file is never "repaired" by an older app.
        return Err(Bad::Newer);
    }
    drop_nulls(&mut value);
    serde_json::from_value::<T>(value).map_err(|_| Bad::Broken(strict))
}

fn drop_nulls(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.retain(|_, x| !x.is_null());
            m.values_mut().for_each(drop_nulls);
        }
        Value::Array(a) => a.iter_mut().for_each(|x| {
            if x.is_null() {
                *x = Value::from(0);
            } else {
                drop_nulls(x);
            }
        }),
        _ => {}
    }
}

/// Sets the damaged file aside and puts back its newest readable snapshot.
fn recover<T: DeserializeOwned + Default>(path: &Path, why: &str) -> Result<T> {
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Another thread may have put it back while this one waited.
    if let Ok(bytes) = std::fs::read(path) {
        if let Ok(v) = parse::<T>(&bytes) {
            return Ok(v);
        }
    }
    let name = display_name(path);
    let aside = sibling(path, &format!("corrupt-{}", now_ms()));
    std::fs::rename(path, &aside).with_context(|| format!("{name} is damaged and could not be set aside. Close other programs using it and try again."))?;
    let aside_name = aside.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    mv_core::logging::global().warn("anime", &format!("{name} could not be read ({why}); set aside as {aside_name}"));
    if let Some((proj, rel)) = project_of(path) {
        for ms in snapshots(&proj, &rel) {
            let snap = history_dir(&proj, &rel).join(format!("{ms}.json"));
            let Ok(bytes) = std::fs::read(&snap) else { continue };
            if let Ok(v) = parse::<T>(&bytes) {
                atomic_write(path, &bytes)?;
                bump();
                recovered(&proj, &rel, Some(ago(ms)));
                warn(format!(
                    "{rel} was damaged and could not be read, so it was put back as it was {}. The damaged copy is kept as {aside_name}.",
                    ago(ms)
                ));
                return Ok(v);
            }
        }
    }
    warn(format!("{name} was damaged and could not be read, and there is no earlier version of it, so it starts empty. The damaged copy is kept as {aside_name}."));
    if let Some((proj, rel)) = project_of(path) {
        recovered(&proj, &rel, None);
    }
    bump();
    Ok(T::default())
}

/// "3 minutes ago" for a unix-ms time.
fn ago(ms: i64) -> String {
    let s = ((now_ms() - ms) / 1000).max(0);
    match s {
        0..=59 => "a moment ago".into(),
        60..=119 => "a minute ago".into(),
        120..=3599 => format!("{} minutes ago", s / 60),
        3600..=7199 => "an hour ago".into(),
        7200..=86_399 => format!("{} hours ago", s / 3600),
        86_400..=172_799 => "a day ago".into(),
        _ => format!("{} days ago", s / 86_400),
    }
}

/// The file's name for messages (never the whole path: it can hold the user's name).
fn display_name(path: &Path) -> String {
    match project_of(path) {
        Some((_, rel)) => rel,
        None => path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "a file".into()),
    }
}

/// `<file>.<suffix>` beside `path`.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(format!(".{suffix}"));
    path.with_file_name(name)
}

/// Writes `bytes` to `path` unless it already holds them, snapshotting the old content first
/// (see [`wants_snapshot`]; `always` for a restore, so the version it replaces is never lost).
fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    write_bytes_with(path, bytes, Some(false))
}

/// `snapshot`: `Some(true)` always keeps the replaced content (a restore), `Some(false)` when it
/// earns one ([`wants_snapshot`]), `None` never (a change the user did not make and would not
/// want to undo, such as crops scored again by a newer version).
fn write_bytes_with(path: &Path, bytes: &[u8], snapshot_mode: Option<bool>) -> Result<()> {
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let old = std::fs::read(path).ok();
    if old.as_deref() == Some(bytes) {
        return Ok(());
    }
    if let (Some(old), Some((proj, rel)), Some(always)) = (old, project_of(path), snapshot_mode) {
        if keeps_history(&rel) && serde_json::from_slice::<Value>(&old).is_ok() && (always || wants_snapshot(&proj, &rel, &old, bytes)) {
            if let Err(e) = snapshot(&proj, &rel, &old) {
                // Saving matters more than its undo copy: log and go on.
                mv_core::logging::global().warn("anime", &format!("no history snapshot of {rel}: {e:#}"));
            }
        }
    }
    atomic_write(path, bytes)?;
    bump();
    Ok(())
}

/// Temp file in the same folder, flushed to disk, then renamed over the target.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let name = display_name(path);
    refuse_removed(path)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("Could not create the folder for {name}. Check that the data drive is connected and has free space."))?;
    }
    let tmp = sibling(path, &format!("tmp-{}-{}", std::process::id(), TMP_SEQ.fetch_add(1, Ordering::SeqCst)));
    let written = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        bail!("Could not save {name}: {e}. Check that the data drive has free space.");
    }
    // Windows refuses the replace while another program (an antivirus, a backup) has the file
    // open for a moment: try a few times.
    let mut last = None;
    for attempt in 0..6u64 {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(40 * (attempt + 1)));
            }
        }
    }
    let _ = std::fs::remove_file(&tmp);
    Err(anyhow!(
        "Could not save {name}: {}. Another program may be using it; close it and try again.",
        last.map(|e| e.to_string()).unwrap_or_default()
    ))
}

/// The project folder holding `path` (`.../anime/projects/<id>/`) and the project-relative path.
fn project_of(path: &Path) -> Option<(PathBuf, String)> {
    for dir in path.ancestors().skip(1) {
        let parent = dir.parent()?;
        let is_projects = parent.file_name().is_some_and(|n| n == "projects");
        let is_anime = parent.parent().and_then(Path::file_name).is_some_and(|n| n == "anime");
        if is_projects && is_anime {
            let rel = path.strip_prefix(dir).ok()?;
            return Some((dir.to_path_buf(), slash(rel)));
        }
    }
    None
}

/// Forward-slash form of a relative path.
fn slash(rel: &Path) -> String {
    rel.components().filter_map(|c| if let Component::Normal(s) = c { Some(s.to_string_lossy().into_owned()) } else { None }).collect::<Vec<_>>().join("/")
}

/// Edited entity files keep history; derived, immutable and scratch files do not.
fn keeps_history(rel: &str) -> bool {
    rel.ends_with(".json")
        && !["history/", "takes/", "work/"].iter().any(|p| rel.starts_with(p))
        && !rel.ends_with("/checks.json")
        && !rel.contains("/exports/")
}

fn history_dir(proj: &Path, rel: &str) -> PathBuf {
    proj.join("history").join(rel.replace('/', "__"))
}

/// Snapshot times of `rel`, newest first.
fn snapshots(proj: &Path, rel: &str) -> Vec<i64> {
    let mut v: Vec<i64> = std::fs::read_dir(history_dir(proj, rel))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    name.strip_suffix(".json")?.parse::<i64>().ok()
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_unstable_by(|a, b| b.cmp(a));
    v
}

/// The JSON of a file without its [`DERIVED`] fields (`None` when it is not JSON).
fn content(bytes: &[u8]) -> Option<Value> {
    let mut v: Value = serde_json::from_slice(bytes).ok()?;
    if let Some(m) = v.as_object_mut() {
        for k in DERIVED {
            m.remove(k);
        }
    }
    Some(v)
}

/// Whether the content being replaced earns a snapshot: only when the new content really
/// differs from it (not just in derived fields or formatting), and when the newest snapshot
/// does not already hold it.
fn wants_snapshot(proj: &Path, rel: &str, old: &[u8], new: &[u8]) -> bool {
    let Some(before) = content(old) else { return false };
    if content(new).is_some_and(|after| after == before) {
        return false;
    }
    let Some(newest) = snapshots(proj, rel).into_iter().next() else { return true };
    std::fs::read(history_dir(proj, rel).join(format!("{newest}.json"))).ok().and_then(|b| content(&b)).is_none_or(|kept| kept != before)
}

/// Copies `old` into `history/<rel>/<unix_ms>.json` and keeps the newest [`HISTORY_KEEP`].
fn snapshot(proj: &Path, rel: &str, old: &[u8]) -> Result<()> {
    let dir = history_dir(proj, rel);
    let mut ms = now_ms();
    while dir.join(format!("{ms}.json")).exists() {
        ms += 1;
    }
    atomic_write(&dir.join(format!("{ms}.json")), old)?;
    for stale in snapshots(proj, rel).into_iter().skip(HISTORY_KEEP) {
        let _ = std::fs::remove_file(dir.join(format!("{stale}.json")));
    }
    Ok(())
}

/// Every project under `projects/` (folders named `.removed-*` are skipped), newest first.
pub fn list_projects() -> Vec<Series> {
    list_projects_in(&projects_dir())
}

fn list_projects_in(dir: &Path) -> Vec<Series> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Series> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| {
            let file = e.path().join("project.json");
            if !file.is_file() {
                return None;
            }
            match read_json::<Series>(&file) {
                Ok(mut s) => {
                    if s.id.is_empty() {
                        s.id = e.file_name().to_string_lossy().into_owned();
                    }
                    Some(s)
                }
                Err(err) => {
                    mv_core::logging::global().warn("anime", &format!("project skipped: {err:#}"));
                    None
                }
            }
        })
        .collect();
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| a.id.cmp(&b.id)));
    out
}

/// Top-level entities: their `schema` is stamped on save, and a newer file is never overwritten.
trait Entity: Serialize + Clone {
    fn schema(&mut self) -> &mut u32;
}

macro_rules! entity {
    ($($t:ty),+) => {$(
        impl Entity for $t {
            fn schema(&mut self) -> &mut u32 {
                &mut self.schema
            }
        }
    )+};
}

entity!(Series, Episode, Story, SceneShots, Character, Place, Prop, Style, VoiceCast, Take);

fn save_entity<T: Entity>(path: &Path, v: &T) -> Result<()> {
    save_entity_with(path, v, false)
}

/// [`save_entity`]; `quiet` keeps no "earlier version" copy of what it replaces.
fn save_entity_with<T: Entity>(path: &Path, v: &T, quiet: bool) -> Result<()> {
    let mut v = v.clone();
    let schema = v.schema();
    if is_newer(*schema) {
        bail!("{NEWER} ({}).", display_name(path));
    }
    *schema = SCHEMA;
    let lock = file_lock(path);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let bytes = serde_json::to_vec_pretty(&v).with_context(|| format!("Could not prepare {} for saving.", display_name(path)))?;
    write_bytes_with(path, &bytes, if quiet { None } else { Some(false) })
}

/// A bible entity saved by a change the user did not make (crops scored again by a newer
/// version): written only when its content changes, with no "earlier version" copy.
pub(crate) enum Quiet<'a> {
    Character(&'a Character),
    Place(&'a Place),
    Prop(&'a Prop),
    Style(&'a Style),
}

/// A path segment made from an id; an empty id is refused (it would name the wrong file).
fn seg(id: &str, what: &str) -> Result<String> {
    if id.trim().is_empty() {
        bail!("Could not save the {what}: it has no id.");
    }
    Ok(mv_core::paths::safe_name(id))
}

/// One project folder: `projects/<series_id>/`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Store {
    /// `mv_core::paths::safe_name(manga_id)`.
    pub id: String,
    pub dir: PathBuf,
}

impl Store {
    /// The project `series_id` (the folder need not exist yet).
    pub fn open(series_id: &str) -> Store {
        Store { id: series_id.to_string(), dir: projects_dir().join(series_id) }
    }

    /// The project of a manga: `projects/<safe_name(id)>`, as always. Only when that folder
    /// already belongs to another manga (two ids that differ in characters `safe_name` drops,
    /// or after its 80th) does this manga get `<safe_name(id)>-<8 hex of sha256(id)>`. No
    /// existing project's folder changes.
    pub fn for_manga(manga_id: &str) -> Store {
        Store::for_manga_in(&projects_dir(), manga_id)
    }

    fn for_manga_in(projects: &Path, manga_id: &str) -> Store {
        let plain = mv_core::paths::safe_name(manga_id);
        let alt = format!("{plain}-{}", &crate::hash::sha256_hex(manga_id.as_bytes())[..8]);
        let owner = |name: &str| -> Option<String> {
            let file = projects.join(name).join("project.json");
            file.is_file().then(|| read_json::<Series>(&file).map(|s| s.manga_id).unwrap_or_default())
        };
        let id = if owner(&alt).is_some() {
            alt
        } else {
            match owner(&plain) {
                Some(other) if !other.is_empty() && other != manga_id => alt,
                _ => plain,
            }
        };
        Store { dir: projects.join(&id), id }
    }

    /// Project-relative path (forward slashes) to an absolute one.
    pub fn path(&self, rel: &str) -> PathBuf {
        self.dir.join(rel)
    }

    /// Absolute path to project-relative with forward slashes; `None` outside the project.
    pub fn rel(&self, abs: &Path) -> Option<String> {
        let rel = abs.strip_prefix(&self.dir).ok()?;
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            return None;
        }
        Some(slash(rel))
    }

    /// `work/`: scratch for in-flight jobs (safe to delete).
    pub fn work_dir(&self) -> PathBuf {
        self.dir.join("work")
    }

    fn ep_dir(&self, ep: &str) -> PathBuf {
        self.dir.join("episodes").join(mv_core::paths::safe_name(ep))
    }

    /// The project's `project.json` (a default series when there is none yet).
    pub fn series(&self) -> Result<Series> {
        let mut s: Series = read_json(&self.dir.join("project.json"))?;
        if s.id.is_empty() {
            s.id = self.id.clone();
        }
        Ok(s)
    }

    /// Saves `project.json`.
    pub fn save_series(&self, series: &Series) -> Result<()> {
        save_entity(&self.dir.join("project.json"), series)
    }

    /// An episode's `episode.json`.
    pub fn episode(&self, ep: &str) -> Result<Episode> {
        let mut e: Episode = read_json(&self.ep_dir(ep).join("episode.json"))?;
        if e.id.is_empty() {
            e.id = ep.to_string();
        }
        Ok(e)
    }

    /// Saves an episode's `episode.json`.
    pub fn save_episode(&self, episode: &Episode) -> Result<()> {
        let id = seg(&episode.id, "episode")?;
        save_entity(&self.dir.join("episodes").join(id).join("episode.json"), episode)
    }

    /// An episode's `story.json` (scenes, beats, lines).
    pub fn story(&self, ep: &str) -> Result<Story> {
        let mut s: Story = read_json(&self.ep_dir(ep).join("story.json"))?;
        if s.episode.is_empty() {
            s.episode = ep.to_string();
        }
        Ok(s)
    }

    /// Saves an episode's `story.json`.
    pub fn save_story(&self, story: &Story) -> Result<()> {
        let id = seg(&story.episode, "story's episode")?;
        save_entity(&self.dir.join("episodes").join(id).join("story.json"), story)
    }

    /// `episodes/<ep>/shots/<scene>.json`.
    pub fn scene_shots(&self, ep: &str, scene: &str) -> Result<SceneShots> {
        let file = self.ep_dir(ep).join("shots").join(format!("{}.json", mv_core::paths::safe_name(scene)));
        let mut s: SceneShots = read_json(&file)?;
        if s.episode.is_empty() {
            s.episode = ep.to_string();
        }
        if s.scene.is_empty() {
            s.scene = scene.to_string();
        }
        Ok(s)
    }

    /// Saves one scene's shots (`shots/<scene>.json`).
    pub fn save_scene_shots(&self, shots: &SceneShots) -> Result<()> {
        let ep = seg(&shots.episode, "shots' episode")?;
        let scene = seg(&shots.scene, "shots' scene")?;
        save_entity(&self.dir.join("episodes").join(ep).join("shots").join(format!("{scene}.json")), shots)
    }

    /// [`update_file`] for one scene's shots (`shots/<scene>.json`, the file
    /// [`Store::scene_shots`] reads): read fresh, `f` applied, saved, all under the file's own
    /// lock. A scene file that is not there (removed meanwhile) is refused, never made empty.
    pub fn update_scene_shots(&self, ep: &str, scene: &str, f: impl FnOnce(&mut SceneShots)) -> Result<SceneShots> {
        self.update_scene_shots_with(ep, scene, false, f)
    }

    /// [`Store::update_scene_shots`]; `create` makes the file when the scene has none yet (a
    /// scene planned for the first time).
    pub(crate) fn update_scene_shots_with(&self, ep: &str, scene: &str, create: bool, f: impl FnOnce(&mut SceneShots)) -> Result<SceneShots> {
        seg(ep, "shots' episode")?;
        seg(scene, "shots' scene")?;
        let file = self.ep_dir(ep).join("shots").join(format!("{}.json", mv_core::paths::safe_name(scene)));
        if !create && !file.is_file() {
            bail!("The shots of this scene are no longer there; the scene may have been planned again or removed.");
        }
        let (ep, scene) = (ep.to_string(), scene.to_string());
        update_file::<SceneShots>(&file, move |s| {
            if s.episode.is_empty() {
                s.episode = ep;
            }
            if s.scene.is_empty() {
                s.scene = scene;
            }
            f(s)
        })
    }

    /// Every scene's shots of an episode, keyed by scene id.
    pub fn all_shots(&self, ep: &str) -> Result<BTreeMap<String, Vec<Shot>>> {
        let mut out = BTreeMap::new();
        for (stem, file) in json_files(&self.ep_dir(ep).join("shots")) {
            let s: SceneShots = read_json(&file)?;
            let key = if s.scene.is_empty() { stem } else { s.scene };
            out.insert(key, s.shots);
        }
        Ok(out)
    }

    /// `episodes/<ep>/checks.json` (derived, rebuildable).
    pub fn checks(&self, ep: &str) -> Result<Vec<Issue>> {
        read_json(&self.ep_dir(ep).join("checks.json"))
    }

    /// Saves an episode's check results (`checks.json`, no history kept).
    pub fn save_checks(&self, ep: &str, issues: &[Issue]) -> Result<()> {
        let id = seg(ep, "checks' episode")?;
        write_json(&self.dir.join("episodes").join(id).join("checks.json"), &issues)
    }

    /// The whole bible (characters, places, props, style, cast).
    pub fn bible(&self) -> Result<BibleIndex> {
        let bible = self.dir.join("bible");
        Ok(BibleIndex {
            characters: entities::<Character>(&bible.join("characters"), |c, id| {
                if c.id.is_empty() {
                    c.id = id
                }
            })?,
            places: entities::<Place>(&bible.join("places"), |p, id| {
                if p.id.is_empty() {
                    p.id = id
                }
            })?,
            props: entities::<Prop>(&bible.join("props"), |p, id| {
                if p.id.is_empty() {
                    p.id = id
                }
            })?,
            style: read_json(&bible.join("style.json"))?,
            cast: read_json(&bible.join("cast.json"))?,
        })
    }

    /// Saves a character of the bible.
    pub fn save_character(&self, c: &Character) -> Result<()> {
        let id = seg(&c.id, "character")?;
        save_entity(&self.dir.join("bible").join("characters").join(format!("{id}.json")), c)
    }

    /// Saves a place of the bible.
    pub fn save_place(&self, p: &Place) -> Result<()> {
        let id = seg(&p.id, "place")?;
        save_entity(&self.dir.join("bible").join("places").join(format!("{id}.json")), p)
    }

    /// Saves a prop of the bible.
    pub fn save_prop(&self, p: &Prop) -> Result<()> {
        let id = seg(&p.id, "prop")?;
        save_entity(&self.dir.join("bible").join("props").join(format!("{id}.json")), p)
    }

    /// Saves the bible's style.
    pub fn save_style(&self, s: &Style) -> Result<()> {
        save_entity(&self.dir.join("bible").join("style.json"), s)
    }

    /// Saves a bible entity quietly (see [`Quiet`]).
    pub(crate) fn save_quiet(&self, e: Quiet<'_>) -> Result<()> {
        let bible = self.dir.join("bible");
        match e {
            Quiet::Character(c) => save_entity_with(&bible.join("characters").join(format!("{}.json", seg(&c.id, "character")?)), c, true),
            Quiet::Place(p) => save_entity_with(&bible.join("places").join(format!("{}.json", seg(&p.id, "place")?)), p, true),
            Quiet::Prop(p) => save_entity_with(&bible.join("props").join(format!("{}.json", seg(&p.id, "prop")?)), p, true),
            Quiet::Style(s) => save_entity_with(&bible.join("style.json"), s, true),
        }
    }

    /// Saves the bible's voice cast.
    pub fn save_cast(&self, c: &VoiceCast) -> Result<()> {
        save_entity(&self.dir.join("bible").join("cast.json"), c)
    }

    /// Stores an output: id = sha256 of `bytes`; writes `takes/<id[..2]>/<id>.<ext>` only if
    /// absent (identical outputs dedupe), then the `.json` beside it.
    ///
    /// Takes are immutable: when the same bytes were stored before, the take saved then is
    /// returned unchanged (whatever `meta` says now). `created_at` is unix seconds.
    pub fn add_take(&self, bytes: &[u8], ext: &str, meta: TakeMeta) -> Result<Take> {
        if bytes.is_empty() {
            bail!("The new picture or sound is empty, so it was not kept. Try again.");
        }
        let id = crate::hash::sha256_hex(bytes);
        if let Some(existing) = self.take(&id) {
            if self.path(&existing.file).is_file() {
                return Ok(existing);
            }
        }
        let rel = format!("takes/{}/{id}.{}", &id[..2], clean_ext(ext));
        let file = self.path(&rel);
        {
            let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            if !file.is_file() {
                atomic_write(&file, bytes)?;
            }
        }
        self.record_take(id, rel, meta)
    }

    /// [`Store::add_take`] for a big file already on disk (a finished animatic): hashed while
    /// streaming (never read whole into memory) and hard-linked into `takes/`, so the export
    /// and its take share one copy on the drive (copied only where links are not possible).
    pub fn add_take_file(&self, src: &Path, ext: &str, meta: TakeMeta) -> Result<Take> {
        let len = std::fs::metadata(src).map(|m| m.len()).map_err(|e| anyhow!("The finished file could not be read back ({e}). Try again."))?;
        if len == 0 {
            bail!("The new picture or sound is empty, so it was not kept. Try again.");
        }
        let id = crate::hash::file_hash(src).map_err(|_| anyhow!("The finished file could not be read back. Try again."))?;
        if let Some(existing) = self.take(&id) {
            if self.path(&existing.file).is_file() {
                return Ok(existing);
            }
        }
        let rel = format!("takes/{}/{id}.{}", &id[..2], clean_ext(ext));
        let file = self.path(&rel);
        {
            let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            if !file.is_file() {
                refuse_removed(&file)?;
                if let Some(dir) = file.parent() {
                    std::fs::create_dir_all(dir).context("Could not create the folder for the project's takes. Check that the data drive has free space.")?;
                }
                if std::fs::hard_link(src, &file).is_err() {
                    let tmp = sibling(&file, &format!("tmp-{}-{}", std::process::id(), TMP_SEQ.fetch_add(1, Ordering::SeqCst)));
                    let copied = std::fs::copy(src, &tmp).and_then(|_| std::fs::rename(&tmp, &file));
                    if let Err(e) = copied {
                        let _ = std::fs::remove_file(&tmp);
                        bail!("Could not keep the finished file in the project's history: {e}. Check that the data drive has free space.");
                    }
                }
            }
        }
        self.record_take(id, rel, meta)
    }

    /// Writes a new take's record (`takes/<id[..2]>/<id>.json`).
    fn record_take(&self, id: String, rel: String, meta: TakeMeta) -> Result<Take> {
        let take = Take {
            schema: SCHEMA,
            id: id.clone(),
            file: rel,
            kind: meta.kind,
            target: meta.target,
            request: meta.request,
            request_hash: meta.request_hash,
            source: meta.source,
            created_at: now_s(),
            w: meta.w,
            h: meta.h,
            seconds: meta.seconds,
            batch: meta.batch,
            note: meta.note,
        };
        write_json(&self.take_json(&id), &take)?;
        Ok(take)
    }

    fn take_json(&self, id: &str) -> PathBuf {
        self.dir.join("takes").join(&id[..2]).join(format!("{id}.json"))
    }

    /// A take's record by id (`None` when unknown or damaged).
    pub fn take(&self, id: &str) -> Option<Take> {
        if !is_take_id(id) {
            return None;
        }
        let file = self.take_json(id);
        if !file.is_file() {
            return None;
        }
        let mut t: Take = read_json(&file).ok()?;
        if t.id.is_empty() {
            return None;
        }
        t.id = id.to_string();
        Some(t)
    }

    /// Absolute path of a take's file.
    pub fn take_file(&self, id: &str) -> Option<PathBuf> {
        if !is_take_id(id) {
            return None;
        }
        if let Some(t) = self.take(id) {
            let p = self.path(&t.file);
            if p.is_file() {
                return Some(p);
            }
        }
        // The take's record is gone or damaged: the file itself is named by its id.
        std::fs::read_dir(self.dir.join("takes").join(&id[..2])).ok()?.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| {
            p.is_file() && p.file_stem().is_some_and(|s| s.to_string_lossy() == id) && p.extension().is_some_and(|x| x != "json")
        })
    }

    /// Snapshot times (unix ms, newest first) of an entity file.
    pub fn history(&self, rel: &str) -> Vec<i64> {
        match clean_rel(rel) {
            Some(rel) => snapshots(&self.dir, &rel),
            None => Vec::new(),
        }
    }

    /// Snapshots the current file, then writes the snapshot `snapshot_ms` back.
    pub fn restore(&self, rel: &str, snapshot_ms: i64) -> Result<()> {
        let rel = clean_rel(rel).ok_or_else(|| anyhow!("That file is not part of this project."))?;
        let snap = history_dir(&self.dir, &rel).join(format!("{snapshot_ms}.json"));
        let bytes = std::fs::read(&snap).map_err(|_| anyhow!("That earlier version of {rel} is no longer there."))?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| anyhow!("That earlier version of {rel} is damaged and cannot be restored. Pick another one."))?;
        if value.get("schema").and_then(Value::as_u64).is_some_and(|s| s > SCHEMA as u64) {
            bail!("{NEWER} ({rel}).");
        }
        let path = self.path(&rel);
        let lock = file_lock(&path);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        write_bytes_with(&path, &bytes, Some(true))
    }

    /// Reads the project file `rel` fresh, applies `f` and saves it, all under that file's own
    /// lock, so a job and a screen changing the same file never lose each other's change (see
    /// [`update_file`]). Answers the value as saved.
    pub fn update_json<T: DeserializeOwned + Serialize + Default>(&self, rel: &str, f: impl FnOnce(&mut T)) -> Result<T> {
        let rel = clean_rel(rel).ok_or_else(|| anyhow!("That file is not part of this project."))?;
        update_file(&self.path(&rel), f)
    }

    /// [`Store::update_json`] for `project.json`. A project without one (removed, or never
    /// made) is refused: [`Store::save_series`] makes it.
    pub fn update_series(&self, f: impl FnOnce(&mut Series)) -> Result<Series> {
        if !self.dir.join("project.json").is_file() {
            bail!("This project is no longer there. Open it again from the list.");
        }
        let id = self.id.clone();
        self.update_json::<Series>("project.json", move |s| {
            if s.id.is_empty() {
                s.id = id;
            }
            f(s)
        })
    }

    /// [`Store::update_json`] for an episode's `story.json`.
    pub fn update_story(&self, ep: &str, f: impl FnOnce(&mut Story)) -> Result<Story> {
        let id = seg(ep, "story's episode")?;
        let ep = ep.to_string();
        self.update_json::<Story>(&format!("episodes/{id}/story.json"), move |s| {
            if s.episode.is_empty() {
                s.episode = ep;
            }
            f(s)
        })
    }

    /// [`Store::update_json`] for the bible's voice cast (`bible/cast.json`).
    pub fn update_cast(&self, f: impl FnOnce(&mut VoiceCast)) -> Result<VoiceCast> {
        self.update_json::<VoiceCast>("bible/cast.json", f)
    }

    /// Runs `f` while holding this project's one bible lock, so a whole "read the bible, change
    /// something, save the changed entities" sequence never interleaves with another one (a
    /// bible job's card update and the Bible screen's commit).
    ///
    /// Lock order: the bible lock first, then an entity's or the cast's own file lock, then the
    /// global write lock. `f` may save bible files (each save takes its own file lock inside),
    /// but must not wait for anything that itself waits for this lock on another thread. Taken
    /// again on the same thread inside `f`, it does not wait for itself: the inner call just
    /// runs (it is already held).
    pub fn with_bible<R>(&self, f: impl FnOnce() -> R) -> R {
        thread_local! {
            static HELD: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        let key = self.dir.join("bible");
        if HELD.with(|h| h.borrow().contains(&key)) {
            return f();
        }
        let lock = bible_lock(&key);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        HELD.with(|h| h.borrow_mut().push(key.clone()));
        // Taken off the thread's list on every way out, a panic in `f` included.
        struct Release(PathBuf);
        impl Drop for Release {
            fn drop(&mut self) {
                HELD.with(|h| h.borrow_mut().retain(|k| *k != self.0));
            }
        }
        let _release = Release(key);
        f()
    }

    /// Applies `f` to the project's id counters (`Series.next_ids`) under the one counters lock,
    /// inside `project.json`'s own read-change-save (as [`Store::update_series`]), and answers the
    /// counters as saved. Every place that hands out ids goes through it.
    /// Lock order: counters, then `project.json`'s file lock, then the global write lock.
    pub(crate) fn bump_counters(&self, f: impl FnOnce(&mut BTreeMap<String, u32>)) -> Result<BTreeMap<String, u32>> {
        static COUNTERS: Mutex<()> = Mutex::new(());
        let _guard = COUNTERS.lock().unwrap_or_else(|e| e.into_inner());
        // As `update_series`, except that a project with no `project.json` yet gets one (the
        // counters were always saved that way); a removed project is still refused (`CLOSING`).
        let id = self.id.clone();
        let series = self.update_json::<Series>("project.json", move |s| {
            if s.id.is_empty() {
                s.id = id;
            }
            f(&mut s.next_ids)
        })?;
        Ok(series.next_ids)
    }
}

/// The per-project bible lock (see [`Store::with_bible`]). Its own map, so it can never be the
/// same mutex as a file's lock.
fn bible_lock(key: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let mut m = LOCKS.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|e| e.into_inner());
    m.entry(key.to_path_buf()).or_default().clone()
}

/// A moment in the user's own time zone for lists: "21 Nov, 04:10" this year, "21 Nov 2025,
/// 04:10" before. Unix seconds or milliseconds both read right.
pub fn local_when(t: i64) -> String {
    use chrono::{Datelike as _, Local, TimeZone as _};
    let secs = if t > 100_000_000_000 { t / 1000 } else { t };
    let Some(when) = Local.timestamp_opt(secs, 0).single() else { return String::new() };
    if when.year() == Local::now().year() {
        when.format("%-d %b, %H:%M").to_string()
    } else {
        when.format("%-d %b %Y, %H:%M").to_string()
    }
}

/// A small JPEG of the top of a manga page, `w`×`h`, cut from the top at that shape (never
/// stretched; a long webtoon page shows its first screen), made once under
/// `anime/cache/pages/` and reused until the page file changes. Blocking: decode off the UI
/// thread.
pub fn page_thumb(src: &Path, w: u32, h: u32) -> Result<PathBuf> {
    let (w, h) = (w.max(1), h.max(1));
    let meta = std::fs::metadata(src).with_context(|| format!("The page {} is not there.", display_name(src)))?;
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    let key = crate::hash::sha256_hex(format!("{}|{}|{mtime}|{w}x{h}", src.display(), meta.len()).as_bytes());
    let out = root().join("cache").join("pages").join(format!("{}.jpg", &key[..32]));
    if out.is_file() {
        return Ok(out);
    }
    let img = image::ImageReader::open(src)
        .and_then(|r| r.with_guessed_format())
        .map_err(|e| anyhow!("The page {} could not be opened: {e}", display_name(src)))?
        .decode()
        .map_err(|e| anyhow!("The page {} could not be read as a picture: {e}", display_name(src)))?;
    let (iw, ih) = (img.width().max(1), img.height().max(1));
    let top_h = ((iw as u64 * h as u64) / w as u64).clamp(1, ih as u64) as u32;
    let small = img.crop_imm(0, 0, iw, top_h).resize_to_fill(w, h, image::imageops::FilterType::Triangle).into_rgb8();
    let mut buf = Vec::with_capacity(16 << 10);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85).encode_image(&small).map_err(|e| anyhow!("The page preview could not be made: {e}"))?;
    atomic_write(&out, &buf)?;
    Ok(out)
}

/// `yyyymmdd-hhmmss` in the user's own time zone (file names the user sees).
pub fn local_stamp(unix_s: i64) -> String {
    use chrono::{Local, TimeZone as _};
    Local.timestamp_opt(unix_s, 0).single().map(|d| d.format("%Y%m%d-%H%M%S").to_string()).unwrap_or_default()
}

// ---------------------------------------------------------------- Magnific pictures made

/// The plain list of every Magnific picture MangaVox made, so the owner can tell them from the
/// ones they made by hand on the same account: `<data>/anime/magnific-made-by-mangavox.csv`.
pub const MAGNIFIC_LIST: &str = "magnific-made-by-mangavox.csv";

/// The list's header line.
pub const MAGNIFIC_LIST_HEADER: &str = "When,Magnific id,Generate pressed by,Model,Project,For,Prompt (first 80 characters)";

/// A take's note when the person at the Magnific window had pressed Generate (MangaVox found it
/// pressed and took the picture as its own).
pub const PRESSED_AT_WINDOW: &str = "Generate pressed by you at the window";

/// The full path of [`MAGNIFIC_LIST`].
pub fn magnific_list_path() -> PathBuf {
    root().join(MAGNIFIC_LIST)
}

/// What the picture site said about one picture it sent back: its own id (the number in the
/// picture's address on magnific.com) and who pressed Generate.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SiteMark {
    pub site_id: String,
    pub pressed_by_app: bool,
    /// The model that drew it, as the page names it ("" when not known).
    pub model: String,
}

/// Marks waiting for their picture's job to store it, by the picture's sha256.
static MARKS: Mutex<Vec<(String, SiteMark)>> = Mutex::new(Vec::new());

/// The picture backend notes what the site said about a picture it hands to a job (the backend
/// answers only the picture's bytes); the job reads it back with [`take_site_mark`].
pub fn note_site_mark(bytes: &[u8], mark: SiteMark) {
    let key = crate::hash::sha256_hex(bytes);
    let mut m = MARKS.lock().unwrap_or_else(|e| e.into_inner());
    m.retain(|(k, _)| *k != key);
    m.push((key, mark));
    // Marks of pictures no job stored (a cancelled job) do not pile up.
    let over = m.len().saturating_sub(64);
    m.drain(..over);
}

/// The mark noted for these picture bytes, removed once read.
pub fn take_site_mark(bytes: &[u8]) -> Option<SiteMark> {
    let key = crate::hash::sha256_hex(bytes);
    let mut m = MARKS.lock().unwrap_or_else(|e| e.into_inner());
    let i = m.iter().position(|(k, _)| *k == key)?;
    Some(m.remove(i).1)
}

/// One line of the list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MadeLine {
    /// Local date and time, "2026-11-21 14:05:09".
    pub when: String,
    pub site_id: String,
    pub pressed_by_app: bool,
    pub model: String,
    pub project: String,
    /// What it is for: "Bong Ju-hyuk · face sheet", "Frame 1.3".
    pub what: String,
    pub prompt: String,
}

/// One CSV field, quoted when it needs to be.
fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

impl MadeLine {
    /// Who pressed Generate, in words.
    pub fn pressed_by(&self) -> &'static str {
        if self.pressed_by_app {
            "MangaVox"
        } else {
            "you, at the window"
        }
    }

    /// The CSV line (no line end): the prompt on one line, its first 80 characters.
    pub fn csv(&self) -> String {
        let prompt: String = self.prompt.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(80).collect();
        let id = if self.site_id.trim().is_empty() { "unknown" } else { self.site_id.trim() };
        [self.when.as_str(), id, self.pressed_by(), &self.model, &self.project, &self.what, &prompt].iter().map(|f| csv_field(f)).collect::<Vec<_>>().join(",")
    }
}

/// Local date and time now, "2026-11-21 14:05:09".
pub fn local_now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Appends a line to `file`, writing the header first when the file is new or empty.
pub fn append_made_line(file: &Path, line: &MadeLine) -> Result<()> {
    static LIST_LOCK: Mutex<()> = Mutex::new(());
    let _held = LIST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("Could not make the folder for {}.", display_name(file)))?;
    }
    let fresh = std::fs::metadata(file).map(|m| m.len() == 0).unwrap_or(true);
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(file).with_context(|| format!("Could not open {}.", display_name(file)))?;
    let mut text = String::new();
    if fresh {
        text.push_str(MAGNIFIC_LIST_HEADER);
        text.push_str("\r\n");
    }
    text.push_str(&line.csv());
    text.push_str("\r\n");
    f.write_all(text.as_bytes()).with_context(|| format!("Could not write to {}.", display_name(file)))?;
    Ok(())
}

/// Records a Magnific picture MangaVox made: a line in [`magnific_list_path`] and the same line
/// in the app log. Blocking (a small file append): call it from a job, never the UI thread. A
/// failure is logged, never stops the picture.
pub fn record_magnific_made(line: &MadeLine) {
    let log = mv_core::logging::global();
    log.info("anime", &format!("Magnific picture made: {}", line.csv()));
    if let Err(e) = append_made_line(&magnific_list_path(), line) {
        log.warn("anime", &format!("could not add to the list of Magnific pictures: {e:#}"));
    }
}

/// What a job storing a picture from the picture backend keeps of the site's mark: the mark
/// noted for these bytes, recorded in the list (and the log) as made for `what` with `prompt`.
/// `None` when the backend noted nothing (a picture that did not come from Magnific's website).
pub fn magnific_picture_made(store: &Store, bytes: &[u8], model: &str, what: &str, prompt: &str) -> Option<SiteMark> {
    let mark = take_site_mark(bytes)?;
    let project = store.series().ok().map(|s| if s.manga_title.trim().is_empty() { s.id } else { s.manga_title.trim().to_string() }).unwrap_or_else(|| store.id.clone());
    record_magnific_made(&MadeLine {
        when: local_now(),
        site_id: mark.site_id.clone(),
        pressed_by_app: mark.pressed_by_app,
        model: if mark.model.trim().is_empty() { model.to_string() } else { mark.model.clone() },
        project,
        what: what.to_string(),
        prompt: prompt.to_string(),
    });
    Some(mark)
}

/// `*.json` files of a folder as (stem, path), sorted by name; temp and set-aside files skipped.
fn json_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut v: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "json"))
                .filter_map(|p| {
                    let stem = p.file_stem()?.to_string_lossy().into_owned();
                    (!stem.contains('.')).then_some((stem, p))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn entities<T: DeserializeOwned + Default>(dir: &Path, fix: impl Fn(&mut T, String)) -> Result<Vec<T>> {
    json_files(dir)
        .into_iter()
        .map(|(stem, file)| {
            let mut v: T = read_json(&file)?;
            fix(&mut v, stem);
            Ok(v)
        })
        .collect()
}

/// A take id: 64 lower-case hex digits (never a path).
fn is_take_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A file extension safe for a take: lower-case letters and digits, `bin` when none.
fn clean_ext(ext: &str) -> String {
    let e: String = ext.trim().trim_start_matches('.').chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect::<String>().to_ascii_lowercase();
    if e.is_empty() {
        "bin".into()
    } else {
        e
    }
}

/// A project-relative path with forward slashes and no way out of the project.
fn clean_rel(rel: &str) -> Option<String> {
    let rel = rel.replace('\\', "/");
    let parts: Vec<&str> = rel.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    if parts.is_empty() || parts.iter().any(|p| *p == ".." || p.contains(':')) || rel.starts_with('/') {
        return None;
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn made(prompt: &str, by_app: bool) -> MadeLine {
        MadeLine {
            when: "2026-11-21 14:05:09".into(),
            site_id: "123456".into(),
            pressed_by_app: by_app,
            model: "Google Nano Banana 2.1".into(),
            project: "Solo, \"Max\" Level".into(),
            what: "Bong Ju-hyuk \u{b7} face sheet".into(),
            prompt: prompt.into(),
        }
    }

    #[test]
    fn a_magnific_list_line_names_the_id_and_who_pressed_generate() {
        let long = format!("Character sheet,\nfront view {}", "x".repeat(200));
        let line = made(&long, true).csv();
        assert_eq!(
            line,
            format!("2026-11-21 14:05:09,123456,MangaVox,Google Nano Banana 2.1,\"Solo, \"\"Max\"\" Level\",Bong Ju-hyuk \u{b7} face sheet,\"Character sheet, front view {}\"", "x".repeat(52))
        );
        // A comma inside a field is quoted.
        assert!(made("Frame", false).csv().contains(",\"you, at the window\","));
        assert!(MadeLine { site_id: String::new(), ..made("p", true) }.csv().contains(",unknown,"));
    }

    #[test]
    fn the_magnific_list_gets_its_header_once_and_only_grows() {
        let dir = tempfile::tempdir().expect("temp");
        let file = dir.path().join("anime").join(MAGNIFIC_LIST);
        append_made_line(&file, &made("one", true)).expect("first");
        append_made_line(&file, &made("two", false)).expect("second");
        let text = std::fs::read_to_string(&file).expect("read");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], MAGNIFIC_LIST_HEADER);
        assert!(lines[1].ends_with(",one") && lines[2].ends_with(",two"));
    }

    #[test]
    fn a_site_mark_is_read_back_once_for_its_picture() {
        let bytes = b"picture bytes for the mark test";
        note_site_mark(bytes, SiteMark { site_id: "77".into(), pressed_by_app: false, model: String::new() });
        assert_eq!(take_site_mark(b"other bytes"), None);
        assert_eq!(take_site_mark(bytes).map(|m| (m.site_id, m.pressed_by_app)), Some(("77".into(), false)));
        assert_eq!(take_site_mark(bytes), None);
    }

    #[test]
    fn project_and_rel_from_a_path() {
        let p = Path::new("x").join("anime").join("projects").join("m1").join("episodes").join("ep01").join("story.json");
        let (proj, rel) = project_of(&p).unwrap_or_default();
        assert_eq!(proj, Path::new("x").join("anime").join("projects").join("m1"));
        assert_eq!(rel, "episodes/ep01/story.json");
        assert!(project_of(Path::new("x/anime/settings.json")).is_none());
        assert!(keeps_history("episodes/ep01/story.json"));
        assert!(!keeps_history("episodes/ep01/checks.json"));
        assert!(!keeps_history("takes/ab/abc.json"));
    }

    #[test]
    fn rel_paths_never_leave_the_project() {
        assert_eq!(clean_rel("episodes/ep01/story.json").as_deref(), Some("episodes/ep01/story.json"));
        assert_eq!(clean_rel("bible\\style.json").as_deref(), Some("bible/style.json"));
        assert!(clean_rel("../x.json").is_none());
        assert!(clean_rel("/abs.json").is_none());
        assert!(clean_rel("C:/x.json").is_none());
        assert_eq!(clean_ext(".PNG"), "png");
        assert_eq!(clean_ext("../"), "bin");
        assert!(!is_take_id("../../etc"));
    }

    #[test]
    fn a_removed_project_is_never_made_again() {
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let proj = tmp.path().join("anime").join("projects").join("gone");
        let file = proj.join("work").join("voice").join("a.wav");
        set_closing(&proj, true);
        assert_eq!(atomic_write(&file, b"x").map_err(|e| e.to_string()), Err(REMOVED.to_string()));
        assert!(make_dir(&proj.join("work").join("cut")).is_err());
        assert!(!proj.exists(), "a job winding down never makes the folder again");
        // Still there (not yet moved): the job may finish writing into it.
        std::fs::create_dir_all(&proj).unwrap_or_else(|e| panic!("{e}"));
        assert!(atomic_write(&file, b"x").is_ok());
        std::fs::remove_dir_all(&proj).unwrap_or_else(|e| panic!("{e}"));
        set_closing(&proj, false);
        assert!(atomic_write(&file, b"x").is_ok(), "a new project of the same id is made as usual");
    }

    #[test]
    fn null_elements_read_as_zero() {
        let mut v = serde_json::json!({"pos": [null, 1.0], "gone": null, "deep": [{"a": [null]}]});
        drop_nulls(&mut v);
        assert_eq!(v, serde_json::json!({"pos": [0, 1.0], "deep": [{"a": [0]}]}));
    }
}
