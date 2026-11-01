//! More Studio tools for the assistant: run any range of a chapter's steps (or one step), and
//! play a video (a chapter's recap, an export, a motion render) in the app's player.
use std::path::PathBuf;

use futures::FutureExt as _;
use mv_recap::Step;
use mv_ui::Route;
use serde_json::{json, Map, Value};
use ui_assistant::tools::{list_arg, str_arg, Phrases, Risk, ToolDef, ToolOutput, ToolResult};

use crate::pipeline;

/// The steps, by name, in pipeline order.
const STEPS: [&str; 6] = ["read", "transcript", "story", "audio", "music", "video"];

fn step_ix(s: Step) -> usize {
    Step::ALL.iter().position(|x| *x == s).unwrap_or(0)
}

/// `from` (optional) and `until` (default video) as steps, checked to be in order.
pub(crate) fn step_range(from: &str, until: &str) -> Result<(Option<Step>, Step), String> {
    let parse = |s: &str| super::parse_step(s).ok_or_else(|| format!("Unknown step \"{s}\" ({})", STEPS.join(", ")));
    let until = if until.trim().is_empty() { Step::Video } else { parse(until)? };
    let from = if from.trim().is_empty() { None } else { Some(parse(from)?) };
    if let Some(f) = from {
        if step_ix(f) > step_ix(until) {
            return Err(format!("\"from\" ({}) comes after \"until\" ({}); steps run Read → Transcript → Story → Audio → Music → Video", f.label(), until.label()));
        }
    }
    Ok((from, until))
}

fn range_text(a: &Map<String, Value>) -> String {
    match step_range(str_arg(a, "from"), str_arg(a, "until")) {
        Ok((Some(f), u)) if f == u => format!("the {} step", f.label()),
        Ok((Some(f), u)) => format!("{} → {}", f.label(), u.label()),
        Ok((None, u)) => format!("the steps until {}", u.label()),
        Err(_) => "steps".into(),
    }
}

async fn run_steps(a: Map<String, Value>) -> ToolResult {
    super::studio_on().await?;
    let (from, until) = step_range(str_arg(&a, "from"), str_arg(&a, "until"))?;
    let groups = super::load_groups().await?;
    let g = super::find_manga(&groups, str_arg(&a, "manga"))?;
    let chapters = super::pick_chapters(g, &list_arg(&a, "chapters"))?;
    let env = super::studio_env().await.ok_or("Could not check the Studio setup")?;
    if !env.runnable() {
        return Err("Studio needs Python and the pipeline scripts. Open Studio and finish Setup first.".into());
    }
    let e = env.clone();
    ui_assistant::on_ui(move |_, cx| {
        let ctx = pipeline::context_from_app(Some(e), cx);
        pipeline::set_context(ctx);
    })
    .await?;
    let prefs = mv_recap::Prefs::load();
    let answers = crate::generate::Answers::load(&prefs);
    let format = answers.format.clone();
    let picked = chapters.clone();
    let (new, widened, already) = mv_ui::bg::blocking(move || pipeline::enqueue(&picked, until, from, &format, None, true)).await.map_err(|e| e.to_string())?;
    let names: Vec<String> = chapters.iter().map(|c| pipeline::short_label(c.number, c.title.as_deref())).collect();
    Ok(ToolOutput::Value(json!({
        "manga": g.title,
        "chapters": names,
        "from": from.map(Step::label),
        "until": until.label(),
        "queued": new,
        "widened": widened,
        "alreadyQueued": already,
        "note": if from.is_some() { "The steps from `from` are made again (their old output is replaced); steps before it are reused. Follow with studio_status." } else { "Steps already done are skipped. Follow with studio_status." },
    })))
}

/// The video a call names: a file path, or a chapter's recap.
async fn video_for(a: &Map<String, Value>) -> Result<(String, PathBuf), String> {
    let path = str_arg(a, "path");
    if !path.is_empty() {
        let p = super::motion::local_path(path)?;
        if !p.is_file() {
            return Err(format!("{} does not exist", p.display()));
        }
        let title = p.file_stem().and_then(|s| s.to_str()).unwrap_or("Video").to_string();
        return Ok((title, p));
    }
    let groups = super::load_groups().await?;
    let g = super::find_manga(&groups, str_arg(a, "manga"))?;
    let ch = str_arg(a, "chapter");
    if ch.is_empty() {
        return Err("Give the video's path, or manga + chapter for a chapter's recap".into());
    }
    let c = super::pick_chapters(g, &[ch.to_string()])?.remove(0);
    let label = pipeline::short_label(c.number, c.title.as_deref());
    match c.video.filter(|p| p.is_file()) {
        Some(p) => Ok((format!("{} · {label}", g.title), p)),
        None => Err(format!("{label} of {} has no video yet. Make it with studio_generate.", g.title)),
    }
}

async fn play(a: Map<String, Value>) -> ToolResult {
    let (title, file) = video_for(&a).await?;
    let request = mv_ui::nav::PlayRequest { title: title.clone(), source: file.display().to_string(), file_idx: None, imdb_id: None, mal_id: None, season: None, episode: None };
    Ok(ToolOutput::Navigate(Route::Player { request }, json!({ "ok": true, "playing": title, "path": file.display().to_string() })))
}

pub(super) fn tools() -> Vec<ToolDef> {
    vec![
        ToolDef::new(
            "studio_run_steps",
            "Run a range of Studio steps for the named chapters of one manga, through the same queue as the dashboard: steps run Read → Transcript → Story → Audio → Music → Video. `until` stops after that step (e.g. until story = only make the script); `from` makes that step and the later ones (up to `until`) again, replacing their output (from = until runs a single step again). Without `from`, steps already done are skipped.",
            json!({ "type": "object", "properties": {
                "manga": { "type": "string", "description": "Manga title or id" },
                "chapters": { "type": "array", "items": { "type": "string" }, "description": "Exactly the chapters the user named: numbers (\"12\"), ranges (\"12-15\") or ids" },
                "from": { "type": "string", "enum": STEPS, "description": "Make this step again, and the ones after it up to until" },
                "until": { "type": "string", "enum": STEPS, "description": "Last step to run (default video)" },
            }, "required": ["manga", "chapters"] }),
            Risk::Act,
        )
        .risk_for(|a| if str_arg(a, "from").is_empty() { Risk::Act } else { Risk::Destructive })
        .phrases(|a| {
            let what = format!("{} for {} of {}", range_text(a), ui_assistant::chapters::describe(&list_arg(a, "chapters")), str_arg(a, "manga"));
            if str_arg(a, "from").is_empty() {
                Phrases::new("Run", "Queueing", "Queued", &what)
            } else {
                Phrases::new("Redo", "Queueing a redo of", "Queued a redo of", &what)
            }
        })
        .run(|a| async move { run_steps(a).await }.boxed()),
        ToolDef::new(
            "studio_play",
            "Play a video in the app's player: a chapter's recap (manga + chapter), or any video file by path (an export from studio_export, a motion_compose render).",
            json!({ "type": "object", "properties": {
                "manga": { "type": "string", "description": "Manga title or id (with chapter)" },
                "chapter": { "type": "string", "description": "Chapter number or id" },
                "path": { "type": "string", "description": "A video file to play instead" },
            }, "required": [] }),
            Risk::Act,
        )
        .phrases(|a| {
            let what = if !str_arg(a, "path").is_empty() {
                PathBuf::from(str_arg(a, "path")).file_name().and_then(|n| n.to_str()).unwrap_or("the video").to_string()
            } else {
                format!("Ch. {} of {}", str_arg(a, "chapter"), str_arg(a, "manga"))
            };
            Phrases::new("Play", "Opening", "Playing", &what)
        })
        .run(|a| async move { play(a).await }.boxed()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_ranges_are_checked() {
        assert_eq!(step_range("", "").unwrap(), (None, Step::Video));
        assert_eq!(step_range("", "story").unwrap(), (None, Step::Story));
        assert_eq!(step_range("audio", "audio").unwrap(), (Some(Step::Audio), Step::Audio));
        assert!(step_range("video", "read").unwrap_err().contains("comes after"));
        assert!(step_range("nope", "").is_err());
        let mut a = Map::new();
        a.insert("from".into(), json!("audio"));
        a.insert("until".into(), json!("audio"));
        assert_eq!(range_text(&a), "the Audio step");
    }

    #[test]
    fn redoing_steps_asks_first() {
        let t = tools();
        let run = t.iter().find(|t| t.name == "studio_run_steps").unwrap();
        let mut a = Map::new();
        a.insert("until".into(), json!("story"));
        assert_eq!(run.risk_for.as_ref().unwrap()(&a), Risk::Act);
        a.insert("from".into(), json!("story"));
        assert_eq!(run.risk_for.as_ref().unwrap()(&a), Risk::Destructive);
    }
}
