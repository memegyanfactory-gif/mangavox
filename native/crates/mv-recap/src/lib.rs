//! mv-recap: the engine behind Studio's manga-to-anime-recap flow (no UI).
//!
//! READ (vision model reads every page, builds a per-manga character "brain") -> TRANSCRIPT (editable text per
//! page) -> STORY (voice-over script in chosen styles) -> export for the Python audio/video pipeline.
//! All async fns are `Send` and run on a tokio runtime (`mv_ui::bg::run`); blocking file/image work is
//! moved to `spawn_blocking` internally. The LLM is reached through `mv_ai::Registry`.

mod brain;
mod export;
mod json;
mod llm;
mod read;
mod reset;
mod store;
mod story;
mod strip;
mod transcript;
mod types;

pub use brain::{safe_id, Brain};
pub use export::export_for_pipeline;
pub use json::extract_json;
pub use read::read_chapter;
pub use reset::{chapter_plan, reset_chapter, reset_phase, reset_plan, Footprint, ResetPlan, ResetReport, Step};
pub use store::{
    chapter_dir, chapter_status, clear_audio, list_pages, load_read, load_story, load_transcript, panel_path, save_read, save_story, save_transcript,
    studio_root,
};
pub use story::{style_hint, write_story};
pub use transcript::{merge_transcript, rewrite_page, transcript_from_read, update_transcript};
pub use types::*;

#[cfg(test)]
mod tests;
