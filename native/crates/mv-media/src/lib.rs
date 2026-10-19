//! mv-media: async clients for Jikan (MyAnimeList), AniList, Cinemeta, Stremio addons and AniSkip.
//! Mirrors the TypeScript services under src/main/services and src/main/addons.

pub mod addons;
pub mod anilist;
pub mod cinemeta;
pub mod episode_art;
pub mod error;
pub mod image_cache;
pub mod jikan;
pub mod skip_times;
pub mod stremio;
pub mod stream_utils;
pub mod types;
mod util;

pub use error::{MediaError, Result};
