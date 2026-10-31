//! Writes one chapter's story on a scratch copy (the user's files are not touched):
//! `cargo run -p mv-recap --example story_probe -- <chapter output dir> <brain dir> <scratch dir> <label> <provider> <model>`
use std::sync::{Arc, Mutex};

use mv_ai::{AiProviderId, CancellationToken, Registry};
use mv_recap::{Brain, ChapterRef, ModelChoice, Progress};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let (src, brain_src, scratch, label) = (std::path::PathBuf::from(&a[1]), std::path::PathBuf::from(&a[2]), std::path::PathBuf::from(&a[3]), a[4].clone());
    let provider: AiProviderId = serde_json::from_value(serde_json::json!(a[5]))?;
    let out = scratch.join("chapter");
    std::fs::create_dir_all(out.join("studio"))?;
    for f in ["read.json", "transcript.json"] {
        let _ = std::fs::copy(src.join("studio").join(f), out.join("studio").join(f));
    }
    std::fs::create_dir_all(scratch.join("brain"))?;
    std::fs::copy(brain_src.join("brain.json"), scratch.join("brain").join("brain.json"))?;
    let brain = Brain::load_in(scratch.join("brain"), "probe", "");
    let ch = ChapterRef { chapter_id: "probe".into(), manga_id: "probe".into(), manga_title: brain.manga_title.clone(), label, pages_dir: scratch.join("pages"), output_dir: out };
    let reg = Arc::new(Registry::new(mv_core::paths::config(), scratch.join("ws")));
    let progress: mv_recap::ProgressFn = Arc::new(|p| {
        if let Progress::Log(l) | Progress::Failed(l) = &p {
            eprintln!("{l}");
        }
    });
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let model = ModelChoice { provider, model: a[6].clone() };
    let stories = rt.block_on(mv_recap::write_story(reg, model, vec![ch], vec![], Arc::new(Mutex::new(brain)), progress, CancellationToken::new()))?;
    let s = &stories[0].1;
    let words: usize = s.segments.iter().map(|g| g.text.split_whitespace().count()).sum();
    println!("TITLE: {}\nSEGMENTS: {}  WORDS: {words}  LAST PAGE: {}\n", s.title, s.segments.len(), s.segments.iter().map(|g| g.page + 1).max().unwrap_or(0));
    for g in &s.segments {
        println!("{}p{} {}", if g.hook { "[HOOK] " } else { "" }, g.page + 1, g.text);
    }
    Ok(())
}
