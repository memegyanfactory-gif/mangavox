#![cfg(feature = "sound")]
//! SOUND (design 8.1): the default cast gives every character a distinct voice of their gender
//! and reuses Studio's series cast; a line's emotion and pace land on the controls its engine
//! has; a voice take carries the hash of what made it, so a changed line or voice outdates only
//! that line. Voices are made by the fake engine (`voices::fake_py`): no Python ever starts.
//! This test binary points `MANGAVOX_HOME` at its own temp folder.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use mv_ai::CancellationToken;
use mv_anime::backend::NoImages;
use mv_anime::queue::{Job, JobCtx, JobKind, RunCtx};
use mv_anime::voices::{self, LineJob, NARRATOR};
use mv_anime::{
    CharVoice, Character, Emotion, EmotionKind, EpisodeRef, Line, LineKind, Pace, Role, Scene, SceneKind, Series, Status, Store, Story, TakeKind, VoiceCast,
};

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

fn store(dir: &Path) -> Store {
    Store { id: "p1".into(), dir: dir.join("anime").join("projects").join("p1") }
}

fn character(id: &str, name: &str, description: &str, role: Role) -> Character {
    Character { id: id.into(), name: name.into(), description: description.into(), role, ..Character::default() }
}

fn bible() -> Vec<Character> {
    vec![
        character("jin", "Jin", "He is a quiet hunter; the hero of the story.", Role::Hero),
        character("han", "Han", "His older brother, a man of few words.", Role::Ally),
        character("goro", "Goro", "A huge man who leads the raid. He shouts.", Role::Threat),
        character("mia", "Mia", "She is a healer. A young woman.", Role::Ally),
        character("rose", "Rose", "The guild's receptionist, a girl who loves her sister.", Role::Neutral),
    ]
}

fn studio_cast(dir: &Path) -> mv_recap::series::VoiceCast {
    mv_recap::series::VoiceCast::load_in(dir.to_path_buf())
}

#[test]
fn every_character_gets_a_distinct_voice_of_their_gender() {
    home();
    let tmp = tempfile::tempdir().expect("temp folder");
    let cast = voices::default_cast(&bible(), &studio_cast(tmp.path()), &HashSet::new());
    assert_eq!(cast.schema, mv_anime::SCHEMA);
    assert_eq!(cast.narrator.character, NARRATOR);
    assert_eq!(cast.narrator.voice_id, mv_studio::voices::default_voice());
    assert_eq!(cast.voices.len(), 5);
    let ids: Vec<&str> = cast.voices.iter().map(|v| v.voice_id.as_str()).collect();
    let unique: HashSet<&str> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len(), "no two characters share a voice: {ids:?}");
    assert!(!ids.contains(&cast.narrator.voice_id.as_str()), "nobody speaks with the narrator's voice");
    let voice = |c: &str| cast.voices.iter().find(|v| v.character == c).map(|v| v.voice_id.clone()).unwrap_or_default();
    for m in ["jin", "han", "goro"] {
        assert!(voice(m).starts_with("am_") || voice(m).starts_with("bm_"), "{m} is male: {}", voice(m));
    }
    for f in ["mia", "rose"] {
        assert!(voice(f).starts_with("af_") || voice(f).starts_with("bf_"), "{f} is female: {}", voice(f));
    }
    // The hero picks first: the first male voice of the catalog.
    assert_eq!(voice("jin"), voices::male_voices()[0]);
    // Every pick is an English Kokoro voice of the app's catalog.
    for id in ids {
        let info = mv_studio::tts::find_voice(id).expect("a catalog voice");
        assert!(info.lang.starts_with("en"));
        assert_eq!(mv_studio::tts::engine_hint(id), "kokoro");
    }
}

#[test]
fn only_installed_voices_are_cast_and_they_are_shared_only_when_none_is_left() {
    home();
    let tmp = tempfile::tempdir().expect("temp folder");
    let installed: HashSet<String> = ["am_adam", "am_eric", "af_bella"].iter().map(|s| s.to_string()).collect();
    let cast = voices::default_cast(&bible(), &studio_cast(tmp.path()), &installed);
    let males: Vec<&str> = cast.voices.iter().filter(|v| ["jin", "han", "goro"].contains(&v.character.as_str())).map(|v| v.voice_id.as_str()).collect();
    assert!(males.iter().all(|v| installed.contains(*v)), "{males:?}");
    // Three men, two male voices: both are used before one is used twice.
    assert!(males.contains(&"am_adam") && males.contains(&"am_eric"), "{males:?}");
    let females: Vec<&str> = cast.voices.iter().filter(|v| ["mia", "rose"].contains(&v.character.as_str())).map(|v| v.voice_id.as_str()).collect();
    assert_eq!(females, vec!["af_bella", "af_bella"], "one female voice installed");
}

#[test]
fn studio_cast_is_reused_with_its_clone_clip_and_locks_are_kept() {
    home();
    let tmp = tempfile::tempdir().expect("temp folder");
    let clip = tmp.path().join("mia_clip.wav");
    std::fs::write(&clip, b"RIFF....").expect("clip");
    let mut studio = studio_cast(tmp.path());
    studio.voices.push(mv_recap::series::CastVoice { name: "Sung Jin".into(), aliases: vec!["Jin".into()], base: "am_onyx".into(), ..Default::default() });
    studio.voices.push(mv_recap::series::CastVoice {
        name: "Mia".into(),
        gender: mv_recap::series::Gender::Female,
        base: "cb_narrator_f".into(),
        file: Some(clip.clone()),
        ..Default::default()
    });
    let cast = voices::default_cast(&bible(), &studio, &HashSet::new());
    let get = |c: &str| cast.voices.iter().find(|v| v.character == c).cloned().unwrap_or_default();
    assert_eq!(get("jin").voice_id, "am_onyx", "matched by alias");
    assert_eq!(get("jin").reference, None, "a Kokoro voice needs no clip");
    assert_eq!(get("mia").voice_id, "cb_narrator_f");
    assert_eq!(get("mia").reference.as_deref(), Some(clip.to_string_lossy().as_ref()), "Chatterbox clones Studio's clip");

    // Recast keeps a locked choice (and its auditions) and casts the others around it.
    let mut current = cast.clone();
    for v in current.voices.iter_mut() {
        if v.character == "han" {
            v.voice_id = "am_michael".into();
            v.locked = true;
            v.auditions = vec!["a".repeat(64)];
        }
    }
    let again = voices::recast(&current, &bible(), &studio_cast(tmp.path()), &HashSet::new());
    let han = again.voices.iter().find(|v| v.character == "han").cloned().unwrap_or_default();
    assert_eq!(han.voice_id, "am_michael");
    assert!(han.locked);
    assert_eq!(han.auditions.len(), 1);
    let others: Vec<&str> = again.voices.iter().filter(|v| v.character != "han").map(|v| v.voice_id.as_str()).collect();
    assert!(!others.contains(&"am_michael"), "nobody else takes the locked voice: {others:?}");
    // A locked voice of a character gone from the bible is not lost.
    current.voices.push(CharVoice { character: "ghost".into(), voice_id: "bm_george".into(), locked: true, ..CharVoice::default() });
    let kept = voices::recast(&current, &bible(), &studio_cast(tmp.path()), &HashSet::new());
    assert!(kept.voices.iter().any(|v| v.character == "ghost" && v.voice_id == "bm_george"));
}

#[test]
fn emotion_and_pace_land_on_each_engines_controls() {
    let e = |kind, strength| Emotion { kind, strength };
    // Kokoro: speed only. Pace × emotion nudge (full strength ±8% at most).
    let calm = voices::controls("am_adam", &e(EmotionKind::Neutral, 0), Pace::Normal, SceneKind::Dialogue, 1.);
    assert!((calm.speed - 1.).abs() < 1e-6);
    assert_eq!(calm.intensity, 4, "unset strength reads as mild: 1 × 3 + 1");
    let scared = voices::controls("am_adam", &e(EmotionKind::Fear, 3), Pace::Normal, SceneKind::Dialogue, 1.);
    assert!((scared.speed - 1.07).abs() < 1e-4, "{}", scared.speed);
    let mild = voices::controls("am_adam", &e(EmotionKind::Fear, 1), Pace::Normal, SceneKind::Dialogue, 1.);
    assert!(mild.speed > 1. && mild.speed < scared.speed, "a milder fear hurries less");
    let grief = voices::controls("af_bella", &e(EmotionKind::Grief, 3), Pace::Slow, SceneKind::Flashback, 1.);
    assert!((grief.speed - 0.9 * 0.92).abs() < 1e-4, "{}", grief.speed);
    let fast = voices::controls("af_bella", &e(EmotionKind::Neutral, 1), Pace::Fast, SceneKind::Action, 1.2);
    assert!((fast.speed - 1.32).abs() < 1e-4);
    assert_eq!(voices::mood_word(EmotionKind::Fear), "fear");
    assert_eq!(voices::mood_word(EmotionKind::Neutral), "");

    // Chatterbox acts it: kind, intensity = strength × 3 + 1, mood; speed is the pace times the
    // acting tempo (fear hurries a little).
    let cb = voices::controls("cb_narrator_m", &e(EmotionKind::Fear, 3), Pace::Fast, SceneKind::Dialogue, 1.);
    assert!(cb.speed > 1.1 && cb.speed < 1.25, "{}", cb.speed);
    assert_eq!(cb.intensity, 10);
    assert_eq!(cb.kind, "horror", "a strong fear in a dialogue scene is acted as dread");
    assert_eq!(cb.mood, "fear");
    let medium = voices::controls("cb_narrator_m", &e(EmotionKind::Anger, 2), Pace::Normal, SceneKind::Dialogue, 1.);
    assert_eq!((medium.intensity, medium.kind.as_str()), (7, "fight"));
    let fight = voices::controls("cb_narrator_m", &e(EmotionKind::Joy, 1), Pace::Normal, SceneKind::Action, 1.);
    assert_eq!((fight.intensity, fight.kind.as_str()), (4, "fight"));
    // The scene kind in Studio's words.
    let words: Vec<&str> = [SceneKind::Dialogue, SceneKind::Action, SceneKind::Dread, SceneKind::Narration, SceneKind::Flashback, SceneKind::Montage]
        .into_iter()
        .map(voices::studio_kind)
        .collect();
    assert_eq!(words, ["dialogue", "fight", "horror", "calm", "emotional", "other"]);
    assert_eq!(voices::line_kind(SceneKind::Dread, &e(EmotionKind::Grief, 3)), "horror", "a loud scene keeps its own kind");
}

#[test]
fn worker_answers_are_read_and_info_lines_kept() {
    let mut notes = Vec::new();
    assert_eq!(voices::worker_answer(r#"{"type":"info","message":"am_x: using af_heart"}"#, &mut notes), None);
    assert_eq!(notes, vec!["am_x: using af_heart".to_string()]);
    assert_eq!(voices::worker_answer(r#"{"ok":true,"path":"x.wav","engine":"kokoro"}"#, &mut notes), Some(Ok("kokoro".into())));
    assert_eq!(voices::worker_answer(r#"{"ok":false,"message":"no model"}"#, &mut notes), Some(Err("no model".into())));
    assert_eq!(voices::worker_answer("loading...", &mut notes), None);
}

// ------------------------------------------------------------------ voicing through the queue

fn line(id: &str, scene: &str, speaker: Option<&str>, text: &str) -> Line {
    Line { id: id.into(), scene: scene.into(), speaker: speaker.map(str::to_string), spoken: text.into(), kind: LineKind::Speech, ..Line::default() }
}

fn project(dir: &Path) -> Store {
    let s = store(dir);
    s.save_series(&Series { id: "p1".into(), episodes: vec![EpisodeRef { id: "ep01".into(), title: "One".into() }], ..Series::default() }).expect("series");
    let story = Story {
        episode: "ep01".into(),
        scenes: vec![Scene { id: "sc1".into(), kind: SceneKind::Dialogue, ..Scene::default() }],
        lines: vec![
            line("l1", "sc1", Some("jin"), "We should not be here."),
            line("l2", "sc1", Some("mia"), "Then why did you come?"),
            line("l3", "sc1", None, "The gate closed behind them."),
            Line { kind: LineKind::Sfx, ..line("l4", "sc1", None, "BOOM") },
            line("l5", "sc1", Some("jin"), "Because she is in there."),
        ],
        ..Story::default()
    };
    s.save_story(&story).expect("story");
    let cast = VoiceCast {
        narrator: CharVoice { character: NARRATOR.into(), voice_id: "af_heart".into(), speed: 1., ..CharVoice::default() },
        voices: vec![
            CharVoice { character: "jin".into(), voice_id: "am_adam".into(), speed: 1., ..CharVoice::default() },
            CharVoice { character: "mia".into(), voice_id: "af_bella".into(), speed: 1., ..CharVoice::default() },
        ],
        ..VoiceCast::default()
    };
    s.save_cast(&cast).expect("cast");
    s.save_character(&character("jin", "Jin", "He", Role::Hero)).expect("jin");
    s.save_character(&character("mia", "Mia", "She", Role::Ally)).expect("mia");
    s
}

fn run_ctx() -> RunCtx {
    RunCtx { ai: Box::new(|_| None), image: Arc::new(NoImages), python: Box::new(|| Ok(voices::fake_py())) }
}

async fn voice(s: &Store, line: &str) -> anyhow::Result<()> {
    let job = Job { kind: JobKind::Voice { episode: "ep01".into(), line: line.into() }, project: "p1".into(), ..Job::default() };
    let run = run_ctx();
    let cancel = CancellationToken::new();
    let progress = |_: f32, _: &str| {};
    let cx = JobCtx { job: &job, store: s, run: &run, progress: &progress, cancel: &cancel };
    voices::run_job(&cx, "ep01", line).await
}

fn scene_kind() -> SceneKind {
    SceneKind::Dialogue
}

#[tokio::test]
async fn voicing_saves_takes_with_their_request_and_only_changed_lines_go_stale() {
    home();
    let tmp = tempfile::tempdir().expect("temp folder");
    let s = project(tmp.path());
    voice(&s, "*").await.expect("voiced");
    let story = s.story("ep01").expect("story");
    let cast = s.bible().expect("bible").cast;
    for l in &story.lines {
        if l.kind == LineKind::Sfx {
            assert!(l.voice.selected.is_none(), "sound effects are never voiced");
            continue;
        }
        let id = l.voice.selected.clone().expect("a selected take");
        assert_eq!(l.voice.takes, vec![id.clone()]);
        assert_eq!(l.voice.status, Status::NeedsReview);
        let take = s.take(&id).expect("take");
        assert_eq!(take.kind, TakeKind::Audio);
        assert!(take.seconds > 0.5, "{}", take.seconds);
        assert!(s.path(&take.file).is_file());
        assert!(!voices::needs_voice(&s, &cast, scene_kind(), l), "{} is current", l.id);
        let req = voices::request(&s, &cast, scene_kind(), l).expect("request");
        assert_eq!(take.request_hash, req.hash);
        let expect_voice = match l.speaker.as_deref() {
            Some("jin") => "am_adam",
            Some("mia") => "af_bella",
            _ => "af_heart",
        };
        assert_eq!(req.voice, expect_voice);
    }
    assert!(!s.work_dir().join("voice").join("l1.wav").exists(), "work files are cleaned up");

    // Nothing changed: voicing all again makes no new take.
    voice(&s, "*").await.expect("nothing to do");
    assert_eq!(s.story("ep01").expect("story").lines[0].voice.takes.len(), 1);

    // A changed line is outdated alone; a note on another line changes nothing.
    let mut story = s.story("ep01").expect("story");
    story.lines[1].spoken = "Then why did you come here?".into();
    story.lines[0].locked.insert("/spoken".into());
    story.lines[0].edited = true;
    s.save_story(&story).expect("saved");
    let stale = |s: &Store| -> Vec<String> {
        let story = s.story("ep01").expect("story");
        let cast = s.bible().expect("bible").cast;
        story.lines.iter().filter(|l| voices::is_outdated(s, &cast, scene_kind(), l)).map(|l| l.id.clone()).collect()
    };
    assert_eq!(stale(&s), vec!["l2".to_string()]);
    voice(&s, "l2").await.expect("re-voiced");
    assert!(stale(&s).is_empty());
    assert_eq!(s.story("ep01").expect("story").lines[1].voice.takes.len(), 2, "the old take stays in the history");

    // A changed voice outdates exactly that character's lines.
    let mut cast = s.bible().expect("bible").cast;
    cast.voices[0].voice_id = "am_eric".into();
    s.save_cast(&cast).expect("cast");
    assert_eq!(stale(&s), vec!["l1".to_string(), "l5".to_string()]);
    // So do a per-line voice override and a changed emotion.
    let mut story = s.story("ep01").expect("story");
    story.lines[2].voice.voice_override = Some("bm_george".into());
    story.lines[1].emotion = Emotion { kind: EmotionKind::Anger, strength: 3 };
    s.save_story(&story).expect("saved");
    assert_eq!(stale(&s), vec!["l1", "l2", "l3", "l5"]);
}

#[tokio::test]
async fn a_single_line_job_explains_lines_it_cannot_voice() {
    home();
    let tmp = tempfile::tempdir().expect("temp folder");
    let s = project(tmp.path());
    let sfx = voice(&s, "l4").await.expect_err("a sound effect");
    assert!(sfx.to_string().contains("sound effect"), "{sfx}");
    let gone = voice(&s, "l99").await.expect_err("no such line");
    assert!(gone.to_string().contains("no longer"), "{gone}");
}

#[tokio::test]
async fn an_audition_is_kept_as_a_take_on_the_cast() {
    home();
    let tmp = tempfile::tempdir().expect("temp folder");
    let s = project(tmp.path());
    let job = Job { kind: JobKind::Audition { character: "mia".into(), voice: "af_nova".into() }, ..Job::default() };
    let run = run_ctx();
    let cancel = CancellationToken::new();
    let progress = |_: f32, _: &str| {};
    let cx = JobCtx { job: &job, store: &s, run: &run, progress: &progress, cancel: &cancel };
    voices::run_audition(&cx, "mia", "af_nova").await.expect("auditioned");
    let cast = s.bible().expect("bible").cast;
    let mia = cast.voices.iter().find(|v| v.character == "mia").cloned().unwrap_or_default();
    assert_eq!(mia.voice_id, "af_bella", "an audition never changes the cast voice");
    assert_eq!(mia.auditions.len(), 1);
    let take = s.take(&mia.auditions[0]).expect("take");
    assert_eq!(take.note, "af_nova");
    assert_eq!(take.target, mv_anime::Target::Audition { character: "mia".into() });
    // Her first line is what she says ("Then why did you come?" is 5 words: 2.0 s at 2.5 words/s).
    assert!((take.seconds - 2.0).abs() < 0.05, "{}", take.seconds);
    voices::run_audition(&cx, NARRATOR, "bm_george").await.expect("narrator audition");
    assert_eq!(s.bible().expect("bible").cast.narrator.auditions.len(), 1);
}

#[tokio::test]
async fn the_fake_engine_writes_one_file_per_job() {
    let tmp = tempfile::tempdir().expect("temp folder");
    let jobs: Vec<LineJob> = (0..3)
        .map(|i| LineJob { line: format!("l{i}"), text: "one two three four five".into(), voice: "am_adam".into(), speed: 1., out: tmp.path().join(format!("v{i}.wav")), ..LineJob::default() })
        .collect();
    let done = std::sync::Mutex::new(Vec::<(String, PathBuf)>::new());
    voices::synth_lines(&voices::fake_py(), &jobs, |line, r| done.lock().expect("lock").push((line.to_string(), r.expect("voiced")))).await.expect("ran");
    let done = done.into_inner().expect("lock");
    assert_eq!(done.len(), 3);
    for (_, p) in &done {
        assert!((voices::wav_seconds(p).expect("wav") - 2.0).abs() < 0.01);
    }
}
