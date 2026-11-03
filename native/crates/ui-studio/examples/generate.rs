//! Studio's Generate without the window:
//! `cargo run -p ui-studio --example generate -- "<manga title>" [from-to]`
//!
//! Queues the chapters (all of them, or the range) with the saved Generate answers and runs the
//! same pipeline the dashboard does (Read → Transcript → Story → Audio → Music → Video), using
//! the assistant's model for the AI steps. Prints each chapter as it moves and exits when none is
//! left to run. Close MangaVox first: the app and this share `studio/pipeline_queue.json`.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use mv_recap::ModelChoice;
use ui_studio::pipeline::{self, Ctx, ItemState, StepState};
use ui_studio::{data, setup};

const STEP_NAMES: [&str; 6] = ["read", "transcript", "story", "audio", "music", "video"];

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let title = args.first().map(|s| s.to_lowercase()).ok_or_else(|| anyhow::anyhow!("usage: generate \"<manga title>\" [from-to]"))?;
    let range = args.get(1).and_then(|r| r.split_once('-')).and_then(|(a, b)| Some((a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?)));

    // Like the app: the Python voice and video scripts find the voice environment through this.
    std::env::set_var("MANGAVOX_DATA", mv_core::paths::root());

    let groups = data::load()?;
    let group = groups.iter().find(|g| g.title.to_lowercase().contains(&title)).ok_or_else(|| anyhow::anyhow!("no downloaded manga matches \"{title}\""))?;
    let chapters: Vec<_> = group.chapters.iter().filter(|c| range.is_none_or(|(a, b)| c.number.is_some_and(|n| n >= a && n <= b))).cloned().collect();
    if chapters.is_empty() {
        anyhow::bail!("no chapters of {} in that range", group.title);
    }

    let rt = mv_ui::bg::runtime();
    let env = rt.block_on(setup::check());
    if !env.runnable() {
        anyhow::bail!("Studio setup is not ready (Python / pipeline scripts)");
    }
    let (provider, model) = assistant_model()?;
    let workspace = mv_core::paths::root().join("assistant-workspace");
    let prefs = mv_recap::Prefs::load();
    pipeline::set_context(Ctx {
        reg: Arc::new(mv_ai::Registry::new(mv_core::paths::config(), workspace)),
        model: Some((ModelChoice { provider, model: model.clone() }, model.clone())),
        env: Some(env),
        speed: prefs.speed,
        resolution: prefs.resolution,
        styles: prefs.styles,
    });

    // Starting the queue would also run other chapters waiting in it: hold them (paused) until
    // these are done, and leave the queue on hold afterwards.
    let ids: Vec<String> = chapters.iter().map(|c| c.id.clone()).collect();
    let held: Vec<u64> = pipeline::snapshot().items.iter().filter(|i| i.state.active() && !ids.contains(&i.chapter.id)).map(|i| i.id).collect();
    for &id in &held {
        pipeline::pause(id);
    }
    if !held.is_empty() {
        println!("holding {} other waiting chapters (paused) until these are done", held.len());
    }

    let ((new, widened, already), errors) = ui_studio::queue_saved(&chapters, true);
    for e in errors {
        println!("warning: {e}");
    }
    println!("{}: {} chapters queued ({new} new, {widened} widened, {already} already waiting), model {model}", group.title, chapters.len());

    let mut seen: HashMap<u64, String> = HashMap::new();
    loop {
        std::thread::sleep(Duration::from_secs(5));
        let q = pipeline::snapshot();
        let mine: Vec<_> = q.items.iter().filter(|i| ids.contains(&i.chapter.id)).collect();
        for i in &mine {
            let step = i.current.map(|s| STEP_NAMES[s]).unwrap_or("-");
            let line = format!("{:?} {step}", i.state);
            if seen.get(&i.id) != Some(&line) {
                let steps: Vec<&str> = i.steps.iter().map(|s| match s {
                    StepState::Done => "done",
                    StepState::Skipped => "skip",
                    StepState::Running => "run",
                    StepState::Failed => "FAIL",
                    StepState::Pending => "-",
                }).collect();
                println!("{:<8} {line:<16} [{}]{}", i.chapter.label(), steps.join(" "), i.error.as_deref().map(|e| format!("  error: {e}")).unwrap_or_default());
                seen.insert(i.id, line);
            }
        }
        if q.paused {
            println!("queue paused; stopping");
            break;
        }
        if !mine.iter().any(|i| i.state.active()) {
            let failed = mine.iter().filter(|i| i.state == ItemState::Failed).count();
            println!("finished: {} done, {failed} failed", mine.len() - failed);
            pipeline::pause_all();
            break;
        }
    }
    Ok(())
}

/// The assistant's saved provider and model (`assistant/threads.json` prefs).
fn assistant_model() -> anyhow::Result<(mv_ai::AiProviderId, String)> {
    let path = mv_core::paths::root().join("assistant").join("threads.json");
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    let prefs = &v["prefs"];
    let provider = serde_json::from_value(prefs["provider"].clone())?;
    let model = prefs["model"].as_str().filter(|m| !m.is_empty()).ok_or_else(|| anyhow::anyhow!("no assistant model chosen"))?;
    Ok((provider, model.to_string()))
}
