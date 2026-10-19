//! Settings → Model Library: what is on this PC, what MangaVox uses for each job, downloads
//! and the engines that run the models.
pub mod jobs;
pub mod scan;
mod view;

pub use view::{LibraryEvent, ModelLibrary, Page};
