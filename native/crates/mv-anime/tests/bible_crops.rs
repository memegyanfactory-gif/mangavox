#![cfg(feature = "bible")]
//! Reference crops (design 6.2) from generated test pages in a temp folder: face, bust and panel
//! crops cut from the full-resolution pages, scored, and the two-signal rule (a crop is used as
//! a reference only when the AI's pick and the Read's panel listing agree, or the user approved
//! it; otherwise it is a suggestion).
//!
//! This test binary points `MANGAVOX_HOME` at its own temp folder (one process, one data root).

mod common;

use std::sync::OnceLock;

use mv_anime::bible::EXTRA_BEST_PANELS;
use mv_anime::crops::{character_cover, merge_crops, merge_cut, panel_crops, pick, pick_cut, place_cover, place_cover_file, place_crops, usable, EXTRA_COVERS, KEEP, MIN_SIDE};
use mv_anime::model::{Character, CropFraming, Place, RefCrop, SourceRef, Target, TakeSource};
use mv_anime::Store;
use mv_recap::{ChapterRead, ChapterRef};
use serde_json::json;

fn manga() -> &'static common::Manga {
    static M: OnceLock<(tempfile::TempDir, common::Manga)> = OnceLock::new();
    &M.get_or_init(|| {
        let dir = common::temp_root();
        common::set_home(dir.path());
        std::env::set_var("MANGAVOX_LOG_DIR", dir.path().join("logs"));
        let m = common::build(dir.path());
        (dir, m)
    })
    .1
}

/// A project in this binary's data root (the root is set before the store reads it).
fn project(id: &str) -> Store {
    manga();
    Store::open(id)
}

fn reads() -> Vec<(ChapterRef, ChapterRead)> {
    manga().chapters.iter().map(|c| (c.chref.clone(), c.read.clone())).collect()
}

fn character(i: usize, picks: serde_json::Value) -> Character {
    let (id, name, alias, _) = common::CHARACTERS[i];
    let mut c = Character { id: id.into(), brain_id: Some(id.into()), name: name.into(), aliases: vec![alias.into()], ..Default::default() };
    c.extra.insert(EXTRA_BEST_PANELS.into(), picks);
    c
}

fn at(crop: &RefCrop) -> (String, usize, Option<usize>) {
    (crop.source.chapter_id.clone(), crop.source.page, crop.source.panel)
}

fn open(store: &Store, crop: &RefCrop) -> image::RgbImage {
    let file = store.path(&crop.file);
    assert!(file.is_file(), "the crop's take file exists: {}", file.display());
    image::open(&file).unwrap_or_else(|e| panic!("open {}: {e}", file.display())).to_rgb8()
}

#[test]
fn crops_follow_the_two_signal_rule() {
    let store = project("crops_signals");
    // Page 0 panel 0 lists Kai (cast_of(0, 0) = [Kai]); page 1 panel 1 lists Ren and Ama only.
    let kai = character(0, json!([
        {"chapter": "c1", "page": 0, "panel": 0, "angle": "front", "framing": "face"},
        {"chapter": "c1", "page": 1, "panel": 1, "angle": "side", "framing": "full"},
        {"chapter": "c1", "page": 99, "panel": 0}
    ]));
    let crops = pick(&store, &kai, &reads());
    assert!(!crops.is_empty() && crops.len() <= KEEP, "{} crops", crops.len());

    let both: Vec<&RefCrop> = crops.iter().filter(|c| at(c) == ("c1".into(), 0, Some(0))).collect();
    assert!(!both.is_empty(), "the panel both signals name is cut");
    assert!(both.iter().all(|c| c.agreed == 2 && usable(c)), "two signals agree: usable as a reference");

    let ai_only: Vec<&RefCrop> = crops.iter().filter(|c| at(c) == ("c1".into(), 1, Some(1))).collect();
    assert!(!ai_only.is_empty(), "the AI-only panel is offered");
    assert!(ai_only.iter().all(|c| c.agreed == 1 && !usable(c)), "one signal: a suggestion, not a reference");
    assert!(ai_only.iter().all(|c| c.framing == CropFraming::Full), "no face box there: the whole panel");

    let listed_only = crops.iter().filter(|c| at(c) != ("c1".into(), 0, Some(0)) && at(c) != ("c1".into(), 1, Some(1))).collect::<Vec<_>>();
    assert!(listed_only.iter().all(|c| c.agreed == 1 && !c.approved), "the Read alone is one signal");
    // Agreed crops come first.
    let first_suggestion = crops.iter().position(|c| c.agreed < 2).unwrap_or(crops.len());
    assert!(crops[first_suggestion..].iter().all(|c| c.agreed < 2), "agreed crops are listed first");

    for c in &crops {
        let img = open(&store, c);
        assert!(img.width().min(img.height()) >= MIN_SIDE, "short side {}", img.width().min(img.height()));
        let take = store.take(&c.take).expect("the take record");
        assert_eq!(take.target, Target::Crop { owner: "kai".into() });
        assert!(matches!(take.source, TakeSource::MangaCrop(SourceRef { .. })));
        assert!(c.score > 0.);
    }
    // The face crop is cut around Kai's face: a good share of it is Kai's colour.
    let face = both.iter().find(|c| c.framing == CropFraming::Face).expect("a face crop");
    let img = open(&store, face);
    let red = common::CHARACTERS[0].3;
    let near = img.pixels().filter(|p| p.0.iter().zip(red).all(|(a, b)| (*a as i32 - b as i32).abs() <= 30)).count();
    let share = near as f32 / (img.width() * img.height()) as f32;
    assert!(share >= 0.08, "Kai's face fills {share:.3} of the face crop");
    assert!(crops.iter().map(|c| &c.take).collect::<std::collections::BTreeSet<_>>().len() == crops.len(), "no take twice");
}

#[test]
fn without_face_boxes_whole_panels_stand_in() {
    let store = project("crops_panels");
    let mut rs = reads();
    for (_, read) in &mut rs {
        for page in &mut read.pages {
            for panel in &mut page.panels {
                for seen in &mut panel.characters {
                    seen.face = None;
                }
            }
        }
    }
    let ren = character(1, json!([]));
    let crops = pick(&store, &ren, &rs);
    assert!(!crops.is_empty(), "panels that list Ren are cut");
    assert!(crops.iter().all(|c| c.framing == CropFraming::Full), "only panel crops");
    assert!(crops.iter().all(|c| !usable(c)), "listing alone is a suggestion");
}

#[test]
fn a_character_never_seen_gets_no_crops() {
    let store = project("crops_none");
    let ghost = Character { id: "ghost".into(), name: "Nobody".into(), ..Default::default() };
    assert!(pick(&store, &ghost, &reads()).is_empty());
}

#[test]
fn panel_crops_keep_the_signal_count_and_merging_keeps_the_users_choices() {
    let store = project("crops_place");
    let picks = vec![
        (SourceRef { chapter_id: "c1".into(), page: 0, panel: Some(1) }, 2),
        (SourceRef { chapter_id: "c2".into(), page: 2, panel: Some(0) }, 1),
        (SourceRef { chapter_id: "c2".into(), page: 2, panel: Some(0) }, 1),
        (SourceRef { chapter_id: "c9".into(), page: 0, panel: Some(0) }, 2),
    ];
    let crops = panel_crops(&store, "pl_tower", &picks, &reads());
    assert_eq!(crops.len(), 2, "duplicates and unknown chapters are skipped");
    assert_eq!(crops[0].agreed, 2);
    assert!(usable(&crops[0]) && !usable(&crops[1]));
    assert!(crops.iter().all(|c| c.framing == CropFraming::Full));

    let mut old = crops.clone();
    old[1].approved = true;
    let fresh = panel_crops(&store, "pl_tower", &picks[..1], &reads());
    let merged = merge_crops(&old, fresh.clone());
    assert!(merged.iter().any(|c| c.take == old[1].take && c.approved), "an approved crop stays");
    assert!(merged.iter().any(|c| c.take == fresh[0].take));
    assert_eq!(merge_crops(&old, Vec::new()), old, "an empty cut keeps what was there");
}

#[test]
fn a_local_re_cut_keeps_what_the_user_accepted_dropped_or_uploaded() {
    let store = project("crops_recut");
    let kai = character(0, json!([{"chapter": "c1", "page": 0, "panel": 0, "angle": "front", "framing": "face"}]));
    let first = pick(&store, &kai, &reads());
    assert!(first.len() >= 3, "{} crops", first.len());
    // The user accepts one, drops one, uploads one, and leaves the rest.
    let mut mine = first.clone();
    mine[0].approved = true;
    mine[0].score = 123.;
    let dropped = mine.remove(1);
    let upload = RefCrop { file: "takes/up/own.png".into(), take: "own".into(), approved: true, score: 1., ..Default::default() };
    mine.insert(0, upload.clone());
    // The same cut again (same pages, same boxes): the same pictures.
    let again = pick_cut(&store, &kai, &reads());
    assert!(again.iter().all(|(_, existed)| *existed), "every picture was cut for Kai before");
    let merged = merge_cut(&mine, again);
    assert_eq!(merged[0], upload, "an upload stays exactly as it is");
    assert!(merged.iter().any(|c| c.take == mine[1].take && c.approved && c.score == 123.), "an accepted crop stays exactly as it is");
    assert!(!merged.iter().any(|c| c.take == dropped.take), "a dropped crop stays out");
    assert!(merged.iter().filter(|c| !c.approved).all(|c| first.iter().any(|f| f.take == c.take)), "the others are the re-scored suggestions");
    assert_eq!(merge_cut(&mine, Vec::new()), mine, "an empty re-cut changes nothing");
}

#[test]
fn a_place_cover_shows_the_place_not_a_person() {
    let store = project("crops_cover");
    // Every panel of the synthetic manga lists people with face boxes, except where removed.
    let mut rs = reads();
    for (_, read) in &mut rs {
        if let Some(panel) = read.pages.iter_mut().find(|p| p.index == 3).and_then(|p| p.panels.get_mut(2)) {
            panel.characters.clear();
        }
    }
    let busy = SourceRef { chapter_id: "c1".into(), page: 0, panel: Some(0) };
    let empty = SourceRef { chapter_id: "c1".into(), page: 3, panel: Some(2) };
    let (cut, covers) = place_crops(&store, "pl_street", &[(busy.clone(), 2), (empty.clone(), 1)], &rs);
    assert_eq!(cut.len(), 2);
    let take_of = |at: &SourceRef| cut.iter().find(|(c, _)| &c.source == at).map(|(c, _)| c.take.clone()).expect("cut");
    assert_eq!(covers, vec![take_of(&empty)], "only the panel without people may be the cover");
    let busy_score = cut.iter().find(|(c, _)| c.source == busy).map(|(c, _)| c.score).unwrap_or(0.);
    let empty_score = cut.iter().find(|(c, _)| c.source == empty).map(|(c, _)| c.score).unwrap_or(0.);
    assert!(empty_score > 0. && busy_score > 0.);
    let mut place = Place { id: "pl_street".into(), crops: cut.iter().map(|(c, _)| c.clone()).collect(), ..Default::default() };
    place.extra.insert(EXTRA_COVERS.into(), json!(covers));
    // Nothing reviewed yet: no cover, whatever the detectors say (the screen shows a neutral tile).
    assert!(place_cover(&place).is_none(), "an unreviewed panel is never a cover by itself");
    assert!(place_cover_file(&store, &place).is_none());
    // The user accepts the busy panel: that one is the cover.
    for c in place.crops.iter_mut().filter(|c| c.source == busy) {
        c.approved = true;
    }
    assert_eq!(place_cover(&place).map(|c| c.source.clone()), Some(busy));
    assert!(place_cover_file(&store, &place).is_some_and(|p| p.is_file()));
}

#[test]
fn a_character_cover_is_an_accepted_crop_else_a_studio_face_else_nothing() {
    let store = project("crops_char_cover");
    let mut c = character(0, json!([]));
    let cut = pick(&store, &c, &reads());
    assert!(!cut.is_empty());
    c.crops = cut;
    let dir = tempfile::tempdir().expect("temp");
    let mut brain = mv_recap::Brain::default();
    // Unreviewed crops and no Studio face: nothing (the screen shows the initials tile).
    assert!(character_cover(&store, &c, &brain).is_none());
    // A face from Studio's character brain, when linked.
    let face = dir.path().join("face.jpg");
    std::fs::write(&face, b"jpg").expect("face");
    brain.characters.push(mv_recap::Character { id: c.brain_id.clone().unwrap_or_default(), faces: vec![dir.path().join("gone.jpg"), face.clone()], ..Default::default() });
    assert_eq!(character_cover(&store, &c, &brain), Some(face.clone()));
    // A crop the user approved wins over it.
    c.crops[0].approved = true;
    let mine = store.path(&c.crops[0].file);
    assert_eq!(character_cover(&store, &c, &brain), Some(mine));
    // Not linked to the brain and nothing approved: nothing again.
    c.crops[0].approved = false;
    c.brain_id = None;
    assert!(character_cover(&store, &c, &brain).is_none());
}
