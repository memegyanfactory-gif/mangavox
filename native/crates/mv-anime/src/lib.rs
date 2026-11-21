//! mv-anime: the Anime Engine behind the "Anime Engine" tab (no UI, no GPUI).
//!
//! A project turns downloaded manga chapters into an animatic, step by step: STORY (chapters ->
//! scenes, beats, lines) -> BIBLE (characters, places, props, style, voices) -> SHOTS (the
//! director plans every cut, film-grammar checks) -> FRAMES (one still per shot, drawn on the
//! Magnific website) -> SOUND (rough voices) -> CUT (stills with camera moves + voices -> MP4).
//! Everything lives under `<data>/anime/` as versioned JSON plus content-addressed takes.
//!
//! Core modules are always compiled; each step module sits behind its own cargo feature so a
//! half-written step never breaks the others (design 2.2 and 10.2). All async fns are `Send`
//! and run on the app's tokio runtime (`mv_ui::bg`); the LLM is reached through `mv_ai`.

// Core (always compiled).
pub mod backend;
pub mod geometry;
pub mod hash;
pub mod json;
pub mod llm;
pub mod merge;
pub mod model;
pub mod pacing;
pub mod queue;
pub mod settings;
pub mod source;
pub mod store;

// Steps, one feature each.
#[cfg(feature = "story")]
pub mod story;

#[cfg(feature = "bible")]
pub mod bible;
#[cfg(feature = "bible")]
pub mod crops;
#[cfg(feature = "bible")]
pub mod sheets;

#[cfg(feature = "shots")]
pub mod checks;
#[cfg(feature = "shots")]
pub mod director;
#[cfg(feature = "shots")]
pub mod patterns;

#[cfg(feature = "frames")]
pub mod compile;
#[cfg(feature = "frames")]
pub mod frames;
#[cfg(feature = "frames")]
pub mod slots;

#[cfg(feature = "sound")]
pub mod voices;

#[cfg(feature = "cut")]
pub mod animatic;
#[cfg(feature = "cut")]
pub mod ffmpeg;

pub use model::*;
pub use store::Store;

/// What a step that is not built yet (or not compiled into this build) answers.
pub const NOT_BUILT: &str = "This step is not built yet";

/// The error for [`NOT_BUILT`].
pub fn not_built() -> anyhow::Error {
    anyhow::anyhow!(NOT_BUILT)
}
