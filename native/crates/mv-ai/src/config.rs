//! Tiny JSON config store: reads/writes the shared `config.json` while leaving unrelated keys untouched.

use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static WRITE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The whole config as an object; a missing or corrupt file reads as empty.
    pub fn read(&self) -> Map<String, Value> {
        match std::fs::read_to_string(&self.path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
            Some(Value::Object(m)) => m,
            _ => Map::new(),
        }
    }

    /// Read-modify-write the config.
    pub fn update(&self, f: impl FnOnce(&mut Map<String, Value>)) -> std::io::Result<()> {
        let _g = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut cfg = self.read();
        f(&mut cfg);
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(&Value::Object(cfg)).unwrap_or_default())
    }
}

/// `obj[key]` as an object, creating it when absent or of the wrong type.
pub fn object_entry<'a>(map: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    let v = map.entry(key.to_string()).or_insert_with(|| Value::Object(Map::new()));
    if !v.is_object() {
        *v = Value::Object(Map::new());
    }
    v.as_object_mut().unwrap()
}
