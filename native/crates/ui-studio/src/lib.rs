//! Studio: turns downloaded manga chapters into an anime-style recap video. The AI
//! (`mv-recap`) reads the pages and writes the story; the Python pipeline in
//! `mv-studio` voices and renders it. The shell calls [`open`] for every route; this
//! crate returns `Some(view)` for `Route::Studio`.
use gpui_kit::*;
use mv_ui::Route;

mod assist;
pub mod audio;
pub use export::join_full_recap;
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

pub fn open(route: &Route, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    // The assistant's video_* tools work from any page.
    vedit_tools::register();
    match route {
        Route::Studio => Some(cx.new(|cx| page::StudioPage::new(window, cx)).into()),
        _ => None,
    }
}
