use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use futures::future::BoxFuture;
use mv_ai::{AiProviderId, AiStepRequest, CancellationToken};

use crate::llm::Llm;
use crate::read::*;
use crate::story::write_story_with;
use crate::*;

struct Fake {
    answers: Mutex<VecDeque<String>>,
    seen: Mutex<Vec<AiStepRequest>>,
}

impl Fake {
    fn new(a: &[&str]) -> Self {
        Fake { answers: Mutex::new(a.iter().map(|s| s.to_string()).collect()), seen: Mutex::new(vec![]) }
    }
}

impl Llm for Fake {
    fn step<'a>(&'a self, req: AiStepRequest, _c: CancellationToken) -> BoxFuture<'a, Result<String>> {
        self.seen.lock().unwrap().push(req);
        let a = self.answers.lock().unwrap().pop_front();
        Box::pin(async move { a.ok_or_else(|| anyhow!("no canned answer")) })
    }
}

fn png(dir: &Path, name: &str, w: u32, h: u32) {
    image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x % 255) as u8, (y % 255) as u8, 90])).save(dir.join(name)).unwrap();
}

fn chapter(root: &Path, pages: usize) -> ChapterRef {
    let pages_dir = root.join("ch").join("pages");
    std::fs::create_dir_all(&pages_dir).unwrap();
    for i in 1..=pages {
        png(&pages_dir, &format!("page{i}.png"), 400, 600);
    }
    ChapterRef {
        chapter_id: "c1".into(),
        manga_id: "m1".into(),
        manga_title: "Test Manga".into(),
        label: "Chapter 1".into(),
        pages_dir,
        output_dir: root.join("ch").join("pipeline"),
    }
}

fn model() -> ModelChoice {
    ModelChoice { provider: AiProviderId::ClaudeCode, model: "x".into() }
}

fn silent() -> ProgressFn {
    Arc::new(|_| {})
}

const PAGE1: &str = r#"Sure! ```json
{"summary":"Ruko meets a stranger.","scene":{"kind":"Fight","mood":"tense","intensity":"7","tags":["duel"]},"memory":"Ruko meets the swordsman",
 "panels":[{"bbox":[0.5,0.0,1.0,0.5],"description":"Two men face off.",
   "characters":[{"name":"Silver-haired swordsman","face":[0.6,0.1,0.8,0.25],"description":"tall, silver hair","traits":["calm"]},
                 {"name":"Ruko","was":null,"face":[0.1,0.1,0.2,0.2]}],
   "lines":[{"speaker":"Ruko","kind":"speech","text":"Who {are} you?"},{"speaker":null,"kind":"sfx","text":"CLANG"}]}]}
``` hope that helps"#;

const PAGE2: &str = r#"{"summary":"He is named.","scene":{"kind":"dialogue"},"panels":[{"bbox":[0,0,1,0.6],"description":"d",
  "characters":[{"name":"Kaito","was":"Silver-haired swordsman","face":[0.6,0.1,0.8,0.25]}],
  "lines":[{"speaker":"Silver-haired swordsman","kind":"speech","text":"I am Kaito."}]}]}"#;

/// The cast check's answer when every name was already right.
const CAST_OK: &str = r#"{"fixes":[],"cast":[]}"#;

#[test]
fn json_extraction() {
    let v = extract_json("```json\n{\"a\": 1}\n```").unwrap();
    assert_eq!(v["a"], 1);
    let v = extract_json("Here {not json} you go: {\"a\": {\"b\": \"x } y\"}} thanks }").unwrap();
    assert_eq!(v["a"]["b"], "x } y");
    assert!(extract_json("no json here").is_none());
    assert!(extract_json("{\"a\": ").is_none());
}

#[test]
fn every_call_asks_for_low_effort() {
    let ask = |provider, model: &str| crate::llm::request(&ModelChoice { provider, model: model.into() }, vec![], vec![], None);
    let r = ask(AiProviderId::ClaudeCode, "claude-opus-5-5");
    assert_eq!((r.effort, r.model.as_str()), (Some(mv_ai::AiEffort::Low), "claude-opus-5-5"));
    assert_eq!(ask(AiProviderId::Antigravity, "gemini-3.8-flash-high").model, "gemini-3.8-flash-low");
    assert_eq!(ask(AiProviderId::Antigravity, "gemini-3.8-flash-medium").model, "gemini-3.8-flash-low");
    assert_eq!(ask(AiProviderId::Antigravity, "gpt-oss-120b-medium").model, "gpt-oss-120b-medium");
}

#[test]
fn page_parsing_is_lenient() {
    let v = extract_json("{\"summary\":\"s\",\"panels\":[{\"lines\":[{\"text\":\"hi\"},{\"text\":\"\"}],\"characters\":[{\"name\":\"\"},{\"name\":\"A\",\"face\":[10,10,20,30]}]}]}").unwrap();
    let p = parse_page(&v, 3, "p.png");
    assert_eq!(p.page.index, 3);
    assert_eq!(p.page.scene.kind, "other");
    assert_eq!(p.page.panels[0].lines.len(), 1);
    assert_eq!(p.page.panels[0].lines[0].kind, "speech");
    assert_eq!(p.page.panels[0].characters.len(), 1);
    let f = p.page.panels[0].characters[0].face.unwrap().0;
    assert!((f[0] - 0.1).abs() < 1e-5 && (f[3] - 0.3).abs() < 1e-5);
    let empty = parse_page(&serde_json::json!({}), 0, "x");
    assert!(empty.page.panels.is_empty());
}

#[test]
fn brain_matching_rename_merge_lock() {
    let dir = tempfile::tempdir().unwrap();
    let mut b = Brain::load_in(dir.path().to_path_buf(), "m", "T");
    let (a, new) = b.observe("Silver-haired swordsman", None, "tall", &["calm".into()]);
    assert!(new);
    let (a2, new2) = b.observe("silver-haired SWORDSMAN", None, "", &[]);
    assert_eq!((a2.as_str(), new2), (a.as_str(), false));
    // placeholder gets a real name
    let (a3, new3) = b.observe("Kaito", Some("Silver-haired swordsman"), "", &[]);
    assert_eq!((a3.as_str(), new3), (a.as_str(), false));
    assert_eq!(b.find("Kaito").unwrap().id, a);
    assert_eq!(b.find("silver-haired swordsman").unwrap().id, a);
    // user lock: AI may not rename
    b.rename(&a, "Kai");
    assert!(b.find("Kai").unwrap().user_locked);
    let (_, new4) = b.observe("Kaito", None, "", &[]);
    assert!(!new4);
    let (b1, _) = b.observe("Ruko", None, "", &[]);
    let (_, _) = b.observe("Rukio", None, "", &[]);
    let (_, _) = b.observe("Zed", Some("Ruko"), "", &[]);
    assert_eq!(b.find("Zed").unwrap().id, b1); // Ruko not locked -> renamed
    let rid = b.find("Rukio").unwrap().id.clone();
    b.merge(&b1, &rid);
    assert!(b.find("Rukio").map(|c| c.id.clone()) == Some(b1.clone()));
    assert_eq!(b.characters.len(), 2);
    b.remove(&b1);
    assert_eq!(b.characters.len(), 1);
    b.save().unwrap();
    let again = Brain::load_in(dir.path().to_path_buf(), "m", "T");
    assert_eq!(again.characters, b.characters);
}

#[test]
fn notes_are_capped() {
    let mut b = Brain::default();
    for i in 0..400 {
        b.add_note(&format!("event number {i} happened in the story"));
    }
    assert!(b.notes.chars().count() <= 4000);
    assert!(b.notes.contains("event number 399"));
    assert!(!b.notes.contains("event number 0 "));
}

#[test]
fn face_crop_writes_small_jpeg() {
    let dir = tempfile::tempdir().unwrap();
    png(dir.path(), "p.png", 800, 1200);
    let img = image::open(dir.path().join("p.png")).unwrap();
    let out = dir.path().join("faces").join("a_0.jpg");
    crop_face(&img, Bbox([0.4, 0.2, 0.6, 0.3]), &out).unwrap();
    let (w, h) = image::image_dimensions(&out).unwrap();
    assert!(w <= 192 && h <= 192 && w > 20);
    let pout = dir.path().join("panel.jpg");
    crop_panel(&img, Bbox([0.0, 0.0, 0.5, 0.5]), &pout).unwrap();
    assert_eq!(image::image_dimensions(&pout).unwrap().0, 404);
}

#[test]
fn transcript_merge_keeps_edited() {
    let read = ChapterRead {
        pages: vec![
            PageRead {
                index: 0,
                panels: vec![Panel {
                    lines: vec![
                        Line { speaker: Some("Ruko".into()), text: "Hi".into(), kind: "speech".into() },
                        Line { speaker: None, text: "BOOM".into(), kind: "sfx".into() },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            },
            PageRead { index: 1, ..Default::default() },
        ],
        ..Default::default()
    };
    let fresh = transcript_from_read(&read);
    assert_eq!(fresh.pages[0].text, "Ruko: Hi\n[SFX] BOOM");
    let old = Transcript {
        pages: vec![TranscriptPage { index: 0, text: "mine".into(), edited: true }, TranscriptPage { index: 1, text: "old".into(), edited: false }],
    };
    let m = merge_transcript(Some(old), fresh);
    assert_eq!(m.pages[0].text, "mine");
    assert!(m.pages[0].edited);
    assert_eq!(m.pages[1].text, "");
}

#[test]
fn natural_page_order() {
    let dir = tempfile::tempdir().unwrap();
    for n in ["page10.jpg", "page2.jpg", "page1.png", "notes.txt", "Page3.WEBP"] {
        std::fs::write(dir.path().join(n), b"x").unwrap();
    }
    let names: Vec<String> = list_pages(dir.path()).iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();
    assert_eq!(names, ["page1.png", "page2.jpg", "Page3.WEBP", "page10.jpg"]);
}

#[test]
fn export_formats() {
    let dir = tempfile::tempdir().unwrap();
    let ch = chapter(dir.path(), 3);
    std::fs::create_dir_all(panels_for(&ch)).unwrap();
    std::fs::write(panel_path(&ch, 1, 0), b"x").unwrap();
    let story = Story {
        segments: vec![
            StorySegment { page: 1, panel: Some(0), text: "One.".into(), delivery: None, sfx: None, hook: false, panels: vec![], tone: None },
            StorySegment { page: 1, panel: Some(5), text: "Two.".into(), delivery: Some(Delivery { kind: "fight".into(), intensity: 14 }), sfx: SfxCue::parse("boom", Some(3.0)), hook: false, panels: vec![], tone: None },
        ],
        ..Default::default()
    };
    export_for_pipeline(&ch, &story).unwrap();
    let script: serde_json::Value = serde_json::from_slice(&std::fs::read(ch.output_dir.join("script_edited.json")).unwrap()).unwrap();
    assert_eq!(script.as_array().unwrap().len(), 3);
    assert_eq!(script[0]["text"], "");
    assert_eq!(script[1]["text"], "One. Two.");
    let beats: serde_json::Value = serde_json::from_slice(&std::fs::read(ch.output_dir.join("narration.studio.json")).unwrap()).unwrap();
    let b = beats["beats"].as_array().unwrap();
    assert_eq!(b.len(), 4); // page0 filler, two segments, page2 filler
    assert_eq!(b[0]["page"], 0);
    assert!(b[1]["panel_image"].as_str().unwrap().ends_with("p0001_00.jpg"));
    assert!(b[2]["panel_image"].as_str().unwrap().ends_with("page2.png"));
    // A passage without a delivery follows its scene; one with a delivery carries it (capped at 10).
    assert!(b[1].get("kind").is_none());
    assert_eq!((b[2]["kind"].as_str(), b[2]["intensity"].as_u64()), (Some("fight"), Some(10)));
    // A sound-effect cue rides on its beat; beats without one carry nothing.
    assert_eq!((b[2]["sfx"].as_str(), b[2]["sfx_strength"].as_u64()), (Some("boom"), Some(3)));
    assert!(b[1].get("sfx").is_none() && b[1].get("sfx_strength").is_none());
    // Old story files without the field still load.
    let old: StorySegment = serde_json::from_str(r#"{"page":2,"panel":null,"text":"x"}"#).unwrap();
    assert_eq!(old.delivery, None);
    assert_eq!(old.sfx, None);
}

#[test]
fn story_sfx_cues_are_validated() {
    // The vocabulary only: spelling variants map onto it, anything else is dropped.
    assert_eq!(SfxCue::parse("impact", Some(2.0)), Some(SfxCue { name: "impact".into(), strength: 2 }));
    assert_eq!(SfxCue::parse(" Sword Clang ", Some(7.0)).map(|c| (c.name, c.strength)), Some(("sword_clang".into(), 3)));
    assert_eq!(SfxCue::parse("door-slam", None).map(|c| c.strength), Some(2), "missing strength: 2");
    assert_eq!(SfxCue::parse("whoosh", Some(0.0)).map(|c| c.strength), Some(1));
    assert_eq!(SfxCue::parse("explosion", Some(2.0)), None);
    assert_eq!(SfxCue::parse("", Some(2.0)), None);

    let read = ChapterRead { pages: (0..3).map(|i| PageRead { index: i, ..Default::default() }).collect(), ..Default::default() };
    let v: serde_json::Value = serde_json::from_str(
        r#"{"segments": [
            {"page": 1, "text": "Calm."},
            {"page": 2, "text": "He strikes.", "sfx": "Impact", "sfx_strength": "3"},
            {"page": 2, "text": "Laser.", "sfx": "laser", "sfx_strength": 2},
            {"page": 3, "text": "Boom.", "sfx": "boom"}
        ]}"#,
    )
    .unwrap();
    let segs = crate::story::parse_segments(&v, &[0, 1, 2], &read);
    let cues: Vec<Option<(String, u8)>> = segs.iter().map(|s| s.sfx.clone().map(|c| (c.name, c.strength))).collect();
    assert_eq!(cues, vec![None, Some(("impact".into(), 3)), None, Some(("boom".into(), 2))]);
    // Saved stories keep the cue; segments without one write no key (old readers unaffected).
    let json = serde_json::to_string(&segs).unwrap();
    assert_eq!(json.matches("\"sfx\"").count(), 2);
    let back: Vec<StorySegment> = serde_json::from_str(&json).unwrap();
    assert_eq!(back, segs);
}

fn panels_for(ch: &ChapterRef) -> std::path::PathBuf {
    chapter_dir(ch).join("panels")
}

#[tokio::test]
async fn read_chapter_end_to_end_and_resume() {
    let dir = tempfile::tempdir().unwrap();
    let ch = chapter(dir.path(), 2);
    let brain = Mutex::new(Brain::load_in(dir.path().join("brain"), "m1", "Test Manga"));
    let llm = Fake::new(&[PAGE1, PAGE2]);
    let events = Arc::new(Mutex::new(vec![]));
    let ev = events.clone();
    let progress: ProgressFn = Arc::new(move |p| ev.lock().unwrap().push(format!("{p:?}")));
    let read = read_chapter_with(&llm, &model(), &ch, &brain, &ReadOptions::default(), &progress, &CancellationToken::new()).await.unwrap();
    assert_eq!(read.pages.len(), 2);
    assert_eq!(read.pages[0].scene.kind, "fight");
    assert_eq!(read.pages[0].scene.intensity, 7);
    // page 2 attaches page image + face thumbnails of known characters
    {
        let seen = llm.seen.lock().unwrap();
        assert_eq!(seen[0].images.len(), 1);
        assert!(seen[1].images.len() >= 3, "the page, the page before it and faces");
        let prompt = &seen[1].messages.last().unwrap().content;
        assert!(prompt.contains("Image 2 = the page BEFORE it") && prompt.contains("Image 3 = reference face of"), "{prompt}");
        assert!(!seen[0].messages.last().unwrap().content.contains("the page BEFORE"), "the first page has none");
        assert!(prompt.contains("Silver-haired swordsman"));
        assert_eq!(seen[1].workspace.as_ref().unwrap(), &chapter_dir(&ch).join("work").join("p0001"));
    }
    // placeholder renamed; speaker canonicalised
    {
        let b = brain.lock().unwrap();
        assert!(b.find("Kaito").is_some() && b.find("Silver-haired swordsman").is_some());
        assert_eq!(read.pages[1].panels[0].lines[0].speaker.as_deref(), Some("Kaito"));
        let k = b.find("Kaito").unwrap();
        assert_eq!(k.appearances, 2);
        assert_eq!(k.first_seen, Some(("c1".into(), 0)));
        assert_eq!(k.faces.len(), 2);
        assert!(k.faces.iter().all(|f| f.is_file()));
        assert!(b.notes.contains("Ruko meets the swordsman"));
        assert!(dir.path().join("brain").join("brain.json").is_file());
    }
    assert!(panel_path(&ch, 0, 0).is_file());
    assert!(panel_path(&ch, 1, 0).is_file());
    assert_eq!(chapter_status(&ch).read_pages, 2);
    assert!(events.lock().unwrap().iter().any(|e| e.contains("Character") && e.contains("Ruko")));

    // resume: everything done -> no model call
    let none = Fake::new(&[]);
    read_chapter_with(&none, &model(), &ch, &brain, &ReadOptions::default(), &progress, &CancellationToken::new()).await.unwrap();
    assert!(none.seen.lock().unwrap().is_empty());
    // single page re-read with garbage twice -> raw text stored, chapter not aborted
    let bad = Fake::new(&["nope", "still nope"]);
    let opts = ReadOptions { force: false, pages: Some(vec![1]) };
    let r = read_chapter_with(&bad, &model(), &ch, &brain, &opts, &progress, &CancellationToken::new()).await.unwrap();
    assert_eq!(r.pages[1].summary, "still nope");
    assert!(r.pages[1].panels.is_empty());
    assert_eq!(r.pages[0].scene.kind, "fight");
}

#[tokio::test]
async fn cancel_keeps_saved_pages() {
    let dir = tempfile::tempdir().unwrap();
    let ch = chapter(dir.path(), 2);
    let brain = Mutex::new(Brain::load_in(dir.path().join("brain"), "m1", "T"));
    let cancel = CancellationToken::new();
    cancel.cancel();
    let llm = Fake::new(&[PAGE1]);
    let e = read_chapter_with(&llm, &model(), &ch, &brain, &ReadOptions::default(), &silent(), &cancel).await.unwrap_err();
    assert_eq!(e.to_string(), "cancelled");
}

#[tokio::test]
async fn story_chunks_and_saves() {
    let dir = tempfile::tempdir().unwrap();
    let ch = chapter(dir.path(), 2);
    let brain = Mutex::new(Brain::load_in(dir.path().join("brain"), "m1", "Test Manga"));
    let llm = Fake::new(&[PAGE1, PAGE2]);
    read_chapter_with(&llm, &model(), &ch, &brain, &ReadOptions::default(), &silent(), &CancellationToken::new()).await.unwrap();
    save_transcript(&ch, &Transcript { pages: vec![TranscriptPage { index: 0, text: "Ruko: EDITED LINE".into(), edited: true }] }).unwrap();

    let story = Fake::new(&[CAST_OK, r#"{"title":"Duel","summary_so_far":"s","segments":[{"page":1,"panel":1,"text":"Ruko draws."},{"page":2,"panel":null,"text":"Kaito speaks."},{"page":9,"text":"stray"}]}"#]);
    let out = write_story_with(&story, &model(), std::slice::from_ref(&ch), &[], &["Hard".to_string()], &brain, &silent(), &CancellationToken::new()).await.unwrap();
    assert_eq!(out.len(), 1);
    let s = &out[0].1;
    assert_eq!(s.title, "Duel");
    assert_eq!(s.segments, vec![
        StorySegment { page: 0, panel: Some(0), text: "Ruko draws.".into(), delivery: None, sfx: None, hook: false, panels: vec![], tone: None },
        StorySegment { page: 1, panel: None, text: "Kaito speaks.".into(), delivery: None, sfx: None, hook: false, panels: vec![], tone: None },
    ]);
    let req = story.seen.lock().unwrap();
    assert_eq!(req.len(), 2, "the cast check, then the story");
    assert_eq!(req[1].effort, Some(mv_ai::AiEffort::Medium), "the story plans its scenes");
    let sys = &req[1].messages[0].content;
    assert!(sys.contains("Hard: blunt"));
    let user = &req[1].messages[1].content;
    assert!(user.contains("EDITED LINE") && !user.contains("Who {are} you?"));
    assert!(user.contains("Kaito"));
    assert_eq!(load_story(&ch).unwrap().title, "Duel");
}

#[tokio::test]
async fn hook_is_written_from_candidates_with_a_json_retry() {
    use crate::hook::*;
    let chapters = vec![
        ("Chapter 1".to_string(), vec!["Ruko meets a stranger.".to_string(), "The stranger draws a blade!".to_string()]),
        ("Chapter 2".to_string(), vec!["Kaito betrays the guild.".to_string()]),
    ];
    let cands = pick_candidates(&chapters, MAX_CANDIDATES);
    let fake = Fake::new(&["not json", r#"{"lines":[{"id":1,"text":"A blade is drawn."},{"id":2,"text":"A friend turns traitor."}],"outro":"Let's see how it begins."}"#]);
    let h = crate::hook::write_hook_with(&fake, &model(), "Test Manga", "STORY SO FAR: Ruko trains.\n", &cands, 20, &CancellationToken::new()).await.unwrap();
    assert_eq!(h.lines, vec![(1, "A blade is drawn.".to_string()), (2, "A friend turns traitor.".to_string())]);
    let seen = fake.seen.lock().unwrap();
    let user = &seen[0].messages[1].content;
    assert!(user.contains("[2] (Chapter 2) Kaito betrays the guild."), "{user}");
    assert!(user.contains("about 52 words"));
    assert!(user.contains("STORY SO FAR: Ruko trains."), "the story is given as context");
    assert_eq!(seen.len(), 2, "one retry after the non-JSON answer");
}

#[test]
fn saved_provider_messages_are_found() {
    use crate::store::provider_message_in;
    let note = "You've hit your session limit · resets 8am (Asia/Kolkata)";
    let read = |s: &str| ChapterRead { pages: vec![PageRead { index: 0, summary: s.into(), ..Default::default() }], ..Default::default() };
    let story = |s: &str| Story { segments: vec![StorySegment { page: 0, panel: None, text: s.into(), delivery: None, sfx: None, hook: false, panels: vec![], tone: None }], ..Default::default() };
    assert_eq!(provider_message_in(Some(&read(note)), Some(&story(note))), Some(Step::Read));
    assert_eq!(provider_message_in(Some(&read("Ruko draws.")), Some(&story(note))), Some(Step::Story));
    assert_eq!(provider_message_in(Some(&read("Ruko draws.")), Some(&story("He has no limit to his rage."))), None);
}

#[test]
fn story_parts_fit_the_budget_and_keep_order() {
    use crate::story::plan_parts;
    let pages: Vec<(usize, usize)> = (0..10).map(|i| (i, 100)).collect();
    assert_eq!(plan_parts(&pages, 5000), vec![(0..10).collect::<Vec<_>>()], "a chapter that fits is sent whole");
    let parts = plan_parts(&pages, 400);
    assert_eq!(parts.len(), 3);
    assert_eq!(parts.concat(), (0..10).collect::<Vec<_>>());
    assert!(parts.iter().all(|p| p.len() >= 3), "parts are about equal: {parts:?}");
}

#[test]
fn recap_before_takes_the_nearest_earlier_chapter() {
    let dir = tempfile::tempdir().unwrap();
    let mut b = Brain::load_in(dir.path().to_path_buf(), "m1", "T");
    b.set_recap("Chapter 2", "two");
    b.set_recap("Chapter 10", "ten");
    b.set_recap("Chapter 1", "one");
    b.set_recap("Chapter 2", "two again");
    assert_eq!(b.recap_before("Chapter 3"), Some("two again"));
    assert_eq!(b.recap_before("Vol. 1 Ch. 11.5"), Some("ten"));
    assert_eq!(b.recap_before("Chapter 1"), None);
    assert_eq!(b.recap_before("Prologue"), None);
}

#[test]
fn export_skips_silent_pages_and_blank_panels() {
    let dir = tempfile::tempdir().unwrap();
    let ch = chapter(dir.path(), 3);
    std::fs::create_dir_all(panels_for(&ch)).unwrap();
    image::RgbImage::from_pixel(64, 64, image::Rgb([3, 3, 3])).save(panel_path(&ch, 1, 0)).unwrap();
    png(&panels_for(&ch), "p0001_01.jpg", 256, 256);
    let panel = |chars: bool| Panel { bbox: None, description: String::new(), characters: if chars { vec![CharacterSeen { name: "Ruko".into(), ..Default::default() }] } else { vec![] }, lines: vec![], ..Default::default() };
    let read = ChapterRead { pages: vec![PageRead { index: 1, panels: vec![panel(false), panel(true)], ..Default::default() }], ..Default::default() };
    save_read(&ch, &read).unwrap();
    let story = Story { segments: vec![StorySegment { page: 1, panel: Some(0), text: "Ruko wakes.".into(), delivery: None, sfx: None, hook: false, panels: vec![], tone: None }], ..Default::default() };
    export_for_pipeline(&ch, &story).unwrap();
    let beats: serde_json::Value = serde_json::from_slice(&std::fs::read(ch.output_dir.join("narration.studio.json")).unwrap()).unwrap();
    let b = beats["beats"].as_array().unwrap();
    assert_eq!(b.len(), 3);
    assert_eq!((b[0]["skip"].as_bool(), b[2]["skip"].as_bool()), (Some(true), Some(true)));
    assert!(b[1].get("skip").is_none());
    assert!(b[1]["panel_image"].as_str().unwrap().ends_with("p0001_01.jpg"), "the black panel is passed over: {}", b[1]);
}

#[tokio::test]
async fn story_sees_the_whole_chapter_and_remembers_it() {
    let dir = tempfile::tempdir().unwrap();
    let ch = chapter(dir.path(), 2);
    let brain = Mutex::new(Brain::load_in(dir.path().join("brain"), "m1", "Test Manga"));
    read_chapter_with(&Fake::new(&[PAGE1, PAGE2]), &model(), &ch, &brain, &ReadOptions::default(), &silent(), &CancellationToken::new()).await.unwrap();
    let llm = Fake::new(&[CAST_OK, r#"{"title":"T","summary_so_far":"Ruko meets Kaito.","segments":[{"page":1,"panel":1,"text":"Ruko meets a stranger."},{"page":2,"panel":1,"text":"Kaito speaks."}]}"#]);
    write_story_with(&llm, &model(), std::slice::from_ref(&ch), &[], &[], &brain, &silent(), &CancellationToken::new()).await.unwrap();
    let seen = llm.seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "the cast check, then one call for the whole chapter");
    let (sys, user) = (&seen[1].messages[0].content, &seen[1].messages[1].content);
    assert!(user.contains("the whole chapter"));
    // The first chapter just begins the story: no in-medias-res hook, no rewind, no outro.
    assert!(user.contains("these pages begin the story"), "{user}");
    assert!(!user.contains("hook") && !user.contains("IN MEDIAS RES") && !user.contains("go back"), "{user}");
    assert!(user.contains("No wrap-up") && !user.contains("next chapter"), "{user}");
    assert!(!user.contains("CALL TO ACTION"), "minute 0 is no reminder mark");
    assert!(!sys.contains("\"hook\""));
    assert!(sys.contains("ONE CONTINUOUS STORY") && sys.contains("\"previously\"") && sys.contains("\"end of chapter\"") && sys.contains("sign-offs"), "{sys}");
    assert!(sys.contains("One or two per chapter at most") && sys.contains("never in serious, sad or tense scenes"), "{sys}");
    let b = brain.lock().unwrap();
    assert_eq!(b.recaps[0].summary, "Ruko meets Kaito.");
    assert_eq!(b.recap_before("Chapter 2"), Some("Ruko meets Kaito."));
}

#[tokio::test]
async fn story_ignores_hooks_and_finishes_what_it_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let ch = chapter(dir.path(), 2);
    let brain = Mutex::new(Brain::load_in(dir.path().join("brain"), "m1", "Test Manga"));
    read_chapter_with(&Fake::new(&[PAGE1, PAGE2]), &model(), &ch, &brain, &ReadOptions::default(), &silent(), &CancellationToken::new()).await.unwrap();
    // The first answer stops after page 1: the rest is asked for. A "hook" key is ignored, and a
    // segment outside the pages asked for is dropped.
    let llm = Fake::new(&[
        CAST_OK,
        r#"{"title":"T","summary_so_far":"s","segments":[{"page":1,"panel":1,"hook":true,"text":"Ruko meets a stranger."}]}"#,
        r#"{"summary_so_far":"s2","segments":[{"page":1,"panel":1,"hook":true,"text":"Not on these pages."},{"page":2,"panel":1,"text":"The stranger is Kaito."}]}"#,
    ]);
    let out = write_story_with(&llm, &model(), std::slice::from_ref(&ch), &[], &[], &brain, &silent(), &CancellationToken::new()).await.unwrap();
    let segs: Vec<(usize, bool, &str)> = out[0].1.segments.iter().map(|s| (s.page, s.hook, s.text.as_str())).collect();
    assert_eq!(segs, vec![(0, false, "Ruko meets a stranger."), (1, false, "The stranger is Kaito.")]);
    {
        let seen = llm.seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert!(seen[1].messages[1].content.contains("LENGTH TARGET: about 250 words (2 lines"), "{}", seen[1].messages[1].content);
        let again = &seen[2].messages[1].content;
        assert!(again.contains("pages 2-2") && again.contains("LAST LINES ALREADY SPOKEN") && again.contains("Ruko meets a stranger."), "{again}");
    }
}

#[tokio::test]
async fn old_stories_export_without_their_hook_segments() {
    let dir = tempfile::tempdir().unwrap();
    let ch = chapter(dir.path(), 2);
    let brain = Mutex::new(Brain::load_in(dir.path().join("brain"), "m1", "Test Manga"));
    read_chapter_with(&Fake::new(&[PAGE1, PAGE2]), &model(), &ch, &brain, &ReadOptions::default(), &silent(), &CancellationToken::new()).await.unwrap();
    let seg = |page: usize, hook: bool, text: &str| StorySegment { page, panel: Some(0), text: text.into(), delivery: None, sfx: None, hook, panels: vec![], tone: None };
    let saved = r#"{"title":"T","segments":[{"page":1,"panel":0,"text":"Kaito names himself.","hook":true},{"page":0,"panel":0,"text":"Ruko meets a stranger."},{"page":1,"panel":0,"text":"The stranger is Kaito."}]}"#;
    std::fs::write(chapter_dir(&ch).join("story.json"), saved).unwrap();
    let story = load_story(&ch).unwrap();
    assert_eq!(story.segments, vec![seg(1, true, "Kaito names himself."), seg(0, false, "Ruko meets a stranger."), seg(1, false, "The stranger is Kaito.")]);
    let out = vec![("c1".to_string(), story)];

    // A chapter never opens on a teaser: the old hook line is not spoken anywhere.
    export_for_pipeline(&ch, &out[0].1).unwrap();
    let script: serde_json::Value = serde_json::from_slice(&std::fs::read(ch.output_dir.join("script_edited.json")).unwrap()).unwrap();
    assert_eq!(script[0]["text"], "Ruko meets a stranger.");
    assert_eq!(script[1]["text"], "The stranger is Kaito.");
    let beats = std::fs::read_to_string(ch.output_dir.join("narration.studio.json")).unwrap();
    assert!(!beats.contains("Kaito names himself."), "{beats}");
}

#[test]
fn subscribe_reminders_fall_on_the_manga_running_time() {
    use crate::story::reminder_at;
    assert_eq!(reminder_at(0.0, 20.0), None, "nothing before minute 27");
    assert_eq!(reminder_at(25.0, 4.0), Some(0.5));
    assert_eq!(reminder_at(27.0, 3.0), Some(0.0), "a mark at the very start belongs to this chapter");
    assert_eq!(reminder_at(24.0, 3.0), None, "... not to the one ending on it");
    assert_eq!(reminder_at(30.0, 70.0), None, "the next mark is at 107");
    assert_eq!(reminder_at(105.0, 4.0), Some(0.5));
    assert_eq!(reminder_at(186.0, 2.0), Some(0.5));
    assert_eq!(reminder_at(10.0, 0.0), None);
    assert_eq!(reminder_at(f64::NAN, 3.0), None);
    // A range of chapters played from the start: ~3.5 minutes each.
    let count = |minutes: f64| (0..).map(|i| i as f64 * 3.5).take_while(|s| *s < minutes).filter(|s| reminder_at(*s, 3.5).is_some()).count();
    assert_eq!(count(60.0), 1);
    assert_eq!(count(120.0), 2);
    assert_eq!(count(480.0), 6);
    assert_eq!(count(1200.0), 15);
}

#[test]
fn words_before_estimates_chapters_without_a_story() {
    use crate::story::words_before;
    // Saved stories count as they are.
    assert_eq!(words_before(&[(Some(600), 40), (Some(800), 60)], &[(600, 40), (800, 60)], (500, 50)), 1400.0);
    // One without a story: from its pages at the samples' words per page (1400 / 100 = 14).
    assert_eq!(words_before(&[(Some(600), 40), (None, 50)], &[(600, 40), (800, 60)], (500, 50)), 600.0 + 700.0);
    // No page count: the average chapter.
    assert_eq!(words_before(&[(None, 0), (None, 0)], &[(600, 40), (800, 60)], (500, 50)), 1400.0);
    // Nothing known yet: this chapter's own estimate.
    assert_eq!(words_before(&[(None, 0), (None, 20)], &[], (500, 50)), 500.0 + 200.0);
    assert_eq!(words_before(&[], &[(600, 40)], (500, 50)), 0.0);
}

#[tokio::test]
async fn later_chapters_continue_the_story_and_carry_planned_reminders() {
    let dir = tempfile::tempdir().unwrap();
    let mut ch = chapter(dir.path(), 2);
    let brain = Mutex::new(Brain::load_in(dir.path().join("brain"), "m1", "Test Manga"));
    read_chapter_with(&Fake::new(&[PAGE1, PAGE2]), &model(), &ch, &brain, &ReadOptions::default(), &silent(), &CancellationToken::new()).await.unwrap();
    brain.lock().unwrap().set_recap("Chapter 0", "Ruko trains alone.");
    let mut read = load_read(&ch).unwrap();
    read.cast_checked = true;
    save_read(&ch, &read).unwrap();
    let words = |n: usize| Story { segments: vec![StorySegment { text: vec!["word"; n].join(" "), ..Default::default() }], ..Default::default() };
    let earlier = ChapterRef { chapter_id: "c0".into(), label: "Chapter 0".into(), pages_dir: dir.path().join("c0").join("pages"), output_dir: dir.path().join("c0"), ..ch.clone() };
    let answer = r#"{"title":"T","summary_so_far":"s","segments":[{"page":1,"panel":1,"text":"Ruko meets a stranger."},{"page":2,"panel":1,"text":"Kaito names himself."}]}"#;
    let brain = &brain;
    let prompt_with = move |manga: Vec<ChapterRef>, ch: ChapterRef| {
        async move {
            let llm = Fake::new(&[answer]);
            write_story_with(&llm, &model(), std::slice::from_ref(&ch), &manga, &[], brain, &silent(), &CancellationToken::new()).await.unwrap();
            let seen = llm.seen.lock().unwrap();
            seen[0].messages[1].content.clone()
        }
    };

    // 5000 words (26.3 min) before this ~1 minute chapter: the 27-minute mark falls midway.
    save_story(&earlier, &words(5000)).unwrap();
    let user = prompt_with(vec![earlier.clone(), ch.clone()], ch.clone()).await;
    assert!(user.contains("STORY SO FAR:\nRuko trains alone."), "{user}");
    assert!(user.contains("continue it seamlessly") && user.contains("No recap of earlier events"), "{user}");
    assert!(!user.contains("left off") && !user.contains("next chapter") && !user.contains("hook"), "{user}");
    assert!(user.contains("No wrap-up"), "{user}");
    assert!(user.contains("CALL TO ACTION: around the middle of these pages") && user.contains("subscribe"), "{user}");

    // 3000 words (15.8 min) before it: no mark in this chapter.
    save_story(&earlier, &words(3000)).unwrap();
    assert!(!prompt_with(vec![earlier.clone(), ch.clone()], ch.clone()).await.contains("CALL TO ACTION"));

    // Without the manga's order: "Chapter 9" has 8 chapters before it, each as long as the average
    // story saved next to it (630 words): 26.5 minutes.
    ch.label = "Chapter 9".into();
    save_story(&ChapterRef { output_dir: dir.path().join("ch").join("other"), ..Default::default() }, &words(630)).unwrap();
    assert!(prompt_with(vec![], ch.clone()).await.contains("CALL TO ACTION: in the first part of these pages"));
    ch.label = "Chapter 3".into();
    assert!(!prompt_with(vec![], ch.clone()).await.contains("CALL TO ACTION"));
}

#[test]
fn length_follows_what_the_chapter_says_and_the_cast_flags_looks() {
    use crate::story::{cast_text, length_target};
    // Seven words a line, forty a story page: chapter 1 of a recap channel's video runs ~1100 words.
    assert_eq!((length_target(10, 0), length_target(141, 27), length_target(208, 10), length_target(2000, 30)), (250, 1100, 1450, 2500));
    let dir = tempfile::tempdir().unwrap();
    let mut b = Brain::load_in(dir.path().to_path_buf(), "m1", "T");
    for (name, n, aliases) in [("Mujin", 50, vec!["Brown-haired young man", "Muj"]), ("Bald elder with glasses", 9, vec![]), ("Crying bald boy", 1, vec![])] {
        b.characters.push(Character { id: name.into(), name: name.into(), aliases: aliases.into_iter().map(String::from).collect(), appearances: n, ..Default::default() });
    }
    let cast = cast_text(&b);
    assert!(cast.starts_with("- Mujin (also called: Muj)"), "{cast}");
    assert!(cast.contains("Not named yet:\n- \"Bald elder with glasses\" (known only by looks)"), "{cast}");
    assert!(!cast.contains("Crying bald boy"), "one-off extras are left out");
}

#[test]
fn cast_labels_are_replaced_as_whole_words() {
    use crate::cast::replace_label;
    assert_eq!(replace_label("The fur-collared man sighs; the Fur-collared man eats.", "Fur-collared man", "Hachiro"), "Hachiro sighs; Hachiro eats.");
    assert_eq!(replace_label("A fur-collared man waves.", "Fur-collared man", "Hachiro"), "A Hachiro waves.");
    assert_eq!(replace_label("Bigfur-collared mania", "Fur-collared man", "Hachiro"), "Bigfur-collared mania", "never inside a word");
    assert_eq!(replace_label("The pink-haired girl cries.", "Pink-haired girl", "Hachiro's coworker"), "Hachiro's coworker cries.");
    assert_eq!(replace_label("Then the pink-haired girl cries.", "Pink-haired girl", "his coworker"), "Then his coworker cries.");
    assert_eq!(replace_label("A pink-haired girl cries.", "Pink-haired girl", "coworker"), "A coworker cries.");
}

#[tokio::test]
async fn cast_check_names_one_person_told_by_several_looks() {
    let dir = tempfile::tempdir().unwrap();
    let ch = chapter(dir.path(), 3);
    let brain = Mutex::new(Brain::load_in(dir.path().join("brain"), "m1", "Test Manga"));
    {
        let mut b = brain.lock().unwrap();
        b.observe("Hachiro", None, "the hero", &[]);
        b.observe("Fur-collared man", None, "", &[]);
        b.add_note("[Chapter 1 p2] The fur-collared man buys ice cream.");
        b.add_note("[Chapter 0 p2] The fur-collared man is someone else here.");
    }
    let person = |n: &str| CharacterSeen { name: n.into(), ..Default::default() };
    let line = |s: &str, t: &str| Line { speaker: Some(s.into()), text: t.into(), kind: "speech".into() };
    let page = |i: usize, summary: &str, who: Vec<&str>, lines: Vec<Line>| PageRead {
        index: i,
        summary: summary.into(),
        scene: Scene { kind: "comedy".into(), ..Default::default() },
        panels: vec![Panel { description: summary.into(), characters: who.into_iter().map(person).collect(), lines, bbox: None, ..Default::default() }],
        ..Default::default()
    };
    let read = ChapterRead {
        chapter_id: "c1".into(),
        pages: vec![
            page(0, "Hachiro walks with a girl.", vec!["Hachiro", "Pink-haired girl"], vec![line("Pink-haired girl", "Sooo, Hachiro...")]),
            page(1, "The fur-collared man buys ice cream.", vec!["Fur-collared man", "Messy-haired man"], vec![line("Fur-collared man", "The cap's stuck...")]),
            page(2, "A horned creature attacks the messy-haired man.", vec!["Horned creature", "Messy-haired man"], vec![line("Horned creature", "Crabstick is here. Die!")]),
        ],
        ..Default::default()
    };
    save_read(&ch, &read).unwrap();
    let llm = Fake::new(&[
        r#"{"fixes":[{"label":"Fur-collared man","name":"Hachiro","pages":[]},{"label":"Messy-haired man","name":"Hachiro","pages":[]},{"label":"Pink-haired girl","name":"Hachiro's coworker"},{"label":"Horned creature","name":"Crabstick","pages":[3]},{"label":"Hachiro","name":"Hachiro"}],
            "cast":[{"name":"Hachiro","who":"the hero, a monster living as a human"},{"name":"Crabstick","who":"a fear who feeds on terror"}]}"#,
        r#"{"title":"T","summary_so_far":"s","segments":[{"page":1,"panel":1,"text":"Hachiro walks home."},{"page":2,"panel":1,"text":"He buys ice cream."},{"page":3,"panel":1,"text":"Crabstick attacks."}]}"#,
    ]);
    write_story_with(&llm, &model(), std::slice::from_ref(&ch), &[], &[], &brain, &silent(), &CancellationToken::new()).await.unwrap();

    let r = load_read(&ch).unwrap();
    assert!(r.cast_checked);
    let names = |i: usize| r.pages[i].panels[0].characters.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(0), vec!["Hachiro", "Hachiro's coworker"]);
    assert_eq!(names(1), vec!["Hachiro"], "two labels of one person in a panel are one person");
    assert_eq!(names(2), vec!["Crabstick", "Hachiro"]);
    assert_eq!(r.pages[1].summary, "Hachiro buys ice cream.");
    assert_eq!(r.pages[1].panels[0].lines[0].speaker.as_deref(), Some("Hachiro"));
    assert_eq!(r.pages[2].panels[0].lines[0].speaker.as_deref(), Some("Crabstick"));
    assert!(load_transcript(&ch).unwrap().pages[1].text.starts_with("Hachiro: "), "the transcript follows");

    let b = brain.lock().unwrap();
    let hachiro = b.find("Hachiro").unwrap();
    assert!(hachiro.aliases.iter().any(|a| a == "Fur-collared man"), "the placeholder folds into the named hero: {hachiro:?}");
    assert!(b.find("Fur-collared man").is_some_and(|c| c.name == "Hachiro"));
    assert!(b.find("Horned creature").is_none_or(|c| c.name != "Crabstick"), "a label split by page stays apart");
    assert!(b.notes.contains("[Chapter 1 p2] Hachiro buys ice cream.") && b.notes.contains("[Chapter 0 p2] The fur-collared man"), "{}", b.notes);
    drop(b);

    let seen = llm.seen.lock().unwrap();
    let check = &seen[0].messages[1].content;
    assert!(check.contains("LABELS USED FOR PEOPLE") && check.contains("- Fur-collared man") && check.contains("Sooo, Hachiro"), "{check}");
    let story = &seen[1].messages[1].content;
    assert!(story.contains("WHO IS WHO IN THIS CHAPTER") && story.contains("- Crabstick: a fear who feeds on terror"), "{story}");
    assert!(!story.contains("Fur-collared man buys"), "{story}");
}

#[tokio::test]
async fn story_pages_left_out_are_written_before_saving() {
    let dir = tempfile::tempdir().unwrap();
    let ch = chapter(dir.path(), 3);
    let brain = Mutex::new(Brain::load_in(dir.path().join("brain"), "m1", "Test Manga"));
    let line = |t: &str| Line { speaker: Some("Ruko".into()), text: t.into(), kind: "speech".into() };
    let panel = |lines: Vec<Line>| Panel { characters: vec![CharacterSeen { name: "Ruko".into(), ..Default::default() }], lines, ..Default::default() };
    let scene = Scene { kind: "fight".into(), ..Default::default() };
    let read = ChapterRead {
        pages: vec![
            PageRead { index: 0, summary: "Ruko draws.".into(), scene: scene.clone(), panels: vec![panel(vec![line("Draw!")])], ..Default::default() },
            PageRead { index: 1, summary: "Ruko strikes without a word.".into(), scene: scene.clone(), panels: vec![panel(vec![])], ..Default::default() },
            PageRead { index: 2, summary: "Ruko wins.".into(), scene, panels: vec![panel(vec![line("Done.")])], ..Default::default() },
        ],
        cast_checked: true,
        ..Default::default()
    };
    save_read(&ch, &read).unwrap();
    let llm = Fake::new(&[
        r#"{"title":"T","summary_so_far":"s","segments":[{"page":1,"panel":1,"text":"Ruko draws."},{"page":3,"panel":1,"text":"Ruko wins."}]}"#,
        r#"{"segments":[{"page":2,"panel":1,"text":"One silent strike ends it."},{"page":3,"text":"not asked for"}]}"#,
    ]);
    let out = write_story_with(&llm, &model(), std::slice::from_ref(&ch), &[], &[], &brain, &silent(), &CancellationToken::new()).await.unwrap();
    let segs: Vec<(usize, &str)> = out[0].1.segments.iter().map(|s| (s.page, s.text.as_str())).collect();
    assert_eq!(segs, vec![(0, "Ruko draws."), (1, "One silent strike ends it."), (2, "Ruko wins.")]);
    let seen = llm.seen.lock().unwrap();
    let fill = &seen[1].messages[1].content;
    assert!(fill.contains("leaves out story pages 2.") && fill.contains("LINE BEFORE: Ruko draws.") && fill.contains("LINE AFTER: Ruko wins.") && fill.contains("Ruko strikes without a word."), "{fill}");
}

#[test]
fn a_passage_moves_through_the_panels_it_lists() {
    use crate::export::panel_parts;
    let seg = |panel: Option<usize>, panels: Vec<usize>, text: &str| StorySegment { page: 0, panel, text: text.into(), panels, ..Default::default() };
    assert_eq!(panel_parts(&seg(Some(0), vec![], "One. Two.")), vec![(Some(0), "One. Two.".to_string())]);
    assert_eq!(panel_parts(&seg(Some(0), vec![2], "He runs. She waits! Done?")), vec![(Some(0), "He runs. She waits!".to_string()), (Some(2), "Done?".to_string())]);
    assert_eq!(panel_parts(&seg(Some(0), vec![1, 2], "Only one sentence.")), vec![(Some(0), "Only one sentence.".to_string())]);
    // The answer's "panels" (1-based) are parsed, the first standing in for a missing "panel".
    let read = ChapterRead { pages: vec![PageRead { index: 0, panels: vec![Panel::default(); 3], ..Default::default() }], ..Default::default() };
    let v = serde_json::json!({"segments":[{"page":1,"panel":1,"panels":[1,3,9],"text":"a"},{"page":1,"panels":[2,3],"text":"b"}]});
    let segs = crate::story::parse_segments(&v, &[0], &read);
    assert_eq!((segs[0].panel, segs[0].panels.clone()), (Some(0), vec![2]));
    assert_eq!((segs[1].panel, segs[1].panels.clone()), (Some(1), vec![2]));

    let dir = tempfile::tempdir().unwrap();
    let ch = chapter(dir.path(), 1);
    std::fs::create_dir_all(panels_for(&ch)).unwrap();
    png(&panels_for(&ch), "p0000_00.jpg", 128, 128);
    png(&panels_for(&ch), "p0000_01.jpg", 130, 128);
    let story = Story { segments: vec![StorySegment { page: 0, panel: Some(0), panels: vec![1], text: "He runs. She waits.".into(), sfx: SfxCue::parse("impact", Some(2.0)), ..Default::default() }], ..Default::default() };
    export_for_pipeline(&ch, &story).unwrap();
    let beats: serde_json::Value = serde_json::from_slice(&std::fs::read(ch.output_dir.join("narration.studio.json")).unwrap()).unwrap();
    let b = beats["beats"].as_array().unwrap();
    assert_eq!(b.len(), 2, "{b:?}");
    assert!(b[0]["panel_image"].as_str().unwrap().ends_with("p0000_00.jpg") && b[1]["panel_image"].as_str().unwrap().ends_with("p0000_01.jpg"));
    assert_eq!((b[0]["text"].as_str(), b[1]["text"].as_str()), (Some("He runs."), Some("She waits.")));
    assert!(b[0].get("sfx").is_some() && b[1].get("sfx").is_none());
    let script: serde_json::Value = serde_json::from_slice(&std::fs::read(ch.output_dir.join("script_edited.json")).unwrap()).unwrap();
    assert_eq!(script[0]["text"], "He runs. She waits.", "the voice reads the passage whole");
}

#[test]
fn rough_panel_boxes_snap_to_the_frames_on_the_page() {
    use crate::panels::{detect, snap};
    // A white page with two framed panels side by side (ink inside), split by a white gutter.
    let mut img = image::RgbImage::from_pixel(1000, 1400, image::Rgb([255, 255, 255]));
    for (x0, x1) in [(50u32, 480u32), (520, 950)] {
        for y in 100..700u32 {
            for x in x0..x1 {
                let edge = x < x0 + 4 || x >= x1 - 4 || y < 104 || y >= 696;
                let v = if edge { 0 } else { 120 + ((x + y) % 40) as u8 };
                img.put_pixel(x, y, image::Rgb([v, v, v]));
            }
        }
    }
    let frames = detect(&image::DynamicImage::ImageRgb8(img));
    assert_eq!(frames.len(), 2, "{frames:?}");
    // The reader's box spills over the gutter into the right panel: it snaps to the left frame.
    let rough = Bbox([0.03, 0.06, 0.56, 0.52]);
    let s = snap(&frames, rough);
    assert!((s.0[0] - 0.05).abs() < 0.01 && (s.0[2] - 0.48).abs() < 0.01 && (s.0[1] - 0.071).abs() < 0.01, "{s:?}");
    // A box matching no frame (two panels at once) stays as read.
    let both = Bbox([0.04, 0.06, 0.96, 0.52]);
    assert_eq!(snap(&frames, both), both);
}

#[test]
fn cast_names_must_be_mentioned_in_the_chapter() {
    use crate::cast::mentioned;
    let said = "hachiro walks home. \"crabstik's the name!\" frankenstein laughs.";
    assert!(mentioned(said, "Hachiro") && mentioned(said, "Crabstik") && mentioned(said, "Rodolphe Frankenstein"));
    assert!(!mentioned(said, "Abyss★9000"), "a later chapter's monster that merely looks alike");
    assert!(mentioned(said, "Hachiro's coworker"), "a role needs no mention");
}

#[test]
fn passages_carry_their_emotion_and_skip_text_only_panels() {
    let read = ChapterRead {
        pages: vec![PageRead {
            index: 0,
            panels: vec![Panel::default(), Panel::default()],
            ..Default::default()
        }],
        ..Default::default()
    };
    let v = serde_json::json!({"segments":[
        {"page":1,"panel":1,"text":"a","emotion":"Comedy","mood":"smug","intensity":12},
        {"page":1,"panel":2,"text":"b","emotion":"giddy"},
        {"page":1,"panel":2,"text":"c"}]});
    let segs = crate::story::parse_segments(&v, &[0], &read);
    assert_eq!(segs[0].tone, Some(Tone { kind: "comedy".into(), mood: "smug".into(), intensity: 10 }));
    assert_eq!((segs[1].tone.clone(), segs[2].tone.clone()), (None, None), "unknown or missing emotions follow the page's scene");

    let dir = tempfile::tempdir().unwrap();
    let ch = chapter(dir.path(), 1);
    std::fs::create_dir_all(panels_for(&ch)).unwrap();
    png(&panels_for(&ch), "p0000_00.jpg", 128, 128);
    png(&panels_for(&ch), "p0000_01.jpg", 130, 128);
    let line = Line { speaker: Some("Ruko".into()), text: "So, about that...".into(), kind: "speech".into() };
    let action = Panel { characters: vec![CharacterSeen { name: "Ruko".into(), ..Default::default() }], ..Default::default() };
    let read = ChapterRead { pages: vec![PageRead { index: 0, panels: vec![Panel { lines: vec![line], ..Default::default() }, action], ..Default::default() }], ..Default::default() };
    save_read(&ch, &read).unwrap();
    let tone = Some(Tone { kind: "comedy".into(), mood: "smug".into(), intensity: 6 });
    let story = Story { segments: vec![StorySegment { page: 0, panel: Some(0), text: "Ruko grins.".into(), tone, ..Default::default() }], ..Default::default() };
    export_for_pipeline(&ch, &story).unwrap();
    let beats: serde_json::Value = serde_json::from_slice(&std::fs::read(ch.output_dir.join("narration.studio.json")).unwrap()).unwrap();
    let b = &beats["beats"][0];
    assert!(b["panel_image"].as_str().unwrap().ends_with("p0000_01.jpg"), "the balloon-only panel is passed over: {b}");
    assert_eq!((b["kind"].as_str(), b["mood"].as_str(), b["intensity"].as_u64()), (Some("comedy"), Some("smug"), Some(6)));
}

#[test]
fn a_caption_page_keeps_the_last_action_picture() {
    let dir = tempfile::tempdir().unwrap();
    let ch = chapter(dir.path(), 2);
    std::fs::create_dir_all(panels_for(&ch)).unwrap();
    png(&panels_for(&ch), "p0000_00.jpg", 128, 128);
    png(&panels_for(&ch), "p0001_00.jpg", 256, 256);
    let action = Panel { characters: vec![CharacterSeen { name: "Dowan".into(), ..Default::default() }], ..Default::default() };
    let caption = Panel { lines: vec![Line { speaker: None, text: "I'm a substitute gamer.".into(), kind: "narration".into() }], ..Default::default() };
    let read = ChapterRead { pages: vec![PageRead { index: 0, panels: vec![action], ..Default::default() }, PageRead { index: 1, panels: vec![caption], ..Default::default() }], ..Default::default() };
    save_read(&ch, &read).unwrap();
    let seg = |page: usize, text: &str| StorySegment { page, panel: Some(0), text: text.into(), ..Default::default() };
    export_for_pipeline(&ch, &Story { segments: vec![seg(0, "Dowan plays."), seg(1, "He is a substitute gamer.")], ..Default::default() }).unwrap();
    let beats: serde_json::Value = serde_json::from_slice(&std::fs::read(ch.output_dir.join("narration.studio.json")).unwrap()).unwrap();
    let img = |i: usize| beats["beats"][i]["panel_image"].as_str().unwrap().to_string();
    assert!(img(1).ends_with("p0000_00.jpg"), "the caption-only page shows the last action panel: {}", img(1));
}
