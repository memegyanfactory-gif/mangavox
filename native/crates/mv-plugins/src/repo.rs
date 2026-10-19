//! Plugin repositories, mirroring `repo-manager.ts` / `plugin-loader.ts`:
//!
//! - repo index URL normalisation + parsing (Tachiyomi/Mihon formats) and the persisted repo list
//!   (`<root>/repos.json`, compatible with the Electron app's file),
//! - installing a repo extension: Mihon APKs cannot run, so the site's engine is fingerprinted and a
//!   sandboxed JS plugin is generated from the matching template (`crate::templates`),
//! - plugin URL resolution, install/uninstall/list into an injectable plugins dir, update checks.

use crate::backend::{FetchBackend, FetchRequest};
use crate::host::{Plugin, PluginOptions};
use crate::manifest::{read_manifest, PluginManifest};
use crate::templates::{
    detect_site_engine, generate_template_plugin, host_of, needs_regeneration, normalize_base_url, Engine, SiteProbe,
    TemplateOptions, LEGACY_HEADER, TEMPLATE_VERSION,
};
use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RepoSource {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub lang: String,
    #[serde(default)]
    pub id: Value,
    #[serde(default)]
    pub base_url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RepoExtension {
    pub name: String,
    pub pkg: String,
    #[serde(default)]
    pub apk: String,
    #[serde(default)]
    pub lang: String,
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub nsfw: i64,
    #[serde(default)]
    pub sources: Vec<RepoSource>,
    /// Icon URL when the index provides one (modern Mihon index: `resources.iconUrl`).
    #[serde(default, rename = "iconUrl", skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
}

fn re(p: &str) -> Regex {
    Regex::new(p).expect("static regex")
}

/// The `url` of a `tachiyomi://add-repo?url=...` / `mihon://add-repo?url=...` share link.
fn deep_link_url(s: &str) -> Option<String> {
    let lower = s.to_ascii_lowercase();
    if !(lower.starts_with("tachiyomi://add-repo") || lower.starts_with("mihon://add-repo")) {
        return None;
    }
    let u = reqwest::Url::parse(s).ok()?;
    u.query_pairs().find(|(k, _)| k == "url").map(|(_, v)| v.into_owned())
}

/// Normalize whatever the user pasted to an `index.min.json` / `index.json` URL.
pub fn normalize_repo_url(input: &str) -> String {
    let mut url = input.trim().trim_end_matches('/').to_string();
    if let Some(inner) = deep_link_url(&url) {
        url = inner.trim().trim_end_matches('/').to_string();
    }
    if url.ends_with("index.min.json") || url.ends_with("index.json") {
        return url;
    }
    if let Some(c) = re(r"^https?://github\.com/([^/]+)/([^/]+?)(\.git)?$").captures(&url) {
        return format!("https://raw.githubusercontent.com/{}/{}/repo/index.min.json", &c[1], &c[2]);
    }
    if url.contains("raw.githubusercontent.com") && !url.ends_with(".json") {
        return format!("{url}/index.min.json");
    }
    if let Some(c) = re(r"^https?://github\.com/([^/]+)/([^/]+)/tree/([^/]+)/(.+)$").captures(&url) {
        return format!(
            "https://raw.githubusercontent.com/{}/{}/{}/{}/index.min.json",
            &c[1], &c[2], &c[3], &c[4]
        );
    }
    format!("{url}/index.min.json")
}

/// Keiyoushi serves a tiny "update your app" stub at index.min.json.
fn is_stub_index(a: &[Value]) -> bool {
    if a.is_empty() || a.len() > 3 {
        return false;
    }
    let pkg_re = re(r"extension\.all\.(keiyoushi|mihon)$");
    let name_re = re(r"(?i)outdated app|update to mihon");
    a.iter().all(|e| {
        let pkg = e.get("pkg").and_then(Value::as_str).unwrap_or("");
        let name = e.get("name").and_then(Value::as_str).unwrap_or("");
        pkg_re.is_match(pkg) || name_re.is_match(name)
    })
}

/// Convert the newer Mihon format (`extensionList.extensions[]`) to the classic shape.
fn convert_modern_index(data: &Value) -> Option<Vec<RepoExtension>> {
    let list = data
        .pointer("/extensionList/extensions")
        .or_else(|| data.get("extensions"))?
        .as_array()?;
    let lang_re = re(r"\.extension\.([^.]+)\.");
    let s = |v: Option<&Value>| v.and_then(Value::as_str).unwrap_or("").to_string();
    let mut out = Vec::new();
    for e in list {
        let Some(pkg) = e.get("packageName").and_then(Value::as_str) else { continue };
        let Some(srcs) = e.get("sources").and_then(Value::as_array) else { continue };
        let pkg_lang = lang_re.captures(pkg).map(|c| c[1].to_string());
        let ename = s(e.get("name"));
        let sources: Vec<RepoSource> = srcs
            .iter()
            .filter(|x| x.get("homeUrl").is_some_and(|v| !v.is_null()) || x.get("baseUrl").is_some_and(|v| !v.is_null()))
            .map(|x| {
                let name = s(x.get("name"));
                let lang = ["language", "lang"]
                    .iter()
                    .map(|k| s(x.get(*k)))
                    .find(|l| !l.is_empty())
                    .or_else(|| pkg_lang.clone())
                    .unwrap_or_else(|| "all".into());
                let base = ["homeUrl", "baseUrl"].iter().map(|k| s(x.get(*k))).find(|u| !u.is_empty()).unwrap_or_default();
                RepoSource {
                    name: if name.is_empty() { ename.clone() } else { name },
                    lang,
                    id: Value::String(match x.get("id") {
                        Some(Value::String(i)) => i.clone(),
                        Some(Value::Null) | None => String::new(),
                        Some(v) => v.to_string(),
                    }),
                    base_url: base,
                }
            })
            .collect();
        if sources.is_empty() {
            continue;
        }
        let apk_url = e.pointer("/resources/apkUrl").and_then(Value::as_str).unwrap_or("");
        out.push(RepoExtension {
            name: ename,
            pkg: pkg.to_string(),
            apk: apk_url.rsplit('/').next().filter(|_| !apk_url.is_empty()).unwrap_or("").to_string(),
            lang: pkg_lang.unwrap_or_else(|| sources[0].lang.clone()),
            // The modern index writes versionCode as a string.
            code: match e.get("versionCode") {
                Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
                Some(Value::String(c)) => c.trim().parse().unwrap_or(0),
                _ => 0,
            },
            version: e.get("versionName").and_then(Value::as_str).unwrap_or("1.0").to_string(),
            nsfw: i64::from(e.get("contentWarning").and_then(Value::as_str) == Some("CONTENT_WARNING_NSFW")),
            sources,
            icon_url: e.pointer("/resources/iconUrl").and_then(Value::as_str).filter(|u| !u.is_empty()).map(String::from),
        });
    }
    Some(out)
}

/// Parse an index document. `None` = not a usable index (invalid JSON or stub).
pub fn parse_repo_index(text: &str) -> Option<Vec<RepoExtension>> {
    let data: Value = serde_json::from_str(text).ok()?;
    if let Value::Array(items) = data {
        if is_stub_index(&items) {
            return None;
        }
        // One malformed entry must not hide the other ~1000 extensions of a repo.
        return Some(items.into_iter().filter_map(|e| serde_json::from_value(e).ok()).collect());
    }
    convert_modern_index(&data)
}

async fn fetch_text(backend: &dyn FetchBackend, url: &str) -> Result<String> {
    let req = FetchRequest { url: url.to_string(), ..Default::default() };
    let r = backend.fetch("repo", req).await.map_err(|e| anyhow!("GET {url}: {e}"))?;
    if !(200..300).contains(&r.status) {
        bail!("GET {url}: HTTP {}", r.status);
    }
    Ok(r.text)
}

/// Fetch and parse a repo's extension list (falls back from a stub `index.min.json` to `index.json`).
pub async fn fetch_repo_extensions(repo_url: &str, backend: &dyn FetchBackend) -> Result<Vec<RepoExtension>> {
    let url = normalize_repo_url(repo_url);
    let direct = parse_repo_index(&fetch_text(backend, &url).await?);
    if direct.as_ref().is_some_and(|d| !d.is_empty()) {
        return Ok(direct.unwrap());
    }
    if let Some(base) = url.strip_suffix("index.min.json") {
        if let Ok(t) = fetch_text(backend, &format!("{base}index.json")).await {
            if let Some(m) = parse_repo_index(&t).filter(|m| !m.is_empty()) {
                return Ok(m);
            }
        }
    }
    direct.ok_or_else(|| anyhow!("Invalid extension index at {url} - expected a JSON array or a Mihon index.json"))
}

/// Resolve a user-supplied plugin URL to `(manifest_url, code_base_url)`.
pub fn resolve_plugin_url(input: &str) -> Result<(String, String)> {
    let t = input.trim();
    if let Some(c) = re(r"^https?://github\.com/([^/]+)/([^/]+)/?$").captures(t) {
        let repo = c[2].trim_end_matches(".git");
        let base = format!("https://raw.githubusercontent.com/{}/{}/main/", &c[1], repo);
        return Ok((format!("{base}manifest.json"), base));
    }
    if re(r"^https?://github\.com/.+/tree/.+").is_match(t) {
        let c = re(r"^https?://github\.com/([^/]+)/([^/]+)/tree/([^/]+)/(.+)$")
            .captures(t.trim_end_matches('/'))
            .ok_or_else(|| anyhow!("Invalid GitHub URL format"))?;
        let base = format!("https://raw.githubusercontent.com/{}/{}/{}/{}/", &c[1], &c[2], &c[3], &c[4]);
        return Ok((format!("{base}manifest.json"), base));
    }
    if let Some(base) = t.strip_suffix("manifest.json") {
        return Ok((t.to_string(), base.to_string()));
    }
    if t.ends_with('/') {
        return Ok((format!("{t}manifest.json"), t.to_string()));
    }
    let base = format!("{t}/");
    Ok((format!("{base}manifest.json"), base))
}

fn safe_id(id: &str) -> bool {
    !id.is_empty() && id != "." && id != ".." && id.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

/// Install a plugin from a URL into `plugins_dir/<id>/` (manifest.json, entry point, `.source_url`).
/// The code is validated by loading it in the sandbox before anything is written.
pub async fn install_plugin_from_url(
    url: &str,
    plugins_dir: &Path,
    backend: Arc<dyn FetchBackend>,
) -> Result<PluginManifest> {
    let (manifest_url, base) = resolve_plugin_url(url)?;
    let manifest_text = fetch_text(backend.as_ref(), &manifest_url).await?;
    let manifest = PluginManifest::parse(&manifest_text)?;
    if !safe_id(&manifest.id) {
        bail!("Invalid plugin id: {}", manifest.id);
    }
    if manifest.entry_point.contains("..") || manifest.entry_point.contains(['/', '\\']) {
        bail!("Invalid entry_point: {}", manifest.entry_point);
    }
    let code_url = format!("{base}{}", manifest.entry_point);
    let code = fetch_text(backend.as_ref(), &code_url).await?;
    if code.len() < 10 {
        bail!("Failed to fetch plugin code from {code_url}");
    }
    Plugin::from_source(manifest.clone(), &code, PluginOptions::default(), backend)
        .await
        .map_err(|e| anyhow!("Plugin validation failed: {e}"))?;

    let dir = plugins_dir.join(&manifest.id);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("manifest.json"), serde_json::to_string_pretty(&manifest)?)?;
    std::fs::write(dir.join(&manifest.entry_point), code)?;
    std::fs::write(dir.join(".source_url"), url.trim())?;
    Ok(manifest)
}

/// Remove an installed plugin. Returns false if it was not installed.
pub fn uninstall_plugin(plugins_dir: &Path, id: &str) -> Result<bool> {
    if !safe_id(id) {
        bail!("Invalid plugin id: {id}");
    }
    let dir = plugins_dir.join(id);
    if !dir.exists() {
        return Ok(false);
    }
    std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
    Ok(true)
}

/// Manifests of every plugin directory (with a manifest.json and an entry point) under `plugins_dir`.
pub fn list_installed(plugins_dir: &Path) -> Vec<PluginManifest> {
    let Ok(rd) = std::fs::read_dir(plugins_dir) else { return vec![] };
    let mut out: Vec<PluginManifest> = rd
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let m = read_manifest(e.path()).ok()?;
            e.path().join(&m.entry_point).exists().then_some(m)
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Whether the plugin in `plugins_dir/<id>` has a different version at its recorded `.source_url`.
pub async fn check_plugin_update(
    plugins_dir: &Path,
    id: &str,
    backend: &dyn FetchBackend,
) -> Result<Option<String>> {
    if !safe_id(id) {
        bail!("Invalid plugin id: {id}");
    }
    let dir = plugins_dir.join(id);
    let Ok(src) = std::fs::read_to_string(dir.join(".source_url")) else { return Ok(None) };
    let (manifest_url, _) = resolve_plugin_url(&src)?;
    let remote = PluginManifest::parse(&fetch_text(backend, &manifest_url).await?)?;
    let local = read_manifest(&dir)?;
    Ok((remote.version != local.version).then_some(remote.version))
}

// ---------------------------------------------------------------------------------------------
// Saved repositories (`<root>/repos.json`, `{"repos": [index url, ...]}`, same file the Electron app wrote)
// ---------------------------------------------------------------------------------------------

const REPOS_FILE: &str = "repos.json";

#[derive(Serialize, Deserialize, Default)]
struct SavedRepos {
    #[serde(default)]
    repos: Vec<String>,
}

fn is_http_url(u: &str) -> bool {
    re(r"^https?://[^/\s]+\S*$").is_match(u)
}

fn save_repos(root: &Path, repos: Vec<String>) -> Result<()> {
    std::fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;
    write_atomic(&root.join(REPOS_FILE), serde_json::to_string_pretty(&SavedRepos { repos })?)
}

/// Saved repository index URLs, in the order they were added. Missing or unreadable file = none.
pub fn list_repos(root: &Path) -> Vec<String> {
    std::fs::read_to_string(root.join(REPOS_FILE))
        .ok()
        .and_then(|t| serde_json::from_str::<SavedRepos>(&t).ok())
        .map(|s| s.repos)
        .unwrap_or_default()
}

/// Save a repository (whatever the user pasted is normalised to its index URL, see
/// [`normalize_repo_url`]). Adding the same repo twice keeps one entry. Returns the stored URL.
/// Does not touch the network; use [`add_repo_verified`] to check the index first.
pub fn add_repo(root: &Path, url: &str) -> Result<String> {
    let normalized = normalize_repo_url(url);
    if !is_http_url(&normalized) {
        bail!("Enter the repository address, for example https://github.com/keiyoushi/extensions");
    }
    let mut repos = list_repos(root);
    if !repos.contains(&normalized) {
        repos.push(normalized.clone());
        save_repos(root, repos)?;
    }
    Ok(normalized)
}

/// Forget a saved repository (the stored index URL or anything that normalises to it).
/// Installed extensions stay installed. Returns false if the repo was not saved.
pub fn remove_repo(root: &Path, url: &str) -> Result<bool> {
    let (raw, normalized) = (url.trim().to_string(), normalize_repo_url(url));
    let repos = list_repos(root);
    let kept: Vec<String> = repos.iter().filter(|r| **r != raw && **r != normalized).cloned().collect();
    if kept.len() == repos.len() {
        return Ok(false);
    }
    save_repos(root, kept)?;
    Ok(true)
}

/// Fetch a pasted repository and, only when it lists extensions, save it. Returns the stored index
/// URL and the extension list so the caller can show it straight away.
pub async fn add_repo_verified(root: &Path, url: &str, backend: &dyn FetchBackend) -> Result<(String, Vec<RepoExtension>)> {
    let normalized = normalize_repo_url(url);
    if !is_http_url(&normalized) {
        bail!("Enter the repository address, for example https://github.com/keiyoushi/extensions");
    }
    let exts = fetch_repo_extensions(&normalized, backend).await?;
    if exts.is_empty() {
        bail!("{normalized} does not list any extensions");
    }
    add_repo(root, &normalized)?;
    Ok((normalized, exts))
}

/// Icon of an extension: the one the index lists, else the classic repo layout (`<repo>/icon/<pkg>.png`).
/// Best effort: not every repo ships icons, so the UI should tolerate a failed load.
pub fn extension_icon_url(repo_url: &str, ext: &RepoExtension) -> String {
    if let Some(u) = ext.icon_url.as_ref().filter(|u| is_http_url(u)) {
        return u.clone();
    }
    let index = normalize_repo_url(repo_url);
    let dir = index.rsplit_once('/').map_or(index.as_str(), |(d, _)| d);
    format!("{dir}/icon/{}.png", ext.pkg)
}

// ---------------------------------------------------------------------------------------------
// Installing repo extensions
// ---------------------------------------------------------------------------------------------

/// Outcome of [`install_repo_extension`]. The plugin is installed even when `warning` is set.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RepoInstallResult {
    /// Plugin id = folder name under the plugins dir (also the source id in the registry).
    pub id: String,
    pub name: String,
    /// Detected engine; `Auto` = the plugin detects it on first use (site was unreachable or walled).
    pub engine: Engine,
    /// Non-fatal problem found while validating (popular list empty or the test request failed).
    pub warning: Option<String>,
}

fn slug(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' }).collect()
}

fn source_lang(ext: &RepoExtension, source_index: usize) -> &str {
    ext.sources
        .get(source_index)
        .map(|s| s.lang.as_str())
        .filter(|l| !l.is_empty())
        .or(Some(ext.lang.as_str()).filter(|l| !l.is_empty()))
        .unwrap_or("en")
}

/// Plugin id of one source of an extension: `repo-<pkg with dots as dashes>-<lang>` (the Electron app's
/// ids). A second source of the same extension and language gets `-<index>` so they cannot overwrite
/// each other.
pub fn repo_plugin_id(ext: &RepoExtension, source_index: usize) -> String {
    let lang = source_lang(ext, source_index);
    let mut id = format!("repo-{}-{}", slug(&ext.pkg), slug(lang));
    if (0..source_index.min(ext.sources.len())).any(|i| source_lang(ext, i) == lang) {
        id.push_str(&format!("-{source_index}"));
    }
    id
}

/// `path` written via a temp file so a crash cannot leave a half-written plugin file.
fn write_atomic(path: &Path, data: impl AsRef<[u8]>) -> Result<()> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!("{name}.tmp"));
    std::fs::write(&tmp, data).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        anyhow!("writing {}: {e}", path.display())
    })
}

/// An existing install made by the old generator (or flagged legacy) keeps its id shapes.
fn has_legacy_ids(plugin_dir: &Path) -> bool {
    let Ok(m) = read_manifest(plugin_dir) else { return false };
    if let Some(style) = m.extra.get("id_style").and_then(Value::as_str) {
        return style == "legacy";
    }
    std::fs::read_to_string(plugin_dir.join(&m.entry_point)).is_ok_and(|c| c.contains(LEGACY_HEADER))
}

/// Install one source of a Tachiyomi/Mihon-compatible repo extension into `plugins_dir/<id>/`.
///
/// Mihon extensions are Android APKs (Kotlin) we cannot run, so the site's engine is fingerprinted
/// (Madara, MangaThemesia, MangaBox, FoolSlide, HeanCMS, Keyoapp, or a heuristic generic fallback) and
/// a sandboxed JS plugin is generated from the matching template. The plugin re-detects lazily if the
/// stored engine stops working. The generated code is validated by loading it in the sandbox before
/// anything is written; afterwards `getPopularManga(1)` is smoke-tested and an empty or failed result
/// is reported as `warning` only (the plugin stays installed). Installing again updates in place.
///
/// Files written: `manifest.json`, `index.js` and `.repo_source` (the repo entry as JSON, which marks
/// the folder as repo-installed for [`installed_repo_plugins`] and [`check_repo_updates`]).
pub async fn install_repo_extension(
    ext: &RepoExtension,
    source_index: usize,
    plugins_dir: &Path,
    backend: Arc<dyn FetchBackend>,
) -> Result<RepoInstallResult> {
    let source = ext.sources.get(source_index).ok_or_else(|| anyhow!("No source found at given index"))?;
    let name = [source.name.as_str(), ext.name.as_str(), ext.pkg.as_str()]
        .into_iter()
        .map(str::trim)
        .find(|n| !n.is_empty())
        .unwrap_or("Source")
        .to_string();
    let plugin_id = repo_plugin_id(ext, source_index);
    if !safe_id(&plugin_id) {
        bail!("Invalid plugin id: {plugin_id}");
    }
    let base_url = normalize_base_url(&source.base_url);
    if host_of(&base_url).is_empty() {
        bail!("{name} has no website address in the repository index");
    }
    let lang = source_lang(ext, source_index).to_string();
    let plugin_dir = plugins_dir.join(&plugin_id);
    let legacy_ids = has_legacy_ids(&plugin_dir);

    // 1. Detect the site engine (falls back to Auto = detect lazily at runtime).
    let probe = tokio::time::timeout(
        Duration::from_secs(60),
        detect_site_engine(&base_url, backend.as_ref(), &plugin_id, Some(&ext.pkg)),
    )
    .await
    .unwrap_or_else(|_| SiteProbe {
        engine: Engine::Auto,
        api_base: None,
        note: Some("engine detection timed out after 60s".into()),
    });
    eprintln!("[repo] {name}: engine={}{}", probe.engine, probe.note.as_ref().map(|n| format!(" ({n})")).unwrap_or_default());

    // 2. Generate the plugin from the matching template.
    let mut extra = serde_json::Map::new();
    extra.insert("engine".into(), json!(probe.engine.as_str()));
    extra.insert("generator".into(), json!("mangavox-template"));
    extra.insert("template_version".into(), json!(TEMPLATE_VERSION));
    extra.insert("repo_pkg".into(), json!(ext.pkg));
    extra.insert("id_style".into(), json!(if legacy_ids { "legacy" } else { "path" }));
    if let Some(api) = &probe.api_base {
        extra.insert("api_base".into(), json!(api));
    }
    let manifest = PluginManifest {
        id: plugin_id.clone(),
        name: name.clone(),
        version: if ext.version.is_empty() { "0.0.0".into() } else { ext.version.clone() },
        base_url: base_url.clone(),
        lang: lang.clone(),
        entry_point: "index.js".into(),
        nsfw: ext.nsfw != 0,
        extra,
    };
    let code = generate_template_plugin(&TemplateOptions {
        name: name.clone(),
        base_url,
        lang,
        engine: probe.engine,
        api_base: probe.api_base.clone(),
        legacy_ids,
    });
    let opts = PluginOptions { call_timeout: Duration::from_secs(60), ..Default::default() };
    let plugin = Plugin::from_source(manifest.clone(), &code, opts, backend)
        .await
        .map_err(|e| anyhow!("Plugin validation failed: {e}"))?;

    // 3. Save before the smoke test so a slow site cannot lose the install.
    std::fs::create_dir_all(&plugin_dir).with_context(|| format!("creating {}", plugin_dir.display()))?;
    write_atomic(&plugin_dir.join("index.js"), &code)?;
    write_atomic(&plugin_dir.join("manifest.json"), serde_json::to_string_pretty(&manifest)?)?;
    write_atomic(&plugin_dir.join(".repo_source"), serde_json::to_string(ext)?)?;

    // 4. Smoke test: popular page 1 should return something.
    let warning = match plugin.get_popular_manga(1).await {
        Ok(v) if v.as_array().is_some_and(|a| !a.is_empty()) => {
            eprintln!("[repo] {name}: validation OK ({} popular titles)", v.as_array().map_or(0, Vec::len));
            None
        }
        Ok(_) => Some(format!(
            "{name} installed, but its popular list came back empty - the site may be down, behind an anti-bot wall, or use an unsupported layout."
        )),
        Err(e) => Some(format!("{name} installed, but a test request failed: {e:#}")),
    };
    if let Some(w) = &warning {
        eprintln!("[repo] {w}");
    }
    Ok(RepoInstallResult { id: plugin_id, name, engine: probe.engine, warning })
}

// ---------------------------------------------------------------------------------------------
// Repo-installed plugins: listing, updates, template upgrades
// ---------------------------------------------------------------------------------------------

/// A plugin that was installed from a repo (has a `.repo_source` file).
#[derive(Debug, Clone, PartialEq)]
pub struct InstalledRepoPlugin {
    pub manifest: PluginManifest,
    /// The repo entry it was installed from (version/code as installed).
    pub extension: RepoExtension,
}

/// The repo entry a plugin was installed from, `None` for plugins not installed from a repo.
pub fn read_repo_source(plugins_dir: &Path, id: &str) -> Option<RepoExtension> {
    if !safe_id(id) {
        return None;
    }
    serde_json::from_str(&std::fs::read_to_string(plugins_dir.join(id).join(".repo_source")).ok()?).ok()
}

/// Every installed plugin that came from a repo, sorted by plugin id.
pub fn installed_repo_plugins(plugins_dir: &Path) -> Vec<InstalledRepoPlugin> {
    list_installed(plugins_dir)
        .into_iter()
        .filter_map(|m| Some(InstalledRepoPlugin { extension: read_repo_source(plugins_dir, &m.id)?, manifest: m }))
        .collect()
}

/// A newer version of an installed repo extension.
#[derive(Debug, Clone, PartialEq)]
pub struct RepoUpdate {
    pub plugin_id: String,
    pub name: String,
    pub installed_version: String,
    pub available_version: String,
    /// Pass these two to [`install_repo_extension`] to update.
    pub extension: RepoExtension,
    pub source_index: usize,
}

fn is_newer(installed: &RepoExtension, remote: &RepoExtension) -> bool {
    if installed.code > 0 || remote.code > 0 {
        remote.code > installed.code
    } else {
        !remote.version.is_empty() && remote.version != installed.version
    }
}

/// Look the installed repo plugins up in `repo_urls` (typically [`list_repos`]) and report the ones
/// with a newer version. Unreachable repos are skipped; no network is used without repo plugins.
pub async fn check_repo_updates(plugins_dir: &Path, repo_urls: &[String], backend: &dyn FetchBackend) -> Vec<RepoUpdate> {
    let installed = installed_repo_plugins(plugins_dir);
    if installed.is_empty() {
        return vec![];
    }
    let mut latest: HashMap<String, RepoExtension> = HashMap::new();
    for url in repo_urls {
        match fetch_repo_extensions(url, backend).await {
            Ok(list) => {
                for e in list {
                    if latest.get(&e.pkg).is_none_or(|cur| is_newer(cur, &e)) {
                        latest.insert(e.pkg.clone(), e);
                    }
                }
            }
            Err(e) => eprintln!("[repo] {url}: {e:#}"),
        }
    }
    installed
        .into_iter()
        .filter_map(|p| {
            let remote = latest.get(&p.extension.pkg).filter(|r| is_newer(&p.extension, r))?;
            // The source to update is the one that yields the installed plugin's id.
            let source_index = (0..remote.sources.len()).find(|i| repo_plugin_id(remote, *i) == p.manifest.id)?;
            Some(RepoUpdate {
                plugin_id: p.manifest.id,
                name: p.manifest.name,
                installed_version: p.manifest.version,
                available_version: remote.version.clone(),
                extension: remote.clone(),
                source_index,
            })
        })
        .collect()
}

/// Regenerate repo plugins on disk that were produced by the old generator or an older template
/// version. Synchronous and offline: the new code detects the engine lazily when the manifest has
/// none. Call it before loading the plugins at startup. Returns how many were regenerated; the
/// previous code is kept as `index.js.bak`.
pub fn upgrade_repo_plugins(plugins_dir: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(plugins_dir) else { return 0 };
    let mut upgraded = 0;
    for e in rd.flatten() {
        let dir = e.path();
        if !dir.is_dir() || !e.file_name().to_string_lossy().starts_with("repo-") {
            continue;
        }
        match upgrade_one(&dir) {
            Ok(true) => upgraded += 1,
            Ok(false) => {}
            Err(err) => eprintln!("[repo] failed to upgrade plugin in {}: {err:#}", dir.display()),
        }
    }
    upgraded
}

fn upgrade_one(dir: &Path) -> Result<bool> {
    if !dir.join("manifest.json").exists() {
        return Ok(false);
    }
    let mut m = read_manifest(dir)?;
    let entry = dir.join(&m.entry_point);
    let Ok(code) = std::fs::read_to_string(&entry) else { return Ok(false) };
    if !needs_regeneration(&code) {
        return Ok(false);
    }
    let legacy = code.contains(LEGACY_HEADER);
    let base_src = if m.base_url.is_empty() {
        re(r"// Base URL: (\S+)").captures(&code).map(|c| c[1].to_string()).unwrap_or_default()
    } else {
        m.base_url.clone()
    };
    let base_url = normalize_base_url(&base_src);
    if host_of(&base_url).is_empty() {
        return Ok(false);
    }
    let str_extra = |k: &str| m.extra.get(k).and_then(Value::as_str).map(str::to_string);
    let pkg = str_extra("repo_pkg").or_else(|| {
        std::fs::read_to_string(dir.join(".repo_source"))
            .ok()
            .and_then(|t| serde_json::from_str::<RepoExtension>(&t).ok())
            .map(|e| e.pkg)
    });
    // The old generator never detected anything: let the runtime detect.
    let engine = if legacy { Engine::Auto } else { str_extra("engine").and_then(|e| Engine::from_id(&e)).unwrap_or(Engine::Auto) };
    let legacy_ids = legacy || str_extra("id_style").as_deref() == Some("legacy");
    let next = generate_template_plugin(&TemplateOptions {
        name: m.name.clone(),
        base_url: base_url.clone(),
        lang: m.lang.clone(),
        engine,
        api_base: str_extra("api_base"),
        legacy_ids,
    });
    m.base_url = base_url;
    m.extra.insert("engine".into(), json!(engine.as_str()));
    m.extra.insert("generator".into(), json!("mangavox-template"));
    m.extra.insert("template_version".into(), json!(TEMPLATE_VERSION));
    m.extra.insert("id_style".into(), json!(if legacy_ids { "legacy" } else { "path" }));
    if let Some(p) = pkg {
        m.extra.insert("repo_pkg".into(), json!(p));
    }
    let mut bak = entry.clone().into_os_string();
    bak.push(".bak");
    std::fs::write(&bak, &code).with_context(|| format!("writing {}", Path::new(&bak).display()))?;
    write_atomic(&entry, next)?;
    write_atomic(&dir.join("manifest.json"), serde_json::to_string_pretty(&m)?)?;
    eprintln!("[repo] regenerated {} ({}) with template v{TEMPLATE_VERSION} (engine: {engine})", m.name, m.id);
    Ok(true)
}
