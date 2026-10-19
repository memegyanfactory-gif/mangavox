//! Video player page (`Route::Player`): full-bleed libmpv video with GPUI controls in a
//! transparent overlay window; torrent/URL/file sources, skip intro/outro, external subtitles, resume.
use gpui_kit::*;
use mv_ui::Route;

mod backend;
pub mod logic;
pub mod mpv;
mod overlay;
mod view;

/// Warm the torrent behind `request` while the user is still choosing (detail pages).
pub fn prefetch(request: &mv_ui::nav::PlayRequest) {
    backend::prefetch(request);
}

pub fn open(route: &Route, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    match route {
        Route::Player { request } => Some(view::open_view(request.clone(), window, cx)),
        _ => None,
    }
}
