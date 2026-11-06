//! Live check that Read and Story work on every AI provider installed on this PC. It sends real
//! requests, so it is ignored by default:
//!
//! ```text
//! MV_LIVE_PAGE=D:\some\manga\page.jpg   (or several pages separated by ;)
//! MV_LIVE_MODELS=antigravity:gemini-3.8-flash-high,claude-code:claude-sonnet-5-5,codex:default
//! cargo test -p mv-recap live_providers -- --ignored --nocapture
//! ```
//!
//! Each `provider:model` uses the provider ids of the app (`claude-code`, `codex`, `gemini-cli`,
//! `opencode`, `grok-cli`, `antigravity`, `anthropic`, ...). The sign-ins and keys are the ones the
//! app uses (its config file); every workspace and chapter folder is a temp folder.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use mv_ai::{AiEffort, AiMessage, CancellationToken, Registry};

use crate::brain::Brain;
use crate::llm::{ask_story_json, request};
use crate::read::read_chapter_with;
use crate::types::{ChapterRef, ModelChoice, Progress, ProgressFn, ReadOptions};

/// The models to check, from `MV_LIVE_MODELS`.
fn models_from_env() -> Result<Vec<ModelChoice>, String> {
    let raw = std::env::var("MV_LIVE_MODELS").unwrap_or_default();
    let mut out = vec![];
    for item in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (provider, model) = item.split_once(':').ok_or_else(|| format!("'{item}' is not provider:model"))?;
        let provider = serde_json::from_value(serde_json::Value::String(provider.trim().to_string())).map_err(|_| format!("unknown provider '{provider}'"))?;
        out.push(ModelChoice { provider, model: model.trim().to_string() });
    }
    Ok(out)
}

/// Read one page the way Studio does, on a temp chapter folder. `Ok` = a real reading came back.
async fn check_read(reg: &Arc<Registry>, model: &ModelChoice, pages: &[PathBuf], root: &std::path::Path) -> Result<String, String> {
    let pages_dir = root.join("pages");
    std::fs::create_dir_all(&pages_dir).map_err(|e| e.to_string())?;
    for (n, page) in pages.iter().enumerate() {
        let ext = page.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_else(|| "jpg".into());
        std::fs::copy(page, pages_dir.join(format!("{:03}.{ext}", n + 1))).map_err(|e| format!("cannot copy the page: {e}"))?;
    }
    let ch = ChapterRef {
        chapter_id: "live".into(),
        manga_id: "live".into(),
        manga_title: "Live check".into(),
        label: "Chapter 1".into(),
        pages_dir,
        output_dir: root.join("out"),
    };
    let brain = Mutex::new(Brain::load_in(root.join("brain"), "live", "Live check"));
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let sink = logs.clone();
    let progress: ProgressFn = Arc::new(move |p| {
        if let Progress::Log(l) | Progress::Failed(l) = p {
            sink.lock().unwrap().push(l);
        }
    });
    let read = read_chapter_with(reg, model, &ch, &brain, &ReadOptions::default(), &progress, &CancellationToken::new()).await.map_err(|e| e.to_string())?;
    let logs = logs.lock().unwrap().join(" | ");
    if read.pages.len() != pages.len() {
        return Err(format!("{} of {} pages were read. {logs}", read.pages.len(), pages.len()));
    }
    for p in &read.pages {
        // A reply that was not JSON is stored as raw text: its scene stays empty.
        if p.scene.kind.is_empty() {
            return Err(format!("page {}: the answer was not a page reading: {}", p.index + 1, p.summary));
        }
        if p.summary.trim().is_empty() && p.panels.is_empty() {
            return Err(format!("page {}: the reading has no summary and no panels", p.index + 1));
        }
    }
    let p = &read.pages[read.pages.len() - 1];
    let lines: usize = read.pages.iter().flat_map(|p| &p.panels).map(|pn| pn.lines.len()).sum();
    Ok(format!("{} pages, {lines} lines; last: {}", read.pages.len(), crate::llm::truncate(&p.summary, 80)))
}

/// One story-style call, built like `story_request` (medium effort), with a tiny prompt.
async fn check_story(reg: &Arc<Registry>, model: &ModelChoice, root: &std::path::Path) -> Result<String, String> {
    let work = root.join("story_work");
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let system = "You are a narrator. Reply with STRICT JSON only: one object, no markdown fences, no commentary. Do not run commands or open files.";
    let prompt = r#"Page 1: a boy finds a glowing sword in a cave. Page 2: he lifts it and the cave shakes.
Retell this in one short passage per page. Reply with exactly: {"title": "...", "segments": [{"page": 0, "text": "..."}, {"page": 1, "text": "..."}]}"#;
    let mut req = request(model, vec![AiMessage::system(system), AiMessage::user(prompt)], vec![], Some(work));
    req.effort = Some(AiEffort::Medium);
    let answer = ask_story_json(reg, req, &CancellationToken::new()).await.map_err(|e| e.to_string())?;
    let v = answer.map_err(|raw| format!("the answer was not JSON: {}", crate::llm::truncate(&raw, 120)))?;
    let n = v.get("segments").and_then(|s| s.as_array()).map(|s| s.len()).unwrap_or(0);
    if n == 0 {
        return Err(format!("the JSON has no segments: {}", crate::llm::truncate(&v.to_string(), 120)));
    }
    Ok(format!("{n} segments"))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "sends real requests to the AI providers; set MV_LIVE_MODELS and MV_LIVE_PAGE"]
async fn read_and_story_work_on_every_installed_provider() {
    let models = match models_from_env() {
        Ok(m) if !m.is_empty() => m,
        Ok(_) => return eprintln!("SKIP: set MV_LIVE_MODELS, e.g. antigravity:gemini-3.8-flash-high,claude-code:claude-sonnet-5-5"),
        Err(e) => panic!("MV_LIVE_MODELS: {e}"),
    };
    // One page, or several separated by `;` (read as one chapter, the way Studio reads it).
    let pages: Vec<PathBuf> = std::env::var("MV_LIVE_PAGE").unwrap_or_default().split(';').map(str::trim).filter(|s| !s.is_empty()).map(PathBuf::from).collect();
    if pages.is_empty() || pages.iter().any(|p| !p.is_file()) {
        return eprintln!("SKIP: set MV_LIVE_PAGE to a manga page image (jpg, png or webp), or several separated by ;");
    }
    let tmp = tempfile::tempdir().expect("temp dir");
    // The registry as the app builds it: the app's config file (sign-ins, keys) and a workspace.
    let reg = Arc::new(Registry::new(mv_core::paths::config(), tmp.path().join("registry_ws")));
    let mut failed: Vec<String> = vec![];
    for (n, m) in models.iter().enumerate() {
        let label = format!("{}:{}", m.provider.as_str(), m.model);
        let root = tmp.path().join(format!("m{n}"));
        let t = Instant::now();
        let read = check_read(&reg, m, &pages, &root.join("read")).await;
        println!("{} read   {label} ({:.0}s) {}", if read.is_ok() { "PASS" } else { "FAIL" }, t.elapsed().as_secs_f32(), read.as_ref().map(|s| s.as_str()).unwrap_or_else(|e| e.as_str()));
        let t = Instant::now();
        let story = check_story(&reg, m, &root.join("story")).await;
        println!("{} story  {label} ({:.0}s) {}", if story.is_ok() { "PASS" } else { "FAIL" }, t.elapsed().as_secs_f32(), story.as_ref().map(|s| s.as_str()).unwrap_or_else(|e| e.as_str()));
        if let Err(e) = read {
            failed.push(format!("read {label}: {e}"));
        }
        if let Err(e) = story {
            failed.push(format!("story {label}: {e}"));
        }
    }
    assert!(failed.is_empty(), "{} check(s) failed:\n{}", failed.len(), failed.join("\n"));
}

/// Live check of the Studio switches on real pages (`MV_LIVE_PAGE`, several separated by `;`):
/// Auto starting from `MV_LIVE_START` (a model that fails, e.g. one that cannot see pictures)
/// must switch and read every page; Light read must pick a cheaper reader and read every page.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "sends real requests to the AI providers; set MV_LIVE_PAGE and MV_LIVE_START"]
async fn auto_switch_and_light_read_work_live() {
    let pages: Vec<PathBuf> = std::env::var("MV_LIVE_PAGE").unwrap_or_default().split(';').map(str::trim).filter(|s| !s.is_empty()).map(PathBuf::from).collect();
    let Some(start) = models_from_env_var("MV_LIVE_START").into_iter().next() else { return eprintln!("SKIP: set MV_LIVE_START=provider:model") };
    if pages.is_empty() {
        return eprintln!("SKIP: set MV_LIVE_PAGE");
    }
    let tmp = tempfile::tempdir().expect("temp dir");
    let reg = Arc::new(Registry::new(mv_core::paths::config(), tmp.path().join("registry_ws")));
    for (name, light, auto) in [("auto", false, true), ("light", true, false)] {
        let root = tmp.path().join(name);
        let pages_dir = root.join("pages");
        std::fs::create_dir_all(&pages_dir).unwrap();
        for (n, p) in pages.iter().enumerate() {
            std::fs::copy(p, pages_dir.join(format!("{:03}.jpg", n + 1))).unwrap();
        }
        let ch = ChapterRef { chapter_id: name.into(), manga_id: "live".into(), manga_title: "Live check".into(), label: "Chapter 1".into(), pages_dir, output_dir: root.join("out") };
        let brain = Arc::new(Mutex::new(Brain::load_in(root.join("brain"), "live", "Live check")));
        let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
        let sink = logs.clone();
        let progress: ProgressFn = Arc::new(move |p| {
            if let Progress::Log(l) | Progress::Failed(l) = p {
                println!("  [{}] {l}", if l.starts_with("Auto") || l.starts_with("Light") { "switch" } else { "log" });
                sink.lock().unwrap().push(l);
            }
        });
        let t = Instant::now();
        let out = crate::auto::read_smart(reg.clone(), start.clone(), ch, brain, ReadOptions::default(), progress, CancellationToken::new(), light, auto).await;
        let lines: usize = out.as_ref().map(|r| r.pages.iter().flat_map(|p| &p.panels).map(|pn| pn.lines.len()).sum()).unwrap_or(0);
        println!("{} {name} ({:.0}s): {}", if out.as_ref().is_ok_and(|r| r.pages.len() == pages.len()) { "PASS" } else { "FAIL" }, t.elapsed().as_secs_f32(), match &out {
            Ok(r) => format!("{} pages, {lines} lines", r.pages.len()),
            Err(e) => format!("{e:#}"),
        });
    }
}

fn models_from_env_var(key: &str) -> Vec<ModelChoice> {
    std::env::var(key)
        .unwrap_or_default()
        .split(',')
        .filter_map(|item| {
            let (p, m) = item.trim().split_once(':')?;
            Some(ModelChoice { provider: serde_json::from_value(serde_json::Value::String(p.trim().into())).ok()?, model: m.trim().into() })
        })
        .collect()
}
