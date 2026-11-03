//! Runs the 3D pipeline on one downloaded chapter without the app, the way the 3D tab does.
//!
//!   cargo run -p mv-3d --example run_chapter -- --list
//!   cargo run -p mv-3d --example run_chapter -- --chapter <id> [--keep-going] [--scene N] [--approve N] [--small] [--offline]
//!
//! Uses the model picked in the app's assistant (or `--provider` / `--model`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use mv_recap::{ChapterRef, ModelChoice};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn chapters() -> Result<Vec<(ChapterRef, usize)>> {
    let conn = rusqlite::Connection::open(mv_core::paths::database())?;
    let mut stmt = conn.prepare(
        "SELECT c.id, c.manga_id, m.title, c.number, c.pages_dir FROM chapter c JOIN manga m ON m.id = c.manga_id
         WHERE c.download_status = 'complete' AND c.pages_dir IS NOT NULL AND c.pages_dir != '' ORDER BY m.title, c.number",
    )?;
    let rows = stmt.query_map([], |r| {
        let number: Option<f64> = r.get(3)?;
        Ok(ChapterRef {
            chapter_id: r.get(0)?,
            manga_id: r.get(1)?,
            manga_title: r.get(2)?,
            label: number.map(|n| format!("Chapter {n}")).unwrap_or_else(|| "Chapter".into()),
            pages_dir: PathBuf::from(r.get::<_, String>(4)?),
            output_dir: PathBuf::new(),
        })
    })?;
    let mut out = vec![];
    for r in rows {
        let mut ch = r?;
        ch.output_dir = mv_core::paths::studio_chapter(&ch.manga_id, &ch.chapter_id);
        let pages = mv_recap::list_pages(&ch.pages_dir).len();
        out.push((ch, pages));
    }
    Ok(out)
}

fn picked_model() -> Option<ModelChoice> {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(mv_core::paths::root().join("assistant").join("threads.json")).ok()?).ok()?;
    let p = v.get("prefs")?;
    Some(ModelChoice { provider: serde_json::from_value(p.get("provider")?.clone()).ok()?, model: p.get("model")?.as_str()?.to_string() })
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let all = chapters()?;
    if args.iter().any(|a| a == "--list") {
        for (ch, pages) in &all {
            let read = mv_recap::load_read(ch).map(|r| r.pages.len()).unwrap_or(0);
            let ep = mv_3d::paths::episode_dir(&ch.manga_id, &ch.chapter_id).join("episode.json").is_file();
            println!("{}  {} · {}  pages={pages} read={read}{}", ch.chapter_id, ch.manga_title, ch.label, if ep { " [3D]" } else { "" });
        }
        return Ok(());
    }
    let id = arg(&args, "--chapter").ok_or_else(|| anyhow!("--chapter <id> (see --list)"))?;
    let (ch, _) = all.into_iter().find(|(c, _)| c.chapter_id == id).ok_or_else(|| anyhow!("no downloaded chapter {id}"))?;
    let dir = mv_3d::paths::episode_dir(&ch.manga_id, &ch.chapter_id);
    if let Some(n) = arg(&args, "--approve").and_then(|n| n.parse::<usize>().ok()) {
        mv_3d::approve(&dir, n - 1)?;
        println!("approved scene {n}");
    }
    let model = match (arg(&args, "--provider"), arg(&args, "--model")) {
        (Some(p), Some(m)) => ModelChoice { provider: serde_json::from_value(serde_json::Value::String(p))?, model: m },
        _ => picked_model().ok_or_else(|| anyhow!("no model picked in the app; pass --provider and --model"))?,
    };
    println!("{} · {} with {}/{}", ch.manga_title, ch.label, model.provider.as_str(), model.model);
    let workspace = mv_core::paths::root().join("assistant-workspace");
    let reg = Arc::new(mv_ai::Registry::new(mv_core::paths::config(), workspace));
    let mut ctx = mv_3d::Ctx::new(reg, model);
    ctx.online = !args.iter().any(|a| a == "--offline");
    if args.iter().any(|a| a == "--small") {
        (ctx.width, ctx.height, ctx.samples) = (960, 540, 12);
    }
    let opts = mv_3d::RunOptions {
        keep_going: args.iter().any(|a| a == "--keep-going"),
        only: arg(&args, "--scene").and_then(|n| n.parse::<usize>().ok()).map(|n| n - 1),
        replan: args.iter().any(|a| a == "--replan"),
    };
    let cid = ch.chapter_id.clone();
    let printer = tokio::spawn(async move {
        let mut shown = 0usize;
        let mut waiting = None;
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let Some(l) = mv_3d::live(&cid) else { continue };
            // The log keeps the newest 300 lines; print what is new.
            let start = shown.min(l.log.len());
            for line in &l.log[start..] {
                println!("  {line}");
            }
            shown = l.log.len();
            if l.waiting != waiting {
                if let Some(w) = &l.waiting {
                    println!("  (waiting) {w}");
                }
                waiting = l.waiting.clone();
            }
        }
    });
    let r = mv_3d::run(ctx, ch, opts).await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    printer.abort();
    let ep = r?;
    for s in &ep.scenes {
        let clip = s.render.as_ref().and_then(|r| r.clip.as_ref()).map(|c| c.display().to_string()).unwrap_or_default();
        println!("scene {}: {} [{}] {clip}", s.index + 1, s.title, s.status.label());
    }
    if let Some(v) = &ep.video {
        println!("episode: {}", v.display());
    }
    println!("episode folder: {}", dir.display());
    Ok(())
}
