#![cfg(feature = "story")]
//! A chapter longer than the digest cap (design 6.1): it is sent in parts cut where the scene
//! kind changes, in order, each later part with the previous part's last scene as context, and
//! the parts' scenes become one chapter. Its own test binary: it rewrites a chapter's Read.

mod common;

use std::sync::Arc;

use anyhow::anyhow;
use mv_ai::CancellationToken;
use mv_anime::backend::NoImages;
use mv_anime::llm::{Ai, Canned};
use mv_anime::queue::{Job, JobCtx, JobKind, RunCtx};
use mv_anime::source::DIGEST_CAP;
use mv_anime::story;
use mv_anime::*;
use serde_json::json;

fn scene(title: &str, pages: [usize; 2], line: &str) -> serde_json::Value {
    json!({"title": title, "kind": "dialogue", "tone": "drama", "colour": "neutral daylight", "intensity": 3, "target_s": 30,
        "summary": format!("{title} happens."), "source": [{"page": pages[0], "panel": 0}, {"page": pages[1], "panel": 0}],
        "beats": [{"purpose": "it happens", "source": [{"page": pages[0], "panel": 1}], "lines": [
            {"speaker": "Ama", "kind": "speech", "original": "", "spoken": line, "emotion": "tender", "strength": 1, "pace": "slow",
             "keep": false, "page": pages[0], "panel": 1}]}]})
}

#[tokio::test]
async fn a_long_chapter_goes_in_parts_cut_at_the_kind_change() {
    let dir = common::temp_root();
    common::set_home(dir.path());
    std::env::set_var("MANGAVOX_LOG_DIR", dir.path().join("logs"));
    let manga = common::build_text_only(dir.path());

    // Chapter 1 becomes long: every page summary is a quarter of the cap and a bit more, so the
    // whole chapter is over the cap and each half (talk, fight) fits.
    let mut long = manga.chapters[0].read.clone();
    for p in long.pages.iter_mut() {
        p.summary = format!("{} {}", p.summary, "words ".repeat(DIGEST_CAP / 4 / 6 + 50));
    }
    let file = manga.chapters[0].chref.output_dir.join("studio").join("read.json");
    std::fs::write(&file, serde_json::to_vec(&long).expect("json")).expect("write the long Read");
    assert_eq!(story::chunk_pages(&long, DIGEST_CAP), vec![vec![0, 1], vec![2, 3]]);

    let store = Store::open("p-long");
    store.save_series(&Series { id: "p-long".into(), manga_id: common::MANGA_ID.into(), manga_title: common::MANGA_TITLE.into(), ..Series::default() }).expect("series");
    store.save_episode(&Episode { id: "ep01".into(), chapters: manga.picks(), ..Episode::default() }).expect("episode");

    let answers = [
        json!({"scenes": [scene("Morning talk", [0, 1], "Good morning.")]}).to_string(),
        json!({"scenes": [scene("Night fight", [2, 3], "Stay behind me.")]}).to_string(),
        json!({"scenes": [scene("Chapter two", [0, 3], "We go on.")]}).to_string(),
    ];
    let canned = Arc::new(Canned::new(answers));
    let ai = Ai::fake(canned.clone());
    let run = RunCtx { ai: Box::new(move |_| Some(ai.clone())), image: Arc::new(NoImages), python: Box::new(|| Err(anyhow!("No voice Python in tests."))) };
    let job = Job { id: 7, project: "p-long".into(), kind: JobKind::Story { episode: "ep01".into() }, ..Job::default() };
    let lines = std::sync::Mutex::new(Vec::<String>::new());
    let progress = |_: f32, l: &str| {
        if let Ok(mut v) = lines.lock() {
            v.push(l.to_string());
        }
    };
    let cancel = CancellationToken::new();
    let cx = JobCtx { job: &job, store: &store, run: &run, progress: &progress, cancel: &cancel };
    story::run_job(&cx, "ep01").await.expect("adapted");

    let seen = canned.seen();
    assert_eq!(seen.len(), 3, "two parts for chapter 1, one call for chapter 2");
    let (one, two) = (&seen[0].messages[1].content, &seen[1].messages[1].content);
    assert!(one.contains("part 1 of 2") && one.contains("[p0]") && one.contains("[p1]") && !one.contains("[p2]"), "part 1");
    assert!(two.contains("part 2 of 2") && two.contains("[p2]") && two.contains("[p3]") && !two.contains("[p0]"), "part 2");
    assert!(one.chars().count() < DIGEST_CAP + 5_000 && two.chars().count() < DIGEST_CAP + 5_000);
    assert!(!one.contains("THE SCENE JUST BEFORE"));
    assert!(two.contains("THE SCENE JUST BEFORE") && two.contains("Title: Morning talk") && two.contains("Ama: \"Good morning.\""), "continuity");
    // Each part asks for its share of the chapter's length (the talk pages pace slower).
    let budget = |m: &str| m.split("about ").nth(1).and_then(|r| r.split(' ').next()).and_then(|n| n.parse::<f32>().ok()).unwrap_or(0.);
    let total = story::chapter_seconds(&long);
    assert!((budget(one) + budget(two) - total).abs() <= 1., "{} + {} vs {total}", budget(one), budget(two));
    assert!(budget(one) > budget(two));
    assert!(lines.lock().map(|v| v.iter().any(|l| l.contains("part 2 of 2"))).unwrap_or(false));

    let s = store.story("ep01").expect("story");
    let titles: Vec<&str> = s.scenes.iter().map(|sc| sc.title.as_str()).collect();
    assert_eq!(titles, vec!["Morning talk", "Night fight", "Chapter two"]);
    let c1: f32 = s.scenes.iter().take(2).map(|sc| sc.target_s).sum();
    assert!((c1 - total).abs() < 0.5, "the chapter's parts share its budget: {c1} vs {total}");
    assert_eq!(s.lines.iter().map(|l| l.speaker.as_deref()).collect::<Vec<_>>(), vec![Some("ama"); 3]);
}
