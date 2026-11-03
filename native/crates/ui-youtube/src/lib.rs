//! YouTube: a YouTube Studio for the connected channel inside MangaVox, drawn like Studio itself.
//! Dashboard (with every recap Studio made and where it stands on YouTube), the channel's videos
//! with an SEO editor and AI titles/descriptions/hashtags, analytics, comments you can answer, the
//! competitors tracker (every recap channel in the niche, hour by hour), the Automation section
//! that runs the channel end to end, the upload queue and release calendar (chapters always in
//! order) and the smart-upload automation. Backend in `mv-youtube`. The shell calls [`open`] for
//! every route; this crate returns `Some(view)` for `Route::YouTube` and `Route::Automation`.
use gpui_kit::*;
use mv_ui::Route;

mod arc;
pub mod automation;
mod automation_view;
mod chart;
mod comments_view;
mod competitors_view;
mod content;
pub mod hub;
mod page;
pub mod rivals;
mod suggest;
mod timing_view;
mod tools;
mod views;
mod when;
mod yt;

pub fn open(route: &Route, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    match route {
        Route::YouTube => Some(cx.new(|cx| page::YouTubePage::new(window, cx)).into()),
        Route::Automation => Some(cx.new(|cx| page::YouTubePage::with_tab(Some(page::Tab::Automation), window, cx)).into()),
        Route::Suggestions => Some(cx.new(|cx| suggest::SuggestionsPage::new(window, cx)).into()),
        _ => None,
    }
}

/// Background work while the app is open (call once at startup): the channel refresh and dip
/// automation (when a channel is connected), the hourly competitor research, the automation
/// runner (jobs carry on after a restart) and the chat's youtube/competitors/automation tools.
pub fn start(cx: &mut App) {
    hub::start(cx);
    rivals::start(cx);
    automation::start(cx);
    tools::register();
}
