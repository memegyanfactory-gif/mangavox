//! librqbit-backed engine: add by magnet / info-hash / .torrent bytes, pick a
//! file, stream it over the local HTTP server, report stats, pause / remove,
//! and evict idle torrents.

use std::collections::{HashMap, HashSet};
use std::io::{self, SeekFrom};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context as _, Result};
use futures::future::BoxFuture;
use librqbit::api::TorrentIdOrHash;
use librqbit::dht::Id20;
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, DhtSessionConfig, ListenerMode,
    ListenerOptions, Magnet, ManagedTorrent, Session, SessionOptions,
};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncSeekExt, ReadBuf};

use crate::http::{mime_from_name, BoxReader, ByteSource, SourceResolver, StreamServer};

/// Public trackers merged into every torrent (same list as the TS engine; wss
/// entries dropped because they are noise for a native client).
pub const DEFAULT_TRACKERS: &[&str] = &[
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://exodus.desync.com:6969/announce",
    "udp://open.demonii.com:1337/announce",
    "udp://tracker.openbittorrent.com:6969/announce",
    "udp://explodie.org:6969/announce",
    "udp://tracker.dler.org:6969/announce",
    "udp://opentracker.io:6969/announce",
    "udp://open.free-tracker.ga:6969/announce",
    "udp://tracker.bittor.pw:1337/announce",
    "udp://tracker-udp.gbitt.info:80/announce",
    "http://tracker.opentrackr.org:1337/announce",
    "https://tracker.gbitt.info:443/announce",
    "https://tracker.tamersunion.org:443/announce",
    "http://nyaa.tracker.wf:7777/announce",
    "https://tracker.yemekyedim.com:443/announce",
    "http://bt.endpot.com:80/announce",
];

const VIDEO_EXTS: &[&str] = &[
    ".mp4", ".mkv", ".avi", ".webm", ".m4v", ".ts", ".m2ts", ".mov", ".flv", ".wmv", ".mpg", ".mpeg",
];

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
    /// Upload cap in bytes/s so streaming keeps the downlink busy. `None` = unlimited.
    pub upload_limit_bps: Option<u32>,
    /// Max peers per torrent.
    pub peer_limit: Option<usize>,
    /// Give up on magnet metadata after this long.
    pub metadata_timeout: Duration,
    /// Torrents with no open streams are dropped from the session after this
    /// long (files stay on disk). Mirrors the TS 5 minute idle cache.
    pub idle_ttl: Duration,
}

impl EngineConfig {
    pub fn new(download_dir: impl Into<PathBuf>) -> Self {
        Self {
            download_dir: download_dir.into(),
            trackers: DEFAULT_TRACKERS.iter().map(|s| s.to_string()).collect(),
            enable_dht: true,
            enable_listen: true,
            upload_limit_bps: Some(512 * 1024),
            peer_limit: Some(300),
            metadata_timeout: Duration::from_secs(120),
            idle_ttl: Duration::from_secs(5 * 60),
        }
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
    file_idx: usize,
    files: Vec<FileEntry>,
    keep: bool,
    active_streams: AtomicUsize,
    last_active: Mutex<Instant>,
}

impl Entry {
    fn touch(&self) {
        *self.last_active.lock().unwrap() = Instant::now();
    }
}

struct Registry {
    session: Arc<Session>,
    entries: Mutex<HashMap<String, Arc<Entry>>>,
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

impl Engine {
    pub async fn new(cfg: EngineConfig) -> Result<Self> {
        std::fs::create_dir_all(&cfg.download_dir)
            .with_context(|| format!("creating {}", cfg.download_dir.display()))?;
        let trackers = cfg
            .trackers
            .iter()
            .filter_map(|t| url::Url::parse(t).ok())
            .collect();
        let opts = SessionOptions {
            dht: cfg.enable_dht.then(|| DhtSessionConfig { persistence: None, ..Default::default() }),
            listen: cfg.enable_listen.then(|| ListenerOptions {
                mode: ListenerMode::TcpAndUtp,
                ..Default::default()
            }),
            trackers,
            peer_limit: cfg.peer_limit,
            ratelimits: librqbit::limits::LimitsConfig {
                upload_bps: cfg.upload_limit_bps.and_then(std::num::NonZeroU32::new),
                download_bps: None,
            },
            ..Default::default()
        };
        let session = Session::new_with_opts(cfg.download_dir.clone(), opts).await?;
        let registry = Arc::new(Registry { session, entries: Mutex::new(HashMap::new()) });
        let server = StreamServer::start(registry.clone()).await?;

        // Idle sweeper; exits once the engine (registry) is dropped.
        let weak: Weak<Registry> = Arc::downgrade(&registry);
        let ttl = cfg.idle_ttl;
        let period = (ttl / 4).clamp(Duration::from_millis(200), Duration::from_secs(30));
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(period).await;
                let Some(reg) = weak.upgrade() else { break };
                reg.evict_idle(ttl).await;
            }
        });

        Ok(Self { cfg, registry, server })
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

    /// Add a torrent (or re-use it if already managed), wait for metadata,
    /// select the file to stream and return its stream URL.
    pub async fn add(&self, source: Source, opts: AddOptions) -> Result<AddedTorrent> {
        let to_add = match &source {
            Source::Magnet(m) => AddTorrent::from_url(m.clone()),
            Source::InfoHash(h) => AddTorrent::from_url(h.clone()),
            Source::TorrentBytes(b) => AddTorrent::from_bytes(b.clone()),
        };
        let add_opts = AddTorrentOptions {
            overwrite: true,
            output_folder: opts.output_dir.as_ref().map(|p| p.to_string_lossy().into_owned()),
            trackers: (!opts.extra_trackers.is_empty()).then(|| opts.extra_trackers.clone()),
            ..Default::default()
        };

        let res = tokio::time::timeout(
            self.cfg.metadata_timeout,
            self.registry.session.add_torrent(to_add, Some(add_opts)),
        )
        .await
        .map_err(|_| anyhow!("torrent metadata timeout after {:?}", self.cfg.metadata_timeout))??;
        let handle = match res {
            AddTorrentResponse::Added(_, h) | AddTorrentResponse::AlreadyManaged(_, h) => h,
            AddTorrentResponse::ListOnly(_) => bail!("unexpected list-only response"),
        };
        handle.wait_until_initialized().await?;
        if handle.is_paused() {
            self.registry.session.unpause(&handle).await?;
        }

        let files: Vec<FileEntry> = handle.with_metadata(|m| {
            m.file_infos
                .iter()
                .enumerate()
                .map(|(idx, f)| FileEntry {
                    idx,
                    path: f.relative_filename.to_string_lossy().replace('\\', "/"),
                    len: f.len,
                })
                .collect()
        })?;
        if files.is_empty() {
            bail!("Torrent has no files");
        }
        let pairs: Vec<(String, u64)> = files.iter().map(|f| (f.path.clone(), f.len)).collect();
        let file_idx = pick_file(&pairs, opts.file_idx).context("No video file found in torrent")?;

        // Only fetch the chosen file (skip samples / extras). Streaming reads
        // still prioritise pieces around the playhead.
        if let Err(e) = self
            .registry
            .session
            .update_only_files(&handle, &HashSet::from([file_idx]))
            .await
        {
            tracing::warn!("could not restrict torrent to file {file_idx}: {e:#}");
        }

        let hash = handle.info_hash().as_string();
        let entry = Arc::new(Entry {
            handle: handle.clone(),
            file_idx,
            files: files.clone(),
            keep: opts.keep,
            active_streams: AtomicUsize::new(0),
            last_active: Mutex::new(Instant::now()),
        });
        self.registry.entries.lock().unwrap().insert(hash.clone(), entry);

        Ok(AddedTorrent {
            stream_url: self.url_for(&hash, file_idx),
            name: handle.name(),
            file_name: files[file_idx].path.clone(),
            file_len: files[file_idx].len,
            info_hash: hash,
            file_idx,
            files,
        })
    }

    fn url_for(&self, hash: &str, idx: usize) -> String {
        format!("{}/stream/{hash}/{idx}", self.server.base_url())
    }

    /// Stream URL for an added torrent (selected file unless `file_idx` given).
    pub fn stream_url(&self, info_hash: &str, file_idx: Option<usize>) -> Result<String> {
        let e = self.entry(info_hash)?;
        Ok(self.url_for(&e.handle.info_hash().as_string(), file_idx.unwrap_or(e.file_idx)))
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
        let total = e.files[e.file_idx].len;
        let downloaded = s.file_progress.get(e.file_idx).copied().unwrap_or(0).min(total);
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
            stream_url: Some(self.url_for(&e.handle.info_hash().as_string(), e.file_idx)),
            downloaded,
            total,
            finished: downloaded >= total,
            paused: e.handle.is_paused(),
            state: s.state.to_string(),
            error: s.error,
        })
    }

    /// Info-hashes currently managed by the engine.
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

    /// Drop a torrent from the session, optionally deleting its files.
    pub async fn remove(&self, info_hash: &str, delete_files: bool) -> Result<()> {
        let key = extract_info_hash(info_hash)?;
        let removed = self.registry.entries.lock().unwrap().remove(&key);
        if removed.is_none() {
            return Ok(());
        }
        self.registry
            .session
            .delete(TorrentIdOrHash::Hash(Id20::from_str(&key).map_err(|e| anyhow!("{e}"))?), delete_files)
            .await
    }

    /// Evict torrents idle for longer than the configured TTL right now.
    pub async fn cleanup_idle(&self) -> usize {
        self.registry.evict_idle(self.cfg.idle_ttl).await
    }

    /// Stop the HTTP server and the librqbit session.
    pub async fn shutdown(self) {
        self.registry.entries.lock().unwrap().clear();
        self.registry.session.stop().await;
    }
}

impl Registry {
    async fn evict_idle(&self, ttl: Duration) -> usize {
        let stale: Vec<(String, Arc<Entry>)> = self
            .entries
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, e)| {
                !e.keep
                    && e.active_streams.load(Ordering::Relaxed) == 0
                    && e.last_active.lock().unwrap().elapsed() >= ttl
            })
            .map(|(k, e)| (k.clone(), e.clone()))
            .collect();
        let mut n = 0;
        for (k, e) in stale {
            self.entries.lock().unwrap().remove(&k);
            if self.session.delete(TorrentIdOrHash::Hash(e.handle.info_hash()), false).await.is_ok() {
                n += 1;
                tracing::info!("evicted idle torrent {k}");
            }
        }
        n
    }
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
            let mut stream = entry.handle.clone().stream(idx).await.map_err(io::Error::other)?;
            if start > 0 {
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
    }

    async fn offline_engine(dir: &Path) -> Engine {
        let mut cfg = EngineConfig::new(dir.join("dl"));
        cfg.enable_dht = false;
        cfg.enable_listen = false;
        Engine::new(cfg).await.unwrap()
    }

    /// Build a .torrent from a local file, add it pointing at the existing
    /// data (verified, so complete), and stream it through the HTTP server.
    #[tokio::test(flavor = "multi_thread")]
    async fn streams_local_torrent_with_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let seed = dir.path().join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        let data: Vec<u8> = (0..=255u8).cycle().take(300_000).collect();
        let file = seed.join("episode.mkv");
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

        let engine = offline_engine(dir.path()).await;
        let added = engine
            .add(Source::TorrentBytes(bytes), AddOptions { output_dir: Some(seed), ..Default::default() })
            .await
            .unwrap();
        assert_eq!(added.file_idx, 0);
        assert_eq!(added.file_len, 300_000);
        assert_eq!(added.info_hash, meta.info_hash().as_string());
        assert_eq!(engine.stream_url(&added.info_hash, None).unwrap(), added.stream_url);

        let c = reqwest::Client::new();
        let r = c.get(&added.stream_url).header("Range", "bytes=100000-100999").send().await.unwrap();
        assert_eq!(r.status(), 206);
        assert_eq!(r.headers()["content-range"], "bytes 100000-100999/300000");
        assert_eq!(r.headers()["content-type"], "video/webm");
        assert_eq!(r.bytes().await.unwrap().as_ref(), &data[100_000..101_000]);

        let r = c.get(&added.stream_url).send().await.unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(r.bytes().await.unwrap().as_ref(), &data[..]);

        let st = engine.stats(&added.info_hash).unwrap();
        assert_eq!(st.total, 300_000);
        assert!(st.finished && (st.progress - 1.0).abs() < 1e-9);

        engine.remove(&added.info_hash, false).await.unwrap();
        assert!(engine.stats(&added.info_hash).is_err());
        let r = c.get(&added.stream_url).send().await.unwrap();
        assert_eq!(r.status(), 404);
        engine.shutdown().await;
    }

    /// Live network test: Big Buck Bunny (public domain). Run with
    /// `cargo test -p mv-torrent -- --ignored live`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn live_magnet_big_buck_bunny() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = EngineConfig::new(dir.path().join("dl"));
        cfg.metadata_timeout = Duration::from_secs(90);
        let engine = Engine::new(cfg).await.unwrap();
        let magnet = "magnet:?xt=urn:btih:dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c&dn=Big+Buck+Bunny";
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
