//! Config and filesystem access for the Settings page. Everything here blocks;
//! callers go through `mv_ui::bg::blocking`. Keys mirror the Electron app so both
//! apps can share `~/.mangavox/config.json`.
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use mv_core::config::Config;
use serde_json::{json, Map, Value};

pub const DEFAULT_PRESET: &str = "rust";

/// Theme presets from src/shared/appearance.ts (ids, copy and the three swatch colours).
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub base: u32,
    pub accent: u32,
    pub secondary: u32,
}

pub const PRESETS: &[Preset] = &[
    Preset { id: "rust", name: "Rust", description: "Oxidized orange on warm charcoal — forged metal", base: 0x0e0907, accent: 0xc8501a, secondary: 0xf0b27a },
    Preset { id: "watchfiwn", name: "Watchfiwn", description: "Cinematic red on true black — streaming-grade", base: 0x0b0b0c, accent: 0xe8392e, secondary: 0xff6b5f },
    Preset { id: "mangavox-emerald", name: "MangaVox Emerald", description: "Emerald and cyan studio lighting", base: 0x07090d, accent: 0x10b981, secondary: 0x60a5fa },
    Preset { id: "chatgpt-green", name: "ChatGPT Green", description: "Charcoal, mint, and connected signals", base: 0x080b0a, accent: 0x10a37f, secondary: 0x9ce6d1 },
    Preset { id: "claude-warm", name: "Claude Warm", description: "Warm terracotta on deep charcoal", base: 0x0d0b0a, accent: 0xd97757, secondary: 0xe8c7a8 },
    Preset { id: "gemini-prism", name: "Gemini Prism", description: "Blue, violet, and cyan refractions", base: 0x080a12, accent: 0x5b8def, secondary: 0xa78bfa },
    Preset { id: "amber-terminal", name: "Amber Terminal", description: "Black glass and restrained amber data", base: 0x090806, accent: 0xf59e0b, secondary: 0xfde68a },
    Preset { id: "cyberpunk", name: "Cyberpunk", description: "Cyan and magenta over graphite", base: 0x07090c, accent: 0x22d3ee, secondary: 0xf472b6 },
    Preset { id: "sakura", name: "Sakura", description: "Rose highlights and drifting petals", base: 0x0d090c, accent: 0xfb7185, secondary: 0xf9a8d4 },
    Preset { id: "ocean", name: "Ocean", description: "Deep neutral water and clear cyan", base: 0x060b0e, accent: 0x06b6d4, secondary: 0x38bdf8 },
    Preset { id: "monochrome", name: "Monochrome", description: "Graphite, silver, and quiet motion", base: 0x090a0b, accent: 0xb8c0cc, secondary: 0x7f8ea3 },
    Preset { id: "aurora", name: "Aurora", description: "Green, blue, and violet circular flow", base: 0x070a0c, accent: 0x2dd4bf, secondary: 0x818cf8 },
];

pub const QUALITIES: &[&str] = &["Any", "720p", "1080p", "4K"];
pub const TIMERS: &[u64] = &[10, 15, 20, 30, 45, 60];
/// (ISO 639-1 code, label)
pub const SUBTITLE_LANGS: &[(&str, &str)] = &[
    ("en", "English"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("de", "German"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("ru", "Russian"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("zh", "Chinese"),
    ("hi", "Hindi"),
    ("ar", "Arabic"),
];

/// (config key, label, hint) for the four download folders, in display order.
pub const STORAGE_KINDS: &[(&str, &str, &str)] = &[
    ("anime", "Anime downloads", "Saved as <Title>/<Title - S01E05>.mkv"),
    ("movies", "Movies & series", "Movies: <Title (Year)>/, series: <Title>/Season 01/"),
    ("manga", "Manga library", "Applies to new downloads; existing chapters stay where they are."),
    ("subtitles", "Subtitles", "A copy of every downloaded subtitle."),
];

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: String,
    pub email: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Playback {
    pub auto_play: bool,
    pub auto_play_timer: u64,
    pub pre_download_next: bool,
    pub preferred_quality: String,
    pub skip_intro: bool,
    pub subtitle_language: String,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            auto_play: true,
            auto_play_timer: 20,
            pre_download_next: true,
            preferred_quality: "1080p".into(),
            skip_intro: false,
            subtitle_language: "en".into(),
        }
    }
}

/// Everything the page needs, read in one background pass.
pub struct Snapshot {
    pub profile: Profile,
    pub playback: Playback,
    pub preset: String,
    pub reduce_motion: bool,
    pub mode: String,
    /// Custom folder per `STORAGE_KINDS` entry; empty string means the default folder.
    pub storage: Vec<String>,
    pub cache_bytes: u64,
}

fn config() -> Config {
    Config::open_default()
}

fn object(cfg: &Config, key: &str) -> Map<String, Value> {
    match cfg.get(key) {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

/// Read-modify-write of one object-valued key, keeping its other fields.
pub(crate) fn update_object(key: &str, f: impl FnOnce(&mut Map<String, Value>)) -> Result<()> {
    let cfg = config();
    let mut m = object(&cfg, key);
    f(&mut m);
    cfg.set(key, Value::Object(m))
}

fn str_field(m: &Map<String, Value>, key: &str) -> Option<String> {
    m.get(key).and_then(Value::as_str).map(str::to_string)
}

pub fn load_snapshot() -> Snapshot {
    let cfg = config();
    let user = object(&cfg, "user");
    let name = str_field(&user, "name").filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "User".into());
    let profile = Profile { name, email: str_field(&user, "email").unwrap_or_default() };

    let pb = object(&cfg, "playback");
    let d = Playback::default();
    let playback = Playback {
        auto_play: pb.get("autoPlay").and_then(Value::as_bool).unwrap_or(d.auto_play),
        auto_play_timer: pb
            .get("autoPlayTimer")
            .and_then(Value::as_u64)
            .filter(|t| TIMERS.contains(t))
            .unwrap_or(d.auto_play_timer),
        pre_download_next: pb.get("preDownloadNext").and_then(Value::as_bool).unwrap_or(d.pre_download_next),
        preferred_quality: str_field(&pb, "preferredQuality")
            .filter(|q| QUALITIES.contains(&q.as_str()))
            .unwrap_or(d.preferred_quality),
        // Stored as their own top-level keys: the Electron PlaybackTab overwrites the
        // whole "playback" object on save and would drop anything else in it.
        skip_intro: cfg.get("playback.skipIntro").and_then(|v| v.as_bool()).unwrap_or(false),
        subtitle_language: cfg
            .get("playback.subtitleLanguage")
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or(d.subtitle_language),
    };

    let ap = object(&cfg, "appearance");
    let preset = str_field(&ap, "preset")
        .filter(|p| PRESETS.iter().any(|x| x.id == p))
        .unwrap_or_else(|| DEFAULT_PRESET.into());
    let reduce_motion = ap.get("reduceMotion").and_then(Value::as_bool).unwrap_or(false);

    let mode = match cfg.get("app.mode").and_then(|v| v.as_str().map(str::to_string)) {
        Some(m) if m == "studio" => m,
        _ => "entertainment".into(),
    };

    let storage_cfg = object(&cfg, "storage");
    let storage = STORAGE_KINDS
        .iter()
        .map(|(kind, _, _)| str_field(&storage_cfg, kind).map(|s| s.trim().to_string()).unwrap_or_default())
        .collect();

    Snapshot {
        profile,
        playback,
        preset,
        reduce_motion,
        mode,
        storage,
        cache_bytes: dir_size(&cache_dir()),
    }
}

pub fn save_profile(name: &str, email: &str) -> Result<()> {
    update_object("user", |m| {
        m.insert("name".into(), json!(name));
        m.insert("email".into(), json!(email));
    })
}

pub fn save_playback(p: &Playback) -> Result<()> {
    update_object("playback", |m| {
        m.insert("autoPlay".into(), json!(p.auto_play));
        m.insert("autoPlayTimer".into(), json!(p.auto_play_timer));
        m.insert("preDownloadNext".into(), json!(p.pre_download_next));
        m.insert("preferredQuality".into(), json!(p.preferred_quality));
    })?;
    let cfg = config();
    cfg.set("playback.skipIntro", json!(p.skip_intro))?;
    cfg.set("playback.subtitleLanguage", json!(p.subtitle_language))
}

pub fn save_preset(preset: &str) -> Result<()> {
    update_object("appearance", |m| {
        m.insert("preset".into(), json!(preset));
    })
}

pub fn save_reduce_motion(on: bool) -> Result<()> {
    update_object("appearance", |m| {
        m.insert("reduceMotion".into(), json!(on));
    })
}

/// Whether the saved `app.mode` is Studio (read once at startup by the shell).
pub fn studio_saved() -> bool {
    config().get("app.mode").and_then(|v| v.as_str().map(|s| s == "studio")).unwrap_or(false)
}

/// Studio defaults kept in `~/.mangavox/studio/prefs.json` (shared with mv-recap, which owns the
/// rest of the file; only these keys are touched).
#[derive(Clone, Debug, PartialEq)]
pub struct StudioPrefs {
    pub voice: String,
    pub speed: f64,
    pub resolution: String,
}

impl Default for StudioPrefs {
    fn default() -> Self {
        Self { voice: String::new(), speed: 1.0, resolution: "1080p".into() }
    }
}

fn studio_prefs_path() -> PathBuf {
    mv_core::paths::root().join("studio").join("prefs.json")
}

pub fn load_studio_prefs() -> StudioPrefs {
    let v: Value = std::fs::read_to_string(studio_prefs_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    let d = StudioPrefs::default();
    StudioPrefs {
        voice: v.get("voice").and_then(Value::as_str).unwrap_or(&d.voice).to_string(),
        speed: v.get("speed").and_then(Value::as_f64).unwrap_or(d.speed),
        resolution: v.get("resolution").and_then(Value::as_str).filter(|r| *r == "720p" || *r == "1080p").unwrap_or(&d.resolution).to_string(),
    }
}

pub fn save_studio_prefs(p: &StudioPrefs) -> Result<()> {
    save_studio_prefs_at(&studio_prefs_path(), p)
}

fn save_studio_prefs_at(path: &Path, p: &StudioPrefs) -> Result<()> {
    let mut root = match std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    };
    root.insert("voice".into(), json!(p.voice));
    root.insert("speed".into(), json!(p.speed));
    root.insert("resolution".into(), json!(p.resolution));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&Value::Object(root))?)?;
    Ok(())
}

pub fn save_mode(mode: &str) -> Result<()> {
    config().set("app.mode", json!(mode))
}

/// Default download folder for a storage kind: `library/<kind>` in the data folder.
pub fn storage_default(kind: &str) -> PathBuf {
    match kind {
        "manga" => mv_core::paths::manga_downloads(),
        "anime" | "movies" => mv_core::paths::library().join(kind),
        _ => mv_core::paths::library().join("subtitles"),
    }
}

/// Sets a custom folder for a kind. An empty path resets to the default. Returns the stored value.
pub fn save_storage(kind: &str, path: &str) -> Result<String> {
    let value = path.trim().to_string();
    if !value.is_empty() {
        std::fs::create_dir_all(&value).with_context(|| format!("Could not create folder {value}"))?;
    }
    update_object("storage", |m| {
        m.insert(kind.to_string(), json!(value));
    })?;
    Ok(value)
}

/// Sets the Model Hub folder (`aiAssistant.hub.modelsDir`); an empty path resets it.
pub fn save_models_dir(dir: &str) -> Result<()> {
    let dir = dir.trim();
    if !dir.is_empty() {
        std::fs::create_dir_all(dir).with_context(|| format!("Could not create folder {dir}"))?;
    }
    update_object("aiAssistant", |m| {
        let hub = m.entry("hub").or_insert_with(|| json!({}));
        if !hub.is_object() {
            *hub = json!({});
        }
        if let Some(h) = hub.as_object_mut() {
            if dir.is_empty() {
                h.remove("modelsDir");
            } else {
                h.insert("modelsDir".into(), json!(dir));
            }
        }
    })
}

pub fn data_dir() -> PathBuf {
    mv_core::paths::root()
}

pub fn cache_dir() -> PathBuf {
    data_dir().join("cache")
}

pub fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else { return 0 };
    entries
        .flatten()
        .map(|e| match e.metadata() {
            Ok(md) if md.is_dir() => dir_size(&e.path()),
            Ok(md) => md.len(),
            Err(_) => 0,
        })
        .sum()
}

/// Deletes ~/.mangavox/cache and recreates it empty. Library and downloads are untouched.
pub fn clear_cache() -> Result<()> {
    let dir = cache_dir();
    if dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("Could not remove {}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(())
}

#[cfg(test)]
mod studio_tests {
    use super::*;

    #[test]
    fn studio_prefs_preserve_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("studio").join("prefs.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"keep":{"a":1},"voice":"old"}"#).unwrap();
        save_studio_prefs_at(&path, &StudioPrefs { voice: "Aria".into(), speed: 1.25, resolution: "720p".into() }).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["keep"]["a"], 1);
        assert_eq!(v["voice"], "Aria");
        assert_eq!(v["speed"], 1.25);
        assert_eq!(v["resolution"], "720p");
    }
}
