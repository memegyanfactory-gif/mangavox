//! Anime / movies pages and the Settings -> Addons panel.
//! The shell calls `open` for every route; this crate returns a view for the routes it owns.
use gpui_kit::*;
use mv_ui::Route;

mod addons_panel;
mod assist;
mod anime_detail;
mod anime_home;
mod common;
mod detail_modal;
mod discover;
mod history;
mod look;
mod movie_detail;
mod movies_home;
mod nfx;
pub mod remote;
mod streams;

pub use mv_media::addons::AddonRegistry;

/// Gives the assistant the movie / series / anime tools (sources, play, download). Called once at startup.
pub fn register_assistant_tools() {
    assist::register();
}

/// Shared installed-addon registry (`~/.mangavox/addons`), for other crates that need streams.
pub fn addon_registry() -> &'static std::sync::Arc<AddonRegistry> {
    common::registry()
}

/// Background warm-up at launch: index the artwork cache, trim old entries, then refresh the
/// Movies and Anime home rows so those pages open from a warm cache. Returns at once.
pub fn warm_up() {
    let rt = mv_ui::bg::runtime();
    rt.spawn_blocking(move || {
        // Let the first frames and the start page have the network first.
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let imgs = common::images();
        imgs.warm();
        imgs.evict(1536 * 1024 * 1024, std::time::Duration::from_secs(60 * 86_400));
        mv_core::cache::DataCache::shared().prune(std::time::Duration::from_secs(30 * 86_400));
        rt.spawn(async {
            let t = std::time::Instant::now();
            let log = |what: &str| eprintln!("[cache] warm-up: {what} ready after {} ms", t.elapsed().as_millis());
            futures::join!(async { movies_home::prefetch().await; log("movies") }, async { anime_home::prefetch().await; log("anime") });
        });
    });
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
