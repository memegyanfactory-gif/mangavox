//! The whole engine in one go (design 6 to 8), on the synthetic manga with canned AI answers
//! and a small fake picture backend: STORY, then BIBLE, then SHOTS for one scene, then FRAMES
//! for its shots (a sheet first, so a character has an identity picture), then the CUT plan.
//! Every job runs through `queue::execute`, as the app's runner does.
//!
//! Each stage's output must be accepted by the next, a second run of each stage keeps what the
//! user edited, and the picture channel follows the design (5.5): a new sheet for a character
//! outdates that character's frames only, a new voice outdates no picture.
//!
//! This binary owns its process: the data root points at a temp folder.
#![cfg(all(feature = "story", feature = "bible", feature = "shots", feature = "frames", feature = "cut"))]

mod common;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use futures::future::BoxFuture;
use mv_ai::CancellationToken;
use mv_anime::animatic::{self, CutPlan};
use mv_anime::backend::{ImageBackend, ImageError, NoImages};
use mv_anime::frames;
use mv_anime::llm::Ai;
use mv_anime::queue::{self, Job, JobCtx, JobKind, RunCtx, Tier};
use mv_anime::{sheets, voices};
use mv_anime::*;
use serde_json::{json, Value};

const PROJECT: &str = "e2e";
const EP: &str = "ep01";
const SCENE: &str = "sc0001";

/// A small picture, different on every call.
struct Pictures {
    calls: AtomicU32,
}

fn png(seed: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(16, 9, |x, y| image::Rgb([(seed % 251) as u8, (x * 13 + seed) as u8, (y * 29) as u8]));
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).expect("encode png");
    out
}

impl ImageBackend for Pictures {
    fn generate<'a>(
        &'a self,
        req: &'a GenRequest,
        files: &'a [(std::path::PathBuf, RefSlot)],
        count: u8,
        on_status: Box<dyn Fn(&str) + Send + Sync + 'a>,
    ) -> BoxFuture<'a, Result<Vec<Vec<u8>>, ImageError>> {
        Box::pin(async move {
            on_status("drawing");
            assert!(!req.prompt.trim().is_empty(), "a request always has a prompt");
            assert_eq!(files.len(), req.refs.len(), "every reference of the request is uploaded");
            assert!(files.iter().all(|(p, _)| p.is_file()), "every reference file exists");
            Ok((0..count.max(1)).map(|_| png(self.calls.fetch_add(1, Ordering::SeqCst) + 1)).collect())
        })
    }
}

/// The data root of this binary and its synthetic manga (pages and panel crops included).
fn manga() -> &'static common::Manga {
    static M: std::sync::OnceLock<(tempfile::TempDir, common::Manga)> = std::sync::OnceLock::new();
    &M.get_or_init(|| {
        let dir = common::temp_root();
        common::set_home(dir.path());
        std::env::set_var("MANGAVOX_LOG_DIR", dir.path().join("logs"));
        let m = common::build(dir.path());
        (dir, m)
    })
    .1
}

fn project() -> Store {
    let m = manga();
    let store = Store::open(PROJECT);
    let series = Series {
        id: PROJECT.into(),
        manga_id: common::MANGA_ID.into(),
        manga_title: common::MANGA_TITLE.into(),
        episodes: vec![EpisodeRef { id: EP.into(), title: "Episode 1".into() }],
        ..Series::default()
    };
    store.save_series(&series).expect("series");
    store.save_episode(&Episode { id: EP.into(), title: "Episode 1".into(), chapters: m.picks(), ..Episode::default() }).expect("episode");
    store
}

/// Runs one job the way the app's runner does (through `queue::execute`).
async fn run(store: &Store, kind: JobKind, answers: &[String], image: Arc<dyn ImageBackend>) -> anyhow::Result<()> {
    let ai = Ai::canned(answers.to_vec());
    let ctx = RunCtx { ai: Box::new(move |_: Tier| Some(ai.clone())), image, python: Box::new(|| Err(anyhow::anyhow!("No voice Python in tests."))) };
    let job = Job { id: 1, project: store.id.clone(), kind, ..Job::default() };
    let progress = |_: f32, _: &str| {};
    let cancel = CancellationToken::new();
    let cx = JobCtx { job: &job, store, run: &ctx, progress: &progress, cancel: &cancel };
    queue::execute(&cx).await
}

// ------------------------------------------------------------------------- canned answers

/// The Story answer for one chapter: a talk (pages 0-1) and a fight (pages 2-3).
fn story_answer(tag: &str) -> String {
    json!({"scenes": [
        {"title": format!("{tag} talk"), "place": "Tower roof", "time": "night", "kind": "dialogue", "tone": "drama", "fight": false,
         "colour": "deep blue threat", "intensity": 3, "target_s": 30, "summary": "Kai and Ren talk on the roof.",
         "source": [{"page": 0, "panel": 0}, {"page": 1, "panel": 2}],
         "beats": [{"purpose": "they talk", "hook": false, "source": [{"page": 0, "panel": 1}], "lines": [
            {"speaker": "Kai", "kind": "speech", "original": "Kai speaks on page 1 panel 2.", "spoken": format!("{tag}: look at the tower."),
             "emotion": "awe", "strength": 2, "pace": "slow", "keep": false, "page": 0, "panel": 1},
            {"speaker": "Ren", "kind": "speech", "original": "Ren speaks on page 1 panel 2.", "spoken": "Ren speaks on page 1 panel 2.",
             "emotion": "fear", "strength": 1, "pace": "normal", "keep": true, "page": 0, "panel": 1}
         ]}]},
        {"title": format!("{tag} fight"), "place": "Tower roof", "time": "night", "kind": "action", "tone": "fight", "fight": true,
         "colour": "burning red", "intensity": 8, "target_s": 20, "summary": "The fight breaks out.",
         "source": [{"page": 2, "panel": 0}, {"page": 3, "panel": 2}],
         "beats": [{"purpose": "the strike", "hook": true, "source": [{"page": 2, "panel": 0}], "lines": [
            {"speaker": null, "kind": "sfx", "original": "CRASH", "spoken": "CRASH", "emotion": "neutral", "strength": 2,
             "pace": "fast", "keep": true, "page": 3, "panel": 2}
         ]}]}
    ]})
    .to_string()
}

/// The Bible answer: Kai and Ren, the roof (set for scene 1), a dagger, the style.
fn bible_answer(description: &str) -> String {
    json!({
        "characters": [
            {"brain_id": "kai", "name": "Kai", "role": "hero", "age": "about 17", "description": description,
             "look": {"hair": "short black hair", "eyes": "dark grey eyes", "build": "lean", "outfit": "grey hoodie",
                      "signature_colour": "#C82828", "keep": ["scar over his left eyebrow"], "height_m": 1.75},
             "voice": {"gender": "male", "age": "teen", "type": "calm, low"},
             "best_panels": [{"chapter": "c1", "page": 0, "panel": 0, "angle": "front", "framing": "face"}]},
            {"brain_id": "ren", "name": "Ren", "role": "threat", "description": "A cold rival.",
             "look": {"hair": "silver hair", "signature_colour": "#2846C8"},
             "voice": {"gender": "female", "age": "adult", "type": "cold"}, "best_panels": []}
        ],
        "places": [{"name": "Tower roof", "kind": "exterior", "description": "A flat concrete roof.", "size_m": [20, 14],
            "landmarks": [{"name": "Water tank", "kind": "feature", "x": 6, "z": -5, "w": 3, "d": 3, "h": 4}],
            "stations": [{"name": "Stair door", "x": 0, "z": 6, "height": 1.5, "yaw": 0, "lens": 24}],
            "states": [{"id": "night", "name": "night", "key_dir_deg": 300, "kelvin": 9000, "mood": "cold"}],
            "scenes": [SCENE], "panels": [{"chapter": "c1", "page": 0, "panel": 1}]}],
        "props": [{"name": "Dagger", "owner": "Kai", "description": "A short black dagger.", "held": true, "states": ["drawn"], "panels": []}],
        "style": {"line": "Thin navy lines, two-tone cel shading.", "negative": ["text"]}
    })
    .to_string()
}

/// The planner's answer for scene 1: a wide of both on line 1, Ren alone on line 2.
fn shots_answer(lines: &[String]) -> String {
    let mark = |c: &str, x: f32, facing: f32| json!({"char": c, "x": x, "z": 0, "facing": facing, "pose": "stand", "visible": "full", "emotion": "resolve", "strength": 2});
    json!({"shots": [
        {"tells": "Kai and Ren face each other on the roof", "size": "ls", "angle": "eye", "station": null,
         "camera": {"x": 0.0, "z": 6.0, "height": 1.5}, "lens": 24, "look_at": "char:kai",
         "marks": [mark("kai", -1.2, 90.), mark("ren", 1.2, 270.)], "lines": [lines[0].clone()],
         "source": [{"page": 0, "panel": 1}], "background": "none", "mouth": "flap", "motion": "mouth_only", "seconds": 4.5},
        {"tells": "Ren answers", "size": "ms", "angle": "eye", "station": null,
         "camera": {"x": 1.5, "z": 3.0, "height": 1.5}, "look_at": "char:ren",
         "marks": [mark("ren", 1.2, 180.)], "lines": [lines[1].clone()], "background": "none", "mouth": "flap", "motion": "mouth_only", "seconds": 3.5}
    ]})
    .to_string()
}

fn three(s: String) -> Vec<String> {
    vec![s.clone(), s.clone(), s]
}

fn shot(store: &Store, id: &str) -> Shot {
    store.scene_shots(EP, SCENE).expect("shots").shots.into_iter().find(|s| s.id == id).unwrap_or_else(|| panic!("shot {id}"))
}

fn character(store: &Store, id: &str) -> Character {
    store.bible().expect("bible").character(id).cloned().unwrap_or_else(|| panic!("character {id}"))
}

fn turnaround(c: &Character) -> Card {
    c.sheets.iter().find(|s| s.kind == SheetKind::Turnaround).map(|s| s.card.clone()).expect("turnaround sheet")
}

// ------------------------------------------------------------------------- the run

#[tokio::test(flavor = "multi_thread")]
async fn a_chapter_goes_from_manga_to_cut_plan_and_user_edits_survive_every_rerun() {
    let store = project();
    let pictures: Arc<dyn ImageBackend> = Arc::new(Pictures { calls: AtomicU32::new(0) });

    // ---- STORY: two chapters adapted; the user retitles scene 1, then the story is adapted again.
    let story_job = JobKind::Story { episode: EP.into() };
    run(&store, story_job.clone(), &[story_answer("A"), story_answer("A")], Arc::new(NoImages)).await.expect("story");
    let mut story = store.story(EP).expect("story saved");
    assert_eq!(story.scenes.len(), 4);
    assert_eq!(story.scenes[0].id, SCENE);
    let scene_lines: Vec<String> = story.lines.iter().filter(|l| l.scene == SCENE).map(|l| l.id.clone()).collect();
    assert_eq!(scene_lines.len(), 2);
    assert_eq!(story.lines.iter().find(|l| l.id == scene_lines[0]).and_then(|l| l.speaker.as_deref()), Some("kai"), "speakers are the brain's ids");
    story.scenes[0].title = "My title".into();
    story.scenes[0].edited = true;
    story.scenes[0].locked.insert("/title".into());
    store.save_story(&story).expect("user edit");
    run(&store, story_job, &[story_answer("B"), story_answer("B")], Arc::new(NoImages)).await.expect("story again");
    let story = store.story(EP).expect("story");
    let first = story.scenes.iter().find(|s| s.id == SCENE).expect("the edited scene stays");
    assert_eq!(first.title, "My title", "a second Story run keeps the user's title");
    assert_eq!(story.lines.iter().filter(|l| l.scene == SCENE).map(|l| l.id.clone()).collect::<Vec<_>>(), scene_lines, "and its lines");

    // ---- BIBLE: built from the story's chapters; the scene the AI places gets the place.
    let bible_job = JobKind::Bible { episode: EP.into() };
    run(&store, bible_job.clone(), &[bible_answer("A quiet hunter.")], Arc::new(NoImages)).await.expect("bible");
    let bible = store.bible().expect("bible");
    let place = bible.places.first().expect("a place").clone();
    assert_eq!(store.story(EP).expect("story").scenes[0].place.as_deref(), Some(place.id.as_str()), "the Story's scene is set in the Bible's place");
    assert!(place.stations.iter().any(|s| s.overhead));
    let voiced: Vec<&str> = bible.cast.voices.iter().map(|v| v.character.as_str()).collect();
    assert!(voiced.contains(&"kai") && voiced.contains(&"ren"), "speakers get a cast row: {voiced:?}");
    // The Sound step casts from the Bible's voice brief (Ren's description names no gender).
    let cast = voices::default_cast(&bible.characters, &mv_recap::series::VoiceCast::default(), &std::collections::HashSet::new());
    let ren_voice = cast.voices.iter().find(|v| v.character == "ren").map(|v| v.voice_id.clone()).unwrap_or_default();
    assert!(voices::female_voices().contains(&ren_voice.as_str()), "Ren's brief says female: {ren_voice}");
    let mut kai = character(&store, "kai");
    kai.description = "My own description.".into();
    kai.locked.insert("/description".into());
    store.save_character(&kai).expect("user edit");
    run(&store, bible_job, &[bible_answer("Another description.")], Arc::new(NoImages)).await.expect("bible again");
    assert_eq!(character(&store, "kai").description, "My own description.", "a second Bible run keeps the user's description");

    // ---- SHOTS: scene 1 planned from the story's lines in the bible's place.
    let plan_job = JobKind::PlanScene { episode: EP.into(), scene: SCENE.into() };
    run(&store, plan_job.clone(), &three(shots_answer(&scene_lines)), Arc::new(NoImages)).await.expect("plan");
    let planned = store.scene_shots(EP, SCENE).expect("shots");
    let ids: Vec<&str> = planned.shots.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, vec!["sh0001", "sh0002"]);
    let mut covered: Vec<String> = planned.shots.iter().flat_map(|s| s.lines.clone()).collect();
    covered.sort();
    let mut want = scene_lines.clone();
    want.sort();
    assert_eq!(covered, want, "every line of the story lands in exactly one shot");
    assert!(planned.shots[0].marks.iter().any(|m| m.character == "kai"), "marks name the bible's characters");
    assert!(store.checks(EP).is_ok());
    let mut edited = planned.clone();
    edited.shots[0].camera.lens_mm = 135;
    edited.shots[0].locked.insert("/camera/lens_mm".into());
    edited.shots[0].edited = true;
    store.save_scene_shots(&edited).expect("user edit");
    run(&store, plan_job, &three(shots_answer(&scene_lines)), Arc::new(NoImages)).await.expect("plan again");
    assert_eq!(shot(&store, "sh0001").camera.lens_mm, 135, "a second plan keeps the user's lens");
    // The shot without the user's work was replaced under a new id.
    let replanned: Vec<String> = store.scene_shots(EP, SCENE).expect("shots").shots.iter().map(|s| s.id.clone()).collect();
    assert_eq!(replanned.len(), 2);
    assert_eq!(replanned[0], "sh0001");
    let ren_shot = replanned[1].clone();
    assert!(shot(&store, &ren_shot).marks.iter().all(|m| m.character == "ren"));

    // ---- FRAMES: Kai's turnaround first (his identity picture in a wide), then both stills.
    let kai_turnaround = JobKind::Sheet { target: Target::Sheet { owner: "kai".into(), sheet: "turnaround".into() }, variants: 1 };
    run(&store, kai_turnaround.clone(), &[], pictures.clone()).await.expect("sheet");
    let sheet = turnaround(&character(&store, "kai"));
    assert!(sheet.selected.is_some());
    assert!(!sheets::outdated(&sheet, &store), "a sheet just drawn is current");
    // The user approves it: only an approved sheet stands for Kai in a frame.
    let mut kai = character(&store, "kai");
    for s in kai.sheets.iter_mut().filter(|s| s.kind == SheetKind::Turnaround) {
        s.card.status = Status::Approved;
    }
    store.save_character(&kai).expect("approve the sheet");
    for id in ["sh0001", ren_shot.as_str()] {
        run(&store, JobKind::Frame { episode: EP.into(), shot: id.into(), variants: 1 }, &[], pictures.clone()).await.expect("frame");
    }
    let s1 = shot(&store, "sh0001");
    assert_eq!(s1.frame.takes.len(), 1);
    let take = store.take(s1.frame.selected.as_deref().expect("selected")).expect("take");
    let req = take.request.clone().expect("request kept");
    let roles: Vec<&str> = req.refs.iter().map(|r| r.1.as_str()).collect();
    assert!(roles.contains(&"identity:kai"), "the bible's sheet is Kai's identity picture: {roles:?}");
    assert!(roles.contains(&"panel"), "the manga panel guides the layout: {roles:?}");
    assert!(!frames::outdated(&s1.frame, &store));
    // The user writes their own prompt and draws again: the prompt stays theirs.
    frames::set_prompt(&store, EP, "sh0001", Some("Kai and Ren on a night roof, my words.")).expect("user prompt");
    run(&store, JobKind::Frame { episode: EP.into(), shot: "sh0001".into(), variants: 1 }, &[], pictures.clone()).await.expect("frame again");
    let s1 = shot(&store, "sh0001");
    assert_eq!(s1.frame.gen.prompt_user.as_deref(), Some("Kai and Ren on a night roof, my words."), "a second Frame run keeps the user's prompt");
    assert_eq!(s1.frame.takes.len(), 2);
    let newest = s1.frame.takes.last().cloned().expect("new take");
    assert_eq!(store.take(&newest).and_then(|t| t.request).map(|r| r.prompt).as_deref(), Some("Kai and Ren on a night roof, my words."));
    frames::select_take(&store, EP, "sh0001", &newest).expect("select");

    // ---- OUTDATED: a new voice changes no picture; a new sheet for Kai outdates Kai's frames only.
    let fresh = |id: &str| !frames::outdated(&shot(&store, id).frame, &store);
    assert!(fresh("sh0001") && fresh(&ren_shot));
    let mut cast = store.bible().expect("bible").cast;
    for v in cast.voices.iter_mut().filter(|v| v.character == "kai") {
        v.voice_id = "am_adam".into();
        v.speed = 1.1;
    }
    store.save_cast(&cast).expect("voice change");
    assert!(fresh("sh0001") && fresh(&ren_shot), "a voice never outdates a picture");
    run(&store, kai_turnaround, &[], pictures.clone()).await.expect("sheet again");
    let mut kai = character(&store, "kai");
    let new_sheet = kai.sheets.iter().find(|s| s.kind == SheetKind::Turnaround).and_then(|s| s.card.takes.last().cloned()).expect("a new sheet take");
    for s in kai.sheets.iter_mut().filter(|s| s.kind == SheetKind::Turnaround) {
        s.card.selected = Some(new_sheet.clone());
    }
    store.save_character(&kai).expect("pick the new sheet");
    assert!(!fresh("sh0001"), "Kai's frame uses the changed sheet: outdated");
    assert!(fresh(&ren_shot), "Ren's frame does not: still current");

    // ---- CUT: both shots with their stills, in order; the user's trim survives a new plan.
    let plan = animatic::plan_cut(&store, EP).expect("cut plan");
    assert_eq!(plan.shots.iter().map(|c| c.shot.as_str()).collect::<Vec<_>>(), vec!["sh0001", ren_shot.as_str()]);
    let still = store.take(&newest).map(|t| t.file);
    assert_eq!(plan.shots[0].still, still, "the selected frame is the cut's still");
    assert!(plan.shots.iter().all(|c| c.still.is_some() && c.seconds >= pacing::CUT_MIN_S));
    assert!(plan.shots[0].missing.iter().any(|m| m.starts_with("voice")), "no voice yet: the line is marked missing");
    assert_eq!(plan.shots[0].subs.len(), 1, "the shot's line is a subtitle");
    assert!((plan.shots[1].start_s - plan.shots[0].seconds).abs() < 1e-3);
    let mut trimmed: CutPlan = plan.clone();
    trimmed.shots[0].trim_out = 0.5;
    store::write_json(&store.path(&format!("episodes/{EP}/cut.json")), &trimmed).expect("user trim");
    let again = animatic::plan_cut(&store, EP).expect("cut plan again");
    assert_eq!(again.shots[0].trim_out, 0.5, "a new cut plan keeps the user's trim");
    assert!(again.shots[0].seconds < plan.shots[0].seconds);
    assert_ne!(again.timing_hash, plan.timing_hash, "the timing channel follows the trim");
    let as_json: Value = serde_json::to_value(&again).expect("serializes");
    assert!(as_json.get("shots").is_some());
}
