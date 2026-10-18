//! Installed Stremio addon registry persisted like the Electron app
//! (`<root>/addons/<id>/manifest.json`, with `_sourceUrl`), plus ranked stream search.
//! Mirrors `src/main/addons/addon-loader.ts`. A `_disabled` flag is added for the native app;
//! the Electron loader ignores unknown keys.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::task::JoinSet;
use tokio::time::{sleep, Sleep};

use crate::error::{MediaError, Result};
use crate::stream_utils::{has_startable, rank_streams};
use crate::stremio::StremioClient;
use crate::types::{AnimeAddonManifest, AnimeTorrentStream};

const CACHE_TTL: Duration = Duration::from_secs(30 * 60);
/// After the first startable source arrives, wait this long for better ones.
const GRACE: Duration = Duration::from_millis(1500);
/// Answer with whatever we have after this, even if addons are still running.
const DEADLINE: Duration = Duration::from_secs(9);

#[derive(Debug, Clone, PartialEq)]
pub struct InstalledAddon {
    pub manifest: AnimeAddonManifest,
    /// URL the addon was installed from.
    pub source_url: String,
    pub enabled: bool,
}

type StreamCache = Mutex<HashMap<String, (Instant, Vec<AnimeTorrentStream>)>>;

pub struct AddonRegistry {
    dir: PathBuf,
    items: Mutex<Vec<InstalledAddon>>,
    cache: StreamCache,
}

fn sort_items(items: &mut [InstalledAddon]) {
    items.sort_by_key(|a| a.manifest.name.to_lowercase());
}

impl AddonRegistry {
    /// Open the registry rooted at `dir` (normally `~/.mangavox/addons`) and load what is installed.
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        let reg = Self { dir: dir.into(), items: Mutex::new(Vec::new()), cache: Mutex::new(HashMap::new()) };
        reg.reload();
        reg
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn items(&self) -> std::sync::MutexGuard<'_, Vec<InstalledAddon>> {
        self.items.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn clear_cache(&self) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// Re-read every `<dir>/*/manifest.json`. Unreadable folders are skipped.
    pub fn reload(&self) {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for entry in rd.flatten() {
                let Ok(text) = std::fs::read_to_string(entry.path().join("manifest.json")) else { continue };
                let Ok(raw) = serde_json::from_str::<Value>(&text) else { continue };
                let source_url = raw.get("_sourceUrl").and_then(Value::as_str).map(str::to_owned);
                let enabled = !raw.get("_disabled").and_then(Value::as_bool).unwrap_or(false);
                let Ok(manifest) = serde_json::from_value::<AnimeAddonManifest>(raw) else { continue };
                // Older builds wrote a built-in "hindi4u" scraper with no Stremio endpoint.
                if manifest.id == "hindi4u" {
                    let _ = std::fs::remove_dir_all(entry.path());
                    continue;
                }
                let source_url = source_url.unwrap_or_else(|| manifest.endpoint.clone());
                out.push(InstalledAddon { manifest, source_url, enabled });
            }
        }
        sort_items(&mut out);
        *self.items() = out;
        self.clear_cache();
    }

    pub fn list(&self) -> Vec<InstalledAddon> {
        self.items().clone()
    }

    fn folder(&self, id: &str) -> Result<PathBuf> {
        if id.is_empty() || id.contains(['/', '\\']) || id == "." || id == ".." {
            return Err(MediaError::InvalidManifest(format!("unsafe addon id: {id}")));
        }
        Ok(self.dir.join(id))
    }

    fn write(&self, addon: &InstalledAddon) -> Result<()> {
        let dir = self.folder(&addon.manifest.id)?;
        let io = |e: std::io::Error| MediaError::InvalidManifest(e.to_string());
        std::fs::create_dir_all(&dir).map_err(io)?;
        let mut v = serde_json::to_value(&addon.manifest).unwrap_or(Value::Null);
        if let Value::Object(m) = &mut v {
            m.insert("_sourceUrl".into(), Value::String(addon.source_url.clone()));
            if !addon.enabled {
                m.insert("_disabled".into(), Value::Bool(true));
            }
        }
        std::fs::write(dir.join("manifest.json"), serde_json::to_string_pretty(&v).unwrap_or_default()).map_err(io)
    }

    /// Fetch the manifest at `url` and install (or replace) the addon.
    pub async fn install(&self, url: &str) -> Result<InstalledAddon> {
        let manifest = StremioClient::new()?.fetch_manifest(url.trim()).await?;
        self.insert(manifest, url.trim().to_string())
    }

    /// Persist an already-fetched manifest.
    pub fn insert(&self, manifest: AnimeAddonManifest, source_url: String) -> Result<InstalledAddon> {
        let addon = InstalledAddon { manifest, source_url, enabled: true };
        self.write(&addon)?;
        {
            let mut items = self.items();
            items.retain(|a| a.manifest.id != addon.manifest.id);
            items.push(addon.clone());
            sort_items(&mut items);
        }
        self.clear_cache();
        Ok(addon)
    }

    pub fn uninstall(&self, id: &str) -> Result<()> {
        let dir = self.folder(id)?;
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        self.items().retain(|a| a.manifest.id != id);
        self.clear_cache();
        Ok(())
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let updated = {
            let mut items = self.items();
            items.iter_mut().find(|a| a.manifest.id == id).map(|a| {
                a.enabled = enabled;
                a.clone()
            })
        };
        if let Some(a) = updated {
            self.write(&a)?;
            self.clear_cache();
        }
        Ok(())
    }

    /// Enabled addons that serve streams for `kind` (`listStreamAddons`).
    pub fn stream_addons(&self, kind: &str) -> Vec<AnimeAddonManifest> {
        self.list()
            .into_iter()
            .filter(|a| a.enabled)
            .map(|a| a.manifest)
            .filter(|m| m.resources.iter().any(|r| r == "stream") && (m.types.is_empty() || m.types.iter().any(|t| t == kind)))
            .collect()
    }

    /// Query every stream addon for `resource_id` (`tt…` for movies, `tt…:S:E` for episodes) and
    /// return the ranked, de-duplicated playable streams. Answers shortly after a startable source
    /// shows up, or after a hard deadline. Non-empty complete results are cached for 30 minutes.
    pub async fn find_streams(&self, resource_id: &str, kind: &str) -> Vec<AnimeTorrentStream> {
        let key = format!("{kind}:{resource_id}");
        if let Some((at, s)) = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            if at.elapsed() < CACHE_TTL {
                return s.clone();
            }
        }
        let providers = self.stream_addons(kind);
        let (ranked, complete) = search_streams(providers, resource_id, kind).await;
        if complete && !ranked.is_empty() {
            self.cache.lock().unwrap_or_else(|e| e.into_inner()).insert(key, (Instant::now(), ranked.clone()));
        }
        ranked
    }
}

/// Returns (ranked streams, whether every addon answered).
async fn search_streams(providers: Vec<AnimeAddonManifest>, resource_id: &str, kind: &str) -> (Vec<AnimeTorrentStream>, bool) {
    let Ok(client) = StremioClient::new() else { return (Vec::new(), true) };
    let client = Arc::new(client);
    let mut set = JoinSet::new();
    for addon in providers {
        let (client, id, kind) = (client.clone(), resource_id.to_string(), kind.to_string());
        set.spawn(async move {
            let streams = client.fetch_streams(&addon.endpoint, &id, &kind).await?;
            Ok::<_, MediaError>(
                streams
                    .into_iter()
                    .map(|mut s| {
                        if s.source.as_deref().unwrap_or("").is_empty() {
                            s.source = Some(addon.name.clone());
                        }
                        s
                    })
                    .collect::<Vec<_>>(),
            )
        });
    }
    let mut collected: Vec<AnimeTorrentStream> = Vec::new();
    let deadline = sleep(DEADLINE);
    tokio::pin!(deadline);
    let mut grace: Option<Pin<Box<Sleep>>> = None;
    let mut complete = true;
    loop {
        tokio::select! {
            r = set.join_next() => match r {
                None => break,
                Some(Ok(Ok(streams))) => {
                    collected.extend(streams);
                    if grace.is_none() && has_startable(&collected) {
                        grace = Some(Box::pin(sleep(GRACE)));
                    }
                }
                Some(_) => {}
            },
            _ = &mut deadline => { complete = false; break }
            _ = async {
                match grace.as_mut() {
                    Some(g) => g.await,
                    None => std::future::pending().await,
                }
            } => { complete = false; break }
        }
    }
    (rank_streams(collected), complete)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str, resources: &[&str], types: &[&str]) -> AnimeAddonManifest {
        AnimeAddonManifest {
            id: id.into(),
            name: format!("Addon {id}"),
            version: "1.0.0".into(),
            description: None,
            icon: None,
            catalogs: vec![],
            endpoint: format!("https://{id}.example"),
            resources: resources.iter().map(|s| s.to_string()).collect(),
            types: types.iter().map(|s| s.to_string()).collect(),
            id_prefixes: vec![],
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("mv-addons-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn persist_reload_disable_remove() {
        let dir = tmp("reg");
        let reg = AddonRegistry::open(&dir);
        assert!(reg.list().is_empty());
        reg.insert(manifest("a", &["stream"], &["movie"]), "https://a.example/manifest.json".into()).unwrap();
        reg.insert(manifest("b", &["catalog"], &[]), "https://b.example/manifest.json".into()).unwrap();
        assert_eq!(reg.stream_addons("movie").len(), 1);
        assert!(reg.stream_addons("series").is_empty());

        reg.set_enabled("a", false).unwrap();
        assert!(reg.stream_addons("movie").is_empty());

        let again = AddonRegistry::open(&dir);
        let list = again.list();
        assert_eq!(list.len(), 2);
        let a = list.iter().find(|x| x.manifest.id == "a").unwrap();
        assert!(!a.enabled);
        assert_eq!(a.source_url, "https://a.example/manifest.json");

        again.uninstall("a").unwrap();
        assert_eq!(AddonRegistry::open(&dir).list().len(), 1);
        assert!(again.insert(manifest("../x", &[], &[]), String::new()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_streams_with_no_addons_is_empty() {
        let reg = AddonRegistry::open(tmp("none"));
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        assert!(rt.block_on(reg.find_streams("tt1", "movie")).is_empty());
    }
}
