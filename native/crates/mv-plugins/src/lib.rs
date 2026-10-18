//! mv-plugins: JavaScript plugin host for MangaVox manga sources.
//!
//! Runs the existing `plugins/*/index.js` scripts unchanged inside QuickJS
//! (rquickjs). A plugin calls `registerSource({...})` and may use the globals
//! `__fetch`, `__render`, `__solve`, `console`, `URL`, `URLSearchParams`,
//! `atob`/`btoa`. Network access goes through an injectable [`FetchBackend`].

mod backend;
mod host;
mod manifest;
pub mod repo;

pub use backend::{FetchBackend, FetchRequest, FetchResponse, ReqwestBackend, RenderResponse};
pub use host::{load_plugin, load_plugin_with, Plugin, PluginOptions};
pub use manifest::{read_manifest, PluginManifest};
