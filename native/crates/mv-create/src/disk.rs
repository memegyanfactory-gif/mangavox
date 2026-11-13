//! What a project keeps on disk and how to delete it: a chapter's video, clips, voices and scene
//! pictures (so it can be made again from any step), one scene's clip, or every character sheet.
//! Everything lives in the project's own folder, in `pages/`, `panels/`, `clips/`, `voice/`,
//! `video/` and `sheets/`; nothing outside that folder is ever touched.
use std::path::{Path, PathBuf};

use crate::project::Project;

/// The folders a chapter's made work is kept in, inside the project folder, with what each holds.
pub const MADE_DIRS: [(&str, &str); 5] = [("pages", "Page pictures"), ("panels", "Scene pictures"), ("clips", "Clips"), ("voice", "Voices"), ("video", "Finished videos")];
/// The folder character sheets, expression sheets and place pictures are kept in.
pub const SHEETS_DIR: &str = "sheets";

/// How much of a chapter's made work to delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clear {
    /// The video, its clips and their voices. The scene pictures stay.
    Film,
    /// The scene pictures too. The written scenes stay.
    Pictures,
    /// Everything, the written scenes included: the chapter starts over.
    All,
}

/// What a delete came to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cleared {
    pub bytes: u64,
    pub files: usize,
    /// Files that could not be removed (open in a player, say). They stay on disk and are found
    /// again by the next delete.
    pub kept: usize,
}

impl std::ops::AddAssign for Cleared {
    fn add_assign(&mut self, o: Cleared) {
        self.bytes += o.bytes;
        self.files += o.files;
        self.kept += o.kept;
    }
}

/// Bytes and number of files of a file, or of everything in a folder.
pub fn disk_use(path: &Path) -> (u64, usize) {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return (0, 0) };
    if !meta.is_dir() {
        return (meta.len(), 1);
    }
    std::fs::read_dir(path).into_iter().flatten().flatten().map(|e| disk_use(&e.path())).fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1))
}

fn remove(paths: &[PathBuf]) -> Cleared {
    let mut c = Cleared::default();
    for f in paths {
        let (bytes, files) = disk_use(f);
        let gone = if f.is_dir() { std::fs::remove_dir_all(f) } else { std::fs::remove_file(f) };
        match gone {
            Ok(()) => {
                c.bytes += bytes;
                c.files += files;
            }
            Err(_) if !f.exists() => {}
            Err(_) => c.kept += files.max(1),
        }
    }
    c
}

fn list(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect()
}

/// Whether the file or folder `name` in the project's `sub/` folder was made for chapter `n`
/// (`pictures`: the scene pictures count too, not only what the film is made of). The names are
/// the ones the jobs give: `ch01-p003-2-<time>.png`, `ch1-work`, `<title>-chapter-1-<time>.mp4`.
fn of_chapter(sub: &str, name: &str, n: u32, pictures: bool) -> bool {
    let stem = format!("ch{n:02}-p");
    match sub {
        "clips" => name.starts_with(&stem) || name == format!("ch{n}-work"),
        "voice" => name.starts_with(&stem),
        "video" => name.strip_suffix(".mp4").and_then(|s| s.rsplit_once('-')).is_some_and(|(head, at)| !at.is_empty() && at.bytes().all(|b| b.is_ascii_digit()) && head.ends_with(&format!("-chapter-{n}"))),
        "panels" => name.starts_with(&stem) && (pictures || name.ends_with("-crop.png")),
        "pages" => pictures && name.starts_with(&stem),
        _ => false,
    }
}

/// Whether `name` in `voice/` is a scene's voice folder (`ch01-p003-2-<time>`), not one of the
/// cast's own voices, which are kept there too.
fn scene_voice(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("ch") else { return false };
    let digits = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
    digits > 0 && rest[digits..].starts_with("-p")
}

/// `rel` as a file inside `dir`, when it is one of the made-work folders' files.
fn made(dir: &Path, rel: Option<&String>) -> Option<PathBuf> {
    let rel = Path::new(rel?);
    let mut parts = rel.components();
    let first = parts.next()?;
    let inside = rel.components().all(|c| matches!(c, std::path::Component::Normal(_)));
    let known = MADE_DIRS.iter().any(|(d, _)| first.as_os_str() == *d);
    (inside && known && parts.next().is_some()).then(|| dir.join(rel)).filter(|f| f.exists())
}

impl Project {
    /// The files and folders on disk that `what` covers of chapter `n`: what the chapter points
    /// at, and every earlier version of it (a redo keeps the old file; they are found by name).
    pub fn chapter_files(&self, n: u32, what: Clear) -> Vec<PathBuf> {
        self.chapter_files_in(&self.dir(), n, what)
    }

    fn chapter_files_in(&self, dir: &Path, n: u32, what: Clear) -> Vec<PathBuf> {
        let pictures = what != Clear::Film;
        let mut out: Vec<PathBuf> = vec![];
        for (sub, _) in MADE_DIRS {
            out.extend(list(&dir.join(sub)).into_iter().filter(|f| f.file_name().is_some_and(|s| of_chapter(sub, &s.to_string_lossy(), n, pictures))));
        }
        if let Some(ch) = self.chapter(n) {
            let mut named: Vec<Option<&String>> = vec![ch.video.as_ref()];
            for pg in &ch.pages {
                if pictures {
                    named.push(pg.image.as_ref());
                }
                for x in &pg.panels {
                    named.push(x.clip.as_ref());
                    if pictures {
                        named.push(x.image.as_ref());
                    }
                }
            }
            for f in named.into_iter().filter_map(|r| made(dir, r)) {
                if !out.contains(&f) {
                    out.push(f);
                }
            }
        }
        // A file another chapter points at too is not this chapter's alone: it stays.
        let shared: std::collections::HashSet<PathBuf> = self.chapters.iter().filter(|c| c.number != n).flat_map(|c| c.video.iter().chain(c.pages.iter().flat_map(|pg| pg.image.iter().chain(pg.panels.iter().flat_map(|x| x.image.iter().chain(x.clip.iter())))))).filter_map(|r| made(dir, Some(r))).collect();
        out.retain(|f| !shared.contains(f));
        out
    }

    /// Bytes and files of what `what` covers of chapter `n`.
    pub fn chapter_disk(&self, n: u32, what: Clear) -> (u64, usize) {
        self.chapter_files(n, what).iter().map(|f| disk_use(f)).fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1))
    }

    /// Deletes what `what` covers of chapter `n`, from disk and from the project (which the
    /// caller saves). The chapter itself and its summary stay, so it can be made again.
    pub fn clear_chapter(&mut self, n: u32, what: Clear) -> Cleared {
        let dir = self.dir();
        self.clear_chapter_in(&dir, n, what)
    }

    fn clear_chapter_in(&mut self, dir: &Path, n: u32, what: Clear) -> Cleared {
        let done = remove(&self.chapter_files_in(dir, n, what));
        if let Some(ch) = self.chapters.iter_mut().find(|c| c.number == n) {
            ch.video = None;
            if what == Clear::All {
                ch.pages.clear();
            }
            for pg in &mut ch.pages {
                if what == Clear::Pictures {
                    pg.image = None;
                    pg.model.clear();
                }
                for x in &mut pg.panels {
                    x.clip = None;
                    if what == Clear::Pictures {
                        x.image = None;
                    }
                }
            }
        }
        done
    }

    /// Deletes one scene's clip and its voice (every version of them), so it is made again.
    pub fn clear_clip(&mut self, n: u32, page: u32, i: usize) -> Cleared {
        let dir = self.dir();
        self.clear_clip_in(&dir, n, page, i)
    }

    fn clear_clip_in(&mut self, dir: &Path, n: u32, page: u32, i: usize) -> Cleared {
        let stem = format!("ch{n:02}-p{page:03}-{}-", i + 1);
        let mut files: Vec<PathBuf> = ["clips", "voice"].iter().flat_map(|sub| list(&dir.join(sub))).filter(|f| f.file_name().is_some_and(|s| s.to_string_lossy().starts_with(&stem))).collect();
        let panel = self.chapters.iter_mut().find(|c| c.number == n).and_then(|c| c.pages.iter_mut().find(|pg| pg.number == page)).and_then(|pg| pg.panels.get_mut(i));
        if let Some(x) = panel {
            if let Some(f) = made(dir, x.clip.as_ref()).filter(|f| !files.contains(f)) {
                files.push(f);
            }
            x.clip = None;
        }
        remove(&files)
    }

    /// Everything on disk made for any chapter: all of `pages/`, `panels/`, `clips/` and `video/`,
    /// and the scenes' voices (the cast's own voices in `voice/` stay).
    fn chapters_files_in(dir: &Path) -> Vec<PathBuf> {
        MADE_DIRS.iter().flat_map(|(sub, _)| list(&dir.join(sub)).into_iter().filter(move |f| *sub != "voice" || f.file_name().is_some_and(|s| scene_voice(&s.to_string_lossy())))).collect()
    }

    /// Bytes and files of everything made for every chapter.
    pub fn chapters_disk(&self) -> (u64, usize) {
        Self::chapters_files_in(&self.dir()).iter().map(|f| disk_use(f)).fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1))
    }

    /// Deletes everything made for every chapter, the written scenes included: the cast, their
    /// sheets and the story stay, and every chapter starts over.
    pub fn clear_chapters(&mut self) -> Cleared {
        let dir = self.dir();
        self.clear_chapters_in(&dir)
    }

    fn clear_chapters_in(&mut self, dir: &Path) -> Cleared {
        let done = remove(&Self::chapters_files_in(dir));
        for ch in &mut self.chapters {
            ch.video = None;
            ch.pages.clear();
        }
        done
    }

    /// Deletes every character sheet, expression sheet and place picture.
    pub fn clear_sheets(&mut self) -> Cleared {
        let dir = self.dir();
        self.clear_sheets_in(&dir)
    }

    fn clear_sheets_in(&mut self, dir: &Path) -> Cleared {
        let done = remove(&list(&dir.join(SHEETS_DIR)));
        for c in &mut self.cast {
            c.sheet = None;
            c.expressions = None;
        }
        for l in &mut self.locations {
            l.sheet = None;
        }
        done
    }

    /// Bytes and files of the character sheets, expression sheets and place pictures.
    pub fn sheets_disk(&self) -> (u64, usize) {
        disk_use(&self.dir().join(SHEETS_DIR))
    }

    /// Bytes of everything in the project folder.
    pub fn disk(&self) -> u64 {
        disk_use(&self.dir()).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{CastMember, Chapter, Location, Page, Panel};

    /// A project folder with two made chapters (1 and 10) and one sheet; returns the project.
    fn made_project(dir: &Path) -> Project {
        let put = |rel: &str| {
            let f = dir.join(rel);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(&f, b"12345678").unwrap();
            rel.to_string()
        };
        let chapter = |n: u32, put: &dyn Fn(&str) -> String| Chapter {
            number: n,
            summary: "What happens".into(),
            pages: vec![Page {
                number: 1,
                image: Some(put(&format!("pages/ch{n:02}-p001-200.png"))),
                model: "Nano Banana".into(),
                panels: (1..=2).map(|i| Panel { image: Some(put(&format!("panels/ch{n:02}-p001-{i}-200.png"))), clip: Some(put(&format!("clips/ch{n:02}-p001-{i}-300.mp4"))), notes: "slower".into(), ..Default::default() }).collect(),
                ..Default::default()
            }],
            video: Some(put(&format!("video/my-manga-chapter-{n}-400.mp4"))),
            ..Default::default()
        };
        let mut p = Project { title: "My manga".into(), ..Default::default() };
        p.chapters = vec![chapter(1, &put), chapter(10, &put)];
        // Older versions a redo left behind, a clip's cropped still, its voice and stitch folders.
        for rel in ["pages/ch01-p001-100.png", "clips/ch01-p001-1-250.mp4", "clips/ch01-p001-11-250.mp4", "panels/ch01-p001-1-crop.png", "voice/ch01-p001-1-300/line-1.wav", "clips/ch1-work/list.txt", "video/my-manga-chapter-1-350.mp4", "voice/kai.wav", "refs/kai-1.png"] {
            put(rel);
        }
        p.cast = vec![CastMember { name: "Kai".into(), sheet: Some(put("sheets/kai-sheet-1.png")), expressions: Some(put("sheets/kai-expressions-1.png")), user_refs: vec!["refs/kai-1.png".into()], ..Default::default() }];
        p.locations = vec![Location { name: "Gym".into(), sheet: Some(put("sheets/loc-gym-1.png")), ..Default::default() }];
        p
    }

    fn left(dir: &Path, sub: &str) -> Vec<String> {
        let mut v: Vec<String> = list(&dir.join(sub)).iter().map(|f| f.file_name().unwrap().to_string_lossy().to_string()).collect();
        v.sort();
        v
    }

    #[test]
    fn a_chapters_files_are_told_by_name() {
        for (sub, name, n, pictures, yes) in [
            ("pages", "ch01-p003-170.png", 1, true, true),
            ("pages", "ch01-p003-170.png", 1, false, false),
            ("pages", "ch10-p003-170.png", 1, true, false),
            ("pages", "ch100-p003-170.png", 10, true, false),
            ("pages", "ch100-p003-170.png", 100, true, true),
            ("panels", "ch01-p003-2-crop.png", 1, false, true),
            ("panels", "ch01-p003-2-170.png", 1, false, false),
            ("clips", "ch1-work", 1, false, true),
            ("clips", "ch11-work", 1, false, false),
            ("video", "my-manga-chapter-1-170.mp4", 1, false, true),
            ("video", "my-manga-chapter-11-170.mp4", 1, false, false),
            ("video", "chapter-1-chapter-2-170.mp4", 1, false, false),
            ("video", "my-manga-chapter-1-final.mp4", 1, false, false),
            ("voice", "kai.wav", 1, true, false),
            ("sheets", "ch01-p001.png", 1, true, false),
        ] {
            assert_eq!(of_chapter(sub, name, n, pictures), yes, "{sub}/{name} for chapter {n}");
        }
    }

    #[test]
    fn deleting_a_chapters_film_keeps_its_pictures_and_other_chapters() {
        let t = tempfile::tempdir().unwrap();
        let mut p = made_project(t.path());
        let done = p.clear_chapter_in(t.path(), 1, Clear::Film);
        // Two clips and two older ones, the cropped still, a voice line, the stitch list, two videos.
        assert_eq!((done.files, done.bytes, done.kept), (9, 72, 0));
        assert_eq!(left(t.path(), "clips"), ["ch10-p001-1-300.mp4", "ch10-p001-2-300.mp4"]);
        assert_eq!(left(t.path(), "video"), ["my-manga-chapter-10-400.mp4"]);
        assert_eq!(left(t.path(), "voice"), ["kai.wav"]);
        assert_eq!(left(t.path(), "pages"), ["ch01-p001-100.png", "ch01-p001-200.png", "ch10-p001-200.png"]);
        let ch = p.chapter(1).unwrap();
        assert!(ch.video.is_none() && ch.pages[0].image.is_some() && ch.pages[0].panels.iter().all(|x| x.clip.is_none() && x.image.is_some() && x.notes == "slower"));
        assert!(p.chapter(10).unwrap().video.is_some());
    }

    #[test]
    fn deleting_pictures_keeps_the_scenes_and_everything_starts_the_chapter_over() {
        let t = tempfile::tempdir().unwrap();
        let mut p = made_project(t.path());
        p.clear_chapter_in(t.path(), 1, Clear::Pictures);
        assert_eq!(left(t.path(), "pages"), ["ch10-p001-200.png"]);
        assert_eq!(left(t.path(), "panels"), ["ch10-p001-1-200.png", "ch10-p001-2-200.png"]);
        let ch = p.chapter(1).unwrap();
        assert!(ch.pages.len() == 1 && ch.pages[0].panels.len() == 2 && ch.pages[0].image.is_none() && ch.pages[0].model.is_empty() && ch.pages[0].panels.iter().all(|x| x.image.is_none()));
        let done = p.clear_chapter_in(t.path(), 10, Clear::All);
        assert_eq!(done.files, 6);
        let ch = p.chapter(10).unwrap();
        assert!(ch.pages.is_empty() && ch.video.is_none() && ch.summary == "What happens");
        for (sub, _) in MADE_DIRS {
            assert_eq!(left(t.path(), sub), if sub == "voice" { vec!["kai.wav".to_string()] } else { vec![] }, "{sub}");
        }
        // The sheets and the user's own pictures are not a chapter's.
        assert_eq!((left(t.path(), SHEETS_DIR).len(), left(t.path(), "refs").len()), (3, 1));
        // Nothing left to delete: nothing happens.
        assert_eq!(p.clear_chapter_in(t.path(), 10, Clear::All), Cleared::default());
    }

    #[test]
    fn one_clip_or_every_sheet_can_go() {
        let t = tempfile::tempdir().unwrap();
        let mut p = made_project(t.path());
        // Scene 1's clip, its older version and its voice; scene 11's and scene 2's stay.
        assert_eq!(p.clear_clip_in(t.path(), 1, 1, 0).files, 3);
        assert_eq!(left(t.path(), "clips"), ["ch01-p001-11-250.mp4", "ch01-p001-2-300.mp4", "ch1-work", "ch10-p001-1-300.mp4", "ch10-p001-2-300.mp4"]);
        let ch = p.chapter(1).unwrap();
        assert!(ch.pages[0].panels[0].clip.is_none() && ch.pages[0].panels[1].clip.is_some() && ch.video.is_some());
        assert_eq!(p.clear_sheets_in(t.path()).files, 3);
        assert!(p.cast[0].sheet.is_none() && p.cast[0].expressions.is_none() && p.locations[0].sheet.is_none() && p.cast[0].user_refs.len() == 1);
        assert!(t.path().join("refs/kai-1.png").is_file());
    }

    #[test]
    fn every_chapter_can_start_over_and_the_cast_keeps_its_voices() {
        let t = tempfile::tempdir().unwrap();
        let mut p = made_project(t.path());
        std::fs::write(t.path().join("voice/chika.wav"), b"x").unwrap();
        // Chapter 1: 2 pages, 2 panels and a crop, 4 clips and the stitch list, a voice line, 2 videos; chapter 10: 6.
        assert_eq!(p.clear_chapters_in(t.path()).files, 19);
        for (sub, _) in MADE_DIRS {
            assert_eq!(left(t.path(), sub), if sub == "voice" { vec!["chika.wav".to_string(), "kai.wav".to_string()] } else { vec![] }, "{sub}");
        }
        assert!(p.chapters.iter().all(|c| c.pages.is_empty() && c.video.is_none() && !c.summary.is_empty()));
        assert!(p.cast[0].sheet.is_some() && left(t.path(), SHEETS_DIR).len() == 3);
    }

    #[test]
    fn a_stored_path_outside_the_made_folders_is_never_deleted() {
        let t = tempfile::tempdir().unwrap();
        let keep = t.path().join("project.json");
        std::fs::write(&keep, b"{}").unwrap();
        std::fs::create_dir_all(t.path().join("sheets")).unwrap();
        std::fs::write(t.path().join("sheets/kai.png"), b"x").unwrap();
        let mut p = Project::default();
        let page = Page { number: 1, image: Some("project.json".into()), panels: vec![Panel { image: Some("sheets/kai.png".into()), clip: Some("../x.mp4".into()), ..Default::default() }], ..Default::default() };
        p.chapters = vec![Chapter { number: 1, pages: vec![page], video: Some(keep.to_string_lossy().to_string()), ..Default::default() }];
        assert_eq!(p.clear_chapter_in(t.path(), 1, Clear::All), Cleared::default());
        assert!(keep.is_file() && t.path().join("sheets/kai.png").is_file());
    }

    #[test]
    fn a_picture_two_chapters_point_at_stays() {
        let t = tempfile::tempdir().unwrap();
        let mut p = made_project(t.path());
        // Chapter 10 shows chapter 1's page picture as well as its own.
        p.chapters[1].pages.push(Page { number: 2, image: Some("pages/ch01-p001-200.png".into()), ..Default::default() });
        p.clear_chapter_in(t.path(), 1, Clear::All);
        assert_eq!(left(t.path(), "pages"), ["ch01-p001-200.png", "ch10-p001-200.png"]);
        assert!(p.chapter(1).unwrap().pages.is_empty());
    }
}
