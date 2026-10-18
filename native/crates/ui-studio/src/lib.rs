//! Studio: turns downloaded manga chapters into narrated audio and video with the
//! Python pipeline in `mv-studio`. The shell calls [`open`] for every route; this
//! crate returns `Some(view)` for `Route::Studio`.
use gpui_kit::*;
use mv_ui::Route;

pub mod data;
mod page;
pub mod service;
pub mod setup;

pub fn open(route: &Route, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    match route {
        Route::Studio => Some(cx.new(|cx| page::StudioPage::new(window, cx)).into()),
        _ => None,
    }
}
