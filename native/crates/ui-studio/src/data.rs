//! Downloaded chapters from the library database, and what the pipeline has
//! already produced for each of them on disk.
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::Connection;

/// File names written by the Python pipeline into `<chapter>/pipeline/`.
pub const VIDEO_FILE: &str = "video_final.mp4";
const TEXT_FILES: [&str; 2] = ["script_edited.json", "ocr_results.json"];

#[derive(Clone, Debug, PartialEq)]
pub struct ChapterItem {
    pub id: String,
    pub manga_id: String,
    pub manga_title: String,
    pub number: Option<f64>,
    pub title: Option<String>,
    pub pages_dir: PathBuf,
    pub output_dir: PathBuf,
    pub page_count: usize,
    /// A script or OCR result exists, so text extraction can be skipped.
    pub has_text: bool,
    pub video: Option<PathBuf>,
    pub audio_files: usize,
}

impl ChapterItem {
    pub fn label(&self) -> String {
        chapter_label(self.number, self.title.as_deref())
    }

    pub fn audio_dir(&self) -> PathBuf {
        self.output_dir.join("audio")
    }

    pub fn has_output(&self) -> bool {
        self.video.is_some() || self.audio_files > 0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MangaGroup {
    pub id: String,
    pub title: String,
    /// Cached cover image (or the first downloaded page when there is none).
    pub cover: Option<PathBuf>,
    pub chapters: Vec<ChapterItem>,
}

/// "Chapter 12 · The Duel", "Chapter 3.5", or the bare title.
pub fn chapter_label(number: Option<f64>, title: Option<&str>) -> String {
    let title = title.map(str::trim).filter(|t| !t.is_empty());
    let num = number.map(|n| if n.fract() == 0.0 { format!("Chapter {}", n as i64) } else { format!("Chapter {n}") });
    match (num, title) {
        (Some(n), Some(t)) if !t.to_lowercase().starts_with("chapter") && !t.to_lowercase().starts_with("ch.") => format!("{n} · {t}"),
        (Some(n), _) => n,
        (None, Some(t)) => t.to_string(),
        (None, None) => "Chapter".into(),
    }
}

/// A chapter's Studio folder, `<data>/studio/projects/<manga>/<chapter>`: keyed by ids so it
/// never depends on where the pages sit. Work older versions saved next to the pages is moved
/// in the first time the chapter loads.
pub fn output_dir_for(manga_id: &str, chapter_id: &str, pages_dir: &Path) -> PathBuf {
    let dir = mv_core::paths::studio_chapter(manga_id, chapter_id);
    if !dir.exists() {
        adopt_legacy(&dir, chapter_id, pages_dir);
    }
    dir
}

/// Where older versions kept a chapter's work: `(folder, shared by the manga's chapters)`.
fn legacy_dirs(pages_dir: &Path) -> Vec<(PathBuf, bool)> {
    let parent = pages_dir.parent().unwrap_or(pages_dir);
    match pages_dir.file_name().and_then(|n| n.to_str()) {
        Some(n) if n.eq_ignore_ascii_case("pages") => vec![(parent.join("pipeline"), false)],
        Some(n) => vec![(parent.join("pipeline").join(n), false), (parent.join("pipeline"), true)],
        None => vec![],
    }
}

fn adopt_legacy(dir: &Path, chapter_id: &str, pages_dir: &Path) {
    for (old, shared) in legacy_dirs(pages_dir) {
        if !old.is_dir() {
            continue;
        }
        if !shared {
            if let Err(e) = mv_core::paths::move_dir(&old, dir) {
                eprintln!("[studio] could not move {} to {}: {e:#}", old.display(), dir.display());
            }
            // The old `<manga>/pipeline` goes once its last chapter moved out.
            if let Some(p) = old.parent().filter(|p| p.file_name().is_some_and(|n| n == "pipeline")) {
                let _ = std::fs::remove_dir(p);
            }
            return;
        }
        // Every downloaded chapter of a manga once wrote here: only the chapter the saved read
        // belongs to takes it, and the per-chapter folders inside stay put.
        let owner = std::fs::read(old.join("studio").join("read.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| v.get("chapter_id").and_then(|s| s.as_str()).map(str::to_string));
        if owner.as_deref() != Some(chapter_id) {
            continue;
        }
        let siblings: Vec<std::ffi::OsString> = old.parent().and_then(|p| std::fs::read_dir(p).ok()).map(|rd| rd.flatten().map(|e| e.file_name()).collect()).unwrap_or_default();
        let _ = std::fs::create_dir_all(dir);
        for e in std::fs::read_dir(&old).into_iter().flatten().flatten() {
            if e.path().is_dir() && siblings.contains(&e.file_name()) {
                continue;
            }
            let _ = mv_core::paths::move_dir(&e.path(), &dir.join(e.file_name()));
        }
        return;
    }
}

fn count_files(dir: &Path, exts: &[&str], min_bytes: u64) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.filter_map(|e| e.ok())
        .filter(|e| {
            let p = e.path();
            let ext_ok = p.extension().and_then(|x| x.to_str()).is_some_and(|x| exts.iter().any(|w| x.eq_ignore_ascii_case(w)));
            ext_ok && e.metadata().map(|m| m.is_file() && m.len() >= min_bytes).unwrap_or(false)
        })
        .count()
}

/// True when a script / OCR file holds at least one non-blank text (an empty OCR
/// result from a failed run must not count as "text ready").
fn file_has_text(path: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else { return false };
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(&raw) else { return false };
    items.iter().any(|it| {
        let text = it.get("text").and_then(|t| t.as_str()).is_some_and(|t| !t.trim().is_empty());
        let texts = it
            .get("texts")
            .and_then(|t| t.as_array())
            .is_some_and(|a| a.iter().any(|t| t.as_str().is_some_and(|t| !t.trim().is_empty())));
        text || texts
    })
}

/// Fills the on-disk fields (text, audio, video, page count) of a chapter.
pub fn scan(ch: &mut ChapterItem) {
    let out = &ch.output_dir;
    ch.has_text = TEXT_FILES.iter().any(|f| file_has_text(&out.join(f)));
    let video = out.join(VIDEO_FILE);
    ch.video = std::fs::metadata(&video).ok().filter(|m| m.len() > 0).map(|_| video);
    ch.audio_files = count_files(&out.join("audio"), &["wav", "mp3", "ogg", "m4a"], 100);
    ch.page_count = count_files(&ch.pages_dir, &["jpg", "jpeg", "png", "webp"], 1);
}

static DB: Mutex<Option<Connection>> = Mutex::new(None);

/// Runs `f` on Studio's connection to the shared database (opened and migrated on first use).
/// Blocking: call from `mv_ui::bg::blocking`.
fn with_db<T>(f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    let mut g = DB.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_none() {
        let mut conn = mv_core::db::open(&mv_core::paths::database())?;
        mv_core::db::migrate(&mut conn)?;
        *g = Some(conn);
    }
    f(g.as_ref().expect("opened"))
}

/// Downloaded chapters grouped by manga, in title / chapter order.
pub fn query_chapters(conn: &Connection) -> Result<Vec<MangaGroup>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.manga_id, m.title, c.number, c.title, c.pages_dir, m.cover_path
         FROM chapter c JOIN manga m ON m.id = c.manga_id
         WHERE c.download_status = 'complete' AND c.pages_dir IS NOT NULL AND c.pages_dir != ''
         ORDER BY m.title COLLATE NOCASE, m.id, c.number, c.added_at",
    )?;
    let rows = stmt.query_map([], |r| {
        let pages: String = r.get(5)?;
        let cover: Option<String> = r.get(6)?;
        let pages_dir = PathBuf::from(pages);
        let ch = ChapterItem {
            id: r.get(0)?,
            manga_id: r.get(1)?,
            manga_title: r.get(2)?,
            number: r.get(3)?,
            title: r.get(4)?,
            output_dir: PathBuf::new(),
            pages_dir,
            page_count: 0,
            has_text: false,
            video: None,
            audio_files: 0,
        };
        Ok((ch, cover))
    })?;
    let mut groups: Vec<MangaGroup> = vec![];
    for row in rows {
        let (ch, cover) = row?;
        match groups.last_mut() {
            Some(g) if g.id == ch.manga_id => g.chapters.push(ch),
            _ => groups.push(MangaGroup { id: ch.manga_id.clone(), title: ch.manga_title.clone(), cover: cover.map(PathBuf::from), chapters: vec![ch] }),
        }
    }
    Ok(groups)
}

/// The cover to show for a manga: the library's cached cover, a local cover file, or the
/// first page of its first downloaded chapter.
fn resolve_cover(g: &MangaGroup) -> Option<PathBuf> {
    let raw = g.cover.as_ref().map(|p| p.to_string_lossy().to_string()).filter(|s| !s.is_empty());
    raw.as_deref()
        .and_then(mv_core::covers::cached_cover)
        .or_else(|| raw.map(PathBuf::from).filter(|p| p.is_file()))
        .or_else(|| {
            let ch = g.chapters.first()?;
            let mut pages: Vec<PathBuf> = std::fs::read_dir(&ch.pages_dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
            pages.sort();
            pages.into_iter().next()
        })
}

/// Loads the library and scans every chapter's pipeline folder.
pub fn load() -> Result<Vec<MangaGroup>> {
    let mut groups = with_db(query_chapters)?;
    for g in groups.iter_mut() {
        g.cover = resolve_cover(g);
        for ch in g.chapters.iter_mut() {
            ch.output_dir = output_dir_for(&ch.manga_id, &ch.id, &ch.pages_dir);
            scan(ch);
        }
    }
    Ok(groups)
}

/// Records a finished pipeline stage on the chapter row (as the Electron orchestrator does).
pub fn mark_stage_complete(chapter_id: &str, stage: &str) -> Result<()> {
    use mv_core::repo::chapter;
    with_db(|c| match stage {
        "ocr" => chapter::update_ocr_status(c, chapter_id, "complete"),
        "tts" => chapter::update_tts_status(c, chapter_id, "complete"),
        "video" => chapter::update_video_status(c, chapter_id, "complete"),
        _ => Ok(()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(chapter_label(Some(12.0), Some("The Duel")), "Chapter 12 · The Duel");
        assert_eq!(chapter_label(Some(3.5), None), "Chapter 3.5");
        assert_eq!(chapter_label(Some(4.0), Some("Chapter 4")), "Chapter 4");
        assert_eq!(chapter_label(None, Some("Oneshot")), "Oneshot");
        assert_eq!(chapter_label(None, Some("  ")), "Chapter");
    }

    fn put(p: &Path, body: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn work_next_to_the_pages_moves_into_the_project_folder() {
        let d = tempfile::tempdir().unwrap();
        // Library layout: <chapter>/pages + <chapter>/pipeline.
        let pages = d.path().join("lib/m/c/pages");
        put(&d.path().join("lib/m/c/pipeline/studio/read.json"), r#"{"chapter_id":"c"}"#);
        let dir = d.path().join("proj/c");
        adopt_legacy(&dir, "c", &pages);
        assert!(dir.join("studio/read.json").is_file());
        assert!(!d.path().join("lib/m/c/pipeline").exists());
    }

    #[test]
    fn the_old_shared_folder_goes_only_to_the_chapter_that_wrote_it() {
        let d = tempfile::tempdir().unwrap();
        let manga = d.path().join("dl/m");
        for c in ["ch-1", "ch-2"] {
            std::fs::create_dir_all(manga.join(c)).unwrap();
        }
        put(&manga.join("pipeline/studio/read.json"), r#"{"chapter_id":"ch-2"}"#);
        put(&manga.join("pipeline/audio/p1.wav"), "x");
        // A newer per-chapter folder inside the shared one must stay where it is.
        put(&manga.join("pipeline/ch-1/studio/read.json"), r#"{"chapter_id":"ch-1"}"#);

        let one = d.path().join("proj/ch-1");
        adopt_legacy(&one, "ch-1", &manga.join("ch-1"));
        assert!(one.join("studio/read.json").is_file(), "ch-1 takes its own folder");
        assert!(manga.join("pipeline/studio/read.json").is_file(), "not ch-1's shared read");

        let two = d.path().join("proj/ch-2");
        adopt_legacy(&two, "ch-2", &manga.join("ch-2"));
        assert!(two.join("studio/read.json").is_file() && two.join("audio/p1.wav").is_file());
    }

    #[test]
    fn scan_finds_text_audio_and_video() {
        let dir = tempfile::tempdir().unwrap();
        let pages = dir.path().join("pages");
        let out = dir.path().join("pipeline");
        std::fs::create_dir_all(&pages).unwrap();
        std::fs::create_dir_all(out.join("audio")).unwrap();
        for i in 0..3 {
            std::fs::write(pages.join(format!("{i:03}.jpg")), b"x").unwrap();
        }
        std::fs::write(pages.join("notes.txt"), b"x").unwrap();
        let mut ch = ChapterItem {
            id: "c".into(),
            manga_id: "m".into(),
            manga_title: "M".into(),
            number: Some(1.0),
            title: None,
            pages_dir: pages,
            output_dir: out.clone(),
            page_count: 0,
            has_text: false,
            video: None,
            audio_files: 0,
        };
        scan(&mut ch);
        assert_eq!(ch.page_count, 3);
        assert!(!ch.has_text && !ch.has_output());

        std::fs::write(out.join("ocr_results.json"), br#"[{"page":0,"texts":[" "]}]"#).unwrap();
        scan(&mut ch);
        assert!(!ch.has_text, "blank OCR output is not text");
        std::fs::write(out.join("ocr_results.json"), br#"[{"page":0,"texts":["Hello"]}]"#).unwrap();
        std::fs::write(out.join("audio").join("page_0000.wav"), vec![0u8; 200]).unwrap();
        std::fs::write(out.join("audio").join("page_0001.wav"), b"tiny").unwrap();
        std::fs::write(out.join(VIDEO_FILE), b"mp4").unwrap();
        scan(&mut ch);
        assert!(ch.has_text);
        assert_eq!(ch.audio_files, 1);
        assert_eq!(ch.video.as_deref(), Some(out.join(VIDEO_FILE).as_path()));
    }

    #[test]
    fn query_groups_downloaded_chapters_by_manga() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = mv_core::db::open(&dir.path().join("t.db")).unwrap();
        mv_core::db::migrate(&mut conn).unwrap();
        use mv_core::repo::{chapter, manga};
        let add = |title: &str| {
            manga::add(
                &conn,
                &manga::AddMangaInput {
                    source_plugin: "src".into(),
                    source_id: title.into(),
                    title: title.into(),
                    author: None,
                    artist: None,
                    description: None,
                    cover_path: None,
                    status: None,
                    genres: vec![],
                },
            )
            .unwrap()
        };
        let b = add("Beta");
        let a = add("Alpha");
        let mut ids = vec![];
        for (m, n) in [(&b, 1.0), (&a, 2.0), (&a, 1.0), (&a, 3.0)] {
            let ch = chapter::add(
                &conn,
                &chapter::AddChapterInput {
                    manga_id: m.id.clone(),
                    source_plugin: "src".into(),
                    source_id: format!("{}-{n}", m.title),
                    number: Some(n),
                    title: None,
                    language: None,
                    page_count: None,
                },
            )
            .unwrap();
            ids.push(ch.id);
        }
        // Chapter 3 of Alpha is not downloaded.
        for id in &ids[..3] {
            chapter::update_download_status(&conn, id, "complete", Some(&format!("/lib/{id}/pages"))).unwrap();
        }
        let groups = query_chapters(&conn).unwrap();
        assert_eq!(groups.iter().map(|g| g.title.as_str()).collect::<Vec<_>>(), vec!["Alpha", "Beta"]);
        assert_eq!(groups[0].chapters.iter().map(|c| c.number).collect::<Vec<_>>(), vec![Some(1.0), Some(2.0)]);
        // The Studio folder is picked (and old work adopted) in `load`, keyed by ids.
        assert_eq!(groups[0].chapters[0].pages_dir, PathBuf::from(format!("/lib/{}/pages", ids[2])));
    }
}
