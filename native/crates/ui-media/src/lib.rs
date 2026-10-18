//! Anime / movies pages and the Settings -> Addons panel.
//! The shell calls `open` for every route; this crate returns a view for the routes it owns.
use gpui_kit::*;
use mv_ui::Route;

mod addons_panel;
mod anime_detail;
mod anime_home;
mod common;
mod discover;
mod history;
mod movie_detail;
mod movies_home;
mod streams;

pub use mv_media::addons::AddonRegistry;

/// Shared installed-addon registry (`~/.mangavox/addons`), for other crates that need streams.
pub fn addon_registry() -> &'static std::sync::Arc<AddonRegistry> {
    common::registry()
}

pub fn open(route: &Route, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    Some(match route {
        Route::AnimeHome => cx.new(|cx| anime_home::AnimeHome::new(window, cx)).into(),
        Route::AnimeDetail { mal_id, title } => cx.new(|cx| anime_detail::AnimeDetail::new(mal_id.clone(), title.clone(), cx)).into(),
        Route::MoviesHome => cx.new(|cx| movies_home::MoviesHome::new(window, cx)).into(),
        Route::MovieDetail { imdb_id, kind, title } => {
            cx.new(|cx| movie_detail::MovieDetail::new(imdb_id.clone(), kind.clone(), title.clone(), cx)).into()
        }
        Route::Discover => cx.new(discover::Discover::new).into(),
        Route::MediaHistory => cx.new(history::MediaHistory::new).into(),
        Route::Favorites => cx.new(history::Favorites::new).into(),
        _ => return None,
    })
}

/// Settings -> Addons panel (Stremio addons). Embedded by ui-settings.
pub fn addons_panel(window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| addons_panel::AddonsPanel::new(window, cx)).into()
}
