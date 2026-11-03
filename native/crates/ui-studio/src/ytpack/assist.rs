//! The assistant's `studio_youtube_package` tool: the same research and AI suggestion as the
//! dashboard's YouTube package card, saved to the same file, and the open card refreshed.
use futures::FutureExt as _;
use serde_json::{json, Map, Value};
use ui_assistant::tools::{self as at, bool_arg, num_arg, str_arg, Phrases, Risk, ToolDef, ToolOutput, ToolResult};

use super::{ask, logic, magnific, research, Panels, Saved, CAST_FOR_MATCH, CAST_FOR_PROMPT};

pub(crate) fn register_tools() {
    // Not a tool, but registered at the same startup call: the reader asks Studio for a manga's
    // one AI video thumbnail through it (`mv_recap::thumb`).
    super::cover_thumb::register();
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
    at::register(thumbnail_tools());
}

// ---------------------------------------------------------------- thumbnails

fn thumbnail_tools() -> Vec<ToolDef> {
    vec![
        ToolDef::new(
            "studio_thumbnails",
            "A manga's YouTube thumbnails (Thumbnails page): the ones made (id, label, prompt, file), the AI's thumbnail ideas, the image model, size and the credits one image costs, whether a Magnific key is set and the credits spent this month.",
            json!({ "type": "object", "properties": { "manga": { "type": "string", "description": "Manga title or id" } }, "required": ["manga"] }),
            Risk::Read,
        )
        .phrases(|a| Phrases::new("Read", "Reading", "Read", &format!("the thumbnails of {}", str_arg(a, "manga"))))
        .run(|a| async move { thumbnails(a).await }.boxed()),
        ToolDef::new(
            "studio_thumbnail_make",
            "Make a YouTube thumbnail with Magnific's image AI. Spends Magnific API credits (studio_thumbnails says how many; always asks the user). `prompt` describes the image (use @Name for a character to attach their face card); without it, idea number `concept` from studio_thumbnails, else the YouTube package's thumbnail prompt.",
            json!({ "type": "object", "properties": {
                "manga": { "type": "string", "description": "Manga title or id" },
                "prompt": { "type": "string", "description": "What the thumbnail shows" },
                "concept": { "type": "number", "description": "Use the AI idea with this number (1-based)" },
                "aspect": { "type": "string", "enum": ["16:9", "9:16", "1:1", "4:3"], "description": "Default: the Thumbnails page's choice" },
            }, "required": ["manga"] }),
            Risk::Destructive,
        )
        .phrases(|a| {
            // The same choice the run makes: a prompt naming characters (@Name) may switch model.
            let prompt = str_arg(a, "prompt");
            let refs: &[bool] = if !prompt.is_empty() {
                if prompt.contains('@') { &[true] } else { &[false] }
            } else if num_arg(a, "concept").is_some() {
                // The idea's prompt is only read when it runs: either way.
                &[false, true]
            } else {
                // The package prompt names no @characters.
                &[false]
            };
            let costs: Vec<f64> = refs.iter().map(|r| {
                let (model, res, _) = final_model(str_arg(a, "aspect"), *r);
                model.cost_at(&res)
            }).collect();
            let what = format!("a thumbnail for {}{}", str_arg(a, "manga"), credits_note(&costs));
            Phrases::new("Spend credits on", "Making", "Made", &what)
        })
        .run(|a| async move { make_thumbnail(a).await }.boxed()),
    ]
}

fn model_and_res() -> (&'static magnific::ImageModel, String, String) {
    let prefs = magnific::Prefs::load();
    let model = magnific::CATALOG.iter().find(|m| m.id == prefs.model).unwrap_or(&magnific::CATALOG[0]);
    let res = if model.resolutions.contains(&prefs.resolution.as_str()) { prefs.resolution.clone() } else { model.resolutions.first().copied().unwrap_or("Auto").to_string() };
    let aspect = if prefs.aspect.is_empty() { "16:9".to_string() } else { prefs.aspect.clone() };
    (model, res, aspect)
}

/// The model, size and aspect a thumbnail is really made with (the approval text and the run
/// both use this): the Thumbnails page's choice, with `aspect` given by the call over it.
fn final_model(aspect: &str, uses_refs: bool) -> (&'static magnific::ImageModel, String, String) {
    let (model, res, aspect_pref) = model_and_res();
    pick_model(model, &magnific::Keys::load(), res, if aspect.is_empty() { aspect_pref } else { aspect.to_string() }, uses_refs)
}

/// `model` unless it cannot do the job: it has no key, character references (a prompt with
/// @Name) need a model that takes them, and a model without aspect control only makes squares;
/// then another model with a key that can (`magnific::usable`). The size falls back to the
/// model's first.
fn pick_model(model: &'static magnific::ImageModel, keys: &magnific::Keys, res: String, aspect: String, uses_refs: bool) -> (&'static magnific::ImageModel, String, String) {
    let model = magnific::usable(model, keys, uses_refs, &aspect);
    let res = if model.resolutions.contains(&res.as_str()) { res } else { model.resolutions.first().copied().unwrap_or("Auto").to_string() };
    (model, res, aspect)
}

/// " (about N Magnific credits)" for the approval, "up to N" when the cost depends on what the
/// run reads; empty when the model's price is unknown.
fn credits_note(costs: &[f64]) -> String {
    let (lo, hi) = costs.iter().fold((f64::MAX, 0f64), |(lo, hi), c| (lo.min(*c), hi.max(*c)));
    if hi <= 0. {
        String::new()
    } else if (hi - lo).abs() < 0.5 {
        format!(" (about {hi:.0} Magnific credits)")
    } else {
        format!(" (up to {hi:.0} Magnific credits)")
    }
}

async fn thumbnails(a: Map<String, Value>) -> ToolResult {
    let groups = mv_ui::bg::blocking(crate::data::load).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
    let g = crate::assist::find_manga(&groups, str_arg(&a, "manga"))?;
    let gallery = super::thumbs::Gallery::load(&g.id);
    let (model, res, aspect) = model_and_res();
    let usage = magnific::Usage::load();
    Ok(ToolOutput::Value(json!({
        "manga": g.title,
        "made": gallery.thumbs.iter().take(20).map(|t| json!({ "id": t.id, "label": t.label, "prompt": t.prompt, "model": t.model, "file": super::thumbs::Gallery::image(&g.id, &t.id).display().to_string() })).collect::<Vec<_>>(),
        "ideas": gallery.concepts.iter().enumerate().map(|(i, c)| json!({ "concept": i + 1, "name": c.name, "why": c.why, "prompt": c.prompt, "text": c.text })).collect::<Vec<_>>(),
        "model": model.name,
        "resolution": res,
        "aspect": aspect,
        "creditsPerImage": model.cost_at(&res),
        "magnificKey": magnific::key().is_some(),
        "otherImageServicesWithKeys": magnific::Keys::load().services,
        "spentThisMonth": usage.month_spent(),
    })))
}

async fn make_thumbnail(a: Map<String, Value>) -> ToolResult {
    if !magnific::Keys::load().any() {
        return Err("No image service key yet. The user adds a Magnific key in Settings → Magnific (or on the Thumbnails page), or a Higgsfield, fal.ai, Replicate, Black Forest Labs or Recraft key in Settings → AI services.".into());
    }
    let groups = mv_ui::bg::blocking(crate::data::load).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
    let g = crate::assist::find_manga(&groups, str_arg(&a, "manga"))?.clone();
    let gallery = super::thumbs::Gallery::load(&g.id);
    let (id, title) = (g.id.clone(), g.title.clone());
    let brain = mv_ui::bg::blocking(move || crate::run::brain_snapshot(&id, &title)).await.map_err(|e| e.to_string())?;
    let cast: Vec<(logic::Cast, std::path::PathBuf)> = logic::lead_cast(&brain, super::thumb_view::CAST_FOR_REFS).into_iter().filter_map(|(c, f)| Some((c, f?))).collect();
    let just_cast: Vec<logic::Cast> = cast.iter().map(|(c, _)| c.clone()).collect();
    let (prompt, label) = match (str_arg(&a, "prompt"), num_arg(&a, "concept")) {
        (p, _) if !p.is_empty() => (p.to_string(), "Custom".to_string()),
        (_, Some(n)) => {
            let c = gallery.concepts.get((n.max(1.) as usize) - 1).ok_or_else(|| format!("There are {} ideas; pick one of 1-{}", gallery.concepts.len(), gallery.concepts.len().max(1)))?;
            (c.prompt.clone(), c.name.clone())
        }
        _ => {
            let saved = Saved::load(&g.id);
            let hook = saved.suggestion.as_ref().map(|s| s.thumbnail_text.clone()).unwrap_or_default();
            let story = logic::story_so_far(&brain.notes, 4);
            let refs: Vec<bool> = cast.iter().map(|_| true).collect();
            (logic::thumbnail_prompt(&g.title, &just_cast, &refs, &story, &hook), "Package idea".to_string())
        }
    };
    let (resolved, order) = super::thumbs::resolve(&prompt, &just_cast, 0);
    // `resolve` only finds references at an @Name, so this is what the approval assumed.
    let (model, res, aspect) = final_model(str_arg(&a, "aspect"), !order.is_empty() || prompt.contains('@'));
    let keys = magnific::Keys::load();
    let Some(key) = magnific::key_for(model) else {
        if keys.any() {
            return Err(magnific::no_fit(&keys, !order.is_empty() || prompt.contains('@'), &aspect));
        }
        return Err(format!("{} needs a {} key. The user adds it in Settings → AI services.", model.name, model.service_label()));
    };
    let mut refs: Vec<magnific::Reference> = vec![];
    let mut looks = vec![];
    for i in order.iter().take(model.refs) {
        let (c, f) = &cast[*i];
        refs.push(magnific::Reference { file: f.clone(), caption: format!("Image {}: {}", refs.len() + 1, c.name) });
        looks.push((c.name.clone(), super::thumbs::look_of(c)));
    }
    let full = super::thumbs::image_prompt(&g.title, &resolved, &looks, 0);
    let tid = format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0));
    let out = super::thumbs::Gallery::image(&g.id, &tid);
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let progress = ui_assistant::tools::progress_handle();
    let (o2, k2, f2, a2, r2) = (out.clone(), key.clone(), full.clone(), aspect.clone(), res.clone());
    let job = async move {
        magnific::generate(model, &k2, &f2, &refs, &a2, &r2, &o2, move |s| {
            if let Some(p) = &progress {
                p(s.to_string());
            }
        })
        .await
    };
    mv_ui::bg::run(job).await.map_err(|e| e.to_string())?.map_err(|e| format!("{e:#}"))?;
    let cost = model.cost_at(&res);
    magnific::record(model, cost);
    let mut gallery = super::thumbs::Gallery::load(&g.id);
    gallery.thumbs.insert(0, super::thumbs::Thumb { id: tid.clone(), prompt, label, parent: None, model: model.name.to_string(), resolution: res.clone(), cost, created: super::now() });
    gallery.save(&g.id);
    super::video_thumb::learn_made(&gallery.thumbs[0].prompt, model.name, &aspect);
    // The new thumbnail can be the one the manga's videos show: draw their posters again.
    let mid = g.id.clone();
    let changed = mv_ui::bg::blocking(move || super::video_thumb::redraw_blocking(&mid)).await.and_then(|r| r).unwrap_or_default();
    ui_assistant::on_ui(move |_, cx| {
        super::thumb_page::notify_changed(cx);
        super::video_thumb::redrawn(changed, cx);
    })
    .await
    .ok();
    Ok(ToolOutput::Value(json!({ "manga": g.title, "file": out.display().to_string(), "model": model.name, "resolution": res, "aspect": aspect, "credits": cost, "note": "Saved on the Thumbnails page (reopen it to see the new image)." })))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str) -> &'static magnific::ImageModel {
        magnific::CATALOG.iter().find(|m| m.id == id).unwrap()
    }

    fn magnific_only() -> magnific::Keys {
        magnific::Keys { magnific: true, services: vec![] }
    }

    fn pick_model(model: &'static magnific::ImageModel, res: String, aspect: String, uses_refs: bool) -> (&'static magnific::ImageModel, String, String) {
        super::pick_model(model, &magnific_only(), res, aspect, uses_refs)
    }

    #[test]
    fn a_model_without_a_key_gives_way_to_one_with_a_key() {
        let fal_only = magnific::Keys { magnific: false, services: vec!["fal"] };
        // The saved Magnific default, but only fal.ai has a key.
        let (m, res, _) = super::pick_model(model("nano-banana-2-1"), &fal_only, "2K".into(), "16:9".into(), true);
        assert_eq!((m.id, res.as_str()), ("fal-nano-banana", "Auto"));
        assert_eq!(super::pick_model(model("fal-flux-pro"), &fal_only, "".into(), "16:9".into(), false).0.id, "fal-flux-pro");
        // References on a fal.ai model without them stay on fal.ai.
        assert_eq!(super::pick_model(model("fal-flux-pro"), &fal_only, "".into(), "16:9".into(), true).0.id, "fal-nano-banana");
        assert_eq!(credits_note(&[m.cost_at(&res)]), "");
    }

    #[test]
    fn the_model_switches_only_when_it_cannot_do_the_job() {
        let first = &magnific::CATALOG[0];
        // A model without references, asked for @characters: the first model, at a size it has.
        let (m, res, _) = pick_model(model("nano-banana-pro"), "4K".into(), "16:9".into(), true);
        assert_eq!(m.id, first.id);
        assert!(first.resolutions.contains(&res.as_str()));
        // Without references it stays.
        assert_eq!(pick_model(model("nano-banana-pro"), "2K".into(), "16:9".into(), false).0.id, "nano-banana-pro");
        // No aspect control: only squares stay.
        assert_eq!(pick_model(model("nano-banana"), "".into(), "16:9".into(), false).0.id, first.id);
        assert_eq!(pick_model(model("nano-banana"), "".into(), "1:1".into(), false).0.id, "nano-banana");
        // The approval quotes the model the run uses.
        let (m, res, _) = pick_model(model("nano-banana-pro"), "4K".into(), "16:9".into(), true);
        assert_eq!(credits_note(&[m.cost_at(&res)]), format!(" (about {:.0} Magnific credits)", first.cost_at(&res)));
    }

    #[test]
    fn the_credit_note_says_about_or_up_to() {
        assert_eq!(credits_note(&[50.]), " (about 50 Magnific credits)");
        assert_eq!(credits_note(&[50., 75.]), " (up to 75 Magnific credits)");
        assert_eq!(credits_note(&[0.]), "");
        assert_eq!(credits_note(&[]), "");
    }
}
