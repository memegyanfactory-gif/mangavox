//! The project store (design 5.1, 5.2, 5.6): atomic writes, history and restore, recovery of a
//! damaged file, content-addressed takes, project listing and the engine settings.
//!
//! This binary owns its process: the data root points at a temp folder for all its tests.
//! Tests run in parallel, so each uses its own project folder.

mod common;

use std::path::Path;
use std::sync::OnceLock;

use mv_anime::store::{self, list_projects, read_json, take_warnings, version, write_json, HISTORY_KEEP, NEWER};
use mv_anime::*;
use serde_json::{json, Value};

static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();

/// Sets the data root once for the whole binary (before anything reads it).
fn home() -> &'static Path {
    HOME.get_or_init(|| {
        let dir = common::temp_root();
        common::set_home(dir.path());
        dir
    })
    .path()
}

fn project(id: &str) -> Store {
    let root = home();
    let s = Store::open(id);
    assert!(s.dir.starts_with(root), "the store must live under the test root");
    assert_eq!(s.dir, root.join("anime").join("projects").join(id));
    s
}

fn story(spoken: &str) -> Story {
    Story {
        episode: "ep01".into(),
        scenes: vec![Scene { id: "sc0001".into(), title: "Rooftop".into(), ..Default::default() }],
        lines: vec![Line { id: "ln0001".into(), scene: "sc0001".into(), spoken: spoken.into(), ..Default::default() }],
        model: "test".into(),
        ..Default::default()
    }
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    v.sort();
    v
}

fn raw(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap_or_default()).unwrap_or(Value::Null)
}

#[test]
fn atomic_write_replaces_whole_files_and_leaves_no_temp_files() {
    let s = project("p_atomic");
    let file = s.path("episodes/ep01/story.json");
    let before = version();
    s.save_story(&story("one")).unwrap_or_else(|e| panic!("{e:#}"));
    assert!(version() > before, "a save bumps the version");
    assert_eq!(files_in(file.parent().unwrap_or(&s.dir)), vec!["story.json"], "no temp file left behind");
    let v = raw(&file);
    assert_eq!(v["schema"], json!(SCHEMA), "the schema is stamped on save");
    assert_eq!(v["lines"][0]["spoken"], "one");

    // The same content again: nothing written, no snapshot, no version bump from this file.
    let mtime = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
    s.save_story(&story("one")).unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(std::fs::metadata(&file).and_then(|m| m.modified()).ok(), mtime);
    assert!(s.history("episodes/ep01/story.json").is_empty());

    // Plain write_json outside any project works and keeps no history.
    let loose = home().join("anime").join("loose-test.json");
    write_json(&loose, &json!({"a": 1})).unwrap_or_else(|e| panic!("{e:#}"));
    write_json(&loose, &json!({"a": 2})).unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(read_json::<Value>(&loose).unwrap_or_default(), json!({"a": 2}));
    assert!(!home().join("anime").join("history").exists());

    // A missing file reads as the default.
    assert_eq!(read_json::<Story>(&s.path("episodes/none/story.json")).unwrap_or_else(|e| panic!("{e:#}")), Story::default());
}

#[test]
fn every_entity_saves_and_loads_and_keeps_fields_from_a_newer_app() {
    let s = project("p_entities");
    let series = Series { id: "p_entities".into(), manga_id: "m1".into(), manga_title: "Test".into(), created_at: 10, ..Default::default() };
    s.save_series(&series).unwrap_or_else(|e| panic!("{e:#}"));
    let back = s.series().unwrap_or_default();
    assert_eq!(back.schema, SCHEMA);
    assert_eq!(back.manga_title, "Test");

    let ep = Episode { id: "ep01".into(), title: "One".into(), chapters: vec![ChapterPick { chapter_id: "c1".into(), manga_id: "m1".into(), label: "Chapter 1".into(), number: Some(1.0), ..Default::default() }], ..Default::default() };
    s.save_episode(&ep).unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(s.episode("ep01").unwrap_or_default().chapters, ep.chapters);
    assert_eq!(s.episode("ep02").unwrap_or_default().id, "ep02", "a missing episode reads empty with its id");

    s.save_story(&story("hello")).unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(s.story("ep01").unwrap_or_default().lines[0].spoken, "hello");

    for scene in ["sc0001", "sc0002"] {
        let shots = SceneShots { episode: "ep01".into(), scene: scene.into(), shots: vec![Shot { id: format!("sh_{scene}"), ..Default::default() }], ..Default::default() };
        s.save_scene_shots(&shots).unwrap_or_else(|e| panic!("{e:#}"));
    }
    assert_eq!(s.scene_shots("ep01", "sc0002").unwrap_or_default().shots[0].id, "sh_sc0002");
    let all = s.all_shots("ep01").unwrap_or_default();
    assert_eq!(all.keys().cloned().collect::<Vec<_>>(), vec!["sc0001", "sc0002"]);
    assert!(s.all_shots("ep09").unwrap_or_default().is_empty());

    let issues = vec![Issue { code: "axis".into(), severity: Severity::Error, scene: "sc0001".into(), ..Default::default() }];
    s.save_checks("ep01", &issues).unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(s.checks("ep01").unwrap_or_default(), issues);
    assert!(s.checks("ep09").unwrap_or_default().is_empty());

    s.save_character(&Character { id: "kai".into(), name: "Kai".into(), ..Default::default() }).unwrap_or_else(|e| panic!("{e:#}"));
    s.save_character(&Character { id: "ama".into(), name: "Ama".into(), ..Default::default() }).unwrap_or_else(|e| panic!("{e:#}"));
    s.save_place(&Place { id: "pl_roof".into(), name: "Roof".into(), ..Default::default() }).unwrap_or_else(|e| panic!("{e:#}"));
    s.save_prop(&Prop { id: "sword".into(), name: "Sword".into(), ..Default::default() }).unwrap_or_else(|e| panic!("{e:#}"));
    s.save_style(&Style { line: "thin lines".into(), ..Default::default() }).unwrap_or_else(|e| panic!("{e:#}"));
    s.save_cast(&VoiceCast { narrator: CharVoice { voice_id: "af_heart".into(), ..Default::default() }, ..Default::default() }).unwrap_or_else(|e| panic!("{e:#}"));
    let bible = s.bible().unwrap_or_default();
    assert_eq!(bible.characters.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["Ama", "Kai"]);
    assert_eq!(bible.character("kai").map(|c| c.schema), Some(SCHEMA));
    assert!(bible.place("pl_roof").is_some() && bible.prop("sword").is_some());
    assert_eq!(bible.style.line, "thin lines");
    assert_eq!(bible.cast.narrator.voice_id, "af_heart");

    // An entity without an id is refused with a message, not written to a wrong file.
    let err = s.save_character(&Character::default()).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(err.contains("no id"), "{err}");

    // A field written by a newer app survives a load and save through this one.
    let file = s.path("bible/characters/kai.json");
    let mut v = raw(&file);
    v["panoramas"] = json!([{"id": "pano1"}]);
    v["look"]["hair"] = json!("silver");
    std::fs::write(&file, serde_json::to_vec_pretty(&v).unwrap_or_default()).unwrap_or_else(|e| panic!("{e}"));
    let mut kai = s.bible().unwrap_or_default().character("kai").cloned().unwrap_or_default();
    assert_eq!(kai.look.hair, "silver");
    kai.name = "Kai Two".into();
    s.save_character(&kai).unwrap_or_else(|e| panic!("{e:#}"));
    let v = raw(&file);
    assert_eq!(v["panoramas"], json!([{"id": "pano1"}]));
    assert_eq!(v["name"], "Kai Two");

    // Paths inside the project.
    assert_eq!(s.rel(&s.path("takes/ab/x.png")).as_deref(), Some("takes/ab/x.png"));
    assert_eq!(s.rel(&home().join("elsewhere.png")), None);
    assert_eq!(s.work_dir(), s.dir.join("work"));
}

#[test]
fn a_file_from_a_newer_app_opens_but_is_never_overwritten() {
    let s = project("p_newer");
    let file = s.path("episodes/ep01/story.json");
    std::fs::create_dir_all(file.parent().unwrap_or(&s.dir)).unwrap_or_else(|e| panic!("{e}"));
    let newer = json!({"schema": 9, "episode": "ep01", "scenes": [{"id": "sc1", "kind": "space_opera", "hologram": true}], "voices_v2": {}});
    std::fs::write(&file, serde_json::to_vec(&newer).unwrap_or_default()).unwrap_or_else(|e| panic!("{e}"));
    let st = s.story("ep01").unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(st.schema, 9);
    assert!(store::is_newer(st.schema));
    assert_eq!(st.scenes[0].kind, SceneKind::Unknown);
    let err = s.save_story(&st).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(err.contains(NEWER), "{err}");
    assert_eq!(raw(&file), newer, "untouched");

    // Unreadable by this build and newer: an error saying so, and the file is not set aside.
    let broken = json!({"schema": 9, "episode": "ep01", "scenes": "now a map"});
    std::fs::write(&file, serde_json::to_vec(&broken).unwrap_or_default()).unwrap_or_else(|e| panic!("{e}"));
    let err = s.story("ep01").err().map(|e| e.to_string()).unwrap_or_default();
    assert!(err.contains(NEWER), "{err}");
    assert_eq!(raw(&file), broken);
    assert_eq!(files_in(file.parent().unwrap_or(&s.dir)), vec!["story.json"]);
}

#[test]
fn history_snapshots_and_restore() {
    let s = project("p_history");
    let rel = "episodes/ep01/story.json";
    s.save_story(&story("v1")).unwrap_or_else(|e| panic!("{e:#}"));
    s.save_story(&story("v2")).unwrap_or_else(|e| panic!("{e:#}"));
    s.save_story(&story("v3")).unwrap_or_else(|e| panic!("{e:#}"));
    let h = s.history(rel);
    assert_eq!(h.len(), 2, "one snapshot per replaced version: {h:?}");
    assert!(h[0] > h[1], "newest first");
    assert!(s.path("history/episodes__ep01__story.json").is_dir(), "history/<rel with / -> __>/");

    // Restore the oldest (v1): the current v3 is snapshotted first.
    s.restore(rel, h[1]).unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(s.story("ep01").unwrap_or_default().lines[0].spoken, "v1");
    let h2 = s.history(rel);
    assert_eq!(h2.len(), 3);
    s.restore(rel, h2[0]).unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(s.story("ep01").unwrap_or_default().lines[0].spoken, "v3", "restoring is itself undoable");

    // Bad requests answer readable errors.
    assert!(s.restore(rel, 12345).is_err());
    assert!(s.restore("../../outside.json", h[0]).is_err());
    assert!(s.history("../x.json").is_empty());

    // Only the newest HISTORY_KEEP snapshots stay.
    for i in 0..(HISTORY_KEEP + 5) {
        s.save_story(&story(&format!("many {i}"))).unwrap_or_else(|e| panic!("{e:#}"));
    }
    assert_eq!(s.history(rel).len(), HISTORY_KEEP);

    // Derived files keep no history.
    s.save_checks("ep01", &[]).unwrap_or_else(|e| panic!("{e:#}"));
    s.save_checks("ep01", &[Issue::default()]).unwrap_or_else(|e| panic!("{e:#}"));
    assert!(s.history("episodes/ep01/checks.json").is_empty());
}

#[test]
fn a_save_that_changes_nothing_real_keeps_no_snapshot() {
    let s = project("p_history_same");
    let rel = "episodes/ep01/story.json";
    s.save_story(&story("v1")).unwrap_or_else(|e| panic!("{e:#}"));
    // Only the derived check findings differ: no new version.
    let mut checked = story("v1");
    checked.extra.insert("issues".into(), json!([{ "code": "x", "scene": "sc0001" }]));
    s.save_story(&checked).unwrap_or_else(|e| panic!("{e:#}"));
    assert!(s.history(rel).is_empty(), "{:?}", s.history(rel));
    // A real change keeps the version before it, once.
    s.save_story(&story("v2")).unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(s.history(rel).len(), 1);
    // A tall page's preview is the top of the page at the asked shape, made once.
    let page = home().join("tall-page.png");
    image::RgbImage::from_pixel(100, 1000, image::Rgb([200, 30, 30])).save(&page).unwrap_or_else(|e| panic!("{e}"));
    let thumb = store::page_thumb(&page, 54, 76).unwrap_or_else(|e| panic!("{e:#}"));
    let made = image::open(&thumb).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((made.width(), made.height()), (54, 76));
    assert_eq!(store::page_thumb(&page, 54, 76).ok(), Some(thumb));
    assert!(store::page_thumb(&home().join("no-such-page.png"), 54, 76).is_err());
    assert!(!store::local_when(1_700_000_000).is_empty());
    assert_eq!(store::local_stamp(1_700_000_000).len(), "20231114-221320".len());
}

/// All warnings are checked in this one test (the warning list is shared by the process).
#[test]
fn a_damaged_file_is_set_aside_and_restored_from_history_with_a_warning() {
    let s = project("p_corrupt");
    let rel = "episodes/ep01/story.json";
    let file = s.path(rel);
    s.save_story(&story("good one")).unwrap_or_else(|e| panic!("{e:#}"));
    s.save_story(&story("good two")).unwrap_or_else(|e| panic!("{e:#}"));
    // Damaged on disk (a crash in another program, a bad sync, a hand edit).
    std::fs::write(&file, b"{\"episode\": \"ep01\", \"scenes\": [ {\"id\": ").unwrap_or_else(|e| panic!("{e}"));
    let _ = take_warnings();

    let st = s.story("ep01").unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(st.lines[0].spoken, "good one", "the newest snapshot came back");
    assert_eq!(s.story("ep01").unwrap_or_default().lines[0].spoken, "good one", "and it is on disk again");
    let names = files_in(file.parent().unwrap_or(&s.dir));
    assert!(names.iter().any(|n| n.starts_with("story.json.corrupt-")), "the damaged copy is kept: {names:?}");
    let w = take_warnings();
    assert!(w.iter().any(|m| m.contains(rel) && m.contains("damaged") && m.contains("put back")), "{w:?}");

    // Wrong types in valid JSON count as damage too.
    std::fs::write(&file, br#"{"episode": "ep01", "lines": "not a list"}"#).unwrap_or_else(|e| panic!("{e}"));
    let st = s.story("ep01").unwrap_or_else(|e| panic!("{e:#}"));
    assert!(!st.lines.is_empty());

    // A number saved as null (it was not finite) is no damage: it reads as 0.
    std::fs::write(&file, br#"{"schema": 1, "episode": "ep01", "scenes": [{"id": "sc1", "target_s": null, "intensity": null}]}"#).unwrap_or_else(|e| panic!("{e}"));
    let before = files_in(file.parent().unwrap_or(&s.dir)).len();
    let st = s.story("ep01").unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(st.scenes[0].target_s, 0.0);
    assert_eq!(files_in(file.parent().unwrap_or(&s.dir)).len(), before, "nothing set aside");

    // No history at all: it starts empty, the damaged copy is kept, the user is told.
    let lonely = s.path("bible/style.json");
    std::fs::create_dir_all(lonely.parent().unwrap_or(&s.dir)).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(&lonely, b"\x00\x01 not json").unwrap_or_else(|e| panic!("{e}"));
    let style: Style = read_json(&lonely).unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(style, Style::default());
    assert!(files_in(lonely.parent().unwrap_or(&s.dir)).iter().any(|n| n.starts_with("style.json.corrupt-")));
    let w = take_warnings();
    assert!(w.iter().any(|m| m.contains("bible/style.json") && m.contains("starts empty")), "{w:?}");
    assert!(take_warnings().is_empty(), "warnings are handed out once");

    // The screen is told in plain words what was put back, and from when.
    let mine: Vec<store::Recovery> = store::take_recoveries().into_iter().filter(|r| r.project == "p_corrupt").collect();
    assert!(mine.iter().any(|r| r.what == "the story of episode 1" && r.restored.as_deref().is_some_and(|w| w.ends_with("ago"))), "{mine:?}");
    assert!(mine.iter().any(|r| r.what == "the bible's style" && r.restored.is_none()), "{mine:?}");
    assert!(mine.iter().all(|r| !r.what.contains('/')), "never a path: {mine:?}");
    assert!(store::take_recoveries().iter().all(|r| r.project != "p_corrupt"), "handed out once");
    assert_eq!(store::plain_what("bible/cast.json"), "the voices");
    assert_eq!(store::plain_what("episodes/ep01/shots/sc0003.json"), "the shots of a scene");
}

#[test]
fn takes_are_content_addressed_and_deduplicated() {
    let s = project("p_takes");
    let meta = |note: &str| TakeMeta {
        kind: TakeKind::Image,
        target: Target::ShotFrame { episode: "ep01".into(), shot: "sh0001".into() },
        request_hash: "rh".into(),
        source: TakeSource::MagnificWeb { model: "nb".into(), res: "1K".into(), free: true, site_id: "x".into() },
        w: 64,
        h: 36,
        note: note.into(),
        ..Default::default()
    };
    let bytes = b"not really a png, but bytes are bytes".to_vec();
    let a = s.add_take(&bytes, ".PNG", meta("first")).unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(a.id, mv_anime::hash::sha256_hex(&bytes));
    assert_eq!(a.file, format!("takes/{}/{}.png", &a.id[..2], a.id));
    assert_eq!(a.schema, SCHEMA);
    assert!(a.created_at > 1_600_000_000, "unix seconds");
    assert_eq!(std::fs::read(s.path(&a.file)).unwrap_or_default(), bytes);

    // Same bytes again: the same take, nothing rewritten, the first record kept.
    let b = s.add_take(&bytes, "png", meta("second")).unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(b, a);
    assert_eq!(b.note, "first");
    let dir = s.path(&format!("takes/{}", &a.id[..2]));
    assert_eq!(files_in(&dir), vec![format!("{}.json", a.id), format!("{}.png", a.id)]);

    // Different bytes: a different take.
    let c = s.add_take(b"other bytes", "wav", meta("voice")).unwrap_or_else(|e| panic!("{e:#}"));
    assert_ne!(c.id, a.id);
    assert!(c.file.ends_with(".wav"));

    assert_eq!(s.take(&a.id), Some(a.clone()));
    assert_eq!(s.take_file(&a.id), Some(s.path(&a.file)));
    // The record is lost: the file is still found by its id.
    std::fs::remove_file(dir.join(format!("{}.json", a.id))).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(s.take(&a.id), None);
    assert_eq!(s.take_file(&a.id), Some(s.path(&a.file)));
    // ... and storing the bytes again writes the record back.
    let d = s.add_take(&bytes, "png", meta("again")).unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(d.id, a.id);
    assert!(s.take(&a.id).is_some());

    // Never a path from an id; never an empty take.
    assert_eq!(s.take("../../../etc/passwd"), None);
    assert_eq!(s.take_file("zz"), None);
    assert!(s.add_take(b"", "png", meta("empty")).is_err());
}

#[test]
fn projects_are_listed_newest_first_without_removed_ones() {
    let old = project("p_list_old");
    let new = project("p_list_new");
    old.save_series(&Series { id: "p_list_old".into(), manga_id: "m_old".into(), created_at: 100, ..Default::default() }).unwrap_or_else(|e| panic!("{e:#}"));
    new.save_series(&Series { id: "p_list_new".into(), manga_id: "m_new".into(), created_at: 200, ..Default::default() }).unwrap_or_else(|e| panic!("{e:#}"));
    let removed = project(".removed-p_list_gone");
    removed.save_series(&Series { id: "gone".into(), created_at: 300, ..Default::default() }).unwrap_or_else(|e| panic!("{e:#}"));
    std::fs::create_dir_all(project("p_list_empty_folder").dir).unwrap_or_else(|e| panic!("{e}"));

    let ids: Vec<String> = list_projects().into_iter().map(|s| s.id).collect();
    let pos = |id: &str| ids.iter().position(|x| x == id);
    assert!(pos("p_list_new").is_some() && pos("p_list_old").is_some(), "{ids:?}");
    assert!(pos("p_list_new") < pos("p_list_old"), "newest first: {ids:?}");
    assert!(pos("gone").is_none() && pos("p_list_empty_folder").is_none(), "{ids:?}");
    assert_eq!(Store::for_manga("m/1 x").id, "m_1_x");
}

#[test]
fn two_manga_whose_names_clean_alike_get_two_folders() {
    let _ = home();
    // "가 x" and "나 x" both clean to "_ x"-like names.
    let first = Store::for_manga("한국/1");
    let plain = first.id.clone();
    first.save_series(&Series { id: plain.clone(), manga_id: "한국/1".into(), ..Default::default() }).unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(Store::for_manga("한국/1").id, plain, "an existing project keeps its folder");
    let second = Store::for_manga("일본/1");
    assert_ne!(second.id, plain, "another manga never opens the first one's project");
    assert!(second.id.starts_with(&plain));
    second.save_series(&Series { id: second.id.clone(), manga_id: "일본/1".into(), ..Default::default() }).unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(Store::for_manga("일본/1").id, second.id, "and finds its own again");
}

#[test]
fn update_json_never_loses_a_change_made_at_the_same_time() {
    let s = project("p_update");
    s.save_story(&story("start")).unwrap_or_else(|e| panic!("{e:#}"));
    std::thread::scope(|scope| {
        for t in 0..8 {
            let s = &s;
            scope.spawn(move || {
                for i in 0..15 {
                    s.update_story("ep01", |st| st.lines.push(Line { id: format!("t{t}-{i}"), scene: "sc0001".into(), ..Default::default() }))
                        .unwrap_or_else(|e| panic!("{e:#}"));
                }
            });
        }
    });
    let back = s.story("ep01").unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(back.lines.len(), 1 + 8 * 15, "every change landed");
    assert_eq!(back.schema, SCHEMA);
    // The project file: refused while there is none, changed in place once there is.
    assert!(s.update_series(|x| x.manga_title = "x".into()).is_err());
    s.save_series(&Series { id: "p_update".into(), manga_id: "m".into(), ..Default::default() }).unwrap_or_else(|e| panic!("{e:#}"));
    let saved = s.update_series(|x| x.manga_title = "Solo".into()).unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!((saved.manga_title.as_str(), saved.manga_id.as_str()), ("Solo", "m"));
    assert_eq!(s.series().map(|x| x.manga_title).unwrap_or_default(), "Solo");
    assert!(s.update_json::<Value>("../escape.json", |_| {}).is_err());
}

#[test]
fn a_number_that_was_not_finite_reads_as_zero_not_as_damage() {
    let s = project("p_nan");
    let shot = Shot {
        id: "sh0001".into(),
        marks: vec![Mark { character: "jin".into(), pos: [f32::NAN, 1.0], ..Default::default() }],
        camera: Camera { look_at: LookAt::Angles { yaw_deg: f32::INFINITY, pitch_deg: 2.0 }, ..Default::default() },
        ..Default::default()
    };
    s.save_scene_shots(&SceneShots { episode: "ep01".into(), scene: "sc0003".into(), shots: vec![shot], ..Default::default() }).unwrap_or_else(|e| panic!("{e:#}"));
    let _ = take_warnings();
    let back = s.scene_shots("ep01", "sc0003").unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(back.shots.len(), 1, "nothing rolled back");
    assert_eq!(back.shots[0].marks[0].pos, [0.0, 1.0]);
    assert_eq!(back.shots[0].camera.look_at, LookAt::Angles { yaw_deg: 0.0, pitch_deg: 2.0 });
    assert!(take_warnings().is_empty(), "not treated as a damaged file");
    assert!(!files_in(&s.path("episodes/ep01/shots")).iter().any(|n| n.contains("corrupt")));
}

#[test]
fn a_big_take_is_linked_not_read_whole() {
    let s = project("p_take_file");
    let src = s.path("episodes/ep01/exports/animatic-1.mp4");
    std::fs::create_dir_all(src.parent().unwrap_or(&s.dir)).unwrap_or_else(|e| panic!("{e}"));
    let bytes = vec![7u8; 300_000];
    std::fs::write(&src, &bytes).unwrap_or_else(|e| panic!("{e}"));
    let t = s.add_take_file(&src, "mp4", TakeMeta { kind: TakeKind::Video, ..Default::default() }).unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(t.id, mv_anime::hash::sha256_hex(&bytes), "the same id as a take made from the bytes");
    assert_eq!(std::fs::read(s.path(&t.file)).unwrap_or_default(), bytes);
    assert_eq!(s.add_take_file(&src, "mp4", TakeMeta::default()).map(|x| x.id).ok(), Some(t.id.clone()));
    let empty = s.path("episodes/ep01/exports/empty.mp4");
    std::fs::write(&empty, b"").unwrap_or_else(|e| panic!("{e}"));
    assert!(s.add_take_file(&empty, "mp4", TakeMeta::default()).is_err());
}

#[test]
fn engine_settings_load_and_save() {
    let _ = home();
    use mv_anime::settings::{self, Settings};
    assert_eq!(settings::path(), store::root().join("settings.json"));
    let mut s = settings::load();
    assert_eq!(s, Settings::default());
    s.llm_parallel = 0;
    s.extra.insert("future_pref".into(), json!(true));
    for i in 0..30 {
        s.record_eta("frame", i as f32);
    }
    s.record_eta("story", f32::NAN);
    settings::save(&s).unwrap_or_else(|e| panic!("{e:#}"));
    let back = settings::load();
    assert_eq!(back.llm_parallel, 1, "at least one LLM job");
    assert_eq!(back.eta["frame"].len(), mv_anime::settings::ETA_KEEP);
    assert_eq!(back.eta["frame"][0], 10.0, "the newest are kept");
    assert!(!back.eta.contains_key("story"));
    assert_eq!(back.eta_mean("frame"), Some(19.5));
    assert_eq!(back.extra.get("future_pref"), Some(&json!(true)));
    let newer = Settings { schema: 5, ..Default::default() };
    assert!(settings::save(&newer).is_err());
}

#[test]
fn the_synthetic_manga_builds() {
    let root = common::temp_root();
    let m = common::build(root.path());
    assert_eq!(m.chapters.len(), common::CHAPTERS);
    for c in &m.chapters {
        assert_eq!(c.pages.len(), common::PAGES);
        assert!(c.pages.iter().all(|p| p.is_file()));
        assert_eq!(mv_recap::list_pages(&c.chref.pages_dir), c.pages);
        assert_eq!(mv_recap::load_read(&c.chref).as_ref(), Some(&c.read));
        assert!(mv_recap::panel_path(&c.chref, 0, 0).is_file());
        assert_eq!(c.read.pages.iter().map(|p| p.panels.len()).collect::<Vec<_>>(), vec![common::PANELS; common::PAGES]);
    }
    let brain = mv_recap::Brain::load_in(root.path().join("studio").join(common::MANGA_ID), common::MANGA_ID, common::MANGA_TITLE);
    assert_eq!(brain.characters.len(), 3);
    assert!(brain.find("Shadow").is_some());
}

// ---------------------------------------------------------------- locks across a whole edit

#[test]
fn the_bible_lock_keeps_two_edits_apart_and_does_not_wait_for_itself() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let s = project("p_bible_lock");
    s.save_character(&Character { id: "kai".into(), name: "Kai".into(), ..Default::default() }).expect("kai");
    let inside = Arc::new(AtomicBool::new(false));
    let (a, flag) = (s.clone(), inside.clone());
    let first = std::thread::spawn(move || {
        a.with_bible(|| {
            flag.store(true, Ordering::SeqCst);
            // Taken again on the same thread: it runs, it does not wait for itself.
            a.with_bible(|| {
                let mut c = a.bible().expect("bible").character("kai").cloned().expect("kai");
                std::thread::sleep(std::time::Duration::from_millis(150));
                c.look.hair = "black".into();
                a.save_character(&c).expect("save");
            });
            flag.store(false, Ordering::SeqCst);
        })
    });
    while !inside.load(Ordering::SeqCst) {
        std::thread::yield_now();
    }
    // A second read-change-save waits for the first one, so neither change is lost.
    let saw_inside = s.with_bible(|| {
        let busy = inside.load(Ordering::SeqCst);
        let mut c = s.bible().expect("bible").character("kai").cloned().expect("kai");
        c.look.eyes = "grey".into();
        s.save_character(&c).expect("save");
        busy
    });
    first.join().expect("first edit");
    assert!(!saw_inside, "the second edit ran while the first one held the bible");
    let kai = s.bible().expect("bible").character("kai").cloned().expect("kai");
    assert_eq!((kai.look.hair.as_str(), kai.look.eyes.as_str()), ("black", "grey"));
}

#[cfg(feature = "story")]
#[test]
fn ids_handed_out_together_never_repeat_and_keep_the_projects_other_changes() {
    let s = project("p_bump_ids");
    s.save_series(&Series { id: "p_bump_ids".into(), ..Default::default() }).expect("series");
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let s = s.clone();
            std::thread::spawn(move || {
                let mut got = 0;
                let after = mv_anime::story::bump_ids(&s, |c| {
                    let n = c.entry("shot".into()).or_insert(0);
                    *n += 1;
                    got = *n;
                })
                .expect("bump");
                // Another change of the project at the same moment (a new episode).
                s.update_series(|series| series.episodes.push(EpisodeRef { id: format!("ep{i:02}"), title: String::new() })).expect("episode");
                assert!(after.get("shot").copied().unwrap_or(0) >= got);
                got
            })
        })
        .collect();
    let mut got: Vec<u32> = threads.into_iter().map(|t| t.join().expect("thread")).collect();
    got.sort_unstable();
    assert_eq!(got, (1..=8).collect::<Vec<u32>>(), "every id handed out once");
    let series = s.series().expect("series");
    assert_eq!(series.next_ids.get("shot"), Some(&8));
    assert_eq!(series.episodes.len(), 8, "no change of the project was lost");
}