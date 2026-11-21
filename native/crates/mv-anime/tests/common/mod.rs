//! Shared test fixtures (design 11.1): a synthetic manga built in a temp folder, with no image,
//! audio or video file in the repository. Every test binary that needs it adds `mod common;`.
//!
//! The manga: [`CHAPTERS`] chapters x [`PAGES`] pages x [`PANELS`] panels, the three
//! [`CHARACTERS`], a few lines per panel (speech, thought, narration, sfx), page scenes whose kind
//! changes half-way through each chapter (dialogue, then fight), Studio's Read of every chapter as
//! `mv_recap::ChapterRead` written to `<root>/studio/projects/m1/c<n>/studio/read.json`, page PNGs
//! with coloured panels and face boxes, Studio's panel crops, and the series brain.
//!
//! Only a dedicated test binary may point `MANGAVOX_HOME` at a temp folder ([`set_home`]):
//! `mv_core::paths::root` is fixed for the life of the process (one test file = one process).
//!
//! These helpers are used by several engineers' tests: keep their names and meaning stable.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use image::{Rgb, RgbImage};
use mv_anime::ChapterPick;
use mv_recap::{Bbox, Brain, ChapterRead, ChapterRef, CharacterSeen, PageRead, Panel};

/// The manga id of the synthetic series.
pub const MANGA_ID: &str = "m1";
/// Its title.
pub const MANGA_TITLE: &str = "Test Manga";
/// Chapters in the series.
pub const CHAPTERS: usize = 2;
/// Pages per chapter.
pub const PAGES: usize = 4;
/// Panels per page (three horizontal bands).
pub const PANELS: usize = 3;
/// Page size in pixels.
pub const PAGE_W: u32 = 600;
pub const PAGE_H: u32 = 900;

/// (brain id, name, alias, signature colour) of the three characters.
pub const CHARACTERS: [(&str, &str, &str, [u8; 3]); 3] =
    [("kai", "Kai", "The Hunter", [200, 40, 40]), ("ren", "Ren", "Shadow", [40, 60, 200]), ("ama", "Ama", "Teacher", [40, 160, 60])];

/// Chapter id of chapter `n` (0-based): "c1", "c2".
pub fn chapter_id(n: usize) -> String {
    format!("c{}", n + 1)
}

/// A fresh temp folder that is removed when dropped.
pub fn temp_root() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("mv-anime-test-").tempdir().unwrap_or_else(|e| panic!("temp folder: {e}"))
}

/// Points the data root at `root` for this process. Call it before anything reads
/// `mv_core::paths::root()`, and only from a test binary that owns its process.
pub fn set_home(root: &Path) {
    std::env::set_var("MANGAVOX_HOME", root);
}

/// One synthetic chapter, written to disk.
#[derive(Clone, Debug)]
pub struct Chapter {
    pub chref: ChapterRef,
    pub read: ChapterRead,
    /// What an episode stores for it (absolute dirs, valid whatever the data root is).
    pub pick: ChapterPick,
    /// Page files in reading order.
    pub pages: Vec<PathBuf>,
}

/// The synthetic manga, written under `root`.
#[derive(Clone, Debug)]
pub struct Manga {
    pub root: PathBuf,
    pub chapters: Vec<Chapter>,
    pub brain: Brain,
}

impl Manga {
    /// Every chapter's pick, in order (for an `Episode`).
    pub fn picks(&self) -> Vec<ChapterPick> {
        self.chapters.iter().map(|c| c.pick.clone()).collect()
    }
}

/// Writes the whole synthetic manga under `root`: pages, panel crops, Read files and the brain.
pub fn build(root: &Path) -> Manga {
    let chapters = (0..CHAPTERS).map(|n| write_chapter(root, n, true)).collect();
    let brain = brain();
    let dir = root.join("studio").join(MANGA_ID);
    write(&dir.join("brain.json"), &serde_json::to_vec_pretty(&brain).unwrap_or_default());
    Manga { root: root.to_path_buf(), chapters, brain }
}

/// Like [`build`] without any picture (pages and panel crops): faster when a test needs only text.
pub fn build_text_only(root: &Path) -> Manga {
    let chapters = (0..CHAPTERS).map(|n| write_chapter(root, n, false)).collect();
    let brain = brain();
    write(&root.join("studio").join(MANGA_ID).join("brain.json"), &serde_json::to_vec_pretty(&brain).unwrap_or_default());
    Manga { root: root.to_path_buf(), chapters, brain }
}

/// The series brain: the three characters with aliases and a description.
pub fn brain() -> Brain {
    let mut b = Brain::default();
    b.manga_id = MANGA_ID.into();
    b.manga_title = MANGA_TITLE.into();
    b.characters = CHARACTERS
        .iter()
        .map(|(id, name, alias, _)| mv_recap::Character {
            id: id.to_string(),
            name: name.to_string(),
            aliases: vec![alias.to_string()],
            description: format!("{name}, a recurring character of the test manga."),
            traits: vec!["brave".into()],
            appearances: (CHAPTERS * PAGES) as u32,
            first_seen: Some((chapter_id(0), 0)),
            ..Default::default()
        })
        .collect();
    b
}

/// Panel `k`'s box on every page: three full-width horizontal bands with a small gutter.
pub fn panel_bbox(k: usize) -> Bbox {
    let h = 1.0 / PANELS as f32;
    Bbox([0.04, k as f32 * h + 0.02, 0.96, (k + 1) as f32 * h - 0.02])
}

/// Characters (indices into [`CHARACTERS`]) seen in panel `k` of page `page`.
pub fn cast_of(page: usize, k: usize) -> Vec<usize> {
    match (page + k) % 3 {
        0 => vec![0],
        1 => vec![0, 1],
        _ => vec![1, 2],
    }
}

/// Face box of the `slot`-th character seen in panel `k` (page-relative, like Studio's).
pub fn face_bbox(k: usize, slot: usize) -> Bbox {
    let p = panel_bbox(k).0;
    let ph = p[3] - p[1];
    let x0 = 0.12 + slot as f32 * 0.42;
    Bbox([x0, p[1] + ph * 0.2, x0 + 0.2, p[1] + ph * 0.75])
}

/// Scene kind of a page: the first half of each chapter is talk, the second a fight.
pub fn page_kind(page: usize) -> &'static str {
    if page < PAGES / 2 {
        "dialogue"
    } else {
        "fight"
    }
}

/// Studio's Read of chapter `n` (0-based), without touching the disk.
pub fn chapter_read(n: usize) -> ChapterRead {
    let pages = (0..PAGES)
        .map(|p| {
            let kind = page_kind(p);
            let panels = (0..PANELS)
                .map(|k| {
                    let cast = cast_of(p, k);
                    let names: Vec<&str> = cast.iter().map(|&i| CHARACTERS[i].1).collect();
                    let mut lines = vec![];
                    if k == 0 && p == 0 {
                        lines.push(line(None, &format!("Chapter {}. The tower rises over the city.", n + 1), "narration"));
                    }
                    for (slot, name) in names.iter().enumerate() {
                        let kind = if slot == 1 && kind == "dialogue" { "thought" } else { "speech" };
                        lines.push(line(Some(name), &format!("{name} speaks on page {} panel {}.", p + 1, k + 1), kind));
                    }
                    if kind == "fight" && k == PANELS - 1 {
                        lines.push(line(None, "CRASH", "sfx"));
                    }
                    Panel {
                        bbox: Some(panel_bbox(k)),
                        description: format!("{} in panel {} of page {}.", names.join(" and "), k + 1, p + 1),
                        characters: cast
                            .iter()
                            .enumerate()
                            .map(|(slot, &i)| CharacterSeen { name: CHARACTERS[i].1.into(), face: Some(face_bbox(k, slot)), is_new: n == 0 && p == 0 })
                            .collect(),
                        lines,
                    }
                })
                .collect();
            PageRead {
                index: p,
                file: page_name(p),
                summary: format!("Chapter {} page {}: {}.", n + 1, p + 1, if kind == "fight" { "the fight breaks out" } else { "they talk" }),
                scene: mv_recap::Scene {
                    kind: kind.into(),
                    mood: if kind == "fight" { "tense" } else { "calm" }.into(),
                    intensity: if kind == "fight" { 8 } else { 3 },
                    tags: vec![],
                },
                panels,
            }
        })
        .collect();
    ChapterRead {
        chapter_id: chapter_id(n),
        pages,
        model: "test-model".into(),
        cast_checked: true,
        cast: CHARACTERS.iter().map(|(_, name, _, _)| (name.to_string(), format!("{name} of the test manga"))).collect(),
        ..Default::default()
    }
}

/// A page picture: white paper, grey panel bands, a coloured face box per character seen.
pub fn page_image(page: usize) -> RgbImage {
    let mut img = RgbImage::from_pixel(PAGE_W, PAGE_H, Rgb([250, 250, 245]));
    for k in 0..PANELS {
        fill(&mut img, panel_bbox(k), Rgb([200, 200, 200]));
        for (slot, &i) in cast_of(page, k).iter().enumerate() {
            fill(&mut img, face_bbox(k, slot), Rgb(CHARACTERS[i].3));
            // A darker stripe so crops have some detail (sharpness scores are not zero).
            let f = face_bbox(k, slot).0;
            let eyes = Bbox([f[0] + 0.03, f[1] + (f[3] - f[1]) * 0.35, f[2] - 0.03, f[1] + (f[3] - f[1]) * 0.42]);
            fill(&mut img, eyes, Rgb([20, 20, 20]));
        }
    }
    img
}

fn page_name(p: usize) -> String {
    format!("page{:02}.png", p + 1)
}

fn line(speaker: Option<&str>, text: &str, kind: &str) -> mv_recap::Line {
    mv_recap::Line { speaker: speaker.map(str::to_string), text: text.into(), kind: kind.into() }
}

fn fill(img: &mut RgbImage, b: Bbox, c: Rgb<u8>) {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let [x0, y0, x1, y1] = b.0;
    for y in (y0 * h) as u32..((y1 * h) as u32).min(img.height()) {
        for x in (x0 * w) as u32..((x1 * w) as u32).min(img.width()) {
            img.put_pixel(x, y, c);
        }
    }
}

fn write(path: &Path, bytes: &[u8]) {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
    }
    std::fs::write(path, bytes).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn write_chapter(root: &Path, n: usize, pictures: bool) -> Chapter {
    let id = chapter_id(n);
    let pages_dir = root.join("library").join("manga").join(MANGA_ID).join(&id);
    let output_dir = root.join("studio").join("projects").join(MANGA_ID).join(&id);
    let chref = ChapterRef {
        chapter_id: id.clone(),
        manga_id: MANGA_ID.into(),
        manga_title: MANGA_TITLE.into(),
        label: format!("Chapter {}", n + 1),
        pages_dir: pages_dir.clone(),
        output_dir: output_dir.clone(),
    };
    let read = chapter_read(n);
    write(&output_dir.join("studio").join("read.json"), &serde_json::to_vec_pretty(&read).unwrap_or_default());
    std::fs::create_dir_all(&pages_dir).unwrap_or_else(|e| panic!("create {}: {e}", pages_dir.display()));
    let mut pages = vec![];
    for p in 0..PAGES {
        let file = pages_dir.join(page_name(p));
        if pictures {
            let img = page_image(p);
            img.save(&file).unwrap_or_else(|e| panic!("save {}: {e}", file.display()));
            for k in 0..PANELS {
                let b = panel_bbox(k).0;
                let (x, y) = ((b[0] * PAGE_W as f32) as u32, (b[1] * PAGE_H as f32) as u32);
                let (w, h) = (((b[2] - b[0]) * PAGE_W as f32) as u32, ((b[3] - b[1]) * PAGE_H as f32) as u32);
                let crop = image::imageops::crop_imm(&img, x, y, w, h).to_image();
                let out = mv_recap::panel_path(&chref, p, k);
                if let Some(dir) = out.parent() {
                    std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
                }
                crop.save(&out).unwrap_or_else(|e| panic!("save {}: {e}", out.display()));
            }
        }
        pages.push(file);
    }
    let pick = ChapterPick {
        chapter_id: id,
        manga_id: MANGA_ID.into(),
        label: chref.label.clone(),
        number: Some((n + 1) as f64),
        pages_dir: pages_dir.to_string_lossy().replace('\\', "/"),
        output_dir: output_dir.to_string_lossy().replace('\\', "/"),
    };
    Chapter { chref, read, pick, pages }
}
