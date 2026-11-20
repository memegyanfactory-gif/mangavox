//! The anime director without the window, on one chapter of a downloaded manga:
//! `cargo run -p mv-create --example anime_board -- "<title>" <chapter> "<pages folder>" [steps]`
//!
//! steps (default all, in order): panels, read, cast, scenes, board, check, rough.
//! Works on the Creative project "<title> (anime)" (made if missing), so the result opens in the
//! app. Uses the assistant's saved AI model. "rough" composites a silent animatic from the
//! panels (no voices; the app voices it) with the VFX pack's Python; set MANGAVOX_ANIME_GPU=0 to
//! keep it off the graphics card.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _, Result};
use mv_ai::{AiMessage, AiProviderId, Registry};
use mv_create::anime::{self, checks, director, SourcePanel};
use mv_create::Project;
use serde_json::Value;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (title, chapter, pages) = match args.as_slice() {
        [t, c, p, ..] => (t.clone(), c.parse::<u32>().context("chapter number")?, PathBuf::from(p)),
        _ => bail!("usage: anime_board \"<title>\" <chapter> \"<pages folder>\" [panels,read,cast,scenes,board,check,rough]"),
    };
    let steps: Vec<String> = args.get(3).map(|s| s.split(',').map(|x| x.trim().to_lowercase()).collect()).unwrap_or_else(|| ["panels", "read", "cast", "scenes", "board", "check", "rough"].map(String::from).to_vec());
    let want = |s: &str| steps.iter().any(|x| x == s);
    std::env::set_var("MANGAVOX_DATA", mv_core::paths::root());

    let name = format!("{title} (anime)");
    let mut p = match Project::list().into_iter().find(|x| x.title == name && x.settings.anime) {
        Some(p) => p,
        None => {
            let mut p = Project::new(&name);
            p.adapts = Some(title.clone());
            p.settings.anime = true;
            p.settings.color = true;
            p.settings.rtl = false;
            p.save()?;
            p
        }
    };
    p.chapter_mut(chapter);
    p.save()?;
    println!("project {} at {}", p.title, p.dir().display());

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let ask = asker()?;
    let status: director::Status = Arc::new(|s: String| println!("  · {s}"));

    if want("panels") {
        println!("[panels] cutting {} into panels", pages.display());
        let mut files: Vec<PathBuf> = std::fs::read_dir(&pages)?.flatten().map(|e| e.path()).filter(|f| f.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_lowercase().as_str(), "jpg" | "jpeg" | "png" | "webp"))).collect();
        files.sort();
        let out = p.dir().join("source").join(format!("ch{chapter:03}"));
        let _ = std::fs::remove_dir_all(&out);
        let v = comp("panels", serde_json::json!({ "pages": files, "out_dir": out, "rtl": p.settings.rtl }), &p.dir())?;
        let dir = p.dir();
        let panels: Vec<SourcePanel> = v["panels"].as_array().into_iter().flatten().filter_map(|x| {
            let f = PathBuf::from(x["file"].as_str()?);
            Some(SourcePanel { file: f.strip_prefix(&dir).unwrap_or(&f).to_string_lossy().replace('\\', "/"), page: x["page"].as_u64().unwrap_or(0) as u32 + 1, w: x["w"].as_u64().unwrap_or(0) as u32, h: x["h"].as_u64().unwrap_or(0) as u32, ..Default::default() })
        }).collect();
        println!("  {} panels from {} pages", panels.len(), files.len());
        p.chapter_mut(chapter).anime = anime::AnimeChapter { panels, ..Default::default() };
        p.save()?;
    }
    if want("read") {
        println!("[read] the vision model reads every panel");
        let panels = rt.block_on(director::read(&ask, &p, chapter, &status))?;
        let story = panels.iter().filter(|x| x.story()).count();
        p.chapter_mut(chapter).anime.panels = panels;
        p.save()?;
        println!("  {story} story panels");
    }
    if want("cast") {
        println!("[cast] gathering the characters and places from the pages");
        let (done, added) = rt.block_on(director::cast(&ask, p.clone(), chapter, "", &status))?;
        p = done;
        p.save()?;
        println!("  {} characters ({added} new), {} places: {}", p.cast.len(), p.locations.len(), p.cast.iter().map(|c| format!("{} ({})", c.name, c.role)).collect::<Vec<_>>().join(", "));
    }
    if want("scenes") {
        println!("[scenes] dividing the chapter");
        p = rt.block_on(director::scenes(&ask, p.clone(), chapter, &status))?;
        p.save()?;
        for s in &p.chapter(chapter).unwrap().anime.scenes {
            println!("  {}. {} [{} · {} · {}] panels {}-{} · {:.0} s", s.number, s.title, s.kind, s.location, s.grade, s.from + 1, s.to, s.seconds);
        }
    }
    if want("board") {
        println!("[board] storyboarding scene by scene, then the check and the revisions");
        p = rt.block_on(director::board(&ask, p.clone(), chapter, "", &status))?;
        p.save()?;
    }
    if want("check") || want("board") {
        checks::autofix(&mut p, chapter);
        let issues = checks::check(&p, chapter);
        p.chapter_mut(chapter).anime.issues = issues.clone();
        p.save()?;
        let a = &p.chapter(chapter).unwrap().anime;
        println!("[check] {} cuts in {} scenes, {:.0} s; {} notes ({} errors)", a.cuts.len(), a.scenes.len(), a.seconds(), issues.len(), issues.iter().filter(|i| i.severity == "error").count());
        for i in &issues {
            println!("  [{}] scene {} cut {}: {}", i.code, i.scene, i.cut, i.message);
        }
        let board = p.dir().join(format!("board-ch{chapter:03}.json"));
        std::fs::write(&board, serde_json::to_vec_pretty(&serde_json::json!({ "treatment": a.treatment, "scenes": a.scenes, "cuts": a.cuts, "issues": a.issues }))?)?;
        println!("  the board: {}", board.display());
    }
    if want("rough") {
        println!("[rough] a silent animatic from the panels");
        let a = p.chapter(chapter).cloned().unwrap_or_default().anime;
        let dir = p.dir();
        let mut list = vec![];
        for c in &a.cuts {
            let kind = a.kind_of(c).to_string();
            // A cut is long enough for its lines even without a voice.
            let mut c = c.clone();
            c.seconds = c.seconds.max(anime::pacing::spoken_seconds(&c.lines));
            let panel = c.from.first().and_then(|i| a.panels.get(*i)).map(|sp| (p.file(&sp.file), sp.w, sp.h)).filter(|(f, ..)| f.is_file());
            let out = dir.join("rough").join(format!("ch{chapter:03}-c{:03}.mp4", c.number));
            let mut job = anime::rough_job(&c, &kind, &out, panel.as_ref().map(|(f, w, h)| (f.as_path(), *w, *h)), &p.settings.video_aspect);
            job["quality"] = "rough".into();
            comp("comp", job, &dir)?;
            print!("\r  cut {} of {}", c.number, a.cuts.len());
            list.push(serde_json::json!({ "video": out, "seconds": c.seconds, "transition": c.transition }));
        }
        println!();
        let out = dir.join("video").join(format!("animatic-ch{chapter:03}.mp4"));
        let v = comp("edit", serde_json::json!({ "out": out, "width": 1280, "height": 720, "fps": 24, "quality": "rough", "cuts": list }), &dir)?;
        p.chapter_mut(chapter).anime.animatic = Some(format!("video/animatic-ch{chapter:03}.mp4"));
        p.save()?;
        println!("  the animatic: {} ({} s)", out.display(), v["seconds"]);
    }
    Ok(())
}

/// The assistant's saved provider and model, as the director's way to ask.
fn asker() -> Result<director::Ask> {
    let path = mv_core::paths::root().join("assistant").join("threads.json");
    let v: Value = serde_json::from_slice(&std::fs::read(&path).with_context(|| format!("could not read {}", path.display()))?)?;
    let provider: AiProviderId = serde_json::from_value(v["prefs"]["provider"].clone()).context("no assistant provider chosen")?;
    let model = v["prefs"]["model"].as_str().filter(|m| !m.is_empty()).ok_or_else(|| anyhow!("no assistant model chosen"))?.to_string();
    println!("AI: {provider:?} {model}");
    let reg = Arc::new(Registry::new(mv_core::paths::config(), mv_core::paths::root().join("assistant-workspace")));
    Ok(Arc::new(move |q: director::Query| {
        let (reg, model) = (reg.clone(), model.clone());
        Box::pin(async move {
            static N: AtomicU64 = AtomicU64::new(0);
            let mut messages = vec![AiMessage::system(q.system), AiMessage::user(q.user)];
            for attempt in 0..2 {
                let req = mv_ai::AiStepRequest { request_id: format!("anime-board-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)), provider_id: provider, model: model.clone(), messages: messages.clone(), tools: None, effort: None, access: None, images: q.images.clone(), workspace: None, lean: true, mcp: None };
                let r = reg.run_step_cancellable(req, mv_ai::emit_noop(), mv_ai::CancellationToken::new()).await;
                if let Some(e) = r.error {
                    bail!(e);
                }
                if let Some(v) = mv_recap::extract_whole_json(&r.text) {
                    return Ok(v);
                }
                if attempt == 0 {
                    messages.push(AiMessage::assistant(r.text));
                    messages.push(AiMessage::user("Your previous answer was not one complete JSON object. Reply with the whole JSON object only."));
                }
            }
            Err(anyhow!("the AI's answer was not valid JSON"))
        })
    }))
}

/// Runs `anime_comp.py <cmd>` with the VFX pack's Python and returns its last answer.
fn comp(cmd: &str, job: Value, dir: &Path) -> Result<Value> {
    let python = mv_core::paths::root().join("models").join("vfx").join("venv").join("Scripts").join("python.exe");
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("..").join("python").join("anime_comp.py");
    let work = dir.join("anim").join("work");
    std::fs::create_dir_all(&work)?;
    let file = work.join(format!("{cmd}-example.json"));
    std::fs::write(&file, serde_json::to_vec(&job)?)?;
    let out = std::process::Command::new(&python).arg(&script).arg(cmd).arg(&file).output().with_context(|| format!("could not start {}", python.display()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let last: Value = text.lines().rev().find_map(|l| serde_json::from_str::<Value>(l.trim()).ok().filter(|v| v.get("ok").is_some())).unwrap_or(Value::Null);
    if last["ok"].as_bool() != Some(true) {
        bail!("anime_comp.py {cmd} failed: {}{}", last["message"].as_str().unwrap_or(""), String::from_utf8_lossy(&out.stderr).lines().rev().take(3).collect::<Vec<_>>().join(" / "));
    }
    Ok(last)
}
