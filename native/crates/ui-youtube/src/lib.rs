//! YouTube: a YouTube Studio for the connected channel inside MangaVox. Dashboard, the channel's
//! videos with an SEO editor and AI titles/descriptions/hashtags, the upload queue and release
//! calendar (chapters always in order), analytics, and the smart-upload automation that pulls the
//! next episode forward when one dips. Backend in `mv-youtube`. The shell calls [`open`] for
//! every route; this crate returns `Some(view)` for `Route::YouTube`.
use gpui_kit::*;
use mv_ui::Route;

mod chart;
mod content;
pub mod hub;
mod page;
mod suggest;
mod views;

pub fn open(route: &Route, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    match route {
        Route::YouTube => Some(cx.new(|cx| page::YouTubePage::new(window, cx)).into()),
        Route::Suggestions => Some(cx.new(|cx| suggest::SuggestionsPage::new(window, cx)).into()),
        _ => None,
    }
}

/// Background refresh and dip automation for a connected channel (call once at startup).
pub fn start(cx: &mut App) {
    hub::start(cx);
}
