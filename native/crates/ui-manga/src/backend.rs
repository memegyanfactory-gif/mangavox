//! Async glue between the UI and the backend crates: the shared source registry
//! (built-in sources + installed JS plugins), the database handle, and image downloads.
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use mv_net::{Http, Opts};
use mv_sources::{
    ChapterInfo, MangaDetails, MangaSearchResult, Source, SourceCtx, SourceError, SourceInfo, SourceRegistry, Tag,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::logic;

static CTX: OnceLock<SourceCtx> = OnceLock::new();

/// Optional: let the shell install a `SourceCtx` (shared HTTP client + wall solver) before the first
/// page opens. Returns false when one was already set / created.
pub fn set_source_ctx(ctx: SourceCtx) -> bool {
    CTX.set(ctx).is_ok()
}

pub fn ctx() -> SourceCtx {
    CTX.get_or_init(|| SourceCtx::new(Http::with_defaults().expect("http client"))).clone()
}

pub fn plugins_dir() -> PathBuf {
    mv_core::paths::root().join("plugins")
}

pub fn pages_cache_dir() -> PathBuf {
    mv_core::paths::root().join("cache").join("pages")
}

// ---------------------------------------------------------------- database

static DB: Mutex<Option<rusqlite::Connection>> = Mutex::new(None);

/// Run `f` on the shared read-write connection (opened + migrated on first use). Blocking: call it
/// from `mv_ui::bg::blocking`, never the UI thread.
pub fn with_db<T>(f: impl FnOnce(&mut rusqlite::Connection) -> Result<T>) -> Result<T> {
    let mut g = DB.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_none() {
        *g = Some(mv_core::db::open_default()?);
    }
    f(g.as_mut().expect("opened"))
}

// ---------------------------------------------------------------- plugins as sources

/// A JS plugin presented through the native [`Source`] trait.
pub struct PluginSource {
    plugin: mv_plugins::Plugin,
}

fn other(e: anyhow::Error) -> SourceError {
    SourceError::Other(e.to_string())
}

/// Deserialize a JSON array leniently: items that don't fit are skipped, and `fix` can patch
/// plugin output that omits optional-in-practice fields.
fn parse_list<T: DeserializeOwned>(v: Value, fix: impl Fn(&mut Value)) -> Vec<T> {
    let Value::Array(items) = v else { return vec![] };
    items
        .into_iter()
        .filter_map(|mut it| {
            fix(&mut it);
            serde_json::from_value(it).ok()
        })
        .collect()
}

fn fix_chapter(v: &mut Value) {
    if let Some(o) = v.as_object_mut() {
        if !matches!(o.get("language"), Some(Value::String(_))) {
            o.insert("language".into(), Value::String("en".into()));
        }
        // numbers sometimes arrive as strings
        if let Some(Value::String(s)) = o.get("number") {
            let n = s.trim().parse::<f64>().ok().and_then(serde_json::Number::from_f64);
            o.insert("number".into(), n.map(Value::Number).unwrap_or(Value::Null));
        }
    }
}

impl PluginSource {
    pub fn new(plugin: mv_plugins::Plugin) -> Self {
        Self { plugin }
    }

    fn need(&self, method: &str, what: &'static str) -> Result<(), SourceError> {
        if self.plugin.has_method(method) {
            Ok(())
        } else {
            Err(SourceError::Unsupported(what))
        }
    }
}

#[async_trait]
impl Source for PluginSource {
    fn info(&self) -> SourceInfo {
        let m = self.plugin.manifest();
        SourceInfo {
            id: m.id.clone(),
            name: m.name.clone(),
            version: m.version.clone(),
            lang: m.lang.clone(),
            nsfw: m.nsfw,
            requires_wall_solver: false,
            icon: m.extra.get("icon").and_then(|v| v.as_str()).map(String::from),
        }
    }

    async fn search_manga(&self, query: &str, page: u32) -> Result<Vec<MangaSearchResult>, SourceError> {
        Ok(parse_list(self.plugin.search_manga(query, page).await.map_err(other)?, |_| {}))
    }

    async fn get_manga_details(&self, manga_id: &str) -> Result<MangaDetails, SourceError> {
        let v = self.plugin.get_manga_details(manga_id).await.map_err(other)?;
        serde_json::from_value(v).map_err(|e| SourceError::Parse(e.to_string()))
    }

    async fn get_chapter_list(&self, manga_id: &str) -> Result<Vec<ChapterInfo>, SourceError> {
        Ok(parse_list(self.plugin.get_chapter_list(manga_id).await.map_err(other)?, fix_chapter))
    }

    async fn get_page_urls(&self, chapter_id: &str) -> Result<Vec<String>, SourceError> {
        let v = self.plugin.get_page_urls(chapter_id).await.map_err(other)?;
        Ok(parse_list(v, |_| {}))
    }

    async fn get_popular_manga(&self, page: u32) -> Result<Vec<MangaSearchResult>, SourceError> {
        self.need("getPopularManga", "popular")?;
        Ok(parse_list(self.plugin.get_popular_manga(page).await.map_err(other)?, |_| {}))
    }

    async fn get_latest_manga(&self, page: u32) -> Result<Vec<MangaSearchResult>, SourceError> {
        self.need("getLatestManga", "latest")?;
        Ok(parse_list(self.plugin.get_latest_manga(page).await.map_err(other)?, |_| {}))
    }

    async fn get_tag_list(&self) -> Result<Vec<Tag>, SourceError> {
        if !self.plugin.has_method("getTagList") {
            return Ok(vec![]);
        }
        Ok(parse_list(self.plugin.get_tag_list().await.map_err(other)?, |_| {}))
    }

    async fn get_manga_by_tag(&self, tag_id: &str, page: u32) -> Result<Vec<MangaSearchResult>, SourceError> {
        self.need("getMangaByTag", "tag browsing")?;
        Ok(parse_list(self.plugin.get_manga_by_tag(tag_id, page).await.map_err(other)?, |_| {}))
    }

    fn download_headers(&self) -> Vec<(String, String)> {
        let base = &self.plugin.manifest().base_url;
        if base.is_empty() {
            vec![]
        } else {
            vec![("Referer".into(), base.clone())]
        }
    }
}

// ---------------------------------------------------------------- registry

static REGISTRY: Mutex<Option<Arc<SourceRegistry>>> = Mutex::new(None);

/// Built-in sources plus every installed plugin, cached until [`reload_registry`].
pub async fn registry() -> Arc<SourceRegistry> {
    let cached = REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).clone();
    match cached {
        Some(r) => r,
        None => reload_registry().await,
    }
}

pub async fn reload_registry() -> Arc<SourceRegistry> {
    let mut reg = SourceRegistry::builtin(ctx());
    let dir = plugins_dir();
    for m in mv_plugins::repo::list_installed(&dir) {
        match mv_plugins::load_plugin(dir.join(&m.id)).await {
            Ok(p) => reg.register(Arc::new(PluginSource::new(p))),
            Err(e) => eprintln!("[ui-manga] plugin {} failed to load: {e:#}", m.id),
        }
    }
    let reg = Arc::new(reg);
    *REGISTRY.lock().unwrap_or_else(|e| e.into_inner()) = Some(reg.clone());
    reg
}

pub async fn source(id: &str) -> Result<Arc<dyn Source>> {
    registry().await.get(id).ok_or_else(|| anyhow!("Source \"{id}\" is not installed"))
}

/// Ids of builtin sources (everything else in the registry is an installed plugin).
pub fn is_installed_plugin(id: &str) -> bool {
    mv_plugins::repo::list_installed(&plugins_dir()).iter().any(|m| m.id == id)
}

// ---------------------------------------------------------------- source enable/disable

const DISABLED_KEY: &str = "disabledSources";

pub fn disabled_sources() -> HashSet<String> {
    mv_core::config::Config::open_default()
        .get(DISABLED_KEY)
        .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
        .unwrap_or_default()
        .into_iter()
        .collect()
}

pub fn set_source_enabled(id: &str, enabled: bool) -> Result<()> {
    let mut set = disabled_sources();
    if enabled {
        set.remove(id);
    } else {
        set.insert(id.to_string());
    }
    let mut v: Vec<String> = set.into_iter().collect();
    v.sort();
    mv_core::config::Config::open_default().set(DISABLED_KEY, serde_json::json!(v))
}

// ---------------------------------------------------------------- downloads

fn origin_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?;
    let host = rest.1.split('/').next()?;
    Some(format!("{}://{host}/", rest.0))
}

async fn fetch_image(url: &str, source_id: Option<&str>) -> Result<(Vec<u8>, &'static str)> {
    let (clean, referer) = mv_sources::util::split_ref_hint(url);
    let mut opts = Opts::new();
    if let Some(id) = source_id {
        if let Some(src) = registry().await.get(id) {
            opts = opts.headers(src.download_headers());
        }
    }
    if let Some(r) = referer.or_else(|| origin_of(&clean)) {
        opts = opts.referer(r);
    }
    let resp = ctx().http.get(&clean, &opts).await?.error_for_status()?;
    let ext = logic::sniff_ext(&resp.body).ok_or_else(|| anyhow!("not an image (blocked or expired link)"))?;
    Ok((resp.body.to_vec(), ext))
}

fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension(format!("part{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Download a reader page into `~/.mangavox/cache/pages/<hash>.<ext>` (cached on disk).
pub async fn download_page(url: String, source_id: Option<String>) -> Result<PathBuf> {
    let (clean, referer) = mv_sources::util::split_ref_hint(&url);
    let stem = logic::page_cache_name(&clean, referer.as_deref());
    let dir = pages_cache_dir();
    if let Some(p) = logic::find_cached(&dir, &stem) {
        return Ok(p);
    }
    let (bytes, ext) = fetch_image(&url, source_id.as_deref()).await?;
    let path = dir.join(format!("{stem}.{ext}"));
    write_atomic(&path, &bytes)?;
    Ok(path)
}

/// Local file for a cover, downloading it into the shared covers cache when missing
/// (same `md5(url).<ext>` naming the Electron app uses).
pub async fn ensure_cover(url: String, source_id: Option<String>) -> Option<PathBuf> {
    if let Some(p) = mv_core::covers::cached_cover(&url) {
        return Some(p);
    }
    if !url.starts_with("http") {
        return None;
    }
    let (bytes, ext) = fetch_image(&url, source_id.as_deref()).await.ok()?;
    let stem = format!("{:x}", md5::compute(url.as_bytes()));
    let path = mv_core::paths::covers().join(format!("{stem}.{ext}"));
    write_atomic(&path, &bytes).ok()?;
    Some(path)
}

/// Resolve every cover that is already on disk (cheap, blocking) and list the ones still missing.
pub fn resolve_covers(urls: impl IntoIterator<Item = (String, Option<String>)>) -> (std::collections::HashMap<String, PathBuf>, Vec<(String, Option<String>)>) {
    let mut have = std::collections::HashMap::new();
    let mut missing = Vec::new();
    let mut seen = HashSet::new();
    for (u, s) in urls {
        if u.is_empty() || !seen.insert(u.clone()) {
            continue;
        }
        match mv_core::covers::cached_cover(&u) {
            Some(p) => {
                have.insert(u, p);
            }
            None if u.starts_with("http") => missing.push((u, s)),
            None => {}
        }
    }
    (have, missing)
}
