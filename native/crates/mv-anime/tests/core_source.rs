//! Source adapters against a synthetic chapter in a temp data folder (design 3.2, 3.3, 6.1).
//! This test binary points `MANGAVOX_HOME` at its own temp folder before anything reads the
//! data root (`mv_core::paths::root` is fixed per process).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use mv_anime::source::{chapter_ref, digest, digest_page, page_file, panel_file, pick_from, read, read_progress, resolve_dir, store_dir, DIGEST_CAP};
use mv_recap::{ChapterRead, ChapterRef, CharacterSeen, Line, PageRead, Panel, Scene};

/// The temp data root of this test binary (created once, kept for the whole run).
fn home() -> &'static Path {
    static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
    HOME.get_or_init(|| {
        let dir = tempfile::tempdir().expect("temp folder");
        std::env::set_var("MANGAVOX_HOME", dir.path());
        std::env::set_var("MANGAVOX_LOG_DIR", dir.path().join("logs"));
        dir
    })
    .path()
}

fn page(index: usize, kind: &str, summary: &str, panels: Vec<Panel>) -> PageRead {
    PageRead { index, file: format!("page{}.jpg", index + 1), summary: summary.into(), scene: Scene { kind: kind.into(), mood: "tense".into(), intensity: 6, tags: vec![] }, panels }
}

fn panel(description: &str, chars: &[&str], lines: &[(Option<&str>, &str, &str)]) -> Panel {
    Panel {
        bbox: None,
        description: description.into(),
        characters: chars.iter().map(|c| CharacterSeen { name: (*c).into(), face: None, is_new: false }).collect(),
        lines: lines.iter().map(|(s, kind, text)| Line { speaker: s.map(Into::into), text: (*text).into(), kind: (*kind).into() }).collect(),
    }
}

/// A chapter of 4 pages, 3 of them read (stored out of order).
fn synthetic_read() -> ChapterRead {
    ChapterRead {
        chapter_id: "c1".into(),
        pages: vec![
            page(
                1,
                "fight",
                "Jin dodges.",
                vec![panel("Jin rolls aside", &["Jin"], &[(None, "sfx", "WHOOSH")]), panel("The beast roars", &["Beast"], &[])],
            ),
            page(
                0,
                "dialogue",
                "Jin and Mira\nenter the gate.",
                vec![
                    panel("Two hunters at a glowing gate", &["Jin", "Mira"], &[(Some("Mira"), "speech", "Stay  close."), (Some("Jin"), "thought", "Here we go.")]),
                    panel("Close on the gate", &[], &[(None, "narration", "The gate hums."), (Some("Jin"), "speech", "  ")]),
                ],
            ),
            page(2, "calm", "Silence.", vec![]),
        ],
        model: "test".into(),
        ..ChapterRead::default()
    }
}

/// Lays out `<home>/lib-test/m1/c1/{pages, pipeline/studio/read.json}` once; returns the chapter.
fn chapter() -> ChapterRef {
    static CH: OnceLock<ChapterRef> = OnceLock::new();
    CH.get_or_init(lay_out).clone()
}

fn lay_out() -> ChapterRef {
    let root = home();
    let base = root.join("lib-test").join("m1").join("c1");
    let pages = base.join("pages");
    let out = base.join("pipeline");
    std::fs::create_dir_all(&pages).expect("pages folder");
    // Natural order: page1, page2, page3, page10 (contents do not matter to the listing).
    for name in ["page1.jpg", "page2.jpg", "page10.jpg", "page3.png", "notes.txt"] {
        std::fs::write(pages.join(name), b"x").expect("page file");
    }
    let studio = out.join("studio");
    std::fs::create_dir_all(&studio).expect("studio folder");
    std::fs::write(studio.join("read.json"), serde_json::to_vec(&synthetic_read()).expect("json")).expect("read.json");
    ChapterRef { chapter_id: "c1".into(), manga_id: "m1".into(), manga_title: "Test Manga".into(), label: "Chapter 1".into(), pages_dir: pages, output_dir: out }
}

#[test]
fn dirs_are_stored_relative_to_the_data_root_when_inside_it() {
    let root = home();
    let inside = root.join("lib-test").join("m1").join("c1").join("pages");
    assert_eq!(store_dir(&inside), "lib-test/m1/c1/pages");
    assert_eq!(resolve_dir("lib-test/m1/c1/pages"), inside);
    let elsewhere = tempfile::tempdir().expect("temp folder");
    let outside = elsewhere.path().join("x");
    assert_eq!(store_dir(&outside), outside.to_string_lossy());
    assert_eq!(resolve_dir(&store_dir(&outside)), outside);
    assert_eq!(store_dir(Path::new("")), "");
    assert_eq!(resolve_dir(""), PathBuf::new());
}

#[test]
fn a_pick_round_trips_to_the_chapter_studio_knows() {
    let ch = chapter();
    let pick = pick_from(&ch, Some(1.0));
    assert_eq!(pick.pages_dir, "lib-test/m1/c1/pages");
    assert_eq!(pick.output_dir, "lib-test/m1/c1/pipeline");
    assert_eq!((pick.chapter_id.as_str(), pick.manga_id.as_str(), pick.label.as_str(), pick.number), ("c1", "m1", "Chapter 1", Some(1.0)));
    assert_eq!(chapter_ref(&pick, "Test Manga"), ch);
}

#[test]
fn read_pages_and_panels_come_through_studio() {
    let ch = chapter();
    let pick = pick_from(&ch, None);
    let r = read(&pick, "Test Manga").expect("the Read is found");
    assert_eq!(r, synthetic_read());
    assert_eq!(read_progress(&pick, "Test Manga"), (3, 4), "3 of 4 pages read");
    assert_eq!(page_file(&pick, 2).and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).as_deref(), Some("page3.png"));
    assert_eq!(page_file(&pick, 3).and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).as_deref(), Some("page10.jpg"));
    assert_eq!(page_file(&pick, 4), None);
    assert_eq!(panel_file(&pick, "Test Manga", 2, 1), ch.output_dir.join("studio").join("panels").join("p0002_01.jpg"));
    // A chapter never read.
    let mut unread = pick.clone();
    unread.output_dir = "lib-test/m1/none".into();
    assert_eq!(read(&unread, "Test Manga"), None);
    assert_eq!(read_progress(&unread, "Test Manga"), (0, 4));
}

#[test]
fn digest_of_a_synthetic_chapter() {
    let r = synthetic_read();
    let p0 = "[p0] scene=dialogue/tense/6 | Jin and Mira enter the gate.\n  (k0) Two hunters at a glowing gate | chars: Jin, Mira\n    Mira [speech]: Stay close.\n    Jin [thought]: Here we go.\n  (k1) Close on the gate\n    ? [narration]: The gate hums.\n";
    let p1 = "[p1] scene=fight/tense/6 | Jin dodges.\n  (k0) Jin rolls aside | chars: Jin\n    ? [sfx]: WHOOSH\n  (k1) The beast roars | chars: Beast\n";
    let p2 = "[p2] scene=calm/tense/6 | Silence.\n";
    assert_eq!(digest_page(&r.pages[1]), p0);
    // Pages in reading order, whatever order the Read stored them in.
    assert_eq!(digest(&r, DIGEST_CAP), format!("{p0}{p1}{p2}"));
    // Cut on a page boundary.
    let cap = p0.chars().count() + p1.chars().count() + 3;
    assert_eq!(digest(&r, cap), format!("{p0}{p1}"));
    // A first page longer than the cap alone is cut, never dropped.
    assert_eq!(digest(&r, 10), p0.chars().take(10).collect::<String>());
    assert_eq!(digest(&ChapterRead::default(), DIGEST_CAP), "");
    assert_eq!(DIGEST_CAP, 60_000);
}
