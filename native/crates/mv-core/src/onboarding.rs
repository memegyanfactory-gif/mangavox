//! First-run setup state: whether this start shows the welcome setup, and what the user chose
//! in it. Kept in `config.json` under `onboarding`:
//!
//! ```json
//! { "version": 1, "status": "in_progress", "step": "manga", "uses": ["manga"],
//!   "steps": { "manga": "configured" }, "seeded": true, "startedAt": "..." }
//! ```
//!
//! The decision is made once per process, right after [`crate::paths::prepare`] and before
//! anything else creates files in the data folder ([`init`]), so files made later in the same
//! run cannot change the answer. Rules, in order:
//!
//! 1. `MANGAVOX_WELCOME=0` never shows it; `MANGAVOX_WELCOME=1` or `--welcome` always does.
//! 2. `onboarding.status` `done` or `skipped` → [`Launch::Done`]; `in_progress` → [`Launch::Resume`].
//! 3. No state but the folder already holds MangaVox data (database, settings, plugins, addons,
//!    voice models, library files, repositories) → [`Launch::Existing`], recorded as done so an
//!    upgrade never sees the setup. Otherwise [`Launch::Fresh`].
//!
//! Re-running the setup from Settings changes nothing here until it finishes.
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::Config;

pub const KEY: &str = "onboarding";
pub const VERSION: u32 = 1;
/// Env override: `1` shows the setup at every start, `0` never shows it.
pub const ENV: &str = "MANGAVOX_WELCOME";
/// Command-line flag that shows the setup at this start.
pub const FLAG: &str = "--welcome";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    InProgress,
    Done,
    Skipped,
}

/// How one setup screen ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepResult {
    /// Something was set up.
    Configured,
    /// Passed over with "Skip this step".
    Skipped,
    /// Started but still running (a voice download), or left for Settings.
    Later,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    #[serde(default)]
    pub version: u32,
    pub status: Status,
    /// The screen to resume at (`welcome`, `uses`, `manga`, `watch`, `voices`, `done`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub step: String,
    /// What the user said they will do: `manga`, `watch`, `studio`.
    #[serde(default)]
    pub uses: Vec<String>,
    #[serde(default)]
    pub steps: BTreeMap<String, StepResult>,
    /// The clean start (no source switched on) was written for this setup.
    #[serde(default)]
    pub seeded: bool,
    /// Recorded automatically for a data folder that already had data.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub existing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

impl State {
    fn new(status: Status) -> Self {
        State { version: VERSION, status, step: String::new(), uses: vec![], steps: BTreeMap::new(), seeded: false, existing: false, started_at: None, completed_at: None }
    }

    pub fn finished(&self) -> bool {
        matches!(self.status, Status::Done | Status::Skipped)
    }

    pub fn uses(&self, what: &str) -> bool {
        self.uses.iter().any(|u| u == what)
    }
}

/// What this start found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launch {
    /// A brand-new data folder.
    Fresh,
    /// The setup was left part-way; it resumes at `step`.
    Resume { step: String },
    /// The setup was finished or skipped before.
    Done,
    /// The folder had data from before the setup existed (or the setup is switched off).
    Existing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Override {
    None,
    /// `MANGAVOX_WELCOME=1` or `--welcome`.
    Force,
    /// `MANGAVOX_WELCOME=0`.
    Suppress,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detection {
    pub launch: Launch,
    /// Shown because of `MANGAVOX_WELCOME=1` / `--welcome`, whatever the folder holds.
    pub forced: bool,
}

impl Detection {
    /// Whether the app opens on the welcome setup.
    pub fn shows_welcome(&self) -> bool {
        self.forced || matches!(self.launch, Launch::Fresh | Launch::Resume { .. })
    }
}

/// Reads the override from the environment value and the command line.
pub fn override_from(env: Option<&str>, args: &[String]) -> Override {
    match env.map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) if matches!(v.as_str(), "0" | "false" | "no" | "off") => Override::Suppress,
        Some(v) if matches!(v.as_str(), "1" | "true" | "yes" | "on") => Override::Force,
        _ if args.iter().any(|a| a == FLAG) => Override::Force,
        _ => Override::None,
    }
}

fn config(root: &Path) -> Config {
    Config::at(root.join("config.json"))
}

fn now() -> String {
    chrono::Local::now().to_rfc3339()
}

/// The stored state, `None` when there is none (or it is unreadable).
pub fn load(root: &Path) -> Option<State> {
    config(root).get(KEY).and_then(|v| serde_json::from_value(v).ok())
}

pub fn save(root: &Path, state: &State) -> Result<()> {
    config(root).set_raw(KEY, serde_json::to_value(state)?)
}

fn non_empty(dir: &Path) -> bool {
    std::fs::read_dir(dir).map(|mut r| r.next().is_some()).unwrap_or(false)
}

/// Any regular file anywhere under `dir`.
fn has_files(dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else { return false };
    rd.flatten().any(|e| {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => has_files(&p),
            Ok(_) => true,
            Err(_) => false,
        }
    })
}

/// True when `root` already holds MangaVox data of any kind. `temp/`, `logs/` and `webview/`
/// do not count: the app creates those itself at every start.
pub fn has_existing_data(root: &Path) -> bool {
    crate::paths::has_data(root)
        || root.join("repos.json").is_file()
        || ["plugins", "addons", "studio", "assistant"].iter().any(|d| non_empty(&root.join(d)))
        || ["models", "tts-venv", "engines"].iter().any(|d| root.join(d).exists())
        || has_files(&root.join("library"))
}

/// Decides what this start shows for the data folder `root`. Records an existing folder as
/// done (so the next start does not look again); writes nothing otherwise.
pub fn detect_in(root: &Path, ov: Override) -> Detection {
    let forced = ov == Override::Force;
    if ov == Override::Suppress {
        return Detection { launch: Launch::Existing, forced: false };
    }
    let launch = match load(root) {
        Some(s) if s.finished() => Launch::Done,
        Some(s) => Launch::Resume { step: s.step },
        None if has_existing_data(root) => {
            let mut s = State::new(Status::Done);
            s.existing = true;
            if let Err(e) = save(root, &s) {
                eprintln!("[onboarding] could not record the existing data folder: {e:#}");
            }
            Launch::Existing
        }
        None => Launch::Fresh,
    };
    Detection { launch, forced }
}

static DETECTED: OnceLock<Detection> = OnceLock::new();

/// Runs [`detect_in`] once for the data folder in use, reading `MANGAVOX_WELCOME` and the
/// command line. Call it from `main` right after `paths::prepare()`.
pub fn init() -> &'static Detection {
    DETECTED.get_or_init(|| {
        let args: Vec<String> = std::env::args().collect();
        let env = std::env::var(ENV).ok();
        detect_in(&crate::paths::root(), override_from(env.as_deref(), &args))
    })
}

/// What [`init`] found at this start (`None` before it ran, e.g. in tests and tools).
pub fn launch() -> Option<&'static Detection> {
    DETECTED.get()
}

/// Starts (or continues) a setup: the stored state, created as `in_progress` when missing.
/// A finished state is returned as it is (Settings' "Run welcome setup again" changes nothing
/// until the new run finishes).
pub fn begin(root: &Path) -> Result<State> {
    if let Some(s) = load(root) {
        return Ok(s);
    }
    let mut s = State::new(Status::InProgress);
    s.started_at = Some(now());
    save(root, &s)?;
    Ok(s)
}

/// Remembers where the setup is (for a resume) and, optionally, how a screen ended. Does
/// nothing to a finished setup.
pub fn mark_step(root: &Path, at: &str, result: Option<(&str, StepResult)>) -> Result<()> {
    let Some(mut s) = load(root).filter(|s| !s.finished()) else { return Ok(()) };
    s.step = at.to_string();
    if let Some((step, r)) = result {
        s.steps.insert(step.to_string(), r);
    }
    save(root, &s)
}

/// Updates an unfinished setup in place.
pub fn update(root: &Path, f: impl FnOnce(&mut State)) -> Result<()> {
    let Some(mut s) = load(root).filter(|s| !s.finished()) else { return Ok(()) };
    f(&mut s);
    save(root, &s)
}

fn finish(root: &Path, status: Status, uses: &[String], steps: BTreeMap<String, StepResult>) -> Result<()> {
    let mut s = load(root).unwrap_or_else(|| State::new(status));
    s.version = VERSION;
    s.status = status;
    s.step = String::new();
    s.uses = uses.to_vec();
    s.steps.extend(steps);
    s.existing = false;
    s.completed_at = Some(now());
    save(root, &s)
}

/// The setup finished with `uses` and the screens' results.
pub fn complete(root: &Path, uses: &[String], steps: BTreeMap<String, StepResult>) -> Result<()> {
    finish(root, Status::Done, uses, steps)
}

/// "Start empty": nothing chosen, nothing set up.
pub fn skip(root: &Path) -> Result<()> {
    finish(root, Status::Skipped, &[], BTreeMap::new())
}

/// Forgets the setup, so the next start decides again (tests and support).
pub fn reset(root: &Path) -> Result<()> {
    config(root).remove(KEY)
}

/// Whether the Anime and Movies catalogs should be warmed up at start: always, unless a
/// finished setup recorded that neither is wanted.
pub fn wants_media(root: &Path) -> bool {
    match load(root) {
        Some(s) if s.finished() && !s.existing => s.uses("watch"),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_empty_folder_is_fresh_and_nothing_is_written() {
        let d = tempfile::tempdir().unwrap();
        // What the app makes itself at every start does not count.
        for dir in ["temp", "logs", "webview"] {
            std::fs::create_dir_all(d.path().join(dir)).unwrap();
            std::fs::write(d.path().join(dir).join("x"), "1").unwrap();
        }
        std::fs::create_dir_all(d.path().join("library").join("manga")).unwrap();
        let det = detect_in(d.path(), Override::None);
        assert_eq!(det, Detection { launch: Launch::Fresh, forced: false });
        assert!(det.shows_welcome());
        assert!(!d.path().join("config.json").exists());
    }

    #[test]
    fn existing_data_skips_the_setup_and_is_remembered() {
        let makers: [fn(&Path); 8] = [
            |p: &Path| std::fs::write(p.join("mangavox.db"), "x").unwrap(),
            |p: &Path| std::fs::write(p.join("config.json"), r#"{"theme":"dark"}"#).unwrap(),
            |p: &Path| std::fs::create_dir_all(p.join("plugins").join("my-plugin")).unwrap(),
            |p: &Path| std::fs::create_dir_all(p.join("addons").join("torrentio")).unwrap(),
            |p: &Path| std::fs::create_dir_all(p.join("models").join("kokoro")).unwrap(),
            |p: &Path| std::fs::create_dir_all(p.join("tts-venv")).unwrap(),
            |p: &Path| std::fs::write(p.join("repos.json"), r#"{"repos":[]}"#).unwrap(),
            |p: &Path| {
                std::fs::create_dir_all(p.join("library").join("manga").join("m1")).unwrap();
                std::fs::write(p.join("library").join("manga").join("m1").join("0001.jpg"), "x").unwrap();
            },
        ];
        for make in makers {
            let d = tempfile::tempdir().unwrap();
            make(d.path());
            let det = detect_in(d.path(), Override::None);
            assert_eq!(det.launch, Launch::Existing);
            assert!(!det.shows_welcome());
            let s = load(d.path()).expect("recorded");
            assert!(s.existing && s.status == Status::Done);
            // Next start: done, and unknown keys in config.json are kept.
            assert_eq!(detect_in(d.path(), Override::None).launch, Launch::Done);
            assert!(wants_media(d.path()));
        }
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("config.json"), r#"{"theme":"dark"}"#).unwrap();
        detect_in(d.path(), Override::None);
        assert_eq!(Config::at(d.path().join("config.json")).get("theme"), Some(serde_json::json!("dark")));
    }

    #[test]
    fn a_setup_left_part_way_resumes_at_its_step() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(detect_in(d.path(), Override::None).launch, Launch::Fresh);
        let s = begin(d.path()).unwrap();
        assert_eq!(s.status, Status::InProgress);
        assert!(s.started_at.is_some());
        // Starting again keeps the same state.
        assert_eq!(begin(d.path()).unwrap(), s);
        mark_step(d.path(), "manga", Some(("uses", StepResult::Configured))).unwrap();
        update(d.path(), |s| s.uses = vec!["manga".into()]).unwrap();
        let det = detect_in(d.path(), Override::None);
        assert_eq!(det.launch, Launch::Resume { step: "manga".into() });
        assert!(det.shows_welcome());
        let s = load(d.path()).unwrap();
        assert_eq!(s.steps.get("uses"), Some(&StepResult::Configured));
        assert!(s.uses("manga"));
    }

    #[test]
    fn finishing_or_starting_empty_ends_it() {
        let d = tempfile::tempdir().unwrap();
        begin(d.path()).unwrap();
        let mut steps = BTreeMap::new();
        steps.insert("voices".to_string(), StepResult::Later);
        complete(d.path(), &["manga".into(), "studio".into()], steps).unwrap();
        let s = load(d.path()).unwrap();
        assert_eq!(s.status, Status::Done);
        assert_eq!(s.steps.get("voices"), Some(&StepResult::Later));
        assert!(s.completed_at.is_some() && s.step.is_empty());
        assert_eq!(detect_in(d.path(), Override::None).launch, Launch::Done);
        assert!(!wants_media(d.path()));
        // A finished setup is not changed by steps of a later run.
        mark_step(d.path(), "watch", Some(("watch", StepResult::Skipped))).unwrap();
        assert_eq!(load(d.path()).unwrap(), s);

        let e = tempfile::tempdir().unwrap();
        begin(e.path()).unwrap();
        skip(e.path()).unwrap();
        let s = load(e.path()).unwrap();
        assert_eq!(s.status, Status::Skipped);
        assert!(s.uses.is_empty());
        assert!(!wants_media(e.path()));
        assert!(!detect_in(e.path(), Override::None).shows_welcome());
        reset(e.path()).unwrap();
        assert!(load(e.path()).is_none());
    }

    #[test]
    fn overrides_force_or_hide_it() {
        assert_eq!(override_from(None, &args(&["mangavox.exe"])), Override::None);
        assert_eq!(override_from(None, &args(&["mangavox.exe", "--welcome"])), Override::Force);
        assert_eq!(override_from(Some("1"), &[]), Override::Force);
        assert_eq!(override_from(Some(" TRUE "), &[]), Override::Force);
        assert_eq!(override_from(Some("0"), &args(&["--welcome"])), Override::Suppress);
        assert_eq!(override_from(Some(""), &args(&["--welcome"])), Override::Force);

        // Forced on a folder with data: shown, and the folder is still recorded as existing.
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("mangavox.db"), "x").unwrap();
        let det = detect_in(d.path(), Override::Force);
        assert!(det.forced && det.shows_welcome());
        assert_eq!(det.launch, Launch::Existing);
        let det = detect_in(d.path(), Override::Force);
        assert_eq!(det.launch, Launch::Done);
        assert!(det.shows_welcome());

        // Suppressed on a fresh folder: not shown and nothing written.
        let e = tempfile::tempdir().unwrap();
        let det = detect_in(e.path(), Override::Suppress);
        assert!(!det.shows_welcome());
        assert!(!e.path().join("config.json").exists());
    }

    #[test]
    fn state_round_trips_with_readable_keys() {
        let d = tempfile::tempdir().unwrap();
        begin(d.path()).unwrap();
        update(d.path(), |s| {
            s.seeded = true;
            s.uses = vec!["watch".into()];
        })
        .unwrap();
        let raw = std::fs::read_to_string(d.path().join("config.json")).unwrap();
        assert!(raw.contains("\"in_progress\"") && raw.contains("\"startedAt\"") && raw.contains("\"seeded\": true"));
        assert!(!raw.contains("existing") && !raw.contains("completedAt"));
        // A corrupt value counts as no state.
        Config::at(d.path().join("config.json")).set(KEY, serde_json::json!("broken")).unwrap();
        assert!(load(d.path()).is_none());
    }
}
