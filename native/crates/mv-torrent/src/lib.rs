//! mv-torrent: librqbit-backed streaming torrent engine plus a local HTTP
//! range server (`/stream/{infohash}/{file_idx}` and `/direct?url=`).
//!
//! Replaces `torrent-engine.ts`, `direct-stream-engine.ts` and the HTTP half of
//! `stream-protocol.ts`. See `docs/RUST_REWRITE_PLAN.md`.
//!
//! ```no_run
//! # async fn demo() -> anyhow::Result<()> {
//! use mv_torrent::{AddOptions, Engine, EngineConfig, Source};
//! let engine = Engine::new(EngineConfig::new("cache/torrents")).await?;
//! let added = engine.add(Source::parse("magnet:?xt=urn:btih:...")?, AddOptions::default()).await?;
//! println!("play {}", added.stream_url); // feed to the player
//! let stats = engine.stats(&added.info_hash)?;
//! engine.remove(&added.info_hash, false).await?;
//! # Ok(()) }
//! ```

mod engine;
mod http;

pub use engine::{
    extract_info_hash, pick_file, AddOptions, AddedTorrent, Engine, EngineConfig, FileEntry,
    Source, TorrentStats, DEFAULT_TRACKERS,
};
pub use http::{
    build_response, mime_from_name, parse_range, BoxReader, ByteSource, LocalFileSource,
    RangeSpec, SourceResolver, StreamServer,
};
