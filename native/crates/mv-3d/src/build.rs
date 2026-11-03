//! ASSETS: makes sure each thing a scene needs is in the library. In order: the asset already
//! resolved for this episode, the library asset the AI named, a library asset with the same
//! name; otherwise a new one is made - a generic prop from Poly Haven, a character from the
//! parametric builder with parameters the AI reads off the manga's panels, anything else from a
//! Blender script the AI writes while looking at those panels (fed its own errors to fix them,
//! then a plain built-in fallback so a scene never stalls on one asset).

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Result};
use mv_ai::CancellationToken;
use mv_recap::{Brain, ChapterRead, ChapterRef, ModelChoice};
use serde_json::{json, Value};

use crate::ai::{ask_code, ask_json, request, Llm};
use crate::library::{copy_refs, Asset, Category, Library, Origin};
use crate::plan::AssetNeed;
use crate::util::truncate;

const SCRIPT_TRIES: usize = 3;
const BLENDER_TIMEOUT: Duration = Duration::from_secs(15 * 60);

pub(crate) struct Builder<'a> {
    pub llm: &'a dyn Llm,
    pub model: &'a ModelChoice,
    pub blender: &'a Path,
    pub scripts: &'a Path,
    pub cancel: &'a CancellationToken,
    pub log: &'a (dyn Fn(String) + Send + Sync),
    pub online: bool,
}

/// What `ensure` did.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Got {
    Reused(String),
    Built(String),
}

impl Got {
    pub fn id(&self) -> &str {
        match self {
            Got::Reused(id) | Got::Built(id) => id,
        }
    }
}

/// An existing library asset for `need`, without building anything.
pub(crate) fn existing(lib: &Library, need: &AssetNeed, manga_id: &str) -> Option<String> {
    let ok = |id: &str| lib.get(id).filter(|a| a.category == need.category && lib.ready(a)).map(|a| a.id.clone());
    need.asset_id
        .as_deref()
        .and_then(ok)
        .or_else(|| need.reuse.as_deref().and_then(ok).filter(|id| !need.category.per_manga() || lib.get(id).and_then(|a| a.manga_id.as_deref()) == Some(manga_id)))
        .or_else(|| lib.find_match(need.category, &need.name, manga_id).map(|a| a.id.clone()))
}

impl Builder<'_> {
    pub async fn ensure(&self, lib: &mut Library, need: &AssetNeed, ch: &ChapterRef, read: &ChapterRead, brain: &Brain) -> Result<Got> {
        if let Some(id) = existing(lib, need, &ch.manga_id) {
            return Ok(Got::Reused(id));
        }
        let id = lib.new_id(&need.name, &ch.manga_title, need.category);
        let rel = format!("{}/{}", need.category.dir(), id);
        let dir = lib.root.join(&rel);
        std::fs::create_dir_all(&dir)?;
        let refs = crate::breakdown::references(ch, read, brain, need, 3);
        let source_panels = copy_refs(&dir, &refs);
        let mut asset = Asset {
            id: id.clone(),
            name: need.name.clone(),
            category: need.category,
            description: need.description.clone(),
            tags: need.tags.clone(),
            manga_id: need.category.per_manga().then(|| ch.manga_id.clone()).or_else(|| (!need.generic).then(|| ch.manga_id.clone())),
            manga_title: ch.manga_title.clone(),
            dir: rel,
            source_panels,
            version: 1,
            ..Default::default()
        };
        (self.log)(format!("Building {} \"{}\"", need.category.label().trim_end_matches('s').to_lowercase(), need.name));
        let result = match need.category {
            Category::Characters => self.character(need, &refs, &dir, &mut asset).await,
            Category::Props | Category::Vehicles if need.generic && self.online => match self.online_model(need, &dir, &mut asset, &lib.root).await {
                Ok(v) => Ok(v),
                Err(e) => {
                    (self.log)(format!("No online model for \"{}\" ({e:#}); modelling it instead", need.name));
                    self.scripted(need, &refs, &dir, &mut asset).await
                }
            },
            _ => self.scripted(need, &refs, &dir, &mut asset).await,
        }?;
        asset.dims = dims_of(&result);
        lib.put(asset)?;
        Ok(Got::Built(id))
    }

    async fn run_job(&self, dir: &Path, mut job: Value, asset: &Asset) -> Result<Value> {
        job["name"] = json!(asset.name);
        job["category"] = json!(asset.category.dir());
        job["out_blend"] = json!(dir.join("asset.blend"));
        job["out_preview"] = json!(dir.join("preview.png"));
        job["outline"] = json!(true);
        let job_path = dir.join("job.json");
        std::fs::write(&job_path, serde_json::to_vec_pretty(&job)?)?;
        crate::blender::run(self.blender, &self.scripts.join("asset_runner.py"), &job_path, &dir.join("blender.log"), BLENDER_TIMEOUT, self.cancel).await
    }

    async fn character(&self, need: &AssetNeed, refs: &[PathBuf], dir: &Path, asset: &mut Asset) -> Result<Value> {
        let user = format!(
            "Character: {}\nLook: {}\nTags: {}\nThe images are this character in the manga (faces and panels).\n\n{}",
            need.name,
            need.description,
            need.tags.join(", "),
            CHARACTER_PARAMS
        );
        let params = match ask_json(self.llm, request(self.model, CHARACTER_SYSTEM, user, refs.to_vec()), self.cancel).await {
            Ok(v) if v.is_object() => v,
            Ok(_) | Err(_) if self.cancel.is_cancelled() => return Err(anyhow!("cancelled")),
            Ok(_) => json!({}),
            Err(e) => {
                (self.log)(format!("The AI could not read \"{}\"'s look ({e:#}); using a plain figure", need.name));
                json!({})
            }
        };
        std::fs::write(dir.join("params.json"), serde_json::to_vec_pretty(&params)?)?;
        asset.origin = Origin::Character;
        asset.params = Some(params.clone());
        self.run_job(dir, json!({ "character": params }), asset).await
    }

    async fn online_model(&self, need: &AssetNeed, dir: &Path, asset: &mut Asset, lib_root: &Path) -> Result<Value> {
        let found = crate::online::find_model(&need.name, &need.tags, &lib_root.join(".cache")).await?.ok_or_else(|| anyhow!("nothing matching on Poly Haven"))?;
        (self.log)(format!("Found \"{}\" on Poly Haven for \"{}\"", found.name, need.name));
        let file = crate::online::download_model(&found.id, &dir.join("src")).await?;
        asset.origin = Origin::Online { source: "Poly Haven".into(), url: format!("https://polyhaven.com/a/{}", found.id), license: crate::online::LICENSE.into() };
        asset.tags.push(format!("polyhaven:{}", found.id));
        self.run_job(dir, json!({ "import_file": file }), asset).await
    }

    async fn scripted(&self, need: &AssetNeed, refs: &[PathBuf], dir: &Path, asset: &mut Asset) -> Result<Value> {
        let brief = script_brief(need);
        let mut last_err = String::new();
        let mut last_code = String::new();
        for attempt in 0..SCRIPT_TRIES {
            let user = if attempt == 0 {
                brief.clone()
            } else {
                format!("{brief}\n\nYour previous script failed in Blender:\n```python\n{}\n```\nError:\n{}\n\nFix it and send the whole script again.", truncate(&last_code, 6000), truncate(&last_err, 1500))
            };
            let code = match ask_code(self.llm, request(self.model, SCRIPT_SYSTEM, user, refs.to_vec()), self.cancel).await {
                Ok(c) => c,
                Err(e) if self.cancel.is_cancelled() => return Err(e),
                Err(e) => {
                    last_err = format!("{e:#}");
                    continue;
                }
            };
            let file = dir.join("build.py");
            std::fs::write(&file, &code)?;
            match self.run_job(dir, json!({ "code_file": file }), asset).await {
                Ok(v) => {
                    asset.origin = Origin::Procedural;
                    return Ok(v);
                }
                Err(e) if self.cancel.is_cancelled() => return Err(e),
                Err(e) => {
                    (self.log)(format!("Script for \"{}\" failed (try {}): {}", need.name, attempt + 1, truncate(&format!("{e:#}").lines().last().unwrap_or_default().to_string(), 160)));
                    last_err = format!("{e:#}");
                    last_code = code;
                }
            }
        }
        (self.log)(format!("Using a simple stand-in for \"{}\"", need.name));
        let file = dir.join("build.py");
        std::fs::write(&file, fallback_script(need))?;
        asset.origin = Origin::Template;
        self.run_job(dir, json!({ "code_file": file }), asset).await
    }

    /// The manga's world map: every location of the library for this manga, pinned. Rebuilt (same
    /// id, next version) when the episode brings a location the map does not show yet.
    pub async fn world_map(&self, lib: &mut Library, ch: &ChapterRef, locations: &[(String, Option<[f32; 2]>)]) -> Result<String> {
        // One map build at a time, on a fresh copy of the index: two chapters of a manga adding
        // locations at once each see the pins the other just saved.
        static MAP_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
        let _guard = MAP_LOCK.get_or_init(Default::default).lock().await;
        lib.save()?;
        let existing = lib.assets.iter().find(|a| a.category == Category::Maps && a.manga_id.as_deref() == Some(ch.manga_id.as_str())).cloned();
        let mut pins: Vec<Value> = existing.as_ref().and_then(|a| a.params.as_ref()).and_then(|p| p.get("locations")).and_then(|l| l.as_array()).cloned().unwrap_or_default();
        let mut added = false;
        for (i, (name, pos)) in locations.iter().enumerate() {
            if pins.iter().any(|p| p.get("name").and_then(|n| n.as_str()).is_some_and(|n| n.eq_ignore_ascii_case(name))) {
                continue;
            }
            let k = (pins.len() + i) as f32;
            let pos = pos.unwrap_or([(k * 2.1).cos() * 0.6, (k * 2.1).sin() * 0.6]);
            pins.push(json!({ "name": name, "pos": pos }));
            added = true;
        }
        if let Some(a) = &existing {
            if !added && lib.ready(a) {
                return Ok(a.id.clone());
            }
        }
        let mut asset = existing.unwrap_or_else(|| {
            let id = lib.new_id("World map", &ch.manga_title, Category::Maps);
            Asset {
                id: id.clone(),
                name: format!("{} world map", ch.manga_title),
                category: Category::Maps,
                description: "Every location of the story, pinned on one terrain".into(),
                manga_id: Some(ch.manga_id.clone()),
                manga_title: ch.manga_title.clone(),
                dir: format!("maps/{id}"),
                origin: Origin::Template,
                ..Default::default()
            }
        });
        asset.version += 1;
        asset.params = Some(json!({ "locations": pins }));
        asset.tags = pins.iter().filter_map(|p| p.get("name").and_then(|n| n.as_str()).map(String::from)).collect();
        let dir = lib.root.join(&asset.dir);
        std::fs::create_dir_all(&dir)?;
        (self.log)(format!("Drawing the world map ({} locations)", pins.len()));
        let v = self.run_job(&dir, json!({ "world_map": { "locations": pins } }), &asset).await?;
        asset.dims = dims_of(&v);
        let id = asset.id.clone();
        lib.put(asset)?;
        Ok(id)
    }

    /// A sky HDRI for an exterior: from the library, else downloaded once and shared.
    pub async fn sky(&self, lib: &mut Library, time_of_day: &str, words: &[String]) -> Result<Option<PathBuf>> {
        if !self.online {
            return Ok(None);
        }
        let Some(found) = crate::online::find_hdri(time_of_day, words, &lib.root.join(".cache")).await? else { return Ok(None) };
        let id = format!("sky-{}", crate::util::slug(&found.id));
        if let Some(a) = lib.get(&id) {
            return Ok(lib.sky_file(a));
        }
        let rel = format!("skies/{id}");
        let dir = lib.root.join(&rel);
        (self.log)(format!("Downloading the sky \"{}\" from Poly Haven", found.name));
        let file = crate::online::download_hdri(&found.id, &dir).await?;
        lib.put(Asset {
            id,
            name: found.name.clone(),
            category: Category::Skies,
            description: format!("{time_of_day} sky"),
            tags: vec![time_of_day.to_string(), format!("polyhaven:{}", found.id)],
            dir: rel,
            origin: Origin::Online { source: "Poly Haven".into(), url: format!("https://polyhaven.com/a/{}", found.id), license: crate::online::LICENSE.into() },
            version: 1,
            ..Default::default()
        })?;
        Ok(Some(file))
    }
}

fn dims_of(v: &Value) -> [f32; 3] {
    let d = v.get("dims").and_then(|d| d.as_array());
    let at = |i: usize| d.and_then(|d| d.get(i)).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
    [at(0), at(1), at(2)]
}

const CHARACTER_SYSTEM: &str = "You are a character designer turning manga characters into simple stylized 3D figures. \
You look at the reference images and pick the figure's parameters. Reply with one JSON object only.";

const CHARACTER_PARAMS: &str = r##"Pick parameters for the figure builder. JSON keys (all optional):
{"height": metres (1.0-2.3), "build": "slim|average|athletic|muscular|heavy|child",
 "skin": "#rrggbb", "hair_color": "#rrggbb", "hair_style": "short|spiky|long|ponytail|swept|bald|bun", "eye_color": "#rrggbb",
 "top_kind": "tshirt|tank|shirt|jacket|robe|shirtless|armor|dress", "top_color": "#rrggbb",
 "bottom_kind": "pants|shorts|skirt", "bottom_color": "#rrggbb", "shoe_color": "#rrggbb", "belt_color": "#rrggbb" or null,
 "accessories": ["sword|headband|glasses|cape|scarf|bag|hat|mask|gloves|earrings"], "accent_color": "#rrggbb"}
Manga pages are often black and white: choose natural, believable colours that fit the character (black hair is #1a1a1a, not grey)."##;

const SCRIPT_SYSTEM: &str = "You are a Blender technical artist for a stylized 3D anime. You write one Python script that models one asset \
from simple, clean shapes, looking at the manga panels you are given. Reply with the script in one ```python block and nothing else.";

const HELPERS: &str = r##"The script runs in a sandbox with only these helpers, plus math, random and Vector (no bpy, no imports):
  mat(name, color="#rrggbb", rough=0.6, metal=0.0, emit=0.0) -> material (emit > 0 glows)
  box(name, size=(x, y, z), loc=(x, y, z), rot=(deg, deg, deg), color="#rrggbb", material=None, parent=None, bevel=0.0)
  cyl(name, r, depth, loc, rot, color, material, parent, verts=24)   # along Z, centred on loc
  cone(name, r1, r2, depth, loc, rot, color, material, parent)
  sphere(name, r, loc, scale=(1, 1, 1), color, material, parent)
  torus(name, r_major, r_minor, loc, rot, color, material, parent)
  plane(name, size=(x, y), loc, rot, color, material, parent)
  text(name, body, loc, size, color, rot=(90, 0, 0), parent)
  smooth(part), set_parent(child, parent)
  empty(name, loc, parent)                                         # group parts under one empty
  light(name, kind="POINT", energy=500, loc, rot, color, size=1.0)  # POINT | SUN | SPOT | AREA lamp
  scatter(fn, count, area=(x, y), seed=1, z=0.0, avoid_center=0.0) # calls fn(i, (x, y, z), rng)
  build_room(width, depth, height, floor, wall, trim, open_front=True)   # floor + back and side walls
  build_outdoor(size, ground, features, seed, feature_color)             # ground + trees and rocks
Shapes return a part: you may read or set part.location, part.rotation (degrees), part.scale and part.parent, and read part.name - nothing else. Rules: metres; the ground is z = 0; centred on x = y = 0; the front faces -Y.
Plain Python only: no import, class, match, global or async; no bpy, open, eval, getattr or .format(); no names starting with "_". Only methods of plain values (list.append, str.join, math.sin ...) may be called. A script that breaks a rule is rejected. Keep it under 160 lines."##;

fn script_brief(need: &AssetNeed) -> String {
    let rules = match need.category {
        Category::Environments => {
            let kind = if need.interior == Some(false) { "an exterior" } else { "a set (interior or exterior)" };
            format!(
                "This is {kind} where scenes are acted. Make it 16 to 30 m across with a floor or terrain at z = 0. Keep a clear stage \
                 of 8 x 8 m around the origin for the characters. Leave the -Y side open for the cameras: no wall and no ceiling on that side, \
                 and no ceiling at all. Put the defining landmarks, furniture and details around the stage so the place is recognisable."
            )
        }
        Category::Vehicles => "A vehicle at real-world size, wheels or hull on the ground.".into(),
        Category::Fx => "A visual effect (energy, aura, fire, smoke, impact) as glowing stylized shapes: use mat(..., emit=3.0). About 1 to 3 m.".into(),
        _ => "A prop at real-world size, resting on the ground (or as it would be held, with its grip near the origin).".into(),
    };
    format!(
        "Asset: {}\nCategory: {}\nWhat it looks like: {}\nTags: {}\nThe images are the manga panels it appears in.\n{rules}\n\n{HELPERS}",
        need.name,
        need.category.dir(),
        need.description,
        need.tags.join(", ")
    )
}

/// A stand-in that always builds: a room or open ground for locations, a block for things.
pub(crate) fn fallback_script(need: &AssetNeed) -> String {
    let words = format!("{} {}", need.name, need.description).to_lowercase();
    match need.category {
        Category::Environments if need.interior == Some(false) || ["forest", "street", "field", "mountain", "outside", "road", "village", "yard"].iter().any(|w| words.contains(w)) => {
            "build_outdoor(size=40, features=14)\n".into()
        }
        Category::Environments => "build_room(width=16, depth=14, height=4.5)\n".into(),
        Category::Fx => "sphere('glow', r=0.8, loc=(0, 0, 1.0), material=mat('glow', '#7fd4ff', emit=4.0))\n".into(),
        Category::Vehicles => "box('body', (1.8, 4.2, 1.2), (0, 0, 0.8), color='#5a6a80', bevel=0.1)\nfor x in (-0.8, 0.8):\n    for y in (-1.4, 1.4):\n        cyl('wheel', r=0.35, depth=0.25, loc=(x, y, 0.35), rot=(0, 90, 0), color='#151515')\n".into(),
        _ => "box('block', (0.5, 0.5, 0.5), (0, 0, 0.25), color='#8a7a6a', bevel=0.03)\n".into(),
    }
}
