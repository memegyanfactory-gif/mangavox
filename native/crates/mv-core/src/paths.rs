//! Where MangaVox keeps its data, and the layout inside it.
//!
//! The data folder is `~/.mangavox` unless `MANGAVOX_HOME` is set or the user picked another one
//! in Settings → Storage (remembered in a small pointer file that never moves with the data).
//!
//! ```text
//! <root>/
//!   config.json, mangavox.db              settings and the library database
//!   library/manga/<manga>/<chapter>/      offline manga chapters
//!   library/anime, library/movies,
//!   library/subtitles                     default video / subtitle download folders
//!   studio/prefs.json                     Studio settings
//!   studio/<manga>/                       a manga's character brain (faces, notes)
//!   studio/projects/<manga>/<chapter>/    a chapter's Studio work: read, script, audio, video
//!   models/, voice-samples/, tts-venv/,
//!   engines/, uv-python/, uv-cache/       local AI models, voice previews, the voice engines
//!   assistant/, assistant-workspace/      AI chats
//!   addons/, plugins/                     installed extensions
//!   cache/                                covers, pages, artwork, torrents (safe to clear)
//!   temp/                                 scratch space, emptied at every start
//!   logs/, webview/
//! ```
//!
//! Work that touches files other parts of the app keep open (moving the folder, resets) is
//! queued in the pointer file and done by [`prepare`] at the next start, before anything opens.
use std::path::{Path, PathBuf, MAIN_SEPARATOR};
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// `~/.mangavox`.
pub fn default_root() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".mangavox")
}

/// The data folder in use (fixed for the life of the process).
pub fn root() -> PathBuf {
    ROOT.get_or_init(|| resolve(&Location::load())).clone()
}

fn resolve(loc: &Location) -> PathBuf {
    if let Some(p) = std::env::var_os("MANGAVOX_HOME").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    loc.root.clone().unwrap_or_else(default_root)
}

pub fn database() -> PathBuf {
    root().join("mangavox.db")
}

pub fn config() -> PathBuf {
    root().join("config.json")
}

pub fn cache() -> PathBuf {
    root().join("cache")
}

pub fn covers() -> PathBuf {
    cache().join("covers")
}

/// Scratch space; emptied by [`prepare`] at every start.
pub fn temp() -> PathBuf {
    root().join("temp")
}

pub fn library() -> PathBuf {
    root().join("library")
}

/// Offline manga chapters: `library/manga/<manga>/<chapter>/`.
pub fn manga_downloads() -> PathBuf {
    library().join("manga")
}

/// `studio/`: Studio prefs plus one brain folder per manga.
pub fn studio() -> PathBuf {
    root().join("studio")
}

/// A manga's Studio folder (its character brain).
pub fn studio_manga(manga_id: &str) -> PathBuf {
    studio().join(safe_name(manga_id))
}

/// Every chapter's Studio work, by manga.
pub fn studio_projects() -> PathBuf {
    studio().join("projects")
}

/// One chapter's Studio work (AI read, transcript, story, narration, video).
pub fn studio_chapter(manga_id: &str, chapter_id: &str) -> PathBuf {
    studio_projects().join(safe_name(manga_id)).join(safe_name(chapter_id))
}

/// A file-name-safe form of an id (same rule Studio has always used for brain folders).
pub fn safe_name(id: &str) -> String {
    let s: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(80).collect();
    if s.is_empty() {
        "item".into()
    } else {
        s
    }
}

// ── the pointer file ────────────────────────────────────────────────────────────────────────────

/// What lives outside the data folder: where it is, plus work queued for the next start.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Location {
    /// Chosen data folder; `None` = [`default_root`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
    /// Move the data from `from` to `to` at the next start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_move: Option<PendingMove>,
    /// Parts to wipe at the next start.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reset: Vec<Part>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingMove {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// `%APPDATA%\MangaVox\location.json` (stays put when the data moves).
pub fn location_file() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("MangaVox").join("location.json")
}

impl Location {
    pub fn load() -> Location {
        Self::load_from(&location_file())
    }

    fn load_from(path: &Path) -> Location {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&location_file())
    }

    fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path).with_context(|| format!("write {}", path.display()))
    }
}

// ── parts that can be reset ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    /// Covers, page images, artwork, torrent pieces. Rebuilt on demand.
    Cache,
    /// Scratch files.
    Temp,
    /// Offline manga chapters (the library keeps the titles; chapters go back to online).
    MangaDownloads,
    /// Anime / movie / subtitle files in the default download folders.
    VideoDownloads,
    /// Studio reads, scripts, narration, videos and character brains (Studio settings stay).
    Studio,
    /// AI chats.
    Chats,
    /// Local AI models, voice previews and the voice engines (tts-venv, engines/, and the
    /// Pythons and package cache uv downloaded for them).
    Models,
    /// Library, reading and watch history (the database), with its downloads and Studio work.
    Library,
    /// Settings back to defaults.
    Settings,
    /// Everything: MangaVox starts like a fresh install.
    Everything,
}

impl Part {
    /// Folders (relative to the data root) this part removes.
    fn dirs(self) -> &'static [&'static str] {
        match self {
            Part::Cache => &["cache"],
            Part::Temp => &["temp"],
            Part::MangaDownloads => &["library/manga", "downloads"],
            Part::VideoDownloads => &["library/anime", "library/movies", "library/subtitles"],
            Part::Studio => &[],
            Part::Chats => &["assistant", "assistant-workspace"],
            Part::Models => &["models", "voice-samples", "tts-venv", "engines", "uv-python", "uv-cache"],
            Part::Library => &["library/manga", "downloads", "studio/projects"],
            Part::Settings => &[],
            Part::Everything => &[],
        }
    }

    /// Bytes this part takes up now (for Settings).
    pub fn size(self) -> u64 {
        let r = root();
        match self {
            Part::Studio => studio_work_entries(&r).iter().map(|p| dir_size(p)).sum(),
            Part::Library => self.dirs().iter().map(|d| dir_size(&r.join(d))).sum::<u64>() + db_files(&r).iter().map(|p| dir_size(p)).sum::<u64>(),
            Part::Settings => dir_size(&r.join("config.json")) + dir_size(&r.join("studio").join("prefs.json")),
            Part::Everything => dir_size(&r),
            p => p.dirs().iter().map(|d| dir_size(&r.join(d))).sum(),
        }
    }

    /// Can be wiped while the app runs (nothing keeps these files open).
    pub fn live(self) -> bool {
        matches!(self, Part::Cache | Part::Temp | Part::VideoDownloads)
    }
}

fn db_files(root: &Path) -> Vec<PathBuf> {
    ["mangavox.db", "mangavox.db-wal", "mangavox.db-shm"].iter().map(|f| root.join(f)).collect()
}

/// Everything in `studio/` except its settings file.
fn studio_work_entries(root: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(root.join("studio")) else { return vec![] };
    rd.flatten().map(|e| e.path()).filter(|p| p.file_name().is_none_or(|n| n != "prefs.json")).collect()
}

/// File or folder size in bytes (0 when missing).
pub fn dir_size(path: &Path) -> u64 {
    let Ok(md) = std::fs::symlink_metadata(path) else { return 0 };
    if !md.is_dir() {
        return md.len();
    }
    std::fs::read_dir(path).map(|rd| rd.flatten().map(|e| dir_size(&e.path())).sum()).unwrap_or(0)
}

fn remove(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(md) if md.is_dir() => std::fs::remove_dir_all(path).with_context(|| format!("remove {}", path.display())),
        Ok(_) => std::fs::remove_file(path).with_context(|| format!("remove {}", path.display())),
        Err(_) => Ok(()),
    }
}

/// Wipe `part` under `root` now. Callers make sure nothing has the files open (see [`Part::live`]).
pub fn wipe(root: &Path, part: Part) -> Result<()> {
    match part {
        Part::Everything => {
            if let Ok(rd) = std::fs::read_dir(root) {
                for e in rd.flatten() {
                    remove(&e.path())?;
                }
            }
        }
        Part::Studio => {
            for p in studio_work_entries(root) {
                remove(&p)?;
            }
        }
        Part::Settings => {
            remove(&root.join("config.json"))?;
            remove(&root.join("studio").join("prefs.json"))?;
        }
        Part::Library => {
            for d in part.dirs() {
                remove(&root.join(d))?;
            }
            for f in db_files(root) {
                remove(&f)?;
            }
        }
        Part::MangaDownloads => {
            for d in part.dirs() {
                remove(&root.join(d))?;
            }
            forget_downloads(&root.join("mangavox.db"))?;
        }
        p => {
            for d in p.dirs() {
                remove(&root.join(d))?;
            }
        }
    }
    Ok(())
}

/// Chapters whose files were deleted go back to online-only.
fn forget_downloads(db: &Path) -> Result<()> {
    if !db.is_file() {
        return Ok(());
    }
    let conn = rusqlite::Connection::open(db)?;
    conn.execute("UPDATE chapter SET download_status = 'none', pages_dir = NULL WHERE download_status != 'none' OR pages_dir IS NOT NULL", [])?;
    Ok(())
}

/// Queue `parts` for the next start (see [`prepare`]).
pub fn schedule_reset(parts: &[Part]) -> Result<()> {
    let mut loc = Location::load();
    for p in parts {
        if !loc.reset.contains(p) {
            loc.reset.push(*p);
        }
    }
    loc.save()
}

// ── moving the data folder ──────────────────────────────────────────────────────────────────────

/// Checks a folder the user picked as the new data folder.
pub fn check_new_root(to: &Path) -> Result<()> {
    let from = root();
    if !to.is_absolute() {
        bail!("Pick a full folder path.");
    }
    let (f, t) = (norm(&from), norm(to));
    if f == t {
        bail!("That is already the data folder.");
    }
    if t.starts_with(&f) || f.starts_with(&t) {
        bail!("The new folder can't be inside the current data folder, or contain it.");
    }
    Ok(())
}

fn norm(p: &Path) -> PathBuf {
    let s = p.to_string_lossy().trim_end_matches(['\\', '/']).to_string();
    PathBuf::from(if cfg!(windows) { s.to_lowercase() } else { s })
}

/// True when `dir` already holds MangaVox data.
pub fn has_data(dir: &Path) -> bool {
    dir.join("mangavox.db").is_file() || dir.join("config.json").is_file()
}

/// Use `to` from the next start. With `move_data` the current data is moved there then.
pub fn choose_root(to: &Path, move_data: bool) -> Result<()> {
    check_new_root(to)?;
    std::fs::create_dir_all(to).with_context(|| format!("create {}", to.display()))?;
    let mut loc = Location::load();
    loc.pending_move = move_data.then(|| PendingMove { from: root(), to: to.to_path_buf() });
    if !move_data {
        loc.root = Some(to.to_path_buf());
    }
    loc.save()
}

/// Copies `from` into `to`, skipping files already there with the same size and time, and
/// reporting `(bytes done, bytes total)`. Files that can't be read are skipped (the copy at the
/// next start picks them up). Run it before restarting so the move at start is quick.
pub fn precopy(from: &Path, to: &Path, progress: &dyn Fn(u64, u64)) -> Result<()> {
    let total = dir_size(from);
    let mut done = 0;
    copy_tree(from, to, &mut |n| {
        done += n;
        progress(done, total);
    }, true)
}

fn same_file(a: &std::fs::Metadata, b: &Path) -> bool {
    let Ok(m) = std::fs::metadata(b) else { return false };
    m.len() == a.len() && m.modified().ok() == a.modified().ok()
}

fn copy_tree(from: &Path, to: &Path, on_bytes: &mut dyn FnMut(u64), lenient: bool) -> Result<()> {
    std::fs::create_dir_all(to).with_context(|| format!("create {}", to.display()))?;
    for e in std::fs::read_dir(from).with_context(|| format!("read {}", from.display()))?.flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let Ok(md) = e.metadata() else { continue };
        if md.is_dir() {
            copy_tree(&src, &dst, on_bytes, lenient)?;
            continue;
        }
        if !same_file(&md, &dst) {
            match std::fs::copy(&src, &dst) {
                Ok(_) => {
                    if let Ok(t) = md.modified() {
                        let _ = std::fs::File::options().write(true).open(&dst).and_then(|f| f.set_modified(t));
                    }
                }
                Err(_) if lenient => {}
                Err(e) => return Err(e).with_context(|| format!("copy {}", src.display())),
            }
        }
        on_bytes(md.len());
    }
    Ok(())
}

/// Moves a folder: a rename when it can (into a missing or empty target), else copy then delete.
pub fn move_dir(from: &Path, to: &Path) -> Result<()> {
    if !from.exists() {
        return Ok(());
    }
    let empty = |d: &Path| std::fs::read_dir(d).map(|mut r| r.next().is_none()).unwrap_or(true);
    if empty(to) {
        let _ = std::fs::remove_dir(to);
        if let Some(p) = to.parent() {
            std::fs::create_dir_all(p).with_context(|| format!("create {}", p.display()))?;
        }
        if std::fs::rename(from, to).is_ok() {
            return Ok(());
        }
    }
    copy_tree(from, to, &mut |_| {}, false)?;
    remove(from)
}

/// Rewrites absolute paths stored in the database from under `old` to under `new`.
pub fn rebase_db_paths(db: &Path, old: &Path, new: &Path) -> Result<usize> {
    if !db.is_file() {
        return Ok(0);
    }
    let conn = rusqlite::Connection::open(db)?;
    let (old, new) = (old.to_string_lossy().to_string(), new.to_string_lossy().to_string());
    let old_dir = format!("{old}{MAIN_SEPARATOR}");
    let mut n = 0;
    for (table, col) in [("chapter", "pages_dir"), ("manga", "cover_path"), ("subtitle_cache", "file_path")] {
        let sql = format!("UPDATE {table} SET {col} = ?2 || substr({col}, length(?1) + 1) WHERE {col} = ?1 OR substr({col}, 1, length(?3)) = ?3");
        match conn.execute(&sql, rusqlite::params![old, new, old_dir]) {
            Ok(k) => n += k,
            Err(e) if e.to_string().contains("no such table") => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(n)
}

// ── start-up ────────────────────────────────────────────────────────────────────────────────────

/// Runs once at start, before anything opens the data folder: finishes a queued move or reset,
/// brings old layouts up to date and empties `temp/`. Returns what it did, for the log.
pub fn prepare() -> Vec<String> {
    let mut log = vec![];
    let mut loc = Location::load();
    if let Some(m) = loc.pending_move.take() {
        match move_dir(&m.from, &m.to) {
            Ok(()) => {
                let _ = rebase_db_paths(&m.to.join("mangavox.db"), &m.from, &m.to);
                loc.root = (norm(&m.to) != norm(&default_root())).then(|| m.to.clone());
                log.push(format!("Moved the data folder to {}", m.to.display()));
            }
            Err(e) => log.push(format!("Could not move the data folder to {}: {e:#}", m.to.display())),
        }
        if let Err(e) = loc.save() {
            log.push(format!("Could not save the data folder location: {e:#}"));
        }
    }
    let root = resolve(&loc);
    if !loc.reset.is_empty() {
        for p in std::mem::take(&mut loc.reset) {
            match wipe(&root, p) {
                Ok(()) => log.push(format!("Reset {p:?}")),
                Err(e) => log.push(format!("Could not reset {p:?}: {e:#}")),
            }
        }
        let _ = loc.save();
    }
    log.extend(migrate_layout(&root));
    let _ = remove(&root.join("temp"));
    let _ = std::fs::create_dir_all(root.join("temp"));
    let _ = ROOT.set(root);
    log
}

/// Older versions kept offline chapters in `downloads/`; they now live in `library/manga/`.
fn migrate_layout(root: &Path) -> Vec<String> {
    let mut log = vec![];
    let (old, new) = (root.join("downloads"), root.join("library").join("manga"));
    if old.is_dir() && !new.exists() {
        let _ = std::fs::create_dir_all(root.join("library"));
        match std::fs::rename(&old, &new) {
            Ok(()) => match rebase_db_paths(&root.join("mangavox.db"), &old, &new) {
                Ok(n) => log.push(format!("Moved offline chapters to {} ({n} paths updated)", new.display())),
                Err(e) => {
                    // Keep the database and the files in step: put the folder back.
                    let _ = std::fs::rename(&new, &old);
                    log.push(format!("Could not update chapter paths, kept downloads/: {e:#}"));
                }
            },
            Err(e) => log.push(format!("Could not move downloads/ to library/manga/: {e}")),
        }
    }
    log
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_round_trips_and_defaults_away() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("location.json");
        assert_eq!(Location::load_from(&f), Location::default());
        let loc = Location { root: Some(d.path().join("data")), pending_move: None, reset: vec![Part::Cache, Part::Everything] };
        loc.save_to(&f).unwrap();
        assert_eq!(Location::load_from(&f), loc);
        let raw = std::fs::read_to_string(&f).unwrap();
        assert!(raw.contains("\"everything\"") && !raw.contains("pending_move"));
    }

    fn db_with_chapter(path: &Path, pages: &str) {
        let c = rusqlite::Connection::open(path).unwrap();
        c.execute_batch("CREATE TABLE chapter (id TEXT, pages_dir TEXT, download_status TEXT); CREATE TABLE manga (id TEXT, cover_path TEXT);").unwrap();
        c.execute("INSERT INTO chapter VALUES ('c1', ?1, 'complete')", [pages]).unwrap();
        c.execute("INSERT INTO chapter VALUES ('c2', 'D:\\elsewhere\\x', 'complete')", []).unwrap();
    }

    #[test]
    fn rebase_rewrites_only_paths_under_the_old_folder() {
        let d = tempfile::tempdir().unwrap();
        let (old, new) = (d.path().join("downloads"), d.path().join("library").join("manga"));
        let db = d.path().join("t.db");
        db_with_chapter(&db, &old.join("m").join("c1").to_string_lossy());
        // A sibling whose name only starts the same must not be touched.
        rusqlite::Connection::open(&db).unwrap().execute("INSERT INTO chapter VALUES ('c3', ?1, 'complete')", [format!("{}-old", old.display())]).unwrap();
        assert_eq!(rebase_db_paths(&db, &old, &new).unwrap(), 1);
        let c = rusqlite::Connection::open(&db).unwrap();
        let got: String = c.query_row("SELECT pages_dir FROM chapter WHERE id = 'c1'", [], |r| r.get(0)).unwrap();
        assert_eq!(PathBuf::from(got), new.join("m").join("c1"));
        let other: String = c.query_row("SELECT pages_dir FROM chapter WHERE id = 'c3'", [], |r| r.get(0)).unwrap();
        assert!(other.ends_with("downloads-old"));
    }

    #[test]
    fn old_downloads_move_into_the_library_with_their_paths() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let page = root.join("downloads").join("m1").join("c1").join("0001.jpg");
        std::fs::create_dir_all(page.parent().unwrap()).unwrap();
        std::fs::write(&page, b"x").unwrap();
        db_with_chapter(&root.join("mangavox.db"), &page.parent().unwrap().to_string_lossy());
        let log = migrate_layout(root);
        assert_eq!(log.len(), 1, "{log:?}");
        assert!(root.join("library").join("manga").join("m1").join("c1").join("0001.jpg").is_file());
        assert!(!root.join("downloads").exists());
        let c = rusqlite::Connection::open(root.join("mangavox.db")).unwrap();
        let got: String = c.query_row("SELECT pages_dir FROM chapter WHERE id = 'c1'", [], |r| r.get(0)).unwrap();
        assert_eq!(PathBuf::from(got), root.join("library").join("manga").join("m1").join("c1"));
        assert!(migrate_layout(root).is_empty(), "runs once");
    }

    #[test]
    fn wipe_parts() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        for f in ["cache/covers/a.jpg", "studio/prefs.json", "studio/m1/brain.json", "studio/projects/m1/c1/read.json", "library/manga/m1/c1/1.jpg", "config.json"] {
            let p = r.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"{}").unwrap();
        }
        db_with_chapter(&r.join("mangavox.db"), &r.join("library/manga/m1/c1").to_string_lossy());
        wipe(r, Part::Cache).unwrap();
        assert!(!r.join("cache").exists());
        wipe(r, Part::Studio).unwrap();
        assert!(r.join("studio/prefs.json").is_file() && !r.join("studio/m1").exists() && !r.join("studio/projects").exists());
        wipe(r, Part::MangaDownloads).unwrap();
        assert!(!r.join("library/manga").exists());
        let c = rusqlite::Connection::open(r.join("mangavox.db")).unwrap();
        let st: String = c.query_row("SELECT download_status FROM chapter WHERE id = 'c1'", [], |r| r.get(0)).unwrap();
        assert_eq!(st, "none");
        drop(c);
        wipe(r, Part::Everything).unwrap();
        assert_eq!(std::fs::read_dir(r).unwrap().count(), 0);
    }

    #[test]
    fn move_dir_renames_or_copies() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("a"), d.path().join("b"));
        std::fs::create_dir_all(from.join("x")).unwrap();
        std::fs::write(from.join("x").join("f.txt"), b"hi").unwrap();
        std::fs::create_dir_all(&to).unwrap();
        move_dir(&from, &to).unwrap();
        assert_eq!(std::fs::read(to.join("x").join("f.txt")).unwrap(), b"hi");
        assert!(!from.exists());
        // Into a folder that already holds something: copy, then delete the source.
        let from2 = d.path().join("c");
        std::fs::create_dir_all(&from2).unwrap();
        std::fs::write(from2.join("g.txt"), b"yo").unwrap();
        move_dir(&from2, &to).unwrap();
        assert!(to.join("g.txt").is_file() && to.join("x").join("f.txt").is_file() && !from2.exists());
    }

    #[test]
    fn safe_names() {
        assert_eq!(safe_name("mangadex:abc/def"), "mangadex_abc_def");
        assert_eq!(safe_name(""), "item");
    }
}
