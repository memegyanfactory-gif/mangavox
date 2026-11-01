//! Private mode (incognito) for the Manga, Anime and Movies areas.
//!
//! While it is on, those areas record nothing: no reading or watch history, resume points,
//! Continue Reading / Watching, taste signals or "recent" entries. Every place that writes one
//! asks [`records_history`] first. Everything else still works (downloads, adding to the
//! library), and what the session creates is noted in a small append-only record
//! (`<data>/private-session.jsonl`: ids and paths only), so the end-of-session offer to clear it
//! survives a crash or a restart. The record is deleted once that offer is answered.
//!
//! Studio, Thumbnails and the rest of the app never look at this module.
use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

static ACTIVE: AtomicBool = AtomicBool::new(false);
/// One writer at a time: page downloads record from several threads.
static WRITE: Mutex<()> = Mutex::new(());

/// One line of the session record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum Entry {
    Started { at: i64 },
    /// A manga chapter queued for download.
    Download { manga: String, chapter: String },
    /// A manga that was not in the library before was added to it.
    LibraryAdd { manga: String },
    /// An anime / movie / series added to My List (it was not listed before).
    ListAdd { media: String },
    /// A manga row created by opening a title that was not in the database before.
    Title { manga: String },
    /// A cache file (cover, page, artwork, catalog) that did not exist before.
    File { path: PathBuf },
}

/// What one private session created, read back from its record.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    /// Unix seconds the session started (the earliest start in the record).
    pub started_at: i64,
    pub downloads: Vec<(String, String)>,
    pub library_adds: Vec<String>,
    pub list_adds: Vec<String>,
    pub titles: Vec<String>,
    pub files: Vec<PathBuf>,
}

impl Session {
    /// Reads a record (one JSON entry per line; unreadable lines, e.g. a half-written last line
    /// after a crash, are skipped). Duplicates are dropped, first one wins.
    pub fn from_lines(text: &str) -> Session {
        let mut s = Session::default();
        let mut started: Option<i64> = None;
        for e in text.lines().filter_map(|l| serde_json::from_str::<Entry>(l.trim()).ok()) {
            match e {
                Entry::Started { at } => started = Some(started.map_or(at, |s| s.min(at))),
                Entry::Download { manga, chapter } => push_new(&mut s.downloads, (manga, chapter)),
                Entry::LibraryAdd { manga } => push_new(&mut s.library_adds, manga),
                Entry::ListAdd { media } => push_new(&mut s.list_adds, media),
                Entry::Title { manga } => push_new(&mut s.titles, manga),
                Entry::File { path } => push_new(&mut s.files, path),
            }
        }
        s.started_at = started.unwrap_or(0);
        s
    }

    /// Nothing was created that could be cleared.
    pub fn is_empty(&self) -> bool {
        self.downloads.is_empty() && self.library_adds.is_empty() && self.list_adds.is_empty() && self.titles.is_empty() && self.files.is_empty()
    }
}

fn push_new<T: PartialEq>(v: &mut Vec<T>, item: T) {
    if !v.contains(&item) {
        v.push(item);
    }
}

/// The record file of a session.
pub struct Record {
    path: PathBuf,
}

impl Record {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Record { path: path.into() }
    }

    /// `<data>/private-session.jsonl`.
    pub fn shared() -> Self {
        Record::at(crate::paths::root().join("private-session.jsonl"))
    }

    pub fn append(&self, e: &Entry) -> std::io::Result<()> {
        let mut line = serde_json::to_string(e)?;
        line.push('\n');
        let _g = WRITE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::OpenOptions::new().create(true).append(true).open(&self.path)?.write_all(line.as_bytes())
    }

    /// The session in the record; `None` when there is no record.
    pub fn load(&self) -> Option<Session> {
        std::fs::read_to_string(&self.path).ok().map(|t| Session::from_lines(&t))
    }

    pub fn remove(&self) {
        let _g = WRITE.lock().unwrap_or_else(|e| e.into_inner());
        let _ = std::fs::remove_file(&self.path);
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Private mode is on.
pub fn active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

/// History, resume points, taste signals and "recent" lists may be written. False while
/// private: every such write asks this first.
pub fn records_history() -> bool {
    !active()
}

/// Turns private mode on and starts the session record. An earlier record that was never
/// answered is kept and continued (its start time stays the session's start).
pub fn start() {
    ACTIVE.store(true, Ordering::SeqCst);
    let _ = Record::shared().append(&Entry::Started { at: now_secs() });
}

/// Turns private mode off and returns what the session created. The record stays until
/// [`finish`], so a crash before the user answers still offers the clean-up at the next start.
pub fn end() -> Session {
    ACTIVE.store(false, Ordering::SeqCst);
    Record::shared().load().unwrap_or_default()
}

/// At startup: a session that was still open (or never answered) when the app closed.
pub fn pending() -> Option<Session> {
    if active() {
        return None;
    }
    Record::shared().load()
}

/// The session has been handled (cleared or kept): forget its record.
pub fn finish() {
    Record::shared().remove();
}

fn note(e: Entry) {
    if active() {
        let _ = Record::shared().append(&e);
    }
}

/// While private: a manga chapter was queued for download.
pub fn record_download(manga_id: &str, chapter_id: &str) {
    note(Entry::Download { manga: manga_id.into(), chapter: chapter_id.into() });
}

/// While private: a manga that was not in the library was added to it.
pub fn record_library_add(manga_id: &str) {
    note(Entry::LibraryAdd { manga: manga_id.into() });
}

/// While private: a title that was not on My List was added to it.
pub fn record_list_add(media_id: &str) {
    note(Entry::ListAdd { media: media_id.into() });
}

/// While private: opening a title created its manga row.
pub fn record_title(manga_id: &str) {
    note(Entry::Title { manga: manga_id.into() });
}

/// While private: a cache file was created (call only when it did not exist before).
pub fn record_file(path: &Path) {
    note(Entry::File { path: path.to_path_buf() });
}

// ── Clean-up plan ─────────────────────────────────────────────────────────────────────────────

/// The current state of the things a session record names, for [`plan`].
pub trait Probe {
    /// Bytes of a downloaded chapter on disk; `None` when it is not downloaded.
    fn download_size(&self, manga: &str, chapter: &str) -> Option<u64>;
    /// The chapter has Studio work (read, script, audio, video).
    fn chapter_in_studio(&self, manga: &str, chapter: &str) -> bool;
    /// Any chapter of the title has Studio work, or Studio keeps its character brain.
    fn title_in_studio(&self, manga: &str) -> bool;
    fn in_library(&self, manga: &str) -> bool;
    fn in_list(&self, media: &str) -> bool;
    fn title_exists(&self, manga: &str) -> bool;
    /// Size and creation time (unix seconds) of a file; `None` when it is gone.
    fn file_info(&self, path: &Path) -> Option<(u64, i64)>;
}

/// What clearing a session can remove: only what the session created and is still there.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CleanPlan {
    pub downloads: Vec<(String, String)>,
    pub download_bytes: u64,
    /// Chapters downloaded in the session that have Studio work: never removed.
    pub studio_kept: Vec<(String, String)>,
    /// Manga to take back out of the library.
    pub library: Vec<String>,
    /// Titles to take off My List.
    pub list: Vec<String>,
    /// Manga rows the session created, to delete.
    pub titles: Vec<String>,
    pub files: Vec<PathBuf>,
    pub file_bytes: u64,
}

impl CleanPlan {
    pub fn is_empty(&self) -> bool {
        self.downloads.is_empty() && self.library.is_empty() && self.list.is_empty() && self.titles.is_empty() && self.files.is_empty()
    }
}

/// Which parts of a [`CleanPlan`] the user chose to clear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    pub downloads: bool,
    pub library: bool,
    pub titles: bool,
    pub files: bool,
}

impl Choice {
    pub const ALL: Choice = Choice { downloads: true, library: true, titles: true, files: true };
}

/// Plans the clean-up of `s`: recorded items that are still there. A chapter with Studio work
/// stays, so does a title Studio uses, and a file older than the session (it existed before)
/// is never selected.
pub fn plan(s: &Session, p: &impl Probe) -> CleanPlan {
    let mut out = CleanPlan::default();
    for (m, c) in &s.downloads {
        let Some(bytes) = p.download_size(m, c) else { continue };
        if p.chapter_in_studio(m, c) {
            out.studio_kept.push((m.clone(), c.clone()));
        } else {
            out.downloads.push((m.clone(), c.clone()));
            out.download_bytes += bytes;
        }
    }
    out.library = s.library_adds.iter().filter(|m| p.in_library(m)).cloned().collect();
    out.list = s.list_adds.iter().filter(|m| p.in_list(m)).cloned().collect();
    out.titles = s.titles.iter().filter(|m| p.title_exists(m) && !p.title_in_studio(m)).cloned().collect();
    for f in &s.files {
        if let Some((bytes, created)) = p.file_info(f) {
            if created >= s.started_at {
                out.files.push(f.clone());
                out.file_bytes += bytes;
            }
        }
    }
    out
}

impl CleanPlan {
    /// The part `c` clears. A title row goes only when nothing kept still needs it: none of its
    /// chapters stays downloaded (Studio work, or downloads left unticked) and it does not stay
    /// in the library.
    pub fn selected(&self, c: Choice) -> CleanPlan {
        let mut kept: HashSet<&str> = self.studio_kept.iter().map(|(m, _)| m.as_str()).collect();
        if !c.downloads {
            kept.extend(self.downloads.iter().map(|(m, _)| m.as_str()));
        }
        if !c.library {
            kept.extend(self.library.iter().map(String::as_str));
        }
        CleanPlan {
            downloads: if c.downloads { self.downloads.clone() } else { vec![] },
            download_bytes: if c.downloads { self.download_bytes } else { 0 },
            studio_kept: self.studio_kept.clone(),
            library: if c.library { self.library.clone() } else { vec![] },
            list: if c.library { self.list.clone() } else { vec![] },
            titles: if c.titles { self.titles.iter().filter(|t| !kept.contains(t.as_str())).cloned().collect() } else { vec![] },
            files: if c.files { self.files.clone() } else { vec![] },
            file_bytes: if c.files { self.file_bytes } else { 0 },
        }
    }
}

/// "120 MB", "3.4 GB", "820 KB".
pub fn human_bytes(b: u64) -> String {
    const K: f64 = 1024.;
    let b = b as f64;
    if b >= K * K * K {
        format!("{:.1} GB", b / (K * K * K))
    } else if b >= K * K {
        format!("{:.0} MB", b / (K * K))
    } else if b >= K {
        format!("{:.0} KB", b / K)
    } else {
        format!("{b:.0} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn record_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let rec = Record::at(dir.path().join("private-session.jsonl"));
        assert_eq!(rec.load(), None, "no record yet");
        rec.append(&Entry::Started { at: 100 }).unwrap();
        rec.append(&Entry::Download { manga: "m1".into(), chapter: "c1".into() }).unwrap();
        rec.append(&Entry::Download { manga: "m1".into(), chapter: "c1".into() }).unwrap();
        rec.append(&Entry::LibraryAdd { manga: "m1".into() }).unwrap();
        rec.append(&Entry::ListAdd { media: "anime:mal:21".into() }).unwrap();
        rec.append(&Entry::Title { manga: "m2".into() }).unwrap();
        rec.append(&Entry::File { path: PathBuf::from("cache/pages/a.jpg") }).unwrap();
        // Continued after a restart that never answered the offer: the first start counts.
        rec.append(&Entry::Started { at: 200 }).unwrap();
        // A crash mid-write leaves half a line: it is skipped.
        std::fs::OpenOptions::new().append(true).open(dir.path().join("private-session.jsonl")).unwrap().write_all(b"{\"k\":\"downl").unwrap();
        let s = rec.load().unwrap();
        assert_eq!(
            s,
            Session {
                started_at: 100,
                downloads: vec![("m1".into(), "c1".into())],
                library_adds: vec!["m1".into()],
                list_adds: vec!["anime:mal:21".into()],
                titles: vec!["m2".into()],
                files: vec![PathBuf::from("cache/pages/a.jpg")],
            }
        );
        assert!(!s.is_empty());
        rec.remove();
        assert_eq!(rec.load(), None, "the record is gone once handled");
    }

    #[test]
    fn nothing_is_recorded_while_off() {
        // `note` is a no-op unless private mode is on (this test never turns it on).
        assert!(!active() && records_history());
        let s = Session::from_lines("");
        assert!(s.is_empty() && s.started_at == 0);
    }

    #[derive(Default)]
    struct Fake {
        downloads: HashMap<(String, String), u64>,
        studio_chapters: HashSet<(String, String)>,
        studio_titles: HashSet<String>,
        library: HashSet<String>,
        list: HashSet<String>,
        titles: HashSet<String>,
        files: HashMap<PathBuf, (u64, i64)>,
    }

    impl Probe for Fake {
        fn download_size(&self, m: &str, c: &str) -> Option<u64> {
            self.downloads.get(&(m.into(), c.into())).copied()
        }
        fn chapter_in_studio(&self, m: &str, c: &str) -> bool {
            self.studio_chapters.contains(&(m.into(), c.into()))
        }
        fn title_in_studio(&self, m: &str) -> bool {
            self.studio_titles.contains(m)
        }
        fn in_library(&self, m: &str) -> bool {
            self.library.contains(m)
        }
        fn in_list(&self, m: &str) -> bool {
            self.list.contains(m)
        }
        fn title_exists(&self, m: &str) -> bool {
            self.titles.contains(m)
        }
        fn file_info(&self, p: &Path) -> Option<(u64, i64)> {
            self.files.get(p).copied()
        }
    }

    fn pair(m: &str, c: &str) -> (String, String) {
        (m.into(), c.into())
    }

    #[test]
    fn plan_selects_only_what_the_session_created() {
        let s = Session {
            started_at: 1_000,
            downloads: vec![pair("m1", "c1"), pair("m1", "c2"), pair("m2", "c9"), pair("m3", "gone")],
            library_adds: vec!["m1".into(), "left".into()],
            list_adds: vec!["movie:imdb:tt1".into(), "movie:imdb:tt2".into()],
            titles: vec!["m2".into(), "m4".into(), "deleted".into()],
            files: vec![PathBuf::from("new.jpg"), PathBuf::from("old.jpg"), PathBuf::from("gone.jpg")],
        };
        let mut f = Fake::default();
        // Downloaded before the session and not in the record: never part of the plan.
        f.downloads.insert(pair("m0", "c0"), 999);
        f.downloads.insert(pair("m1", "c1"), 10);
        f.downloads.insert(pair("m1", "c2"), 20);
        f.downloads.insert(pair("m2", "c9"), 5);
        // A privately downloaded chapter that Studio has worked on stays.
        f.studio_chapters.insert(pair("m1", "c2"));
        // "left" was taken out of the library again during the session.
        f.library.extend(["m1".to_string(), "m0".to_string()]);
        f.list.insert("movie:imdb:tt1".into());
        f.titles.extend(["m2".to_string(), "m4".to_string(), "m0".to_string()]);
        f.studio_titles.insert("m4".into());
        f.files.insert(PathBuf::from("new.jpg"), (7, 1_500));
        // Created before the session started (it existed before): kept.
        f.files.insert(PathBuf::from("old.jpg"), (100, 900));

        let p = plan(&s, &f);
        assert_eq!(p.downloads, vec![pair("m1", "c1"), pair("m2", "c9")]);
        assert_eq!(p.download_bytes, 15);
        assert_eq!(p.studio_kept, vec![pair("m1", "c2")]);
        assert_eq!(p.library, vec!["m1".to_string()]);
        assert_eq!(p.list, vec!["movie:imdb:tt1".to_string()]);
        assert_eq!(p.titles, vec!["m2".to_string()], "m4 is used by Studio, m0 is older than the session");
        assert_eq!(p.files, vec![PathBuf::from("new.jpg")]);
        assert_eq!(p.file_bytes, 7);
        assert!(!p.is_empty());

        // Everything ticked: the title row goes with its downloads.
        assert_eq!(p.selected(Choice::ALL).titles, vec!["m2".to_string()]);
        // Downloads kept: the title row they belong to stays too.
        let keep_dl = p.selected(Choice { downloads: false, ..Choice::ALL });
        assert!(keep_dl.downloads.is_empty() && keep_dl.download_bytes == 0 && keep_dl.titles.is_empty());
        // Nothing ticked: nothing removed.
        assert!(p.selected(Choice { downloads: false, library: false, titles: false, files: false }).is_empty());
    }

    #[test]
    fn a_title_kept_in_the_library_is_not_deleted() {
        let s = Session { started_at: 0, library_adds: vec!["m".into()], titles: vec!["m".into()], ..Default::default() };
        let mut f = Fake::default();
        f.library.insert("m".into());
        f.titles.insert("m".into());
        let p = plan(&s, &f);
        assert_eq!(p.selected(Choice::ALL).titles, vec!["m".to_string()]);
        assert!(p.selected(Choice { library: false, ..Choice::ALL }).titles.is_empty());
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(820 * 1024), "820 KB");
        assert_eq!(human_bytes(120 * 1024 * 1024), "120 MB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024 + 400 * 1024 * 1024), "3.4 GB");
    }
}
