//! Plugin repo basics, mirroring `repo-manager.ts` / `plugin-loader.ts`:
//! repo index URL normalisation + parsing (Tachiyomi/Mihon formats), plugin
//! URL resolution, install/uninstall/list into an injectable plugins dir.
//!
//! Not ported here: template generation for Mihon extensions
//! (`installRepoExtension`, `src/main/plugins/templates`).

use crate::backend::{FetchBackend, FetchRequest};
use crate::host::{Plugin, PluginOptions};
use crate::manifest::{read_manifest, PluginManifest};
use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

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
}

fn re(p: &str) -> Regex {
    Regex::new(p).expect("static regex")
}

/// Normalize whatever the user pasted to an `index.min.json` / `index.json` URL.
pub fn normalize_repo_url(input: &str) -> String {
    let url = input.trim().trim_end_matches('/').to_string();
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
fn is_stub_index(data: &Value) -> bool {
    let Some(a) = data.as_array() else { return false };
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
            code: e.get("versionCode").and_then(Value::as_i64).unwrap_or(0),
            version: e.get("versionName").and_then(Value::as_str).unwrap_or("1.0").to_string(),
            nsfw: i64::from(e.get("contentWarning").and_then(Value::as_str) == Some("CONTENT_WARNING_NSFW")),
            sources,
        });
    }
    Some(out)
}

/// Parse an index document. `None` = not a usable index (invalid JSON or stub).
pub fn parse_repo_index(text: &str) -> Option<Vec<RepoExtension>> {
    let data: Value = serde_json::from_str(text).ok()?;
    if data.is_array() {
        if is_stub_index(&data) {
            return None;
        }
        return serde_json::from_value(data).ok();
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
