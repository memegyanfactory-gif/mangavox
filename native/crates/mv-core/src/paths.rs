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
//!   music/library/                        Studio's downloaded background-music tracks
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
use std::path::{Path, PathBuf};
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

/// Background-music tracks Studio downloads once and keeps (`music/library/`).
pub fn music_library() -> PathBuf {
    root().join("music").join("library")
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
    /// An old data folder to delete (left by a move; retried at each start until it is gone).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<PathBuf>,
    /// What the last move at start did, for Settings to show once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved: Option<String>,
    /// Why the last move at start failed (the old folder stayed in use).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub move_error: Option<String>,
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
//
// A move is done in two steps so nothing is copied while files are open:
// 1. Settings copies the data to the new folder while the app runs ([`precopy`], skipped on the
//    same drive where a rename is instant) and queues the move ([`choose_root`]).
// 2. At the next start, before anything opens the data folder, [`complete_move`] renames the
//    folder or brings the copy up to date (mirroring deletions), checks every file arrived,
//    rewrites the absolute paths stored in the database, JSON files and Python environments,
//    then [`prepare`] switches the pointer file to the new folder. Only after that is the old
//    folder deleted (in the background, retried at later starts until it is gone). Any failure
//    before the switch leaves the old folder in use, untouched.

/// Top-level folders that are not copied (emptied at every start anyway).
const SKIP_COPY: &[&str] = &["temp"];
/// Top-level folders whose files may be left behind when they can't be copied (rebuilt on demand).
const EXPENDABLE: &[&str] = &["cache", "temp", "logs", "webview"];
/// Folders whose JSON files are third-party (package trees, browser profile): never rewritten.
const NO_REWRITE: &[&str] = &["site-packages", "uv-cache", "uv-python", "webview", "node_modules", "Lib", "lib"];
/// Left in a folder while a copy into it is unfinished: such a folder is not usable data yet.
const INCOMPLETE: &str = ".mangavox-move-incomplete";
/// JSON files larger than this are left as they are (MangaVox never writes such big ones).
const MAX_JSON: u64 = 64 * 1024 * 1024;

/// Checks a folder the user picked as the new data folder.
pub fn check_new_root(to: &Path) -> Result<()> {
    if std::env::var_os("MANGAVOX_HOME").is_some_and(|p| !p.is_empty()) {
        bail!("The data folder is set by the MANGAVOX_HOME environment variable. Change or remove it to use another folder.");
    }
    check_move(&root(), to)
}

/// Can `to` replace `from` as the data folder? It must be another folder, not inside `from` or
/// around it, and either empty (or missing) or already a MangaVox data folder.
pub fn check_move(from: &Path, to: &Path) -> Result<()> {
    if !to.is_absolute() {
        bail!("Pick a full folder path.");
    }
    let (f, t) = (norm(from), norm(to));
    if f == t {
        bail!("That is already the data folder.");
    }
    if t.starts_with(&f) || f.starts_with(&t) {
        bail!("The new folder can't be inside the current data folder, or contain it.");
    }
    if to.exists() && !to.is_dir() {
        bail!("{} is a file, not a folder.", to.display());
    }
    if !is_empty_dir(to) && !has_data(to) && !to.join(INCOMPLETE).exists() {
        bail!("{} already has other files in it. Pick an empty folder.", to.display());
    }
    Ok(())
}

fn norm(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s).trim_end_matches(['\\', '/']).to_string();
    PathBuf::from(if cfg!(windows) { s.replace('/', "\\").to_lowercase() } else { s })
}

/// True when `dir` already holds MangaVox data (not a copy still in progress).
pub fn has_data(dir: &Path) -> bool {
    (dir.join("mangavox.db").is_file() || dir.join("config.json").is_file()) && !dir.join(INCOMPLETE).exists()
}

/// A folder a move started copying into but did not finish (see [`precopy`]).
pub fn is_unfinished_copy(dir: &Path) -> bool {
    dir.join(INCOMPLETE).is_file()
}

/// Deletes an unfinished copy left by a cancelled move (only a folder [`precopy`] marked).
pub fn discard_incomplete(dir: &Path) -> Result<()> {
    if !is_unfinished_copy(dir) || !safe_to_delete(dir, &root()) {
        return Ok(());
    }
    let left = remove_lenient(dir);
    if left > 0 {
        bail!("{left} file(s) in {} could not be removed", dir.display());
    }
    Ok(())
}

/// Missing, or a folder with nothing in it.
pub fn is_empty_dir(dir: &Path) -> bool {
    std::fs::read_dir(dir).map(|mut r| r.next().is_none()).unwrap_or(true)
}

/// Whether `a` and `b` (or their nearest existing parents) are on the same drive, so a move
/// between them is a rename rather than a copy.
pub fn same_volume(a: &Path, b: &Path) -> bool {
    let existing = |p: &Path| p.ancestors().find(|x| x.exists()).and_then(|x| x.canonicalize().ok());
    let (Some(x), Some(y)) = (existing(a), existing(b)) else { return false };
    #[cfg(windows)]
    {
        let volume = |p: &Path| p.components().next().map(|c| c.as_os_str().to_string_lossy().to_lowercase());
        volume(&x).is_some() && volume(&x) == volume(&y)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        matches!((std::fs::metadata(&x), std::fs::metadata(&y)), (Ok(m), Ok(n)) if m.dev() == n.dev())
    }
    #[cfg(not(any(windows, unix)))]
    {
        x == y
    }
}

/// Use `to` from the next start. With `move_data` the current data is moved there then.
pub fn choose_root(to: &Path, move_data: bool) -> Result<()> {
    check_new_root(to)?;
    std::fs::create_dir_all(to).with_context(|| format!("create {}", to.display()))?;
    let mut loc = Location::load();
    loc.pending_move = move_data.then(|| PendingMove { from: root(), to: to.to_path_buf() });
    if !move_data {
        loc.root = (norm(to) != norm(&default_root())).then(|| to.to_path_buf());
    }
    loc.move_error = None;
    loc.moved = None;
    loc.save()
}

/// What a copy did; `failed` lists files (relative to the data folder) it could not copy.
#[derive(Debug, Default)]
pub struct SyncReport {
    pub files: u64,
    pub bytes: u64,
    pub failed: Vec<(PathBuf, String)>,
}

/// Copies `from` into `to` while the app runs, skipping files already there with the same size
/// and time, and reporting `(bytes done, bytes total)`. Files that can't be read now are skipped:
/// the copy at the next start picks them up. Run it before restarting so the move at start is quick.
pub fn precopy(from: &Path, to: &Path, progress: &dyn Fn(u64, u64)) -> Result<SyncReport> {
    let total = copy_size(from);
    let mut done = 0;
    let mut report = SyncReport::default();
    std::fs::create_dir_all(to).with_context(|| format!("create {}", to.display()))?;
    std::fs::write(to.join(INCOMPLETE), b"MangaVox is moving its data here; this folder is not complete yet.
").with_context(|| format!("write to {}", to.display()))?;
    sync_tree(from, to, Path::new(""), &SyncOpts { mirror: false, retries: 0 }, &mut report, &mut |n| {
        done += n;
        progress(done, total);
    });
    Ok(report)
}

/// Bytes a move copies (the data folder without `temp/`).
pub fn copy_size(from: &Path) -> u64 {
    dir_size(from).saturating_sub(SKIP_COPY.iter().map(|d| dir_size(&from.join(d))).sum())
}

/// Copies a folder tree (used by [`move_dir`]); `lenient` skips files that can't be copied.
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
            match copy_file(&src, &dst, &md, 0) {
                Ok(()) => {}
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
    if is_empty_dir(to) {
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

struct SyncOpts {
    /// Delete what `to` has and `from` hasn't (the final pass, so stale files such as an old
    /// database journal never survive).
    mirror: bool,
    /// Extra attempts for a file that fails to copy (a scanner or indexer may hold it briefly).
    retries: u32,
}

fn same_file(a: &std::fs::Metadata, b: &Path) -> bool {
    let Ok(m) = std::fs::metadata(b) else { return false };
    m.is_file() && m.len() == a.len() && m.modified().ok() == a.modified().ok()
}

/// The database and its journal are copied again in the final pass whatever their size and time.
fn always_copy(rel: &Path) -> bool {
    rel.parent().is_some_and(|p| p.as_os_str().is_empty()) && rel.file_name().is_some_and(|n| n.to_string_lossy().starts_with("mangavox.db"))
}

fn top_level(rel: &Path) -> String {
    rel.components().next().map(|c| c.as_os_str().to_string_lossy().to_string()).unwrap_or_default()
}

fn copy_file(src: &Path, dst: &Path, md: &std::fs::Metadata, retries: u32) -> std::io::Result<()> {
    let mut attempt = 0;
    loop {
        // A read-only copy left by an earlier pass can't be overwritten.
        if let Ok(m) = std::fs::symlink_metadata(dst) {
            if m.is_dir() {
                let _ = std::fs::remove_dir_all(dst);
            } else if m.permissions().readonly() {
                let mut p = m.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                p.set_readonly(false);
                let _ = std::fs::set_permissions(dst, p);
            }
        }
        match std::fs::copy(src, dst) {
            Ok(_) => {
                if let Ok(t) = md.modified() {
                    let _ = std::fs::File::options().write(true).open(dst).and_then(|f| f.set_modified(t));
                }
                return Ok(());
            }
            Err(e) if attempt >= retries => return Err(e),
            Err(_) => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
        }
    }
}

/// Makes `to_root/rel` a copy of `from_root/rel`. Links are recreated pointing into `to_root`.
/// Failures are collected in `report` and the copy keeps going.
fn sync_tree(from_root: &Path, to_root: &Path, rel: &Path, opts: &SyncOpts, report: &mut SyncReport, on_bytes: &mut dyn FnMut(u64)) {
    let (from, to) = (from_root.join(rel), to_root.join(rel));
    let rd = match std::fs::read_dir(&from) {
        Ok(rd) => rd,
        Err(e) => return report.failed.push((rel.to_path_buf(), e.to_string())),
    };
    if let Err(e) = std::fs::create_dir_all(&to) {
        return report.failed.push((rel.to_path_buf(), e.to_string()));
    }
    let mut names = std::collections::HashSet::new();
    for e in rd.flatten() {
        let r = rel.join(e.file_name());
        if rel.as_os_str().is_empty() && SKIP_COPY.iter().any(|s| e.file_name() == *s) {
            continue;
        }
        names.insert(e.file_name());
        let (src, dst) = (e.path(), to_root.join(&r));
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            if let Err(err) = copy_link(&src, &dst, from_root, to_root) {
                report.failed.push((r, format!("{err:#}")));
            }
        } else if ft.is_dir() {
            if std::fs::symlink_metadata(&dst).is_ok_and(|m| !m.is_dir()) {
                let _ = remove_entry(&dst);
            }
            sync_tree(from_root, to_root, &r, opts, report, on_bytes);
        } else {
            let Ok(md) = e.metadata() else { continue };
            if (opts.mirror && always_copy(&r)) || !same_file(&md, &dst) {
                if let Err(err) = copy_file(&src, &dst, &md, opts.retries) {
                    report.failed.push((r, err.to_string()));
                    continue;
                }
            }
            report.files += 1;
            report.bytes += md.len();
            on_bytes(md.len());
        }
    }
    if opts.mirror {
        for e in std::fs::read_dir(&to).into_iter().flatten().flatten() {
            let ours = rel.as_os_str().is_empty() && e.file_name() == INCOMPLETE;
            if !ours && !names.contains(&e.file_name()) {
                if let Err(err) = remove_entry(&e.path()) {
                    report.failed.push((rel.join(e.file_name()), format!("could not delete the stale copy: {err:#}")));
                }
            }
        }
    }
}

/// Recreates the link `src` at `dst`, re-pointed into `to_root` when it points into `from_root`
/// (uv links its Python folders with junctions).
fn copy_link(src: &Path, dst: &Path, from_root: &Path, to_root: &Path) -> Result<()> {
    let target = std::fs::read_link(src).with_context(|| format!("read link {}", src.display()))?;
    let target = rebase_path(&target, from_root, to_root).unwrap_or(target);
    if std::fs::symlink_metadata(dst).is_ok() {
        if std::fs::read_link(dst).is_ok_and(|t| norm(&t) == norm(&target)) {
            return Ok(());
        }
        remove_entry(dst)?;
    }
    let is_dir = std::fs::metadata(src).map(|m| m.is_dir()).unwrap_or(false) || target.is_dir();
    make_link(&target, dst, is_dir)
}

fn make_link(target: &Path, link: &Path, is_dir: bool) -> Result<()> {
    #[cfg(windows)]
    {
        let made = if is_dir { std::os::windows::fs::symlink_dir(target, link) } else { std::os::windows::fs::symlink_file(target, link) };
        if made.is_ok() {
            return Ok(());
        }
        if is_dir {
            // Symbolic links need Developer Mode; junctions (what uv makes) don't.
            let abs = if target.is_absolute() { target.to_path_buf() } else { link.parent().map(|p| p.join(target)).unwrap_or_else(|| target.to_path_buf()) };
            let abs = PathBuf::from(abs.to_string_lossy().replace('/', "\\"));
            use std::os::windows::process::CommandExt;
            let out = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(link).arg(&abs).creation_flags(0x0800_0000).output()?;
            if out.status.success() {
                return Ok(());
            }
            bail!("could not link {} to {}: {}", link.display(), abs.display(), String::from_utf8_lossy(&out.stderr).trim());
        }
        made.with_context(|| format!("link {}", link.display()))
    }
    #[cfg(unix)]
    {
        let _ = is_dir;
        std::os::unix::fs::symlink(target, link).with_context(|| format!("link {}", link.display()))
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (target, link, is_dir);
        bail!("links are not supported here")
    }
}

/// Removes a file, link (without following it) or folder.
fn remove_entry(path: &Path) -> Result<()> {
    let md = match std::fs::symlink_metadata(path) {
        Ok(md) => md,
        Err(_) => return Ok(()),
    };
    let r = if md.file_type().is_symlink() {
        std::fs::remove_dir(path).or_else(|_| std::fs::remove_file(path))
    } else if md.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path).or_else(|e| {
            if !md.permissions().readonly() {
                return Err(e);
            }
            let mut p = md.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            p.set_readonly(false);
            std::fs::set_permissions(path, p)?;
            std::fs::remove_file(path)
        })
    };
    r.with_context(|| format!("remove {}", path.display()))
}

/// Deletes what it can under `path`, leaving files that are in use. Returns how many entries are left.
fn remove_lenient(path: &Path) -> u64 {
    let Ok(md) = std::fs::symlink_metadata(path) else { return 0 };
    if md.file_type().is_symlink() || !md.is_dir() {
        return u64::from(remove_entry(path).is_err());
    }
    let mut left: u64 = std::fs::read_dir(path).into_iter().flatten().flatten().map(|e| remove_lenient(&e.path())).sum();
    if std::fs::remove_dir(path).is_err() && left == 0 {
        left = 1;
    }
    left
}

/// Every file `to` should have after a move (all of `from` but `temp/`), with its size.
fn manifest(root: &Path, rel: &Path, out: &mut Vec<(PathBuf, u64)>) {
    for e in std::fs::read_dir(root.join(rel)).into_iter().flatten().flatten() {
        if rel.as_os_str().is_empty() && SKIP_COPY.iter().any(|s| e.file_name() == *s) {
            continue;
        }
        let r = rel.join(e.file_name());
        match e.file_type() {
            Ok(ft) if ft.is_dir() => manifest(root, &r, out),
            Ok(ft) if ft.is_file() => out.push((r, e.metadata().map(|m| m.len()).unwrap_or(0))),
            _ => {}
        }
    }
}

/// Checks `to` holds every file of `from` with the same size. Returns (files, bytes) checked;
/// missing files under [`EXPENDABLE`] folders are listed in `left_behind` instead of failing.
fn verify_copy(from: &Path, to: &Path, left_behind: &mut Vec<String>) -> Result<(u64, u64)> {
    let mut files = vec![];
    manifest(from, Path::new(""), &mut files);
    let mut bad = vec![];
    let (mut n, mut bytes) = (0, 0);
    for (rel, len) in files {
        let ok = std::fs::metadata(to.join(&rel)).is_ok_and(|m| m.len() == len);
        if ok {
            n += 1;
            bytes += len;
        } else if EXPENDABLE.contains(&top_level(&rel).as_str()) {
            left_behind.push(rel.display().to_string());
        } else {
            bad.push(rel.display().to_string());
        }
    }
    if !bad.is_empty() {
        bail!("{} file(s) did not arrive intact in the new folder, e.g. {}", bad.len(), bad.iter().take(3).cloned().collect::<Vec<_>>().join(", "));
    }
    Ok((n, bytes))
}

/// What [`complete_move`] did.
#[derive(Debug, Default)]
pub struct MoveReport {
    /// The folder was renamed in place (same drive): the old folder no longer exists.
    pub renamed: bool,
    /// Files and bytes checked in the new folder after copying.
    pub files: u64,
    pub bytes: u64,
    /// Stored paths updated (database values, JSON values, Python environments, links).
    pub rewritten: usize,
    /// Cache or log files that could not be copied (rebuilt on demand).
    pub left_behind: Vec<String>,
    /// Problems that did not stop the move.
    pub warnings: Vec<String>,
}

/// Moves the data from `m.from` to `m.to`. Nothing may have files open in either folder. On
/// success `m.to` holds a verified copy with every stored path pointing into it; `m.from` is
/// left in place (unless it was renamed) for the caller to delete once the switch is saved.
/// On error `m.from` is still a complete data folder.
pub fn complete_move(m: &PendingMove) -> Result<MoveReport> {
    let (from, to) = (m.from.as_path(), m.to.as_path());
    let (f, t) = (norm(from), norm(to));
    if f == t || t.starts_with(&f) || f.starts_with(&t) {
        bail!("{} can't be moved into {}", from.display(), to.display());
    }
    let mut report = MoveReport::default();
    if !from.is_dir() {
        if has_data(to) {
            report.warnings.push(format!("{} was already gone; using the data in {}", from.display(), to.display()));
            return Ok(report);
        }
        bail!("The data folder {} is missing.", from.display());
    }
    if is_empty_dir(to) {
        let _ = std::fs::remove_dir(to);
        if let Some(p) = to.parent() {
            std::fs::create_dir_all(p).with_context(|| format!("create {}", p.display()))?;
        }
        report.renamed = std::fs::rename(from, to).is_ok();
    } else if !has_data(to) && !to.join(INCOMPLETE).exists() {
        bail!("{} already has other files in it; MangaVox will not copy over them.", to.display());
    }
    if report.renamed {
        // Same drive: the data is all there. Fix the stored paths; if that fails, put it back.
        match rebase_all(to, from, to) {
            Ok(n) => report.rewritten = n,
            Err(e) => {
                if std::fs::rename(to, from).is_ok() {
                    return Err(e.context("Could not update the paths stored in the data folder"));
                }
                report.warnings.push(format!("Some stored paths may still point to the old folder: {e:#}"));
            }
        }
        return Ok(report);
    }
    // Until the copy is checked the folder is marked unfinished, so it is never taken for data.
    std::fs::write(to.join(INCOMPLETE), b"MangaVox is moving its data here; this folder is not complete yet.\n").with_context(|| format!("write to {}", to.display()))?;
    let mut sync = SyncReport::default();
    sync_tree(from, to, Path::new(""), &SyncOpts { mirror: true, retries: 3 }, &mut sync, &mut |_| {});
    let (soft, hard): (Vec<_>, Vec<_>) = sync.failed.into_iter().partition(|(rel, _)| EXPENDABLE.contains(&top_level(rel).as_str()));
    if !hard.is_empty() {
        let some: Vec<String> = hard.iter().take(3).map(|(r, e)| format!("{} ({e})", r.display())).collect();
        bail!("Could not copy {} file(s), e.g. {}", hard.len(), some.join("; "));
    }
    report.left_behind.extend(soft.into_iter().map(|(r, _)| r.display().to_string()));
    let (n, bytes) = verify_copy(from, to, &mut report.left_behind)?;
    report.left_behind.sort();
    report.left_behind.dedup();
    (report.files, report.bytes) = (n, bytes);
    report.rewritten = rebase_all(to, from, to).context("Could not update the paths stored in the new folder")?;
    std::fs::remove_file(to.join(INCOMPLETE)).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) }).context("Could not mark the new folder complete")?;
    Ok(report)
}

/// Rewrites stored absolute paths under `root` from `old` to `new`: the database, then JSON files,
/// Python environment configs and links.
fn rebase_all(root: &Path, old: &Path, new: &Path) -> Result<usize> {
    let n = rebase_db_paths(&root.join("mangavox.db"), old, new)?;
    Ok(n + rebase_tree(root, Path::new(""), old, new, false))
}

/// `s` with `old` at its start replaced by `new`, if `s` is `old` or a path under it. Matching
/// ignores case and slash direction on Windows and a `\\?\` prefix.
pub fn rebase_str(s: &str, old: &Path, new: &Path) -> Option<String> {
    let body = s.strip_prefix(r"\\?\").unwrap_or(s);
    let old_s = old.to_string_lossy();
    let old_s = old_s.strip_prefix(r"\\?\").unwrap_or(&old_s).trim_end_matches(['\\', '/']);
    if old_s.is_empty() {
        return None;
    }
    let (head, rest) = body.split_at_checked(old_s.len())?;
    let same = if cfg!(windows) { head.replace('/', "\\").to_lowercase() == old_s.replace('/', "\\").to_lowercase() } else { head == old_s };
    if !same || !(rest.is_empty() || rest.starts_with(['\\', '/'])) {
        return None;
    }
    let new_s = new.to_string_lossy();
    Some(format!("{}{rest}", new_s.trim_end_matches(['\\', '/'])))
}

/// [`rebase_str`] for a path.
pub fn rebase_path(p: &Path, old: &Path, new: &Path) -> Option<PathBuf> {
    rebase_str(&p.to_string_lossy(), old, new).map(PathBuf::from)
}

/// Rebases every string (and object key) in a JSON value; returns how many changed.
pub fn rebase_json(v: &mut serde_json::Value, old: &Path, new: &Path) -> usize {
    use serde_json::Value;
    match v {
        Value::String(s) => match rebase_str(s, old, new) {
            Some(n) => {
                *s = n;
                1
            }
            None => 0,
        },
        Value::Array(a) => a.iter_mut().map(|x| rebase_json(x, old, new)).sum(),
        Value::Object(m) => {
            let mut n: usize = m.values_mut().map(|x| rebase_json(x, old, new)).sum();
            if m.keys().any(|k| rebase_str(k, old, new).is_some()) {
                let entries: Vec<(String, Value)> = std::mem::take(m).into_iter().collect();
                for (k, val) in entries {
                    let k = match rebase_str(&k, old, new) {
                        Some(nk) => {
                            n += 1;
                            nk
                        }
                        None => k,
                    };
                    m.insert(k, val);
                }
            }
            n
        }
        _ => 0,
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("mvmove.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Rebases one JSON file in place; returns how many values changed.
pub fn rebase_json_file(path: &Path, old: &Path, new: &Path) -> usize {
    if std::fs::metadata(path).map_or(true, |m| m.len() > MAX_JSON) {
        return 0;
    }
    let Ok(text) = std::fs::read_to_string(path) else { return 0 };
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&text) else { return 0 };
    let n = rebase_json(&mut v, old, new);
    if n == 0 {
        return 0;
    }
    let out = if text.contains('\n') { serde_json::to_string_pretty(&v) } else { serde_json::to_string(&v) };
    match out.map(|o| write_atomic(path, o.as_bytes())) {
        Ok(Ok(())) => n,
        _ => 0,
    }
}

/// Rebases `home = …` style lines of a Python `pyvenv.cfg` (the venv's Python lives under the
/// data folder when MangaVox installed it with uv).
fn rebase_cfg_file(path: &Path, old: &Path, new: &Path) -> usize {
    let Ok(text) = std::fs::read_to_string(path) else { return 0 };
    let mut n = 0;
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            let Some((k, v)) = line.split_once('=') else { return line.to_string() };
            match rebase_str(v.trim(), old, new) {
                Some(nv) => {
                    n += 1;
                    format!("{} = {nv}", k.trim_end())
                }
                None => line.to_string(),
            }
        })
        .collect();
    if n > 0 && write_atomic(path, (lines.join("\n") + "\n").as_bytes()).is_err() {
        return 0;
    }
    n
}

/// Walks `root/rel` rewriting JSON files, `pyvenv.cfg` files and links that point under `old`.
fn rebase_tree(root: &Path, rel: &Path, old: &Path, new: &Path, third_party: bool) -> usize {
    let mut n = 0;
    for e in std::fs::read_dir(root.join(rel)).into_iter().flatten().flatten() {
        let name = e.file_name();
        let name_s = name.to_string_lossy();
        if rel.as_os_str().is_empty() && (SKIP_COPY.contains(&name_s.as_ref()) || name_s == "logs") {
            continue;
        }
        let r = rel.join(&name);
        let path = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            if let Some(t) = std::fs::read_link(&path).ok().and_then(|t| rebase_path(&t, old, new)) {
                let is_dir = std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false) || t.is_dir();
                if remove_entry(&path).is_ok() && make_link(&t, &path, is_dir).is_ok() {
                    n += 1;
                }
            }
        } else if ft.is_dir() {
            n += rebase_tree(root, &r, old, new, third_party || NO_REWRITE.contains(&name_s.as_ref()));
        } else if name_s.eq_ignore_ascii_case("pyvenv.cfg") {
            n += rebase_cfg_file(&path, old, new);
        } else if !third_party && name_s.to_ascii_lowercase().ends_with(".json") {
            n += rebase_json_file(&path, old, new);
        }
    }
    n
}

/// Rewrites absolute paths stored in the database from under `old` to under `new`: every text
/// value in every table that is such a path, or JSON holding such paths.
pub fn rebase_db_paths(db: &Path, old: &Path, new: &Path) -> Result<usize> {
    if !db.is_file() {
        return Ok(0);
    }
    let mut conn = rusqlite::Connection::open(db)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    // Only rows mentioning the folder's name are read (LIKE ignores ASCII case, and JSON-escaped
    // backslashes don't matter for one path component).
    let needle = old.file_name().map(|n| n.to_string_lossy().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")).filter(|n| !n.is_empty());
    let tx = conn.transaction()?;
    let tables: Vec<(String, String)> = {
        let mut st = tx.prepare("SELECT name, COALESCE(sql, '') FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let virtual_tables: Vec<&str> = tables.iter().filter(|(_, sql)| sql.trim_start().to_uppercase().starts_with("CREATE VIRTUAL")).map(|(n, _)| n.as_str()).collect();
    let mut n = 0;
    for (table, _) in &tables {
        if virtual_tables.iter().any(|v| table == v || table.starts_with(&format!("{v}_"))) {
            continue;
        }
        let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        let cols: Vec<String> = {
            let mut st = tx.prepare(&format!("PRAGMA table_info({})", q(table)))?;
            let rows = st.query_map([], |r| r.get::<_, String>(1))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for col in cols {
            let filter = if needle.is_some() { format!(" AND {c} LIKE ?1 ESCAPE '\\'", c = q(&col)) } else { String::new() };
            let sql = format!("SELECT rowid, {c} FROM {t} WHERE typeof({c}) = 'text'{filter}", c = q(&col), t = q(table));
            let mut st = match tx.prepare(&sql) {
                Ok(st) => st,
                Err(_) => break, // WITHOUT ROWID table: no paths are kept in those.
            };
            let pattern = needle.as_ref().map(|nd| format!("%{nd}%"));
            let params: Vec<&dyn rusqlite::ToSql> = pattern.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
            let rows: Vec<(i64, String)> = st.query_map(params.as_slice(), |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
            drop(st);
            for (rowid, value) in rows {
                let changed = rebase_str(&value, old, new).or_else(|| {
                    let t = value.trim_start();
                    if !(t.starts_with('{') || t.starts_with('[')) {
                        return None;
                    }
                    let mut v: serde_json::Value = serde_json::from_str(&value).ok()?;
                    (rebase_json(&mut v, old, new) > 0).then(|| v.to_string())
                });
                if let Some(nv) = changed {
                    n += tx.execute(&format!("UPDATE {t} SET {c} = ?1 WHERE rowid = ?2", t = q(table), c = q(&col)), rusqlite::params![nv, rowid])?;
                }
            }
        }
    }
    tx.commit()?;
    Ok(n)
}

/// May `old` be deleted after the data moved to `active`? Never the active folder, a folder
/// around or inside it, a drive root or the home folder.
pub fn safe_to_delete(old: &Path, active: &Path) -> bool {
    let (o, a) = (norm(old), norm(active));
    let home = dirs::home_dir().map(|h| norm(&h));
    old.is_absolute() && o.parent().is_some_and(|p| p.parent().is_some()) && o != a && !o.starts_with(&a) && !a.starts_with(&o) && home.as_ref() != Some(&o)
}

/// Deletes the old data folder after a move. Returns how many entries could not be removed yet
/// (in use); 0 when it is gone.
pub fn remove_old_root(old: &Path, active: &Path) -> Result<u64> {
    if !safe_to_delete(old, active) {
        bail!("{} is not safe to delete", old.display());
    }
    Ok(remove_lenient(old))
}

// ── start-up ────────────────────────────────────────────────────────────────────────────────────

/// Runs once at start, before anything opens the data folder: finishes a queued move or reset,
/// brings old layouts up to date and empties `temp/`. Returns what it did, for the log.
/// `defer_move`: another MangaVox may still have the files open, so a queued move waits.
pub fn prepare(defer_move: bool) -> Vec<String> {
    let (root, log) = prepare_in(&location_file(), defer_move, true);
    let _ = ROOT.set(root);
    log
}

/// [`prepare`] with the pointer file at `file`; returns the data folder to use. The old folder
/// left by a move is deleted on a background thread when `background` is set, else right away.
fn prepare_in(file: &Path, defer_move: bool, background: bool) -> (PathBuf, Vec<String>) {
    let mut log = vec![];
    let mut loc = Location::load_from(file);
    if let Some(m) = loc.pending_move.clone() {
        if std::env::var_os("MANGAVOX_HOME").is_some_and(|p| !p.is_empty()) {
            log.push("A data folder move is queued but MANGAVOX_HOME is set; it waits for a start without it".into());
        } else if defer_move {
            log.push("The previous MangaVox is still running; the data folder move waits for the next start".into());
        } else {
            log.extend(finish_move(&mut loc, &m, file));
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
        let _ = loc.save_to(file);
    }
    log.extend(migrate_layout(&root));
    let _ = remove(&root.join("temp"));
    let _ = std::fs::create_dir_all(root.join("temp"));
    if let Some(old) = loc.cleanup.clone() {
        if safe_to_delete(&old, &root) {
            // The old folder is no longer used: delete it without holding up the start.
            let (active, file) = (root.clone(), file.to_path_buf());
            let work = move || {
                let left = remove_old_root(&old, &active).unwrap_or(1);
                if left == 0 || !old.exists() {
                    let mut l = Location::load_from(&file);
                    if l.cleanup.as_ref() == Some(&old) {
                        l.cleanup = None;
                        let _ = l.save_to(&file);
                    }
                }
            };
            if background {
                std::thread::spawn(work);
            } else {
                work();
            }
        } else {
            loc.cleanup = None;
            let _ = loc.save_to(file);
        }
    }
    (root, log)
}

/// Does the queued move `m` and records the outcome in `loc` (saved). On failure the old folder
/// stays in use and the error is kept for Settings to show.
fn finish_move(loc: &mut Location, m: &PendingMove, file: &Path) -> Vec<String> {
    let mut log = vec![];
    let before = loc.clone();
    loc.pending_move = None;
    match complete_move(m) {
        Ok(r) => {
            loc.root = (norm(&m.to) != norm(&default_root())).then(|| m.to.clone());
            loc.cleanup = (!r.renamed && m.from.exists()).then(|| m.from.clone());
            loc.move_error = None;
            let mut msg = format!("Moved the data folder from {} to {}", m.from.display(), m.to.display());
            if !r.left_behind.is_empty() {
                msg.push_str(&format!(" ({} cache or log file(s) were in use and left behind)", r.left_behind.len()));
            }
            loc.moved = Some(msg.clone());
            log.push(format!("{msg}: {} files, {} bytes checked, {} stored paths updated", r.files, r.bytes, r.rewritten));
            log.extend(r.warnings);
            if let Err(e) = loc.save_to(file) {
                // The pointer still names the old folder, so keep using it (the next start must agree).
                log.push(format!("Could not save the new data folder location, staying on the old one: {e:#}"));
                if r.renamed {
                    let _ = std::fs::rename(&m.to, &m.from);
                }
                *loc = Location { pending_move: None, ..before };
            }
        }
        Err(e) => {
            let msg = format!("{e:#}");
            log.push(format!("Could not move the data folder to {}, still using {}: {msg}", m.to.display(), m.from.display()));
            loc.move_error = Some(format!("Could not move the data folder to {}. MangaVox still uses {}. {msg}", m.to.display(), m.from.display()));
            loc.moved = None;
            if let Err(e) = loc.save_to(file) {
                log.push(format!("Could not save the data folder location: {e:#}"));
            }
        }
    }
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
        let loc = Location { root: Some(d.path().join("data")), pending_move: None, reset: vec![Part::Cache, Part::Everything], ..Default::default() };
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

    // ── moving the data folder ──

    fn put(p: &Path, body: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn read_json(p: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    #[test]
    fn rebase_str_matches_whole_folder_names_only() {
        let (old, new) = (Path::new(r"C:\Users\me\.mangavox"), Path::new(r"E:\MangaVox"));
        if cfg!(windows) {
            assert_eq!(rebase_str(r"C:\Users\me\.mangavox\library\manga\m\c", old, new).as_deref(), Some(r"E:\MangaVox\library\manga\m\c"));
            // Case and slash direction don't matter on Windows; a \\?\ prefix is dropped.
            assert_eq!(rebase_str(r"c:/users/ME/.mangavox/studio/x.json", old, new).as_deref(), Some(r"E:\MangaVox/studio/x.json"));
            assert_eq!(rebase_str(r"\\?\C:\Users\me\.mangavox\uv-python\cpython", old, new).as_deref(), Some(r"E:\MangaVox\uv-python\cpython"));
            assert_eq!(rebase_str(r"C:\Users\me\.mangavox", old, new).as_deref(), Some(r"E:\MangaVox"));
        }
        let (old, new) = (Path::new("/home/me/.mangavox"), Path::new("/mnt/big/mv"));
        assert_eq!(rebase_str("/home/me/.mangavox/library/a", old, new).as_deref(), Some("/mnt/big/mv/library/a"));
        // A sibling whose name only starts the same, or a path elsewhere, is left alone.
        assert_eq!(rebase_str("/home/me/.mangavox-backup/a", old, new), None);
        assert_eq!(rebase_str("/home/me/Videos/a.mkv", old, new), None);
        assert_eq!(rebase_str("", old, new), None);
        assert_eq!(rebase_str("é", old, new), None);
    }

    #[test]
    fn rebase_json_rewrites_values_and_keys() {
        let (old, new) = (Path::new("/o/.mangavox"), Path::new("/n/mv"));
        let mut v = serde_json::json!({
            "items": [{"pages_dir": "/o/.mangavox/library/manga/m/c", "output_dir": "/o/.mangavox/studio/projects/m/c", "n": 3}],
            "/o/.mangavox/cache/x": true,
            "outside": "/elsewhere/downloads",
        });
        assert_eq!(rebase_json(&mut v, old, new), 3);
        assert_eq!(v["items"][0]["pages_dir"], "/n/mv/library/manga/m/c");
        assert_eq!(v["items"][0]["output_dir"], "/n/mv/studio/projects/m/c");
        assert_eq!(v["/n/mv/cache/x"], true);
        assert_eq!(v["outside"], "/elsewhere/downloads");
    }

    #[test]
    fn check_move_refuses_unsafe_targets() {
        let d = tempfile::tempdir().unwrap();
        let from = d.path().join("data");
        put(&from.join("config.json"), b"{}");
        assert!(check_move(&from, &from).is_err(), "same folder");
        assert!(check_move(&from, &from.join("sub")).is_err(), "inside");
        assert!(check_move(&from, d.path()).is_err(), "around");
        assert!(check_move(&from, Path::new("relative")).is_err());
        let busy = d.path().join("busy");
        put(&busy.join("holiday.jpg"), b"x");
        assert!(check_move(&from, &busy).is_err(), "unrelated files");
        put(&d.path().join("file"), b"x");
        assert!(check_move(&from, &d.path().join("file")).is_err(), "a file");
        assert!(check_move(&from, &d.path().join("new")).is_ok(), "missing folder");
        std::fs::create_dir_all(d.path().join("empty")).unwrap();
        assert!(check_move(&from, &d.path().join("empty")).is_ok(), "empty folder");
        let other = d.path().join("other");
        put(&other.join("mangavox.db"), b"x");
        assert!(check_move(&from, &other).is_ok(), "another data folder");
        // An unfinished copy counts as ours, but not as usable data.
        put(&busy.join(INCOMPLETE), b"");
        assert!(check_move(&from, &busy).is_ok());
        put(&busy.join("config.json"), b"{}");
        assert!(!has_data(&busy));
    }

    #[test]
    fn safe_to_delete_guards() {
        let d = tempfile::tempdir().unwrap();
        let (old, active) = (d.path().join("old"), d.path().join("new"));
        assert!(safe_to_delete(&old, &active));
        assert!(!safe_to_delete(&active, &active));
        assert!(!safe_to_delete(d.path(), &active), "contains the active folder");
        assert!(!safe_to_delete(&active.join("x"), &active), "inside the active folder");
        if let Some(h) = dirs::home_dir() {
            assert!(!safe_to_delete(&h, &active));
        }
        let drive = if cfg!(windows) { PathBuf::from(r"Z:\") } else { PathBuf::from("/") };
        assert!(!safe_to_delete(&drive, &active));
    }

    /// A data folder like the real one: settings with folders inside and outside it, the Studio
    /// queue, the database, a Python environment and scratch files.
    fn sample_root(root: &Path, outside: &Path) {
        let s = |p: PathBuf| p.to_string_lossy().to_string();
        put(
            &root.join("config.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "storage": {"manga": s(root.join("library").join("manga")), "anime": s(outside.to_path_buf())},
                "aiAssistant": {"hub": {"modelsDir": s(root.join("models"))}},
            }))
            .unwrap()
            .as_bytes(),
        );
        put(
            &root.join("studio").join("pipeline_queue.json"),
            serde_json::to_string(&serde_json::json!({"items": [{
                "pages_dir": s(root.join("library").join("manga").join("m1").join("c1")),
                "output_dir": s(root.join("studio").join("projects").join("m1").join("c1")),
            }]}))
            .unwrap()
            .as_bytes(),
        );
        put(&root.join("library").join("manga").join("m1").join("c1").join("0001.jpg"), &vec![7u8; 300_000]);
        put(&root.join("studio").join("projects").join("m1").join("c1").join("video.mp4"), b"mp4");
        put(&root.join("tts-venv").join("pyvenv.cfg"), format!("home = {}\ninclude-system-site-packages = false\n", s(root.join("uv-python").join("cpython-3.12"))).as_bytes());
        put(&root.join("tts-venv").join("Lib").join("site-packages").join("pkg").join("meta.json"), format!("{{\"p\": {:?}}}", s(root.join("x"))).as_bytes());
        put(&root.join("temp").join("scratch.bin"), b"scratch");
        let c = rusqlite::Connection::open(root.join("mangavox.db")).unwrap();
        c.execute_batch("CREATE TABLE chapter (id TEXT, pages_dir TEXT); CREATE TABLE character (id TEXT, face_crop_path TEXT); CREATE TABLE pipeline_job (id TEXT, config TEXT);").unwrap();
        c.execute("INSERT INTO chapter VALUES ('c1', ?1)", [s(root.join("library").join("manga").join("m1").join("c1"))]).unwrap();
        c.execute("INSERT INTO chapter VALUES ('c2', ?1)", [s(outside.join("c2"))]).unwrap();
        c.execute("INSERT INTO character VALUES ('k', ?1)", [s(root.join("studio").join("m1").join("face.png"))]).unwrap();
        c.execute("INSERT INTO pipeline_job VALUES ('j', ?1)", [serde_json::json!({"out": s(root.join("studio").join("out"))}).to_string()]).unwrap();
    }

    fn assert_moved(from: &Path, to: &Path, outside: &Path) {
        let s = |p: PathBuf| p.to_string_lossy().to_string();
        assert!(!to.join(INCOMPLETE).exists(), "marked complete");
        assert!(has_data(to));
        assert_eq!(std::fs::read(to.join("library/manga/m1/c1/0001.jpg")).unwrap().len(), 300_000);
        let cfg = read_json(&to.join("config.json"));
        assert_eq!(cfg["storage"]["manga"], s(to.join("library").join("manga")));
        assert_eq!(cfg["storage"]["anime"], s(outside.to_path_buf()), "folders outside stay");
        assert_eq!(cfg["aiAssistant"]["hub"]["modelsDir"], s(to.join("models")));
        let q = read_json(&to.join("studio/pipeline_queue.json"));
        assert_eq!(q["items"][0]["pages_dir"], s(to.join("library").join("manga").join("m1").join("c1")));
        assert_eq!(q["items"][0]["output_dir"], s(to.join("studio").join("projects").join("m1").join("c1")));
        let cfg_text = std::fs::read_to_string(to.join("tts-venv/pyvenv.cfg")).unwrap();
        assert!(cfg_text.contains(&format!("home = {}", s(to.join("uv-python").join("cpython-3.12")))), "{cfg_text}");
        // Third-party package files are never touched.
        assert_eq!(read_json(&to.join("tts-venv/Lib/site-packages/pkg/meta.json"))["p"], s(from.join("x")));
        let c = rusqlite::Connection::open(to.join("mangavox.db")).unwrap();
        let get = |sql: &str| -> String { c.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(get("SELECT pages_dir FROM chapter WHERE id = 'c1'"), s(to.join("library").join("manga").join("m1").join("c1")));
        assert_eq!(get("SELECT pages_dir FROM chapter WHERE id = 'c2'"), s(outside.join("c2")));
        assert_eq!(get("SELECT face_crop_path FROM character"), s(to.join("studio").join("m1").join("face.png")));
        let job: serde_json::Value = serde_json::from_str(&get("SELECT config FROM pipeline_job")).unwrap();
        assert_eq!(job["out"], s(to.join("studio").join("out")));
    }

    #[test]
    fn move_by_copy_then_restart() {
        let d = tempfile::tempdir().unwrap();
        let (from, to, outside) = (d.path().join("old"), d.path().join("new"), d.path().join("Videos"));
        sample_root(&from, &outside);
        // While the app runs: a database journal exists and gets copied ahead.
        put(&from.join("mangavox.db-wal"), b"journal");
        put(&from.join("assistant").join("gone.json"), b"{}");
        let calls = std::cell::Cell::new(0);
        let r = precopy(&from, &to, &|done, total| {
            calls.set(calls.get() + 1);
            assert!(done <= total);
        })
        .unwrap();
        assert!(calls.get() > 0 && r.failed.is_empty(), "{:?}", r.failed);
        assert!(is_unfinished_copy(&to) && !has_data(&to), "not usable before the restart");
        assert!(!to.join("temp").exists(), "temp is not copied");
        // Then the app quits: the journal is folded into the database, a chat is deleted and a
        // page is added. The restart must copy exactly what is there now.
        std::fs::remove_file(from.join("mangavox.db-wal")).unwrap();
        std::fs::remove_file(from.join("assistant").join("gone.json")).unwrap();
        put(&from.join("library/manga/m1/c1/0002.jpg"), b"new page");

        let r = complete_move(&PendingMove { from: from.clone(), to: to.clone() }).unwrap();
        assert!(!r.renamed);
        assert!(r.rewritten >= 7, "{r:?}");
        assert!(!to.join("mangavox.db-wal").exists(), "a stale journal would corrupt the database");
        assert!(!to.join("assistant").join("gone.json").exists());
        assert!(to.join("library/manga/m1/c1/0002.jpg").is_file());
        assert_moved(&from, &to, &outside);
        // The old folder is untouched until the switch is saved.
        assert!(has_data(&from) && from.join("library/manga/m1/c1/0001.jpg").is_file());
        assert_eq!(remove_old_root(&from, &to).unwrap(), 0);
        assert!(!from.exists());
    }

    #[test]
    fn move_by_rename_fixes_paths() {
        let d = tempfile::tempdir().unwrap();
        let (from, to, outside) = (d.path().join("old"), d.path().join("new"), d.path().join("Videos"));
        sample_root(&from, &outside);
        std::fs::create_dir_all(&to).unwrap();
        let r = complete_move(&PendingMove { from: from.clone(), to: to.clone() }).unwrap();
        assert!(r.renamed && !from.exists());
        assert_moved(&from, &to, &outside);
    }

    #[cfg(windows)]
    #[test]
    fn junctions_are_recreated_pointing_into_the_new_folder() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("old"), d.path().join("new"));
        put(&from.join("config.json"), b"{}");
        put(&from.join("uv-python/cpython-3.12.9/python.exe"), b"py");
        let link = from.join("uv-python").join("cpython-3.12");
        let out = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(from.join("uv-python").join("cpython-3.12.9")).output().unwrap();
        assert!(out.status.success(), "{out:?}");
        put(&to.join(INCOMPLETE), b"");
        put(&to.join("config.json"), b"{}");
        complete_move(&PendingMove { from: from.clone(), to: to.clone() }).unwrap();
        let t = std::fs::read_link(to.join("uv-python").join("cpython-3.12")).unwrap();
        assert_eq!(norm(&t), norm(&to.join("uv-python").join("cpython-3.12.9")));
        assert!(to.join("uv-python/cpython-3.12/python.exe").is_file());
        // Deleting the old folder never follows the link into the new one.
        assert_eq!(remove_old_root(&from, &to).unwrap(), 0);
        assert!(to.join("uv-python/cpython-3.12.9/python.exe").is_file());
    }

    #[cfg(windows)]
    #[test]
    fn a_locked_file_stops_the_move_and_keeps_the_old_folder() {
        use std::os::windows::fs::OpenOptionsExt;
        let d = tempfile::tempdir().unwrap();
        let (from, to, outside) = (d.path().join("old"), d.path().join("new"), d.path().join("Videos"));
        sample_root(&from, &outside);
        put(&to.join(INCOMPLETE), b"");
        put(&to.join("leftover.txt"), b"x");
        // A cache file in use is fine (rebuilt on demand)...
        put(&from.join("cache/covers/a.jpg"), b"cover");
        let cache_lock = std::fs::OpenOptions::new().read(true).share_mode(0).open(from.join("cache/covers/a.jpg")).unwrap();
        // ...but a library file in use is not.
        let lock = std::fs::OpenOptions::new().read(true).share_mode(0).open(from.join("library/manga/m1/c1/0001.jpg")).unwrap();
        let file = d.path().join("location.json");
        let mut loc = Location { pending_move: Some(PendingMove { from: from.clone(), to: to.clone() }), ..Default::default() };
        loc.save_to(&file).unwrap();
        let log = finish_move(&mut loc, &PendingMove { from: from.clone(), to: to.clone() }, &file);
        assert!(log[0].contains("Could not move"), "{log:?}");
        let saved = Location::load_from(&file);
        assert!(saved.pending_move.is_none() && saved.root.is_none() && saved.cleanup.is_none());
        assert!(saved.move_error.as_deref().unwrap_or("").contains("0001.jpg"), "{saved:?}");
        assert!(is_unfinished_copy(&to), "the partial copy is never taken for data");
        assert!(has_data(&from) && from.join("library/manga/m1/c1/0001.jpg").is_file());
        drop(lock);
        // Retried with only the cache file locked: the move goes through and says so.
        let log = finish_move(&mut loc, &PendingMove { from: from.clone(), to: to.clone() }, &file);
        assert!(log[0].starts_with("Moved"), "{log:?}");
        let saved = Location::load_from(&file);
        assert_eq!(saved.root.as_deref(), Some(to.as_path()));
        assert_eq!(saved.cleanup.as_deref(), Some(from.as_path()));
        assert!(saved.moved.as_deref().unwrap_or("").contains("left behind"), "{saved:?}");
        drop(cache_lock);
        assert_moved(&from, &to, &outside);
    }

    #[test]
    fn prepare_finishes_a_queued_move_and_deletes_the_old_folder() {
        let d = tempfile::tempdir().unwrap();
        let (from, to, outside) = (d.path().join("old"), d.path().join("other-drive").join("MangaVox"), d.path().join("Videos"));
        sample_root(&from, &outside);
        precopy(&from, &to, &|_, _| {}).unwrap();
        let file = d.path().join("location.json");
        Location { root: Some(from.clone()), pending_move: Some(PendingMove { from: from.clone(), to: to.clone() }), ..Default::default() }.save_to(&file).unwrap();
        // Deferred (old instance still running): nothing happens.
        let (root, _) = prepare_in(&file, true, false);
        assert_eq!(root, from);
        assert!(Location::load_from(&file).pending_move.is_some());
        let (root, log) = prepare_in(&file, false, false);
        assert_eq!(root, to, "{log:?}");
        assert!(to.join("temp").is_dir(), "temp recreated in the new folder");
        let saved = Location::load_from(&file);
        assert_eq!(saved.root.as_deref(), Some(to.as_path()));
        assert!(saved.pending_move.is_none() && saved.cleanup.is_none() && saved.moved.is_some(), "{saved:?}");
        assert!(!from.exists(), "old folder deleted after the switch");
        assert_moved(&from, &to, &outside);
        // The next start just uses it.
        let (root, log) = prepare_in(&file, false, false);
        assert_eq!(root, to);
        assert!(log.is_empty(), "{log:?}");
    }
}
