//! Studio: turns downloaded manga chapters into an anime-style recap video. The AI
//! (`mv-recap`) reads the pages and writes the story; the Python pipeline in
//! `mv-studio` voices and renders it. The shell calls [`open`] for every route; this
//! crate returns `Some(view)` for `Route::Studio`.
use gpui_kit::*;
use mv_ui::Route;

mod assist;
mod auto_export;
pub use auto_export::folder_for as auto_export_folder;
mod cropper;
pub mod audio;
pub use export::join_full_recap;
pub use generate::queue_saved;
pub mod data;
mod dashboard;
mod delete;
mod encode;
mod export;
mod generate;
mod issues;
pub mod look;
mod music;
pub mod pipeline;
mod queue_view;
mod restyle;
mod page;
mod phases;
mod run;
pub mod service;
pub mod setup;
mod setup_view;
mod side;
mod tile_menu;
pub mod vedit;
mod vedit_frame;
mod vedit_history;
mod vedit_keys;
mod vedit_media;
mod vedit_manga;
mod vedit_plan;
mod vedit_play;
mod vedit_tl;
mod vedit_tools;
mod vedit_view;
pub mod video;
mod widgets;
mod ytpack;

/// Gives the assistant Studio's tools (library, Generate / Redo, queue, export, look, captions,
/// YouTube package) and its proactive suggestions. Called once at startup.
pub fn register_assistant_tools() {
    assist::register();
}

/// The one Studio page of the session: leaving Studio and coming back finds it as it was, its
/// export (progress, Cancel, the exports queued after it) and Generate state included.
struct Kept(Entity<page::StudioPage>);
impl Global for Kept {}

pub fn open(route: &Route, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    // The assistant's video_* tools work from any page.
    vedit_tools::register();
    match route {
        Route::Studio => {
            if let Some(Kept(page)) = cx.try_global::<Kept>() {
                let page = page.clone();
                // Chapters may have been downloaded or deleted elsewhere meanwhile.
                page.update(cx, |p, cx| p.reload_library(cx));
                return Some(page.into());
            }
            let page = cx.new(|cx| page::StudioPage::new(window, cx));
            cx.set_global(Kept(page.clone()));
            Some(page.into())
        }
        Route::Thumbnails => Some(cx.new(ytpack::ThumbnailsPage::new).into()),
        Route::YouTubeVideos => Some(cx.new(ytpack::VideosPage::new).into()),
        _ => None,
    }
}

/// Studio's image models (Magnific and the services in Settings › AI services), for other pages
/// that make images (Creative).
pub mod images {
    pub use crate::ytpack::magnific::{generate, key, key_for, record, usable, ImageModel, Keys, Reference, Shape, Via, CATALOG, KEY_REJECTED};
}

/// Settings › Magnific: the API key, the thumbnail defaults and the credits spent.
pub fn magnific_settings(window: &mut Window, cx: &mut App) -> AnyView {
    ytpack::magnific_settings(window, cx)
}
