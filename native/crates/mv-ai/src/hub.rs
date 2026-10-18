//! Model Hub: search Hugging Face, inspect GGUF files, download (resumable) and import into Ollama.
//! Downloads land in `<models dir>/<owner>__<repo>/`.

use crate::config::{object_entry, ConfigStore};
use crate::gguf::group_gguf_files;
use crate::providers::gemini::encode_uri_component;
use crate::stream::{AiError, Result};
use crate::types::*;
use futures_util::StreamExt;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

pub const HF: &str = "https://huggingface.co";
const META: &str = "mangavox-models.json";

/// Called with a snapshot whenever a download changes state or makes progress.
pub type ProgressFn = Arc<dyn Fn(HubDownload) + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HubSort {
    #[default]
    Downloads,
    Likes,
    LastModified,
    TrendingScore,
}

impl HubSort {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Downloads => "downloads",
            Self::Likes => "likes",
            Self::LastModified => "lastModified",
            Self::TrendingScore => "trendingScore",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchOpts {
    pub sort: HubSort,
    pub gguf_only: bool,
    pub limit: u32,
}

impl Default for SearchOpts {
    fn default() -> Self {
        Self { sort: HubSort::Downloads, gguf_only: true, limit: 40 }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HubConfigPatch {
    pub hf_token: Option<String>,
    pub models_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HubConfigPublic {
    pub has_token: bool,
    pub models_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportResult {
    pub ok: bool,
    pub name: Option<String>,
    pub error: Option<String>,
}

struct Entry {
    dl: HubDownload,
    cancel: CancellationToken,
}

#[derive(Clone)]
pub struct ModelHub {
    client: reqwest::Client,
    config: ConfigStore,
    default_models_dir: PathBuf,
    cache_dir: PathBuf,
    hf_base: String,
    ollama_bin: Option<PathBuf>,
    downloads: Arc<Mutex<HashMap<String, Entry>>>,
}

#[derive(Deserialize)]
struct HfListItem {
    id: String,
    downloads: Option<u64>,
    likes: Option<u64>,
    #[serde(rename = "lastModified")]
    last_modified: Option<String>,
    tags: Option<Vec<String>>,
    pipeline_tag: Option<String>,
    gated: Option<Value>,
}

fn to_summary(m: HfListItem) -> HubModelSummary {
    let (author, name) = match m.id.split_once('/') {
        Some((a, n)) => (a.to_string(), n.to_string()),
        None => (m.id.clone(), m.id.clone()),
    };
    let gated = match &m.gated {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(s)) => !s.is_empty(),
        _ => true,
    };
    HubModelSummary {
        repo: m.id,
        author,
        name,
        downloads: m.downloads.unwrap_or(0),
        likes: m.likes.unwrap_or(0),
        updated_at: m.last_modified,
        tags: m.tags.unwrap_or_default(),
        pipeline: m.pipeline_tag,
        gated,
    }
}

fn valid_repo(repo: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[\w.-]+/[\w.-]+$").unwrap()).is_match(repo) && !repo.split('/').any(|p| p.chars().all(|c| c == '.'))
}

pub fn safe_name(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[^\w.-]+").unwrap()).replace_all(s, "_").into_owned()
}

/// First lines of a model card without front-matter, images, tables and code.
pub fn readme_excerpt(md: &str) -> String {
    static FM: OnceLock<Regex> = OnceLock::new();
    static IMG: OnceLock<Regex> = OnceLock::new();
    static SKIP: OnceLock<Regex> = OnceLock::new();
    let fm = FM.get_or_init(|| Regex::new(r"^---[\s\S]*?---\s*").unwrap());
    let img = IMG.get_or_init(|| Regex::new(r"!\[[^\]]*\]\([^)]*\)").unwrap());
    let skip = SKIP.get_or_init(|| Regex::new(r"^\s*(<|!\[|\||```)").unwrap());
    let body = img.replace_all(&fm.replace(md, ""), "").into_owned();
    let joined = body.lines().filter(|l| !l.trim().is_empty() && !skip.is_match(l)).take(14).collect::<Vec<_>>().join("\n");
    joined.chars().take(1800).collect()
}

fn file_name(p: &str) -> String {
    p.rsplit(['/', '\\']).next().unwrap_or(p).to_string()
}

impl ModelHub {
    /// `config_path`: the app's config.json. `data_root`: e.g. `~/.mangavox` (default models dir is
    /// `<data_root>/models`, scratch files go to `<data_root>/cache`).
    pub fn new(config_path: impl Into<PathBuf>, data_root: impl AsRef<Path>) -> Self {
        let root = data_root.as_ref();
        Self {
            client: crate::stream::build_client(),
            config: ConfigStore::new(config_path),
            default_models_dir: root.join("models"),
            cache_dir: root.join("cache"),
            hf_base: HF.to_string(),
            ollama_bin: None,
            downloads: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Point at a different Hugging Face host (tests).
    pub fn with_hf_base(mut self, base: impl Into<String>) -> Self {
        self.hf_base = base.into().trim_end_matches('/').to_string();
        self
    }

    /// Use a specific `ollama` executable instead of searching.
    pub fn with_ollama_binary(mut self, bin: impl Into<PathBuf>) -> Self {
        self.ollama_bin = Some(bin.into());
        self
    }

    fn hub_config(&self) -> (Option<String>, Option<String>) {
        let cfg = self.config.read();
        let hub = cfg.get("aiAssistant").and_then(|a| a.get("hub"));
        let get = |k: &str| hub.and_then(|h| h.get(k)).and_then(Value::as_str).filter(|s| !s.trim().is_empty()).map(String::from);
        (get("hfToken"), get("modelsDir"))
    }

    pub fn models_dir(&self) -> PathBuf {
        let dir = match self.hub_config().1 {
            Some(d) => PathBuf::from(d.trim()),
            None => self.default_models_dir.clone(),
        };
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn hf_headers(&self) -> Vec<(&'static str, String)> {
        match self.hub_config().0 {
            Some(t) => vec![("authorization", format!("Bearer {t}"))],
            None => vec![],
        }
    }

    pub fn set_hub_config(&self, patch: &HubConfigPatch) -> std::io::Result<()> {
        self.config.update(|cfg| {
            let hub = object_entry(object_entry(cfg, "aiAssistant"), "hub");
            if let Some(t) = &patch.hf_token {
                hub.insert("hfToken".into(), Value::String(t.clone()));
            }
            if let Some(d) = &patch.models_dir {
                hub.insert("modelsDir".into(), Value::String(d.clone()));
            }
            hub.retain(|_, v| v.as_str().is_some_and(|s| !s.is_empty()));
        })
    }

    pub fn hub_config_public(&self) -> HubConfigPublic {
        HubConfigPublic { has_token: self.hub_config().0.is_some(), models_dir: self.models_dir() }
    }

    async fn hf_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        let mut rb = self.client.get(url).timeout(Duration::from_secs(15));
        for (k, v) in self.hf_headers() {
            rb = rb.header(k, v);
        }
        let res = rb.send().await?;
        let status = res.status().as_u16();
        if !res.status().is_success() {
            let hint = if status == 401 || status == 403 { " — this repo may be gated; add a token in Model Hub settings" } else { "" };
            return Err(AiError::http(format!("Hugging Face {status}{hint}"), Some(status)));
        }
        Ok(res.json::<T>().await?)
    }

    // ── Search & detail ─────────────────────────────────────────────────────

    pub async fn search_hub(&self, query: &str, opts: &SearchOpts) -> Result<Vec<HubModelSummary>> {
        let mut params = vec![("sort", opts.sort.as_str().to_string()), ("direction", "-1".into()), ("limit", opts.limit.to_string())];
        if !query.trim().is_empty() {
            params.push(("search", query.trim().to_string()));
        }
        if opts.gguf_only {
            params.push(("filter", "gguf".into()));
        }
        let url = reqwest::Url::parse_with_params(&format!("{}/api/models", self.hf_base), &params).map_err(|e| AiError::msg(e.to_string()))?;
        let list: Vec<HfListItem> = self.hf_json(url.as_str()).await?;
        Ok(list.into_iter().map(to_summary).collect())
    }

    pub async fn hub_detail(&self, repo: &str) -> Result<HubModelDetail> {
        if !valid_repo(repo) {
            return Err(AiError::msg("Invalid repo id"));
        }
        let info_url = format!("{}/api/models/{repo}", self.hf_base);
        let tree_url = format!("{}/api/models/{repo}/tree/main?recursive=true", self.hf_base);
        let (info, tree) = tokio::join!(self.hf_json::<Value>(&info_url), self.hf_json::<Vec<Value>>(&tree_url));
        let info = info?;
        let tree = tree.unwrap_or_default();

        let mut description = None;
        let mut rb = self.client.get(format!("{}/{repo}/raw/main/README.md", self.hf_base)).timeout(Duration::from_secs(8));
        for (k, v) in self.hf_headers() {
            rb = rb.header(k, v);
        }
        if let Ok(res) = rb.send().await {
            if res.status().is_success() {
                if let Ok(text) = res.text().await {
                    description = Some(readme_excerpt(&text));
                }
            }
        }

        let entries: Vec<(String, u64)> = tree
            .iter()
            .filter(|e| e.get("type").and_then(Value::as_str) == Some("file"))
            .filter_map(|e| {
                let path = e.get("path")?.as_str()?.to_string();
                let size = e.pointer("/lfs/size").and_then(Value::as_u64).or_else(|| e.get("size").and_then(Value::as_u64)).unwrap_or(0);
                Some((path, size))
            })
            .collect();
        let files = group_gguf_files(&entries);

        let gguf = info.get("gguf").cloned().unwrap_or(Value::Null);
        let params = gguf.get("total").and_then(Value::as_f64).or_else(|| info.pointer("/safetensors/total").and_then(Value::as_f64));
        let license = info
            .pointer("/cardData/license")
            .and_then(Value::as_str)
            .map(String::from)
            .or_else(|| {
                info.get("tags")
                    .and_then(Value::as_array)
                    .and_then(|t| t.iter().filter_map(Value::as_str).find_map(|t| t.strip_prefix("license:")))
                    .map(String::from)
            });
        let arch = gguf
            .get("architecture")
            .and_then(Value::as_str)
            .or_else(|| info.pointer("/config/model_type").and_then(Value::as_str))
            .map(String::from);
        let summary = serde_json::from_value::<HfListItem>(info.clone()).map(to_summary).map_err(|e| AiError::msg(e.to_string()))?;
        Ok(HubModelDetail {
            summary,
            license,
            arch,
            params_b: params.map(|p| ((p / 1e9) * 10.0).round() / 10.0),
            context: gguf.get("context_length").and_then(Value::as_u64),
            description,
            files,
        })
    }

    // ── Downloads ───────────────────────────────────────────────────────────

    pub fn list_downloads(&self) -> Vec<HubDownload> {
        self.downloads.lock().unwrap().values().map(|e| e.dl.clone()).collect()
    }

    pub fn cancel_download(&self, id: &str) -> bool {
        match self.downloads.lock().unwrap().get(id) {
            Some(e) if e.dl.state.is_active() => {
                e.cancel.cancel();
                true
            }
            _ => false,
        }
    }

    /// Wait until a download leaves the queued/downloading states.
    pub async fn wait_download(&self, id: &str) -> Option<HubDownload> {
        loop {
            let dl = self.downloads.lock().unwrap().get(id).map(|e| e.dl.clone())?;
            if !dl.state.is_active() {
                return Some(dl);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Start downloading one file (or every shard of a split GGUF) from a repo. Must run inside a tokio runtime.
    pub fn download_hub_file(&self, repo: &str, file: &HubModelFile, progress: ProgressFn) -> Result<HubDownload> {
        if !valid_repo(repo) {
            return Err(AiError::msg("Invalid repo id"));
        }
        let parts = match &file.parts {
            Some(p) if !p.is_empty() => p.clone(),
            _ => vec![file.path.clone()],
        };
        let dir = self.models_dir().join(safe_name(&repo.replace('/', "__")));
        let urls: Vec<(String, PathBuf)> = parts
            .iter()
            .map(|p| {
                let enc = p.split('/').map(encode_uri_component).collect::<Vec<_>>().join("/");
                (format!("{}/{repo}/resolve/main/{enc}", self.hf_base), dir.join(file_name(p)))
            })
            .collect();
        Ok(self.start_download(Some(repo.to_string()), file_name(&parts[0]), file.size, urls, progress))
    }

    /// Download any direct link (e.g. a .gguf from a website).
    pub fn download_url(&self, url: &str, progress: ProgressFn) -> Result<HubDownload> {
        let u = reqwest::Url::parse(url).map_err(|_| AiError::msg("That is not a valid URL"))?;
        if !matches!(u.scheme(), "http" | "https") {
            return Err(AiError::msg("Only http(s) links can be downloaded"));
        }
        let last = u.path_segments().and_then(|mut s| s.next_back()).unwrap_or("");
        let decoded = percent_decode(last);
        let name = safe_name(if decoded.is_empty() { "model.gguf" } else { &decoded });
        let dest = self.models_dir().join("direct").join(&name);
        Ok(self.start_download(None, name, 0, vec![(u.to_string(), dest)], progress))
    }

    fn start_download(&self, repo: Option<String>, file: String, total: u64, urls: Vec<(String, PathBuf)>, progress: ProgressFn) -> HubDownload {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dest0 = urls[0].1.to_string_lossy().into_owned();
        let mut map = self.downloads.lock().unwrap();
        if let Some(e) = map.values().find(|e| e.dl.dest == dest0 && e.dl.state.is_active()) {
            return e.dl.clone();
        }
        let id = format!("dl_{}_{}", crate::stream::now_millis(), SEQ.fetch_add(1, Ordering::Relaxed));
        let dl = HubDownload {
            id: id.clone(),
            repo,
            file,
            url: urls[0].0.clone(),
            dest: dest0,
            received: 0,
            total,
            bytes_per_sec: 0,
            state: HubDownloadState::Queued,
            error: None,
            imported_as: None,
        };
        let cancel = CancellationToken::new();
        map.insert(id.clone(), Entry { dl: dl.clone(), cancel: cancel.clone() });
        drop(map);
        let hub = self.clone();
        tokio::spawn(async move { hub.run_download(id, urls, cancel, progress).await });
        dl
    }

    /// Mutate a download entry and notify the progress callback.
    fn update(&self, id: &str, progress: &ProgressFn, f: impl FnOnce(&mut HubDownload)) {
        let snap = {
            let mut map = self.downloads.lock().unwrap();
            let Some(e) = map.get_mut(id) else { return };
            f(&mut e.dl);
            e.dl.clone()
        };
        progress(snap);
    }

    async fn run_download(&self, id: String, urls: Vec<(String, PathBuf)>, cancel: CancellationToken, progress: ProgressFn) {
        self.update(&id, &progress, |d| d.state = HubDownloadState::Downloading);
        let outcome = self.download_all(&id, &urls, &cancel, &progress).await;
        self.update(&id, &progress, |d| {
            d.bytes_per_sec = 0;
            match &outcome {
                Ok(()) => d.state = HubDownloadState::Done,
                Err(_) if cancel.is_cancelled() => d.state = HubDownloadState::Cancelled,
                Err(e) => {
                    d.state = HubDownloadState::Error;
                    d.error = Some(e.to_string());
                }
            }
        });
        if outcome.is_ok() {
            let snap = self.downloads.lock().unwrap().get(&id).map(|e| e.dl.clone());
            if let Some(d) = snap {
                let _ = self.write_meta(&d);
            }
        }
    }

    async fn download_all(&self, id: &str, urls: &[(String, PathBuf)], cancel: &CancellationToken, progress: &ProgressFn) -> Result<()> {
        let mut done_before: u64 = 0;
        let mut last_emit = Instant::now() - Duration::from_secs(1);
        let mut window_start = Instant::now();
        let mut window_bytes: u64 = 0;
        for (url, dest) in urls {
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            if let Ok(md) = tokio::fs::metadata(dest).await {
                done_before += md.len();
                continue;
            }
            let part = PathBuf::from(format!("{}.part", dest.to_string_lossy()));
            let mut have = tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);
            let mut res = self.get_range(url, have, cancel).await?;
            if res.status().as_u16() == 416 && have > 0 {
                // stale partial file: start over
                let _ = tokio::fs::remove_file(&part).await;
                have = 0;
                res = self.get_range(url, 0, cancel).await?;
            }
            let status = res.status().as_u16();
            if !res.status().is_success() {
                return Err(AiError::msg(if status == 401 || status == 403 {
                    "Access denied — this model is gated. Accept its licence on huggingface.co and add your token.".to_string()
                } else {
                    format!("Download failed ({status})")
                }));
            }
            let resumed = status == 206;
            let len = res.content_length().unwrap_or(0);
            if urls.len() == 1 && len > 0 {
                let total = len + if resumed { have } else { 0 };
                self.update(id, progress, |d| {
                    if d.total == 0 {
                        d.total = total
                    }
                });
            }
            let mut received = if resumed { have } else { 0 };
            let mut out = tokio::fs::OpenOptions::new().create(true).write(true).append(resumed).truncate(!resumed).open(&part).await?;
            let mut stream = res.bytes_stream();
            loop {
                let next = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => { let _ = out.flush().await; return Err(AiError::Cancelled); }
                    n = stream.next() => n,
                };
                let Some(chunk) = next else { break };
                let chunk = chunk?;
                out.write_all(&chunk).await?;
                received += chunk.len() as u64;
                window_bytes += chunk.len() as u64;
                let now = Instant::now();
                let mut rate = None;
                if now.duration_since(window_start) >= Duration::from_secs(1) {
                    rate = Some(window_bytes * 1000 / now.duration_since(window_start).as_millis().max(1) as u64);
                    window_start = now;
                    window_bytes = 0;
                }
                if rate.is_some() || now.duration_since(last_emit) > Duration::from_millis(300) {
                    last_emit = now;
                    let total_received = done_before + received;
                    self.update(id, progress, |d| {
                        d.received = total_received;
                        if let Some(r) = rate {
                            d.bytes_per_sec = r;
                        }
                    });
                }
            }
            out.flush().await?;
            drop(out);
            tokio::fs::rename(&part, dest).await?;
            done_before += received;
        }
        self.update(id, progress, |d| {
            d.received = done_before;
            if d.total == 0 {
                d.total = done_before;
            }
        });
        Ok(())
    }

    async fn get_range(&self, url: &str, have: u64, cancel: &CancellationToken) -> Result<reqwest::Response> {
        let mut rb = self.client.get(url);
        if url.starts_with(&self.hf_base) {
            for (k, v) in self.hf_headers() {
                rb = rb.header(k, v);
            }
        }
        if have > 0 {
            rb = rb.header("range", format!("bytes={have}-"));
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(AiError::Cancelled),
            r = rb.send() => Ok(r?),
        }
    }

    // ── Local files ─────────────────────────────────────────────────────────

    fn meta_path(&self) -> PathBuf {
        self.models_dir().join(META)
    }

    fn read_meta(&self) -> serde_json::Map<String, Value> {
        match std::fs::read_to_string(self.meta_path()).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
            Some(Value::Object(m)) => m,
            _ => Default::default(),
        }
    }

    fn rel(&self, p: &Path) -> String {
        p.strip_prefix(self.models_dir()).unwrap_or(p).to_string_lossy().into_owned()
    }

    fn set_meta(&self, path: &Path, key: &str, value: &str) -> std::io::Result<()> {
        let mut meta = self.read_meta();
        let rel = self.rel(path);
        let entry = object_entry(&mut meta, &rel);
        entry.insert(key.into(), Value::String(value.into()));
        std::fs::write(self.meta_path(), serde_json::to_string_pretty(&Value::Object(meta)).unwrap_or_default())
    }

    fn write_meta(&self, d: &HubDownload) -> std::io::Result<()> {
        match &d.repo {
            Some(repo) => self.set_meta(Path::new(&d.dest), "repo", repo),
            None => Ok(()),
        }
    }

    pub fn list_local_models(&self) -> Vec<LocalModelFile> {
        static EXT: OnceLock<Regex> = OnceLock::new();
        static SHARD: OnceLock<Regex> = OnceLock::new();
        let ext = EXT.get_or_init(|| Regex::new(r"(?i)\.(gguf|safetensors|bin)$").unwrap());
        let shard = SHARD.get_or_init(|| Regex::new(r"(?i)-0000[2-9]-of-|-000[1-9]\d-of-").unwrap());
        let root = self.models_dir();
        let meta = self.read_meta();
        let mut out = Vec::new();
        fn walk(dir: &Path, depth: u32, f: &mut dyn FnMut(&Path, &std::fs::Metadata)) {
            if depth > 3 {
                return;
            }
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let p = e.path();
                let Ok(md) = e.metadata() else { continue };
                if md.is_dir() {
                    walk(&p, depth + 1, f);
                } else {
                    f(&p, &md);
                }
            }
        }
        walk(&root, 0, &mut |p, md| {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if !ext.is_match(&name) || shard.is_match(&name) {
                return;
            }
            let rel = p.strip_prefix(&root).unwrap_or(p).to_string_lossy().into_owned();
            let m = meta.get(&rel);
            let get = |k: &str| m.and_then(|m| m.get(k)).and_then(Value::as_str).map(String::from);
            let modified = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0.0, |d| d.as_secs_f64() * 1000.0);
            out.push(LocalModelFile { path: p.to_string_lossy().into_owned(), name, size: md.len(), modified_at: modified, repo: get("repo"), imported_as: get("importedAs") });
        });
        out.sort_by(|a, b| b.modified_at.partial_cmp(&a.modified_at).unwrap_or(std::cmp::Ordering::Equal));
        out
    }

    fn inside_models_dir(&self, path: &Path) -> bool {
        let root = self.models_dir();
        path != root && path.starts_with(&root) && !path.components().any(|c| matches!(c, Component::ParentDir))
    }

    /// Delete a local model file (all shards of a split GGUF). Only paths inside the models folder.
    pub fn delete_local_model(&self, path: &Path) -> bool {
        static SHARD: OnceLock<Regex> = OnceLock::new();
        if !self.inside_models_dir(path) || !path.exists() {
            return false;
        }
        let s = path.to_string_lossy().into_owned();
        let re = SHARD.get_or_init(|| Regex::new(r"(?i)^(.*)-00001-of-(\d{5})\.gguf$").unwrap());
        if let Some(c) = re.captures(&s) {
            let count: u32 = c[2].parse().unwrap_or(0);
            for i in 1..=count {
                let _ = std::fs::remove_file(format!("{}-{:05}-of-{}.gguf", &c[1], i, &c[2]));
            }
        } else {
            let _ = std::fs::remove_file(path);
        }
        true
    }

    fn resolve_ollama(&self) -> Option<PathBuf> {
        if let Some(b) = &self.ollama_bin {
            return Some(b.clone());
        }
        let exe = if cfg!(windows) { "ollama.exe" } else { "ollama" };
        let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
        if cfg!(windows) {
            if let Some(l) = std::env::var_os("LOCALAPPDATA") {
                dirs.push(PathBuf::from(l).join("Programs").join("Ollama"));
            }
            if let Some(p) = std::env::var_os("ProgramFiles") {
                dirs.push(PathBuf::from(p).join("Ollama"));
            }
        } else {
            dirs.extend(["/usr/local/bin", "/opt/homebrew/bin", "/usr/bin"].map(PathBuf::from));
        }
        dirs.into_iter().map(|d| d.join(exe)).find(|p| p.is_file())
    }

    /// Register a downloaded GGUF with Ollama (`ollama create <name> -f Modelfile`).
    pub async fn import_into_ollama(&self, path: &Path, name: Option<&str>) -> ImportResult {
        static SHARD: OnceLock<Regex> = OnceLock::new();
        static BAD: OnceLock<Regex> = OnceLock::new();
        let fail = |e: &str| ImportResult { ok: false, name: None, error: Some(e.to_string()) };
        if !self.inside_models_dir(path) || !path.exists() {
            return fail("File not found in the models folder");
        }
        let stem = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let stem = SHARD.get_or_init(|| Regex::new(r"(?i)(-00001-of-\d{5})?\.gguf$").unwrap()).replace(&stem, "").into_owned();
        let raw = name.filter(|n| !n.is_empty()).map(String::from).unwrap_or(stem).to_lowercase();
        let cleaned = BAD.get_or_init(|| Regex::new(r"[^a-z0-9._-]+").unwrap()).replace_all(&raw, "-").into_owned();
        let model_name: String = cleaned.trim_matches('-').chars().take(60).collect();
        if model_name.is_empty() {
            return fail("Could not derive a model name");
        }
        let Some(bin) = self.resolve_ollama() else { return fail("Ollama is not installed. Get it from ollama.com.") };
        if std::fs::create_dir_all(&self.cache_dir).is_err() {
            return fail("Could not create the cache folder");
        }
        let modelfile = self.cache_dir.join(format!("Modelfile.{model_name}"));
        if let Err(e) = std::fs::write(&modelfile, format!("FROM {}\n", path.display())) {
            return fail(&e.to_string());
        }
        let mut cmd = tokio::process::Command::new(&bin);
        cmd.args(["create", &model_name, "-f"]).arg(&modelfile).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000);
        let out = match cmd.output().await {
            Ok(o) => o,
            Err(e) => return fail(&e.to_string()),
        };
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let last = stderr.trim().lines().last().unwrap_or("").to_string();
            return fail(&if last.is_empty() { format!("ollama create exited with {}", out.status.code().map_or("null".into(), |c| c.to_string())) } else { last });
        }
        let _ = self.set_meta(path, "importedAs", &model_name);
        ImportResult { ok: true, name: Some(model_name), error: None }
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn readme_excerpt_strips_noise() {
        let md = "---\nlicense: mit\n---\n![banner](x.png)\n# Title\n\n<div>html</div>\n| a | b |\n```py\nx\n```\nReal text line.\n";
        assert_eq!(readme_excerpt(md), "# Title\nx\nReal text line.");
        let long = (0..40).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        assert_eq!(readme_excerpt(&long).lines().count(), 14);
    }

    #[test]
    fn summary_and_names() {
        let item: HfListItem = serde_json::from_str(r#"{"id":"unsloth/Qwen3-GGUF","downloads":5,"gated":"auto","tags":["gguf"]}"#).unwrap();
        let s = to_summary(item);
        assert_eq!((s.author.as_str(), s.name.as_str(), s.downloads, s.gated), ("unsloth", "Qwen3-GGUF", 5, true));
        assert_eq!(safe_name("a b/c"), "a_b_c");
        assert!(valid_repo("a/b.c-d") && !valid_repo("a/b/c") && !valid_repo("../x"));
        assert_eq!(percent_decode("a%20b%2"), "a b%2");
    }

    /// Minimal HTTP server with Range support serving `body` at any path.
    async fn serve(body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let body = body.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_lowercase();
                    let start = req.lines().find_map(|l| l.strip_prefix("range: bytes=")).and_then(|r| r.trim_end_matches('-').trim().parse::<usize>().ok());
                    let head = match start {
                        Some(s) => format!(
                            "HTTP/1.1 206 Partial Content\r\ncontent-length: {}\r\ncontent-range: bytes {}-{}/{}\r\nconnection: close\r\n\r\n",
                            body.len() - s, s, body.len() - 1, body.len()
                        ),
                        None => format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len()),
                    };
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(&body[start.unwrap_or(0)..]).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        format!("http://{addr}")
    }

    fn noop_progress() -> ProgressFn {
        Arc::new(|_| {})
    }

    #[tokio::test]
    async fn download_resumes_from_a_part_file_and_records_meta() {
        let dir = tempfile::tempdir().unwrap();
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let base = serve(body.clone()).await;
        let hub = ModelHub::new(dir.path().join("config.json"), dir.path()).with_hf_base(base);
        // pre-existing partial download
        let target_dir = hub.models_dir().join("acme__tiny");
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::write(target_dir.join("tiny-Q4_K_M.gguf.part"), &body[..50_000]).unwrap();

        let events = Arc::new(Mutex::new(Vec::<HubDownloadState>::new()));
        let ev = events.clone();
        let progress: ProgressFn = Arc::new(move |d| ev.lock().unwrap().push(d.state));
        let file = HubModelFile { path: "tiny-Q4_K_M.gguf".into(), size: 200_000, quant: None, bits: None, parts: None };
        let started = hub.download_hub_file("acme/tiny", &file, progress).unwrap();
        // starting the same destination again returns the running download
        assert_eq!(hub.download_hub_file("acme/tiny", &file, noop_progress()).unwrap().id, started.id);
        let done = hub.wait_download(&started.id).await.unwrap();
        assert_eq!(done.state, HubDownloadState::Done, "{:?}", done.error);
        assert_eq!(done.received, 200_000);
        assert_eq!(std::fs::read(target_dir.join("tiny-Q4_K_M.gguf")).unwrap(), body);
        assert!(!target_dir.join("tiny-Q4_K_M.gguf.part").exists());
        assert!(events.lock().unwrap().contains(&HubDownloadState::Done));

        let local = hub.list_local_models();
        assert_eq!(local.len(), 1);
        assert_eq!(local[0].repo.as_deref(), Some("acme/tiny"));
        assert!(hub.delete_local_model(Path::new(&local[0].path)));
        assert!(hub.list_local_models().is_empty());
        // outside the models folder is refused
        let outside = dir.path().join("config.json");
        std::fs::write(&outside, "{}").unwrap();
        assert!(!hub.delete_local_model(&outside));
    }

    #[tokio::test]
    async fn download_url_and_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let base = serve(vec![7u8; 10]).await;
        let hub = ModelHub::new(dir.path().join("config.json"), dir.path());
        let d = hub.download_url(&format!("{base}/some%20model.gguf"), noop_progress()).unwrap();
        assert_eq!(d.file, "some_model.gguf");
        let done = hub.wait_download(&d.id).await.unwrap();
        assert_eq!(done.state, HubDownloadState::Done);
        assert_eq!(done.total, 10);
        assert!(hub.download_url("ftp://x/y", noop_progress()).is_err());
        assert!(!hub.cancel_download(&d.id)); // already finished

        // an unreachable host ends in the error state, and cancelling before connect works too
        let d = hub.download_url("http://127.0.0.1:9/never.gguf", noop_progress()).unwrap();
        let fin = hub.wait_download(&d.id).await.unwrap();
        assert!(matches!(fin.state, HubDownloadState::Error | HubDownloadState::Cancelled));
    }

    #[tokio::test]
    async fn shard_delete_and_config_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let hub = ModelHub::new(dir.path().join("config.json"), dir.path());
        hub.set_hub_config(&HubConfigPatch { hf_token: Some("hf_secret".into()), models_dir: None }).unwrap();
        let p = hub.hub_config_public();
        assert!(p.has_token);
        assert_eq!(p.models_dir, dir.path().join("models"));
        let m = hub.models_dir().join("r");
        std::fs::create_dir_all(&m).unwrap();
        for i in 1..=2 {
            std::fs::write(m.join(format!("big-0000{i}-of-00002.gguf")), b"x").unwrap();
        }
        // shards other than the first are hidden from the listing
        assert_eq!(hub.list_local_models().len(), 1);
        assert!(hub.delete_local_model(&m.join("big-00001-of-00002.gguf")));
        assert!(!m.join("big-00002-of-00002.gguf").exists());
        hub.set_hub_config(&HubConfigPatch { hf_token: Some(String::new()), models_dir: None }).unwrap();
        assert!(!hub.hub_config_public().has_token);
        let r = hub.import_into_ollama(&dir.path().join("nope.gguf"), None).await;
        assert!(!r.ok);
    }

    /// Live: public Hugging Face search + detail.
    #[tokio::test]
    #[ignore]
    async fn live_hf_search_and_detail() {
        let dir = tempfile::tempdir().unwrap();
        let hub = ModelHub::new(dir.path().join("config.json"), dir.path());
        let found = hub.search_hub("qwen", &SearchOpts { limit: 3, ..Default::default() }).await.unwrap();
        assert!(!found.is_empty());
        let detail = hub.hub_detail(&found[0].repo).await.unwrap();
        assert!(!detail.files.is_empty());
    }
}
