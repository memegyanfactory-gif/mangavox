//! `~/.mangavox/config.json` as a loosely-typed JSON object (unknown keys are preserved).
use anyhow::Result;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct Config {
    path: PathBuf,
    lock: Mutex<()>,
}

impl Config {
    /// Config at the default location (`paths::config()`).
    pub fn open_default() -> Self {
        Self::at(crate::paths::config())
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into(), lock: Mutex::new(()) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read(&self) -> Map<String, Value> {
        match std::fs::read_to_string(&self.path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
            Some(Value::Object(m)) => m,
            _ => Map::new(), // missing or corrupted -> defaults, like the Electron app
        }
    }

    fn write(&self, map: &Map<String, Value>) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_string_pretty(map)?)?;
        if let Err(e) = std::fs::rename(&tmp, &self.path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    /// The whole config object.
    pub fn get_all(&self) -> Value {
        let _g = self.lock.lock().unwrap();
        Value::Object(self.read())
    }

    /// A top-level key (`None` when absent or null).
    pub fn get(&self, key: &str) -> Option<Value> {
        let _g = self.lock.lock().unwrap();
        self.read().remove(key).filter(|v| !v.is_null())
    }

    /// Sets a top-level key. `trakt` is shallow-merged into the existing object, like the IPC handler.
    pub fn set(&self, key: &str, value: Value) -> Result<()> {
        let _g = self.lock.lock().unwrap();
        let mut map = self.read();
        let new = if key == "trakt" {
            let mut merged = match map.remove(key) {
                Some(Value::Object(m)) => m,
                _ => Map::new(),
            };
            if let Value::Object(v) = value {
                merged.extend(v);
            }
            Value::Object(merged)
        } else {
            value
        };
        map.insert(key.to_string(), new);
        self.write(&map)
    }

    /// Sets a key verbatim (no merge) — for services that own a key such as `aiAssistant`.
    pub fn set_raw(&self, key: &str, value: Value) -> Result<()> {
        let _g = self.lock.lock().unwrap();
        let mut map = self.read();
        map.insert(key.to_string(), value);
        self.write(&map)
    }

    pub fn remove(&self, key: &str) -> Result<()> {
        let _g = self.lock.lock().unwrap();
        let mut map = self.read();
        if map.remove(key).is_some() {
            self.write(&map)?;
        }
        Ok(())
    }

    /// The saved [`RECAP_MUSIC_LEVEL`] (1, as rendered, when nothing was saved).
    pub fn recap_music_level(&self) -> f64 {
        music_level_of(self.get(RECAP_MUSIC_LEVEL).as_ref())
    }

    /// Saves [`RECAP_MUSIC_LEVEL`], kept to 0..=1 in whole percents.
    pub fn set_recap_music_level(&self, level: f64) -> Result<()> {
        let level = if level.is_finite() { (level.clamp(0.0, 1.0) * 100.0).round() / 100.0 } else { 1.0 };
        self.set(RECAP_MUSIC_LEVEL, Value::from(level))
    }
}

/// How loud a recap's background music plays, from 0 (off) to 1 (as Studio rendered it). Recap
/// videos carry two sound tracks, "Narration + music" and "Narration only" (the same narration on
/// both): the player mixes the two to this level while one plays, and Studio's exports bake it
/// in, so what is heard while watching is what gets exported.
pub const RECAP_MUSIC_LEVEL: &str = "recap.musicLevel";

/// A stored [`RECAP_MUSIC_LEVEL`]: a number, kept to 0..=1; anything else (or nothing) is 1.
pub fn music_level_of(v: Option<&Value>) -> f64 {
    v.and_then(Value::as_f64).filter(|x| x.is_finite()).map(|x| x.clamp(0.0, 1.0)).unwrap_or(1.0)
}

/// The app's [`RECAP_MUSIC_LEVEL`] in whole percents once read (`u32::MAX`: not yet): the player
/// sets it and Studio's export panel shows it every frame, without reading the file each time.
static MUSIC_LEVEL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(u32::MAX);

/// The app's recap music level (see [`RECAP_MUSIC_LEVEL`]).
pub fn music_level() -> f64 {
    use std::sync::atomic::Ordering;
    match MUSIC_LEVEL.load(Ordering::Relaxed) {
        u32::MAX => {
            let level = Config::open_default().recap_music_level();
            MUSIC_LEVEL.store((level * 100.0).round() as u32, Ordering::Relaxed);
            level
        }
        p => p as f64 / 100.0,
    }
}

/// Saves the app's recap music level (blocking: it writes the config file).
pub fn save_music_level(level: f64) -> Result<()> {
    let c = Config::open_default();
    c.set_recap_music_level(level)?;
    MUSIC_LEVEL.store((c.recap_music_level() * 100.0).round() as u32, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn roundtrip_preserves_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("config.json");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, r#"{"future":{"a":[1,2]},"theme":"dark"}"#).unwrap();
        let c = Config::at(&p);
        assert_eq!(c.get("theme"), Some(json!("dark")));
        c.set("theme", json!("light")).unwrap();
        c.set("new", json!(3)).unwrap();
        assert_eq!(c.get("future"), Some(json!({"a":[1,2]})));
        assert_eq!(c.get("theme"), Some(json!("light")));
        assert_eq!(c.get("missing"), None);
        let again = Config::at(&p);
        assert_eq!(again.get("new"), Some(json!(3)));
        assert!(!std::fs::read_dir(p.parent().unwrap()).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().ends_with(".tmp")));
    }

    #[test]
    fn missing_and_corrupt_files_give_defaults_and_trakt_merges() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.json");
        let c = Config::at(&p);
        assert_eq!(c.get_all(), json!({}));
        std::fs::write(&p, "{not json").unwrap();
        assert_eq!(c.get("x"), None);
        c.set("trakt", json!({"token":"t"})).unwrap();
        c.set("trakt", json!({"user":"u"})).unwrap();
        assert_eq!(c.get("trakt"), Some(json!({"token":"t","user":"u"})));
        c.remove("trakt").unwrap();
        assert_eq!(c.get("trakt"), None);
    }

    #[test]
    fn recap_music_level_is_saved_in_whole_percents() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::at(dir.path().join("config.json"));
        assert_eq!(c.recap_music_level(), 1.0, "as rendered until the viewer changes it");
        c.set_recap_music_level(0.504).unwrap();
        assert_eq!(c.recap_music_level(), 0.5);
        assert_eq!(c.get(RECAP_MUSIC_LEVEL), Some(json!(0.5)));
        c.set_recap_music_level(-3.0).unwrap();
        assert_eq!(c.recap_music_level(), 0.0, "off");
        c.set_recap_music_level(f64::NAN).unwrap();
        assert_eq!(c.recap_music_level(), 1.0);
        assert_eq!(music_level_of(Some(&json!(7))), 1.0);
        assert_eq!(music_level_of(Some(&json!("loud"))), 1.0);
        assert_eq!(music_level_of(None), 1.0);
    }
}
