//! Video player page (`Route::Player`): full-bleed libmpv video with GPUI controls in a
//! transparent overlay window; torrent/URL/file sources, skip intro/outro, external subtitles, resume.
use gpui_kit::*;
use mv_ui::Route;

pub mod backend;
pub mod logic;
pub mod mpv;
mod overlay;
mod ui;
mod view;

static WATCH_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Bumped whenever playback progress is saved, so pages can refresh their "watched" marks.
pub fn watch_epoch() -> u64 {
    WATCH_EPOCH.load(std::sync::atomic::Ordering::Relaxed)
}

pub(crate) fn bump_watch_epoch() {
    WATCH_EPOCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

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
