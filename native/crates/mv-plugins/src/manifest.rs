use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

fn default_version() -> String {
    "0.0.0".into()
}
fn default_lang() -> String {
    "en".into()
}
fn default_entry() -> String {
    "index.js".into()
}

/// `manifest.json` of a plugin. Unknown fields (engine, template_version, ...) are kept in `extra`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default = "default_lang")]
    pub lang: String,
    #[serde(default = "default_entry")]
    pub entry_point: String,
    #[serde(default)]
    pub nsfw: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl PluginManifest {
    pub fn parse(text: &str) -> Result<Self> {
        let m: PluginManifest =
            serde_json::from_str(text).context("Invalid manifest.json: could not parse")?;
        if m.id.is_empty() || m.name.is_empty() {
            anyhow::bail!("Invalid manifest: missing required fields (id, name)");
        }
        Ok(m)
    }
}

/// Read `<dir>/manifest.json`.
pub fn read_manifest(dir: impl AsRef<Path>) -> Result<PluginManifest> {
    let p = dir.as_ref().join("manifest.json");
    let text = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
    PluginManifest::parse(&text)
}
