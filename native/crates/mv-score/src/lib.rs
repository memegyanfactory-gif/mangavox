//! mv-score: MangaVox's music and sound-effect engine (no UI): royalty-free library tracks or its own
//! synth score, nothing that Content ID can claim.
//!
//! SPOT (cue sheet from the Read phase's scenes and the narration timing, optionally refined by the AI)
//! -> COMPOSE (one genre preset per cue - phonk, epic orchestral, dark tension, horror, sad piano,
//! lo-fi, mystery - as notes on fixed DAW-style tracks: pads, keys, lead, bass, drums, hits & risers, SFX)
//! -> RENDER (built-in synth, or the optional orchestra SoundFont) -> MASTER (loudness set against the
//! narration, ducked under the voice, peak-limited) -> `score.wav`, which the video stage mixes in.
//!
//! By default (`Source::Library`) the cue sheet is scored with produced tracks from the studio
//! library instead (`library.rs`, Kevin MacLeod, CC BY 4.0), mastered the same way.

pub mod ai;
pub mod chapter;
mod compose;
pub mod demo;
pub mod library;
pub mod mix;
mod model;
pub mod orchestra;
pub mod spot;
pub mod synth;
pub mod theory;

pub use chapter::{build, compose_chapter, credits, ensure, is_fresh, load_score, load_settings, preview_wav, save_settings, score_wav};
pub use compose::compose as compose_for;
pub use model::*;

#[cfg(test)]
mod tests;
