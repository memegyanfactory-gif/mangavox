//! Studio: turns downloaded manga chapters into an anime-style recap video. The AI
//! (`mv-recap`) reads the pages and writes the story; the Python pipeline in
//! `mv-studio` voices and renders it. The shell calls [`open`] for every route; this
//! crate returns `Some(view)` for `Route::Studio`.
use gpui_kit::*;
use mv_ui::Route;

pub mod audio;
pub use export::join_full_recap;
pub mod data;
mod dashboard;
mod delete;
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
pub mod video;
mod widgets;
mod ytpack;

pub fn open(route: &Route, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    match route {
        Route::Studio => Some(cx.new(|cx| page::StudioPage::new(window, cx)).into()),
        _ => None,
    }
}
