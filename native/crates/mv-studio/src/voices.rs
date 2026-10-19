//! Voice packs: the model files the local TTS engines need, what each pack gives you
//! (voices, quality, speed, size), whether it is on disk, a resumable downloader with
//! progress, and the narration voice defaults (app-wide and per chapter).
//!
//! Files land where `kokoro_tts.py` looks for them (`~/.mangavox/models/kokoro`,
//! `.../piper/en_US`, `.../piper/hi_IN`). Downloads write `<file>.part` and resume from it
//! with an HTTP Range request, so an interrupted download continues where it stopped.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use tokio::io::AsyncWriteExt;

use crate::bridge::CancelToken;
use crate::error::StudioError;
use crate::tts;

/// One model file of a pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackFile {
    /// Folder under `~/.mangavox/models`.
    pub dir: &'static str,
    pub name: &'static str,
    pub url: &'static str,
    /// Expected bytes (0 for small config files).
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoicePack {
    pub id: &'static str,
    pub name: &'static str,
    pub engine: &'static str,
    pub summary: &'static str,
    pub languages: &'static str,
    /// 1–5.
    pub quality: u8,
    /// 1–5, how fast it narrates on a CPU.
    pub speed: u8,
    pub recommended: bool,
    pub files: &'static [PackFile],
    /// Catalog voice ids this pack provides. Empty means every Kokoro voice.
    pub voices: &'static [&'static str],
}

impl VoicePack {
    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// Catalog voices this pack makes available.
    pub fn voice_ids(&self) -> Vec<&'static str> {
        if self.voices.is_empty() {
            tts::voice_catalog().iter().filter(|v| tts::engine_hint(v.id) == "kokoro").map(|v| v.id).collect()
        } else {
            self.voices.to_vec()
        }
    }
}

const KOKORO_VOICES: PackFile = PackFile { dir: "kokoro", name: "voices-v1.0.bin", url: concat!("https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/", "voices-v1.0.bin"), size: 28_214_398 };

macro_rules! piper {
    ($id:literal, $name:literal, $summary:literal, $lang:literal, $dir:literal, $file:literal, $base:literal, $size:literal, $voice:literal) => {
        VoicePack {
            id: $id,
            name: $name,
            engine: "Piper",
            summary: $summary,
            languages: $lang,
            quality: 2,
            speed: 5,
            recommended: false,
            files: &[
                PackFile { dir: $dir, name: concat!($file, ".onnx"), url: concat!($base, $file, ".onnx"), size: $size },
                PackFile { dir: $dir, name: concat!($file, ".onnx.json"), url: concat!($base, $file, ".onnx.json"), size: 0 },
            ],
            voices: &[$voice],
        }
    };
}

/// Every pack, best first.
pub const PACKS: &[VoicePack] = &[
    VoicePack {
        id: "kokoro",
        name: "Kokoro v1.0",
        engine: "Kokoro",
        summary: "The most natural voices that still run fast on a normal CPU. 14 English (US and UK) and 4 Hindi narrators.",
        languages: "English, Hindi",
        quality: 4,
        speed: 4,
        recommended: true,
        files: &[PackFile { dir: "kokoro", name: "kokoro-v1.0.onnx", url: concat!("https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/", "kokoro-v1.0.onnx"), size: 325_532_387 }, KOKORO_VOICES],
        voices: &[],
    },
    VoicePack {
        id: "kokoro-lite",
        name: "Kokoro v1.0 Lite",
        engine: "Kokoro",
        summary: "The same Kokoro voices, compressed to a third of the size. Faster on older PCs, slightly less crisp. Used only when the full pack is not installed.",
        languages: "English, Hindi",
        quality: 3,
        speed: 5,
        recommended: false,
        files: &[PackFile { dir: "kokoro", name: "kokoro-v1.0.int8.onnx", url: concat!("https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/", "kokoro-v1.0.int8.onnx"), size: 92_361_271 }, KOKORO_VOICES],
        voices: &[],
    },
    piper!("piper-en-amy", "Piper · Amy (US)", "Clear US female. Very fast and light, more robotic than Kokoro.", "English", "piper/en_US", "en_US-amy-medium", "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US/amy/medium/", 63_201_294, "en_piper_f"),
    piper!("piper-en-lessac", "Piper · Lessac (US)", "Steady US male. Very fast and light.", "English", "piper/en_US", "en_US-lessac-medium", "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US/lessac/medium/", 63_201_294, "en_piper_m"),
    piper!("piper-en-kristin", "Piper · Kristin (US)", "Smooth US female. Very fast and light.", "English", "piper/en_US", "en_US-kristin-medium", "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/kristin/medium/", 63_531_379, "en_piper_f2"),
    piper!("piper-en-ryan", "Piper · Ryan (US)", "Natural US male. Very fast and light.", "English", "piper/en_US", "en_US-ryan-medium", "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/ryan/medium/", 63_201_294, "en_piper_m2"),
    piper!("piper-hi-priyamvada", "Piper · Priyamvada (Hindi)", "Clear Hindi female. Very fast and light.", "Hindi", "piper/hi_IN", "hi_IN-priyamvada-medium", "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/hi/hi_IN/priyamvada/medium/", 63_516_050, "hi_piper_f"),
    piper!("piper-hi-pratham", "Piper · Pratham (Hindi)", "Steady Hindi male. Very fast and light.", "Hindi", "piper/hi_IN", "hi_IN-pratham-medium", "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/hi/hi_IN/pratham/medium/", 63_516_050, "hi_piper_m"),
    piper!("piper-hi-rohan", "Piper · Rohan (Hindi)", "Young Hindi male. Very fast and light.", "Hindi", "piper/hi_IN", "hi_IN-rohan-medium", "https://huggingface.co/rhasspy/piper-voices/resolve/main/hi/hi_IN/rohan/medium/", 62_950_044, "hi_piper_rohan"),
];


pub fn find_pack(id: &str) -> Option<&'static VoicePack> {
    PACKS.iter().find(|p| p.id == id)
}

/// `~/.mangavox/models`.
pub fn models_root() -> PathBuf {
    crate::python::mangavox_home().join("models")
}

/// Where a pack file lives. Kokoro honours `MANGAVOX_KOKORO_DIR` like `kokoro_tts.py`.
pub fn file_path(root: &Path, f: &PackFile) -> PathBuf {
    if f.dir == "kokoro" {
        if let Some(dir) = std::env::var_os("MANGAVOX_KOKORO_DIR") {
            return PathBuf::from(dir).join(f.name);
        }
    }
    root.join(f.dir).join(f.name)
}

fn part_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

/// A finished file is only ever created by renaming a complete `.part`, so existing is enough.
fn file_done(root: &Path, f: &PackFile) -> bool {
    std::fs::metadata(file_path(root, f)).map(|m| m.len() > 0).unwrap_or(false)
}

pub fn installed_in(root: &Path, pack: &VoicePack) -> bool {
    pack.files.iter().all(|f| file_done(root, f))
}

pub fn installed(pack: &VoicePack) -> bool {
    installed_in(&models_root(), pack)
}

/// Bytes already on disk for a pack that is not complete (finished files plus `.part`s).
pub fn partial_bytes_in(root: &Path, pack: &VoicePack) -> u64 {
    pack.files
        .iter()
        .map(|f| {
            let p = file_path(root, f);
            std::fs::metadata(&p).or_else(|_| std::fs::metadata(part_path(&p))).map(|m| m.len()).unwrap_or(0)
        })
        .sum()
}

/// Deletes a pack's files, keeping any file another installed pack still uses.
pub fn remove_in(root: &Path, pack: &VoicePack) -> Result<(), StudioError> {
    for f in pack.files {
        let shared = PACKS.iter().any(|o| o.id != pack.id && o.files.contains(f) && installed_in(root, o));
        let p = file_path(root, f);
        let _ = std::fs::remove_file(part_path(&p));
        if !shared && p.exists() {
            std::fs::remove_file(&p)?;
        }
    }
    Ok(())
}

pub fn remove(pack: &VoicePack) -> Result<(), StudioError> {
    remove_in(&models_root(), pack)
}

/// Download progress for one pack.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
    pub bytes_per_sec: f64,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.done as f64 / self.total as f64).min(1.0) as f32
        }
    }
}

const ATTEMPTS: u32 = 5;

/// Downloads every missing file of `pack` into `root`, resuming `.part` files. Retries
/// network failures with backoff; `on_progress` is called a few times per second.
pub async fn download_in(root: &Path, pack: &VoicePack, cancel: &CancelToken, mut on_progress: impl FnMut(Progress)) -> Result<(), StudioError> {
    let client = reqwest::Client::builder()
        .user_agent("MangaVox/1.0")
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| StudioError::Download(format!("could not start the download: {e}")))?;
    let mut total: u64 = pack.size();
    let mut finished: u64 = 0;
    let started = Instant::now();
    let mut fresh: u64 = 0;
    for f in pack.files {
        let dest = file_path(root, f);
        if file_done(root, f) {
            finished += std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(f.size);
            continue;
        }
        if let Some(dir) = dest.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        let part = part_path(&dest);
        let mut last_err = String::new();
        let mut complete = false;
        let mut counted = f.size;
        for attempt in 0..ATTEMPTS {
            if cancel.is_cancelled() {
                return Err(StudioError::Cancelled);
            }
            if attempt > 0 {
                let wait = Duration::from_secs(1 << attempt.min(4));
                tokio::select! {
                    _ = cancel.cancelled() => return Err(StudioError::Cancelled),
                    _ = tokio::time::sleep(wait) => {}
                }
            }
            let have = tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);
            let mut req = client.get(f.url);
            if have > 0 {
                req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
            }
            let resp = tokio::select! {
                _ = cancel.cancelled() => return Err(StudioError::Cancelled),
                r = req.send() => r,
            };
            let mut resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    last_err = network_error(&e);
                    continue;
                }
            };
            let status = resp.status();
            let resumed = status == reqwest::StatusCode::PARTIAL_CONTENT;
            if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
                // The .part is already whole (or the server changed the file); start over.
                let _ = tokio::fs::remove_file(&part).await;
                last_err = "the server rejected the resume request".into();
                continue;
            }
            if !status.is_success() {
                last_err = format!("the server answered HTTP {}", status.as_u16());
                if status.is_client_error() && status != reqwest::StatusCode::TOO_MANY_REQUESTS && status != reqwest::StatusCode::REQUEST_TIMEOUT {
                    break;
                }
                continue;
            }
            let offset = if resumed { have } else { 0 };
            let file_total = resp.content_length().map(|n| n + offset);
            if let Some(t) = file_total {
                // Swap the catalog size for the real one.
                total = total - counted.min(total) + t;
                counted = t;
            }
            let mut out = if resumed {
                tokio::fs::OpenOptions::new().append(true).open(&part).await?
            } else {
                tokio::fs::File::create(&part).await?
            };
            let mut written = offset;
            let mut last_tick = Instant::now();
            let mut broke = false;
            loop {
                let chunk = tokio::select! {
                    _ = cancel.cancelled() => {
                        let _ = out.flush().await;
                        return Err(StudioError::Cancelled);
                    }
                    c = resp.chunk() => c,
                };
                match chunk {
                    Ok(Some(bytes)) => {
                        out.write_all(&bytes).await.map_err(|e| StudioError::Download(format!("could not write {}: {e}", f.name)))?;
                        written += bytes.len() as u64;
                        fresh += bytes.len() as u64;
                        if last_tick.elapsed() >= Duration::from_millis(200) {
                            last_tick = Instant::now();
                            let secs = started.elapsed().as_secs_f64().max(0.001);
                            on_progress(Progress { done: finished + written, total, bytes_per_sec: fresh as f64 / secs });
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        last_err = network_error(&e);
                        broke = true;
                        break;
                    }
                }
            }
            out.flush().await?;
            drop(out);
            if broke {
                continue;
            }
            if file_total.is_some_and(|t| t != written) {
                last_err = format!("{} ended early ({written} bytes)", f.name);
                continue;
            }
            tokio::fs::rename(&part, &dest).await?;
            finished += written;
            complete = true;
            break;
        }
        if !complete {
            return Err(StudioError::Download(format!("{} could not be downloaded: {last_err}. Try again to resume.", f.name)));
        }
    }
    let secs = started.elapsed().as_secs_f64().max(0.001);
    on_progress(Progress { done: finished, total: total.max(finished), bytes_per_sec: fresh as f64 / secs });
    Ok(())
}

fn network_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "the connection timed out".into()
    } else if e.is_connect() {
        "could not reach the server (check your internet connection)".into()
    } else {
        e.to_string()
    }
}

// ---------------------------------------------------------------- download registry

/// State of a pack download, shared by every view.
#[derive(Debug, Clone, PartialEq)]
pub enum DownloadState {
    Running(Progress),
    Failed(String),
}

static DOWNLOADS: Mutex<Option<HashMap<&'static str, (DownloadState, CancelToken)>>> = Mutex::new(None);

fn with_downloads<T>(f: impl FnOnce(&mut HashMap<&'static str, (DownloadState, CancelToken)>) -> T) -> T {
    let mut g = DOWNLOADS.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

pub fn download_state(id: &str) -> Option<DownloadState> {
    with_downloads(|m| m.get(id).map(|(s, _)| s.clone()))
}

pub fn any_running() -> bool {
    with_downloads(|m| m.values().any(|(s, _)| matches!(s, DownloadState::Running(_))))
}

pub fn cancel_download(id: &str) {
    with_downloads(|m| {
        if let Some((_, c)) = m.remove(id) {
            c.cancel();
        }
    });
}

pub fn clear_error(id: &str) {
    with_downloads(|m| {
        if matches!(m.get(id), Some((DownloadState::Failed(_), _))) {
            m.remove(id);
        }
    });
}

/// Downloads a pack, publishing progress for [`download_state`]. Returns at once when
/// the pack is already downloading. Run it on the tokio runtime.
pub async fn run_download(id: &'static str) -> Result<(), StudioError> {
    let pack = find_pack(id).ok_or_else(|| StudioError::Download(format!("unknown voice pack {id}")))?;
    let cancel = CancelToken::new();
    let fresh = with_downloads(|m| {
        if matches!(m.get(id), Some((DownloadState::Running(_), _))) {
            return false;
        }
        let start = Progress { done: partial_bytes_in(&models_root(), pack), total: pack.size(), bytes_per_sec: 0.0 };
        m.insert(id, (DownloadState::Running(start), cancel.clone()));
        true
    });
    if !fresh {
        return Ok(());
    }
    let res = download_in(&models_root(), pack, &cancel, |p| {
        with_downloads(|m| {
            if let Some((s, _)) = m.get_mut(id) {
                *s = DownloadState::Running(p);
            }
        })
    })
    .await;
    with_downloads(|m| match &res {
        Ok(()) | Err(StudioError::Cancelled) => {
            m.remove(id);
        }
        Err(e) => {
            m.insert(id, (DownloadState::Failed(e.to_string()), CancelToken::new()));
        }
    });
    res
}

// ---------------------------------------------------------------- default voice

/// Fallback when nothing valid is saved.
pub const FALLBACK_VOICE: &str = "af_heart";

fn studio_dir() -> PathBuf {
    crate::python::mangavox_home().join("studio")
}

fn read_json(path: &Path) -> Map<String, Value> {
    match std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

fn write_json(path: &Path, m: Map<String, Value>) -> Result<(), StudioError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&Value::Object(m))?)?;
    Ok(())
}

fn valid(id: &str) -> bool {
    tts::find_voice(id).is_some()
}

/// The narration voice new chapters use: Studio's `prefs.json` `voice`, then
/// `config.json` `pipeline.tts_voice`, then [`FALLBACK_VOICE`].
pub fn default_voice() -> String {
    default_voice_in(&studio_dir().join("prefs.json"), &crate::python::config_path())
}

fn default_voice_in(prefs: &Path, config: &Path) -> String {
    let from_prefs = read_json(prefs).get("voice").and_then(Value::as_str).map(str::to_string);
    let from_config = read_json(config).get("pipeline").and_then(|p| p.get("tts_voice")).and_then(Value::as_str).map(str::to_string);
    from_prefs.into_iter().chain(from_config).find(|v| valid(v)).unwrap_or_else(|| FALLBACK_VOICE.into())
}

/// Saves the default voice in both places it is read from (other keys are kept).
pub fn set_default_voice(id: &str) -> Result<(), StudioError> {
    set_default_voice_in(&studio_dir().join("prefs.json"), &crate::python::config_path(), id)
}

fn set_default_voice_in(prefs: &Path, config: &Path, id: &str) -> Result<(), StudioError> {
    if !valid(id) {
        return Err(StudioError::Preview(format!("unknown voice {id}")));
    }
    let mut p = read_json(prefs);
    p.insert("voice".into(), json!(id));
    write_json(prefs, p)?;
    let mut c = read_json(config);
    let pipeline = c.entry("pipeline").or_insert_with(|| json!({}));
    if !pipeline.is_object() {
        *pipeline = json!({});
    }
    if let Some(o) = pipeline.as_object_mut() {
        o.insert("tts_voice".into(), json!(id));
    }
    write_json(config, c)
}

/// Voices picked for single chapters (`studio/chapter-voices.json`, chapter id -> voice).
fn chapter_file() -> PathBuf {
    studio_dir().join("chapter-voices.json")
}

/// The chapter map, read once and kept in memory (Studio asks for it on every repaint).
static CHAPTER_VOICES: Mutex<Option<Map<String, Value>>> = Mutex::new(None);

fn with_chapter_voices<T>(f: impl FnOnce(&mut Map<String, Value>) -> T) -> T {
    let mut g = CHAPTER_VOICES.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(|| read_json(&chapter_file())))
}

/// The voice picked for this chapter, if it has its own.
pub fn chapter_voice(chapter_id: &str) -> Option<String> {
    with_chapter_voices(|m| m.get(chapter_id).and_then(Value::as_str).filter(|v| valid(v)).map(str::to_string))
}

/// Sets (or with `None` clears) a chapter's own voice.
pub fn set_chapter_voice(chapter_id: &str, voice: Option<&str>) -> Result<(), StudioError> {
    with_chapter_voices(|m| {
        let changed = match voice {
            Some(v) => m.insert(chapter_id.to_string(), json!(v)).and_then(|old| old.as_str().map(|o| o != v)).unwrap_or(true),
            None => m.remove(chapter_id).is_some(),
        };
        if changed {
            write_json(&chapter_file(), m.clone())?;
        }
        Ok(())
    })
}

/// The voice a chapter is narrated with: its own pick, else the default.
pub fn voice_for_chapter(chapter_id: &str) -> String {
    chapter_voice(chapter_id).unwrap_or_else(default_voice)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_consistent() {
        let mut ids: Vec<_> = PACKS.iter().map(|p| p.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), PACKS.len());
        for p in PACKS {
            assert!((1..=5).contains(&p.quality) && (1..=5).contains(&p.speed), "{}", p.id);
            for v in p.voice_ids() {
                assert!(tts::find_voice(v).is_some(), "{} lists unknown voice {v}", p.id);
            }
        }
        assert_eq!(find_pack("kokoro").unwrap().voice_ids().len(), 18);
        assert_eq!(find_pack("piper-en-amy").unwrap().voice_ids(), vec!["en_piper_f"]);
        // Every Piper voice in the catalog has a pack.
        for v in tts::voice_catalog().iter().filter(|v| tts::engine_hint(v.id) == "piper") {
            assert!(PACKS.iter().any(|p| p.voices.contains(&v.id)), "{}", v.id);
        }
    }

    #[test]
    fn remove_keeps_files_shared_with_an_installed_pack() {
        let dir = tempdir();
        let root = dir.as_path();
        let full = find_pack("kokoro").unwrap();
        let lite = find_pack("kokoro-lite").unwrap();
        for f in full.files.iter().chain(lite.files) {
            let p = root.join(f.dir).join(f.name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        assert!(installed_in(root, full) && installed_in(root, lite));
        remove_in(root, full).unwrap();
        assert!(!installed_in(root, full));
        assert!(installed_in(root, lite), "voices-v1.0.bin must stay for the lite pack");
        remove_in(root, lite).unwrap();
        assert!(!root.join("kokoro").join("voices-v1.0.bin").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn default_voice_reads_prefs_then_config() {
        let dir = tempdir();
        let prefs = dir.join("studio").join("prefs.json");
        let config = dir.join("config.json");
        assert_eq!(default_voice_in(&prefs, &config), FALLBACK_VOICE);
        std::fs::write(&config, r#"{"pipeline":{"tts_voice":"am_adam","x":1},"other":true}"#).unwrap();
        assert_eq!(default_voice_in(&prefs, &config), "am_adam");
        set_default_voice_in(&prefs, &config, "bf_emma").unwrap();
        assert_eq!(default_voice_in(&prefs, &config), "bf_emma");
        let c: Value = serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(c["pipeline"]["tts_voice"], "bf_emma");
        assert_eq!(c["pipeline"]["x"], 1);
        assert_eq!(c["other"], true);
        assert!(set_default_voice_in(&prefs, &config, "nope").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!("mv-voices-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Serves a file over HTTP that drops the connection halfway once, then honours Range.
    #[test]
    fn download_resumes_after_a_dropped_connection() {
        use std::io::{Read, Write};
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let served = body.clone();
        std::thread::spawn(move || {
            for (n, stream) in listener.incoming().enumerate() {
                let Ok(mut s) = stream else { continue };
                let mut req = [0u8; 2048];
                let len = s.read(&mut req).unwrap_or(0);
                let head = String::from_utf8_lossy(&req[..len]).to_lowercase();
                let start = head.split("range: bytes=").nth(1).and_then(|r| r.split('-').next()).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
                let rest = &served[start..];
                let status = if start > 0 { "206 Partial Content" } else { "200 OK" };
                let _ = write!(s, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n", rest.len());
                // First response: send half, then hang up.
                let upto = if n == 0 { rest.len() / 2 } else { rest.len() };
                let _ = s.write_all(&rest[..upto]);
            }
        });
        let url: &'static str = Box::leak(format!("http://127.0.0.1:{port}/model.onnx").into_boxed_str());
        let files: &'static [PackFile] = Box::leak(vec![PackFile { dir: "t", name: "model.onnx", url, size: 200_000 }].into_boxed_slice());
        let pack = VoicePack { id: "t", name: "t", engine: "t", summary: "", languages: "", quality: 1, speed: 1, recommended: false, files, voices: &["af_heart"] };
        let dir = tempdir();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let mut last = None;
        rt.block_on(download_in(&dir, &pack, &CancelToken::new(), |p| last = Some(p))).unwrap();
        assert_eq!(std::fs::read(dir.join("t").join("model.onnx")).unwrap(), body);
        assert!(!dir.join("t").join("model.onnx.part").exists());
        let last = last.unwrap();
        assert_eq!(last.done, 200_000);
        assert_eq!(last.total, 200_000);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
