//! librqbit-backed engine: add by magnet / info-hash / .torrent bytes, pick a
//! file, stream it over the local HTTP server, report stats, pause / remove,
//! prefetch (warm a source before playback) and evict idle torrents.
//!
//! Streaming latency work lives mostly in the vendored librqbit (see
//! `vendor/librqbit/Cargo.toml`): playhead pieces are striped across the
//! fastest peers, seeks pre-empt the old window, per-peer request queues are
//! sized to ~0.5 s of that peer's rate. This file adds the session-level
//! pieces: persistent DHT + fastresume, container-index warm-up (first / last
//! pieces), Stremio-style prefetch, and a linger cache so replay and seek-back
//! are instant.

use std::collections::{HashMap, HashSet};
use std::io::{self, SeekFrom};
use std::net::{Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context as _, Result};
use futures::future::{BoxFuture, FutureExt, Shared};
use librqbit::api::TorrentIdOrHash;
use librqbit::dht::{DhtPersistenceConfig, Id20};
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ConnectionOptions, DhtSessionConfig, FileStream,
    ListenerMode, ListenerOptions, Magnet, ManagedTorrent, PeerConnectionOptions, Session, SessionOptions,
    SessionPersistenceConfig,
};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncSeekExt, ReadBuf};

use crate::http::{mime_from_name, BoxReader, ByteSource, SourceResolver, StreamServer};

/// Public trackers merged into every torrent, on top of the magnet's own
/// `tr=` list (ngosang/trackerslist "best", 2026-10, plus a few HTTP(S)
/// fallbacks for networks that block UDP). wss:// entries are WebTorrent-only.
pub const DEFAULT_TRACKERS: &[&str] = &[
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://open.demonii.com:1337/announce",
    "udp://exodus.desync.com:6969/announce",
    "udp://tracker.skynetcloud.site:6969/announce",
    "udp://tracker.qu.ax:6969/announce",
    "udp://tracker.ducks.party:1984/announce",
    "udp://tracker.tryhackx.org:6969/announce",
    "udp://tracker.nyaa.vc:6969/announce",
    "udp://explodie.org:6969/announce",
    "udp://tracker.bittor.pw:1337/announce",
    "udp://tracker-udp.gbitt.info:80/announce",
    "udp://tracker.dler.org:6969/announce",
    "udp://tracker.peerfect.org:6969/announce",
    "udp://opentracker.io:6969/announce",
    "http://tracker.opentrackr.org:1337/announce",
    "http://tracker2.dler.org:80/announce",
    "http://tracker.renfei.net:8080/announce",
    "https://tracker.gbitt.info:443/announce",
    "http://nyaa.tracker.wf:7777/announce",
];

const VIDEO_EXTS: &[&str] = &[
    ".mp4", ".mkv", ".avi", ".webm", ".m4v", ".ts", ".m2ts", ".mov", ".flv", ".wmv", ".mpg", ".mpeg",
];

/// Bytes at the start of the file warmed as soon as metadata arrives
/// (MP4 `ftyp`/`moov` at the front, MKV EBML header + SeekHead + Tracks).
const HEAD_WARM_BYTES: u64 = 4 * 1024 * 1024;
/// Warm-up streams are dropped after this long even if incomplete.
const WARMER_TTL: Duration = Duration::from_secs(90);
/// Connected peers per torrent. Measured: 55 (also Stremio's default) beats
/// 250 clearly on seek latency and in-order throughput, since every extra slow
/// peer holds a few outstanding requests that keep arriving after a seek.
const DEFAULT_PEER_LIMIT: usize = 55;

#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Default download / cache directory (injectable; tests use temp dirs).
    pub download_dir: PathBuf,
    /// Trackers merged into every torrent.
    pub trackers: Vec<String>,
    /// Run the DHT node.
    pub enable_dht: bool,
    /// Accept incoming peer connections (TCP + uTP).
    pub enable_listen: bool,
    /// Ask the router to forward the listen port (UPnP).
    pub enable_upnp: bool,
    /// Persist the DHT routing table and fastresume state (piece bitfields)
    /// in [`EngineConfig::session_dir`] so restarts bootstrap and resume fast.
    pub persist_session: bool,
    /// Where DHT / fastresume state lives. Default `<download_dir>/.mv-session`.
    pub session_dir: Option<PathBuf>,
    /// Upload cap in bytes/s so streaming keeps the downlink busy. `None` = unlimited.
    pub upload_limit_bps: Option<u32>,
    /// Max peers per torrent.
    pub peer_limit: Option<usize>,
    /// Give up on magnet metadata after this long.
    pub metadata_timeout: Duration,
    /// A released torrent (no open streams) keeps downloading this long, then
    /// is paused (kept in the session: instant resume, no re-check).
    pub linger: Duration,
    /// Released torrents are dropped from the session after this long idle
    /// (files stay on disk in the cache dir).
    pub idle_ttl: Duration,
    /// Prefetched torrents that never started playing are dropped after this.
    pub prefetch_ttl: Duration,
    /// At most this many prefetches at once (oldest unplayed one is dropped).
    pub max_prefetches: usize,
    /// At most this many idle torrents kept in the session (LRU beyond that).
    pub max_cached_torrents: usize,
}

impl EngineConfig {
    pub fn new(download_dir: impl Into<PathBuf>) -> Self {
        Self {
            download_dir: download_dir.into(),
            trackers: DEFAULT_TRACKERS.iter().map(|s| s.to_string()).collect(),
            enable_dht: true,
            enable_listen: true,
            enable_upnp: true,
            persist_session: true,
            session_dir: None,
            upload_limit_bps: Some(512 * 1024),
            peer_limit: Some(DEFAULT_PEER_LIMIT),
            metadata_timeout: Duration::from_secs(120),
            linger: Duration::from_secs(90),
            idle_ttl: Duration::from_secs(30 * 60),
            prefetch_ttl: Duration::from_secs(3 * 60),
            max_prefetches: 3,
            max_cached_torrents: 12,
        }
    }

    fn session_dir(&self) -> PathBuf {
        self.session_dir.clone().unwrap_or_else(|| self.download_dir.join(".mv-session"))
    }
}

/// What to add.
#[derive(Debug, Clone)]
pub enum Source {
    /// `magnet:?xt=urn:btih:...`
    Magnet(String),
    /// Bare 40-char hex info-hash.
    InfoHash(String),
    /// Raw `.torrent` file contents.
    TorrentBytes(Vec<u8>),
}

impl Source {
    /// Classify a magnet URI or bare info-hash string.
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        if s.starts_with("magnet:") {
            Ok(Source::Magnet(s.to_string()))
        } else if s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit()) {
            Ok(Source::InfoHash(s.to_ascii_lowercase()))
        } else {
            bail!("not a magnet URI or 40-char info-hash")
        }
    }

    /// Lower-case hex info-hash.
    pub fn info_hash(&self) -> Result<String> {
        match self {
            Source::Magnet(m) => extract_info_hash(m),
            Source::InfoHash(h) => extract_info_hash(h),
            Source::TorrentBytes(b) => Ok(librqbit::torrent_from_bytes(b)
                .map_err(|e| anyhow!("bad .torrent: {e}"))?
                .info_hash
                .as_string()),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct AddOptions {
    /// File to stream; the largest video (else largest file) when `None`.
    pub file_idx: Option<usize>,
    /// Extra trackers for this torrent only.
    pub extra_trackers: Vec<String>,
    /// Override the engine download dir for this torrent.
    pub output_dir: Option<PathBuf>,
    /// Exempt from idle eviction (background downloads).
    pub keep: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub idx: usize,
    pub path: String,
    pub len: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddedTorrent {
    pub info_hash: String,
    pub name: Option<String>,
    pub file_idx: usize,
    pub file_name: String,
    pub file_len: u64,
    pub files: Vec<FileEntry>,
    pub stream_url: String,
}

/// Mirrors the renderer's `TorrentStatus` (camelCase over IPC). Progress and
/// totals refer to the selected file.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentStats {
    pub download_speed: f64,
    pub upload_speed: f64,
    pub peers: u32,
    /// 0.0..=1.0
    pub progress: f64,
    pub ready: bool,
    pub stream_url: Option<String>,
    pub downloaded: u64,
    pub total: u64,
    pub finished: bool,
    pub paused: bool,
    pub state: String,
    pub error: Option<String>,
}

struct Entry {
    handle: Arc<ManagedTorrent>,
    file_idx: AtomicUsize,
    files: Vec<FileEntry>,
    keep: AtomicBool,
    /// `add()` was called (vs. prefetch only).
    played: AtomicBool,
    active_streams: AtomicUsize,
    last_active: Mutex<Instant>,
    /// Container-index warm-up streams (high priority first/last pieces).
    warmers: Mutex<Vec<Warmer>>,
}

struct Warmer {
    stream: FileStream,
    file_idx: usize,
    start: u64,
    len: u64,
    head: bool,
    created: Instant,
}

impl Entry {
    fn touch(&self) {
        *self.last_active.lock().unwrap() = Instant::now();
    }
    fn idle_for(&self) -> Duration {
        self.last_active.lock().unwrap().elapsed()
    }
    fn is_idle(&self) -> bool {
        !self.keep.load(Ordering::Relaxed) && self.active_streams.load(Ordering::Relaxed) == 0
    }
    fn file_idx(&self) -> usize {
        self.file_idx.load(Ordering::Relaxed)
    }
}

type PendingAdd = Shared<BoxFuture<'static, Result<Arc<Entry>, String>>>;

struct Registry {
    cfg: EngineConfig,
    session: Arc<Session>,
    runtime: tokio::runtime::Handle,
    entries: Mutex<HashMap<String, Arc<Entry>>>,
    pending: Mutex<HashMap<String, PendingAdd>>,
}

pub struct Engine {
    cfg: EngineConfig,
    registry: Arc<Registry>,
    server: StreamServer,
}

/// Lower-case hex info-hash from a magnet URI or a bare hash.
pub fn extract_info_hash(s: &str) -> Result<String> {
    let s = s.trim();
    let id = if s.starts_with("magnet:") {
        Magnet::parse(s)?.as_id20().context("magnet has no BTv1 info-hash")?
    } else {
        Id20::from_str(s).map_err(|e| anyhow!("bad info-hash: {e}"))?
    };
    Ok(id.as_string())
}

/// Pick the file to stream: explicit index if valid, else the largest video,
/// else the largest file. Input is `(path, len)` per file.
pub fn pick_file(files: &[(String, u64)], requested: Option<usize>) -> Option<usize> {
    if let Some(i) = requested {
        if i < files.len() {
            return Some(i);
        }
    }
    let largest = |it: &mut dyn Iterator<Item = usize>| it.max_by_key(|&i| files[i].1);
    let is_video = |n: &str| {
        let l = n.to_ascii_lowercase();
        VIDEO_EXTS.iter().any(|e| l.ends_with(e))
    };
    largest(&mut (0..files.len()).filter(|&i| is_video(&files[i].0)))
        .or_else(|| largest(&mut (0..files.len())))
}

/// Tail bytes warmed for the container index (MP4 `moov` at the end, MKV
/// Cues): ~0.2 % of the file, clamped to [2 MiB, 8 MiB].
fn tail_warm_bytes(file_len: u64) -> u64 {
    (file_len / 500).clamp(2 * 1024 * 1024, 8 * 1024 * 1024).min(file_len)
}

fn listen_port_file(dir: &Path) -> PathBuf {
    dir.join("listen-port")
}

impl Engine {
    pub async fn new(cfg: EngineConfig) -> Result<Self> {
        std::fs::create_dir_all(&cfg.download_dir)
            .with_context(|| format!("creating {}", cfg.download_dir.display()))?;
        let session_dir = cfg.session_dir();
        if cfg.persist_session {
            std::fs::create_dir_all(&session_dir)
                .with_context(|| format!("creating {}", session_dir.display()))?;
        }

        // Re-use the last listen port so UPnP mappings and peers' caches stay
        // valid across restarts; fall back to a random one if it's taken.
        let saved_port: Option<u16> = cfg
            .persist_session
            .then(|| std::fs::read_to_string(listen_port_file(&session_dir)).ok())
            .flatten()
            .and_then(|s| s.trim().parse().ok());
        let session = match Self::start_session(&cfg, &session_dir, saved_port.unwrap_or(0)).await {
            Ok(s) => s,
            Err(e) if saved_port.is_some() => {
                tracing::warn!("listen port {saved_port:?} unavailable ({e:#}); using a random one");
                Self::start_session(&cfg, &session_dir, 0).await?
            }
            Err(e) => return Err(e),
        };
        if cfg.persist_session {
            if let Some(port) = session.listen_addr().map(|a| a.port()) {
                let _ = std::fs::write(listen_port_file(&session_dir), port.to_string());
            }
        }

        let registry = Arc::new(Registry {
            cfg: cfg.clone(),
            session,
            runtime: tokio::runtime::Handle::current(),
            entries: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
        });
        registry.adopt_restored().await;
        let server = StreamServer::start(registry.clone()).await?;

        // Housekeeping; exits once the engine (registry) is dropped.
        let weak: Weak<Registry> = Arc::downgrade(&registry);
        let period = (cfg.linger.min(cfg.idle_ttl) / 4).clamp(Duration::from_millis(200), Duration::from_secs(5));
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(period).await;
                let Some(reg) = weak.upgrade() else { break };
                reg.housekeeping().await;
            }
        });

        Ok(Self { cfg, registry, server })
    }

    async fn start_session(cfg: &EngineConfig, session_dir: &Path, port: u16) -> Result<Arc<Session>> {
        let trackers = cfg.trackers.iter().filter_map(|t| url::Url::parse(t).ok()).collect();
        let opts = SessionOptions {
            dht: cfg.enable_dht.then(|| DhtSessionConfig {
                persistence: cfg.persist_session.then(|| DhtPersistenceConfig {
                    dump_interval: Some(Duration::from_secs(60)),
                    config_filename: Some(session_dir.join("dht.json")),
                }),
                ..Default::default()
            }),
            persistence: cfg
                .persist_session
                .then(|| SessionPersistenceConfig::Json { folder: Some(session_dir.to_path_buf()) }),
            fastresume: cfg.persist_session,
            listen: cfg.enable_listen.then(|| ListenerOptions {
                mode: ListenerMode::TcpAndUtp,
                listen_addr: SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)),
                enable_upnp_port_forwarding: cfg.enable_upnp,
                ..Default::default()
            }),
            connect: Some(ConnectionOptions {
                peer_opts: Some(PeerConnectionOptions {
                    // Dead peers fail fast so the queue moves to live ones.
                    connect_timeout: Some(Duration::from_secs(4)),
                    read_write_timeout: Some(Duration::from_secs(20)),
                    keep_alive_interval: None,
                }),
                ..Default::default()
            }),
            trackers,
            peer_limit: cfg.peer_limit,
            // Upstream default 8 is shared by disk IO and torrent adds; scaled down (not below 8)
            // by the system resource limit since piece hashing runs on these threads.
            runtime_worker_threads: Some((32 * mv_core::limits::percent() as usize / 100).max(8)),
            ratelimits: librqbit::limits::LimitsConfig {
                upload_bps: cfg.upload_limit_bps.and_then(std::num::NonZeroU32::new),
                download_bps: None,
            },
            ..Default::default()
        };
        Session::new_with_opts(cfg.download_dir.clone(), opts).await
    }

    pub fn config(&self) -> &EngineConfig {
        &self.cfg
    }

    pub fn download_dir(&self) -> &Path {
        &self.cfg.download_dir
    }

    /// Base URL of the local HTTP server, e.g. `http://127.0.0.1:53211`.
    pub fn server_base_url(&self) -> String {
        self.server.base_url()
    }

    /// Add a torrent (or re-use it if already managed / prefetched), wait for
    /// metadata, select the file to stream and return its stream URL. Other
    /// idle torrents (prefetches, lingering ones) are paused so playback gets
    /// the whole downlink.
    pub async fn add(&self, source: Source, opts: AddOptions) -> Result<AddedTorrent> {
        let entry = self.registry.clone().ensure(source, opts, true).await?;
        let hash = entry.handle.info_hash().as_string();
        self.registry.pause_others(&hash).await;
        let idx = entry.file_idx();
        Ok(AddedTorrent {
            stream_url: self.url_for(&hash, idx),
            name: entry.handle.name(),
            file_name: entry.files[idx].path.clone(),
            file_len: entry.files[idx].len,
            info_hash: hash,
            file_idx: idx,
            files: entry.files.clone(),
        })
    }

    /// Warm a source in the background (Stremio-style instant start): add the
    /// torrent, fetch metadata, then the container index (first / last pieces
    /// of the chosen file) and keep downloading from the start. A later
    /// [`Engine::add`] with the same info-hash re-uses it (even mid-metadata).
    /// Cheap to call repeatedly; unplayed prefetches are dropped after
    /// `prefetch_ttl`, when more than `max_prefetches` are warm, or via
    /// [`Engine::cancel_prefetch`]. Returns the info-hash. Callable from any
    /// thread (spawns on the engine's runtime).
    pub fn prefetch(&self, source: Source, opts: AddOptions) -> Result<String> {
        let hash = source.info_hash()?;
        if let Some(e) = self.registry.entries.lock().unwrap().get(&hash).cloned() {
            e.touch();
            return Ok(hash);
        }
        if self.registry.pending.lock().unwrap().contains_key(&hash) {
            return Ok(hash);
        }
        let reg = self.registry.clone();
        let h = hash.clone();
        self.registry.runtime.spawn(async move {
            reg.limit_prefetches(&h).await;
            if let Err(e) = reg.clone().ensure(source, opts, false).await {
                tracing::debug!("prefetch {h} failed: {e:#}");
            }
        });
        Ok(hash)
    }

    /// Drop a prefetched torrent that never started playing (files stay in
    /// the cache). No-op for torrents being played or kept.
    pub async fn cancel_prefetch(&self, info_hash: &str) -> Result<()> {
        let key = extract_info_hash(info_hash)?;
        let pending = self.registry.pending.lock().unwrap().get(&key).cloned();
        if let Some(p) = pending {
            let _ = p.await;
        }
        let e = self.registry.entries.lock().unwrap().get(&key).cloned();
        if let Some(e) = e {
            if !e.played.load(Ordering::Relaxed) && e.is_idle() {
                self.registry.evict(&key).await;
            }
        }
        Ok(())
    }

    fn url_for(&self, hash: &str, idx: usize) -> String {
        format!("{}/stream/{hash}/{idx}", self.server.base_url())
    }

    /// Stream URL for an added torrent (selected file unless `file_idx` given).
    pub fn stream_url(&self, info_hash: &str, file_idx: Option<usize>) -> Result<String> {
        let e = self.entry(info_hash)?;
        Ok(self.url_for(&e.handle.info_hash().as_string(), file_idx.unwrap_or(e.file_idx())))
    }

    /// URL that proxies a plain HTTP(S) stream through the local server with
    /// Range forwarding (replaces the TS direct-stream server).
    pub fn direct_url(&self, target: &str) -> String {
        url::Url::parse_with_params(&format!("{}/direct", self.server.base_url()), [("url", target)])
            .map(String::from)
            .unwrap_or_default()
    }

    fn entry(&self, info_hash: &str) -> Result<Arc<Entry>> {
        let key = extract_info_hash(info_hash)?;
        self.registry
            .entries
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or_else(|| anyhow!("torrent {key} is not active"))
    }

    pub fn stats(&self, info_hash: &str) -> Result<TorrentStats> {
        let e = self.entry(info_hash)?;
        let s = e.handle.stats();
        let idx = e.file_idx();
        let total = e.files[idx].len;
        let downloaded = s.file_progress.get(idx).copied().unwrap_or(0).min(total);
        const MIB: f64 = 1024.0 * 1024.0;
        let (down, up, peers) = match &s.live {
            Some(l) => (l.download_speed.mbps * MIB, l.upload_speed.mbps * MIB, l.snapshot.peer_stats.live),
            None => (0.0, 0.0, 0),
        };
        Ok(TorrentStats {
            download_speed: down,
            upload_speed: up,
            peers,
            progress: if total == 0 { 0.0 } else { downloaded as f64 / total as f64 },
            ready: true,
            stream_url: Some(self.url_for(&e.handle.info_hash().as_string(), idx)),
            downloaded,
            total,
            finished: downloaded >= total,
            paused: e.handle.is_paused(),
            state: s.state.to_string(),
            error: s.error,
        })
    }

    /// Info-hashes currently managed by the engine (playing, lingering,
    /// prefetched).
    pub fn active(&self) -> Vec<String> {
        self.registry.entries.lock().unwrap().keys().cloned().collect()
    }

    pub async fn pause(&self, info_hash: &str) -> Result<()> {
        let e = self.entry(info_hash)?;
        self.registry.session.pause(&e.handle).await
    }

    pub async fn resume(&self, info_hash: &str) -> Result<()> {
        let e = self.entry(info_hash)?;
        e.touch();
        self.registry.session.unpause(&e.handle).await
    }

    /// Release a torrent.
    ///
    /// `delete_files == false` (what the player does on exit): the torrent is
    /// *released*, not dropped: it keeps downloading for `linger`, is then
    /// paused, and stays in the session for `idle_ttl` so replaying or
    /// seeking back is instant (no metadata fetch, no re-check). `stats()`
    /// keeps working meanwhile. Use [`Engine::evict`] to drop it right away.
    ///
    /// `delete_files == true`: drop it now and delete its files.
    pub async fn remove(&self, info_hash: &str, delete_files: bool) -> Result<()> {
        let key = extract_info_hash(info_hash)?;
        if !delete_files {
            if let Some(e) = self.registry.entries.lock().unwrap().get(&key) {
                e.played.store(true, Ordering::Relaxed);
                e.warmers.lock().unwrap().clear();
                e.touch();
            }
            return Ok(());
        }
        let removed = self.registry.entries.lock().unwrap().remove(&key);
        if removed.is_none() {
            return Ok(());
        }
        self.registry
            .session
            .delete(TorrentIdOrHash::Hash(Id20::from_str(&key).map_err(|e| anyhow!("{e}"))?), true)
            .await
    }

    /// Drop a torrent from the session right away (files stay on disk).
    pub async fn evict(&self, info_hash: &str) -> Result<()> {
        let key = extract_info_hash(info_hash)?;
        self.registry.evict(&key).await;
        Ok(())
    }

    /// Run the idle policy now (pause lingering, evict expired). Returns how
    /// many torrents were dropped from the session.
    pub async fn cleanup_idle(&self) -> usize {
        self.registry.housekeeping().await
    }

    /// Stop the HTTP server and the librqbit session.
    pub async fn shutdown(self) {
        let entries: Vec<Arc<Entry>> = self.registry.entries.lock().unwrap().drain().map(|(_, e)| e).collect();
        // Persist idle torrents as paused so the next start doesn't resume them.
        for e in entries {
            e.warmers.lock().unwrap().clear();
            if e.is_idle() && !e.handle.is_paused() {
                let _ = self.registry.session.pause(&e.handle).await;
            }
        }
        self.registry.session.stop().await;
    }
}

impl Registry {
    /// Find or add the torrent and make sure its file is selected.
    fn ensure(self: Arc<Self>, source: Source, opts: AddOptions, play: bool) -> BoxFuture<'static, Result<Arc<Entry>>> {
        async move {
            let hash = source.info_hash()?;
            loop {
                let existing = self.entries.lock().unwrap().get(&hash).cloned();
                if let Some(e) = existing {
                    self.reuse(&e, &opts, play).await;
                    return Ok(e);
                }
                let pending = self.pending.lock().unwrap().get(&hash).cloned();
                if let Some(p) = pending {
                    // Join the in-flight add (e.g. a prefetch), then re-use it.
                    p.await.map_err(|e| anyhow!(e))?;
                    continue;
                }
                let fut: PendingAdd = {
                    let reg = self.clone();
                    let (source, opts, hash) = (source.clone(), opts.clone(), hash.clone());
                    async move {
                        let res = reg.clone().add_new(source, opts, play).await.map_err(|e| format!("{e:#}"));
                        reg.pending.lock().unwrap().remove(&hash);
                        res
                    }
                    .boxed()
                    .shared()
                };
                self.pending.lock().unwrap().insert(hash.clone(), fut.clone());
                // Run it on the runtime so a dropped caller can't cancel a
                // shared add half-way.
                let res = self.runtime.spawn(fut).await.map_err(|e| anyhow!("add task: {e}"))?;
                let e = res.map_err(|e| anyhow!(e))?;
                if play {
                    self.reuse(&e, &opts, play).await;
                }
                return Ok(e);
            }
        }
        .boxed()
    }

    async fn reuse(&self, e: &Arc<Entry>, opts: &AddOptions, play: bool) {
        e.touch();
        if play {
            e.played.store(true, Ordering::Relaxed);
        }
        if opts.keep {
            e.keep.store(true, Ordering::Relaxed);
        }
        if e.handle.is_paused() {
            if let Err(err) = self.session.unpause(&e.handle).await {
                tracing::warn!("unpause failed: {err:#}");
            }
        }
        if let Some(want) = opts.file_idx.filter(|&i| i < e.files.len()) {
            if want != e.file_idx() {
                e.file_idx.store(want, Ordering::Relaxed);
                self.select_files(e, want).await;
                self.start_warmers(e, want);
            }
        }
    }

    async fn select_files(&self, e: &Entry, idx: usize) {
        let mut set: HashSet<usize> = e.handle.only_files().unwrap_or_default().into_iter().collect();
        if set.len() == 1 && set.contains(&idx) {
            return;
        }
        // Keep only the streamed file (skip samples / extras).
        set.clear();
        set.insert(idx);
        if let Err(err) = self.session.update_only_files(&e.handle, &set).await {
            tracing::warn!("could not restrict torrent to file {idx}: {err:#}");
        }
    }

    async fn add_new(self: Arc<Self>, source: Source, opts: AddOptions, play: bool) -> Result<Arc<Entry>> {
        let hash = source.info_hash()?;
        // "Clear cache" may have wiped these while we run.
        let _ = std::fs::create_dir_all(&self.cfg.download_dir);
        if self.cfg.persist_session {
            let _ = std::fs::create_dir_all(self.cfg.session_dir());
        }

        let handle = match self.session.get(TorrentIdOrHash::Hash(Id20::from_str(&hash).map_err(|e| anyhow!("{e}"))?)) {
            Some(h) => h,
            None => {
                let to_add = |src: &Source| match src {
                    Source::Magnet(m) => AddTorrent::from_url(m.clone()),
                    Source::InfoHash(h) => AddTorrent::from_url(h.clone()),
                    Source::TorrentBytes(b) => AddTorrent::from_bytes(b.clone()),
                };
                let make_opts = |only: Option<Vec<usize>>| AddTorrentOptions {
                    overwrite: true,
                    output_folder: opts.output_dir.as_ref().map(|p| p.to_string_lossy().into_owned()),
                    trackers: (!opts.extra_trackers.is_empty()).then(|| opts.extra_trackers.clone()),
                    // Select the requested file up front so nothing else is
                    // fetched while we pick (addon sources carry fileIdx).
                    only_files: only,
                    ..Default::default()
                };
                let add = async {
                    let first = self.session.add_torrent(to_add(&source), Some(make_opts(opts.file_idx.map(|i| vec![i])))).await;
                    match first {
                        Err(e) if opts.file_idx.is_some() && format!("{e:#}").contains("out of range") => {
                            self.session.add_torrent(to_add(&source), Some(make_opts(None))).await
                        }
                        other => other,
                    }
                };
                let res = tokio::time::timeout(self.cfg.metadata_timeout, add)
                    .await
                    .map_err(|_| anyhow!("torrent metadata timeout after {:?}", self.cfg.metadata_timeout))??;
                match res {
                    AddTorrentResponse::Added(_, h) | AddTorrentResponse::AlreadyManaged(_, h) => h,
                    AddTorrentResponse::ListOnly(_) => bail!("unexpected list-only response"),
                }
            }
        };
        handle.wait_until_initialized().await?;
        if handle.is_paused() {
            self.session.unpause(&handle).await?;
        }

        let files = files_of(&handle)?;
        if files.is_empty() {
            bail!("Torrent has no files");
        }
        let pairs: Vec<(String, u64)> = files.iter().map(|f| (f.path.clone(), f.len)).collect();
        let file_idx = pick_file(&pairs, opts.file_idx).context("No video file found in torrent")?;

        let entry = Arc::new(Entry {
            handle: handle.clone(),
            file_idx: AtomicUsize::new(file_idx),
            files,
            keep: AtomicBool::new(opts.keep),
            played: AtomicBool::new(play),
            active_streams: AtomicUsize::new(0),
            last_active: Mutex::new(Instant::now()),
            warmers: Mutex::new(Vec::new()),
        });
        self.select_files(&entry, file_idx).await;
        self.start_warmers(&entry, file_idx);
        self.entries.lock().unwrap().insert(hash, entry.clone());
        Ok(entry)
    }

    /// Open high-priority warm-up streams over the head and tail of the file
    /// so the demuxer finds its index (moov / Cues) immediately and early
    /// seeks don't wait. They are dropped when done (see `housekeeping`).
    fn start_warmers(&self, e: &Arc<Entry>, file_idx: usize) {
        let len = e.files[file_idx].len;
        if len == 0 || e.handle.with_metadata(|_| ()).is_err() {
            return;
        }
        let e = e.clone();
        self.runtime.spawn(async move {
            let tail = tail_warm_bytes(len);
            let specs = [(0u64, HEAD_WARM_BYTES.min(len), true), (len - tail, tail, false)];
            for (start, wlen, head) in specs {
                let Ok(mut s) = e.handle.clone().stream(file_idx).await else { return };
                if s.is_range_available(start, wlen) {
                    continue;
                }
                s.set_priority(wlen, false, true);
                if start > 0 && s.seek(SeekFrom::Start(start)).await.is_err() {
                    continue;
                }
                e.warmers.lock().unwrap().push(Warmer { stream: s, file_idx, start, len: wlen, head, created: Instant::now() });
            }
        });
    }

    /// Torrents restored from the fastresume store: track them as idle
    /// cache entries (paused; resumed on demand).
    async fn adopt_restored(&self) {
        let handles: Vec<Arc<ManagedTorrent>> =
            self.session.with_torrents(|it| it.map(|(_, h)| h.clone()).collect());
        for h in handles {
            let _ = tokio::time::timeout(Duration::from_secs(5), h.wait_until_initialized()).await;
            let Ok(files) = files_of(&h) else { continue };
            if files.is_empty() {
                continue;
            }
            let pairs: Vec<(String, u64)> = files.iter().map(|f| (f.path.clone(), f.len)).collect();
            let only = h.only_files().and_then(|o| o.first().copied());
            let Some(idx) = pick_file(&pairs, only) else { continue };
            if !h.is_paused() {
                let _ = self.session.pause(&h).await;
            }
            let hash = h.info_hash().as_string();
            self.entries.lock().unwrap().insert(
                hash,
                Arc::new(Entry {
                    handle: h,
                    file_idx: AtomicUsize::new(idx),
                    files,
                    keep: AtomicBool::new(false),
                    played: AtomicBool::new(true),
                    active_streams: AtomicUsize::new(0),
                    last_active: Mutex::new(Instant::now()),
                    warmers: Mutex::new(Vec::new()),
                }),
            );
        }
    }

    /// Playback focus: pause every other idle torrent (prefetches, lingering).
    async fn pause_others(&self, keep_hash: &str) {
        let others: Vec<Arc<Entry>> = self
            .entries
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, e)| k.as_str() != keep_hash && e.is_idle())
            .map(|(_, e)| e.clone())
            .collect();
        for e in others {
            e.warmers.lock().unwrap().clear();
            if !e.handle.is_paused() {
                let _ = self.session.pause(&e.handle).await;
            }
        }
    }

    /// Keep at most `max_prefetches` unplayed prefetches (drop the oldest).
    async fn limit_prefetches(&self, incoming: &str) {
        let mut pre: Vec<(String, Duration)> = self
            .entries
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, e)| k.as_str() != incoming && !e.played.load(Ordering::Relaxed) && e.is_idle())
            .map(|(k, e)| (k.clone(), e.idle_for()))
            .collect();
        let max = self.cfg.max_prefetches.max(1);
        if pre.len() < max {
            return;
        }
        pre.sort_by(|a, b| b.1.cmp(&a.1));
        let excess = pre.len() + 1 - max;
        for (k, _) in pre.into_iter().take(excess) {
            self.evict(&k).await;
        }
    }

    async fn evict(&self, key: &str) {
        let removed = self.entries.lock().unwrap().remove(key);
        if let Some(e) = removed {
            e.warmers.lock().unwrap().clear();
            if self.session.delete(TorrentIdOrHash::Hash(e.handle.info_hash()), false).await.is_ok() {
                tracing::info!("evicted torrent {key}");
            }
        }
    }

    /// Idle policy + warm-up stream cleanup. Returns evicted count.
    async fn housekeeping(&self) -> usize {
        let all: Vec<(String, Arc<Entry>)> =
            self.entries.lock().unwrap().iter().map(|(k, e)| (k.clone(), e.clone())).collect();
        let mut evict = Vec::new();
        let mut idle_cached = Vec::new();
        for (k, e) in &all {
            // Drop finished / stale warmers.
            e.warmers.lock().unwrap().retain(|w| {
                w.created.elapsed() < WARMER_TTL && !w.stream.is_range_available(w.start, w.len)
            });
            if !e.is_idle() {
                continue;
            }
            let idle = e.idle_for();
            let played = e.played.load(Ordering::Relaxed);
            if (!played && idle >= self.cfg.prefetch_ttl) || idle >= self.cfg.idle_ttl {
                evict.push(k.clone());
                continue;
            }
            if played && idle >= self.cfg.linger && !e.handle.is_paused() {
                e.warmers.lock().unwrap().clear();
                let _ = self.session.pause(&e.handle).await;
            }
            idle_cached.push((k.clone(), idle));
        }
        if idle_cached.len() > self.cfg.max_cached_torrents {
            idle_cached.sort_by(|a, b| b.1.cmp(&a.1));
            let excess = idle_cached.len() - self.cfg.max_cached_torrents;
            evict.extend(idle_cached.into_iter().take(excess).map(|(k, _)| k));
        }
        let n = evict.len();
        for k in evict {
            self.evict(&k).await;
        }
        n
    }
}

fn files_of(handle: &ManagedTorrent) -> Result<Vec<FileEntry>> {
    handle.with_metadata(|m| {
        m.file_infos
            .iter()
            .enumerate()
            .map(|(idx, f)| FileEntry {
                idx,
                path: f.relative_filename.to_string_lossy().replace('\\', "/"),
                len: f.len,
            })
            .collect()
    })
}

// ---- HTTP glue ------------------------------------------------------------

impl SourceResolver for Registry {
    fn resolve(&self, info_hash: String, file_idx: usize) -> BoxFuture<'static, Option<Arc<dyn ByteSource>>> {
        let entry = self.entries.lock().unwrap().get(&info_hash).cloned();
        let session = self.session.clone();
        Box::pin(async move {
            let entry = entry?;
            let file = entry.files.get(file_idx)?.clone();
            entry.touch();
            if entry.handle.is_paused() {
                let _ = session.unpause(&entry.handle).await;
            }
            Some(Arc::new(TorrentFileSource {
                content_type: mime_from_name(&file.path).to_string(),
                len: file.len,
                file_idx,
                entry,
            }) as Arc<dyn ByteSource>)
        })
    }
}

struct TorrentFileSource {
    entry: Arc<Entry>,
    file_idx: usize,
    len: u64,
    content_type: String,
}

impl ByteSource for TorrentFileSource {
    fn len(&self) -> u64 {
        self.len
    }
    fn content_type(&self) -> String {
        self.content_type.clone()
    }
    fn open(&self, start: u64) -> BoxFuture<'static, io::Result<BoxReader>> {
        let entry = self.entry.clone();
        let idx = self.file_idx;
        Box::pin(async move {
            // Guard first so the torrent counts as busy while the stream opens.
            let guard = StreamGuard::new(entry.clone());
            // The player is reading the head itself now; its warmer would only
            // compete with the real read position.
            entry.warmers.lock().unwrap().retain(|w| !(w.head && w.file_idx == idx));
            let mut stream = entry.handle.clone().stream(idx).await.map_err(io::Error::other)?;
            if start > 0 {
                // A seek: re-prioritises around `start` and pre-empts the old
                // window inside librqbit.
                stream.seek(SeekFrom::Start(start)).await?;
            }
            Ok(Box::pin(GuardedReader { inner: stream, _guard: guard }) as BoxReader)
        })
    }
}

/// Counts an open HTTP stream against its torrent so idle eviction skips it.
struct StreamGuard(Arc<Entry>);

impl StreamGuard {
    fn new(e: Arc<Entry>) -> Self {
        e.active_streams.fetch_add(1, Ordering::Relaxed);
        e.touch();
        Self(e)
    }
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        self.0.active_streams.fetch_sub(1, Ordering::Relaxed);
        self.0.touch();
    }
}

struct GuardedReader<R> {
    inner: R,
    _guard: StreamGuard,
}

impl<R: AsyncRead + Unpin> AsyncRead for GuardedReader<R> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(n: &str, l: u64) -> (String, u64) {
        (n.to_string(), l)
    }

    #[test]
    fn picks_files() {
        let files = vec![f("a/sample.mkv", 10), f("a/ep.mkv", 900), f("a/cover.jpg", 5000), f("a/ep.nfo", 1)];
        assert_eq!(pick_file(&files, None), Some(1)); // largest video, not largest file
        assert_eq!(pick_file(&files, Some(3)), Some(3)); // explicit wins
        assert_eq!(pick_file(&files, Some(99)), Some(1)); // invalid index falls back
        let no_video = vec![f("x.bin", 5), f("y.dat", 50)];
        assert_eq!(pick_file(&no_video, None), Some(1));
        assert_eq!(pick_file(&[], None), None);
    }

    #[test]
    fn info_hash_extraction() {
        let h = "dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c";
        assert_eq!(extract_info_hash(h).unwrap(), h);
        assert_eq!(extract_info_hash(&h.to_uppercase()).unwrap(), h);
        assert_eq!(extract_info_hash(&format!("magnet:?xt=urn:btih:{}&dn=x", h.to_uppercase())).unwrap(), h);
        assert!(extract_info_hash("nope").is_err());
        assert!(matches!(Source::parse(h).unwrap(), Source::InfoHash(_)));
        assert!(matches!(Source::parse("magnet:?xt=urn:btih:abc").unwrap(), Source::Magnet(_)));
        assert!(Source::parse("http://x").is_err());
        assert_eq!(Source::parse(h).unwrap().info_hash().unwrap(), h);
    }

    #[test]
    fn warm_sizes() {
        assert_eq!(tail_warm_bytes(100), 100);
        assert_eq!(tail_warm_bytes(100 << 20), 2 << 20);
        assert_eq!(tail_warm_bytes(2 << 30), (2u64 << 30) / 500);
        assert_eq!(tail_warm_bytes(40 << 30), 8 << 20);
    }

    fn offline_cfg(dir: &Path) -> EngineConfig {
        let mut cfg = EngineConfig::new(dir.join("dl"));
        cfg.enable_dht = false;
        cfg.enable_listen = false;
        cfg.enable_upnp = false;
        cfg.trackers.clear();
        cfg
    }

    async fn local_torrent(dir: &Path, name: &str, len: usize) -> (PathBuf, Vec<u8>, Vec<u8>, String) {
        let seed = dir.join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        let data: Vec<u8> = (0..len).map(|i| (i * 7 % 251) as u8).collect();
        let file = seed.join(name);
        std::fs::write(&file, &data).unwrap();
        let spawner = librqbit::spawn_utils::BlockingSpawner::new(2);
        let meta = librqbit::create_torrent(
            &file,
            librqbit::CreateTorrentOptions { name: None, trackers: vec![], piece_length: Some(16384) },
            &spawner,
        )
        .await
        .unwrap();
        let bytes = meta.as_bytes().unwrap().to_vec();
        (seed, data, bytes, meta.info_hash().as_string())
    }

    /// Build a .torrent from a local file, add it pointing at the existing
    /// data (verified, so complete), and stream it through the HTTP server.
    #[tokio::test(flavor = "multi_thread")]
    async fn streams_local_torrent_with_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let (seed, data, bytes, hash) = local_torrent(dir.path(), "episode.mkv", 300_000).await;

        let engine = Engine::new(offline_cfg(dir.path())).await.unwrap();
        let added = engine
            .add(Source::TorrentBytes(bytes), AddOptions { output_dir: Some(seed), ..Default::default() })
            .await
            .unwrap();
        assert_eq!(added.file_idx, 0);
        assert_eq!(added.file_len, 300_000);
        assert_eq!(added.info_hash, hash);
        assert_eq!(engine.stream_url(&added.info_hash, None).unwrap(), added.stream_url);

        let c = reqwest::Client::new();
        let r = c.get(&added.stream_url).header("Range", "bytes=100000-100999").send().await.unwrap();
        assert_eq!(r.status(), 206);
        assert_eq!(r.headers()["content-range"], "bytes 100000-100999/300000");
        assert_eq!(r.headers()["content-type"], "video/webm");
        assert_eq!(r.bytes().await.unwrap().as_ref(), &data[100_000..101_000]);

        // mpv-style open-ended range, then a seek back.
        let r = c.get(&added.stream_url).header("Range", "bytes=299000-").send().await.unwrap();
        assert_eq!(r.status(), 206);
        assert_eq!(r.bytes().await.unwrap().as_ref(), &data[299_000..]);
        let r = c.head(&added.stream_url).send().await.unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(r.headers()["content-length"], "300000");
        assert_eq!(r.headers()["accept-ranges"], "bytes");

        let r = c.get(&added.stream_url).send().await.unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(r.bytes().await.unwrap().as_ref(), &data[..]);

        let st = engine.stats(&added.info_hash).unwrap();
        assert_eq!(st.total, 300_000);
        assert!(st.finished && (st.progress - 1.0).abs() < 1e-9);

        // Release keeps it (instant replay), evict drops it.
        engine.remove(&added.info_hash, false).await.unwrap();
        assert!(engine.stats(&added.info_hash).is_ok());
        assert_eq!(c.get(&added.stream_url).header("Range", "bytes=0-9").send().await.unwrap().status(), 206);
        engine.evict(&added.info_hash).await.unwrap();
        assert!(engine.stats(&added.info_hash).is_err());
        let r = c.get(&added.stream_url).send().await.unwrap();
        assert_eq!(r.status(), 404);
        engine.shutdown().await;
    }

    /// prefetch() then add() share one torrent; idle policy pauses / evicts.
    #[tokio::test(flavor = "multi_thread")]
    async fn prefetch_is_reused_and_expires() {
        let dir = tempfile::tempdir().unwrap();
        let (seed, data, bytes, hash) = local_torrent(dir.path(), "movie.mp4", 200_000).await;
        let mut cfg = offline_cfg(dir.path());
        cfg.prefetch_ttl = Duration::from_millis(300);
        cfg.linger = Duration::from_millis(200);
        cfg.idle_ttl = Duration::from_secs(600);
        let engine = Engine::new(cfg).await.unwrap();
        let opts = AddOptions { output_dir: Some(seed), ..Default::default() };

        let h = engine.prefetch(Source::TorrentBytes(bytes.clone()), opts.clone()).unwrap();
        assert_eq!(h, hash);
        // add() while the prefetch may still be running joins it.
        let added = engine.add(Source::TorrentBytes(bytes.clone()), opts.clone()).await.unwrap();
        assert_eq!(added.info_hash, hash);
        assert_eq!(engine.active(), vec![hash.clone()]);
        let body = reqwest::get(&added.stream_url).await.unwrap().bytes().await.unwrap();
        assert_eq!(body.as_ref(), &data[..]);

        // Played + released: lingers, then gets paused but stays cached.
        engine.remove(&hash, false).await.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        engine.cleanup_idle().await;
        let st = engine.stats(&hash).unwrap();
        assert!(st.paused, "released torrent should be paused after linger");
        // Re-adding resumes the cached torrent instantly.
        let again = engine.add(Source::TorrentBytes(bytes.clone()), opts.clone()).await.unwrap();
        assert_eq!(again.info_hash, hash);
        assert!(!engine.stats(&hash).unwrap().paused);
        engine.evict(&hash).await.unwrap();

        // An unplayed prefetch expires after prefetch_ttl.
        engine.prefetch(Source::TorrentBytes(bytes.clone()), opts.clone()).unwrap();
        for _ in 0..50 {
            if engine.active().contains(&hash) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(engine.active().contains(&hash));
        tokio::time::sleep(Duration::from_millis(400)).await;
        engine.cleanup_idle().await;
        assert!(engine.active().is_empty());

        // cancel_prefetch drops it immediately.
        engine.prefetch(Source::TorrentBytes(bytes), opts).unwrap();
        engine.cancel_prefetch(&hash).await.unwrap();
        for _ in 0..50 {
            if engine.active().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
            engine.cancel_prefetch(&hash).await.unwrap();
        }
        assert!(engine.active().is_empty());
        engine.shutdown().await;
    }

    /// Fastresume: a torrent kept in the session survives an engine restart
    /// without a full re-check and is adopted as a paused cache entry.
    #[tokio::test(flavor = "multi_thread")]
    async fn session_persists_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let (seed, _data, bytes, hash) = local_torrent(dir.path(), "ep.mkv", 100_000).await;
        let engine = Engine::new(offline_cfg(dir.path())).await.unwrap();
        engine
            .add(Source::TorrentBytes(bytes.clone()), AddOptions { output_dir: Some(seed.clone()), ..Default::default() })
            .await
            .unwrap();
        engine.remove(&hash, false).await.unwrap();
        engine.shutdown().await;

        let engine = Engine::new(offline_cfg(dir.path())).await.unwrap();
        assert_eq!(engine.active(), vec![hash.clone()]);
        let st = engine.stats(&hash).unwrap();
        assert!(st.paused && st.finished);
        engine.shutdown().await;
    }

    /// Live network test: Big Buck Bunny (public domain). Run with
    /// `cargo test -p mv-torrent -- --ignored live`. For timings use
    /// `cargo run -p mv-torrent --release --example bench`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn live_magnet_big_buck_bunny() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = EngineConfig::new(dir.path().join("dl"));
        cfg.metadata_timeout = Duration::from_secs(90);
        let engine = Engine::new(cfg).await.unwrap();
        let magnet = "magnet:?xt=urn:btih:dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c&dn=Big+Buck+Bunny&tr=udp%3A%2F%2Fexplodie.org%3A6969&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337";
        let added = engine.add(Source::parse(magnet).unwrap(), AddOptions::default()).await.unwrap();
        assert!(added.file_len > 100_000_000);
        // Read 256 KiB from the middle of the file while it is downloading.
        let mid = added.file_len / 2;
        let r = reqwest::Client::new()
            .get(&added.stream_url)
            .header("Range", format!("bytes={}-{}", mid, mid + 262_143))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 206);
        assert_eq!(r.bytes().await.unwrap().len(), 262_144);
        let st = engine.stats(&added.info_hash).unwrap();
        println!("stats: {st:?}");
        assert!(st.peers > 0);
        engine.remove(&added.info_hash, true).await.unwrap();
        engine.shutdown().await;
    }
}
