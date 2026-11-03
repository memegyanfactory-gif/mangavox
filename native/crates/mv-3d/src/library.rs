//! The reusable asset library: `library/index.json` plus one folder per asset under its category
//! (`characters/`, `environments/`, `maps/`, `props/`, `vehicles/`, `fx/`, `skies/`). Each asset
//! knows what it is (name, description, tags), where it belongs (its manga, or shared), which
//! manga panels it was made from, and its `.blend` and preview render. Later scenes and chapters
//! find it here instead of building it again.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::util::{now, read_json, slug, tokens, write_json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Characters,
    Environments,
    Maps,
    #[default]
    Props,
    Vehicles,
    Fx,
    /// Sky / lighting images (HDRIs) for exteriors.
    Skies,
}

impl Category {
    pub const ALL: [Category; 7] =
        [Category::Characters, Category::Environments, Category::Maps, Category::Props, Category::Vehicles, Category::Fx, Category::Skies];

    /// Folder name inside the library.
    pub fn dir(self) -> &'static str {
        match self {
            Category::Characters => "characters",
            Category::Environments => "environments",
            Category::Maps => "maps",
            Category::Props => "props",
            Category::Vehicles => "vehicles",
            Category::Fx => "fx",
            Category::Skies => "skies",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Category::Characters => "Characters",
            Category::Environments => "Locations",
            Category::Maps => "World maps",
            Category::Props => "Props",
            Category::Vehicles => "Vehicles",
            Category::Fx => "Effects",
            Category::Skies => "Skies",
        }
    }

    /// What an AI answer calls the category ("character", "location", "vehicle", ...).
    pub fn parse(s: &str) -> Option<Category> {
        let s = s.trim().to_lowercase();
        let s = s.strip_suffix('s').unwrap_or(&s);
        Some(match s {
            "character" | "person" | "creature" | "cast" => Category::Characters,
            "environment" | "location" | "set" | "place" | "building" => Category::Environments,
            "map" | "world" | "world_map" => Category::Maps,
            "prop" | "object" | "item" | "weapon" => Category::Props,
            "vehicle" | "car" | "ship" | "mount" => Category::Vehicles,
            "fx" | "effect" | "vfx" | "magic" => Category::Fx,
            "sky" | "skie" | "hdri" => Category::Skies,
            _ => return None,
        })
    }

    /// Characters, locations and maps belong to one manga; props, vehicles, effects and skies
    /// can be shared by every manga.
    pub fn per_manga(self) -> bool {
        matches!(self, Category::Characters | Category::Environments | Category::Maps)
    }
}

/// How an asset was made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Origin {
    /// A Blender script the AI wrote from the manga's panels.
    #[default]
    Procedural,
    /// The parametric character builder (`mv3d.build_character`) with AI-picked parameters.
    Character,
    /// Built-in fallback when the AI's script kept failing.
    Template,
    /// Downloaded from a free library.
    Online { source: String, url: String, license: String },
    /// A 3D model generated from a reference image by an image-to-3D service.
    ImageTo3d { service: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Asset {
    pub id: String,
    pub name: String,
    pub category: Category,
    /// What it looks like, for the AI and for people.
    pub description: String,
    pub tags: Vec<String>,
    /// `None`: shared by every manga.
    pub manga_id: Option<String>,
    pub manga_title: String,
    /// Folder relative to the library root (`characters/ganghyeok-choi`).
    pub dir: String,
    /// The manga panels it was modelled from (copied into `<dir>/refs`).
    pub source_panels: Vec<String>,
    /// Bounding size in metres (x, y, z) as built.
    pub dims: [f32; 3],
    pub origin: Origin,
    /// Builder input: the character parameters or the world map's locations.
    pub params: Option<serde_json::Value>,
    /// Chapters whose scenes use it.
    pub used_in: Vec<String>,
    pub version: u32,
    pub created: i64,
    pub updated: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Index {
    assets: Vec<Asset>,
}

#[derive(Debug, Clone)]
pub struct Library {
    pub root: PathBuf,
    pub assets: Vec<Asset>,
    /// Assets this copy changed since it was loaded; [`Library::save`] merges only these.
    dirty: HashSet<String>,
}

/// Several runs (different chapters, or the app and a test runner) share one index: every save
/// re-reads it and merges under this lock, so no run drops what another one added.
static INDEX_LOCK: Mutex<()> = Mutex::new(());

impl Library {
    pub fn open(root: impl Into<PathBuf>) -> Library {
        let root = root.into();
        let idx: Index = read_json(&root.join("index.json")).unwrap_or_default();
        Library { root, assets: idx.assets, dirty: HashSet::new() }
    }

    /// A library not read yet (shown empty until the real one loads).
    pub fn empty(root: impl Into<PathBuf>) -> Library {
        Library { root: root.into(), assets: vec![], dirty: HashSet::new() }
    }

    /// The app's library (`<data>/studio/3d/library`).
    pub fn open_default() -> Library {
        Library::open(crate::paths::library())
    }

    /// Writes this copy's changes into the index on disk, keeping assets other runs added meanwhile,
    /// and refreshes this copy with the merged result.
    pub fn save(&mut self) -> Result<()> {
        let _g = INDEX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let path = self.root.join("index.json");
        let mut disk: Index = read_json(&path).unwrap_or_default();
        for id in &self.dirty {
            let Some(mine) = self.assets.iter().find(|a| &a.id == id) else { continue };
            match disk.assets.iter_mut().find(|a| &a.id == id) {
                Some(theirs) => {
                    let mut merged = mine.clone();
                    if mine.category == Category::Maps {
                        merge_pins(&mut merged, theirs);
                    }
                    for c in &theirs.used_in {
                        if !merged.used_in.contains(c) {
                            merged.used_in.push(c.clone());
                        }
                    }
                    *theirs = merged;
                }
                None => disk.assets.push(mine.clone()),
            }
        }
        write_json(&path, &disk)?;
        self.assets = disk.assets;
        self.dirty.clear();
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.id == id)
    }

    pub fn dir_of(&self, a: &Asset) -> PathBuf {
        self.root.join(&a.dir)
    }

    pub fn blend_of(&self, a: &Asset) -> PathBuf {
        self.dir_of(a).join("asset.blend")
    }

    pub fn preview_of(&self, a: &Asset) -> PathBuf {
        self.dir_of(a).join("preview.png")
    }

    /// The HDRI of a sky asset.
    pub fn sky_file(&self, a: &Asset) -> Option<PathBuf> {
        let rd = std::fs::read_dir(self.dir_of(a)).ok()?;
        rd.flatten().map(|e| e.path()).find(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("hdr") || e.eq_ignore_ascii_case("exr")))
    }

    /// The asset is on disk and usable.
    pub fn ready(&self, a: &Asset) -> bool {
        if a.category == Category::Skies {
            return self.sky_file(a).is_some();
        }
        self.blend_of(a).is_file()
    }

    pub fn in_category(&self, c: Category) -> Vec<&Asset> {
        self.assets.iter().filter(|a| a.category == c).collect()
    }

    /// An existing asset that is the same thing: same category, same manga for per-manga
    /// categories, and the same name (or nearly: every word of the shorter name in the other).
    pub fn find_match(&self, category: Category, name: &str, manga_id: &str) -> Option<&Asset> {
        let want = tokens(name);
        if want.is_empty() {
            return None;
        }
        let mut best: Option<(f32, &Asset)> = None;
        for a in self.assets.iter().filter(|a| a.category == category && self.ready(a)) {
            if category.per_manga() && a.manga_id.as_deref() != Some(manga_id) {
                continue;
            }
            let have = tokens(&a.name);
            let score = similarity(&want, &have);
            // Shared props of another manga need an exact match; one manga's own things a close one.
            let need = if a.manga_id.as_deref() == Some(manga_id) { 0.75 } else { 0.99 };
            if score >= need && best.is_none_or(|(s, _)| score > s) {
                best = Some((score, a));
            }
        }
        best.map(|(_, a)| a)
    }

    /// A fresh id for a new asset: `<slug>` or `<slug>-2` ...
    pub fn new_id(&self, name: &str, manga_title: &str, category: Category) -> String {
        let base = if category.per_manga() && !manga_title.is_empty() {
            format!("{}-{}", slug(&manga_title.chars().take(16).collect::<String>()), slug(name))
        } else {
            slug(name)
        };
        // Under the index lock, the new folder is created right away: two runs building the same
        // name at once get two ids instead of sharing one folder.
        let _g = INDEX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let on_disk: Index = read_json(&self.root.join("index.json")).unwrap_or_default();
        let taken = |id: &str| self.get(id).is_some() || on_disk.assets.iter().any(|a| a.id == id) || self.root.join(category.dir()).join(id).exists();
        let mut id = base.clone();
        let mut n = 2;
        while taken(&id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        let _ = std::fs::create_dir_all(self.root.join(category.dir()).join(&id));
        id
    }

    /// Adds or replaces an asset (same id) and saves the index.
    pub fn put(&mut self, mut a: Asset) -> Result<()> {
        a.updated = now();
        if a.created == 0 {
            a.created = a.updated;
        }
        self.dirty.insert(a.id.clone());
        match self.assets.iter_mut().find(|x| x.id == a.id) {
            Some(x) => *x = a,
            None => self.assets.push(a),
        }
        self.save()
    }

    pub fn mark_used(&mut self, id: &str, chapter_id: &str) {
        if let Some(a) = self.assets.iter_mut().find(|a| a.id == id) {
            if !a.used_in.iter().any(|c| c == chapter_id) {
                a.used_in.push(chapter_id.to_string());
                self.dirty.insert(id.to_string());
            }
        }
    }

    /// Short listing for the AI: one line per asset it may reuse in this manga.
    pub fn catalog_for(&self, manga_id: &str) -> String {
        let mut out = String::new();
        for a in &self.assets {
            if !self.ready(a) || (a.category.per_manga() && a.manga_id.as_deref() != Some(manga_id)) {
                continue;
            }
            out.push_str(&format!("- id={} [{}] {}: {}\n", a.id, a.category.dir(), a.name, crate::util::truncate(&a.description, 90)));
        }
        out
    }
}

/// A world map saved by two runs keeps every pin: the other copy's locations missing from
/// this one are added (by name), and the newer version number wins.
fn merge_pins(mine: &mut Asset, theirs: &Asset) {
    let pins = |a: &Asset| a.params.as_ref().and_then(|p| p.get("locations")).and_then(|l| l.as_array()).cloned().unwrap_or_default();
    let name = |p: &serde_json::Value| p.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_lowercase();
    let mut all = pins(mine);
    for p in pins(theirs) {
        if !all.iter().any(|q| name(q) == name(&p)) {
            all.push(p);
        }
    }
    for t in &theirs.tags {
        if !mine.tags.contains(t) {
            mine.tags.push(t.clone());
        }
    }
    mine.version = mine.version.max(theirs.version);
    mine.params = Some(serde_json::json!({ "locations": all }));
}

fn similarity(a: &[String], b: &[String]) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let hit = short.iter().filter(|w| long.contains(w)).count();
    if hit < short.len() {
        return hit as f32 / short.len() as f32 * 0.7;
    }
    // Every word of the shorter name is in the longer one; equal length is a perfect match.
    0.75 + 0.25 * short.len() as f32 / long.len() as f32
}

/// Copies the reference panels into the asset folder and returns their file names.
pub(crate) fn copy_refs(dir: &Path, panels: &[PathBuf]) -> Vec<String> {
    let refs = dir.join("refs");
    let _ = std::fs::create_dir_all(&refs);
    let mut out = vec![];
    for (i, p) in panels.iter().enumerate() {
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("jpg");
        let name = format!("ref_{i:02}.{ext}");
        if std::fs::copy(p, refs.join(&name)).is_ok() {
            out.push(format!("refs/{name}"));
        }
    }
    out
}
