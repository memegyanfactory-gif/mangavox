//! Studio: turns downloaded manga chapters into an anime-style recap video. The AI
//! (`mv-recap`) reads the pages and writes the story; the Python pipeline in
//! `mv-studio` voices and renders it. The shell calls [`open`] for every route; this
//! crate returns `Some(view)` for `Route::Studio`.
use gpui_kit::*;
use mv_ui::Route;

pub mod audio;
pub mod data;
mod page;
mod phases;
mod run;
pub mod service;
pub mod setup;
mod setup_view;
mod side;

pub fn open(route: &Route, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    match route {
        Route::Studio => Some(cx.new(|cx| page::StudioPage::new(window, cx)).into()),
        _ => None,
    }
}
