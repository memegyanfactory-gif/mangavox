#![cfg(feature = "story")]
//! Story step, pure parts (design 6.1): reading the model's answer onto the model with stable
//! ids, chunking at scene-kind boundaries, the length budget, the R3 line checks and the merge
//! that keeps the user's edits. No data folder and no model call.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use mv_anime::source::digest_page;
use mv_anime::story::{self, budgets, chapter_seconds, chunk_pages, colour_theme, fit_budget, map_reply, merge, replace_scene, review, scene_floor, Cast, Ids, MapCtx, COLOUR_THEMES, PROMPT};
use mv_anime::*;
use serde_json::{json, Value};

fn cast() -> Cast {
    Cast::new(&BibleIndex::default(), &common::brain())
}

fn gate() -> Place {
    Place { id: "pl_gate".into(), name: "Tower Gate".into(), ..Place::default() }
}

/// A full answer for chapter 1 with every kind of field, a few lenient spellings and mistakes.
fn answer() -> Value {
    json!({"scenes": [
        {"title": "Talk at the tower", "place": "tower gate", "time": "Day", "kind": "dialogue", "tone": "drama", "fight": false,
         "colour": "neutral daylight", "intensity": 3, "target_s": 40, "summary": "Kai and Ren talk under the tower.",
         "source": [{"page": 0, "panel": 0}, {"page": 1, "panel": 2}],
         "beats": [
            {"purpose": "establish the tower", "hook": false, "source": [], "lines": []},
            {"purpose": "they talk", "hook": false, "source": [{"page": 0, "panel": 1}], "lines": [
                {"speaker": "Kai", "kind": "speech", "original": "Kai speaks on page 1 panel 2.", "spoken": "Look at that tower.",
                 "emotion": "awe", "strength": 2, "pace": "slow", "keep": false, "page": 0, "panel": 1},
                {"speaker": "Shadow", "kind": "thought", "original": "Ren speaks on page 1 panel 2.", "spoken": "I don't like it.",
                 "emotion": "scared", "strength": "strong", "pace": "normal", "keep": false, "page": 0, "panel": 1},
                {"speaker": null, "kind": "narration", "original": "Chapter 1. The tower rises over the city.", "spoken": "reworded anyway",
                 "emotion": "neutral", "strength": 1, "pace": "slow", "keep": true, "page": 0, "panel": 0}
            ]}
         ]},
        {"title": "The fight", "place": "Rooftop", "time": "night", "kind": "action", "tone": "fight", "fight": true,
         "colour": "blood red sky", "intensity": 8, "target_s": "30s", "summary": "The fight breaks out.",
         "source": [{"page": 2}, {"page": 3, "panel": 2}],
         "beats": [
            {"purpose": "the strike", "hook": true, "source": [{"page": 2, "panel": 0}], "lines": [
                {"speaker": "Stranger", "kind": "speech", "original": "", "spoken": "Behind you!", "emotion": "fear", "strength": 3,
                 "pace": "fast", "keep": false, "page": null, "panel": null},
                {"speaker": null, "kind": "sfx", "original": "CRASH", "spoken": "CRASH", "emotion": "neutral", "strength": 2,
                 "pace": "fast", "keep": true, "page": 3, "panel": 2},
                {"speaker": "kai", "kind": "speech", "original": "", "spoken": "Ha ha, this is so much fun for every one of us here tonight!",
                 "emotion": "joy", "strength": 2, "pace": "fast", "keep": false, "page": 9, "panel": 0}
            ]}
         ]}
    ]})
}

fn map(v: &Value, counters: &BTreeMap<String, u32>) -> anyhow::Result<story::Adapted> {
    let read = common::chapter_read(0);
    let cast = cast();
    let places = [gate()];
    let cx = MapCtx { chapter_id: "c1", read: &read, cast: &cast, places: &places };
    let mut ids = Ids::new(counters, &Story::default());
    map_reply(v, &cx, &mut ids)
}

fn src(page: usize, panel: Option<usize>) -> SourceRef {
    SourceRef { chapter_id: "c1".into(), page, panel }
}

#[test]
fn a_good_answer_is_read_onto_the_model() {
    let counters = BTreeMap::from([("scene".to_string(), 5), ("line".to_string(), 40)]);
    let a = map(&answer(), &counters).expect("read");
    assert_eq!(a.scenes.len(), 2);
    let (talk, fight) = (&a.scenes[0], &a.scenes[1]);

    // Ids continue from the project's counters.
    assert_eq!((talk.id.as_str(), fight.id.as_str()), ("sc0005", "sc0006"));
    assert_eq!(talk.beats[0].id, "bt0001");
    assert_eq!(a.lines[0].id, "ln0040");
    let ids: BTreeSet<&str> = a.lines.iter().map(|l| l.id.as_str()).collect();
    assert_eq!(ids.len(), a.lines.len(), "line ids are unique");

    // Scene fields.
    assert_eq!(talk.place.as_deref(), Some("pl_gate"), "a bible place by name, any case");
    assert_eq!((talk.time_of_day, talk.kind, talk.tone, talk.fight), (TimeOfDay::Day, SceneKind::Dialogue, Tone::Drama, false));
    assert_eq!(talk.colour.name, "neutral daylight");
    assert_eq!(talk.intensity, 3);
    assert_eq!(talk.source, vec![src(0, Some(0)), src(1, Some(2))]);
    assert_eq!(talk.beats.len(), 2, "an added beat with no line is kept");
    assert!(talk.beats[0].source.is_empty() && talk.beats[0].lines.is_empty());
    assert_eq!((fight.kind, fight.tone, fight.fight, fight.time_of_day), (SceneKind::Action, Tone::Fight, true, TimeOfDay::Night));
    assert_eq!(fight.colour.name, "burning red", "a colour outside the list maps onto it");
    assert_eq!(fight.place, None);
    assert!(fight.summary.starts_with("Place: Rooftop."), "an unknown place is kept for the Bible step: {}", fight.summary);
    assert_eq!(fight.source, vec![src(2, None), src(3, Some(2))]);
    assert!(fight.beats[0].hook);

    // Lines: speakers by name and alias, narration, keep, sfx, emotion and strength.
    let line = |spoken: &str| a.lines.iter().find(|l| l.spoken == spoken).unwrap_or_else(|| panic!("line {spoken}"));
    let kai = line("Look at that tower.");
    assert_eq!((kai.speaker.as_deref(), kai.kind, kai.scene.as_str()), (Some("kai"), LineKind::Speech, "sc0005"));
    assert_eq!((kai.emotion.kind, kai.emotion.strength, kai.pace), (EmotionKind::Awe, 2, Pace::Slow));
    assert_eq!(kai.source, Some(src(0, Some(1))));
    let ren = line("I don't like it.");
    assert_eq!((ren.speaker.as_deref(), ren.kind), (Some("ren"), LineKind::Thought), "an alias finds the character");
    assert_eq!((ren.emotion.kind, ren.emotion.strength), (EmotionKind::Fear, 3));
    let narr = line("Chapter 1. The tower rises over the city.");
    assert!(narr.keep_words && narr.speaker.is_none() && narr.kind == LineKind::Narration, "keep: spoken is the original");
    let stranger = line("Behind you!");
    assert_eq!(stranger.speaker.as_deref(), Some("Stranger"), "an unknown speaker keeps their name");
    assert_eq!(stranger.source, None, "an added line has no panel");
    let crash = line("CRASH");
    assert_eq!((crash.kind, crash.speaker.as_deref()), (LineKind::Sfx, None));
    let joke = a.lines.iter().find(|l| l.emotion.kind == EmotionKind::Joy).expect("joke");
    assert_eq!(joke.speaker.as_deref(), Some("kai"), "names match in any case");
    assert_eq!(joke.source, None, "a page the chapter does not have is dropped");
    assert!(a.issues.iter().any(|i| i.code == "unknown_page" && i.scene == "sc0006"), "{:?}", a.issues);

    // Beats list their lines in order, every line belongs to its scene.
    for s in &a.scenes {
        for b in &s.beats {
            for id in &b.lines {
                let l = a.lines.iter().find(|l| &l.id == id).expect("beat line exists");
                assert_eq!(l.scene, s.id);
            }
        }
    }
}

#[test]
fn the_lines_are_checked_against_their_scene() {
    let a = map(&answer(), &BTreeMap::new()).expect("read");
    let story = Story { scenes: a.scenes, lines: a.lines, ..Story::default() };
    let found = review(&story, &cast().ids());
    let codes: Vec<&str> = found.iter().map(|i| i.code.as_str()).collect();
    assert!(codes.contains(&"unknown_speaker"), "{codes:?}");
    assert!(found.iter().any(|i| i.code == "unknown_speaker" && i.message.contains("Stranger")));
    assert!(codes.contains(&"tone_mismatch"), "a cheerful spoken line in a fight: {codes:?}");
    assert!(codes.contains(&"long_line_under_threat"), "{codes:?}");
    assert!(found.iter().all(|i| !i.message.contains("CRASH")), "sound effects are not checked");
    assert!(found.iter().all(|i| i.fix.as_deref().is_some_and(|f| !f.is_empty())), "every issue says what fixes it");
    // An edited line is the user's choice: only its speaker is checked.
    let mut edited = story.clone();
    for l in edited.lines.iter_mut() {
        l.edited = true;
    }
    let found = review(&edited, &cast().ids());
    assert!(found.iter().all(|i| i.code == "unknown_speaker"), "{found:?}");
}

#[test]
fn speakers_are_linked_once_the_bible_knows_them() {
    let a = map(&answer(), &BTreeMap::new()).expect("read");
    let mut story = Story { scenes: a.scenes, lines: a.lines, ..Story::default() };
    // The issue tells the user what building the Bible does now.
    let found = review(&story, &cast().ids());
    let unknown = found.iter().find(|i| i.code == "unknown_speaker").expect("unknown speaker");
    assert!(unknown.fix.as_deref().is_some_and(|f| f.contains("linked")), "{unknown:?}");
    // A second line by the stranger, locked by the user, keeps its name.
    let mut locked = story.lines.iter().find(|l| l.speaker.as_deref() == Some("Stranger")).cloned().expect("stranger");
    locked.id = "ln0999".into();
    locked.locked.insert("/speaker".into());
    story.lines.push(locked);
    // The Bible step adds the stranger as a character.
    let bible = BibleIndex { characters: vec![Character { id: "ch_stranger".into(), name: "Stranger".into(), ..Character::default() }], ..BibleIndex::default() };
    let cast = Cast::new(&bible, &common::brain());
    assert_eq!(story::relink_speakers(&mut story, &cast), 1);
    let speakers: Vec<Option<&str>> = story.lines.iter().filter(|l| l.spoken == "Behind you!").map(|l| l.speaker.as_deref()).collect();
    assert_eq!(speakers, vec![Some("ch_stranger"), Some("Stranger")], "linked, except the locked one");
    // The saved checks were made again: the locked name is still not in the cast.
    assert!(story::issues(&story).iter().any(|i| i.code == "unknown_speaker" && i.message.contains("Stranger")));
}

#[test]
fn a_cut_off_answer_names_the_pages_it_did_not_reach() {
    let read = common::chapter_read(0);
    let all: Vec<usize> = read.pages.iter().map(|p| p.index).collect();
    let a = map(&answer(), &BTreeMap::new()).expect("read");
    assert!(story::uncovered_tail(&a, "c1", &all, &read).is_empty(), "the whole chapter is covered");
    // Only the first scene arrived.
    let cut = story::Adapted { scenes: a.scenes[..1].to_vec(), lines: a.lines.iter().filter(|l| l.scene == a.scenes[0].id).cloned().collect(), issues: Vec::new() };
    let tail = story::uncovered_tail(&cut, "c1", &all, &read);
    assert_eq!(tail, all.iter().copied().filter(|p| *p > 1).collect::<Vec<_>>(), "the pages after the last one it reached");
}

#[test]
fn junk_answers_fail_with_a_readable_message() {
    for junk in [json!({"note": "I adapted it in my head."}), json!({"scenes": []}), json!({"scenes": [{"title": ""}, 3, "x"]})] {
        let e = map(&junk, &BTreeMap::new()).expect_err("no scenes");
        assert!(e.to_string().contains("no scenes") && e.to_string().contains("Try again"), "{e}");
    }
    // Scenes under another key are still found.
    let wrapped = json!({"script": {"scenes": [{"title": "Only", "summary": "s", "source": [{"page": 0}]}]}});
    assert_eq!(map(&wrapped, &BTreeMap::new()).expect("found").scenes.len(), 1);
}

#[test]
fn a_cut_off_answer_keeps_its_finished_scenes() {
    let full = answer().to_string();
    let at = full.find("The fight breaks out").expect("second scene");
    let cut = &full[..at + 8];
    let v = mv_anime::json::parse_reply(cut).expect("salvaged");
    let a = map(&v, &BTreeMap::new()).expect("read");
    assert_eq!(a.scenes[0].title, "Talk at the tower");
    assert_eq!(a.lines.iter().filter(|l| l.scene == a.scenes[0].id).count(), 3, "the finished scene is whole");
}

#[test]
fn a_scene_without_a_source_sits_by_its_neighbour() {
    let v = json!({"scenes": [
        {"title": "Bridge", "summary": "Kai walks to the gate.", "beats": [{"purpose": "walk", "lines": []}]},
        {"title": "Gate", "summary": "At the gate.", "source": [{"page": 1, "panel": 1}], "beats": [{"purpose": "arrive"}]}
    ]});
    let a = map(&v, &BTreeMap::new()).expect("read");
    assert_eq!(a.scenes[0].source, vec![src(1, None)]);
}

#[test]
fn words_outside_the_lists_fall_back_sensibly() {
    assert_eq!(colour_theme("Deep Blue Threat", SceneKind::Dialogue, Tone::Drama), "deep blue threat");
    assert_eq!(colour_theme("grey flashback tones", SceneKind::Dialogue, Tone::Drama), "grey memory");
    assert_eq!(colour_theme("", SceneKind::Dread, Tone::Drama), "deep blue threat");
    assert_eq!(colour_theme("???", SceneKind::Flashback, Tone::Drama), "grey memory");
    let v = json!({"scenes": [{"title": "x", "summary": "y", "kind": "battle", "tone": "spooky", "time": "evening", "source": [0],
        "beats": [{"purpose": "p", "lines": [{"speaker": "?", "kind": "", "spoken": "Who's there?", "emotion": "terrified", "strength": 9}]}]}]});
    let a = map(&v, &BTreeMap::new()).expect("read");
    let (s, l) = (&a.scenes[0], &a.lines[0]);
    assert_eq!((s.kind, s.tone, s.time_of_day), (SceneKind::Action, Tone::Drama, TimeOfDay::Dusk));
    assert_eq!((l.speaker.as_deref(), l.kind, l.emotion.kind, l.emotion.strength), (None, LineKind::Narration, EmotionKind::Fear, 3));
    assert_eq!(l.pace, Pace::Fast, "action lines default to fast");
}

#[test]
fn the_prompt_names_every_choice_the_code_reads() {
    for (name, _) in COLOUR_THEMES {
        assert!(PROMPT.contains(name), "colour {name}");
    }
    for w in ["drama", "romance", "action", "fight", "horror", "comedy", "calm", "mystery"] {
        assert!(PROMPT.contains(w), "tone {w}");
    }
    for w in ["neutral", "joy", "sadness", "anger", "fear", "surprise", "disgust", "resolve", "pride", "grief", "despair", "relief", "awe", "tender", "menace"] {
        assert!(PROMPT.contains(w), "emotion {w}");
    }
    for w in ["\"scenes\"", "\"beats\"", "\"lines\"", "\"target_s\"", "\"strength\"", "\"keep\"", "\"hook\"", "\"source\"", "day, dusk, night, dawn, indoor"] {
        assert!(PROMPT.contains(w), "shape {w}");
    }
    // The shape at the end of the prompt is valid JSON that reads as one scene.
    let shape = PROMPT.lines().rev().find(|l| l.starts_with("{\"scenes\"")).expect("shape line");
    let v: Value = serde_json::from_str(shape).expect("valid JSON");
    assert_eq!(map(&v, &BTreeMap::new()).expect("reads").scenes.len(), 1);
}

#[test]
fn long_chapters_are_cut_where_the_scene_kind_changes() {
    let read = common::chapter_read(0);
    let size = |p: usize| digest_page(&read.pages[p]).chars().count();
    let total: usize = (0..4).map(size).sum();
    assert_eq!(chunk_pages(&read, total), vec![vec![0, 1, 2, 3]], "fits: one call");
    // Room for three pages: still cut where the talk turns into the fight (pages 0-1 | 2-3).
    assert_eq!(chunk_pages(&read, size(0) + size(1) + size(2)), vec![vec![0, 1], vec![2, 3]]);
    assert_eq!(chunk_pages(&read, (size(0) + size(1)).max(size(2) + size(3))), vec![vec![0, 1], vec![2, 3]]);
    // A run too big on its own is cut at page boundaries.
    let one = (0..4).map(size).max().unwrap_or(1);
    assert_eq!(chunk_pages(&read, one), vec![vec![0], vec![1], vec![2], vec![3]]);
    // Several short runs share a part when they fit.
    let mut mixed = read.clone();
    for (p, kind) in mixed.pages.iter_mut().zip(["dialogue", "fight", "dialogue", "fight"]) {
        p.scene.kind = kind.into();
    }
    let sizes: Vec<usize> = (0..4).map(|p| digest_page(&mixed.pages[p]).chars().count()).collect();
    assert_eq!(chunk_pages(&mixed, sizes[0] + sizes[1] + sizes[2]), vec![vec![0, 1, 2], vec![3]]);
    assert!(chunk_pages(&mv_recap::ChapterRead::default(), 100).is_empty());
}

fn scene(id: &str, kind: SceneKind, target_s: f32, lines: &[&str]) -> Scene {
    Scene {
        id: id.into(),
        title: id.into(),
        kind,
        target_s,
        source: vec![src(0, Some(0))],
        beats: vec![Beat { id: format!("b-{id}"), lines: lines.iter().map(|l| l.to_string()).collect(), ..Beat::default() }],
        ..Scene::default()
    }
}

fn line(id: &str, scene: &str, spoken: &str) -> Line {
    Line { id: id.into(), scene: scene.into(), speaker: Some("kai".into()), spoken: spoken.into(), source: Some(src(0, Some(0))), ..Line::default() }
}

#[test]
fn scene_lengths_share_the_budget_but_never_cut_a_line_short() {
    let long = "word ".repeat(50);
    let lines = vec![line("l1", "a", "Hello there."), line("l2", "b", &long)];
    let mut scenes = vec![scene("a", SceneKind::Dialogue, 30., &["l1"]), scene("b", SceneKind::Dialogue, 10., &["l2"]), scene("c", SceneKind::Action, 0., &[])];
    let floor_b = scene_floor(&scenes[1], &lines);
    assert!(floor_b > 20., "50 words take about 20 s: {floor_b}");
    fit_budget(&mut scenes, &lines, 120.);
    let sum: f32 = scenes.iter().map(|s| s.target_s).sum();
    assert!((sum - 120.).abs() < 0.5, "shared out: {sum}");
    assert!(scenes[1].target_s >= floor_b - 0.05, "never under its lines: {}", scenes[1].target_s);
    assert!(scenes[2].target_s >= 2., "a scene with no length gets one from its panels");
    assert!(scenes[0].target_s > scenes[2].target_s, "the AI's proportions are kept: {scenes:?}");
    // A budget smaller than the lines: every scene at its floor.
    fit_budget(&mut scenes, &lines, 5.);
    assert!(scenes[1].target_s >= floor_b - 0.05);
    let floor_a = scene_floor(&scenes[0], &lines);
    assert!(scenes[0].target_s >= floor_a - 1e-4 && scenes[0].target_s <= floor_a + 0.1 + 1e-4, "{} for {floor_a}", scenes[0].target_s);
    // No budget: the AI's lengths, raised to the floors.
    let mut free = vec![scene("a", SceneKind::Dialogue, 33., &["l1"])];
    fit_budget(&mut free, &lines, 0.);
    assert_eq!(free[0].target_s, 33.);
}

#[test]
fn chapters_get_six_to_nine_minutes_or_a_share_of_the_target() {
    let reads = vec![Some(common::chapter_read(0)), Some(common::chapter_read(1))];
    let auto = chapter_seconds(&common::chapter_read(0));
    assert!((360. ..=540.).contains(&auto), "{auto}");
    let mut ep = Episode { chapters: vec![ChapterPick::default(), ChapterPick::default()], ..Episode::default() };
    assert_eq!(budgets(&ep, &reads), vec![auto, auto]);
    assert_eq!(budgets(&ep, &[Some(common::chapter_read(0)), None]), vec![auto, auto], "an unread chapter counts as an average one");
    ep.target_minutes = 4.;
    let b = budgets(&ep, &reads);
    assert!((b[0] - 120.).abs() < 0.01 && (b[1] - 120.).abs() < 0.01, "{b:?}");
}

/// An old story: chapter c1 with scenes at pages 0 (edited), 1 (a locked line), 2 (untouched)
/// and chapter c2 at page 0 (untouched).
fn old_story() -> Story {
    let mut a = scene("sc0001", SceneKind::Dialogue, 20., &["ln0001"]);
    a.edited = true;
    a.title = "My own title".into();
    let mut b = scene("sc0002", SceneKind::Dialogue, 20., &["ln0002"]);
    b.source = vec![src(1, Some(0))];
    let mut c = scene("sc0003", SceneKind::Action, 20., &["ln0003"]);
    c.source = vec![src(2, Some(0))];
    let mut d = scene("sc0004", SceneKind::Dialogue, 20., &["ln0004"]);
    d.source = vec![SourceRef { chapter_id: "c2".into(), page: 0, panel: Some(0) }];
    let mut locked = line("ln0002", "sc0002", "My words.");
    locked.edited = true;
    locked.locked = BTreeSet::from(["/spoken".to_string()]);
    Story {
        episode: "ep01".into(),
        scenes: vec![a, b, c, d],
        lines: vec![line("ln0001", "sc0001", "Old one."), locked, line("ln0003", "sc0003", "Old three."), line("ln0004", "sc0004", "Old four.")],
        ..Story::default()
    }
}

#[test]
fn a_merge_keeps_edited_and_locked_scenes_and_replaces_the_rest() {
    let old = old_story();
    // A new adaptation of c1: scenes at pages 0, 2 and 3; its ids clash with the old ones.
    let mut n0 = scene("sc0001", SceneKind::Dialogue, 20., &["ln0001"]);
    n0.title = "New zero".into();
    let mut n2 = scene("sc0002", SceneKind::Action, 20., &["ln0002"]);
    n2.title = "New two".into();
    n2.source = vec![src(2, Some(0))];
    let mut n3 = scene("sc0009", SceneKind::Action, 20., &["ln0009"]);
    n3.title = "New three".into();
    n3.source = vec![src(3, Some(1))];
    let new = Story {
        scenes: vec![n0, n2, n3],
        lines: vec![line("ln0001", "sc0001", "New zero line."), line("ln0002", "sc0002", "New two line."), line("ln0009", "sc0009", "New three line.")],
        model: "m".into(),
        ..Story::default()
    };
    let merged = merge(&old, new);
    let titles: Vec<&str> = merged.scenes.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, vec!["My own title", "sc0002", "New two", "New three", "sc0004"], "kept, kept, new, new, other chapter");
    // Kept scenes and their lines are untouched.
    assert_eq!(merged.scenes[0], old.scenes[0]);
    assert_eq!(merged.scenes[1], old.scenes[1]);
    assert_eq!(merged.scenes[4], old.scenes[3]);
    for id in ["ln0001", "ln0002", "ln0004"] {
        let before = old.lines.iter().find(|l| l.id == id);
        let after = merged.lines.iter().find(|l| l.id == id);
        assert_eq!(before, after, "line {id}");
    }
    // The untouched c1 scene at page 2 is gone; new ids never clash.
    assert!(merged.lines.iter().all(|l| l.spoken != "Old three."));
    let scene_ids: BTreeSet<&str> = merged.scenes.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(scene_ids.len(), 5);
    let line_ids: BTreeSet<&str> = merged.lines.iter().map(|l| l.id.as_str()).collect();
    assert_eq!(line_ids.len(), merged.lines.len());
    let two = &merged.scenes[2];
    assert_ne!(two.id, "sc0002");
    assert_eq!(two.id, "sc0003", "the new scene on page 2 takes the id of the scene it replaces (its shots stay attached)");
    let two_line = merged.lines.iter().find(|l| l.spoken == "New two line.").expect("new line");
    assert_eq!(two_line.scene, two.id);
    assert_ne!(two_line.id, "ln0002", "a clashing line id is renewed");
    assert!(two.beats[0].lines.contains(&two_line.id));
    assert!(merged.lines.iter().all(|l| l.spoken != "New zero line."), "the new scene on the kept scene's page is left out");
    assert_eq!(merged.model, "m");
}

#[test]
fn adapting_one_scene_again_keeps_its_locks_and_edited_lines() {
    let mut old = old_story();
    old.scenes[1].locked.insert("/tone".into());
    old.scenes[1].tone = Tone::Horror;
    let mut fresh_scene = scene("sc0100", SceneKind::Action, 12., &["ln0100", "ln0101"]);
    fresh_scene.title = "Again".into();
    fresh_scene.tone = Tone::Comedy;
    fresh_scene.source = vec![src(1, Some(0))];
    let mut twin = line("ln0100", "sc0100", "Their new words.");
    twin.source = Some(src(0, Some(0)));
    let fresh = Story { scenes: vec![fresh_scene], lines: vec![twin, line("ln0101", "sc0100", "Another new line.")], ..Story::default() };
    let out = replace_scene(&old, "sc0002", fresh);
    let s = &out.scenes[1];
    assert_eq!(s.id, "sc0002");
    assert_eq!(s.title, "Again", "an unlocked field takes the new value");
    assert_eq!(s.tone, Tone::Horror, "a locked field stays");
    assert!(s.locked.contains("/tone") && s.edited == old.scenes[1].edited);
    let mine = out.lines.iter().find(|l| l.id == "ln0002").expect("the edited line stays");
    assert_eq!(mine.spoken, "My words.");
    assert!(out.lines.iter().any(|l| l.spoken == "Another new line." && l.scene == "sc0002"));
    assert!(s.beats.iter().flat_map(|b| &b.lines).any(|id| id == "ln0002"));
    // Other scenes are untouched; an unknown scene changes nothing.
    assert_eq!(out.scenes[0], old.scenes[0]);
    assert_eq!(replace_scene(&old, "nope", Story::default()), old);
}
