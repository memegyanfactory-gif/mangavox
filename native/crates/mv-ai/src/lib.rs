//! mv-ai: multi-provider AI layer (port of src/main/ai). See docs/RUST_REWRITE_PLAN.md.
//!
//! * [`Registry`] — provider catalog, settings in config.json, detection, model lists, `run_step`.
//! * [`ModelHub`] — Hugging Face search/detail, resumable GGUF downloads, local models, Ollama import.
//! * Streaming deltas arrive through an [`Emit`] callback ([`StreamEvent`]).

pub mod config;
pub mod gguf;
pub mod hub;
pub mod providers;
pub mod registry;
pub mod stream;
pub mod transcript;
pub mod types;

pub use gguf::{group_gguf_files, parse_quant, QuantInfo};
pub use hub::{HubConfigPatch, HubConfigPublic, HubSort, ImportResult, ModelHub, ProgressFn, SearchOpts};
pub use registry::{friendly_error, provider_def, ProviderDef, Registry, TestResult, PROVIDERS};
pub use stream::{AiError, Result};
pub use tokio_util::sync::CancellationToken;
pub use transcript::build_transcript;
pub use types::*;
