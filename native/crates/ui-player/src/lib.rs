//! Video player page (`Route::Player`): libmpv in an owned popup window, GPUI control bar,
//! torrent/URL/file sources, skip intro/outro, external subtitles and resume.
use gpui_kit::*;
use mv_ui::Route;

mod backend;
pub mod logic;
pub mod mpv;
mod view;

pub fn open(route: &Route, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    match route {
        Route::Player { request } => Some(view::open_view(request.clone(), window, cx)),
        _ => None,
    }
}
