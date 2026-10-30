//! The assistant's `studio_youtube_package` tool: the same research and AI suggestion as the
//! dashboard's YouTube package card, saved to the same file, and the open card refreshed.
use futures::FutureExt as _;
use serde_json::{json, Map, Value};
use ui_assistant::tools::{self as at, bool_arg, str_arg, Phrases, Risk, ToolDef, ToolOutput, ToolResult};

use super::{ask, logic, research, Panels, Saved, CAST_FOR_MATCH, CAST_FOR_PROMPT};

pub(crate) fn register_tools() {
    at::register(vec![ToolDef::new(
        "studio_youtube_package",
        "The YouTube package for a manga's recap: searches YouTube for recaps of the same manga (views, channels, a views prediction), then has the AI write a title, alternative titles, description and thumbnail concept, plus a thumbnail prompt for an image AI. Saved on the Studio dashboard's YouTube package card. `research: false` reuses the last search; `suggest: false` only researches.",
        json!({ "type": "object", "properties": {
            "manga": { "type": "string", "description": "Manga title or id" },
            "research": { "type": "boolean", "description": "Search YouTube again (default: only when never done)" },
            "suggest": { "type": "boolean", "description": "Write title / description / thumbnail (default true)" },
        }, "required": ["manga"] }),
        Risk::Act,
    )
    .phrases(|a| Phrases::new("Make the YouTube package for", "Making the YouTube package for", "Made the YouTube package for", str_arg(a, "manga")))
    .run(|a| async move { package(a).await }.boxed())]);
}

/// The manga has an AI title saved (so no "suggest a title" card for it).
pub(crate) fn has_title(manga_id: &str) -> bool {
    Saved::load(manga_id).suggestion.is_some_and(|s| !s.title.trim().is_empty())
}

async fn package(a: Map<String, Value>) -> ToolResult {
    let groups = mv_ui::bg::blocking(crate::data::load).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
    let g = crate::assist::find_manga(&groups, str_arg(&a, "manga"))?.clone();
    let others: Vec<String> = groups.iter().filter(|o| o.id != g.id).map(|o| o.title.clone()).collect();
    let (id, title) = (g.id.clone(), g.title.clone());
    let brain = mv_ui::bg::blocking(move || crate::run::brain_snapshot(&id, &title)).await.map_err(|e| e.to_string())?;
    let cast_all = logic::lead_cast(&brain, CAST_FOR_MATCH);
    let cast_names: Vec<String> = cast_all.iter().flat_map(|(c, _)| std::iter::once(c.name.clone()).chain(c.aliases.iter().cloned())).collect();
    let cast: Vec<(logic::Cast, Option<std::path::PathBuf>)> = cast_all.into_iter().take(CAST_FOR_PROMPT).collect();
    let story = logic::story_so_far(&brain.notes, 4);

    let mut saved = Saved::load(&g.id);
    let mut notes: Vec<String> = vec![];
    if bool_arg(&a, "research").unwrap_or(saved.researched_at == 0) {
        match research(g.title.clone(), others, cast_names).await {
            Ok(mut s) => {
                s.suggestion = saved.suggestion.take();
                saved = s;
                saved.save(&g.id);
            }
            Err(e) if saved.researched_at > 0 => notes.push(format!("YouTube research failed ({e:#}); used the last one")),
            Err(e) => notes.push(format!("YouTube research failed: {e:#}")),
        }
    }
    let comparable = saved.comparable();
    let prediction = saved.prediction.clone().unwrap_or_else(|| logic::predict(&comparable, &saved.reference_recent, &saved.reference_name));
    if bool_arg(&a, "suggest").unwrap_or(true) {
        let (reg, (provider, model)) = ui_assistant::on_ui(|_, cx| (ui_assistant::registry(cx), ui_assistant::assistant_model(cx))).await?;
        let provider = serde_json::from_value::<mv_ai::AiProviderId>(Value::String(provider)).map_err(|_| "Pick an AI model first".to_string())?;
        let just_cast: Vec<logic::Cast> = cast.iter().map(|(c, _)| c.clone()).collect();
        let b = logic::Brief {
            title: &g.title,
            aliases: saved.names.get(1..).unwrap_or(&[]),
            cast: &just_cast,
            story: &story,
            comparable: &comparable,
            reference: &saved.reference_recent,
            reference_name: &saved.reference_name,
            prediction: &prediction,
        };
        let (system, user) = logic::suggest_prompt(&b);
        match ask(reg, provider, model, system, user).await {
            Ok(s) => {
                saved.suggestion = Some(s);
                saved.save(&g.id);
            }
            Err(e) => notes.push(format!("The AI could not write suggestions: {e:#}")),
        }
    }
    // The dashboard card (if it was opened) shows the new package at once.
    let id = g.id.clone();
    ui_assistant::on_ui(move |_, cx| {
        if let Some(p) = cx.try_global::<Panels>().and_then(|p| p.0.get(&id).cloned()) {
            p.update(cx, |p, cx| {
                p.saved = Saved::load(&id);
                p.open = true;
                cx.notify();
            });
        }
    })
    .await
    .ok();

    let just_cast: Vec<logic::Cast> = cast.iter().map(|(c, _)| c.clone()).collect();
    let refs: Vec<bool> = cast.iter().map(|(_, f)| f.is_some()).collect();
    let hook = saved.suggestion.as_ref().map(|s| s.thumbnail_text.clone()).unwrap_or_default();
    let thumb_prompt = logic::thumbnail_prompt(&g.title, &just_cast, &refs, &story, &hook);
    Ok(ToolOutput::Value(json!({
        "manga": g.title,
        "suggestion": saved.suggestion,
        "prediction": { "low": prediction.low, "high": prediction.high, "median": prediction.median, "confidence": prediction.confidence, "why": prediction.reasoning },
        "comparable": comparable.iter().take(5).map(|v| json!({ "title": v.title, "views": v.views, "channel": v.channel, "published": v.published })).collect::<Vec<_>>(),
        "thumbnailPrompt": thumb_prompt,
        "notes": notes,
    })))
}
