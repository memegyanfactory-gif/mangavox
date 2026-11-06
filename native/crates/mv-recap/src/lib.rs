//! mv-recap: the engine behind Studio's manga-to-anime-recap flow (no UI).
//!
//! READ (vision model reads every page, builds a per-manga character "brain") -> TRANSCRIPT (editable text per
//! page) -> STORY (voice-over script in chosen styles) -> export for the Python audio/video pipeline.
//! All async fns are `Send` and run on a tokio runtime (`mv_ui::bg::run`); blocking file/image work is
//! moved to `spawn_blocking` internally. The LLM is reached through `mv_ai::Registry`.

pub mod auto;
mod brain;
mod cast;
pub mod cast_profile;
mod export;
pub mod hook;
mod json;
mod llm;
mod manual;
pub mod media;
mod panels;
mod read;
mod repeats;
pub mod series;
mod reset;
mod store;
mod story;
mod strip;
pub mod thumb;
mod transcript;
mod types;

pub use brain::{safe_id, Brain, ChapterRecap};
pub use export::export_for_pipeline;
pub use json::extract_json;
pub use llm::retry_now;
pub use manual::{load_manual, reset_manual_page, set_manual_page, ManualPage, ManualPanels};
pub use auto::{read_chapter_smart, write_story_anyway_smart, write_story_for_manga_smart};
pub use read::read_chapter;
pub use reset::{chapter_plan, reset_chapter, reset_phase, reset_plan, Footprint, ResetPlan, ResetReport, Step};
pub use store::{
    chapter_dir, chapter_status, clear_audio, clear_same_as, list_pages, load_read, load_same_as, load_story, load_transcript, panel_path, save_read, save_same_as, save_story, save_transcript,
    studio_root,
};
pub use story::{style_hint, write_story, write_story_anyway, write_story_for_manga, SAME_CHAPTER_NOTE};
pub use transcript::{merge_transcript, rewrite_page, transcript_from_read, update_transcript};
pub use types::*;

#[cfg(test)]
mod live_providers;
#[cfg(test)]
mod tests;
