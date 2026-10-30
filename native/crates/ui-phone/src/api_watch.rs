//! Watch on the phone (API.md "Watch on phone"). Finds a source like the desktop Play button, opens
//! it on the PC's torrent engine (or its direct proxy for http links) without the desktop player,
//! probes it with ffprobe and streams it to the phone: as is with HTTP Range when the phone can
//! play the file, else as fragmented MP4 from ffmpeg (video copied or re-encoded to H.264, audio to
//! AAC stereo). Sessions are random ids tied to the device, end after [`IDLE`] without requests,
//! and the phone never names a URL or path: only a title id, or a chapter id for that chapter's
//! Studio recap video (a local file, served as is; not watch history).
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Path as UrlPath, Query};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use futures::StreamExt as _;
use mv_media::types::AnimeTorrentStream;
use mv_ui::nav::PlayRequest;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt as _;
use tokio::sync::oneshot;
use ui_media::remote;
use ui_player::backend::{self, Ident};
use ui_player::logic::{self, SourceKind};

use crate::api_anime::mal_ok;
use crate::api_media::friendly;
use crate::auth::{random_b64, CurrentDevice};
use crate::server::{ApiError, ApiResult};

/// A session nobody asked anything of for this long is stopped.
const IDLE: Duration = Duration::from_secs(30 * 60);
/// Sessions per device; a new one stops the oldest.
const MAX_PER_DEVICE: usize = 3;
/// How long `POST /api/watch` may look for a source that opens (torrent metadata, probing).
const OPEN_BUDGET: Duration = Duration::from_secs(90);
/// One source gets at most this long before the next is tried.
const PER_SOURCE: Duration = Duration::from_secs(60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(45);
/// Sources tried before giving up.
const MAX_TRIES: usize = 3;
/// Largest subtitle file served (and read from ffmpeg or the subtitle site).
const SUB_MAX: u64 = 4 << 20;
/// Embedded subtitles are read from the whole file: allow for a slow torrent.
const SUB_TIMEOUT: Duration = Duration::from_secs(300);
/// External subtitle tracks offered (English first).
const MAX_EXTERNAL: usize = 5;

pub const NO_FFMPEG: &str = "Install FFmpeg on your PC to watch this on your phone";

pub fn routes() -> Router {
    Router::new()
        .route("/api/watch", post(start))
        .route("/api/watch/{id}/stream", get(stream))
        .route("/api/watch/{id}/status", get(status))
        .route("/api/watch/{id}/progress", post(progress))
        .route("/api/watch/{id}/stop", post(stop))
        .route("/api/watch/{id}/sub/{file}", get(subtitle))
}

// ---------------------------------------------------------------- probing and mode

/// How the phone gets the video.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// The file as is, with Range (the phone plays it natively).
    Direct,
    /// Video copied, audio to AAC stereo, fragmented MP4.
    Remux,
    /// Video to H.264 (at most 1280 wide), audio to AAC stereo, fragmented MP4.
    Transcode,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::Direct => "direct",
            Mode::Remux => "remux",
            Mode::Transcode => "transcode",
        }
    }
}

/// An embedded text subtitle track: `n` is its number among the subtitle streams (`0:s:n`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Track {
    pub n: usize,
    pub lang: Option<String>,
    pub title: Option<String>,
}

/// What ffprobe found.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Probe {
    /// ffprobe's `format_name`, e.g. `mov,mp4,m4a,3gp,3g2,mj2` or `matroska,webm`.
    pub container: String,
    pub duration: Option<f64>,
    /// (stream index, codec, pixel format).
    pub video: Option<(usize, String, String)>,
    /// (stream index, codec).
    pub audio: Option<(usize, String)>,
    pub subs: Vec<Track>,
}

/// Reads `ffprobe -show_streams -show_format -of json` output. Cover art is not the video; the
/// default audio track wins over the first; only text subtitles are kept.
pub(crate) fn parse_probe(v: &Value) -> Probe {
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let flag = |st: &Value, k: &str| st.pointer(&format!("/disposition/{k}")).and_then(Value::as_i64) == Some(1);
    let tag = |st: &Value, k: &str| st.pointer(&format!("/tags/{k}")).and_then(Value::as_str).map(str::to_string).filter(|t| !t.trim().is_empty());
    let mut p = Probe {
        container: v.pointer("/format/format_name").and_then(Value::as_str).unwrap_or("").to_string(),
        duration: v.pointer("/format/duration").and_then(Value::as_str).and_then(|d| d.parse::<f64>().ok()).filter(|d| d.is_finite() && *d > 0.0),
        ..Default::default()
    };
    let streams = v.get("streams").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut audio_default = None;
    let mut sub_n = 0;
    for st in &streams {
        let idx = st.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let codec = s(st, "codec_name");
        match st.get("codec_type").and_then(Value::as_str) {
            Some("video") if p.video.is_none() && !flag(st, "attached_pic") => p.video = Some((idx, codec, s(st, "pix_fmt"))),
            Some("audio") => {
                if p.audio.is_none() {
                    p.audio = Some((idx, codec.clone()));
                }
                if audio_default.is_none() && flag(st, "default") {
                    audio_default = Some((idx, codec));
                }
            }
            Some("subtitle") => {
                if matches!(codec.as_str(), "subrip" | "srt" | "ass" | "ssa" | "mov_text" | "webvtt" | "text") {
                    p.subs.push(Track { n: sub_n, lang: tag(st, "language"), title: tag(st, "title") });
                }
                sub_n += 1;
            }
            _ => {}
        }
    }
    if audio_default.is_some() {
        p.audio = audio_default;
    }
    p
}

/// Direct when a phone browser plays the file as is (MP4 with 8-bit H.264 and AAC / MP3, or a
/// .webm with VP8 / VP9 / AV1 and Opus / Vorbis), remux when only the container or audio is the
/// problem, else transcode.
pub(crate) fn decide(p: &Probe, file_name: &str) -> Mode {
    let Some((_, v, pix)) = &p.video else { return Mode::Transcode };
    let audio = p.audio.as_ref().map(|(_, a)| a.as_str());
    let h264 = v == "h264" && matches!(pix.as_str(), "yuv420p" | "yuvj420p" | "");
    let mp4 = p.container.contains("mp4") || p.container.contains("mov");
    let webm = p.container.contains("webm") && file_name.to_ascii_lowercase().ends_with(".webm");
    if mp4 && h264 && audio.is_none_or(|a| matches!(a, "aac" | "mp3")) {
        return Mode::Direct;
    }
    if webm && matches!(v.as_str(), "vp8" | "vp9" | "av1") && audio.is_none_or(|a| matches!(a, "opus" | "vorbis")) {
        return Mode::Direct;
    }
    if h264 {
        Mode::Remux
    } else {
        Mode::Transcode
    }
}

/// Encoder threads: half the cores (at least 2), so the PC stays usable while it encodes.
fn threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get() / 2).unwrap_or(2).max(2)
}

/// ffmpeg arguments for a remux / transcode to fragmented MP4 on stdout, starting at `start`.
pub(crate) fn ffmpeg_args(url: &str, p: &Probe, mode: Mode, start: f64, threads: usize) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-nostdin", "-loglevel", "error"].map(String::from).to_vec();
    if start > 0.5 {
        a.extend(["-ss".into(), format!("{start:.2}")]);
    }
    a.extend(["-i".into(), url.to_string()]);
    match &p.video {
        Some((i, ..)) => a.extend(["-map".into(), format!("0:{i}")]),
        None => a.extend(["-map".into(), "0:v:0?".into()]),
    }
    match &p.audio {
        Some((i, _)) => a.extend(["-map".into(), format!("0:{i}")]),
        None => a.extend(["-map".into(), "0:a:0?".into()]),
    }
    a.extend(["-sn", "-dn"].map(String::from));
    if mode == Mode::Remux {
        a.extend(["-c:v", "copy"].map(String::from));
    } else {
        let t = threads.to_string();
        a.extend(
            ["-c:v", "libx264", "-preset", "veryfast", "-crf", "23", "-pix_fmt", "yuv420p", "-profile:v", "high", "-g", "96", "-vf", "scale='min(1280,iw)':-2", "-threads", &t]
                .map(String::from),
        );
    }
    a.extend(["-c:a", "aac", "-ac", "2", "-b:a", "160k", "-max_muxing_queue_size", "1024"].map(String::from));
    a.extend(["-movflags", "frag_keyframe+empty_moov+default_base_moof", "-f", "mp4", "pipe:1"].map(String::from));
    a
}

// ---------------------------------------------------------------- subtitles

/// SRT to WebVTT: a header, `.` for `,` in the timings, `{\an8}`-style tags dropped.
pub(crate) fn srt_to_vtt(srt: &str) -> String {
    let text = srt.trim_start_matches('\u{feff}').replace("\r\n", "\n").replace('\r', "\n");
    if text.trim_start().starts_with("WEBVTT") {
        return text;
    }
    let mut out = String::from("WEBVTT\n\n");
    for line in text.lines() {
        if line.contains("-->") {
            out.push_str(&line.replace(',', "."));
        } else {
            let mut clean = String::with_capacity(line.len());
            let mut depth = 0;
            for c in line.chars() {
                match c {
                    '{' => depth += 1,
                    '}' if depth > 0 => depth -= 1,
                    _ if depth == 0 => clean.push(c),
                    _ => {}
                }
            }
            out.push_str(&clean);
        }
        out.push('\n');
    }
    out
}

/// Readable name of a language code ("eng" -> "English"); the code itself when unknown.
fn lang_name(code: &str) -> String {
    let n = match code.to_ascii_lowercase().as_str() {
        "en" | "eng" => "English",
        "es" | "spa" => "Spanish",
        "fr" | "fre" | "fra" => "French",
        "de" | "ger" | "deu" => "German",
        "it" | "ita" => "Italian",
        "pt" | "por" | "pob" => "Portuguese",
        "ja" | "jpn" => "Japanese",
        "ar" | "ara" => "Arabic",
        "hi" | "hin" => "Hindi",
        "ru" | "rus" => "Russian",
        "zh" | "chi" | "zho" => "Chinese",
        "ko" | "kor" => "Korean",
        "nl" | "dut" | "nld" => "Dutch",
        "tr" | "tur" => "Turkish",
        "pl" | "pol" => "Polish",
        _ => return code.to_string(),
    };
    n.to_string()
}

#[derive(Clone, Debug)]
enum SubSource {
    Embedded(usize),
    External(String),
}

#[derive(Clone, Debug)]
struct Sub {
    id: String,
    label: String,
    lang: String,
    source: SubSource,
}

/// External tracks to offer: English ones first, then one per other common language.
fn pick_external(list: Vec<mv_media::types::StremioSubtitle>) -> Vec<Sub> {
    let english = |l: &str| matches!(l, "en" | "eng");
    let wanted = ["spa", "es", "fre", "fra", "fr", "ger", "deu", "de", "por", "pob", "pt", "ita", "it", "ara", "hin"];
    let mut out: Vec<mv_media::types::StremioSubtitle> = list.iter().filter(|s| english(&s.lang)).take(2).cloned().collect();
    for s in list.iter().filter(|s| wanted.contains(&s.lang.as_str())) {
        if out.len() >= MAX_EXTERNAL {
            break;
        }
        if !out.iter().any(|o| lang_name(&o.lang) == lang_name(&s.lang)) {
            out.push(s.clone());
        }
    }
    out.into_iter()
        .filter(|s| s.url.starts_with("https://") || s.url.starts_with("http://"))
        .enumerate()
        .map(|(i, s)| Sub { id: format!("x{i}"), label: format!("{} (online)", lang_name(&s.lang)), lang: s.lang.clone(), source: SubSource::External(s.url) })
        .collect()
}

// ---------------------------------------------------------------- tools

struct Tools {
    ffmpeg: Option<PathBuf>,
    ffprobe: Option<PathBuf>,
}

fn which(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(&exe)).find(|p| p.is_file())
}

/// ffmpeg and ffprobe on PATH (ffprobe also next to ffmpeg). Looked up again while missing, so
/// installing FFmpeg works without restarting.
fn tools() -> Arc<Tools> {
    static T: Mutex<Option<Arc<Tools>>> = Mutex::new(None);
    let mut g = T.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(t) = g.as_ref().filter(|t| t.ffmpeg.is_some() && t.ffprobe.is_some()) {
        return t.clone();
    }
    let ffmpeg = which("ffmpeg");
    let ffprobe = which("ffprobe").or_else(|| {
        let p = ffmpeg.as_ref()?.with_file_name(if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" });
        p.is_file().then_some(p)
    });
    let t = Arc::new(Tools { ffmpeg, ffprobe });
    *g = Some(t.clone());
    t
}

/// A command that never opens a console window, killed when dropped.
fn quiet(program: &PathBuf) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(program);
    c.stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    c
}

async fn probe(ffprobe: &PathBuf, url: &str) -> Result<Probe, String> {
    let mut c = quiet(ffprobe);
    c.args(["-v", "error", "-show_streams", "-show_format", "-of", "json", url]).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = tokio::time::timeout(PROBE_TIMEOUT, c.output()).await.map_err(|_| "The video took too long to start".to_string())?.map_err(|e| format!("ffprobe failed: {e}"))?;
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|_| "The video could not be read".to_string())?;
    let p = parse_probe(&v);
    if p.video.is_none() && p.audio.is_none() {
        return Err("The video could not be read".into());
    }
    Ok(p)
}

// ---------------------------------------------------------------- sessions

struct Live {
    last: Instant,
    /// Stops the running ffmpeg stream (a newer request or `stop` sends it).
    ffmpeg: Option<oneshot::Sender<()>>,
    subs: HashMap<String, Arc<String>>,
}

struct Session {
    device: String,
    /// The title's watch history entry; `None` for a Studio recap, whose progress is not recorded.
    ident: Option<Ident>,
    /// Torrent info-hash, released on stop.
    hash: Option<String>,
    /// The engine's local URL for the file (`/stream/<hash>/<idx>` or `/direct?url=`), or the
    /// path of [`Session::file`].
    url: String,
    /// A file on this PC (a Studio recap video), served directly instead of through the engine.
    file: Option<PathBuf>,
    mode: Mode,
    probe: Probe,
    subs: Vec<Sub>,
    created: Instant,
    live: Mutex<Live>,
}

impl Session {
    fn live(&self) -> std::sync::MutexGuard<'_, Live> {
        self.live.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn touch(&self) {
        self.live().last = Instant::now();
    }
}

static SESSIONS: Mutex<Option<HashMap<String, Arc<Session>>>> = Mutex::new(None);

fn with_sessions<T>(f: impl FnOnce(&mut HashMap<String, Arc<Session>>) -> T) -> T {
    f(SESSIONS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new))
}

/// Kills the session's ffmpeg and releases its torrent unless another session still streams it.
fn release(s: &Session) {
    if let Some(tx) = s.live().ffmpeg.take() {
        let _ = tx.send(());
    }
    if let Some(h) = &s.hash {
        let shared = with_sessions(|m| m.values().any(|o| o.hash.as_ref() == Some(h)));
        if !shared {
            backend::release_torrent(h.clone());
        }
    }
}

fn remove(id: &str) -> Option<Arc<Session>> {
    let s = with_sessions(|m| m.remove(id))?;
    release(&s);
    Some(s)
}

/// Stops sessions idle for [`IDLE`].
fn prune() {
    let idle: Vec<String> = with_sessions(|m| m.iter().filter(|(_, s)| s.live().last.elapsed() >= IDLE).map(|(k, _)| k.clone()).collect());
    for id in idle {
        remove(&id);
        eprintln!("[phone] watch session expired");
    }
}

/// Stops every session (Phone access turned off).
pub fn stop_all() {
    let ids: Vec<String> = with_sessions(|m| m.keys().cloned().collect());
    for id in ids {
        remove(&id);
    }
}

/// Adds a session (stopping the device's oldest past [`MAX_PER_DEVICE`]) and returns its id.
fn insert(s: Session) -> String {
    static SWEEPER: OnceLock<()> = OnceLock::new();
    SWEEPER.get_or_init(|| {
        tokio::spawn(async {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                prune();
            }
        });
    });
    prune();
    let mine: Vec<(String, Instant)> = with_sessions(|m| m.iter().filter(|(_, o)| o.device == s.device).map(|(k, o)| (k.clone(), o.created)).collect());
    if mine.len() >= MAX_PER_DEVICE {
        let mut mine = mine;
        mine.sort_by_key(|(_, c)| *c);
        for (id, _) in mine.iter().take(mine.len() + 1 - MAX_PER_DEVICE) {
            remove(id);
        }
    }
    let id = random_b64(24);
    with_sessions(|m| m.insert(id.clone(), Arc::new(s)));
    id
}

/// The device's session `id` (refreshing its idle timer); 404 for anyone else's or unknown ids.
fn session(id: &str, device: &str) -> ApiResult<Arc<Session>> {
    let s = with_sessions(|m| m.get(id).cloned()).filter(|s| s.device == device).ok_or_else(|| ApiError::not_found("This video session has ended"))?;
    s.touch();
    Ok(s)
}

// ---------------------------------------------------------------- opening a source

/// Sources a phone streams best first: the desktop's order, but 4K, remuxes and HEVC / 10-bit
/// releases (which need a heavy transcode) after the rest.
fn rank_for_phone(list: Vec<(PlayRequest, AnimeTorrentStream)>) -> Vec<PlayRequest> {
    let penalty = |s: &AnimeTorrentStream| -> u8 {
        let text = format!("{} {}", s.quality.as_deref().unwrap_or(""), s.title).to_lowercase();
        let words: Vec<&str> = text.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
        let has = |ws: &[&str]| words.iter().any(|w| ws.contains(w));
        let mut p = 0;
        if has(&["2160p", "4k", "uhd"]) {
            p += 4;
        }
        if has(&["remux", "bdremux"]) {
            p += 3;
        }
        if has(&["hevc", "x265", "h265", "10bit", "av1"]) {
            p += 2;
        }
        if has(&["hdr", "hdr10", "dv", "dovi"]) {
            p += 1;
        }
        if s.seeders.is_some_and(|n| n < 3) {
            p += 2;
        }
        p
    };
    let mut list: Vec<(u8, PlayRequest)> = list.into_iter().map(|(r, s)| (penalty(&s), r)).collect();
    list.sort_by_key(|(p, _)| *p);
    list.into_iter().map(|(_, r)| r).collect()
}

struct Opened {
    url: String,
    hash: Option<String>,
    file_name: String,
}

/// Opens a source on the PC's engine: torrents wait for metadata, http links go through the
/// engine's Range-forwarding proxy. Local files are never streamed from here.
async fn open(req: &PlayRequest) -> Result<Opened, String> {
    let engine = backend::engine().await.map_err(|e| format!("The PC's stream engine did not start: {e}"))?;
    match logic::classify_source(&req.source) {
        SourceKind::Torrent(src) => {
            let added = engine.add(src, mv_torrent::AddOptions { file_idx: req.file_idx, ..Default::default() }).await.map_err(|e| format!("The torrent could not be opened: {e}"))?;
            Ok(Opened { url: added.stream_url, hash: Some(added.info_hash), file_name: added.file_name })
        }
        SourceKind::Url(u) => {
            let file_name = reqwest::Url::parse(&u).ok().and_then(|p| p.path_segments().and_then(|mut s| s.next_back().map(str::to_string))).unwrap_or_default();
            Ok(Opened { url: engine.direct_url(&u), hash: None, file_name })
        }
        SourceKind::File(_) => Err("This source cannot be streamed to a phone".into()),
    }
}

/// Mode for a file ffprobe could not look at: a .mp4 / .m4v plays as is, anything else is
/// transcoded to be safe.
fn guess_mode(file_name: &str) -> Mode {
    let n = file_name.to_ascii_lowercase();
    if n.ends_with(".mp4") || n.ends_with(".m4v") {
        Mode::Direct
    } else {
        Mode::Transcode
    }
}

#[derive(Deserialize)]
struct WatchBody {
    imdb: Option<String>,
    mal: Option<String>,
    /// `kind: "recap"`: the chapter whose Studio recap video to play.
    chapter: Option<String>,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    title: String,
    season: Option<u32>,
    episode: Option<u32>,
}

fn imdb_ok(id: &str) -> bool {
    id.len() > 2 && id.len() <= 16 && id.starts_with("tt") && id[2..].bytes().all(|c| c.is_ascii_digit())
}

async fn start(Extension(dev): CurrentDevice, Json(b): Json<WatchBody>) -> ApiResult<Json<Value>> {
    let title: String = b.title.trim().chars().take(200).collect();
    let (season, episode) = (b.season.map(|s| s.min(10_000)), b.episode.map(|e| e.clamp(1, 100_000)));
    let anime = b.kind == "anime";
    if b.kind == "recap" {
        return start_recap(dev, b.chapter, title).await;
    }
    let found = match b.kind.as_str() {
        "anime" => {
            let mal = b.mal.as_deref().map(str::trim).filter(|m| mal_ok(m)).ok_or_else(|| ApiError::bad_request("mal must be a MyAnimeList id"))?.to_string();
            remote::anime_sources(if title.is_empty() { mal.clone() } else { title.clone() }, mal, episode).await
        }
        "movie" | "series" => {
            let imdb = b.imdb.as_deref().map(str::trim).filter(|i| imdb_ok(i)).ok_or_else(|| ApiError::bad_request("imdb must be an IMDb id (tt...)"))?.to_string();
            remote::sources(if title.is_empty() { imdb.clone() } else { title.clone() }, imdb, b.kind == "movie", season, episode).await
        }
        _ => return Err(ApiError::bad_request("kind must be movie, series, anime or recap")),
    };
    let found = found.map_err(|e| ApiError::not_found(friendly(e)))?;
    if found.is_empty() {
        return Err(ApiError::not_found("No sources found for this title."));
    }
    let tools = tools();
    let deadline = Instant::now() + OPEN_BUDGET;
    let mut error = String::new();
    let mut opened = None;
    for req in rank_for_phone(found).into_iter().take(MAX_TRIES) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left < Duration::from_secs(10) {
            break;
        }
        let attempt = async {
            let t = Instant::now();
            let o = open(&req).await?;
            eprintln!("[phone] watch: opened {:?} after {} ms, probing", o.file_name, t.elapsed().as_millis());
            let p = match &tools.ffprobe {
                Some(ffprobe) => Some(probe(ffprobe, &o.url).await.inspect_err(|_| release_opened(&o))?),
                None => None,
            };
            Ok::<_, String>((o, p))
        };
        match tokio::time::timeout(left.min(PER_SOURCE), attempt).await {
            Ok(Ok((o, p))) => {
                let mode = p.as_ref().map_or_else(|| guess_mode(&o.file_name), |p| decide(p, &o.file_name));
                if mode != Mode::Direct && tools.ffmpeg.is_none() {
                    release_opened(&o);
                    error = NO_FFMPEG.into();
                    continue;
                }
                opened = Some((req, o, p.unwrap_or_default(), mode));
                break;
            }
            Ok(Err(e)) => {
                eprintln!("[phone] watch: a source failed: {e}");
                error = e;
            }
            Err(_) => {
                if let Some(h) = backend::source_hash(&req) {
                    backend::release_torrent(h);
                }
                error = "The video did not start in time (the source may have no seeders). Try again or pick another title.".into();
            }
        }
    }
    let Some((req, o, probe, mode)) = opened else {
        let status = if error == NO_FFMPEG {
            StatusCode::SERVICE_UNAVAILABLE
        } else if error.contains("in time") {
            StatusCode::GATEWAY_TIMEOUT
        } else {
            StatusCode::BAD_GATEWAY
        };
        return Err(ApiError(status, if error.is_empty() { "No playable sources were found for this title.".into() } else { error }));
    };
    eprintln!("[phone] watch on phone: {} ({}, {})", req.title, mode.as_str(), o.file_name);

    let ident = backend::ident(&req);
    let imdb = req.imdb_id.clone().filter(|i| i.starts_with("tt") && !anime);
    let (resume, external, next) = tokio::join!(
        {
            let ident = ident.clone();
            async move { tokio::task::spawn_blocking(move || backend::load_resume(&ident)).await.ok().flatten() }
        },
        async {
            let Some(imdb) = imdb.clone() else { return vec![] };
            match tokio::time::timeout(Duration::from_secs(8), backend::subtitles(imdb, req.season, req.episode)).await {
                Ok(Ok(list)) => pick_external(list),
                _ => vec![],
            }
        },
        next_of(&req, anime),
    );
    let mut subs: Vec<Sub> = probe
        .subs
        .iter()
        .map(|t| {
            let lang = t.lang.clone().unwrap_or_else(|| "und".into());
            let label = match (&t.title, t.lang.as_deref()) {
                (Some(title), _) => title.clone(),
                (None, Some(l)) => lang_name(l),
                (None, None) => format!("Track {}", t.n + 1),
            };
            Sub { id: format!("e{}", t.n), label, lang, source: SubSource::Embedded(t.n) }
        })
        .collect();
    subs.extend(external);

    let duration = probe.duration;
    {
        let (ident, dur) = (ident.clone(), duration.unwrap_or(0.0));
        tokio::task::spawn_blocking(move || backend::save_event(&ident, "play", 0.0, dur));
    }
    remote::remember(&req);
    let title = req.title.clone();
    let sub_list: Vec<(String, String, String)> = subs.iter().map(|s| (s.id.clone(), s.label.clone(), s.lang.clone())).collect();
    let now = Instant::now();
    let id = insert(Session {
        device: dev.id.clone(),
        ident: Some(ident),
        hash: o.hash,
        url: o.url,
        file: None,
        mode,
        probe,
        subs,
        created: now,
        live: Mutex::new(Live { last: now, ffmpeg: None, subs: HashMap::new() }),
    });
    let subtitles: Vec<Value> = sub_list.into_iter().map(|(sid, label, lang)| json!({ "id": sid, "label": label, "lang": lang, "url": format!("/api/watch/{id}/sub/{sid}.vtt") })).collect();
    Ok(Json(json!({
        "id": id,
        "title": title,
        "duration": duration,
        "mode": mode.as_str(),
        "url": format!("/api/watch/{id}/stream"),
        "resume": resume,
        "subtitles": subtitles,
        "next": next,
    })))
}

/// A chapter's Studio recap video: a local MP4 (H.264 + AAC from Studio), played as is when ffprobe
/// agrees or is missing. No history, resume, subtitles or next episode.
async fn start_recap(dev: crate::auth::Device, chapter: Option<String>, title: String) -> ApiResult<Json<Value>> {
    let chapter = chapter.map(|c| c.trim().to_string()).filter(|c| !c.is_empty() && c.len() <= 200).ok_or_else(|| ApiError::bad_request("chapter must be a chapter id"))?;
    let (m, ch) = crate::api_manga::load_chapter(chapter).await?;
    let label = if title.is_empty() { m.title.clone() } else { title };
    let video = tokio::task::spawn_blocking(move || ui_manga::recap::find(&m.id, &ch.id).and_then(|r| r.video)).await.ok().flatten().ok_or_else(|| ApiError::not_found("This chapter has no recap video"))?;
    let url = video.to_string_lossy().into_owned();
    let tools = tools();
    // A file ffprobe cannot read is still Studio's own MP4: play it as is.
    let probe = match &tools.ffprobe {
        Some(ffprobe) => probe(ffprobe, &url).await.ok(),
        None => None,
    };
    let mode = probe.as_ref().map_or(Mode::Direct, |p| decide(p, "video_final.mp4"));
    if mode != Mode::Direct && tools.ffmpeg.is_none() {
        return Err(ApiError::unavailable(NO_FFMPEG));
    }
    eprintln!("[phone] watch recap on phone ({})", mode.as_str());
    let probe = probe.unwrap_or_default();
    let duration = probe.duration;
    let now = Instant::now();
    let id = insert(Session {
        device: dev.id.clone(),
        ident: None,
        hash: None,
        url,
        file: Some(video),
        mode,
        probe,
        subs: vec![],
        created: now,
        live: Mutex::new(Live { last: now, ffmpeg: None, subs: HashMap::new() }),
    });
    Ok(Json(json!({
        "id": id,
        "title": label,
        "duration": duration,
        "mode": mode.as_str(),
        "url": format!("/api/watch/{id}/stream"),
        "resume": null,
        "subtitles": [],
        "next": null,
    })))
}

fn release_opened(o: &Opened) {
    if let Some(h) = &o.hash {
        let shared = with_sessions(|m| m.values().any(|s| s.hash.as_ref() == Some(h)));
        if !shared {
            backend::release_torrent(h.clone());
        }
    }
}

/// The following episode, when known: `{ season, episode, title }`.
async fn next_of(req: &PlayRequest, anime: bool) -> Value {
    let Some(ep) = req.episode else { return Value::Null };
    let found = if anime {
        let Some(mal) = req.mal_id.clone() else { return Value::Null };
        tokio::time::timeout(Duration::from_secs(6), remote::anime_next(&mal, ep)).await.ok().flatten().map(|(e, t)| (1, e, t))
    } else {
        let Some(imdb) = req.imdb_id.clone() else { return Value::Null };
        tokio::time::timeout(Duration::from_secs(6), remote::next_episode(&imdb, req.season.unwrap_or(1), ep)).await.ok().flatten()
    };
    found.map_or(Value::Null, |(s, e, t)| json!({ "season": s, "episode": e, "title": t }))
}

// ---------------------------------------------------------------- streaming

fn local_http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| reqwest::Client::builder().no_proxy().connect_timeout(Duration::from_secs(5)).build().unwrap_or_default())
}

#[derive(Deserialize)]
struct StreamQuery {
    start: Option<f64>,
}

async fn stream(Extension(dev): CurrentDevice, UrlPath(id): UrlPath<String>, Query(q): Query<StreamQuery>, headers: HeaderMap) -> ApiResult<Response> {
    let s = session(&id, &dev.id)?;
    match s.mode {
        Mode::Direct => match &s.file {
            Some(f) => serve_local(f, &headers).await,
            None => proxy(&s, &headers).await,
        },
        _ => piped(s, q.start.filter(|t| t.is_finite()).unwrap_or(0.0).clamp(0.0, 1e6)),
    }
}

/// The file as is from the engine, forwarding Range so the phone seeks natively.
async fn proxy(s: &Session, headers: &HeaderMap) -> ApiResult<Response> {
    let mut rq = local_http().get(&s.url);
    if let Some(r) = headers.get(header::RANGE) {
        rq = rq.header(header::RANGE, r.clone());
    }
    let up = rq.send().await.map_err(|_| ApiError(StatusCode::BAD_GATEWAY, "The video stream stopped".into()))?;
    let status = up.status();
    let mut out = HeaderMap::new();
    for h in [header::CONTENT_RANGE, header::CONTENT_LENGTH, header::ACCEPT_RANGES] {
        if let Some(v) = up.headers().get(&h) {
            out.insert(h, v.clone());
        }
    }
    let mime = if s.probe.container.contains("webm") { "video/webm" } else { "video/mp4" };
    out.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    out.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    out.entry(header::ACCEPT_RANGES).or_insert(HeaderValue::from_static("bytes"));
    let mut r = Response::new(Body::from_stream(up.bytes_stream()));
    *r.status_mut() = status;
    *r.headers_mut() = out;
    Ok(r)
}

/// A local file (a recap video) as is, with Range.
async fn serve_local(path: &Path, headers: &HeaderMap) -> ApiResult<Response> {
    let mut rq = axum::http::Request::new(Body::empty());
    for h in [header::RANGE, header::IF_RANGE] {
        if let Some(v) = headers.get(&h) {
            rq.headers_mut().insert(h, v.clone());
        }
    }
    let res = tower_http::services::ServeFile::new(path).try_call(rq).await.map_err(|_| ApiError::not_found("This video session has ended"))?;
    let (mut parts, body) = res.into_parts();
    parts.headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(Response::from_parts(parts, Body::new(body)))
}

/// Fragmented MP4 from ffmpeg, from `start` seconds. A newer request for the session (a seek)
/// or `stop` ends this one; so does the phone going away (the body is dropped, ffmpeg killed).
fn piped(s: Arc<Session>, start: f64) -> ApiResult<Response> {
    let ffmpeg = tools().ffmpeg.clone().ok_or_else(|| ApiError::unavailable(NO_FFMPEG))?;
    let mut c = quiet(&ffmpeg);
    c.args(ffmpeg_args(&s.url, &s.probe, s.mode, start, threads())).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().map_err(|e| ApiError::unavailable(format!("{NO_FFMPEG} ({e})")))?;
    let stdout = child.stdout.take().ok_or_else(|| ApiError::internal("no ffmpeg output"))?;
    if let Some(mut err) = child.stderr.take() {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = (&mut err).take(4096).read_to_end(&mut buf).await;
            let msg = String::from_utf8_lossy(&buf);
            if !msg.trim().is_empty() {
                eprintln!("[phone] ffmpeg: {}", msg.trim());
            }
            // Keep draining so ffmpeg never blocks on a full stderr pipe.
            let _ = tokio::io::copy(&mut err, &mut tokio::io::sink()).await;
        });
    }
    let (tx, rx) = oneshot::channel();
    if let Some(old) = s.live().ffmpeg.replace(tx) {
        let _ = old.send(());
    }
    let reader = tokio_util::io::ReaderStream::with_capacity(stdout, 64 * 1024);
    let body = futures::stream::unfold((child, reader, rx, s), |(child, mut reader, mut rx, s)| async move {
        tokio::select! {
            _ = &mut rx => None,
            chunk = reader.next() => {
                let chunk = chunk?;
                s.touch();
                Some((chunk, (child, reader, rx, s)))
            }
        }
    });
    let mut r = Response::new(Body::from_stream(body));
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("video/mp4"));
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(r)
}

async fn status(Extension(dev): CurrentDevice, UrlPath(id): UrlPath<String>) -> ApiResult<Json<Value>> {
    let s = session(&id, &dev.id)?;
    let Some(hash) = &s.hash else {
        return Ok(Json(json!({ "state": "ready", "peers": null, "down_bps": null, "progress": null, "error": null })));
    };
    let stats = backend::engine_if_started().and_then(|e| e.stats(hash).ok());
    Ok(Json(match stats {
        Some(st) => json!({
            "state": if st.error.is_some() { "error" } else { "ready" },
            "peers": st.peers,
            "down_bps": st.download_speed.round() as u64,
            "progress": st.progress,
            "error": st.error,
        }),
        None => json!({ "state": "error", "peers": null, "down_bps": null, "progress": null, "error": "The torrent stopped on the PC" }),
    }))
}

#[derive(Deserialize)]
struct ProgressBody {
    pos: f64,
    #[serde(default)]
    duration: f64,
}

async fn progress(Extension(dev): CurrentDevice, UrlPath(id): UrlPath<String>, Json(b): Json<ProgressBody>) -> ApiResult<Json<Value>> {
    let s = session(&id, &dev.id)?;
    let ok = |v: f64| v.is_finite() && (0.0..=1e6).contains(&v);
    if !ok(b.pos) || !ok(b.duration) {
        return Err(ApiError::bad_request("pos and duration must be seconds"));
    }
    let dur = if b.duration > 0.0 { b.duration } else { s.probe.duration.unwrap_or(0.0) };
    // A recap is not a title: nothing goes into watch history.
    let Some(ident) = s.ident.clone() else { return Ok(Json(json!({}))) };
    if dur <= 0.0 {
        return Ok(Json(json!({})));
    }
    let kind = if logic::is_complete(b.pos, dur) { "complete" } else { "progress" };
    let pos = b.pos.min(dur);
    tokio::task::spawn_blocking(move || backend::save_event(&ident, kind, pos, dur)).await.map_err(ApiError::internal)?.map_err(ApiError::from)?;
    Ok(Json(json!({})))
}

async fn stop(Extension(dev): CurrentDevice, UrlPath(id): UrlPath<String>) -> ApiResult<Json<Value>> {
    session(&id, &dev.id)?;
    remove(&id);
    Ok(Json(json!({})))
}

async fn read_capped(mut r: impl tokio::io::AsyncRead + Unpin) -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = (&mut r).take(SUB_MAX).read_to_end(&mut buf).await;
    buf
}

async fn fetch_sub(s: &Session, sub: &Sub) -> Result<String, ApiError> {
    let failed = || ApiError(StatusCode::BAD_GATEWAY, "The subtitles could not be loaded".into());
    match &sub.source {
        SubSource::Embedded(n) => {
            let ffmpeg = tools().ffmpeg.clone().ok_or_else(|| ApiError::unavailable(NO_FFMPEG))?;
            let mut c = quiet(&ffmpeg);
            c.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-i", &s.url, "-map", &format!("0:s:{n}"), "-f", "webvtt", "pipe:1"]).stdout(Stdio::piped()).stderr(Stdio::null());
            let mut child = c.spawn().map_err(|_| failed())?;
            let out = child.stdout.take().ok_or_else(failed)?;
            let bytes = tokio::time::timeout(SUB_TIMEOUT, read_capped(out)).await.map_err(|_| failed())?;
            let _ = child.kill().await;
            let text = String::from_utf8_lossy(&bytes).into_owned();
            text.trim_start().starts_with("WEBVTT").then_some(text).ok_or_else(failed)
        }
        SubSource::External(url) => {
            let res = local_http().get(url).timeout(Duration::from_secs(20)).send().await.map_err(|_| failed())?;
            if !res.status().is_success() {
                return Err(failed());
            }
            let mut body = Vec::new();
            let mut st = res.bytes_stream();
            while let Some(chunk) = st.next().await {
                let chunk = chunk.map_err(|_| failed())?;
                if body.len() as u64 + chunk.len() as u64 > SUB_MAX {
                    return Err(failed());
                }
                body.extend_from_slice(&chunk);
            }
            Ok(srt_to_vtt(&String::from_utf8_lossy(&body)))
        }
    }
}

async fn subtitle(Extension(dev): CurrentDevice, UrlPath((id, file)): UrlPath<(String, String)>) -> ApiResult<Response> {
    let s = session(&id, &dev.id)?;
    let sid = file.strip_suffix(".vtt").ok_or_else(|| ApiError::not_found("No such subtitles"))?;
    let sub = s.subs.iter().find(|x| x.id == sid).cloned().ok_or_else(|| ApiError::not_found("No such subtitles"))?;
    let hit = s.live().subs.get(sid).cloned();
    let text = match hit {
        Some(t) => t,
        None => {
            let t = Arc::new(fetch_sub(&s, &sub).await?);
            s.live().subs.insert(sid.to_string(), t.clone());
            t
        }
    };
    let mut r = Response::new(Body::from(text.as_str().to_owned()));
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/vtt; charset=utf-8"));
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=3600"));
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_of(s: &str) -> Probe {
        parse_probe(&serde_json::from_str(s).unwrap())
    }

    const MP4_H264_AAC: &str = r#"{"streams":[{"index":0,"codec_name":"h264","codec_type":"video","pix_fmt":"yuv420p"},{"index":1,"codec_name":"aac","codec_type":"audio","disposition":{"default":1}}],
        "format":{"format_name":"mov,mp4,m4a,3gp,3g2,mj2","duration":"596.474195"}}"#;
    const MKV_H264_AC3: &str = r#"{"streams":[{"index":0,"codec_name":"h264","codec_type":"video","pix_fmt":"yuv420p"},
        {"index":1,"codec_name":"ac3","codec_type":"audio","tags":{"language":"eng"},"disposition":{"default":0}},
        {"index":2,"codec_name":"aac","codec_type":"audio","tags":{"language":"jpn"},"disposition":{"default":1}},
        {"index":3,"codec_name":"hdmv_pgs_subtitle","codec_type":"subtitle"},
        {"index":4,"codec_name":"subrip","codec_type":"subtitle","tags":{"language":"eng","title":"English [SDH]"}},
        {"index":5,"codec_name":"ass","codec_type":"subtitle","tags":{"language":"spa"}},
        {"index":6,"codec_name":"mjpeg","codec_type":"video","disposition":{"attached_pic":1}}],
        "format":{"format_name":"matroska,webm","duration":"1420.5"}}"#;
    const MKV_HEVC: &str = r#"{"streams":[{"index":0,"codec_name":"mjpeg","codec_type":"video","disposition":{"attached_pic":1}},{"index":1,"codec_name":"hevc","codec_type":"video","pix_fmt":"yuv420p10le"},{"index":2,"codec_name":"eac3","codec_type":"audio"}],
        "format":{"format_name":"matroska,webm"}}"#;

    #[test]
    fn mode_from_probe() {
        let p = probe_of(MP4_H264_AAC);
        assert_eq!(p.duration, Some(596.474195));
        assert_eq!(decide(&p, "Big.Buck.Bunny.mp4"), Mode::Direct);

        let p = probe_of(MKV_H264_AC3);
        assert_eq!(p.video, Some((0, "h264".into(), "yuv420p".into())));
        assert_eq!(p.audio, Some((2, "aac".into())), "the default audio track");
        assert_eq!(p.subs, vec![Track { n: 1, lang: Some("eng".into()), title: Some("English [SDH]".into()) }, Track { n: 2, lang: Some("spa".into()), title: None }]);
        assert_eq!(decide(&p, "Show.S01E01.mkv"), Mode::Remux, "H.264 in Matroska");

        let p = probe_of(MKV_HEVC);
        assert_eq!(p.video.as_ref().map(|v| v.0), Some(1), "cover art is not the video");
        assert_eq!(decide(&p, "Movie.2160p.mkv"), Mode::Transcode);

        // H.264 High 10 cannot be played by phones; MP4 with AC-3 needs the audio converted.
        let ten = MP4_H264_AAC.replace("yuv420p", "yuv420p10le");
        assert_eq!(decide(&probe_of(&ten), "x.mp4"), Mode::Transcode);
        let ac3 = MP4_H264_AAC.replace("\"aac\"", "\"ac3\"");
        assert_eq!(decide(&probe_of(&ac3), "x.mp4"), Mode::Remux);
        // WebM plays as is only when it really is a .webm with web codecs.
        let webm = r#"{"streams":[{"index":0,"codec_name":"vp9","codec_type":"video","pix_fmt":"yuv420p"},{"index":1,"codec_name":"opus","codec_type":"audio"}],"format":{"format_name":"matroska,webm"}}"#;
        assert_eq!(decide(&probe_of(webm), "clip.webm"), Mode::Direct);
        assert_eq!(decide(&probe_of(webm), "clip.mkv"), Mode::Transcode);
        assert_eq!(decide(&Probe::default(), ""), Mode::Transcode);
        assert_eq!(guess_mode("A.MP4"), Mode::Direct);
        assert_eq!(guess_mode("a.mkv"), Mode::Transcode);
    }

    #[test]
    fn ffmpeg_command() {
        let p = probe_of(MKV_H264_AC3);
        let a = ffmpeg_args("http://127.0.0.1:1/stream/x/0", &p, Mode::Remux, 61.0, 4).join(" ");
        assert!(a.starts_with("-hide_banner -nostdin -loglevel error -ss 61.00 -i http://127.0.0.1:1/stream/x/0 -map 0:0 -map 0:2 -sn -dn -c:v copy"), "{a}");
        assert!(a.ends_with("-movflags frag_keyframe+empty_moov+default_base_moof -f mp4 pipe:1"), "{a}");
        let a = ffmpeg_args("u", &probe_of(MKV_HEVC), Mode::Transcode, 0.0, 4).join(" ");
        assert!(!a.contains("-ss") && a.contains("-map 0:1 -map 0:2") && a.contains("libx264") && a.contains("-threads 4") && a.contains("-c:a aac -ac 2"), "{a}");
    }

    #[test]
    fn srt_becomes_webvtt() {
        let srt = "\u{feff}1\r\n00:00:01,600 --> 00:00:04,200\r\n{\\an8}Hello, world\r\n\r\n2\r\n00:01:02,000 --> 00:01:03,500\r\n<i>Bye</i>\r\n";
        let vtt = srt_to_vtt(srt);
        assert_eq!(vtt, "WEBVTT\n\n1\n00:00:01.600 --> 00:00:04.200\nHello, world\n\n2\n00:01:02.000 --> 00:01:03.500\n<i>Bye</i>\n");
        assert_eq!(srt_to_vtt("WEBVTT\n\n00:01.000 --> 00:02.000\nx\n"), "WEBVTT\n\n00:01.000 --> 00:02.000\nx\n");
    }

    #[test]
    fn phone_friendly_sources_first() {
        let s = |title: &str, src: &str| (PlayRequest { title: "t".into(), source: src.into(), file_idx: None, imdb_id: None, mal_id: None, season: None, episode: None }, AnimeTorrentStream { title: title.into(), seeders: Some(50), info_hash: src.into(), ..Default::default() });
        let ranked = rank_for_phone(vec![s("Movie 2160p HDR REMUX", "a"), s("Movie 1080p x265 10bit", "b"), s("Movie 1080p x264", "c")]);
        assert_eq!(ranked.iter().map(|r| r.source.as_str()).collect::<Vec<_>>(), ["c", "b", "a"]);
    }

    #[test]
    fn external_subtitles_english_first() {
        let sub = |lang: &str, url: &str| mv_media::types::StremioSubtitle { title: lang.into(), lang: lang.into(), url: url.into() };
        let picked = pick_external(vec![sub("fre", "https://s/1"), sub("eng", "https://s/2"), sub("eng", "https://s/3"), sub("eng", "https://s/4"), sub("spa", "https://s/5"), sub("fra", "https://s/6"), sub("ger", "file:///x")]);
        let labels: Vec<&str> = picked.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["English (online)", "English (online)", "French (online)", "Spanish (online)"]);
        assert_eq!(picked[0].id, "x0");
    }
}
