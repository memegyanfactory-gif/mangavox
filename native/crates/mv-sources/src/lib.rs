//! mv-sources: manga sources for the native MangaVox app.
//!
//! * [`Source`] / [`SourceCtx`] / [`WallSolver`]: the plugin-equivalent interface.
//! * Native scrapers: [`mangadex`], [`webtoon`], [`ehentai`], [`hentai2read`] (plain HTTP) and
//!   [`comick`], [`nhentai`] (plain HTTP with a `WallSolver` fallback), [`mangafire`] (needs a
//!   `WallSolver`: the site is a JS SPA with signed requests).
//! * [`SourceRegistry`]: list / look up the built-in sources.
//!
//! Every parser is a pure function over a response body (`parse_*`), covered by offline tests
//! against real saved responses in `tests/fixtures`.

pub mod comick;
pub mod ehentai;
pub mod hentai2read;
pub mod mangadex;
pub mod mangafire;
pub mod nhentai;
mod registry;
mod source;
mod types;
pub mod util;
pub mod webtoon;

pub use registry::{builtin_for_host, SourceRegistry};
pub use source::{chapter_language, set_chapter_language, set_smart_bypass, smart_bypass, Result, Source, SourceCtx, SourceError, WallSolver};
pub use types::{ChapterInfo, MangaDetails, MangaSearchResult, SourceInfo, Tag};
