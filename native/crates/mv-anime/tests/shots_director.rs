#![cfg(feature = "shots")]
//! The director (design 6.3): reading the planner's reply (ids, placement, every line in exactly
//! one shot, the length floor), the revise loop (kept only when better, capped), junk and
//! cut-off replies, the merge that keeps the user's locks, and the PlanScene job end to end on
//! a project in a temp folder. Every model answer is canned (`Ai::canned`); no network.

use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::anyhow;
use mv_ai::{AiStepResult, CancellationToken};
use mv_anime::backend::NoImages;
use mv_anime::director::{self, merge, merge_report, plan_scene, read_reply, settle, SceneInput, REVISIONS};
use mv_anime::llm::{Ai, Canned, UNREADABLE};
use mv_anime::model::*;
use mv_anime::pacing;
use mv_anime::queue::{Job, JobCtx, JobKind, RunCtx};
use mv_anime::store::Store;
use serde_json::{json, Value};

// ------------------------------------------------------------------------- fixtures

fn place() -> Place {
    let st = |id: &str, pos: [f32; 3], yaw: f32, pitch: f32, overhead: bool| Station { id: id.into(), name: id.replace('_', " "), pos, yaw_deg: yaw, pitch_deg: pitch, lens_mm: 35, overhead };
    Place {
        id: "pl_hall".into(),
        name: "Statue hall".into(),
        plan: FloorPlan {
            w_m: 12.,
            d_m: 8.,
            landmarks: vec![Landmark { id: "lm_door".into(), name: "Door".into(), kind: LandmarkKind::Door, pos: [0., -4.], size: [2., 0.3], h_m: 3., ..Landmark::default() }],
            ..FloorPlan::default()
        },
        stations: vec![st("st_south", [0., 1.5, 3.5], 0., 0., false), st("st_top", [0., 10., 0.], 0., -90., true), st("st_east", [5.5, 1.5, 0.], 270., 0., false)],
        states: vec![PlaceState { id: "day".into(), name: "day".into(), key_dir_deg: 120., kelvin: 5600, mood: "cold".into() }],
        ..Place::default()
    }
}

fn bible() -> BibleIndex {
    let ch = |id: &str, name: &str, role: Role| Character { id: id.into(), name: name.into(), role, ..Character::default() };
    BibleIndex {
        characters: vec![ch("jinwoo", "Jinwoo", Role::Hero), ch("song", "Song", Role::Ally), ch("statue", "The Statue", Role::Threat)],
        places: vec![place()],
        props: vec![Prop { id: "pr_knife".into(), name: "Knife".into(), owner: Some("jinwoo".into()), held: true, ..Prop::default() }],
        ..BibleIndex::default()
    }
}

fn scene() -> Scene {
    Scene {
        id: "sc0001".into(),
        title: "The vote".into(),
        place: Some("pl_hall".into()),
        kind: SceneKind::Dialogue,
        intensity: 4,
        target_s: 30.,
        summary: "Jinwoo and Song argue in front of the statue.".into(),
        beats: vec![Beat { id: "bt1".into(), purpose: "They argue".into(), lines: vec!["ln1".into(), "ln2".into()], ..Beat::default() }],
        ..Scene::default()
    }
}

fn line(id: &str, speaker: &str, text: &str) -> Line {
    Line { id: id.into(), scene: "sc0001".into(), speaker: Some(speaker.into()), kind: LineKind::Speech, spoken: text.into(), ..Line::default() }
}

fn lines() -> Vec<Line> {
    vec![
        line("ln1", "song", "We have to go back now."),
        line("ln2", "jinwoo", "No. Look at the statue first."),
        line("ln3", "song", "It moved. I am sure it moved when we turned around."),
        line("ln4", "jinwoo", "Then nobody turns around."),
    ]
}

fn input<'a>(scene: &'a Scene, lines: &'a [Line], bible: &'a BibleIndex, new_place: bool) -> SceneInput<'a> {
    SceneInput { scene, lines, bible, new_place }
}

fn pair_marks() -> Value {
    json!([
        {"char": "jinwoo", "x": -1.2, "z": 0, "facing": 90, "pose": "stand", "visible": "full", "emotion": "resolve", "strength": 2},
        {"char": "song", "x": 1.2, "z": 0, "facing": 270, "visible": "full", "emotion": "fear", "strength": 3}
    ])
}

/// A reply shot: size, seconds, line ids; camera south of the pair.
fn rs(size: &str, secs: f32, ids: &[&str]) -> Value {
    json!({"tells": format!("a {size}"), "size": size, "angle": "eye", "station": null, "camera": {"x": 0.3, "z": 3.0, "height": 1.5},
           "look_at": "char:song", "marks": pair_marks(), "lines": ids, "mouth": "flap", "motion": "mouth_only", "seconds": secs})
}

fn reply(shots: Vec<Value>) -> String {
    json!({ "shots": shots }).to_string()
}

fn progress() -> impl Fn(String) + Send + Sync {
    |_s: String| {}
}

fn covered_once(shots: &[Shot], lines: &[Line]) {
    let mut seen = Vec::new();
    for s in shots {
        seen.extend(s.lines.iter().cloned());
    }
    let mut want: Vec<String> = lines.iter().map(|l| l.id.clone()).collect();
    let mut got = seen.clone();
    want.sort();
    got.sort();
    assert_eq!(got, want, "every line in exactly one shot: {seen:?}");
}

fn floors_hold(shots: &[Shot], lines: &[Line], kind: SceneKind) {
    for s in shots {
        let ls: Vec<Line> = s.lines.iter().filter_map(|id| lines.iter().find(|l| &l.id == id).cloned()).collect();
        let floor = pacing::spoken_seconds(&ls, kind);
        assert!(s.planned_s + 1e-4 >= floor, "{} s under its {floor} s of lines", s.planned_s);
        assert!(s.planned_s >= pacing::CUT_MIN_S && s.planned_s.is_finite());
    }
}

// ------------------------------------------------------------------------- reading a reply

#[test]
fn a_reply_is_read_placed_and_settled() {
    let (sc, ls, b) = (scene(), lines(), bible());
    let inp = input(&sc, &ls, &b, true);
    let v = json!({"shots": [
        // On a station: the station owns the position.
        {"tells": "where everyone stands", "size": "ls", "angle": "high", "station": "st_south", "camera": {"x": 0, "z": 3.5, "height": 1.5},
         "look_at": "char:jinwoo", "marks": pair_marks(), "lines": ["ln1"], "seconds": 1.0, "source": [{"page": 3, "panel": 1}], "origin": "manga", "beat": "bt1"},
        // Off the plan: clamped inside it; a line twice keeps the first.
        {"size": "Over the shoulder", "camera": {"x": -30, "z": 2, "height": 1.4}, "look_at": "char:song", "marks": pair_marks(),
         "lines": ["ln2", "ln2", "ln1"], "mouth": "flap", "lens": 80, "seconds": 3},
        // Unknown ids are dropped and noted; synonyms are understood.
        {"size": "Close-Up", "angle": "Low Angle", "move": {"kind": "static", "speed": "slow"}, "look_at": "char:Song",
         "marks": [{"char": "ghost", "x": 0, "z": 0}, {"char": "Song", "x": 1.2, "z": 0, "facing": 270, "visible": "full",
                    "holding": [{"prop": "Knife", "hand": "left"}, {"prop": "pr_gun"}]}],
         "lines": ["ln9"], "station": "st_nowhere"},
        // No camera at all: put south of the subject; an overhead goes to the overhead station.
        {"size": "top-down", "marks": pair_marks(), "seconds": "4.5s"}
    ]});
    let (shots, notes) = read_reply(&inp, &v);
    assert_eq!(shots.len(), 4);
    covered_once(&shots, &ls);
    floors_hold(&shots, &ls, sc.kind);
    // ln3 and ln4 were missing: they go to the shot holding the line before them.
    assert_eq!(shots[1].lines, vec!["ln2", "ln3", "ln4"]);
    assert_eq!(shots[0].lines, vec!["ln1"]);

    let s0 = &shots[0];
    assert_eq!((s0.camera.station.as_deref(), s0.camera.pos), (Some("st_south"), None));
    assert_eq!((s0.camera.size, s0.camera.angle, s0.origin, s0.beat.as_deref()), (ShotSize::Ls, Angle::High, Origin::Manga, Some("bt1")));
    assert_eq!(s0.source, vec![SourceRef { chapter_id: String::new(), page: 3, panel: Some(1) }]);
    assert!(s0.planned_s >= 2.0, "raised to the floor of its line");
    assert_eq!(s0.background, BackgroundKind::Matched, "a layout shot needs the room");

    let s1 = &shots[1];
    assert_eq!(s1.camera.size, ShotSize::Ots);
    assert_eq!(s1.camera.lens_mm, 85, "80 mm snaps to 85");
    let p = s1.camera.pos.unwrap_or_default();
    assert!((p[0] - -6.).abs() < 1e-4 && (p[2] - 2.).abs() < 1e-4, "clamped into the 12 x 8 m plan: {p:?}");
    assert_eq!(s1.background, BackgroundKind::Backdrop);
    assert_eq!(s1.mouth, MouthPolicy::Flap);
    assert_eq!(s1.motion, MotionKind::MouthOnly, "a flapping mouth on a held drawing");

    let s2 = &shots[2];
    assert_eq!((s2.camera.size, s2.camera.angle, s2.camera.movement.kind), (ShotSize::Cu, Angle::Low, Move::Hold));
    assert_eq!(s2.camera.station, None);
    assert!(s2.camera.pos.is_some(), "no camera given: placed south of the subject");
    assert_eq!(s2.marks.iter().map(|m| m.character.as_str()).collect::<Vec<_>>(), vec!["song"], "the name maps to the id; the ghost is dropped");
    assert_eq!(s2.marks[0].holding, vec![Hold { prop: "pr_knife".into(), hand: Hand::Left }]);
    assert_eq!(s2.camera.look_at, LookAt::Character("song".into()));
    assert!(s2.lines.is_empty());
    assert_eq!(s2.origin, Origin::Anime);

    let s3 = &shots[3];
    assert_eq!((s3.camera.size, s3.camera.angle, s3.camera.station.as_deref()), (ShotSize::Overhead, Angle::Top, Some("st_top")));
    assert_eq!(s3.planned_s, 4.5);

    let dropped: BTreeSet<String> = notes.iter().filter(|n| n.code == "plan-unknown").map(|n| n.message.clone()).collect();
    for want in ["\"ghost\"", "\"pr_gun\"", "\"ln9\"", "\"st_nowhere\""] {
        assert!(dropped.iter().any(|m| m.contains(want)), "{want} not noted: {dropped:?}");
    }
    assert!(notes.iter().all(|n| n.scene == "sc0001" && n.severity == Severity::Info));
    // "Reset to Auto" has the suggestion in the shot's own shape.
    assert_eq!(s1.suggested.pointer("/camera/lens_mm"), Some(&json!(85)));
    assert!(s1.suggested.get("frame").is_none() && s1.suggested.get("id").is_none());
}

#[test]
fn the_length_floor_and_limits_hold() {
    let (sc, b) = (scene(), bible());
    let ls = vec![line("ln1", "song", &"word ".repeat(20)), line("ln2", "jinwoo", "Short."), line("ln3", "jinwoo", "Also short.")];
    let inp = input(&sc, &ls, &b, false);
    let v = json!({"shots": [rs("ms", 0.5, &["ln1"]), rs("ms", 90., &["ln2"]), rs("ms", -3., &["ln3"])]});
    let (shots, _) = read_reply(&inp, &v);
    floors_hold(&shots, &ls, sc.kind);
    // 20 words = 8 s + 0.9 lead + 0.4 tail.
    assert!((shots[0].planned_s - 9.3).abs() < 0.11, "{}", shots[0].planned_s);
    assert_eq!(shots[1].planned_s, pacing::CUT_MAX_S, "90 s is cut to the 20 s maximum");
    assert!(shots[2].planned_s > 0.4, "a nonsense length gets a default");
    // A thought moves no mouth.
    let thought = vec![Line { kind: LineKind::Thought, ..line("ln1", "song", "I should run.") }];
    let inp = input(&sc, &thought, &b, false);
    let (shots, _) = read_reply(&inp, &json!({"shots": [rs("mcu", 3., &["ln1"])]}));
    assert_eq!(shots[0].mouth, MouthPolicy::Closed);
    assert_eq!(shots[0].motion, MotionKind::Hold);
}

#[test]
fn every_line_lands_in_exactly_one_shot() {
    let (sc, ls, b) = (scene(), lines(), bible());
    let inp = input(&sc, &ls, &b, false);
    // Nobody carries a line: they are shared out in order.
    let (shots, _) = read_reply(&inp, &json!({"shots": [rs("ls", 4., &[]), rs("ms", 3., &[]), rs("mcu", 3., &[])]}));
    covered_once(&shots, &ls);
    assert_eq!(shots[0].lines, vec!["ln1", "ln2"]);
    // A missing first line goes to the shot holding the line after it.
    let (shots, _) = read_reply(&inp, &json!({"shots": [rs("ls", 4., &[]), rs("ms", 3., &["ln2", "ln3"]), rs("mcu", 3., &["ln4", "ln3"])]}));
    covered_once(&shots, &ls);
    assert_eq!(shots[1].lines, vec!["ln1", "ln2", "ln3"]);
    assert_eq!(shots[2].lines, vec!["ln4"]);
    // A shot whose lines the user locked keeps them over an earlier unlocked shot.
    let mut shots = vec![Shot { lines: vec!["ln1".into(), "ln2".into()], ..Shot::default() }, Shot { lines: vec!["ln2".into()], locked: ["/lines".to_string()].into(), ..Shot::default() }];
    settle(&mut shots, &inp);
    assert_eq!((shots[0].lines.clone(), shots[1].lines.clone()), (vec!["ln1".to_string(), "ln3".into(), "ln4".into()], vec!["ln2".to_string()]));
}

#[test]
fn the_prompt_carries_the_scene_the_place_and_the_rules() {
    let (sc, ls, b) = (scene(), lines(), bible());
    let (system, user) = director::prompt(&input(&sc, &ls, &b, true));
    for want in ["FLOOR PLAN FIRST", "\"shots\":[{\"tells\"", "Never \"generate\"", "EXACTLY ONE shot", "full face"] {
        assert!(system.contains(want), "system prompt lacks {want:?}");
    }
    for want in [
        "SCENE sc0001",
        "OPENS IN A NEW PLACE",
        "ln3 | Song (song) | speech",
        "jinwoo | Jinwoo | role hero",
        "statue | The Statue | role threat",
        "pr_knife | Knife",
        "PLACE pl_hall",
        "lm_door \"Door\" door at (0.0, -4.0)",
        "st_top \"st top\"",
        "OVERHEAD",
        "key light from 120°",
        "PACING for a dialogue scene",
        "COVERAGE PATTERNS",
        "sudden_death",
        "THE CHECKS",
    ] {
        assert!(user.contains(want), "user prompt lacks {want:?}");
    }
    // Without a place the model is told to invent and keep one.
    let nowhere = Scene { place: None, ..scene() };
    let (_, user) = director::prompt(&input(&nowhere, &ls, &b, false));
    assert!(user.contains("PLACE: none") && user.contains("RETURNS"));
}

// ------------------------------------------------------------------------- the revise loop

/// New place, no layout shot: an error (score 3) and nothing else.
fn bad() -> String {
    reply(vec![rs("ms", 4., &["ln1", "ln2"]), rs("mcu", 5., &["ln3", "ln4"])])
}

/// Worse: still no layout, and a run of four close shots.
fn worse() -> String {
    reply(vec![rs("mcu", 4., &["ln1"]), rs("cu", 3., &["ln2"]), rs("mcu", 5., &["ln3"]), rs("cu", 3., &["ln4"])])
}

/// Better: opens on the layout.
fn better() -> String {
    reply(vec![rs("ls", 4.5, &["ln1", "ln2"]), rs("ms", 5., &["ln3", "ln4"])])
}

fn score_of(text: &str) -> usize {
    let (sc, ls, b) = (scene(), lines(), bible());
    let inp = input(&sc, &ls, &b, true);
    let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let (shots, _) = read_reply(&inp, &v);
    mv_anime::checks::score(&mv_anime::checks::check_scene(&sc, &shots, &ls, &b, true))
}

#[tokio::test]
async fn a_revision_is_kept_only_when_it_scores_better_and_the_loop_stops_at_its_cap() {
    assert!(score_of(&worse()) > score_of(&bad()) && score_of(&better()) < score_of(&bad()));
    let (sc, ls, b) = (scene(), lines(), bible());
    let inp = input(&sc, &ls, &b, true);

    // bad -> worse (thrown away) -> better (kept); a third revise would never be asked.
    let canned = Arc::new(Canned::new([bad(), worse(), better(), better()]));
    let plan = plan_scene(&Ai::fake(canned.clone()), &inp, &progress(), &CancellationToken::new()).await;
    let plan = plan.unwrap_or_default();
    assert_eq!(plan.revisions, 2);
    assert_eq!(canned.seen().len(), 3, "one plan call and {REVISIONS} revisions");
    assert_eq!(plan.shots[0].camera.size, ShotSize::Ls, "the better revision is the plan");
    assert!(!plan.issues.iter().any(|i| i.code == "layout-early"));
    // The revise call carries the plan and the check's notes.
    let seen = canned.seen();
    let rev = &seen[1];
    assert!(rev.messages[0].content.contains("THIS IS A REVISION"));
    assert!(rev.messages[1].content.contains("YOUR PLAN") && rev.messages[1].content.contains("layout-early") && rev.messages[1].content.contains("\"cut\":1"));

    // bad -> worse -> worse: the first plan stays, after exactly two revisions.
    let canned = Arc::new(Canned::new([bad(), worse(), worse(), better()]));
    let plan = plan_scene(&Ai::fake(canned.clone()), &inp, &progress(), &CancellationToken::new()).await.unwrap_or_default();
    assert_eq!((plan.revisions, canned.seen().len()), (2, 3));
    assert_eq!(plan.shots.iter().map(|s| s.camera.size).collect::<Vec<_>>(), vec![ShotSize::Ms, ShotSize::Mcu]);
    assert!(plan.issues.iter().any(|i| i.code == "layout-early"));

    // A clean plan is not revised.
    let canned = Arc::new(Canned::new([better()]));
    let plan = plan_scene(&Ai::fake(canned.clone()), &inp, &progress(), &CancellationToken::new()).await.unwrap_or_default();
    assert_eq!((plan.revisions, canned.seen().len()), (0, 1));

    // A revise call that fails keeps the plan it had.
    let canned = Arc::new(Canned::from_results([AiStepResult { text: bad(), ..AiStepResult::default() }, Canned::error("server overloaded")]));
    let plan = plan_scene(&Ai::fake(canned.clone()), &inp, &progress(), &CancellationToken::new()).await;
    let plan = plan.unwrap_or_default();
    assert_eq!(plan.shots.len(), 2);
    assert_eq!(plan.revisions, 1);
}

// ------------------------------------------------------------------------- bad replies

#[tokio::test]
async fn junk_fails_cleanly_and_a_cut_off_reply_is_salvaged() {
    let (sc, ls, b) = (scene(), lines(), bible());
    let inp = input(&sc, &ls, &b, false);
    // Junk twice (the nudge too): the basic plan, noted, with the raw reply kept.
    let ai = Ai::canned(["I'd rather not plan this scene.", "Sorry, still no JSON here."]);
    let plan = plan_scene(&ai, &inp, &progress(), &CancellationToken::new()).await.unwrap_or_else(|e| panic!("junk falls back: {e:#}"));
    assert!(plan.fallback && plan.unreadable.as_deref().is_some_and(|r| r.contains("still no JSON")));
    assert!(plan.notes.iter().any(|n| n.code == "plan-fallback" && n.message.contains("could not be read")), "{:?}", plan.notes);
    covered_once(&plan.shots, &ls);

    // Cut off in the middle of the second shot: the first is kept, every line still placed.
    let cut = r#"{"shots":[{"tells":"master","size":"ls","station":"st_south","marks":[{"char":"jinwoo","x":-1.2,"z":0,"facing":90}],"lines":["ln1","ln2"],"seconds":5},{"tells":"song","size":"mcu","lines":["ln3"#;
    let plan = plan_scene(&Ai::canned([cut]), &inp, &progress(), &CancellationToken::new()).await;
    let plan = plan.unwrap_or_default();
    assert!(!plan.shots.is_empty() && !plan.fallback);
    covered_once(&plan.shots, &ls);
    floors_hold(&plan.shots, &ls, sc.kind);

    // Valid JSON without a usable shot: a basic plan from the patterns, noted.
    let plan = plan_scene(&Ai::canned([r#"{"shots":[]}"#]), &input(&sc, &ls, &b, true), &progress(), &CancellationToken::new()).await;
    let plan = plan.unwrap_or_default();
    assert!(plan.fallback);
    assert!(plan.notes.iter().any(|n| n.code == "plan-fallback" && n.severity == Severity::Warning));
    covered_once(&plan.shots, &ls);
    floors_hold(&plan.shots, &ls, sc.kind);
    assert!(mv_anime::checks::is_layout(&plan.shots[0]), "a new place opens on the arrival funnel");
    assert!(!plan.issues.iter().any(|i| i.code == "axis" || i.code == "lines-cover"), "{:#?}", plan.issues);

    // A cancel stops at once.
    let token = CancellationToken::new();
    token.cancel();
    let r = plan_scene(&Ai::canned([better()]), &inp, &progress(), &token).await;
    assert!(r.is_err_and(|e| mv_anime::llm::is_cancelled(&e)));
}

// ------------------------------------------------------------------------- merging

fn new_shot(size: ShotSize, lens: u16, ids: &[&str]) -> Shot {
    let mut s = Shot { lines: ids.iter().map(|s| s.to_string()).collect(), planned_s: 3., tells: format!("new {ids:?}"), ..Shot::default() };
    s.camera.size = size;
    s.camera.lens_mm = lens;
    s
}

#[test]
fn locked_fields_survive_a_re_plan() {
    // sh0001: the user locked the lens and moved Song; sh0002: nothing of the user's;
    // sh0003: has a picture and only covers a line the new plan puts elsewhere;
    // sh0004: locked, covers no line, sits second.
    let mut s1 = new_shot(ShotSize::Ms, 135, &["ln1"]);
    s1.id = "sh0001".into();
    s1.edited = true;
    s1.locked = ["/camera/lens_mm".to_string(), "/marks/song/pos".to_string()].into();
    s1.marks = vec![Mark { character: "song".into(), pos: [3., 1.], ..Mark::default() }, Mark { character: "jinwoo".into(), pos: [0., 0.], ..Mark::default() }];
    let mut s2 = new_shot(ShotSize::Cu, 85, &["ln2"]);
    s2.id = "sh0002".into();
    let mut s3 = new_shot(ShotSize::Insert, 85, &["ln9"]);
    s3.id = "sh0003".into();
    s3.frame.takes = vec!["take1".into()];
    s3.frame.selected = Some("take1".into());
    let mut s4 = new_shot(ShotSize::Ecu, 85, &[]);
    s4.id = "sh0004".into();
    s4.locked = ["/tells".to_string()].into();
    s4.tells = "the user's own".into();
    let old = vec![s1, s4, s2, s3];

    let mut n1 = new_shot(ShotSize::Ls, 24, &["ln1"]);
    n1.marks = vec![Mark { character: "song".into(), pos: [-2., 0.], ..Mark::default() }, Mark { character: "jinwoo".into(), pos: [1., 1.], ..Mark::default() }];
    let new = vec![n1, new_shot(ShotSize::Ms, 50, &["ln2"]), new_shot(ShotSize::Mcu, 85, &["ln3", "ln4"])];
    let (out, notes) = merge_report(&old, new.clone(), "sc0001");

    assert_eq!(out.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), vec!["sh0001", "sh0004", "", "", "sh0003"]);
    let m = &out[0];
    assert_eq!(m.camera.lens_mm, 135, "the locked lens stays");
    assert_eq!(m.camera.size, ShotSize::Ls, "an unlocked field takes the new plan's value");
    assert_eq!(m.marks.iter().find(|x| x.character == "song").map(|x| x.pos), Some([3., 1.]), "the locked mark stays");
    assert_eq!(m.marks.iter().find(|x| x.character == "jinwoo").map(|x| x.pos), Some([1., 1.]));
    assert!(m.locked.contains("/camera/lens_mm") && m.edited);
    assert_eq!(out[1].tells, "the user's own");
    // The pictured shot is never deleted: it moves to the end with a note.
    assert_eq!(out[4].frame.selected.as_deref(), Some("take1"));
    assert_eq!(notes.len(), 1);
    assert_eq!((notes[0].code.as_str(), notes[0].severity, notes[0].shot.as_deref()), ("plan-kept", Severity::Info, Some("sh0003")));
    // sh0002 had nothing of the user's: replaced.
    assert!(!out.iter().any(|s| s.id == "sh0002"));
    assert_eq!(merge(&old, new).len(), 5);
    // Nothing of the user's at all: the new plan as it is.
    assert_eq!(merge(&[new_shot(ShotSize::Ms, 50, &["ln1"])], vec![new_shot(ShotSize::Ls, 24, &["ln1"])])[0].camera.size, ShotSize::Ls);
}

#[test]
fn a_pictured_shot_keeps_what_its_picture_shows_through_a_re_plan() {
    let mut s = new_shot(ShotSize::Cu, 85, &["ln1"]);
    s.id = "sh0007".into();
    s.tells = "Song, close".into();
    s.marks = vec![Mark { character: "song".into(), pos: [1., 0.], ..Mark::default() }];
    s.frame = Card { status: Status::Approved, takes: vec!["t1".into()], selected: Some("t1".into()), ..Card::default() };
    let mut wide = new_shot(ShotSize::Ls, 24, &["ln1", "ln2"]);
    wide.marks = vec![Mark { character: "song".into(), pos: [3., 2.], ..Mark::default() }, Mark { character: "jinwoo".into(), pos: [-3., 2.], ..Mark::default() }];
    wide.planned_s = 6.;
    let (out, _) = merge_report(std::slice::from_ref(&s), vec![wide], "sc0001");
    let m = &out[0];
    assert_eq!(m.id, "sh0007");
    assert_eq!((m.camera.size, m.camera.lens_mm), (ShotSize::Cu, 85), "the approved close-up stays a close-up");
    assert_eq!(m.marks, s.marks);
    assert_eq!(m.tells, "Song, close");
    assert_eq!(m.lines, vec!["ln1", "ln2"], "lines and timing follow the new plan");
    assert_eq!(m.planned_s, 6.);
    assert_eq!(m.frame.status, Status::Approved);
    assert!(m.locked.is_empty(), "no lock is written into the shot");
}

#[test]
fn a_reply_that_stops_early_never_makes_one_huge_shot() {
    let (sc, b) = (scene(), bible());
    let ls: Vec<Line> = (1..=14).map(|i| line(&format!("ln{i}"), if i % 2 == 0 { "jinwoo" } else { "song" }, "A few words here.")).collect();
    let inp = input(&sc, &ls, &b, false);
    let (shots, notes) = read_reply(&inp, &json!({"shots": [rs("ls", 4., &["ln1"]), rs("ms", 3., &["ln2"])]}));
    covered_once(&shots, &ls);
    assert!(shots.iter().all(|s| s.lines.len() <= mv_anime::checks::SHOT_MAX_LINES), "{:?}", shots.iter().map(|s| s.lines.len()).collect::<Vec<_>>());
    assert!(shots.len() > 3, "the left-out lines got cuts of their own");
    assert!(shots.iter().all(|s| s.marks.iter().any(|m| m.visible != Visibility::Offscreen)), "every added cut frames somebody");
    assert!(notes.iter().any(|n| n.code == "plan-extra-cuts"));
    floors_hold(&shots, &ls, sc.kind);
}

#[tokio::test]
async fn a_very_long_scene_is_planned_in_parts() {
    let b = bible();
    let ls: Vec<Line> = (1..=45).map(|i| line(&format!("ln{i}"), if i % 2 == 0 { "jinwoo" } else { "song" }, "Short words.")).collect();
    let mut sc = scene();
    sc.beats = vec![
        Beat { id: "bt1".into(), purpose: "first".into(), lines: ls[..32].iter().map(|l| l.id.clone()).collect(), ..Beat::default() },
        Beat { id: "bt2".into(), purpose: "second".into(), lines: ls[32..].iter().map(|l| l.id.clone()).collect(), ..Beat::default() },
    ];
    sc.target_s = 200.;
    let inp = input(&sc, &ls, &b, true);
    let canned = Arc::new(Canned::new((0..10).map(|_| r#"{"shots":[]}"#.to_string())));
    let plan = plan_scene(&Ai::fake(canned.clone()), &inp, &progress(), &CancellationToken::new()).await.unwrap_or_else(|e| panic!("{e:#}"));
    covered_once(&plan.shots, &ls);
    assert!(plan.notes.iter().any(|n| n.code == "plan-parts" && n.message.contains("2 parts")), "{:?}", plan.notes);
    let users: Vec<String> = canned.seen().iter().filter(|r| !r.messages[0].content.contains("THIS IS A REVISION")).map(|r| r.messages[1].content.clone()).collect();
    assert_eq!(users.len(), 2, "one planner call per part");
    assert!(users[0].contains("ln1 |") && !users[0].contains("ln40 |"), "the first part: the first beat");
    assert!(users[1].contains("ln40 |") && !users[1].contains("ln1 |"), "the second part: the rest");
    assert!(users[1].contains("RETURNS"), "only the first part opens the place");
}

#[test]
fn small_reply_mistakes_are_not_taken_literally() {
    let (sc, ls, b) = (scene(), lines(), bible());
    let inp = input(&sc, &ls, &b, false);
    let beat = |v: Value| {
        let mut s = rs("ms", 3., &["ln1"]);
        s["beat"] = v;
        read_reply(&inp, &json!({"shots": [s]})).0[0].beat.clone()
    };
    assert_eq!(beat(json!("bt1")), Some("bt1".into()));
    assert_eq!(beat(json!(1)), Some("bt1".into()));
    assert_eq!(beat(json!("1")), Some("bt1".into()));
    assert_eq!(beat(json!("bt0001")), None, "another scene's beat id is not this scene's first beat");
    // The template's example camera at (0, 0), on top of Song: placed from the size instead.
    let mut s = rs("ms", 3., &["ln1"]);
    s["camera"] = json!({"x": 0, "z": 0, "height": 1.5});
    s["marks"] = json!([{"char": "song", "x": 0.2, "z": 0, "facing": 180, "visible": "full"}]);
    let shot = &read_reply(&inp, &json!({"shots": [s]})).0[0];
    let p = shot.camera.pos.unwrap_or_default();
    assert!(p[2] > 0.5, "not on Song: {p:?}");
}

#[test]
fn the_basic_plan_frames_nobody_who_is_not_there_and_a_bridge_is_short() {
    let (sc, b) = (scene(), bible());
    let ls = vec![line("ln1", "song", "Is anyone here?"), line("ln2", "song", "Hello?"), line("ln3", "song", "Anyone?"), line("ln4", "song", "Hm.")];
    let nobody_else = Scene { summary: "Song alone.".into(), title: "Alone".into(), ..sc.clone() };
    let plan = director::fallback_plan(&input(&nobody_else, &ls, &b, false));
    covered_once(&plan, &ls);
    assert!(plan.iter().all(|s| s.marks.iter().any(|m| m.visible != Visibility::Offscreen)), "{:?}", plan.iter().map(|s| s.tells.clone()).collect::<Vec<_>>());
    // A bridge shot without its own length holds about 2 s, not a layout shot's length.
    let mut shots = vec![Shot { bridge: true, camera: Camera { size: ShotSize::Overhead, ..Camera::default() }, ..Shot::default() }];
    settle(&mut shots, &input(&sc, &[], &b, false));
    assert!((1.5..=2.5).contains(&shots[0].planned_s), "{}", shots[0].planned_s);
}

// ------------------------------------------------------------------------- the job

struct Project {
    _dir: tempfile::TempDir,
    store: Store,
}

fn project() -> Project {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("no temp folder: {e}"));
    let store = Store { id: "series1".into(), dir: dir.path().join("p") };
    let story = Story { episode: "ep01".into(), scenes: vec![scene()], lines: lines(), ..Story::default() };
    let ok = store.save_story(&story).and_then(|_| {
        let b = bible();
        for c in &b.characters {
            store.save_character(c)?;
        }
        for p in &b.places {
            store.save_place(p)?;
        }
        for p in &b.props {
            store.save_prop(p)?;
        }
        Ok(())
    });
    assert!(ok.is_ok(), "{ok:?}");
    Project { _dir: dir, store }
}

async fn run_job(store: &Store, ai: Ai, job_id: u64) -> anyhow::Result<()> {
    let job = Job { id: job_id, kind: JobKind::PlanScene { episode: "ep01".into(), scene: "sc0001".into() }, ..Job::default() };
    let run = RunCtx { ai: Box::new(move |_| Some(ai.clone())), image: Arc::new(NoImages), python: Box::new(|| Err(anyhow!("No voice Python in tests."))) };
    let progress = |_f: f32, _s: &str| {};
    let cancel = CancellationToken::new();
    let cx = JobCtx { job: &job, store, run: &run, progress: &progress, cancel: &cancel };
    director::run_job(&cx, "ep01", "sc0001").await
}

#[tokio::test]
async fn the_job_plans_saves_and_re_plans_keeping_locks() {
    let p = project();
    run_job(&p.store, Ai::canned([better()]), 1).await.unwrap_or_else(|e| panic!("{e:#}"));
    let saved = p.store.scene_shots("ep01", "sc0001").unwrap_or_default();
    assert_eq!(saved.shots.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), vec!["sh0001", "sh0002"]);
    assert_eq!(saved.revisions, 0);
    assert!(saved.planned_by.contains("mv-anime-fake-"));
    covered_once(&saved.shots, &lines());
    assert_eq!(p.store.series().unwrap_or_default().next_ids.get("shot"), Some(&2));
    assert!(p.store.checks("ep01").is_ok());

    // The user locks the lens of the first shot, then plans again.
    let mut edited = saved.clone();
    edited.shots[0].camera.lens_mm = 135;
    edited.shots[0].locked.insert("/camera/lens_mm".into());
    edited.shots[0].edited = true;
    assert!(p.store.save_scene_shots(&edited).is_ok());
    let again = reply(vec![rs("ls", 4.5, &["ln1", "ln2"]), rs("ms", 3., &["ln3"]), rs("mcu", 3., &["ln4"])]);
    run_job(&p.store, Ai::canned([again]), 2).await.unwrap_or_else(|e| panic!("{e:#}"));
    let saved = p.store.scene_shots("ep01", "sc0001").unwrap_or_default();
    assert_eq!(saved.shots.len(), 3);
    assert_eq!((saved.shots[0].id.as_str(), saved.shots[0].camera.lens_mm), ("sh0001", 135), "the lock survived the re-plan");
    let ids: BTreeSet<&str> = saved.shots.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids.len(), 3, "ids stay unique: {ids:?}");
    assert!(!ids.contains(""));
    covered_once(&saved.shots, &lines());

    // Junk: the job fails with the readable message and keeps the raw answer for the user.
    let err = run_job(&p.store, Ai::canned(["nonsense", "more nonsense"]), 3).await;
    assert!(err.as_ref().is_err_and(|e| e.to_string() == UNREADABLE), "{err:?}");
    let kept = std::fs::read_dir(p.store.work_dir().join("ai")).map(|d| d.count()).unwrap_or(0);
    assert_eq!(kept, 1);
    // The shots from before are untouched.
    assert_eq!(p.store.scene_shots("ep01", "sc0001").unwrap_or_default().shots.len(), 3);

    // No AI set up, or a scene that is gone: a message the user can act on.
    let job = Job::default();
    let run = RunCtx { ai: Box::new(|_| None), image: Arc::new(NoImages), python: Box::new(|| Err(anyhow!("none"))) };
    let progress = |_f: f32, _s: &str| {};
    let cancel = CancellationToken::new();
    let cx = JobCtx { job: &job, store: &p.store, run: &run, progress: &progress, cancel: &cancel };
    let e = director::run_job(&cx, "ep01", "sc0001").await.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(e.contains("engine settings"), "{e}");
    let e = director::run_job(&cx, "ep01", "sc9999").await.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(e.contains("no longer in the story"), "{e}");
}
