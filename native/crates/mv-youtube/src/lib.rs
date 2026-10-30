//! YouTube for Studio: connect a channel (OAuth for installed apps, loopback redirect + PKCE),
//! upload and schedule recap videos (resumable upload, `status.publishAt`), read channel and
//! per-video analytics, and the smart scheduler that keeps a manga's chapters in order and pulls
//! the next episode forward when the latest one dips. No UI here: `ui-youtube` draws it and
//! `ui-assistant` exposes it to the chat as tools.
//!
//! Google APIs used:
//! - OAuth 2.0 for desktop apps: <https://developers.google.com/identity/protocols/oauth2/native-app>
//! - YouTube Data API v3: <https://developers.google.com/youtube/v3/docs>
//! - YouTube Analytics API v2: <https://developers.google.com/youtube/analytics/reference/reports/query>
pub mod ai;
pub mod api;
pub mod auth;
pub mod planner;
pub mod seo;
pub mod store;
pub mod trends;
pub mod types;

pub use api::YouTube;
pub use types::*;
