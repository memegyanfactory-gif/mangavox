//! App-wide navigation. Pages call `navigate(Route::…, window, cx)`; the shell
//! installs the handler at startup with `set_navigator`.
use std::rc::Rc;

use gpui_kit::{App, EntityId, Global, Window};

/// Every screen in the app. Params are plain data so routes can be stored in history.
#[derive(Clone, Debug, PartialEq)]
pub enum Route {
    // Manga
    Browse,
    Library,
    MangaHistory,
    MangaDownloads,
    MangaDetail { manga_id: String, source_id: Option<String> },
    Reader { manga_id: String, chapter_id: String },
    // Anime / movies
    AnimeHome,
    AnimeDetail { mal_id: String, title: String },
    MoviesHome,
    MovieDetail { imdb_id: String, kind: String, title: String },
    Discover,
    MediaHistory,
    Favorites,
    // Playback
    Player { request: PlayRequest },
    // Studio
    Studio,
    // Phone companion (QR pairing, see ui-phone)
    Mobile,
    /// Channel dashboard, uploads, scheduler and insights for the connected YouTube channel.
    YouTube,
    /// Studio suggestions: trending manga recaps on YouTube, what to recap next, the channel meter.
    Suggestions,
    // App
    Settings { tab: Option<String> },
    /// First-run setup (full window; also "Run welcome setup again" in Settings › About).
    Welcome,
    Back,
}

/// What the player should open.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayRequest {
    pub title: String,
    /// magnet / info-hash / http(s) URL / local file path
    pub source: String,
    pub file_idx: Option<usize>,
    pub imdb_id: Option<String>,
    pub mal_id: Option<String>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
}

type Handler = Rc<dyn Fn(Route, &mut Window, &mut App)>;

struct Navigator(Handler);
impl Global for Navigator {}

pub fn set_navigator(cx: &mut App, f: impl Fn(Route, &mut Window, &mut App) + 'static) {
    cx.set_global(Navigator(Rc::new(f)));
}

pub fn navigate(route: Route, window: &mut Window, cx: &mut App) {
    if let Some(nav) = cx.try_global::<Navigator>() {
        let f = nav.0.clone();
        f(route, window, cx);
    }
}

/// Where Back leads from the current page; the shell keeps it up to date so pages can
/// label their own back control ("Back to Library").
struct BackTarget(Option<Route>);
impl Global for BackTarget {}

pub fn set_back_target(cx: &mut App, route: Option<Route>) {
    cx.set_global(BackTarget(route));
}

pub fn back_target(cx: &App) -> Option<Route> {
    cx.try_global::<BackTarget>().and_then(|b| b.0.clone())
}

/// A page's own Back step (close its dialog, leave a sub-view), asked before the shell leaves
/// the page. Returns true when it handled Back. Keyed by the page's view, so a kept-alive page
/// that is not showing never takes Back.
type LocalBack = Rc<dyn Fn(&mut Window, &mut App) -> bool>;

#[derive(Default)]
struct LocalBacks(std::collections::HashMap<EntityId, LocalBack>);
impl Global for LocalBacks {}

/// Registers `f` as `view`'s own Back step (replaces an earlier one).
pub fn set_local_back(cx: &mut App, view: EntityId, f: impl Fn(&mut Window, &mut App) -> bool + 'static) {
    cx.default_global::<LocalBacks>().0.insert(view, Rc::new(f));
}

/// Runs `view`'s own Back step; false when it has none or nothing to undo.
pub fn local_back(view: EntityId, window: &mut Window, cx: &mut App) -> bool {
    let f = cx.try_global::<LocalBacks>().and_then(|b| b.0.get(&view).cloned());
    f.is_some_and(|f| f(window, cx))
}

/// The page on screen; the shell keeps it up to date. Pages the shell keeps alive in history
/// use it to tell when they are not the one showing (Read aloud pauses then).
struct CurrentRoute(Option<Route>);
impl Global for CurrentRoute {}

pub fn set_current_route(cx: &mut App, route: Route) {
    cx.set_global(CurrentRoute(Some(route)));
}

pub fn current_route(cx: &App) -> Option<Route> {
    cx.try_global::<CurrentRoute>().and_then(|r| r.0.clone())
}
