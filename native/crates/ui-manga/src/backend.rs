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
    mv_sources::set_smart_bypass(smart_bypass());
    mv_sources::set_chapter_language(&preferred_language());
    CTX.set(ctx).is_ok()
}

pub fn ctx() -> SourceCtx {
    CTX.get_or_init(|| {
        mv_sources::set_smart_bypass(smart_bypass());
        mv_sources::set_chapter_language(&preferred_language());
        SourceCtx::new(Http::with_defaults().expect("http client"))
    })
    .clone()
}

const SMART_BYPASS_KEY: &str = "sources.smartBypass";

/// Settings › Sources › Smart bypass (on unless switched off): anti-bot walls are cleared in the
/// hidden browser and the request is retried.
pub fn smart_bypass() -> bool {
    mv_core::config::Config::open_default().get(SMART_BYPASS_KEY).and_then(|v| v.as_bool()).unwrap_or(true)
}

pub fn set_smart_bypass(on: bool) -> Result<()> {
    mv_sources::set_smart_bypass(on);
    mv_core::config::Config::open_default().set(SMART_BYPASS_KEY, serde_json::json!(on))
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
    /// Its icon: the manifest's, else the repository index's for an extension.
    icon: Option<String>,
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
        let m = plugin.manifest();
        let icon = m.extra.get("icon").and_then(|v| v.as_str()).map(String::from).or_else(|| {
            mv_plugins::repo::read_repo_source(&plugins_dir(), &m.id).and_then(|e| e.icon_url).filter(|u| u.starts_with("http"))
        });
        Self { plugin, icon }
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
            icon: self.icon.clone(),
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
    // A repo extension for a site a built-in source reads (MangaDex, WEBTOON...) was installed as a
    // generated scraper by older builds; the built-in source replaces it.
    for p in mv_plugins::repo::installed_repo_plugins(&dir) {
        if let Some(builtin) = mv_sources::builtin_for_host(&p.manifest.base_url) {
            match set_source_enabled(builtin, true).and_then(|_| mv_plugins::repo::uninstall_plugin(&dir, &p.manifest.id)) {
                Ok(_) => eprintln!("[ui-manga] {} now uses the built-in {builtin} source", p.manifest.id),
                Err(e) => eprintln!("[ui-manga] could not switch {} to the built-in source: {e:#}", p.manifest.id),
            }
        }
    }
    // Repo plugins made by an older template are regenerated offline before they load.
    let upgraded = mv_plugins::repo::upgrade_repo_plugins(&dir);
    if upgraded > 0 {
        eprintln!("[ui-manga] regenerated {upgraded} repository plugin(s) from the current templates");
    }
    for m in mv_plugins::repo::list_installed(&dir) {
        match mv_plugins::load_plugin_with(dir.join(&m.id), mv_plugins::PluginOptions::default(), plugin_backend()).await {
            Ok(p) => reg.register(Arc::new(PluginSource::new(p))),
            Err(e) => eprintln!("[ui-manga] plugin {} failed to load: {e:#}", m.id),
        }
    }
    let reg = Arc::new(reg);
    *REGISTRY.lock().unwrap_or_else(|e| e.into_inner()) = Some(reg.clone());
    bump_sources();
    reg
}

pub async fn source(id: &str) -> Result<Arc<dyn Source>> {
    registry().await.get(id).ok_or_else(|| anyhow!("Source \"{id}\" is not installed"))
}

/// Page URLs of a chapter from the first of `tries` (`(source, chapter source id)`: the chapter's
/// own source, then other sources that have the same chapter) that serves any, with the source
/// used. A chapter its source cannot serve (licensed, taken down) is read from another one.
pub async fn page_urls(tries: Vec<(String, String)>) -> Result<(String, Vec<String>)> {
    let mut first_err: Option<anyhow::Error> = None;
    for (src_id, chapter_source_id) in tries {
        let got = match source(&src_id).await {
            Ok(src) => src.get_page_urls(&chapter_source_id).await.map_err(|e| anyhow!("{e}")),
            Err(e) => Err(e),
        };
        match got {
            Ok(urls) if !urls.is_empty() => return Ok((src_id, urls)),
            Ok(_) => first_err = first_err.or_else(|| Some(anyhow!("The source returned no pages for this chapter"))),
            Err(e) => first_err = first_err.or(Some(e)),
        }
    }
    Err(first_err.unwrap_or_else(|| anyhow!("The source returned no pages for this chapter")))
}

/// `(source, chapter source id)` of the same chapter in a title's other sources (same number,
/// another source), the preferred language first. For [`page_urls`].
pub fn alternate_sources(all: &[mv_core::repo::chapter::Chapter], ch: &mv_core::repo::chapter::Chapter) -> Vec<(String, String)> {
    let lang = preferred_language();
    let mut alts: Vec<&mv_core::repo::chapter::Chapter> =
        all.iter().filter(|x| x.id != ch.id && x.source_plugin != ch.source_plugin && x.number.is_some() && x.number == ch.number).collect();
    alts.sort_by_key(|x| !logic::lang_matches(&x.language, &lang));
    alts.into_iter().map(|a| (a.source_plugin.clone(), a.source_id.clone())).collect()
}

/// Ids of builtin sources (everything else in the registry is an installed plugin).
pub fn is_installed_plugin(id: &str) -> bool {
    mv_plugins::repo::list_installed(&plugins_dir()).iter().any(|m| m.id == id)
}

static SOURCES_VERSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Bumped whenever the installed sources change (install, removal, registry reload), so Browse
/// and Settings reload their source lists when they are shown again.
pub fn sources_version() -> u64 {
    SOURCES_VERSION.load(std::sync::atomic::Ordering::Relaxed)
}

fn bump_sources() {
    SOURCES_VERSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// The sources Browse lists, in registry order: every installed plugin and the built-in sources
/// the user added. Built-in sources are not installed until added.
pub fn installed_sources(all: Vec<SourceInfo>) -> Vec<SourceInfo> {
    let builtin: HashSet<String> = builtin_sources().into_iter().map(|s| s.id).collect();
    let off = disabled_sources();
    all.into_iter().filter(|s| !builtin.contains(&s.id) || !off.contains(&s.id)).collect()
}

/// Removes a source so Browse stops listing it: an installed plugin's folder is deleted and the
/// registry reloaded; a built-in source is taken off the installed list (it can be added again).
pub async fn remove_source(id: &str) -> Result<()> {
    if builtin_sources().iter().any(|s| s.id == id) {
        return set_source_enabled(id, false);
    }
    mv_plugins::repo::uninstall_plugin(&plugins_dir(), id)?;
    // Drop any switch saved for it, so a reinstall starts clean.
    set_source_enabled(id, true)?;
    reload_registry().await;
    Ok(())
}

// ---------------------------------------------------------------- plugin network access

type BoxFut<T> = std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, String>> + Send>>;

/// How plugins reach the network: the app's shared client (browser user agent, cookie jar,
/// retries) with the WebView2 wall solver behind it, so a Cloudflare-walled site that a repo
/// extension points at can still be read. GET requests go through [`SourceCtx::fetch`], which
/// retries a wall with the solver's cookies and then inside the browser; `__render` and
/// `__solve` use the solver directly. Other requests use plain reqwest.
pub struct WallBackend {
    ctx: SourceCtx,
    plain: mv_plugins::ReqwestBackend,
}

impl WallBackend {
    pub fn new(ctx: SourceCtx) -> Self {
        Self { ctx, plain: mv_plugins::ReqwestBackend::new() }
    }
}

fn to_fetch_response(r: mv_net::Resp) -> mv_plugins::FetchResponse {
    let text = r.text();
    let headers = r.headers.into_iter().collect();
    mv_plugins::FetchResponse::from_text(r.status, text, r.url, headers)
}

impl mv_plugins::FetchBackend for WallBackend {
    fn fetch(&self, plugin_id: &str, req: mv_plugins::FetchRequest) -> BoxFut<mv_plugins::FetchResponse> {
        let method = req.method.as_deref().unwrap_or("GET").to_ascii_uppercase();
        let json_body = matches!(&req.body, Some(Value::Object(_) | Value::Array(_)));
        let no_body = matches!(&req.body, None | Some(Value::Null));
        if !(method == "GET" && no_body) && !(method == "POST" && json_body) {
            return self.plain.fetch(plugin_id, req);
        }
        let ctx = self.ctx.clone();
        let id = plugin_id.to_string();
        Box::pin(async move {
            let mut opts = Opts::new().headers(req.headers.clone()).timeout(std::time::Duration::from_millis(req.timeout_ms.unwrap_or(20_000)));
            if let Some(r) = req.referer.clone() {
                opts = opts.referer(r);
            }
            let resp = if method == "GET" {
                ctx.fetch(&id, &req.url, &opts).await.map_err(|e| e.to_string())?
            } else {
                ctx.http.post_json(&req.url, req.body.as_ref().unwrap_or(&Value::Null), &opts).await.map_err(|e| e.to_string())?
            };
            Ok(to_fetch_response(resp))
        })
    }

    fn render(&self, _plugin_id: &str, url: String, opts: Value) -> BoxFut<mv_plugins::RenderResponse> {
        let solver = self.ctx.solver.clone();
        Box::pin(async move {
            let Some(solver) = solver else {
                return Err("This page needs the built-in browser (WebView2), which is not available".to_string());
            };
            let wait = opts.get("waitForSelector").and_then(Value::as_str).map(str::to_string);
            let html = solver.fetch_html(&url, wait.as_deref()).await.map_err(|e| format!("{e:#}"))?;
            let wall = mv_net::detect_wall(200, "text/html", "", "", &html).is_some();
            Ok(mv_plugins::RenderResponse { html, url, status: 200, wall })
        })
    }

    fn solve(&self, _plugin_id: &str, url: String) -> BoxFut<bool> {
        let ctx = self.ctx.clone();
        Box::pin(async move {
            let Some(solver) = ctx.solver.clone().filter(|_| mv_sources::smart_bypass()) else { return Ok(false) };
            let cookies = solver.cookies(&url).await.map_err(|e| format!("{e:#}"))?;
            for (n, v) in &cookies {
                let _ = ctx.http.add_cookie(&url, &format!("{n}={v}"));
            }
            Ok(!cookies.is_empty())
        })
    }
}

static PLUGIN_BACKEND: OnceLock<Arc<dyn mv_plugins::FetchBackend>> = OnceLock::new();

/// The network backend every plugin loads and installs with (see [`WallBackend`]).
pub fn plugin_backend() -> Arc<dyn mv_plugins::FetchBackend> {
    PLUGIN_BACKEND.get_or_init(|| Arc::new(WallBackend::new(ctx()))).clone()
}

// ---------------------------------------------------------------- repositories and plugin installs

/// The repository browser and the plugin-link box from Settings › Sources, for the welcome setup.
pub use crate::sources::{repo_label, source_avatar, trust_note, PluginLinkBox, RepoBrowser, SourcesEvent};

/// Where `repos.json` lives (the data folder).
pub fn repos_root() -> PathBuf {
    mv_core::paths::root()
}

// ---------------------------------------------------------------- source enable/disable

const DISABLED_KEY: &str = "disabledSources";

/// The sources compiled into the app (MangaDex, ComicK, MangaFire, WEBTOON and the 18+ ones), in
/// display order. Cheap: nothing is fetched.
pub fn builtin_sources() -> Vec<SourceInfo> {
    SourceRegistry::builtin(ctx()).list()
}

/// `disabled` after switching each of `builtin` on when it is in `enabled` and off otherwise.
/// Entries for anything else (plugins) are kept as they are. Sorted, for a stable config file.
pub fn with_builtins(disabled: &HashSet<String>, builtin: &[String], enabled: &HashSet<String>) -> Vec<String> {
    let mut out: HashSet<String> = disabled.iter().filter(|id| !builtin.contains(id)).cloned().collect();
    out.extend(builtin.iter().filter(|id| !enabled.contains(*id)).cloned());
    let mut v: Vec<String> = out.into_iter().collect();
    v.sort();
    v
}

/// Switches the built-in sources in `enabled` on and every other built-in source off; plugin
/// switches are left alone.
pub fn set_builtins_enabled(enabled: &HashSet<String>) -> Result<()> {
    let builtin: Vec<String> = builtin_sources().into_iter().map(|s| s.id).collect();
    let v = with_builtins(&disabled_sources(), &builtin, enabled);
    mv_core::config::Config::open_default().set(DISABLED_KEY, serde_json::json!(v))?;
    bump_sources();
    Ok(())
}

/// A clean start: no built-in source is switched on, so Browse shows nothing until the user adds
/// or enables a source.
pub fn seed_clean_start() -> Result<()> {
    set_builtins_enabled(&HashSet::new())
}

/// Sources taken out of Browse. Before anything is saved every built-in source is in it: a new
/// install starts with no source at all.
pub fn disabled_sources() -> HashSet<String> {
    match mv_core::config::Config::open_default().get(DISABLED_KEY).and_then(|v| serde_json::from_value::<Vec<String>>(v).ok()) {
        Some(v) => v.into_iter().collect(),
        None => builtin_sources().into_iter().map(|s| s.id).collect(),
    }
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
    mv_core::config::Config::open_default().set(DISABLED_KEY, serde_json::json!(v))?;
    bump_sources();
    Ok(())
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

/// Root of the offline chapter downloads: `<data>/library/manga/<manga_id>/<chapter_id>/`.
/// A custom "Manga library" folder from Settings → Storage wins.
pub fn downloads_dir() -> PathBuf {
    config_str("storage.manga").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).map(PathBuf::from).unwrap_or_else(mv_core::paths::manga_downloads)
}

/// Save page `index` of a chapter into `dir` as `0001.<ext>`, reusing the reader's page cache
/// when the page was already viewed.
pub async fn save_page(url: String, source_id: Option<String>, dir: PathBuf, index: usize) -> Result<PathBuf> {
    let (clean, referer) = mv_sources::util::split_ref_hint(&url);
    let stem = format!("{:04}", index + 1);
    let cached = logic::find_cached(&pages_cache_dir(), &logic::page_cache_name(&clean, referer.as_deref()));
    let (bytes, ext) = match cached {
        Some(p) => {
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("jpg").to_string();
            (std::fs::read(&p)?, ext)
        }
        None => {
            let (b, e) = fetch_image(&url, source_id.as_deref()).await?;
            (b, e.to_string())
        }
    };
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

/// Covers already on disk (`url -> file`) and the `(url, source)` pairs still to download.
pub type CoverLookup = (std::collections::HashMap<String, PathBuf>, Vec<(String, Option<String>)>);

/// Resolve every cover that is already on disk (cheap, blocking) and list the ones still missing.
pub fn resolve_covers(urls: impl IntoIterator<Item = (String, Option<String>)>) -> CoverLookup {
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

// ---------------------------------------------------------------- reading preferences

const LANG_KEY: &str = "chapterLanguage";

/// Preferred chapter language (ISO code, default "en"; "all" disables the filter).
pub fn preferred_language() -> String {
    mv_core::config::Config::open_default()
        .get(LANG_KEY)
        .and_then(|v| v.as_str().map(str::to_string))
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "en".into())
}

pub fn config_str(key: &str) -> Option<String> {
    mv_core::config::Config::open_default().get(key).and_then(|v| v.as_str().map(str::to_string))
}

pub fn set_config_str(key: &str, value: &str) {
    let _ = mv_core::config::Config::open_default().set(key, serde_json::json!(value));
}

// ---------------------------------------------------------------- change tracking

static DATA_VERSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Bumped whenever reading progress, read state or library membership changes, so pages that are
/// kept alive in history (Library, History, Detail, Browse) can reload when they are shown again.
pub fn data_version() -> u64 {
    DATA_VERSION.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn bump_data() {
    DATA_VERSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

// ---------------------------------------------------------------- library membership

const BROWSE_ONLY_KEY: &str = "mangaBrowseOnly";

/// Titles that exist in the database only because they were opened from Browse (not added to the
/// library). Kept in the config so the shared database schema stays untouched.
pub fn browse_only() -> HashSet<String> {
    mv_core::config::Config::open_default()
        .get(BROWSE_ONLY_KEY)
        .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
        .unwrap_or_default()
        .into_iter()
        .collect()
}

pub fn set_in_library(manga_id: &str, in_library: bool) {
    let mut set = browse_only();
    let changed = if in_library { set.remove(manga_id) } else { set.insert(manga_id.to_string()) };
    if changed {
        let mut v: Vec<String> = set.into_iter().collect();
        v.sort();
        let _ = mv_core::config::Config::open_default().set(BROWSE_ONLY_KEY, serde_json::json!(v));
        bump_data();
    }
}

// ---------------------------------------------------------------- chapter extras

/// Scanlation group and upload date per chapter (`source_id` -> extras). The database has no
/// columns for these, so they live for the session, filled whenever a chapter list is fetched.
#[derive(Clone, Default, Debug)]
pub struct ChapterExtra {
    pub group: Option<String>,
    pub date: Option<String>,
}

static EXTRAS: Mutex<Option<std::collections::HashMap<String, std::collections::HashMap<String, ChapterExtra>>>> = Mutex::new(None);

pub fn store_extras(manga_id: &str, chapters: &[ChapterInfo]) {
    let map = chapters
        .iter()
        .map(|c| {
            let date = c.published_at.as_deref().map(|d| d.chars().take(10).collect::<String>()).filter(|d| !d.is_empty());
            (c.source_id.clone(), ChapterExtra { group: c.scanlation_group.clone().filter(|g| !g.is_empty()), date })
        })
        .collect();
    EXTRAS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(manga_id.to_string(), map);
}

pub fn extras(manga_id: &str) -> Option<std::collections::HashMap<String, ChapterExtra>> {
    EXTRAS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(manga_id).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn clean_start_turns_every_builtin_off_and_keeps_plugin_switches() {
        let builtin: Vec<String> = ["mangadex", "comick", "nhentai"].iter().map(|s| s.to_string()).collect();
        // Fresh config: everything built in goes off.
        assert_eq!(with_builtins(&HashSet::new(), &builtin, &HashSet::new()), vec!["comick", "mangadex", "nhentai"]);
        // A chosen source stays on; a disabled plugin stays disabled; an old choice is replaced.
        let disabled = set(&["repo-x-en", "comick"]);
        assert_eq!(with_builtins(&disabled, &builtin, &set(&["comick"])), vec!["mangadex", "nhentai", "repo-x-en"]);
        // Everything on.
        assert_eq!(with_builtins(&disabled, &builtin, &set(&["mangadex", "comick", "nhentai"])), vec!["repo-x-en"]);
    }

    #[test]
    fn the_seven_builtin_sources_are_listed() {
        let ids: Vec<String> = builtin_sources().into_iter().map(|s| s.id).collect();
        assert_eq!(ids.len(), 7, "{ids:?}");
        assert!(builtin_sources().iter().filter(|s| s.nsfw).count() >= 3);
    }
}

/// `fut`, or `None` after `secs` seconds: one slow or blocked source (an anti-bot check, a dead
/// site) must not hold up a search across every source.
pub async fn within<T>(secs: u64, fut: impl std::future::Future<Output = T>) -> Option<T> {
    tokio::time::timeout(std::time::Duration::from_secs(secs), fut).await.ok()
}
