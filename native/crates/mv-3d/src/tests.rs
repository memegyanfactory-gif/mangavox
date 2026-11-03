use std::sync::Mutex;

use mv_ai::{AiProviderId, CancellationToken};
use mv_recap::{Brain, ChapterRead, ChapterRef, ModelChoice, PageRead, Panel};
use serde_json::json;

use crate::ai::{extract_code, Canned};
use crate::library::{Asset, Category, Library};
use crate::plan::{AssetNeed, Episode, ScenePlan};
use crate::stage::{fallback_blocking, parse_blocking, scene_spec, ActorInfo, RenderSettings};

fn model() -> ModelChoice {
    ModelChoice { provider: AiProviderId::ClaudeCode, model: "test".into() }
}

fn chapter() -> (ChapterRef, ChapterRead) {
    let ch = ChapterRef { chapter_id: "c1".into(), manga_id: "m1".into(), manga_title: "Test Manga".into(), label: "Chapter 1".into(), ..Default::default() };
    let page = |i: usize, who: &str| PageRead {
        index: i,
        file: format!("{i:04}.jpg"),
        summary: format!("page {i}"),
        panels: vec![Panel { description: "a gym with dumbbells".into(), characters: vec![mv_recap::CharacterSeen { name: who.into(), ..Default::default() }], ..Default::default() }],
        ..Default::default()
    };
    (ch, ChapterRead { chapter_id: "c1".into(), pages: vec![page(0, "Kang"), page(1, "Kang"), page(2, "Mira")], ..Default::default() })
}

#[test]
fn categories_parse_and_have_folders() {
    assert_eq!(Category::parse("location"), Some(Category::Environments));
    assert_eq!(Category::parse("Characters"), Some(Category::Characters));
    assert_eq!(Category::parse("vehicle"), Some(Category::Vehicles));
    assert_eq!(Category::parse("nonsense"), None);
    let dirs: Vec<&str> = Category::ALL.iter().map(|c| c.dir()).collect();
    assert!(dirs.contains(&"maps") && dirs.contains(&"props") && dirs.contains(&"fx"));
}

#[test]
fn library_matches_same_manga_and_reuses() {
    let tmp = tempfile::tempdir().unwrap();
    let mut lib = Library::open(tmp.path());
    let mut add = |id: &str, name: &str, cat: Category, manga: Option<&str>| {
        let dir = format!("{}/{id}", cat.dir());
        std::fs::create_dir_all(tmp.path().join(&dir)).unwrap();
        std::fs::write(tmp.path().join(&dir).join("asset.blend"), b"x").unwrap();
        lib.put(Asset { id: id.into(), name: name.into(), category: cat, manga_id: manga.map(String::from), dir, ..Default::default() }).unwrap();
    };
    add("gym", "Ganghyeok's Gym", Category::Environments, Some("m1"));
    add("chair", "Wooden chair", Category::Props, None);
    let lib = Library::open(tmp.path());
    assert_eq!(lib.assets.len(), 2, "index saved and reloaded");
    assert_eq!(lib.find_match(Category::Environments, "Ganghyeok Gym", "m1").map(|a| a.id.as_str()), Some("gym"));
    assert!(lib.find_match(Category::Environments, "Ganghyeok Gym", "other-manga").is_none(), "locations belong to their manga");
    assert_eq!(lib.find_match(Category::Props, "wooden chairs", "any").map(|a| a.id.as_str()), Some("chair"), "shared props match across manga");
    assert!(lib.find_match(Category::Props, "throne", "m1").is_none());
    assert_ne!(lib.new_id("Wooden chair", "", Category::Props), "chair");
    assert!(lib.catalog_for("m1").contains("id=gym"));
    assert!(!lib.catalog_for("m2").contains("id=gym"));
}

#[test]
fn breakdown_parse_keeps_consistent_plan() {
    let v = json!({
        "title": "Day One", "logline": "A trainer meets a fighter.",
        "assets": [
            {"key": "gym", "category": "environment", "name": "Gym", "description": "big room", "pages": [0, 1, 99], "interior": true, "map_pos": [2.0, 0.1]},
            {"key": "kang", "category": "character", "name": "Kang"},
            {"key": "mira", "category": "character", "name": "Mira"},
            {"key": "dumbbell", "category": "prop", "name": "Dumbbell", "generic": true},
            {"key": "unused", "category": "prop", "name": "Unused"},
            {"key": "bad", "category": "weird", "name": "Bad"}
        ],
        "scenes": [
            {"title": "B", "pages": [2, 2], "location": "nowhere", "characters": ["mira", "ghost"], "props": []},
            {"title": "A", "pages": [0, 1], "location": "gym", "characters": ["kang"], "props": ["dumbbell"]}
        ]
    });
    let (title, _, needs, scenes) = crate::breakdown::parse(&v, 3).unwrap();
    assert_eq!(title, "Day One");
    assert_eq!(scenes.len(), 2);
    assert_eq!(scenes[0].title, "A", "sorted by page");
    assert_eq!(scenes[1].index, 1);
    assert_eq!(scenes[1].location, "gym", "unknown location falls back to a known one");
    assert_eq!(scenes[1].characters, vec!["mira"], "unknown characters dropped");
    assert!(!needs.iter().any(|n| n.key == "unused" || n.key == "bad"));
    let gym = needs.iter().find(|n| n.key == "gym").unwrap();
    assert_eq!(gym.pages, vec![0, 1], "pages past the chapter dropped");
    assert_eq!(gym.map_pos, Some([1.0, 0.1]), "map position clamped");
}

#[test]
fn cast_counts_canonical_names() {
    let (_, read) = chapter();
    let brain = Brain::default();
    let c = crate::breakdown::cast(&read, &brain, 5);
    assert_eq!(c[0].name, "Kang");
    assert_eq!(c[0].count, 2);
}

fn actors() -> Vec<ActorInfo> {
    vec![
        ActorInfo { key: "kang".into(), name: "Kang".into(), kind: "character", dims: [0.7, 0.4, 1.8], blend: "k.blend".into() },
        ActorInfo { key: "mira".into(), name: "Mira".into(), kind: "character", dims: [0.6, 0.4, 1.6], blend: "m.blend".into() },
    ]
}

#[test]
fn blocking_parse_clamps_and_fills_in() {
    let v = json!({
        "placements": [{"actor": "kang", "pos": [40, -2], "facing": "mira"}, {"actor": "ghost", "pos": [0, 0]}],
        "shots": [
            {"framing": "Over Shoulder", "subject": "mira", "secondary": "kang", "duration": 30,
             "actions": [{"actor": "kang", "type": "Walk_To", "target": "mira", "start": 1, "duration": 99}, {"actor": "ghost", "type": "jump"}],
             "line": {"speaker": "kang", "text": "Hello"}},
            {"framing": "dutch", "subject": "nobody"}
        ]
    });
    let b = parse_blocking(&v, &actors(), 5.0).unwrap();
    assert_eq!(b.placements.len(), 2, "unknown actor dropped, forgotten actor added");
    assert_eq!(b.placements[0].pos, [5.0, -2.0]);
    assert_eq!(b.shots[0].framing, "over_shoulder");
    assert_eq!(b.shots[0].duration, 7.0);
    assert_eq!(b.shots[0].actions.len(), 1);
    assert_eq!(b.shots[0].actions[0].kind, "walk_to");
    assert!(b.shots[0].actions[0].start + b.shots[0].actions[0].duration <= 7.0 + 1e-4);
    assert_eq!(b.shots[1].framing, "medium");
    assert_eq!(b.shots[1].subject, "kang");
    assert!(parse_blocking(&json!({"shots": []}), &actors(), 5.0).is_err());
}

#[test]
fn scene_spec_turns_seconds_into_frames() {
    let scene = ScenePlan { title: "x".into(), time_of_day: "night".into(), ..Default::default() };
    let b = fallback_blocking(&actors(), &scene);
    let rs = RenderSettings { width: 640, height: 360, fps: 24, samples: 8 };
    let spec = scene_spec(&b, &actors(), None, None, &scene, &rs, std::path::Path::new("out"));
    let shots = spec["shots"].as_array().unwrap();
    assert_eq!(shots.len(), 3, "wide plus one per character");
    assert_eq!(shots[0]["frames"], json!([1, 96]));
    assert_eq!(shots[1]["frames"][0], json!(97));
    assert_eq!(spec["time_of_day"], "night");
    assert_eq!(spec["actors"].as_array().unwrap().len(), 2);
    let a = &spec["actions"].as_array().unwrap()[0];
    assert!(a["frames"][0].as_u64().unwrap() >= 97);
}

#[test]
fn code_is_taken_from_the_fence() {
    assert_eq!(extract_code("Sure!\n```python\nbox('a')\n```\nDone"), "box('a')");
    assert_eq!(extract_code("box('b')"), "box('b')");
}

#[test]
fn online_matching_needs_the_main_word() {
    let list = json!({
        "wooden_chair_01": {"name": "Wooden Chair 01", "tags": ["seat", "wood"], "categories": ["furniture"]},
        "office_desk": {"name": "Office Desk", "tags": ["table"], "categories": ["furniture"]}
    });
    assert_eq!(crate::online::best_model(&list, "Old wooden chair", &[]).map(|f| f.id), Some("wooden_chair_01".into()));
    assert!(crate::online::best_model(&list, "Sword", &["weapon".into()]).is_none());
    let skies = json!({"night_a": {"name": "Night", "categories": ["night", "outdoor"], "tags": []}, "day_a": {"name": "Day", "categories": ["skies", "outdoor"], "tags": ["urban"]}});
    assert_eq!(crate::online::best_hdri(&skies, "night", &[]).map(|f| f.id), Some("night_a".into()));
    assert_eq!(crate::online::best_hdri(&skies, "day", &[]).map(|f| f.id), Some("day_a".into()));
    assert!(crate::online::safe_rel("../evil.gltf").is_err());
    assert!(crate::online::safe_rel("textures/a.exe").is_err());
    assert!(crate::online::safe_rel("textures/a.jpg").is_ok());
}

#[test]
fn blender_output_is_parsed() {
    let ok = crate::blender::parse_output("noise\nMV3D_RESULT {\"dims\": [1, 2, 3]}\n", true).unwrap();
    assert_eq!(ok["dims"][2], 3);
    let err = crate::blender::parse_output("MV3D_ERROR {\"error\": \"boom\", \"trace\": \"Traceback: boom\"}", false).unwrap_err();
    assert!(err.to_string().contains("boom"));
    assert!(crate::blender::parse_output("nothing", false).is_err());
    // A result line printed before a failure is not a result.
    assert!(crate::blender::parse_output("MV3D_RESULT {\"dims\": [9, 9, 9]}
boom", false).is_err());
    // The last result wins.
    let last = crate::blender::parse_output("MV3D_RESULT {\"n\": 1}
MV3D_RESULT {\"n\": 2}", true).unwrap();
    assert_eq!(last["n"], 2);
}

#[tokio::test]
async fn breakdown_runs_against_a_canned_model() {
    let (ch, read) = chapter();
    let answer = json!({
        "title": "T", "logline": "L",
        "assets": [{"key": "gym", "category": "environment", "name": "Gym"}, {"key": "kang", "category": "character", "name": "Kang"}],
        "scenes": [{"title": "S", "pages": [0, 2], "location": "gym", "characters": ["kang"]}]
    });
    let llm = Canned(Mutex::new(vec![format!("Here you go:\n```json\n{answer}\n```")]), Mutex::new(vec![]));
    let (_, _, needs, scenes) = crate::breakdown::run(&llm, &model(), &ch, &read, &Brain::default(), "", &CancellationToken::new()).await.unwrap();
    assert_eq!(needs.len(), 2);
    assert_eq!(scenes[0].characters, vec!["kang"]);
    let sent = llm.1.lock().unwrap();
    let prompt = sent[0].messages.last().unwrap().content.clone();
    assert!(prompt.contains("Kang (2)"), "the cast with panel counts goes to the AI");
}

#[test]
fn episode_lists_scene_needs() {
    let ep = Episode {
        assets: vec![AssetNeed { key: "gym".into(), ..Default::default() }, AssetNeed { key: "kang".into(), category: Category::Characters, ..Default::default() }],
        scenes: vec![ScenePlan { location: "gym".into(), characters: vec!["kang".into(), "ghost".into()], ..Default::default() }],
        ..Default::default()
    };
    assert_eq!(ep.scene_needs(0), vec!["gym", "kang"]);
    assert!(!ep.all_approved());
}

#[test]
fn scripts_install_once() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = crate::scripts::install_in(tmp.path()).unwrap();
    for f in ["mv3d.py", "asset_runner.py", "assemble.py"] {
        assert!(dir.join(f).is_file());
    }
}

#[test]
fn two_runs_saving_the_library_keep_both_assets() {
    let tmp = tempfile::tempdir().unwrap();
    let mut a = Library::open(tmp.path());
    let mut b = Library::open(tmp.path());
    let id_a = a.new_id("Sword", "", Category::Props);
    let id_b = b.new_id("Sword", "", Category::Props);
    assert_ne!(id_a, id_b, "the first id is reserved on disk");
    a.put(Asset { id: id_a.clone(), name: "Sword".into(), dir: format!("props/{id_a}"), ..Default::default() }).unwrap();
    b.put(Asset { id: id_b.clone(), name: "Sword".into(), dir: format!("props/{id_b}"), ..Default::default() }).unwrap();
    b.mark_used(&id_a, "c2");
    a.mark_used(&id_a, "c1");
    a.save().unwrap();
    b.save().unwrap();
    let lib = Library::open(tmp.path());
    assert_eq!(lib.assets.len(), 2, "the later save kept the other run's asset");
    let used = &lib.get(&id_a).unwrap().used_in;
    assert!(used.contains(&"c1".to_string()) && used.contains(&"c2".to_string()), "uses from both runs merged: {used:?}");
}

#[test]
fn map_pins_from_both_runs_survive() {
    let tmp = tempfile::tempdir().unwrap();
    let map = |pins: serde_json::Value, v: u32| Asset {
        id: "m-world-map".into(),
        name: "World map".into(),
        category: Category::Maps,
        manga_id: Some("m".into()),
        dir: "maps/m-world-map".into(),
        params: Some(json!({ "locations": pins })),
        version: v,
        ..Default::default()
    };
    let mut a = Library::open(tmp.path());
    let mut b = Library::open(tmp.path());
    b.put(map(json!([{ "name": "Gym" }, { "name": "Arena" }]), 2)).unwrap();
    a.put(map(json!([{ "name": "Gym" }, { "name": "Palace" }]), 2)).unwrap();
    let lib = Library::open(tmp.path());
    let pins = lib.get("m-world-map").unwrap().params.as_ref().unwrap()["locations"].as_array().unwrap().clone();
    let names: Vec<&str> = pins.iter().filter_map(|p| p["name"].as_str()).collect();
    assert_eq!(names.len(), 3, "{names:?}");
    assert!(names.contains(&"Arena") && names.contains(&"Palace"));
}
