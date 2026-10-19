//! Finds AI models that are already on this PC: Ollama, LM Studio, the app's own models
//! folder, the Hugging Face cache, Kokoro / Piper voices, Whisper, PaddleOCR and ComfyUI.
//! Everything here blocks and only reads the filesystem (plus one short localhost probe for
//! Ollama); callers run it through `mv_ui::bg::blocking`.
use std::collections::HashSet;
use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Language,
    Voice,
    Transcription,
    Ocr,
    ImageGen,
    Audio,
    Other,
}

impl Category {
    pub const ALL: [Category; 7] = [
        Category::Language,
        Category::Voice,
        Category::Transcription,
        Category::Ocr,
        Category::ImageGen,
        Category::Audio,
        Category::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Language => "Language models",
            Category::Voice => "Voice",
            Category::Transcription => "Transcription",
            Category::Ocr => "Text recognition",
            Category::ImageGen => "Image & video generation",
            Category::Audio => "Audio tools",
            Category::Other => "Other",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Category::Language => "Chat and vision models that run on this PC through Ollama, LM Studio or a GGUF file.",
            Category::Voice => "Text-to-speech voices used to narrate manga in Studio.",
            Category::Transcription => "Speech-to-text models such as Whisper.",
            Category::Ocr => "Models that read the text printed on manga pages.",
            Category::ImageGen => "Diffusion checkpoints and add-ons, for example from ComfyUI.",
            Category::Audio => "Audio processing such as separating voice from music.",
            Category::Other => "Models MangaVox found but can't place in a category.",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Category::Language => "icons/message-square.svg",
            Category::Voice => "icons/audio-lines.svg",
            Category::Transcription => "icons/captions.svg",
            Category::Ocr => "icons/scan-text.svg",
            Category::ImageGen => "icons/image.svg",
            Category::Audio => "icons/activity.svg",
            Category::Other => "icons/box.svg",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Ollama,
    LmStudio,
    MangaVox,
    HuggingFace,
    Kokoro,
    Piper,
    Whisper,
    PaddleOcr,
    ComfyUi,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Ollama => "Ollama",
            Source::LmStudio => "LM Studio",
            Source::MangaVox => "MangaVox",
            Source::HuggingFace => "Hugging Face cache",
            Source::Kokoro => "Kokoro",
            Source::Piper => "Piper",
            Source::Whisper => "Whisper",
            Source::PaddleOcr => "PaddleOCR",
            Source::ComfyUi => "ComfyUI",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocalModel {
    /// Stable id, e.g. `ollama:dolphin3:8b` or `lmstudio:qwen/qwen3.5-9b`.
    pub id: String,
    /// What the consuming engine calls it (Ollama tag, LM Studio model key, or a file path).
    pub key: String,
    pub name: String,
    pub family: String,
    pub category: Category,
    pub source: Source,
    pub size: u64,
    pub path: PathBuf,
    pub vision: bool,
    pub quant: Option<String>,
    pub thumbnail: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EngineId {
    Ollama,
    LmStudio,
    PythonTts,
    Ffmpeg,
    ComfyUi,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineState {
    Running,
    Stopped,
    Ready,
    /// Prerequisites are present but MangaVox has not set it up yet.
    NeedsSetup,
    NotInstalled,
}

#[derive(Clone, Debug)]
pub struct Engine {
    pub id: EngineId,
    pub name: &'static str,
    pub state: EngineState,
    pub detail: String,
    /// Executable or install folder, when known.
    pub path: Option<PathBuf>,
}

impl Engine {
    pub fn installed(&self) -> bool {
        self.state != EngineState::NotInstalled
    }

    /// "installed, stopped" style status text.
    pub fn status_text(&self) -> &'static str {
        match self.state {
            EngineState::Running => "running",
            EngineState::Stopped => "installed, stopped",
            EngineState::Ready => "installed",
            EngineState::NeedsSetup => "not set up yet",
            EngineState::NotInstalled => "not installed",
        }
    }
}

/// Where to look. `detect` fills it from the environment; tests point it at temp folders.
#[derive(Clone, Debug, Default)]
pub struct Roots {
    pub ollama: Vec<PathBuf>,
    pub lmstudio_models: Vec<PathBuf>,
    pub lmstudio_hub: Option<PathBuf>,
    pub app_models: PathBuf,
    pub hf_hub: Vec<PathBuf>,
    pub kokoro: Vec<PathBuf>,
    pub piper: Vec<PathBuf>,
    pub whisper: Vec<PathBuf>,
    pub paddle: Vec<PathBuf>,
    pub comfy: Vec<PathBuf>,
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from)
}

impl Roots {
    pub fn detect(app_models: PathBuf) -> Self {
        let home = home();
        let mangavox = mv_core::paths::root();

        let mut ollama = vec![];
        if let Some(p) = env_path("OLLAMA_MODELS") {
            ollama.push(p);
        }
        ollama.push(home.join(".ollama").join("models"));

        let lms_root = home.join(".lmstudio");
        let mut lmstudio_models = vec![];
        if let Some(p) = lmstudio_downloads_folder(&lms_root.join("settings.json")) {
            lmstudio_models.push(p);
        }
        lmstudio_models.push(lms_root.join("models"));
        lmstudio_models.push(home.join(".cache").join("lm-studio").join("models"));

        let mut hf_hub = vec![];
        if let Some(p) = env_path("HF_HUB_CACHE") {
            hf_hub.push(p);
        }
        if let Some(p) = env_path("HF_HOME") {
            hf_hub.push(p.join("hub"));
        }
        hf_hub.push(home.join(".cache").join("huggingface").join("hub"));

        let mut kokoro = vec![];
        if let Some(p) = env_path("MANGAVOX_KOKORO_DIR") {
            kokoro.push(p);
        }
        kokoro.push(mangavox.join("models").join("kokoro"));
        if app_models != mangavox.join("models") {
            kokoro.push(app_models.join("kokoro"));
        }

        let mut piper = vec![mangavox.join("models").join("piper")];
        if app_models != mangavox.join("models") {
            piper.push(app_models.join("piper"));
        }

        Roots {
            ollama: dedup(ollama),
            lmstudio_models: dedup(lmstudio_models),
            lmstudio_hub: Some(lms_root.join("hub").join("models")),
            app_models,
            hf_hub: dedup(hf_hub),
            kokoro: dedup(kokoro),
            piper: dedup(piper),
            whisper: vec![home.join(".cache").join("whisper")],
            paddle: vec![home.join(".paddleocr"), home.join(".paddlex").join("official_models")],
            comfy: comfy_model_roots(),
        }
    }
}

fn dedup(v: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    v.into_iter().filter(|p| seen.insert(p.to_string_lossy().to_lowercase())).collect()
}

/// `downloadsFolder` from LM Studio's settings.json (where it keeps models when moved).
fn lmstudio_downloads_folder(settings: &Path) -> Option<PathBuf> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(settings).ok()?).ok()?;
    v.get("downloadsFolder")?.as_str().filter(|s| !s.trim().is_empty()).map(PathBuf::from)
}

/// Model folders of ComfyUI installs: Comfy Desktop's shared folder, its installs, and
/// a portable `ComfyUI` next to the user's home.
fn comfy_model_roots() -> Vec<PathBuf> {
    let mut out = vec![];
    if let Some(appdata) = dirs::config_dir() {
        let desktop = appdata.join("Comfy Desktop");
        if let Ok(text) = std::fs::read_to_string(desktop.join("shared_model_paths.yaml")) {
            out.extend(yaml_base_paths(&text));
        }
        if let Ok(text) = std::fs::read_to_string(desktop.join("installations.json")) {
            if let Ok(Value::Array(list)) = serde_json::from_str::<Value>(&text) {
                for inst in list {
                    if let Some(p) = inst.get("installPath").and_then(Value::as_str) {
                        out.push(PathBuf::from(p).join("ComfyUI").join("models"));
                    }
                }
            }
        }
        // Older ComfyUI Desktop builds.
        if let Ok(text) = std::fs::read_to_string(appdata.join("ComfyUI").join("config.json")) {
            if let Some(p) = serde_json::from_str::<Value>(&text).ok().and_then(|v| v.get("basePath")?.as_str().map(PathBuf::from)) {
                out.push(p.join("models"));
            }
        }
    }
    let home = home();
    out.push(home.join("ComfyUI").join("models"));
    out.push(home.join("Documents").join("ComfyUI").join("models"));
    dedup(out.into_iter().filter(|p| p.is_dir()).collect())
}

/// `base_path: '...'` values from a ComfyUI extra-model-paths YAML.
pub fn yaml_base_paths(text: &str) -> Vec<PathBuf> {
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("base_path:"))
        .map(|v| v.trim().trim_matches(|c| c == '\'' || c == '"').to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .collect()
}

// ── Small filesystem helpers ────────────────────────────────────────────────

/// Total size of regular files under `path` (symlinks are not followed or counted).
pub fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else { return 0 };
    entries
        .flatten()
        .map(|e| match std::fs::symlink_metadata(e.path()) {
            Ok(md) if md.is_dir() => dir_size(&e.path()),
            Ok(md) if md.is_file() => md.len(),
            _ => 0,
        })
        .sum()
}

fn files_in(dir: &Path) -> Vec<(PathBuf, u64)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    rd.flatten()
        .filter_map(|e| {
            let md = e.metadata().ok()?;
            md.is_file().then(|| (e.path(), md.len()))
        })
        .collect()
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut v: Vec<PathBuf> = rd.flatten().filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false)).map(|e| e.path()).collect();
    v.sort();
    v
}

/// Files under `dir` up to `depth` levels deep.
fn walk(dir: &Path, depth: usize, out: &mut Vec<(PathBuf, u64)>) {
    out.extend(files_in(dir));
    if depth > 0 {
        for d in subdirs(dir) {
            walk(&d, depth - 1, out);
        }
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn ext_is(p: &Path, exts: &[&str]) -> bool {
    p.extension().map(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x))).unwrap_or(false)
}

/// "Qwen3.5-9B-GGUF" -> "Qwen3.5 9B".
pub fn pretty_repo(repo: &str) -> String {
    let mut s = repo.to_string();
    for suffix in ["-GGUF", "_GGUF", "-gguf", "-Instruct-GGUF"] {
        if let Some(stripped) = s.strip_suffix(suffix) {
            s = stripped.to_string();
        }
    }
    let s = s.replace(['-', '_'], " ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s,
    }
}

/// GGUF file stem with the trailing quant tag removed, lower-cased — the key LM Studio
/// gives models it did not download from its own catalog.
pub fn gguf_model_key(file: &str) -> String {
    let stem = file.strip_suffix(".gguf").or_else(|| file.strip_suffix(".GGUF")).unwrap_or(file);
    let quant = mv_ai::parse_quant(file).quant;
    let base = match quant {
        Some(q) => {
            // ASCII lowering keeps byte offsets valid for slicing `stem`.
            let lower = stem.to_ascii_lowercase();
            let ql = q.to_ascii_lowercase();
            match lower.rfind(&ql) {
                Some(i) if i > 0 => stem[..i].trim_end_matches(['.', '-', '_']).to_string(),
                _ => stem.to_string(),
            }
        }
        None => stem.to_string(),
    };
    base.to_lowercase()
}

const VISION_WORDS: &[&str] = &["llava", "minicpm-v", "-vl", "vl:", "vision", "moondream", "bakllava", "qwen2.5vl", "qwen3-vl", "gemma3", "gemma4", "pixtral", "mistral-small3"];

pub fn looks_vision(name: &str) -> bool {
    let l = name.to_lowercase();
    VISION_WORDS.iter().any(|w| l.contains(w))
}

// ── Ollama ──────────────────────────────────────────────────────────────────

/// Ollama tag from a manifest path relative to `manifests/`: `host/namespace/model/tag`.
pub fn ollama_name(rel: &[String]) -> Option<String> {
    let [host, ns, model, tag] = rel else { return None };
    Some(if host == "registry.ollama.ai" && ns == "library" {
        format!("{model}:{tag}")
    } else if host == "registry.ollama.ai" {
        format!("{ns}/{model}:{tag}")
    } else {
        format!("{host}/{ns}/{model}:{tag}")
    })
}

pub fn scan_ollama(root: &Path) -> Vec<LocalModel> {
    let manifests = root.join("manifests");
    let blobs = root.join("blobs");
    let mut out = vec![];
    for host in subdirs(&manifests) {
        for ns in subdirs(&host) {
            for model in subdirs(&ns) {
                for (tag_path, _) in files_in(&model) {
                    let rel: Vec<String> = [&host, &ns, &model, &tag_path].iter().map(|p| file_name(p)).collect();
                    let Some(name) = ollama_name(&rel) else { continue };
                    let Ok(text) = std::fs::read_to_string(&tag_path) else { continue };
                    let Ok(m) = serde_json::from_str::<Value>(&text) else { continue };
                    let layers = m.get("layers").and_then(Value::as_array).cloned().unwrap_or_default();
                    let size: u64 = layers.iter().filter_map(|l| l.get("size")?.as_u64()).sum();
                    let projector = layers.iter().any(|l| l.get("mediaType").and_then(Value::as_str).map(|t| t.ends_with("projector")).unwrap_or(false));
                    let config = m
                        .pointer("/config/digest")
                        .and_then(Value::as_str)
                        .and_then(|d| std::fs::read_to_string(blobs.join(d.replace(':', "-"))).ok())
                        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                        .unwrap_or(Value::Null);
                    let family = config.get("model_family").and_then(Value::as_str).unwrap_or("").to_string();
                    let clip = config.get("model_families").and_then(Value::as_array).map(|a| a.iter().any(|f| f.as_str() == Some("clip") || f.as_str() == Some("mllama"))).unwrap_or(false);
                    let params = config.get("model_type").and_then(Value::as_str).unwrap_or("");
                    let family_text = match (family.is_empty(), params.is_empty()) {
                        (false, false) => format!("{family} {params}"),
                        (false, true) => family.clone(),
                        _ => "Ollama model".into(),
                    };
                    let embed = name.contains("embed");
                    out.push(LocalModel {
                        id: format!("ollama:{name}"),
                        key: name.clone(),
                        name: name.clone(),
                        family: family_text,
                        category: if embed { Category::Other } else { Category::Language },
                        source: Source::Ollama,
                        size,
                        path: tag_path.clone(),
                        vision: projector || clip || looks_vision(&name),
                        quant: config.get("file_type").and_then(Value::as_str).map(str::to_string),
                        thumbnail: None,
                    });
                }
            }
        }
    }
    out
}

/// `GET /api/tags` on a local Ollama: `Some(names)` when it answers.
pub fn ollama_running_tags(url: &str) -> Option<Vec<(String, u64)>> {
    let rest = url.trim().strip_prefix("http://")?;
    let host = rest.split('/').next().filter(|h| !h.is_empty())?;
    let addr: SocketAddr = std::net::ToSocketAddrs::to_socket_addrs(&if host.contains(':') { host.to_string() } else { format!("{host}:80") }).ok()?.next()?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(400)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    write!(stream, "GET /api/tags HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n").ok()?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw).ok()?;
    let body = raw.split_once("\r\n\r\n")?.1;
    let v: Value = serde_json::from_str(body).ok()?;
    Some(
        v.get("models")?
            .as_array()?
            .iter()
            .filter_map(|m| Some((m.get("name")?.as_str()?.to_string(), m.get("size").and_then(Value::as_u64).unwrap_or(0))))
            .collect(),
    )
}

fn port_open(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok()
}

// ── LM Studio ───────────────────────────────────────────────────────────────

/// One catalog entry of LM Studio's hub (`hub/models/<owner>/<name>`).
struct HubEntry {
    key: String,
    repos: Vec<String>,
    vision: bool,
    family: Option<String>,
    thumbnail: Option<PathBuf>,
}

fn lmstudio_hub_entries(hub: &Path) -> Vec<HubEntry> {
    let mut out = vec![];
    for owner in subdirs(hub) {
        for model in subdirs(&owner) {
            let Some(m) = std::fs::read_to_string(model.join("manifest.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { continue };
            let key = format!("{}/{}", m.get("owner").and_then(Value::as_str).unwrap_or(&file_name(&owner)), m.get("name").and_then(Value::as_str).unwrap_or(&file_name(&model)));
            let mut repos = vec![];
            for dep in m.get("dependencies").and_then(Value::as_array).into_iter().flatten() {
                for src in dep.get("sources").and_then(Value::as_array).into_iter().flatten() {
                    if let (Some(u), Some(r)) = (src.get("user").and_then(Value::as_str), src.get("repo").and_then(Value::as_str)) {
                        repos.push(format!("{u}/{r}"));
                    }
                }
            }
            let yaml = std::fs::read_to_string(model.join("model.yaml")).unwrap_or_default();
            let thumb = model.join("thumbnail.png");
            out.push(HubEntry {
                key,
                repos,
                vision: yaml.lines().any(|l| l.trim() == "vision: true"),
                family: yaml_first_list_item(&yaml, "architectures:"),
                thumbnail: thumb.is_file().then_some(thumb),
            });
        }
    }
    out
}

/// First `- item` under a `key:` line of a simple YAML document.
fn yaml_first_list_item(yaml: &str, key: &str) -> Option<String> {
    let mut lines = yaml.lines().skip_while(|l| l.trim() != key);
    lines.next()?;
    lines.next()?.trim().strip_prefix("- ").map(|s| s.trim().to_string())
}

pub fn scan_lmstudio(models_root: &Path, hub: Option<&Path>) -> Vec<LocalModel> {
    let hub_entries = hub.map(lmstudio_hub_entries).unwrap_or_default();
    let mut out = vec![];
    for user in subdirs(models_root) {
        for repo_dir in subdirs(&user) {
            let repo = format!("{}/{}", file_name(&user), file_name(&repo_dir));
            let mut files = vec![];
            walk(&repo_dir, 2, &mut files);
            let main = files
                .iter()
                .filter(|(p, _)| ext_is(p, &["gguf"]) && !file_name(p).to_lowercase().contains("mmproj"))
                .max_by_key(|(_, s)| *s);
            let Some((main_path, _)) = main else { continue };
            let has_mmproj = files.iter().any(|(p, _)| file_name(p).to_lowercase().contains("mmproj"));
            let size: u64 = files.iter().map(|(_, s)| *s).sum();
            let main_name = file_name(main_path);
            let hub_hit = hub_entries.iter().find(|h| h.repos.iter().any(|r| r.eq_ignore_ascii_case(&repo)));
            let key = hub_hit.map(|h| h.key.clone()).unwrap_or_else(|| gguf_model_key(&main_name));
            let lower = repo.to_lowercase();
            out.push(LocalModel {
                id: format!("lmstudio:{key}"),
                key,
                name: pretty_repo(&file_name(&repo_dir)),
                family: hub_hit.and_then(|h| h.family.clone()).unwrap_or_else(|| file_name(&user)),
                category: if lower.contains("embed") { Category::Other } else { Category::Language },
                source: Source::LmStudio,
                size,
                path: repo_dir.clone(),
                vision: hub_hit.map(|h| h.vision).unwrap_or(false) || has_mmproj,
                quant: mv_ai::parse_quant(&main_name).quant,
                thumbnail: hub_hit.and_then(|h| h.thumbnail.clone()),
            });
        }
    }
    out
}

// ── MangaVox models folder ──────────────────────────────────────────────────

pub fn scan_app_gguf(dir: &Path) -> Vec<LocalModel> {
    let mut files = vec![];
    walk(dir, 3, &mut files);
    let entries: Vec<(String, u64)> = files
        .iter()
        .filter(|(p, _)| ext_is(p, &["gguf"]))
        .filter_map(|(p, s)| Some((p.strip_prefix(dir).ok()?.to_string_lossy().replace('\\', "/"), *s)))
        .collect();
    mv_ai::group_gguf_files(&entries)
        .into_iter()
        .map(|f| {
            let path = dir.join(&f.path);
            let parent = f.path.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
            let fname = file_name(&path);
            let repo = parent.replace("__", "/");
            LocalModel {
                id: format!("gguf:{}", f.path),
                key: path.to_string_lossy().into_owned(),
                name: pretty_repo(fname.trim_end_matches(".gguf")),
                family: if repo.is_empty() { "GGUF".into() } else { repo },
                category: Category::Language,
                source: Source::MangaVox,
                size: f.size,
                vision: looks_vision(&f.path),
                path,
                quant: f.quant.clone(),
                thumbnail: None,
            }
        })
        .collect()
}

// ── Hugging Face cache ──────────────────────────────────────────────────────

/// (category, family) for a Hugging Face repo id.
pub fn classify_hf(repo: &str) -> (Category, &'static str) {
    let l = repo.to_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| l.contains(x));
    if has(&["whisper"]) {
        (Category::Transcription, "Whisper")
    } else if has(&["kokoro"]) {
        (Category::Voice, "Kokoro")
    } else if has(&["piper"]) {
        (Category::Voice, "Piper")
    } else if has(&["xtts", "f5-tts", "f5tts", "parler", "bark", "-tts", "tts-", "styletts", "chatterbox"]) {
        (Category::Voice, "Text to speech")
    } else if has(&["manga-ocr", "manga_ocr"]) {
        (Category::Ocr, "manga-ocr")
    } else if has(&["paddleocr", "trocr", "-ocr", "ocr-", "got-ocr", "florence"]) {
        (Category::Ocr, "OCR")
    } else if has(&["demucs", "uvr", "mdx"]) {
        (Category::Audio, "Source separation")
    } else if has(&["ltx-video", "ltx-2", "stable-diffusion", "sdxl", "flux", "wan2", "wan-", "hunyuanvideo", "cogvideo", "animatediff", "kandinsky", "pixart", "sd-turbo", "sd3"]) {
        (Category::ImageGen, "Diffusion")
    } else if has(&["embed", "bge-", "e5-", "minilm", "sentence-transformers"]) {
        (Category::Other, "Embeddings")
    } else if has(&["gguf", "llama", "qwen", "mistral", "mixtral", "gemma", "phi-", "phi3", "phi4", "deepseek", "gpt-oss", "smollm", "granite", "glm", "falcon", "olmo", "yi-"]) {
        (Category::Language, "Transformers")
    } else {
        (Category::Other, "Hugging Face")
    }
}

pub fn scan_hf(hub: &Path) -> Vec<LocalModel> {
    let mut out = vec![];
    for dir in subdirs(hub) {
        let name = file_name(&dir);
        let Some(rest) = name.strip_prefix("models--") else { continue };
        let repo = rest.replacen("--", "/", 1);
        let blobs = dir.join("blobs");
        let size = if blobs.is_dir() { dir_size(&blobs) + dir_size(&dir.join("snapshots")) } else { dir_size(&dir) };
        // Partial downloads and metadata-only entries.
        if size < 1024 * 1024 {
            continue;
        }
        let (category, family) = classify_hf(&repo);
        let short = repo.rsplit('/').next().unwrap_or(&repo).to_string();
        out.push(LocalModel {
            id: format!("hf:{repo}"),
            key: repo.clone(),
            name: short,
            family: format!("{family} · {}", repo.split('/').next().unwrap_or("")),
            category,
            source: Source::HuggingFace,
            size,
            path: dir.clone(),
            vision: false,
            quant: None,
            thumbnail: None,
        });
    }
    out
}

// ── Voices, Whisper, OCR ────────────────────────────────────────────────────

pub const KOKORO_FILES: [&str; 2] = ["kokoro-v1.0.onnx", "voices-v1.0.bin"];

pub fn scan_kokoro(dir: &Path) -> Option<LocalModel> {
    let onnx = dir.join(KOKORO_FILES[0]);
    let voices = dir.join(KOKORO_FILES[1]);
    let onnx_len = std::fs::metadata(&onnx).ok()?.len();
    let voices_len = std::fs::metadata(&voices).ok()?.len();
    // Same check as kokoro_tts.py: a truncated download is not a model.
    if onnx_len <= 1_000_000 {
        return None;
    }
    Some(LocalModel {
        id: "kokoro:v1.0".into(),
        key: dir.to_string_lossy().into_owned(),
        name: "Kokoro v1.0 voices".into(),
        family: "Kokoro 82M".into(),
        category: Category::Voice,
        source: Source::Kokoro,
        size: onnx_len + voices_len,
        path: dir.to_path_buf(),
        vision: false,
        quant: None,
        thumbnail: None,
    })
}

/// Piper voices: `<name>.onnx` next to `<name>.onnx.json`.
pub fn scan_piper(dir: &Path) -> Vec<LocalModel> {
    let mut files = vec![];
    walk(dir, 3, &mut files);
    files
        .iter()
        .filter(|(p, _)| ext_is(p, &["onnx"]))
        .filter(|(p, _)| p.with_file_name(format!("{}.json", file_name(p))).is_file())
        .map(|(p, s)| {
            let stem = file_name(p).trim_end_matches(".onnx").to_string();
            LocalModel {
                id: format!("piper:{stem}"),
                key: stem.clone(),
                name: stem.clone(),
                family: "Piper voice".into(),
                category: Category::Voice,
                source: Source::Piper,
                size: *s,
                path: p.clone(),
                vision: false,
                quant: None,
                thumbnail: None,
            }
        })
        .collect()
}

pub fn scan_whisper(dir: &Path) -> Vec<LocalModel> {
    files_in(dir)
        .into_iter()
        .filter(|(p, _)| ext_is(p, &["pt", "bin"]))
        .map(|(p, s)| {
            let stem = file_name(&p).rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or_default();
            LocalModel {
                id: format!("whisper:{stem}"),
                key: p.to_string_lossy().into_owned(),
                name: format!("whisper-{stem}"),
                family: "OpenAI Whisper".into(),
                category: Category::Transcription,
                source: Source::Whisper,
                size: s,
                path: p,
                vision: false,
                quant: None,
                thumbnail: None,
            }
        })
        .collect()
}

pub fn scan_paddle(dir: &Path) -> Option<LocalModel> {
    let size = dir_size(dir);
    (size > 1024 * 1024).then(|| LocalModel {
        id: format!("paddle:{}", file_name(dir)),
        key: dir.to_string_lossy().into_owned(),
        name: "PaddleOCR models".into(),
        family: "PaddleOCR".into(),
        category: Category::Ocr,
        source: Source::PaddleOcr,
        size,
        path: dir.to_path_buf(),
        vision: false,
        quant: None,
        thumbnail: None,
    })
}

// ── ComfyUI ─────────────────────────────────────────────────────────────────

/// ComfyUI model sub-folders worth listing, with a readable label.
pub const COMFY_FOLDERS: &[(&str, &str)] = &[
    ("checkpoints", "Checkpoint"),
    ("diffusion_models", "Diffusion model"),
    ("unet", "Diffusion model"),
    ("loras", "LoRA"),
    ("vae", "VAE"),
    ("text_encoders", "Text encoder"),
    ("clip", "Text encoder"),
    ("clip_vision", "CLIP vision"),
    ("controlnet", "ControlNet"),
    ("upscale_models", "Upscaler"),
    ("latent_upscale_models", "Upscaler"),
    ("audio_encoders", "Audio encoder"),
];

/// True for the main models of a generation setup (not add-ons such as LoRAs or VAEs).
pub fn is_comfy_base(family: &str) -> bool {
    matches!(family, "Checkpoint" | "Diffusion model")
}

pub fn scan_comfy(root: &Path) -> Vec<LocalModel> {
    let mut out = vec![];
    for (folder, label) in COMFY_FOLDERS {
        let mut files = vec![];
        walk(&root.join(folder), 2, &mut files);
        for (p, s) in files {
            if s < 10 * 1024 * 1024 || !ext_is(&p, &["safetensors", "ckpt", "pt", "pth", "gguf", "bin", "sft"]) {
                continue;
            }
            let fname = file_name(&p);
            let stem = fname.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or(fname.clone());
            out.push(LocalModel {
                id: format!("comfy:{folder}/{fname}"),
                key: p.to_string_lossy().into_owned(),
                name: stem,
                family: label.to_string(),
                category: Category::ImageGen,
                source: Source::ComfyUi,
                size: s,
                path: p,
                vision: false,
                quant: None,
                thumbnail: None,
            });
        }
    }
    out
}

// ── Engines ─────────────────────────────────────────────────────────────────

/// Finds an executable on PATH (adds `.exe` on Windows).
pub fn which(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(&exe)).find(|p| p.is_file())
}

fn local_programs() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("Programs"))
}

pub fn ollama_binary() -> Option<PathBuf> {
    which("ollama").or_else(|| {
        let p = local_programs()?.join("Ollama").join("ollama.exe");
        p.is_file().then_some(p)
    })
}

pub fn lms_binary() -> Option<PathBuf> {
    which("lms").or_else(|| {
        let p = home().join(".lmstudio").join("bin").join(if cfg!(windows) { "lms.exe" } else { "lms" });
        p.is_file().then_some(p)
    })
}

pub fn lmstudio_app() -> Option<PathBuf> {
    let p = local_programs()?.join("LM Studio").join("LM Studio.exe");
    p.is_file().then_some(p)
}

pub fn comfy_app() -> Option<PathBuf> {
    let dir = local_programs()?.join("Comfy Desktop");
    let rd = std::fs::read_dir(&dir).ok()?;
    rd.flatten()
        .map(|e| e.path())
        .find(|p| ext_is(p, &["exe"]) && !file_name(p).to_lowercase().contains("uninstall"))
}

pub fn ffmpeg_binary() -> Option<PathBuf> {
    which("ffmpeg")
}

/// The Python environment Studio installs voices into (`kokoro_tts.py` VENV_DIR).
pub fn tts_venv() -> PathBuf {
    mv_core::paths::root().join("tts-venv")
}

pub struct EngineInputs {
    pub ollama_url: String,
    pub ollama_live: Option<Vec<(String, u64)>>,
    pub kokoro_ready: bool,
}

pub fn detect_engines(inp: &EngineInputs) -> Vec<Engine> {
    let ollama_bin = ollama_binary();
    let ollama = Engine {
        id: EngineId::Ollama,
        name: "Ollama",
        state: if inp.ollama_live.is_some() {
            EngineState::Running
        } else if ollama_bin.is_some() {
            EngineState::Stopped
        } else {
            EngineState::NotInstalled
        },
        detail: match (&inp.ollama_live, &ollama_bin) {
            (Some(tags), _) => format!("Serving {} model{} at {}", tags.len(), if tags.len() == 1 { "" } else { "s" }, inp.ollama_url),
            (None, Some(_)) => "Start it to use Ollama models for chat and for reading pages.".into(),
            (None, None) => "Runs local chat and vision models. Install it from ollama.com.".into(),
        },
        path: ollama_bin,
    };

    let lms = lms_binary();
    let lm_app = lmstudio_app();
    let lm_running = port_open(1234);
    let lmstudio = Engine {
        id: EngineId::LmStudio,
        name: "LM Studio",
        state: if lm_running {
            EngineState::Running
        } else if lms.is_some() || lm_app.is_some() {
            EngineState::Stopped
        } else {
            EngineState::NotInstalled
        },
        detail: if lm_running {
            "Local server is answering on port 1234.".into()
        } else if lms.is_some() || lm_app.is_some() {
            "Start the local server so the assistant can use LM Studio models.".into()
        } else {
            "Desktop app for running GGUF models. Get it from lmstudio.ai.".into()
        },
        path: lms.or(lm_app),
    };

    let venv = tts_venv();
    let venv_py = if cfg!(windows) { venv.join("Scripts").join("python.exe") } else { venv.join("bin").join("python") };
    let python = which("python").or_else(|| which("python3")).or_else(|| which("py"));
    let python_tts = Engine {
        id: EngineId::PythonTts,
        name: "Python voice engine",
        state: if venv_py.is_file() {
            EngineState::Ready
        } else if python.is_some() {
            EngineState::NeedsSetup
        } else {
            EngineState::NotInstalled
        },
        detail: match (venv_py.is_file(), python.is_some(), inp.kokoro_ready) {
            (true, _, true) => "Voice environment and Kokoro voices are ready.".into(),
            (true, _, false) => "Voice environment is set up; Kokoro voices are not downloaded yet.".into(),
            (false, true, _) => "Python is installed. Studio sets up the voice environment the first time you narrate.".into(),
            (false, false, _) => "Needs Python 3.10 or newer for narration and page reading.".into(),
        },
        path: if venv_py.is_file() { Some(venv) } else { python },
    };

    let ffmpeg_bin = ffmpeg_binary();
    let ffmpeg = Engine {
        id: EngineId::Ffmpeg,
        name: "FFmpeg",
        state: if ffmpeg_bin.is_some() { EngineState::Ready } else { EngineState::NotInstalled },
        detail: if ffmpeg_bin.is_some() {
            "Used to render narrated videos and convert audio.".into()
        } else {
            "Needed to render videos. Install with: winget install -e --id Gyan.FFmpeg".into()
        },
        path: ffmpeg_bin,
    };

    let comfy = comfy_app();
    let comfy_running = port_open(8188) || port_open(8000);
    let comfyui = Engine {
        id: EngineId::ComfyUi,
        name: "ComfyUI",
        state: if comfy.is_some() && comfy_running {
            EngineState::Running
        } else if comfy.is_some() {
            EngineState::Stopped
        } else {
            EngineState::NotInstalled
        },
        detail: if comfy.is_some() {
            "Image and video generation workflows. MangaVox lists its models here.".into()
        } else {
            "Optional: node-based image and video generation.".into()
        },
        path: comfy,
    };

    vec![ollama, lmstudio, python_tts, ffmpeg, comfyui]
}

// ── Whole inventory ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct System {
    pub gpu: Option<String>,
    pub vram_gb: f64,
    pub ram_gb: f64,
}

#[derive(Clone, Debug)]
pub struct Inventory {
    pub models: Vec<LocalModel>,
    pub engines: Vec<Engine>,
    pub models_dir: PathBuf,
    pub free_bytes: Option<u64>,
    pub ollama_url: String,
}

impl Inventory {
    pub fn engine(&self, id: EngineId) -> Option<&Engine> {
        self.engines.iter().find(|e| e.id == id)
    }

    pub fn model(&self, id: &str) -> Option<&LocalModel> {
        self.models.iter().find(|m| m.id == id)
    }

    pub fn count(&self, c: Category) -> usize {
        self.models.iter().filter(|m| m.category == c).count()
    }

    pub fn kokoro_ready(&self) -> bool {
        self.models.iter().any(|m| m.source == Source::Kokoro)
    }

    /// Piper voice file stems that are present (e.g. `en_US-amy-medium`).
    pub fn piper_stems(&self) -> HashSet<String> {
        self.models.iter().filter(|m| m.source == Source::Piper).map(|m| m.key.clone()).collect()
    }
}

/// Every model found under `roots`, de-duplicated by path, largest categories first.
pub fn scan_models(roots: &Roots, ollama_live: Option<&[(String, u64)]>) -> Vec<LocalModel> {
    let mut all = vec![];
    for r in &roots.ollama {
        all.extend(scan_ollama(r));
    }
    // A running Ollama with a custom models folder: list what it serves.
    if let Some(live) = ollama_live {
        for (name, size) in live {
            if !all.iter().any(|m: &LocalModel| m.source == Source::Ollama && m.key == *name) {
                all.push(LocalModel {
                    id: format!("ollama:{name}"),
                    key: name.clone(),
                    name: name.clone(),
                    family: "Ollama model".into(),
                    category: Category::Language,
                    source: Source::Ollama,
                    size: *size,
                    path: PathBuf::new(),
                    vision: looks_vision(name),
                    quant: None,
                    thumbnail: None,
                });
            }
        }
    }
    let hub = roots.lmstudio_hub.as_deref();
    for r in &roots.lmstudio_models {
        all.extend(scan_lmstudio(r, hub));
    }
    all.extend(scan_app_gguf(&roots.app_models));
    for r in &roots.hf_hub {
        all.extend(scan_hf(r));
    }
    for r in &roots.kokoro {
        all.extend(scan_kokoro(r));
    }
    for r in &roots.piper {
        all.extend(scan_piper(r));
    }
    for r in &roots.whisper {
        all.extend(scan_whisper(r));
    }
    for r in &roots.paddle {
        all.extend(scan_paddle(r));
    }
    for r in &roots.comfy {
        all.extend(scan_comfy(r));
    }
    let mut seen = HashSet::new();
    all.retain(|m| seen.insert(m.id.clone()) && (m.path.as_os_str().is_empty() || seen.insert(m.path.to_string_lossy().to_lowercase())));
    all.sort_by(|a, b| a.category.cmp(&b.category).then(b.size.cmp(&a.size)));
    all
}

/// Free space on the drive holding `path`.
pub fn free_space(path: &Path) -> Option<u64> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let probe = path.ancestors().find(|p| p.exists()).unwrap_or(path);
    let probe = probe.canonicalize().unwrap_or_else(|_| probe.to_path_buf());
    let probe_s = probe.to_string_lossy().trim_start_matches(r"\\?\").to_lowercase();
    disks
        .list()
        .iter()
        .filter(|d| probe_s.starts_with(&d.mount_point().to_string_lossy().to_lowercase()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}

fn configured_ollama_url() -> String {
    let cfg = mv_core::config::Config::open_default();
    cfg.get("ollama")
        .and_then(|o| o.get("url").and_then(Value::as_str).map(|s| s.trim().trim_end_matches('/').to_string()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:11434".into())
}

/// Full scan of this PC. Takes a second or two on a cold disk cache.
pub fn scan(app_models: PathBuf) -> Inventory {
    let ollama_url = configured_ollama_url();
    let live = ollama_running_tags(&ollama_url);
    let roots = Roots::detect(app_models.clone());
    let models = scan_models(&roots, live.as_deref());
    let kokoro_ready = models.iter().any(|m| m.source == Source::Kokoro);
    let engines = detect_engines(&EngineInputs { ollama_url: ollama_url.clone(), ollama_live: live, kokoro_ready });
    Inventory { free_bytes: free_space(&app_models), models, engines, models_dir: app_models, ollama_url }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    #[test]
    fn ollama_manifests_become_tags_with_family_and_size() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let manifest = r#"{"config":{"digest":"sha256:abc","size":10},"layers":[{"mediaType":"application/vnd.ollama.image.model","size":4000},{"mediaType":"application/vnd.ollama.image.projector","size":600}]}"#;
        write(&root.join("manifests/registry.ollama.ai/library/llava/7b"), manifest.as_bytes());
        write(&root.join("manifests/registry.ollama.ai/someone/tiny/latest"), br#"{"layers":[{"size":5}]}"#);
        write(&root.join("blobs/sha256-abc"), br#"{"model_family":"llama","model_type":"7B","file_type":"Q4_0"}"#);
        let mut list = scan_ollama(root);
        list.sort_by(|a, b| a.key.cmp(&b.key));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].key, "llava:7b");
        assert_eq!(list[0].size, 4600);
        assert!(list[0].vision);
        assert_eq!(list[0].family, "llama 7B");
        assert_eq!(list[0].quant.as_deref(), Some("Q4_0"));
        assert_eq!(list[1].key, "someone/tiny:latest");
        assert!(!list[1].vision);
        assert_eq!(ollama_name(&["ghcr.io".into(), "a".into(), "b".into(), "c".into()]).unwrap(), "ghcr.io/a/b:c");
    }

    #[test]
    fn lmstudio_uses_hub_keys_and_file_keys() {
        let dir = tempfile::tempdir().unwrap();
        let models = dir.path().join("models");
        let hub = dir.path().join("hub");
        write(&models.join("lmstudio-community/Qwen3.5-9B-GGUF/Qwen3.5-9B-Q4_K_M.gguf"), &[0; 64]);
        write(&models.join("lmstudio-community/Qwen3.5-9B-GGUF/mmproj-Qwen3.5-9B-BF16.gguf"), &[0; 8]);
        write(&models.join("TheBloke/MythoMax-L2-13B-GGUF/mythomax-l2-13b.Q4_K_S.gguf"), &[0; 32]);
        write(&models.join("someone/not-a-model/readme.md"), b"x");
        write(
            &hub.join("qwen/qwen3.5-9b/manifest.json"),
            br#"{"owner":"qwen","name":"qwen3.5-9b","dependencies":[{"sources":[{"type":"huggingface","user":"lmstudio-community","repo":"Qwen3.5-9B-GGUF"}]}]}"#,
        );
        write(&hub.join("qwen/qwen3.5-9b/model.yaml"), b"metadataOverrides:\n  architectures:\n    - qwen35\n  vision: true\n");
        let mut list = scan_lmstudio(&models, Some(&hub));
        list.sort_by(|a, b| a.key.cmp(&b.key));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].key, "mythomax-l2-13b");
        assert_eq!(list[0].name, "MythoMax L2 13B");
        assert_eq!(list[1].key, "qwen/qwen3.5-9b");
        assert_eq!(list[1].family, "qwen35");
        assert_eq!(list[1].size, 72);
        assert!(list[1].vision);
        assert_eq!(list[1].quant.as_deref(), Some("Q4_K_M"));
    }

    #[test]
    fn model_keys_and_names() {
        assert_eq!(gguf_model_key("Qwythos-9B-Claude-Mythos-5-1M-Q4_K_M.gguf"), "qwythos-9b-claude-mythos-5-1m");
        assert_eq!(gguf_model_key("mythomax-l2-kimiko-v2-13b.Q4_K_S.gguf"), "mythomax-l2-kimiko-v2-13b");
        assert_eq!(pretty_repo("Qwen3.5-9B-GGUF"), "Qwen3.5 9B");
        assert_eq!(classify_hf("Systran/faster-whisper-medium.en").0, Category::Transcription);
        assert_eq!(classify_hf("adefossez/HTDemucs").0, Category::Audio);
        assert_eq!(classify_hf("Lightricks/LTX-Video").0, Category::ImageGen);
        assert_eq!(classify_hf("kha-white/manga-ocr-base").0, Category::Ocr);
        assert_eq!(classify_hf("hexgrad/Kokoro-82M").0, Category::Voice);
        assert_eq!(classify_hf("Qwen/Qwen2.5-7B-Instruct").0, Category::Language);
        assert_eq!(yaml_base_paths("x:\n  base_path: 'C:\\m'\n  is_default: true\n"), vec![PathBuf::from("C:\\m")]);
    }

    #[test]
    fn hf_voices_whisper_comfy_and_app_models() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let mb = vec![0u8; 1024 * 1024 + 1];
        write(&d.join("hf/models--Systran--faster-whisper-small.en/blobs/aaa"), &mb);
        write(&d.join("hf/models--Empty--repo/refs/main"), b"x");
        let hf = scan_hf(&d.join("hf"));
        assert_eq!(hf.len(), 1);
        assert_eq!(hf[0].name, "faster-whisper-small.en");
        assert_eq!(hf[0].category, Category::Transcription);

        write(&d.join("kokoro/kokoro-v1.0.onnx"), &mb);
        write(&d.join("kokoro/voices-v1.0.bin"), b"v");
        assert!(scan_kokoro(&d.join("kokoro")).is_some());
        write(&d.join("kokoro2/kokoro-v1.0.onnx"), b"tiny");
        write(&d.join("kokoro2/voices-v1.0.bin"), b"v");
        assert!(scan_kokoro(&d.join("kokoro2")).is_none());

        write(&d.join("piper/en_US/en_US-amy-medium.onnx"), b"o");
        write(&d.join("piper/en_US/en_US-amy-medium.onnx.json"), b"{}");
        write(&d.join("piper/en_US/orphan.onnx"), b"o");
        let piper = scan_piper(&d.join("piper"));
        assert_eq!(piper.len(), 1);
        assert_eq!(piper[0].key, "en_US-amy-medium");

        let big = vec![0u8; 10 * 1024 * 1024 + 1];
        write(&d.join("comfy/checkpoints/sdxl.safetensors"), &big);
        write(&d.join("comfy/loras/small.safetensors"), b"s");
        let comfy = scan_comfy(&d.join("comfy"));
        assert_eq!(comfy.len(), 1);
        assert_eq!(comfy[0].family, "Checkpoint");

        write(&d.join("app/acme__tiny/tiny-Q4_K_M.gguf"), &[0; 16]);
        let app = scan_app_gguf(&d.join("app"));
        assert_eq!(app.len(), 1);
        assert_eq!(app[0].family, "acme/tiny");
        assert_eq!(app[0].quant.as_deref(), Some("Q4_K_M"));

        let roots = Roots { hf_hub: vec![d.join("hf")], kokoro: vec![d.join("kokoro")], piper: vec![d.join("piper")], comfy: vec![d.join("comfy")], app_models: d.join("app"), ..Default::default() };
        let all = scan_models(&roots, Some(&[("extra:1b".to_string(), 7)]));
        assert_eq!(all.len(), 6);
        assert_eq!(all[0].category, Category::Language);
        assert!(all.iter().any(|m| m.key == "extra:1b"));
    }

    #[test]
    fn engines_report_a_state_for_each_tool() {
        let e = detect_engines(&EngineInputs { ollama_url: "http://127.0.0.1:1".into(), ollama_live: None, kokoro_ready: false });
        assert_eq!(e.len(), 5);
        assert!(e.iter().all(|x| !x.detail.is_empty()));
    }

    /// `cargo test -p ui-settings -- --ignored --nocapture print_this_pc` lists what this PC has.
    #[test]
    #[ignore]
    fn print_this_pc() {
        let inv = scan(mv_core::paths::root().join("models"));
        for m in &inv.models {
            println!("{:<26} {:<16} {:>9} {:<44} {}", m.category.label(), m.source.label(), crate::ui::format_bytes(m.size), m.name, m.key);
        }
        for e in &inv.engines {
            println!("engine {:<20} {:?} {:?}", e.name, e.state, e.path);
        }
        println!("free: {:?}", inv.free_bytes.map(crate::ui::format_bytes));
    }
}
