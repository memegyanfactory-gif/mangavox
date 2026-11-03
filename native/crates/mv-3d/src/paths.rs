//! Where the 3D work lives: `<data>/studio/3d` (the data folder can sit on another drive, so
//! every path starts at `mv_core::paths`).

use std::path::PathBuf;

/// `<data>/studio/3d`
pub fn root() -> PathBuf {
    mv_core::paths::studio().join("3d")
}

/// The shared asset library (`index.json` plus one folder per category).
pub fn library() -> PathBuf {
    root().join("library")
}

/// The Blender scripts the app writes out before each run.
pub fn scripts() -> PathBuf {
    root().join("scripts")
}

/// One chapter's episode: `episode.json`, the scenes' renders and the joined video.
pub fn episode_dir(manga_id: &str, chapter_id: &str) -> PathBuf {
    root().join("episodes").join(mv_core::paths::safe_name(manga_id)).join(mv_core::paths::safe_name(chapter_id))
}
