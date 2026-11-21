#![cfg(feature = "story")]
//! The Story job end to end (design 6.1) on the synthetic manga, with canned model answers:
//! a full run, a re-run that keeps the user's edits, a later chapter failing, junk and cut-off
//! answers, the length budget and "Adapt this scene again". Each test uses its own project.

mod common;

use std::sync::{Arc, OnceLock};

use anyhow::anyhow;
use mv_ai::{AiRole, AiStepResult, CancellationToken};
use mv_anime::backend::NoImages;
use mv_anime::llm::{Ai, Canned, UNREADABLE};
use mv_anime::queue::{Job, JobCtx, JobKind, RunCtx};
use mv_anime::story::{self, PROMPT};
use mv_anime::*;
use serde_json::json;

/// The data root of this test binary and the synthetic manga in it (made once).
fn manga() -> &'static common::Manga {
    static M: OnceLock<(tempfile::TempDir, common::Manga)> = OnceLock::new();
    &M.get_or_init(|| {
        let dir = common::temp_root();
        common::set_home(dir.path());
        std::env::set_var("MANGAVOX_LOG_DIR", dir.path().join("logs"));
        let m = common::build_text_only(dir.path());
        (dir, m)
    })
    .1
}

const EP: &str = "ep01";

/// A project for one test: the series and its one episode with both chapters.
fn project(id: &str, target_minutes: f32) -> Store {
    let m = manga();
    let store = Store::open(id);
    let series = Series { id: id.into(), manga_id: common::MANGA_ID.into(), manga_title: common::MANGA_TITLE.into(), ..Series::default() };
    store.save_series(&series).expect("series");
    let ep = Episode { id: EP.into(), title: "Episode 1".into(), chapters: m.picks(), target_minutes, ..Episode::default() };
    store.save_episode(&ep).expect("episode");
    store
}

fn text(t: impl Into<String>) -> AiStepResult {
    AiStepResult { text: t.into(), ..AiStepResult::default() }
}

/// An answer for one chapter: a talk (pages 0-1) and a fight (pages 2-3); `tag` marks every title.
fn answer(tag: &str) -> String {
    json!({"scenes": [
        {"title": format!("{tag} talk"), "place": "Tower gate", "time": "day", "kind": "dialogue", "tone": "drama", "fight": false,
         "colour": "neutral daylight", "intensity": 3, "target_s": 30, "summary": "Kai and Ren talk under the tower.",
         "source": [{"page": 0, "panel": 0}, {"page": 1, "panel": 2}],
         "beats": [
            {"purpose": "establish the tower", "hook": false, "source": [], "lines": []},
            {"purpose": "they talk", "hook": false, "source": [{"page": 0, "panel": 1}], "lines": [
                {"speaker": "Kai", "kind": "speech", "original": "Kai speaks on page 1 panel 2.", "spoken": format!("{tag}: look at it."),
                 "emotion": "awe", "strength": 2, "pace": "slow", "keep": false, "page": 0, "panel": 1},
                {"speaker": "Ren", "kind": "thought", "original": "Ren speaks on page 1 panel 2.", "spoken": "Ren speaks on page 1 panel 2.",
                 "emotion": "fear", "strength": 1, "pace": "normal", "keep": true, "page": 0, "panel": 1}
            ]}
         ]},
        {"title": format!("{tag} fight"), "place": "Rooftop", "time": "night", "kind": "action", "tone": "fight", "fight": true,
         "colour": "burning red", "intensity": 8, "target_s": 20, "summary": "The fight breaks out.",
         "source": [{"page": 2, "panel": 0}, {"page": 3, "panel": 2}],
         "beats": [
            {"purpose": "the strike", "hook": true, "source": [{"page": 2, "panel": 0}], "lines": [
                {"speaker": "Stranger", "kind": "speech", "original": "", "spoken": "Behind you!", "emotion": "fear", "strength": 3,
                 "pace": "fast", "keep": false, "page": null, "panel": null},
                {"speaker": null, "kind": "sfx", "original": "CRASH", "spoken": "CRASH", "emotion": "neutral", "strength": 2,
                 "pace": "fast", "keep": true, "page": 3, "panel": 2}
            ]}
         ]}
    ]})
    .to_string()
}

struct Ran {
    result: anyhow::Result<()>,
    canned: Arc<Canned>,
}

fn run_ctx(canned: &Arc<Canned>) -> RunCtx {
    let ai = Ai::fake(canned.clone());
    RunCtx { ai: Box::new(move |_| Some(ai.clone())), image: Arc::new(NoImages), python: Box::new(|| Err(anyhow!("No voice Python in tests."))) }
}

fn job(store: &Store) -> Job {
    Job { id: 42, project: store.id.clone(), kind: JobKind::Story { episode: EP.into() }, ..Job::default() }
}

/// Runs the Story job of `store` with these answers.
async fn adapt(store: &Store, answers: Vec<AiStepResult>) -> Ran {
    let canned = Arc::new(Canned::from_results(answers));
    let run = run_ctx(&canned);
    let job = job(store);
    let progress = |_: f32, _: &str| {};
    let cancel = CancellationToken::new();
    let cx = JobCtx { job: &job, store, run: &run, progress: &progress, cancel: &cancel };
    let result = story::run_job(&cx, EP).await;
    Ran { result, canned }
}

fn chapter_scenes<'a>(s: &'a Story, chapter: &str) -> Vec<&'a Scene> {
    s.scenes.iter().filter(|sc| sc.source.first().is_some_and(|r| r.chapter_id == chapter)).collect()
}

fn seconds(scenes: &[&Scene]) -> f32 {
    scenes.iter().map(|s| s.target_s).sum()
}

#[tokio::test]
async fn every_chapter_is_adapted_with_stable_ids_and_saved() {
    let store = project("p-good", 0.);
    let ran = adapt(&store, vec![text(answer("A")), text(answer("B"))]).await;
    ran.result.expect("adapted");
    let s = store.story(EP).expect("story");
    let ids: Vec<&str> = s.scenes.iter().map(|sc| sc.id.as_str()).collect();
    assert_eq!(ids, vec!["sc0001", "sc0002", "sc0003", "sc0004"]);
    let titles: Vec<&str> = s.scenes.iter().map(|sc| sc.title.as_str()).collect();
    assert_eq!(titles, vec!["A talk", "A fight", "B talk", "B fight"]);
    assert_eq!(chapter_scenes(&s, "c1").len(), 2);
    assert_eq!(chapter_scenes(&s, "c2").len(), 2);
    // Lines: unique ids, each in its scene's beats, speakers mapped to the cast.
    assert_eq!(s.lines.len(), 8);
    let line_ids: std::collections::BTreeSet<&str> = s.lines.iter().map(|l| l.id.as_str()).collect();
    assert_eq!(line_ids.len(), 8);
    for l in &s.lines {
        let sc = s.scenes.iter().find(|sc| sc.id == l.scene).expect("its scene");
        assert!(sc.beats.iter().any(|b| b.lines.contains(&l.id)), "{} sits in a beat", l.id);
    }
    let speakers: Vec<Option<&str>> = s.lines.iter().take(4).map(|l| l.speaker.as_deref()).collect();
    assert_eq!(speakers, vec![Some("kai"), Some("ren"), Some("Stranger"), None]);
    assert_eq!(s.lines[2].source, None);
    assert_eq!(s.lines[3].source, Some(SourceRef { chapter_id: "c1".into(), page: 3, panel: Some(2) }));
    assert!(s.model.contains("mv-anime-fake"), "{}", s.model);
    assert!(s.made_at > 0);
    // Checks are saved with the story.
    let found = story::issues(&s);
    assert!(found.iter().any(|i| i.code == "unknown_speaker" && i.message.contains("Stranger")), "{found:?}");
    // The project's id counters moved past what was used.
    let series = store.series().expect("series");
    assert!(series.next_ids.get("scene").copied().unwrap_or(0) >= 5, "{:?}", series.next_ids);
    assert!(series.next_ids.get("line").copied().unwrap_or(0) >= 9, "{:?}", series.next_ids);
    // Auto length: 6 minutes for each of these short chapters.
    assert!((seconds(&chapter_scenes(&s, "c1")) - 360.).abs() < 0.5, "{}", seconds(&chapter_scenes(&s, "c1")));
    assert!((seconds(&chapter_scenes(&s, "c2")) - 360.).abs() < 0.5);
    // One call per chapter, in order, the second with the first's last scene as context.
    let seen = ran.canned.seen();
    assert_eq!(seen.len(), 2);
    assert_eq!((seen[0].messages[0].role, seen[0].messages[0].content.as_str()), (AiRole::System, PROMPT));
    let first = &seen[0].messages[1].content;
    assert!(first.contains("CHAPTER: Chapter 1 (chapter 1 of 2"), "{first}");
    assert!(first.contains("Kai (also called The Hunter)"), "the cast with aliases: {first}");
    assert!(first.contains("[p0]") && first.contains("[p3]"), "the whole digest");
    assert!(first.contains("LENGTH BUDGET: about 360 seconds"), "{first}");
    assert!(!first.contains("THE SCENE JUST BEFORE"));
    let second = &seen[1].messages[1].content;
    assert!(second.contains("CHAPTER: Chapter 2") && second.contains("THE SCENE JUST BEFORE") && second.contains("Title: A fight"), "{second}");
    assert!(!second.contains("CRASH\"") && second.contains("Stranger: \"Behind you!\""), "sound effects are not context: {second}");
    assert!(seen.iter().all(|r| r.images.is_empty()));
}

#[tokio::test]
async fn a_rerun_keeps_edited_and_locked_scenes_and_lines() {
    let store = project("p-rerun", 0.);
    adapt(&store, vec![text(answer("A")), text(answer("A"))]).await.result.expect("first run");
    let mut s = store.story(EP).expect("story");
    // The user retitles the first scene of chapter 1 and locks a line of chapter 2's talk.
    s.scenes[0].title = "My title".into();
    s.scenes[0].edited = true;
    s.scenes[0].locked.insert("/title".into());
    let talk2 = s.scenes[2].id.clone();
    let line = s.lines.iter_mut().find(|l| l.scene == talk2).expect("a line");
    line.spoken = "My own words.".into();
    line.edited = true;
    line.locked.insert("/spoken".into());
    let locked_line = line.id.clone();
    store.save_story(&s).expect("saved");
    let before = store.story(EP).expect("story");

    adapt(&store, vec![text(answer("B")), text(answer("B"))]).await.result.expect("second run");
    let after = store.story(EP).expect("story");
    let titles: Vec<&str> = after.scenes.iter().map(|sc| sc.title.as_str()).collect();
    assert_eq!(titles, vec!["My title", "B fight", "A talk", "B fight"], "kept, replaced, kept, replaced");
    // Kept scenes and every line in them are exactly as the user left them.
    for keep in [&before.scenes[0], &before.scenes[2]] {
        let now = after.scenes.iter().find(|sc| sc.id == keep.id).expect("kept scene");
        assert_eq!(now, keep);
        let old_lines: Vec<&Line> = before.lines.iter().filter(|l| l.scene == keep.id).collect();
        let new_lines: Vec<&Line> = after.lines.iter().filter(|l| l.scene == keep.id).collect();
        assert_eq!(old_lines, new_lines);
    }
    assert!(after.lines.iter().any(|l| l.id == locked_line && l.spoken == "My own words."));
    // The untouched scenes were replaced, each by the new scene starting on its page, which takes
    // its id (so a plan made for it stays attached); the new talk scenes gave way to the kept ones.
    assert_eq!(after.scenes[1].id, before.scenes[1].id);
    assert_eq!(after.scenes[3].id, before.scenes[3].id);
    assert!(!after.lines.iter().any(|l| l.scene == before.scenes[1].id && l.spoken == "Behind you!" && before.lines.iter().any(|b| b.id == l.id)), "its lines are the new ones");
    assert!(!after.lines.iter().any(|l| l.spoken == "B: look at it."));
    assert!(after.lines.iter().all(|l| after.scenes.iter().any(|sc| sc.id == l.scene)));
}

#[tokio::test]
async fn a_failure_in_a_later_chapter_keeps_the_earlier_ones() {
    let store = project("p-fail", 0.);
    // First run fails on chapter 2: chapter 1 is saved anyway.
    let down = || Canned::error("The AI service is down.");
    let ran = adapt(&store, vec![text(answer("A")), down(), down(), down()]).await;
    let e = ran.result.expect_err("chapter 2 fails");
    let msg = e.to_string();
    assert!(msg.contains("Could not adapt Chapter 2") && msg.contains("The AI service is down.") && msg.contains("kept"), "{msg}");
    let s = store.story(EP).expect("story");
    assert_eq!(s.scenes.iter().map(|sc| sc.title.as_str()).collect::<Vec<_>>(), vec!["A talk", "A fight"]);

    // A complete run, then one that fails on chapter 2 again: chapter 1 is new, chapter 2 as before.
    adapt(&store, vec![text(answer("A")), text(answer("A"))]).await.result.expect("complete");
    let before = store.story(EP).expect("story");
    let ran = adapt(&store, vec![text(answer("B")), down(), down(), down()]).await;
    assert!(ran.result.is_err());
    let after = store.story(EP).expect("story");
    let titles: Vec<&str> = after.scenes.iter().map(|sc| sc.title.as_str()).collect();
    assert_eq!(titles, vec!["B talk", "B fight", "A talk", "A fight"]);
    let c2_before: Vec<&Scene> = chapter_scenes(&before, "c2");
    assert_eq!(chapter_scenes(&after, "c2"), c2_before, "chapter 2 untouched");
}

fn saved_answers(store: &Store) -> Vec<std::path::PathBuf> {
    let dir = store.work_dir().join("ai");
    std::fs::read_dir(&dir).map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect()).unwrap_or_default()
}

#[tokio::test]
async fn junk_answers_fail_cleanly_with_a_readable_message() {
    let store = project("p-junk", 0.);
    let ran = adapt(&store, vec![text("What a lovely chapter! I would adapt it with care."), text("Sorry, I cannot do JSON today.")]).await;
    let msg = ran.result.expect_err("unreadable").to_string();
    assert!(msg.starts_with("Could not adapt Chapter 1: "), "{msg}");
    assert!(msg.contains(UNREADABLE) && msg.contains("work/ai/"), "{msg}");
    assert!(!msg.contains("kept."), "nothing before it to keep: {msg}");
    let files = saved_answers(&store);
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(std::fs::read_to_string(&files[0]).unwrap_or_default(), "Sorry, I cannot do JSON today.");
    assert!(store.story(EP).expect("story").scenes.is_empty(), "nothing saved");
    assert_eq!(ran.canned.seen().len(), 2, "one nudge, no more");

    // Valid JSON without scenes fails the same way.
    let store = project("p-noscenes", 0.);
    let ran = adapt(&store, vec![text(r#"{"note": "I summarised it instead."}"#)]).await;
    let msg = ran.result.expect_err("no scenes").to_string();
    assert!(msg.contains("no scenes") && msg.contains("Chapter 1"), "{msg}");
    assert_eq!(saved_answers(&store).len(), 1);
}

/// The fight scene of [`answer`] alone (pages 2-3).
fn fight_only(tag: &str) -> String {
    let full: serde_json::Value = serde_json::from_str(&answer(tag)).unwrap_or_default();
    json!({"scenes": [full["scenes"][1].clone()]}).to_string()
}

/// [`answer`] cut off at the start of its second scene (the fight on pages 2-3 never arrives).
fn cut_answer(tag: &str) -> String {
    let full = answer(tag);
    let at = full.find("{\"beats\":[{\"hook\":true").unwrap_or(full.len() / 2);
    full[..at + 14].to_string()
}

#[tokio::test]
async fn a_cut_off_answer_is_salvaged_and_the_rest_asked_for() {
    let store = project("p-cut", 0.);
    let ran = adapt(&store, vec![text(cut_answer("A")), text(fight_only("A")), text(answer("B"))]).await;
    ran.result.expect("adapted");
    let seen = ran.canned.seen();
    assert_eq!(seen.len(), 3, "no nudge: the cut-off answer was read; one more call for the pages it missed");
    let again = &seen[1].messages[1].content;
    assert!(again.contains("stopped before its last pages") && again.contains("[p2]") && again.contains("[p3]") && !again.contains("[p0]"), "{again}");
    assert!(again.contains("THE SCENE JUST BEFORE") && again.contains("Title: A talk"), "{again}");
    let s = store.story(EP).expect("story");
    let c1 = chapter_scenes(&s, "c1");
    assert_eq!(c1.iter().map(|sc| sc.title.as_str()).collect::<Vec<_>>(), vec!["A talk", "A fight"]);
    assert_eq!(s.lines.iter().filter(|l| l.scene == c1[0].id).count(), 2, "the finished scene is whole");
    assert_eq!(chapter_scenes(&s, "c2").len(), 2);
    assert!(!story::issues(&s).iter().any(|i| i.code == story::PAGES_LEFT_OUT));
}

#[tokio::test]
async fn pages_still_missing_after_asking_again_are_named() {
    let store = project("p-cut2", 0.);
    let nothing = json!({"scenes": [{"title": "Filler", "summary": "Nothing happens."}]}).to_string();
    let ran = adapt(&store, vec![text(cut_answer("A")), text(nothing), text(answer("B"))]).await;
    ran.result.expect("adapted anyway");
    let s = store.story(EP).expect("story");
    let warned: Vec<Issue> = story::issues(&s).into_iter().filter(|i| i.code == story::PAGES_LEFT_OUT).collect();
    assert_eq!(warned.len(), 1, "{warned:?}");
    assert_eq!(warned[0].severity, Severity::Warning);
    assert!(warned[0].message.contains("pages 3-4 of Chapter 1"), "{}", warned[0].message);
}

#[tokio::test]
async fn a_scene_whose_shots_hold_the_users_work_is_kept() {
    let store = project("p-shots", 0.);
    adapt(&store, vec![text(answer("A")), text(answer("A"))]).await.result.expect("first run");
    let s = store.story(EP).expect("story");
    // Chapter 1's fight has an approved picture; chapter 2's fight only a plan.
    let pictured = Shot { id: "sh0001".into(), frame: Card { status: Status::Approved, takes: vec!["t1".into()], selected: Some("t1".into()), ..Card::default() }, ..Shot::default() };
    store.save_scene_shots(&SceneShots { episode: EP.into(), scene: s.scenes[1].id.clone(), shots: vec![pictured], ..SceneShots::default() }).expect("shots");
    store.save_scene_shots(&SceneShots { episode: EP.into(), scene: s.scenes[3].id.clone(), shots: vec![Shot { id: "sh0002".into(), ..Shot::default() }], ..SceneShots::default() }).expect("shots");
    assert_eq!(story::replaced_with_shots(&store, EP), 1, "only the scene with a plan and nothing of the user's would be replaced");

    adapt(&store, vec![text(answer("B")), text(answer("B"))]).await.result.expect("second run");
    let after = store.story(EP).expect("story");
    let titles: Vec<&str> = after.scenes.iter().map(|sc| sc.title.as_str()).collect();
    assert_eq!(titles, vec!["B talk", "A fight", "B talk", "B fight"], "the pictured scene stays as it was");
    assert_eq!(after.scenes[1], s.scenes[1]);
    assert_eq!(after.scenes[3].id, s.scenes[3].id, "the plan stays attached to the scene on its page");
    assert_eq!(story::replaced_with_shots(&store, EP), 1);
}

#[tokio::test]
async fn the_episode_target_is_shared_by_the_chapters() {
    let store = project("p-budget", 4.);
    adapt(&store, vec![text(answer("A")), text(answer("B"))]).await.result.expect("adapted");
    let s = store.story(EP).expect("story");
    for ch in ["c1", "c2"] {
        let total = seconds(&chapter_scenes(&s, ch));
        assert!((total - 120.).abs() < 0.5, "{ch}: {total}");
    }
    // The AI's proportions survive the scaling (30 : 20).
    let c1 = chapter_scenes(&s, "c1");
    assert!((c1[0].target_s / c1[1].target_s - 1.5).abs() < 0.05, "{} / {}", c1[0].target_s, c1[1].target_s);
}

#[tokio::test]
async fn adapting_one_scene_again_keeps_its_locks_and_edits() {
    let store = project("p-scene", 0.);
    adapt(&store, vec![text(answer("A")), text(answer("A"))]).await.result.expect("adapted");
    let mut s = store.story(EP).expect("story");
    let id = s.scenes[1].id.clone();
    s.scenes[1].tone = Tone::Horror;
    s.scenes[1].edited = true;
    s.scenes[1].locked.insert("/tone".into());
    let edited = s.lines.iter_mut().find(|l| l.scene == id && l.spoken == "Behind you!").expect("line");
    edited.spoken = "Watch out!".into();
    edited.edited = true;
    let edited_id = edited.id.clone();
    store.save_story(&s).expect("saved");
    let before = store.story(EP).expect("story");

    let again = json!({"scenes": [{"title": "Again fight", "kind": "action", "tone": "comedy", "fight": true, "colour": "burning red",
        "intensity": 9, "target_s": 25, "summary": "Kai fights back.", "source": [{"page": 2, "panel": 0}],
        "beats": [{"purpose": "Kai answers", "source": [{"page": 2, "panel": 1}], "lines": [
            {"speaker": "Kai", "kind": "speech", "original": "Kai speaks on page 3 panel 2.", "spoken": "Not today.", "emotion": "resolve",
             "strength": 3, "pace": "fast", "keep": false, "page": 2, "panel": 1}]}]}]})
    .to_string();
    let canned = Arc::new(Canned::new([again]));
    let run = run_ctx(&canned);
    let job = job(&store);
    let progress = |_: f32, _: &str| {};
    let cancel = CancellationToken::new();
    let cx = JobCtx { job: &job, store: &store, run: &run, progress: &progress, cancel: &cancel };
    story::adapt_scene(&cx, EP, &id).await.expect("adapted again");

    let user = &canned.seen()[0].messages[1].content;
    assert!(user.contains("exactly ONE scene") && user.contains("[p2]") && user.contains("[p3]") && !user.contains("[p0]"), "{user}");
    assert!(user.contains("THE SCENE JUST BEFORE") && user.contains("Title: A talk"), "{user}");
    let after = store.story(EP).expect("story");
    let sc = after.scenes.iter().find(|sc| sc.id == id).expect("same id");
    assert_eq!(sc.title, "Again fight");
    assert_eq!(sc.tone, Tone::Horror, "the locked tone stays");
    assert!(sc.edited && sc.locked.contains("/tone"));
    assert!(after.lines.iter().any(|l| l.id == edited_id && l.spoken == "Watch out!" && l.scene == id), "the edited line stays");
    assert!(after.lines.iter().any(|l| l.spoken == "Not today." && l.scene == id && l.speaker.as_deref() == Some("kai")));
    assert!(sc.target_s >= 2.);
    // Every other scene and line is untouched.
    for (b, a) in before.scenes.iter().zip(&after.scenes) {
        if b.id != id {
            assert_eq!(a, b);
        }
    }
    for l in before.lines.iter().filter(|l| l.scene != id) {
        assert!(after.lines.contains(l), "{}", l.id);
    }
    assert!(store.dir.join("episodes").join(EP).join("story.json").is_file());
}
