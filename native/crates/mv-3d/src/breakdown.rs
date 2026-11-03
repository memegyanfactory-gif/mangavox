//! BREAKDOWN: the AI reads the chapter (mv-recap's page readings and the manga's character
//! brain) and returns the episode: its scenes in order and every location, character and prop
//! they need, each with a visual description a 3D artist could build from.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{bail, Result};
use mv_ai::CancellationToken;
use mv_recap::{Brain, ChapterRead, ChapterRef, ModelChoice};
use serde_json::Value;

use crate::ai::{ask_json, request, Llm};
use crate::library::Category;
use crate::plan::{AssetNeed, ScenePlan};
use crate::util::{slug, truncate};

const SYSTEM: &str = "You are the director and production designer of a 3D anime adaptation of a manga. \
You turn one chapter into one episode: you split it into scenes and list every asset the 3D team must build. \
Reply with one JSON object only, no prose.";

/// A character of the chapter: canonical name, how often they appear, their look.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CastMember {
    pub name: String,
    pub count: usize,
    pub look: String,
}

/// Who appears in the chapter, most present first (at most `max`).
pub(crate) fn cast(read: &ChapterRead, brain: &Brain, max: usize) -> Vec<CastMember> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut order: Vec<String> = vec![];
    for p in &read.pages {
        for panel in &p.panels {
            for c in &panel.characters {
                let name = brain.canonical(&c.name);
                if name.is_empty() {
                    continue;
                }
                if !counts.contains_key(&name) {
                    order.push(name.clone());
                }
                *counts.entry(name).or_default() += 1;
            }
        }
    }
    let mut v: Vec<CastMember> = order
        .into_iter()
        .map(|name| {
            let look = brain.find(&name).map(|c| {
                let traits = if c.traits.is_empty() { String::new() } else { format!(" ({})", c.traits.join(", ")) };
                format!("{}{}", c.description, traits)
            });
            CastMember { count: counts[&name], look: look.unwrap_or_default(), name }
        })
        .collect();
    v.sort_by(|a, b| b.count.cmp(&a.count));
    v.truncate(max);
    v
}

pub(crate) fn prompt(ch: &ChapterRef, read: &ChapterRead, brain: &Brain, catalog: &str, cast: &[CastMember]) -> String {
    let mut s = format!("Manga: {}. Chapter: {}. {} pages.\n", ch.manga_title, ch.label, read.pages.len());
    if let Some(r) = brain.recap_before(&ch.label) {
        s.push_str(&format!("\nStory so far: {}\n", truncate(r, 1500)));
    }
    s.push_str("\nCast of this chapter (use these names exactly; panel count, look):\n");
    for c in cast {
        s.push_str(&format!("- {} ({}): {}\n", c.name, c.count, truncate(&c.look, 240)));
    }
    if !catalog.trim().is_empty() {
        s.push_str("\nAssets already in the library (when an asset is one of these, put its id in \"reuse\"):\n");
        s.push_str(catalog);
    }
    s.push_str("\nPages (0-based):\n");
    for p in &read.pages {
        s.push_str(&format!("[p{}] ({}, {}) {}\n", p.index, p.scene.kind, p.scene.mood, truncate(&p.summary, 260)));
        for (i, panel) in p.panels.iter().enumerate().take(6) {
            let who: Vec<String> = panel.characters.iter().map(|c| brain.canonical(&c.name)).collect();
            let lines: Vec<String> = panel.lines.iter().filter(|l| l.kind != "sfx").take(2).map(|l| format!("{}: \"{}\"", l.speaker.as_deref().unwrap_or("-"), truncate(&l.text, 90))).collect();
            s.push_str(&format!("   panel {i}: {}", truncate(&panel.description, 170)));
            if !who.is_empty() {
                s.push_str(&format!(" | who: {}", who.join(", ")));
            }
            if !lines.is_empty() {
                s.push_str(&format!(" | {}", lines.join(" ")));
            }
            s.push('\n');
        }
    }
    s.push_str(
        r#"
Answer with this JSON:
{"title": "episode title", "logline": "one sentence",
 "assets": [{"key": "short_snake_case", "category": "environment|character|prop|vehicle|fx", "name": "...",
   "description": "what it looks like for a 3D artist: shapes, sizes in metres, colours, materials; for a location the whole set: floor, walls or terrain, the main furniture or landmarks",
   "tags": ["..."], "pages": [0, 3], "generic": false, "interior": true, "map_pos": [-0.4, 0.2], "reuse": null}],
 "scenes": [{"title": "...", "pages": [first, last], "location": "asset key", "time_of_day": "day|morning|dawn|dusk|evening|night|interior",
   "mood": "...", "characters": ["asset keys"], "props": ["asset keys"], "summary": "what happens, beat by beat, as it should be acted"}]}
Rules:
- 3 to 12 scenes, in page order, covering every page; a scene is one location and continuous time.
- Every character who is seen or speaks in a scene is in its "characters" (at most 6, the important ones). Characters use the cast names.
- Props: only things characters hold or use, or that matter to the story (at most 5 per scene). Vehicles and effects (fire, aura, energy blast) are their own categories.
- Every location used by a scene is an asset with category "environment"; map_pos places it on the world map (-1..1).
- generic is true only for ordinary real-world objects (chair, car, tree), never for something unique to this story.
- Skip text-only pages (credits, notices): leave them inside a neighbouring scene.
"#,
    );
    s
}

/// Turns the AI's answer into needs and scenes; drops anything that does not hold together.
pub(crate) fn parse(v: &Value, pages: usize) -> Result<(String, String, Vec<AssetNeed>, Vec<ScenePlan>)> {
    let str_of = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().trim().to_string();
    let list = |v: &Value, k: &str| -> Vec<String> { v.get(k).and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|s| s.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect()).unwrap_or_default() };
    let mut needs: Vec<AssetNeed> = vec![];
    for a in v.get("assets").and_then(|a| a.as_array()).into_iter().flatten() {
        let name = str_of(a, "name");
        let Some(category) = Category::parse(&str_of(a, "category")) else { continue };
        if name.is_empty() || matches!(category, Category::Skies | Category::Maps) {
            continue;
        }
        let mut key = slug(&str_of(a, "key")).replace('-', "_");
        if key == "asset" {
            key = slug(&name).replace('-', "_");
        }
        if needs.iter().any(|n| n.key == key) {
            continue;
        }
        let pages_of: Vec<usize> = a.get("pages").and_then(|p| p.as_array()).map(|p| p.iter().filter_map(|x| x.as_u64()).map(|x| x as usize).filter(|x| *x < pages).collect()).unwrap_or_default();
        let map_pos = a.get("map_pos").and_then(|p| p.as_array()).and_then(|p| Some([p.first()?.as_f64()? as f32, p.get(1)?.as_f64()? as f32]));
        needs.push(AssetNeed {
            key,
            name,
            category,
            description: str_of(a, "description"),
            tags: list(a, "tags"),
            pages: pages_of,
            generic: a.get("generic").and_then(|g| g.as_bool()).unwrap_or(false),
            reuse: a.get("reuse").and_then(|r| r.as_str()).map(str::to_string).filter(|r| !r.is_empty()),
            asset_id: None,
            interior: a.get("interior").and_then(|g| g.as_bool()),
            map_pos: map_pos.map(|[x, y]| [x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0)]),
        });
    }
    let mut scenes: Vec<ScenePlan> = vec![];
    for s in v.get("scenes").and_then(|a| a.as_array()).into_iter().flatten() {
        let pr: Vec<usize> = s.get("pages").and_then(|p| p.as_array()).map(|p| p.iter().filter_map(|x| x.as_u64()).map(|x| x as usize).collect()).unwrap_or_default();
        let (first, last) = match (pr.first(), pr.last()) {
            (Some(a), Some(b)) => ((*a).min(pages.saturating_sub(1)), (*b).min(pages.saturating_sub(1))),
            _ => continue,
        };
        let known = |k: &String, cat: Option<Category>| needs.iter().any(|n| &n.key == k && cat.is_none_or(|c| n.category == c));
        let mut location = slug(&str_of(s, "location")).replace('-', "_");
        if !known(&location, Some(Category::Environments)) {
            location = needs.iter().find(|n| n.category == Category::Environments).map(|n| n.key.clone()).unwrap_or_default();
        }
        let characters: Vec<String> = list(s, "characters").into_iter().map(|k| slug(&k).replace('-', "_")).filter(|k| known(k, Some(Category::Characters))).take(6).collect();
        let props: Vec<String> = list(s, "props").into_iter().map(|k| slug(&k).replace('-', "_")).filter(|k| known(k, None)).take(5).collect();
        scenes.push(ScenePlan {
            index: scenes.len(),
            title: str_of(s, "title"),
            pages: [first.min(last), first.max(last)],
            location,
            time_of_day: str_of(s, "time_of_day").to_lowercase(),
            mood: str_of(s, "mood"),
            characters,
            props,
            summary: str_of(s, "summary"),
            ..Default::default()
        });
    }
    if scenes.is_empty() {
        bail!("the AI's breakdown had no usable scenes");
    }
    scenes.sort_by_key(|s| s.pages[0]);
    for (i, s) in scenes.iter_mut().enumerate() {
        s.index = i;
    }
    // A scene without a location gets a plain one, so every scene has a set to stand on.
    if scenes.iter().any(|s| s.location.is_empty()) {
        let key = "main_location".to_string();
        if !needs.iter().any(|n| n.key == key) {
            needs.push(AssetNeed { key: key.clone(), name: "Main location".into(), category: Category::Environments, description: "A simple open set".into(), ..Default::default() });
        }
        for s in scenes.iter_mut().filter(|s| s.location.is_empty()) {
            s.location = key.clone();
        }
    }
    // Only keep needs some scene uses.
    needs.retain(|n| scenes.iter().any(|s| s.location == n.key || s.characters.contains(&n.key) || s.props.contains(&n.key)));
    Ok((str_of(v, "title"), str_of(v, "logline"), needs, scenes))
}

pub(crate) async fn run(
    llm: &dyn Llm,
    model: &ModelChoice,
    ch: &ChapterRef,
    read: &ChapterRead,
    brain: &Brain,
    catalog: &str,
    cancel: &CancellationToken,
) -> Result<(String, String, Vec<AssetNeed>, Vec<ScenePlan>)> {
    let cast = cast(read, brain, 14);
    let user = prompt(ch, read, brain, catalog, &cast);
    let v = ask_json(llm, request(model, SYSTEM, user, vec![]), cancel).await?;
    parse(&v, read.pages.len())
}

/// Reference images of a need: panels where it appears (largest first), faces for characters.
pub(crate) fn references(ch: &ChapterRef, read: &ChapterRead, brain: &Brain, need: &AssetNeed, max: usize) -> Vec<PathBuf> {
    let mut scored: Vec<(f32, PathBuf)> = vec![];
    let words = crate::util::tokens(&need.name);
    for p in &read.pages {
        let on_page = need.pages.contains(&p.index);
        for (i, panel) in p.panels.iter().enumerate() {
            let area = panel.bbox.map(|b| b.area()).unwrap_or(0.3);
            let score = match need.category {
                Category::Characters => panel.characters.iter().any(|c| brain.canonical(&c.name).eq_ignore_ascii_case(&need.name)).then_some(area / (panel.characters.len() as f32)),
                Category::Environments => on_page.then_some(area * (1.0 + 0.3 * (panel.characters.is_empty() as u8 as f32))),
                _ => {
                    let d = panel.description.to_lowercase();
                    (on_page && words.iter().any(|w| d.contains(w.as_str()))).then_some(area)
                }
            };
            if let Some(score) = score {
                let crop = mv_recap::panel_path(ch, p.index, i);
                let path = if crop.is_file() { crop } else { ch.pages_dir.join(&p.file) };
                if path.is_file() && !scored.iter().any(|(_, q)| q == &path) {
                    scored.push((score, path));
                }
            }
        }
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut out: Vec<PathBuf> = vec![];
    if need.category == Category::Characters {
        if let Some(c) = brain.find(&need.name) {
            out.extend(c.faces.iter().filter(|f| f.is_file()).take(2).cloned());
        }
    }
    out.extend(scored.into_iter().map(|(_, p)| p));
    if out.is_empty() {
        if let Some(p) = need.pages.first().and_then(|i| read.pages.iter().find(|p| p.index == *i)) {
            let page = ch.pages_dir.join(&p.file);
            if page.is_file() {
                out.push(page);
            }
        }
    }
    out.truncate(max);
    out
}
