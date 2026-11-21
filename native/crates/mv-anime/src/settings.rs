//! Engine-wide preferences in `<data>/anime/settings.json` (design 3.9). Never in Studio's
//! `prefs.json` or the app's `config.json`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Most LLM jobs that may run at once (the runner and the settings file agree on it).
pub const LLM_PARALLEL_MAX: u8 = 4;

/// Finished durations kept per job kind for the ETA.
pub const ETA_KEEP: usize = 20;

/// `anime/settings.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub schema: u32,
    /// LLM jobs that may run at once (1 to [`LLM_PARALLEL_MAX`]).
    pub llm_parallel: u8,
    /// Last finished durations (seconds) per `JobKind` variant name.
    pub eta: BTreeMap<String, Vec<f32>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { schema: crate::model::SCHEMA, llm_parallel: 1, eta: BTreeMap::new(), extra: Map::new() }
    }
}

/// `<data>/anime/settings.json`.
pub fn path() -> PathBuf {
    crate::store::root().join("settings.json")
}

/// The saved settings, or the defaults. A damaged file is set aside (see `store::read_json`)
/// and the defaults are used; values out of range are brought back into range.
pub fn load() -> Settings {
    let mut s: Settings = match crate::store::read_json(&path()) {
        Ok(s) => s,
        Err(e) => {
            mv_core::logging::global().warn("anime", &format!("Engine settings could not be read, using the defaults: {e:#}"));
            Settings::default()
        }
    };
    s.normalize();
    s
}

/// Writes the settings atomically. A file made by a newer MangaVox is not overwritten.
pub fn save(settings: &Settings) -> Result<()> {
    if settings.schema > crate::model::SCHEMA {
        bail!(crate::store::NEWER);
    }
    let mut s = settings.clone();
    s.schema = crate::model::SCHEMA;
    s.normalize();
    crate::store::write_json(&path(), &s)
}

impl Settings {
    /// Brings every value into its range: at least one LLM job, at most [`ETA_KEEP`] durations
    /// per kind, no negative or broken durations.
    pub fn normalize(&mut self) {
        self.llm_parallel = self.llm_parallel.clamp(1, LLM_PARALLEL_MAX);
        for v in self.eta.values_mut() {
            v.retain(|x| x.is_finite() && *x >= 0.0);
            if v.len() > ETA_KEEP {
                v.drain(..v.len() - ETA_KEEP);
            }
        }
        self.eta.retain(|_, v| !v.is_empty());
    }

    /// Adds a finished duration for a job kind (keeps the newest [`ETA_KEEP`]).
    pub fn record_eta(&mut self, kind: &str, seconds: f32) {
        if seconds.is_finite() && seconds >= 0.0 {
            self.eta.entry(kind.to_string()).or_default().push(seconds);
            self.normalize();
        }
    }

    /// Mean of the kept durations of a job kind, if any.
    pub fn eta_mean(&self, kind: &str) -> Option<f32> {
        let v = self.eta.get(kind).filter(|v| !v.is_empty())?;
        Some(v.iter().sum::<f32>() / v.len() as f32)
    }
}
