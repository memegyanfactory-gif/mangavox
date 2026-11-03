//! Writes a full recap's hook from copies of made chapters (the user's files are not touched):
//! `cargo run -p mv-recap --example hook_probe -- <scratch dir> <seconds> <provider> <model> <brain dir> <label>=<chapter output dir>...`
//! Prints the idea and each line with the beat it is shown over; the candidates offered are
//! written to `<scratch dir>/candidates.txt`.
use std::sync::Arc;

use mv_ai::{AiProviderId, CancellationToken, Registry};
use mv_recap::hook::{pick_candidates, story_context, write_hook, MAX_CANDIDATES};
use mv_recap::{Brain, ModelChoice};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let scratch = std::path::PathBuf::from(&a[1]);
    let seconds: u32 = a[2].parse()?;
    let provider: AiProviderId = serde_json::from_value(serde_json::json!(a[3]))?;
    let model = ModelChoice { provider, model: a[4].clone() };
    let brain = Brain::load_in(std::path::PathBuf::from(&a[5]), "probe", "");
    let mut chapters = vec![];
    for arg in &a[6..] {
        let (label, dir) = arg.split_once('=').expect("label=dir");
        let sm: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(std::path::Path::new(dir).join("sync_map.json"))?)?;
        let texts: Vec<String> = sm["beat_segments"].as_array().into_iter().flatten().map(|b| if b["panel_image"].is_string() { b["text"].as_str().unwrap_or("").trim().to_string() } else { String::new() }).collect();
        chapters.push((label.to_string(), texts));
    }
    let cands = pick_candidates(&chapters, MAX_CANDIDATES);
    let story = story_context(&brain, &chapters.last().map(|c| c.0.clone()).unwrap_or_default());
    std::fs::create_dir_all(&scratch)?;
    let mut dump = story.clone();
    for c in &cands {
        dump += &format!("[{}] ({}) {}\n", c.id, c.chapter, c.text);
    }
    std::fs::write(scratch.join("candidates.txt"), dump)?;
    let reg = Arc::new(Registry::new(mv_core::paths::config(), scratch.join("ws")));
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let h = rt.block_on(write_hook(reg, model, &brain.manga_title, &story, &cands, seconds, CancellationToken::new()))?;
    let flat: Vec<&String> = chapters.iter().flat_map(|c| c.1.iter()).collect();
    println!("IDEA: {}\nWORDS: {} (candidates {})\n", h.idea, h.words(), cands.len());
    for (id, text) in &h.lines {
        println!("[{id}] {text}\n      shown over: {}", flat.get(*id).map(|s| s.as_str()).unwrap_or("?"));
    }
    println!("OUTRO (cover): {}", h.outro);
    Ok(())
}
