//! Studio's tools for the assistant (registered in `ui_assistant::tools` at startup) and its
//! proactive chat suggestions. Everything goes through what the dashboard uses: `data` for the
//! chapters, [`generate::queue_generate`] and the app-wide [`pipeline`] queue for Generate / Redo,
//! [`export::export_joined`] for the full recap, `look` and `video` for the branding and captions.
//! So the Studio page shows the assistant's work live.
//!
//! Only the chapters a call names are touched: chapter specs must each match, nothing is widened.
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use futures::FutureExt as _;
use mv_recap::{ModelChoice, Step};
use serde_json::{json, Map, Value};
use ui_assistant::chapters::{self, compress};
use ui_assistant::suggest::{self, Suggestion};
use ui_assistant::tools::{self as at, bool_arg, list_arg, num_arg, str_arg, Phrases, Risk, ToolDef, ToolOutput, ToolResult};

use crate::data::{ChapterItem, MangaGroup};
use crate::export::{self, ExportPrefs, Mode};
use crate::generate::{self, Answers};
use crate::look::{Effect, Look, Preset, POSITIONS};
use crate::pipeline::{self, ItemState, QueueItem, StepState};
use crate::setup::{self, Env};

/// Chapters one suggestion names at most (the rest go in "and N more").
const SUGGEST_CHAPTERS: usize = 12;

pub fn register() {
    at::register(tools());
    suggest::register("studio", suggestions);
    crate::ytpack::register_tools();
}

fn obj(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

fn s(d: &str) -> Value {
    json!({ "type": "string", "description": d })
}

fn chapters_schema() -> Value {
    json!({ "type": "array", "items": { "type": "string" }, "description": "Exactly the chapters the user asked for: numbers (\"12\"), ranges (\"12-15\") or chapter ids. \"all\" only when the user said all chapters." })
}

const STEP_NAMES: [&str; 5] = ["read", "story", "audio", "music", "video"];

fn tools() -> Vec<ToolDef> {
    vec![
        ToolDef::new(
            "studio_library",
            "Studio's downloaded manga. Without `manga`: every manga with how many chapters are downloaded, recapped (video made), queued or failed. With `manga`: each chapter (number, id) and which steps are done (Read, Transcript, Story, Audio, Music, Video), plus its queue state.",
            obj(json!({ "manga": s("Optional manga title or id") }), &[]),
            Risk::Read,
        )
        .phrases(|a| Phrases::new("Read", "Reading", "Read", &target(a, "the Studio library")))
        .run(|a| async move { library(a).await }.boxed()),
        ToolDef::new(
            "studio_status",
            "What Studio is doing now: the Generate queue (each chapter's state, current step, progress and error), whether it is paused, and a running full-recap export.",
            obj(json!({}), &[]),
            Risk::Read,
        )
        .phrases(|_| Phrases::new("Check", "Checking", "Checked", "Studio's progress"))
        .run(|_| async move { Ok(ToolOutput::Value(status_json())) }.boxed()),
        ToolDef::new(
            "studio_voices",
            "Narrator voices for recaps: id, name, language and whether it is installed. Pass an id as `voice` to studio_generate.",
            obj(json!({ "lang": s("Optional language code, e.g. en or hi") }), &[]),
            Risk::Read,
        )
        .phrases(|_| Phrases::new("List", "Listing", "Listed", "narrator voices"))
        .run(|a| async move { Ok(ToolOutput::Value(voices_json(str_arg(&a, "lang")))) }.boxed()),
        ToolDef::new(
            "studio_generate",
            "Make recap videos for the named chapters of one manga, through the same queue as the dashboard's Generate (Read → Transcript → Story → Audio → Music → Video, skipping steps already done). `redo_from` makes that step and every later one again (overwrites them). Optional: narrator `voice` (studio_voices), `format` landscape or reels, `captions`, `music`, and the AI `provider` + `model` for the story.",
            obj(
                json!({
                    "manga": s("Manga title or id"),
                    "chapters": chapters_schema(),
                    "redo_from": { "type": "string", "enum": STEP_NAMES, "description": "Make this step and the later ones again" },
                    "voice": s("Narrator voice id"),
                    "format": { "type": "string", "enum": ["landscape", "reels"] },
                    "captions": { "type": "boolean", "description": "Burn subtitles into the video" },
                    "music": { "type": "boolean", "description": "Compose background music" },
                    "provider": s("AI provider id for the story (e.g. ollama, anthropic); with model"),
                    "model": s("AI model id for the story"),
                }),
                &["manga", "chapters"],
            ),
            Risk::Act,
        )
        .risk_for(|a| if str_arg(a, "redo_from").is_empty() { Risk::Act } else { Risk::Destructive })
        .phrases(|a| {
            let what = format!("{} of {}", chapters_text(a), titled(a));
            if str_arg(a, "redo_from").is_empty() {
                Phrases::new("Generate recaps for", "Queueing recaps for", "Queued recaps for", &what)
            } else {
                Phrases::new(&format!("Redo from {}", str_arg(a, "redo_from")), &format!("Redoing from {}", str_arg(a, "redo_from")), &format!("Queued a redo from {}", str_arg(a, "redo_from")), &format!("for {what}"))
            }
        })
        .run(|a| async move { generate_tool(a).await }.boxed()),
        ToolDef::new(
            "studio_queue",
            "Control the Generate queue: pause, resume, cancel or retry (failed chapters restart from the step that failed) the named chapters of a manga, or the whole queue with pause_all / resume_all / cancel_all / clear_finished.",
            obj(
                json!({
                    "action": { "type": "string", "enum": ["pause", "resume", "cancel", "retry", "pause_all", "resume_all", "cancel_all", "clear_finished"] },
                    "manga": s("Manga title or id (for per-chapter actions)"),
                    "chapters": chapters_schema(),
                }),
                &["action"],
            ),
            Risk::Act,
        )
        .phrases(|a| {
            let act = str_arg(a, "action");
            let what = if act.ends_with("_all") || act == "clear_finished" { "the recap queue".to_string() } else { format!("{} of {}", chapters_text(a), titled(a)) };
            match act {
                "pause" | "pause_all" => Phrases::new("Pause", "Pausing", "Paused", &what),
                "resume" | "resume_all" => Phrases::new("Resume", "Resuming", "Resumed", &what),
                "cancel" | "cancel_all" => Phrases::new("Cancel", "Cancelling", "Cancelled", &what),
                "retry" => Phrases::new("Retry", "Retrying", "Retried", &what),
                _ => Phrases::new("Clear finished chapters from", "Clearing", "Cleared", "the recap queue"),
            }
        })
        .run(|a| async move { queue_tool(a).await }.boxed()),
        ToolDef::new(
            "studio_export",
            "Export the full recap of a manga: every made chapter video joined in order, as one video (`whole`, optionally with an AI `hook` intro), `parts`, or 9:16 `reels`, with the look's watermark. Saves into `folder` (default: Videos\\MangaVox). Fails if the files exist unless `overwrite` is true (ask the user first).",
            obj(
                json!({
                    "manga": s("Manga title or id"),
                    "kind": { "type": "string", "enum": ["whole", "parts", "reels"] },
                    "parts": { "type": "number", "description": "Number of parts (2, 3, 4, 6, 8 or 10)" },
                    "hook": { "type": "boolean", "description": "AI teaser before chapter 1 (whole only)" },
                    "hook_seconds": { "type": "number", "description": "15, 20 or 30" },
                    "music": { "type": "boolean", "description": "Keep the background music (default true)" },
                    "folder": s("Output folder (optional)"),
                    "overwrite": { "type": "boolean", "description": "Replace existing files of the same name" },
                }),
                &["manga"],
            ),
            Risk::Act,
        )
        .risk_for(|a| if bool_arg(a, "overwrite") == Some(true) { Risk::Destructive } else { Risk::Act })
        .phrases(|a| Phrases::new("Export the full recap of", "Exporting the full recap of", "Exported the full recap of", &titled(a)))
        .run(|a| async move { export_tool(a).await }.boxed()),
        ToolDef::new(
            "studio_look",
            "The video look and branding used for new renders and exports: preset (clean, cinematic, recap, dynamic), effects (grade, glow, vignette, grain, impact), text watermark and logo. Call without arguments to read it; pass fields to change them.",
            obj(
                json!({
                    "preset": { "type": "string", "enum": Preset::ALL.map(Preset::key) },
                    "effects": { "type": "object", "description": "Effect on/off, e.g. {\"grain\": false}", "properties": { "grade": {"type":"boolean"}, "glow": {"type":"boolean"}, "vignette": {"type":"boolean"}, "grain": {"type":"boolean"}, "impact": {"type":"boolean"} } },
                    "watermark": { "type": "boolean", "description": "Show the text watermark" },
                    "watermark_text": s("Watermark text, e.g. @mychannel"),
                    "watermark_position": { "type": "string", "enum": POSITIONS },
                    "logo": { "type": "boolean", "description": "Show the logo (one must be chosen in Studio first)" },
                    "logo_position": { "type": "string", "enum": POSITIONS },
                }),
                &[],
            ),
            Risk::Read,
        )
        .risk_for(|a| if a.is_empty() { Risk::Read } else { Risk::Act })
        .phrases(|a| if a.is_empty() { Phrases::new("Read", "Reading", "Read", "the video look") } else { Phrases::new("Change", "Changing", "Changed", "the video look") })
        .run(|a| async move { look_tool(a).await }.boxed()),
        ToolDef::new(
            "studio_captions",
            "Caption (subtitle) style for new renders: on/off, template, size and place. Without arguments: the current setting and the template ids. Applies to every chapter's next render (and to `chapters` of `manga` that have their own setting).",
            obj(
                json!({
                    "enabled": { "type": "boolean" },
                    "style": s("Caption template id"),
                    "size": { "type": "number", "description": "Size multiplier, 0.6 to 1.6" },
                    "place": { "type": "string", "enum": ["template", "top", "middle", "bottom"] },
                }),
                &[],
            ),
            Risk::Read,
        )
        .risk_for(|a| if a.is_empty() { Risk::Read } else { Risk::Act })
        .phrases(|a| if a.is_empty() { Phrases::new("Read", "Reading", "Read", "the caption style") } else { Phrases::new("Change", "Changing", "Changed", "the caption style") })
        .run(|a| async move { captions_tool(a).await }.boxed()),
    ]
}

fn titled(a: &Map<String, Value>) -> String {
    target(a, "the manga")
}

fn target(a: &Map<String, Value>, fallback: &str) -> String {
    let m = str_arg(a, "manga");
    if m.is_empty() || chapters::looks_like_id(m) { fallback.into() } else { m.to_string() }
}

fn chapters_text(a: &Map<String, Value>) -> String {
    chapters::describe(&list_arg(a, "chapters"))
}

// ---------------------------------------------------------------- pure helpers

/// The manga `q` names: its id, its exact title, or the one title that contains it.
pub(crate) fn find_manga<'a>(groups: &'a [MangaGroup], q: &str) -> Result<&'a MangaGroup, String> {
    let q = q.trim();
    if q.is_empty() {
        return Err("Say which manga".into());
    }
    let low = q.to_lowercase();
    if let Some(g) = groups.iter().find(|g| g.id == q || g.title.to_lowercase() == low) {
        return Ok(g);
    }
    let hits: Vec<&MangaGroup> = groups.iter().filter(|g| g.title.to_lowercase().contains(&low)).collect();
    match hits.as_slice() {
        [one] => Ok(one),
        [] => Err(format!("No downloaded manga matches \"{q}\". Studio only sees manga with downloaded chapters.")),
        many => Err(format!("\"{q}\" matches several manga: {}. Which one?", many.iter().map(|g| g.title.as_str()).collect::<Vec<_>>().join(", "))),
    }
}

/// The chapters of `g` the specs name, in reading order. Every spec must match a downloaded
/// chapter; one that does not is an error (the call names wrong chapters, so nothing runs).
pub(crate) fn pick_chapters(g: &MangaGroup, raw: &[String]) -> Result<Vec<ChapterItem>, String> {
    if raw.is_empty() {
        return Err("Say which chapters (numbers like \"12\" or \"12-15\")".into());
    }
    let items: Vec<(&str, Option<f64>)> = g.chapters.iter().map(|c| (c.id.as_str(), c.number)).collect();
    match chapters::pick(&items, raw) {
        Ok(ix) => Ok(ix.into_iter().map(|i| g.chapters[i].clone()).collect()),
        Err(missing) => {
            let have = compress(&g.chapters.iter().filter_map(|c| c.number).collect::<Vec<_>>());
            let one = missing.len() == 1;
            Err(format!(
                "{} of {} {} not downloaded (downloaded: {}). Download {} first, or pick other chapters.",
                missing.join(", "),
                g.title,
                if one { "is" } else { "are" },
                if have.is_empty() { "none".into() } else { have },
                if one { "it" } else { "them" }
            ))
        }
    }
}

pub(crate) fn parse_step(s: &str) -> Option<Step> {
    match s.trim().to_lowercase().as_str() {
        "read" | "ocr" | "pages" => Some(Step::Read),
        "transcript" => Some(Step::Transcript),
        "story" | "script" => Some(Step::Story),
        "audio" | "narration" | "voice" => Some(Step::Audio),
        "music" | "score" => Some(Step::Music),
        "video" | "render" => Some(Step::Video),
        _ => None,
    }
}

fn state_name(s: ItemState) -> &'static str {
    match s {
        ItemState::Queued => "queued",
        ItemState::Running => "running",
        ItemState::Paused => "paused",
        ItemState::Done => "done",
        ItemState::Failed => "failed",
    }
}

fn item_json(it: &QueueItem) -> Value {
    let step = it.current_step();
    let error = it.error.as_deref().map(|e| match step {
        Some(s) => crate::issues::explain(s, e, &it.chapter.label(), Some(it.id)).0,
        None => e.to_string(),
    });
    json!({
        "manga": it.chapter.manga_title,
        "chapter": crate::pipeline::short_label(it.chapter.number, it.chapter.title.as_deref()),
        "chapterId": it.chapter.id,
        "state": state_name(it.state),
        "step": step.map(Step::label),
        "until": it.until_step().label(),
        "redoFrom": it.force.map(|f| Step::ALL[f.min(5)].label()),
        "progress": (it.progress() * 100.0).round(),
        "error": error,
        "note": it.note,
    })
}

fn status_json() -> Value {
    let q = pipeline::snapshot();
    let export = EXPORT.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|(job, title)| {
        let (p, label) = job.snapshot();
        json!({ "manga": title, "progress": (p * 100.0).round(), "doing": label })
    });
    json!({
        "queuePaused": q.paused,
        "waitingForResume": q.interrupted,
        "active": q.active(),
        "chapters": q.items.iter().map(item_json).collect::<Vec<_>>(),
        "export": export,
    })
}

/// Done steps of a chapter, as names.
fn steps_done(st: &mv_recap::PhaseStatus) -> Vec<&'static str> {
    let mut v = vec![];
    if st.total_pages > 0 && st.read_pages >= st.total_pages {
        v.push("Read");
    }
    if st.transcript {
        v.push("Transcript");
    }
    if st.story {
        v.push("Story");
    }
    if st.audio_files > 0 {
        v.push("Audio");
    }
    if st.music {
        v.push("Music");
    }
    if st.video.is_some() {
        v.push("Video");
    }
    v
}

// ---------------------------------------------------------------- shared async bits

async fn load_groups() -> Result<Vec<MangaGroup>, String> {
    mv_ui::bg::blocking(crate::data::load).await.map_err(|e| e.to_string())?.map_err(|e| format!("Could not read the library: {e}"))
}

async fn studio_on() -> Result<(), String> {
    if ui_assistant::on_ui(|_, cx| mv_ui::mode::studio_enabled(cx)).await.unwrap_or(true) {
        Ok(())
    } else {
        Err("Studio is off. Turn on Studio mode in Settings → Mode first.".into())
    }
}

/// The last Studio setup check, or a fresh one when none ran yet (Studio was never opened).
async fn studio_env() -> Option<Env> {
    if let Some(c) = setup::latest() {
        return Some(c.env);
    }
    mv_ui::bg::run(setup::check()).await.ok()
}

// ---------------------------------------------------------------- tools

async fn library(a: Map<String, Value>) -> ToolResult {
    let groups = load_groups().await?;
    let q = pipeline::snapshot();
    let m = str_arg(&a, "manga").to_string();
    if m.is_empty() {
        let rows: Vec<Value> = groups
            .iter()
            .map(|g| {
                let in_queue = |s: fn(ItemState) -> bool| g.chapters.iter().filter(|c| q.for_chapter(&c.id).is_some_and(|i| s(i.state))).count();
                json!({
                    "manga": g.title,
                    "id": g.id,
                    "downloaded": compress(&g.chapters.iter().filter_map(|c| c.number).collect::<Vec<_>>()),
                    "chapters": g.chapters.len(),
                    "recapped": g.chapters.iter().filter(|c| c.video.is_some()).count(),
                    "queued": in_queue(|s| s.active()),
                    "failed": in_queue(|s| s == ItemState::Failed),
                })
            })
            .collect();
        return Ok(ToolOutput::Value(json!({ "manga": rows })));
    }
    let g = find_manga(&groups, &m)?.clone();
    let chapters = g.chapters.clone();
    let rows = mv_ui::bg::blocking(move || {
        chapters
            .iter()
            .map(|c| {
                let st = mv_recap::chapter_status(&crate::page::chref(c));
                (c.clone(), st)
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| e.to_string())?;
    let list: Vec<Value> = rows
        .iter()
        .map(|(c, st)| {
            let item = q.for_chapter(&c.id);
            json!({
                "chapter": crate::pipeline::short_label(c.number, c.title.as_deref()),
                "number": c.number,
                "id": c.id,
                "pages": c.page_count,
                "done": steps_done(st),
                "video": st.video.is_some(),
                "queue": item.map(|i| state_name(i.state)),
                "error": item.filter(|i| i.state == ItemState::Failed).and_then(|i| i.error.clone()),
            })
        })
        .collect();
    Ok(ToolOutput::Value(json!({ "manga": g.title, "id": g.id, "chapters": list })))
}

fn voices_json(lang: &str) -> Value {
    let default = mv_studio::voices::default_voice();
    let list: Vec<Value> = mv_studio::tts::VOICES
        .iter()
        .filter(|v| lang.is_empty() || v.lang == lang)
        .map(|v| {
            let installed = mv_studio::voices::pack_for_voice(v.id).map(mv_studio::voices::installed).unwrap_or(true);
            json!({ "id": v.id, "name": v.label, "about": v.desc, "lang": v.lang, "installed": installed, "default": v.id == default })
        })
        .collect();
    json!({ "voices": list })
}

async fn generate_tool(a: Map<String, Value>) -> ToolResult {
    studio_on().await?;
    let groups = load_groups().await?;
    let g = find_manga(&groups, str_arg(&a, "manga"))?;
    let chapters = pick_chapters(g, &list_arg(&a, "chapters"))?;
    let redo = match str_arg(&a, "redo_from") {
        "" => None,
        s => Some(parse_step(s).ok_or_else(|| format!("Unknown step \"{s}\" (read, story, audio, music or video)"))?),
    };
    let voice = str_arg(&a, "voice").to_string();
    if !voice.is_empty() && mv_studio::tts::find_voice(&voice).is_none() {
        return Err(format!("Unknown voice \"{voice}\". Use studio_voices for the ids."));
    }
    let env = studio_env().await.ok_or("Could not check the Studio setup")?;
    if !env.runnable() {
        return Err("Studio needs Python and the pipeline scripts. Open Studio and finish Setup first.".into());
    }
    let model = match (str_arg(&a, "provider"), str_arg(&a, "model")) {
        ("", "") => None,
        (p, m) if !p.is_empty() && !m.is_empty() => {
            let provider = serde_json::from_value::<mv_ai::AiProviderId>(Value::String(p.to_string())).map_err(|_| format!("Unknown AI provider \"{p}\""))?;
            Some((ModelChoice { provider, model: m.to_string() }, m.to_string()))
        }
        _ => return Err("Give both provider and model, or neither (the assistant's model is used)".into()),
    };

    let prefs = mv_recap::Prefs::load();
    let mut answers = Answers::load(&prefs);
    if !voice.is_empty() {
        answers.voice = Some(voice);
    }
    match str_arg(&a, "format") {
        "" => {}
        "reels" => answers.format = mv_recap::FORMAT_REELS.into(),
        _ => answers.format = mv_recap::FORMAT_LANDSCAPE.into(),
    }
    if let Some(c) = bool_arg(&a, "captions") {
        answers.captions = c;
    }
    if let Some(m) = bool_arg(&a, "music") {
        answers.music = m;
    }
    // Same context the dashboard hands over (the page re-syncs its own on its next Generate).
    let e = env.clone();
    ui_assistant::on_ui(move |_, cx| {
        let mut ctx = pipeline::context_from_app(Some(e), cx);
        if model.is_some() {
            ctx.model = model;
        }
        pipeline::set_context(ctx);
    })
    .await?;
    let default = prefs.voice.clone().unwrap_or_default();
    let picked = chapters.clone();
    let ((new, widened, already), errors) = mv_ui::bg::blocking(move || generate::queue_generate(&picked, &answers, redo, true, &default, |c| c.video.is_some())).await.map_err(|e| e.to_string())?;
    let names: Vec<String> = chapters.iter().map(|c| crate::pipeline::short_label(c.number, c.title.as_deref())).collect();
    ui_assistant::on_ui(move |_, cx| {
        mv_ui::toast::show(cx, mv_ui::toast::Toast::success(format!("Assistant queued {} chapter{} for recaps", new + widened, if new + widened == 1 { "" } else { "s" })).icon(mv_ui::ph::ROCKET_LAUNCH));
    })
    .await
    .ok();
    Ok(ToolOutput::Value(json!({
        "manga": g.title,
        "chapters": names,
        "queued": new,
        "widened": widened,
        "alreadyQueued": already,
        "redoFrom": redo.map(Step::label),
        "warnings": errors,
        "note": if env.ffmpeg { "Watch progress with studio_status." } else { "FFmpeg is missing: narration works, the Video step will fail until FFmpeg is installed." },
    })))
}

async fn queue_tool(a: Map<String, Value>) -> ToolResult {
    let action = str_arg(&a, "action").to_string();
    match action.as_str() {
        "pause_all" => pipeline::pause_all(),
        "resume_all" => pipeline::resume_all(),
        "cancel_all" => pipeline::cancel_all(),
        "clear_finished" => pipeline::clear_finished(),
        "pause" | "resume" | "cancel" | "retry" => {
            let groups = load_groups().await?;
            let g = find_manga(&groups, str_arg(&a, "manga"))?;
            let chapters = pick_chapters(g, &list_arg(&a, "chapters"))?;
            let q = pipeline::snapshot();
            let mut done = vec![];
            let mut skipped = vec![];
            for c in &chapters {
                let label = crate::pipeline::short_label(c.number, c.title.as_deref());
                let Some(it) = q.for_chapter(&c.id) else {
                    skipped.push(format!("{label}: not in the queue"));
                    continue;
                };
                let ok = match action.as_str() {
                    "pause" if matches!(it.state, ItemState::Queued | ItemState::Running) => {
                        pipeline::pause(it.id);
                        true
                    }
                    "resume" if it.state == ItemState::Paused => {
                        pipeline::resume(it.id);
                        true
                    }
                    "retry" if it.state == ItemState::Failed => {
                        pipeline::resume(it.id);
                        true
                    }
                    "cancel" if it.state.active() => {
                        pipeline::cancel(it.id);
                        true
                    }
                    _ => false,
                };
                if ok {
                    done.push(label);
                } else {
                    skipped.push(format!("{label}: {}", state_name(it.state)));
                }
            }
            return Ok(ToolOutput::Value(json!({ "action": action, "chapters": done, "skipped": skipped, "status": status_json() })));
        }
        other => return Err(format!("Unknown action \"{other}\"")),
    }
    Ok(ToolOutput::Value(json!({ "action": action, "status": status_json() })))
}

/// The running assistant export and its manga's title (one at a time).
static EXPORT: Mutex<Option<(export::Job, String)>> = Mutex::new(None);

fn default_export_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join("Videos").join("MangaVox")
}

/// Export choices from the call's args. `Err` when they do not fit.
pub(crate) fn export_prefs(a: &Map<String, Value>) -> Result<ExportPrefs, String> {
    let mut p = ExportPrefs::default();
    p.kind = match str_arg(a, "kind") {
        "" | "whole" | "one" => 0,
        "parts" => 1,
        "reels" => 2,
        other => return Err(format!("Unknown kind \"{other}\" (whole, parts or reels)")),
    };
    if let Some(n) = num_arg(a, "parts") {
        let n = n.round() as usize;
        p.parts = *export::PART_COUNTS.iter().min_by_key(|c| c.abs_diff(n)).unwrap_or(&6);
    }
    p.hook = bool_arg(a, "hook").unwrap_or(false) && p.kind == 0;
    if let Some(s) = num_arg(a, "hook_seconds") {
        p.hook_secs = *export::HOOK_SECONDS.iter().min_by_key(|c| c.abs_diff(s.round() as u32)).unwrap_or(&20);
    }
    p.music = bool_arg(a, "music").unwrap_or(true);
    Ok(p)
}

/// Files in `dir` that an export named `name` would replace.
pub(crate) fn existing_exports(dir: &std::path::Path, name: &str) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|rd| rd.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|f| f.starts_with(name) && f.ends_with(".mp4")).collect())
        .unwrap_or_default()
}

async fn export_tool(a: Map<String, Value>) -> ToolResult {
    studio_on().await?;
    let prefs = export_prefs(&a)?;
    let groups = load_groups().await?;
    let g = find_manga(&groups, str_arg(&a, "manga"))?.clone();
    let made: Vec<ChapterItem> = g.chapters.iter().filter(|c| c.video.as_ref().is_some_and(|p| p.is_file())).cloned().collect();
    if made.is_empty() {
        return Err(format!("No chapter of {} has a video yet. Generate recaps first.", g.title));
    }
    let dir = match str_arg(&a, "folder") {
        "" => default_export_dir(),
        f => PathBuf::from(f),
    };
    let name = export::export_name(&g.title, &made);
    let existing = existing_exports(&dir, &name);
    if !existing.is_empty() && bool_arg(&a, "overwrite") != Some(true) {
        return Err(format!("{} already has {}. Ask the user whether to replace them, then call again with overwrite: true (or pick another folder).", dir.display(), existing.join(", ")));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let env = studio_env().await;
    let font = export::mark_font(env.as_ref());
    let job = export::Job::new(&g.id, "Joining the chapters");
    let r = job.reporter();
    {
        let mut slot = EXPORT.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            return Err("An export is already running. Wait for it (studio_status shows its progress).".into());
        }
        *slot = Some((job, g.title.clone()));
    }
    let res = run_export(g.clone(), made.clone(), prefs, dir.clone(), name, font, env, r).await;
    *EXPORT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let (files, hook_note) = res?;
    let shown = files.first().cloned().unwrap_or_else(|| dir.clone());
    let title = g.title.clone();
    let n = files.len();
    ui_assistant::on_ui(move |_, cx| {
        let what = if n == 1 { "the full recap".to_string() } else { format!("{n} files") };
        mv_ui::toast::show(cx, mv_ui::toast::Toast::success(format!("Exported {what} of {title}")).icon(mv_ui::ph::CHECK_CIRCLE).action("Show in folder", move |_, cx| cx.reveal_path(&shown)));
    })
    .await
    .ok();
    Ok(ToolOutput::Value(json!({
        "manga": g.title,
        "chapters": made.len(),
        "files": files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "note": hook_note.trim_start_matches(" · "),
    })))
}

#[allow(clippy::too_many_arguments)]
async fn run_export(g: MangaGroup, made: Vec<ChapterItem>, prefs: ExportPrefs, dir: PathBuf, name: String, font: Option<PathBuf>, env: Option<Env>, r: export::Reporter) -> Result<(Vec<PathBuf>, String), String> {
    let files: Vec<PathBuf> = made.iter().filter_map(|c| c.video.clone()).collect();
    let out = export::joined_path(&g.id);
    let rr = r.clone();
    let joined = mv_ui::bg::blocking(move || export::join(&files, &out, &rr)).await.map_err(|e| e.to_string())?.map_err(|e| format!("Could not join the chapter videos: {e:#}"))?;
    let hook = if prefs.mode() == Mode::Whole && prefs.hook {
        let Some((python, layout)) = env.and_then(|e| Some((e.python.ok()?, e.scripts.ok()?))) else {
            return Err("The hook is voiced by Studio's Python, which is not set up yet. Finish Setup, or export without the hook.".into());
        };
        let (reg, model) = ui_assistant::on_ui(|_, cx| {
            let (provider, model) = ui_assistant::assistant_model(cx);
            let model = serde_json::from_value::<mv_ai::AiProviderId>(Value::String(provider)).ok().map(|provider| ModelChoice { provider, model });
            (ui_assistant::registry(cx), model)
        })
        .await?;
        Some(export::HookJob { manga_id: g.id.clone(), manga_title: g.title.clone(), cover: g.cover.clone().filter(|p| p.is_file()), chapters: made.clone(), seconds: prefs.hook_secs, model, reg, python, layout })
    } else {
        None
    };
    // The hook needs the GPUI background executor: the rest runs on the UI thread's executor.
    let (tx, rx) = futures::channel::oneshot::channel();
    ui_assistant::on_ui(move |_, cx| {
        let ex = cx.background_executor().clone();
        cx.spawn(async move |_| {
            let out = export::export_joined(joined, hook, prefs, dir, name, font, r, ex).await;
            let _ = tx.send(out);
        })
        .detach();
    })
    .await?;
    let (res, note) = rx.await.map_err(|_| "The export stopped".to_string())?;
    res.map(|f| (f, note)).map_err(|e| format!("Could not export: {e:#}"))
}

/// Bumped when the assistant changes the look, so an open Look panel reloads it.
async fn look_tool(a: Map<String, Value>) -> ToolResult {
    let mut look = Look::load();
    if !a.is_empty() {
        apply_look(&mut look, &a)?;
        look.save().map_err(|e| format!("Could not save the look: {e}"))?;
    }
    Ok(ToolOutput::Value(look_json(&look)))
}

pub(crate) fn apply_look(look: &mut Look, a: &Map<String, Value>) -> Result<(), String> {
    if let Some(p) = Some(str_arg(a, "preset")).filter(|p| !p.is_empty()) {
        look.set_preset(Preset::parse(p).ok_or_else(|| format!("Unknown preset \"{p}\""))?);
    }
    if let Some(Value::Object(fx)) = a.get("effects") {
        for (k, v) in fx {
            let e = Effect::ALL.into_iter().find(|e| e.key() == k).ok_or_else(|| format!("Unknown effect \"{k}\""))?;
            look.set_effect(e, v.as_bool().unwrap_or(false));
        }
    }
    if let Some(on) = bool_arg(a, "watermark") {
        look.text.on = on;
    }
    if let Some(t) = a.get("watermark_text").and_then(Value::as_str) {
        look.text.text = t.trim().chars().take(60).collect();
        if !look.text.text.is_empty() && bool_arg(a, "watermark").is_none() {
            look.text.on = true;
        }
    }
    let pos = |k: &str| -> Result<Option<usize>, String> {
        match str_arg(a, k) {
            "" => Ok(None),
            p => POSITIONS.iter().position(|x| *x == p).map(Some).ok_or_else(|| format!("Unknown position \"{p}\"")),
        }
    };
    if let Some(p) = pos("watermark_position")? {
        look.text.pos = p;
    }
    if let Some(on) = bool_arg(a, "logo") {
        if on && look.logo.path.is_none() {
            return Err("No logo is chosen yet. Pick one in Studio → Video → Look & branding first.".into());
        }
        look.logo.on = on;
    }
    if let Some(p) = pos("logo_position")? {
        look.logo.pos = p;
    }
    Ok(())
}

fn look_json(l: &Look) -> Value {
    let fx = l.effects();
    json!({
        "preset": l.preset.key(),
        "effects": Effect::ALL.iter().map(|e| (e.key().to_string(), Value::Bool(fx.get(*e)))).collect::<Map<String, Value>>(),
        "watermark": { "on": l.text.on, "text": l.text.text, "position": POSITIONS[l.text.pos.min(8)] },
        "logo": { "on": l.logo.on, "chosen": l.logo.path.is_some(), "position": POSITIONS[l.logo.pos.min(8)] },
        "presets": Preset::ALL.map(Preset::key),
    })
}

async fn captions_tool(a: Map<String, Value>) -> ToolResult {
    let mut d = crate::video::default_setting();
    if !a.is_empty() {
        if let Some(on) = bool_arg(&a, "enabled") {
            d.enabled = on;
        }
        if let Some(st) = Some(str_arg(&a, "style")).filter(|s| !s.is_empty()) {
            if !crate::video::styles().iter().any(|x| x.id == st) {
                return Err(format!("Unknown caption style \"{st}\""));
            }
            d.style = st.to_string();
        }
        if let Some(sz) = num_arg(&a, "size") {
            crate::restyle::set_size(&mut d, sz.clamp(0.6, 1.6) as f32);
        }
        match str_arg(&a, "place") {
            "" => {}
            p => {
                let ix = ["template", "top", "middle", "bottom"].iter().position(|x| *x == p).ok_or_else(|| format!("Unknown place \"{p}\""))?;
                crate::restyle::set_place(&mut d, ix);
            }
        }
        crate::video::save_default(&d);
    }
    Ok(ToolOutput::Value(json!({
        "enabled": d.enabled,
        "style": d.style,
        "size": d.size,
        "styles": crate::video::styles().iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
        "note": "Applies to the next render of each chapter; re-make made videos with studio_generate redo_from video.",
    })))
}

// ---------------------------------------------------------------- suggestions

static CACHE: Mutex<Option<(Instant, u64, Vec<Suggestion>)>> = Mutex::new(None);

/// Proactive cards: failed chapters, chapters without a recap next to recapped ones, finished
/// runs to export, recapped manga without a YouTube title. Cached for a minute (the library
/// scan reads every chapter folder) unless the queue changed.
fn suggestions() -> Vec<Suggestion> {
    let v = pipeline::version();
    if let Some((at, ver, list)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        if *ver == v && at.elapsed() < Duration::from_secs(60) {
            return list.clone();
        }
    }
    let groups = crate::data::load().unwrap_or_default();
    let q = pipeline::snapshot();
    let titled: Vec<String> = groups.iter().filter(|g| crate::ytpack::has_title(&g.id)).map(|g| g.id.clone()).collect();
    let list = suggestions_from(&groups, &q, &titled);
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), v, list.clone()));
    list
}

pub(crate) fn suggestions_from(groups: &[MangaGroup], q: &pipeline::Queue, titled: &[String]) -> Vec<Suggestion> {
    let mut out = vec![];
    for it in q.items.iter().filter(|i| i.state == ItemState::Failed) {
        let ch = crate::pipeline::short_label(it.chapter.number, it.chapter.title.as_deref());
        let step = it.current_step().map(Step::label).unwrap_or("a step");
        let failed_step = it.steps.iter().position(|s| *s == StepState::Failed).map(|i| Step::ALL[i].label()).unwrap_or(step);
        out.push(Suggestion::new(
            format!("studio-failed-{}", it.chapter.id),
            format!("{ch} of {} failed at {failed_step}: retry?", it.chapter.manga_title),
            it.error.clone().unwrap_or_default().lines().next().unwrap_or("").chars().take(120).collect::<String>(),
            format!("Retry {ch} of {} in Studio (it failed at {failed_step}).", it.chapter.manga_title),
        ));
    }
    for g in groups {
        let recapped = g.chapters.iter().filter(|c| c.video.is_some()).count();
        let busy = |c: &ChapterItem| q.for_chapter(&c.id).is_some_and(|i| i.state.active() || i.state == ItemState::Failed);
        let missing: Vec<&ChapterItem> = g.chapters.iter().filter(|c| !c.has_output() && !busy(c)).collect();
        // Only for manga the user already recaps: new downloads of those are the likely next job.
        if recapped > 0 && !missing.is_empty() {
            let nums: Vec<f64> = missing.iter().filter_map(|c| c.number).take(SUGGEST_CHAPTERS).collect();
            let more = missing.len().saturating_sub(nums.len());
            let list = compress(&nums);
            let named = if more > 0 { format!("Ch. {list} (and {more} more)") } else { format!("Ch. {list}") };
            out.push(Suggestion::new(
                format!("studio-new-{}-{}", g.id, missing.len()),
                format!("{} chapter{} of {} without a recap: generate?", missing.len(), if missing.len() == 1 { "" } else { "s" }, g.title),
                named.clone(),
                format!("Generate recaps for {named} of {}.", g.title),
            ));
        }
        let finished = g.chapters.iter().any(|c| q.for_chapter(&c.id).is_some_and(|i| i.state == ItemState::Done));
        let active = g.chapters.iter().any(|c| q.for_chapter(&c.id).is_some_and(|i| i.state.active()));
        if finished && !active && recapped > 0 {
            out.push(Suggestion::new(
                format!("studio-export-{}-{recapped}", g.id),
                format!("{} recap{} of {} ready: export the full recap?", recapped, if recapped == 1 { "" } else { "s" }, g.title),
                "Every made chapter joined in order, with your watermark",
                format!("Export the full recap of {} as one video.", g.title),
            ));
        }
        if recapped >= 3 && !titled.contains(&g.id) {
            out.push(Suggestion::new(
                format!("studio-yt-{}", g.id),
                format!("Suggested title for {}?", g.title),
                "Research YouTube and write a title, description and thumbnail idea",
                format!("Make the YouTube package for {}: research it and suggest a title, description and thumbnail.", g.title),
            ));
        }
    }
    out.truncate(6);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ch(id: &str, n: f64, video: bool) -> ChapterItem {
        ChapterItem {
            id: id.into(),
            manga_id: "m".into(),
            manga_title: "Solo".into(),
            number: Some(n),
            title: None,
            pages_dir: PathBuf::new(),
            output_dir: PathBuf::new(),
            page_count: 10,
            has_text: false,
            video: video.then(|| PathBuf::from("v.mp4")),
            audio_files: 0,
        }
    }

    fn group() -> MangaGroup {
        MangaGroup { id: "m".into(), title: "Solo Leveling".into(), cover: None, chapters: vec![ch("a", 1., true), ch("b", 2., false), ch("c", 3., false), ch("d", 4.5, false), ch("e", 7., false)] }
    }

    #[test]
    fn manga_is_found_by_id_title_or_unique_part() {
        let gs = vec![group(), MangaGroup { id: "n".into(), title: "Solo Max".into(), cover: None, chapters: vec![] }];
        assert_eq!(find_manga(&gs, "m").unwrap().id, "m");
        assert_eq!(find_manga(&gs, "solo leveling").unwrap().id, "m");
        assert_eq!(find_manga(&gs, "max").unwrap().id, "n");
        assert!(find_manga(&gs, "solo").unwrap_err().contains("several"));
        assert!(find_manga(&gs, "naruto").is_err());
        assert!(find_manga(&gs, " ").is_err());
    }

    #[test]
    fn only_the_named_chapters_are_picked() {
        let g = group();
        let ids = |v: Vec<ChapterItem>| v.into_iter().map(|c| c.id).collect::<Vec<_>>();
        assert_eq!(ids(pick_chapters(&g, &["2".into(), "Ch. 3".into()]).unwrap()), ["b", "c"]);
        assert_eq!(ids(pick_chapters(&g, &["2-4.5".into()]).unwrap()), ["b", "c", "d"]);
        assert_eq!(ids(pick_chapters(&g, &["e".into()]).unwrap()), ["e"]);
        assert_eq!(ids(pick_chapters(&g, &["all".into()]).unwrap()).len(), 5);
        // One wrong chapter stops the whole call: nothing is widened or guessed.
        let err = pick_chapters(&g, &["2".into(), "5".into()]).unwrap_err();
        assert!(err.contains("5 of Solo Leveling is not downloaded"), "{err}");
        assert!(err.contains("1-3, 4.5, 7"), "{err}");
        assert!(pick_chapters(&g, &[]).is_err());
    }

    #[test]
    fn steps_parse() {
        assert_eq!(parse_step("Audio"), Some(Step::Audio));
        assert_eq!(parse_step("render"), Some(Step::Video));
        assert_eq!(parse_step("nope"), None);
    }

    #[test]
    fn export_choices_snap_to_what_the_card_offers() {
        let mut a = Map::new();
        assert_eq!(export_prefs(&a).unwrap().mode(), Mode::Whole);
        a.insert("kind".into(), json!("parts"));
        a.insert("parts".into(), json!(5));
        a.insert("hook".into(), json!(true));
        let p = export_prefs(&a).unwrap();
        assert_eq!(p.mode(), Mode::Parts(4));
        assert!(!p.hook, "the hook is for one video only");
        a.insert("kind".into(), json!("whole"));
        a.insert("hook_seconds".into(), json!(28));
        let p = export_prefs(&a).unwrap();
        assert!(p.hook && p.hook_secs == 30);
        a.insert("kind".into(), json!("gif"));
        assert!(export_prefs(&a).is_err());
    }

    #[test]
    fn existing_exports_are_found() {
        let dir = std::env::temp_dir().join(format!("mv-assist-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Solo - Ch. 1 to Ch. 3.mp4"), b"x").unwrap();
        std::fs::write(dir.join("Other.mp4"), b"x").unwrap();
        assert_eq!(existing_exports(&dir, "Solo - Ch. 1 to Ch. 3"), ["Solo - Ch. 1 to Ch. 3.mp4"]);
        assert!(existing_exports(&dir, "Nope").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn look_changes_apply_and_bad_values_fail() {
        let mut l = Look::default();
        let mut a = Map::new();
        a.insert("preset".into(), json!("cinematic"));
        a.insert("watermark_text".into(), json!("@mychannel"));
        a.insert("effects".into(), json!({ "grain": false }));
        a.insert("watermark_position".into(), json!("br"));
        apply_look(&mut l, &a).unwrap();
        assert_eq!(l.preset, Preset::Cinematic);
        assert!(l.text.on && l.text.text == "@mychannel" && l.text.pos == 8);
        assert!(!l.effects().grain);
        let mut b = Map::new();
        b.insert("logo".into(), json!(true));
        assert!(apply_look(&mut l, &b).unwrap_err().contains("No logo"));
        let mut c = Map::new();
        c.insert("preset".into(), json!("neon"));
        assert!(apply_look(&mut l, &c).is_err());
    }

    #[test]
    fn suggestions_name_exact_chapters() {
        let g = group();
        let mut q = pipeline::Queue::default();
        q.add(crate::pipeline::QChapter::from_item(&g.chapters[1]), Step::Video, None, "landscape", None);
        q.items[0].state = ItemState::Failed;
        q.items[0].steps[3] = StepState::Failed;
        q.items[0].error = Some("tts crashed".into());
        let s = suggestions_from(&[g.clone()], &q, &[]);
        assert!(s[0].title.contains("Ch. 2 of Solo failed at Audio"), "{:?}", s[0]);
        assert!(s[0].prompt.starts_with("Retry Ch. 2 of Solo"));
        // Ch. 2 is failed (shown above), Ch. 1 is made: the rest have no recap.
        let new = s.iter().find(|x| x.id.starts_with("studio-new")).unwrap();
        assert_eq!(new.prompt, "Generate recaps for Ch. 3, 4.5, 7 of Solo Leveling.");
        // No recapped chapter: no "generate" nudge for a manga the user never recaps.
        let fresh = MangaGroup { chapters: vec![ch("x", 1., false)], ..group() };
        assert!(suggestions_from(&[fresh], &pipeline::Queue::default(), &[]).is_empty());
    }
}
