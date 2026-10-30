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
            StorySegment { page: 1, panel: Some(0), text: "One.".into(), delivery: None },
            StorySegment { page: 1, panel: Some(5), text: "Two.".into(), delivery: Some(Delivery { kind: "fight".into(), intensity: 14 }) },
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
    // Old story files without the field still load.
    let old: StorySegment = serde_json::from_str(r#"{"page":2,"panel":null,"text":"x"}"#).unwrap();
    assert_eq!(old.delivery, None);
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
        assert!(seen[1].images.len() >= 2);
        let prompt = &seen[1].messages.last().unwrap().content;
        assert!(prompt.contains("Image 2 = reference face of"));
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

    let story = Fake::new(&[r#"{"title":"Duel","summary_so_far":"s","segments":[{"page":1,"panel":1,"text":"Ruko draws."},{"page":2,"panel":null,"text":"Kaito speaks."},{"page":9,"text":"stray"}]}"#]);
    let out = write_story_with(&story, &model(), std::slice::from_ref(&ch), &["Hard".to_string()], &brain, &silent(), &CancellationToken::new()).await.unwrap();
    assert_eq!(out.len(), 1);
    let s = &out[0].1;
    assert_eq!(s.title, "Duel");
    assert_eq!(s.segments, vec![
        StorySegment { page: 0, panel: Some(0), text: "Ruko draws.".into(), delivery: None },
        StorySegment { page: 1, panel: None, text: "Kaito speaks.".into(), delivery: None },
    ]);
    let req = story.seen.lock().unwrap();
    let sys = &req[0].messages[0].content;
    assert!(sys.contains("Hard: blunt"));
    let user = &req[0].messages[1].content;
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
    let h = crate::hook::write_hook_with(&fake, &model(), "Test Manga", &cands, 20, &CancellationToken::new()).await.unwrap();
    assert_eq!(h.lines, vec![(1, "A blade is drawn.".to_string()), (2, "A friend turns traitor.".to_string())]);
    let seen = fake.seen.lock().unwrap();
    let user = &seen[0].messages[1].content;
    assert!(user.contains("[2] (Chapter 2) Kaito betrays the guild."), "{user}");
    assert!(user.contains("about 52 words"));
    assert_eq!(seen.len(), 2, "one retry after the non-JSON answer");
}
