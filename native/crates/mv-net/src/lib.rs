//! mv-net: the shared async HTTP layer for MangaVox.
//!
//! A thin wrapper over `reqwest` (rustls, cookies, gzip/brotli, optional SOCKS5/HTTP proxy) that
//! adds what the scrapers need: a desktop-browser User-Agent, a per-host rate limiter, retry with
//! backoff for 429 / 5xx / connect / timeout failures, and `get_text` / `get_json` / `get_bytes`
//! helpers that take extra headers and a `Referer`.
//!
//! Behaviour mirrors the Electron app's `dns-fetch.ts` (plain GET/POST returning status + text,
//! redirects followed, proxy aware) and `native-fetch.ts` (anti-bot wall detection so callers can
//! hand off to a browser-based solver).

mod client;
mod wall;

pub use client::{Body, Http, NetConfig, NetError, Opts, Resp, BROWSER_UA};
pub use wall::{detect_wall, WallKind};

pub type Result<T> = std::result::Result<T, NetError>;
