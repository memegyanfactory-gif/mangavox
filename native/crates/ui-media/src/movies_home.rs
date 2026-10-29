//! Movies home, Netflix-style (see `nfx`): an inset rotating billboard, Continue Watching, Top 10
//! rows, trending / acclaimed / genre rows of logo boxart with hover cards, "Explore All" grids
//! and search. Data comes from Cinemeta.
use std::collections::{HashMap, HashSet};
use std::time::Instant;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::media::{self, PlaybackState};
use mv_core::repo::watchlist;
use mv_media::cinemeta::{CinemetaCatalog, CinemetaClient};
use mv_media::types::AnimeSearchResult;
use mv_ui::motion::reduced;
use mv_ui::theme::{icon as isz, radius, ramp};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, disclosure, empty_view, error_view, glyphs, icon, inline_alert, muted, navigate, ph, AlertKind, ButtonKind, Interactive, RampExt, Route, Tokens};

use crate::common::{self, announce_list_toggle, db, fetch, fetch_cached, fraction, mark_watching, media_id, media_title, route_for_media, set_watchlist, Load, DAY, SEARCH_DEBOUNCE};
use crate::detail_modal::detail_modal;
use crate::look::{self, OnClick, RowOffsets};
use crate::movie_detail::MovieDetail;
use crate::nfx::{self, CardKey, EDGE};
use crate::streams::{self, PlayBase, StreamState};

/// Genre rows shown on the home page, in order.
const ROW_GENRES: &[&str] = &["Action", "Comedy", "Sci-Fi", "Thriller", "Animation", "Horror", "Romance", "Documentary"];

/// A Top 10 item (numeral + poster, see `look::top10_card`) is this many times its height wide
/// at 1.037; this height makes it exactly one boxart wide, so the row pages evenly.
const TOP10_H: f32 = 0.96;

/// Cinemeta pages hold 100 titles.
const CINEMETA_PAGE: usize = 100;

pub fn cinemeta() -> Result<CinemetaClient, String> {
    CinemetaClient::new().map_err(|e| e.to_string())
}

/// Cache key + loader for one Cinemeta catalog page (shared with the startup warm-up).
pub fn catalog_key(movie: bool, catalog: CinemetaCatalog, genre: Option<&str>) -> String {
    format!("cinemeta:{}:{}:{}:1", if movie { "movie" } else { "series" }, catalog.as_str(), genre.unwrap_or(""))
}

pub async fn catalog(movie: bool, catalog: CinemetaCatalog, genre: Option<&'static str>, page: u32) -> Result<Vec<AnimeSearchResult>, String> {
    let c = cinemeta()?;
    let r = if movie { c.movies_catalog(catalog, page, genre).await } else { c.series_catalog(catalog, page, genre).await };
    r.map_err(|e| e.to_string())
}

/// Fill the cache for the Home tab's rows (fresh entries are only read into memory).
pub async fn prefetch() {
    let jobs = MoviesHome::plan(Tab::Home).into_iter().map(|(_, _, _, movie, cat, genre)| {
        let key = catalog_key(movie, cat, genre);
        async move {
            match mv_core::cache::DataCache::shared().get::<Vec<AnimeSearchResult>>(&key) {
                Some((_, age)) if age < DAY => {}
                _ => {
                    let _ = common::cached(key, catalog(movie, cat, genre, 1)).await;
                }
            }
        }
    });
    futures::future::join_all(jobs).await;
}

/// Cinemeta artwork by IMDb id, for items whose catalog entry lacks it.
pub fn background_of(r: &AnimeSearchResult) -> Option<String> {
    r.banner_url.clone().filter(|u| !u.is_empty()).or_else(|| r.imdb_id.as_ref().map(|id| format!("https://images.metahub.space/background/medium/{id}/img")))
}

pub fn logo_of(r: &AnimeSearchResult) -> Option<String> {
    r.logo_url.clone().filter(|u| !u.is_empty()).or_else(|| {
        r.imdb_id.as_ref().filter(|id| id.starts_with("tt")).map(|id| format!("https://images.metahub.space/logo/medium/{id}/img"))
    })
}

pub fn kind_of(r: &AnimeSearchResult) -> &'static str {
    if r.anime_type.as_deref() == Some("Movie") {
        "movie"
    } else {
        "series"
    }
}

/// Season and episode to resume a series at: the history row has only the episode number, so
/// it is matched against the season last started from these pages.
pub fn series_point(imdb: &str, episode: Option<i64>) -> Option<(u32, u32)> {
    let ep = u32::try_from(episode?).ok()?;
    let (s, last) = streams::last_played(imdb)?;
    // The player may have moved on to later episodes of the same season by itself.
    (ep >= last).then_some((s, ep))
}

/// The rows of a home tab ("home" | "movies" | "series") for the phone companion:
/// (id, title, top 10?, movie?, catalog, genre), same order as this page.
#[allow(clippy::type_complexity)]
pub(crate) fn home_rows(tab: &str) -> Vec<(&'static str, String, bool, bool, CinemetaCatalog, Option<&'static str>)> {
    let tab = match tab {
        "movies" => Tab::Movies,
        "series" => Tab::Series,
        _ => Tab::Home,
    };
    MoviesHome::plan(tab).into_iter().map(|(id, title, look, movie, cat, genre)| (id, title, look == Look::Top10, movie, cat, genre)).collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Home,
    Movies,
    Series,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    Landscape,
    Top10,
}

struct Row {
    id: &'static str,
    title: String,
    look: Look,
    /// (movie?, catalog, genre), to load more pages for See all.
    src: (bool, CinemetaCatalog, Option<&'static str>),
    load: Load<Vec<AnimeSearchResult>>,
}

/// A row opened with "See all": every title of the catalog, a page at a time.
struct Expanded {
    id: &'static str,
    title: String,
    src: (bool, CinemetaCatalog, Option<&'static str>),
    items: Vec<AnimeSearchResult>,
    page: u32,
    loading: bool,
    done: bool,
    error: Option<String>,
}

/// Where the user was on this page: Back returns there, and the tab and genre open again at
/// the next launch.
#[derive(Clone)]
struct Place {
    tab: Tab,
    genre: Option<&'static str>,
    scroll: Point<Pixels>,
    offsets: RowOffsets,
}

/// The Genres menu, as (Cinemeta genre, title on Movies, title on Series), in Netflix's order.
const GENRES: &[(&str, &str, &str)] = &[
    ("Action", "Action", "Action"),
    ("Adventure", "Adventure", "Adventure"),
    ("Animation", "Animation", "Animation"),
    ("Comedy", "Comedies", "Comedies"),
    ("Crime", "Crime", "Crime"),
    ("Documentary", "Documentaries", "Docuseries"),
    ("Drama", "Dramas", "Dramas"),
    ("Family", "Family", "Kids & Family"),
    ("Fantasy", "Fantasy", "Fantasy"),
    ("History", "History", "History"),
    ("Horror", "Horror", "Horror"),
    ("Mystery", "Mysteries", "Mysteries"),
    ("Romance", "Romance", "Romance"),
    ("Sci-Fi", "Sci-Fi", "Sci-Fi"),
    ("Sport", "Sports", "Sports"),
    ("Thriller", "Thrillers", "Thrillers"),
    ("Biography", "Biographical", "Biographical"),
    ("War", "War", "War"),
    ("Western", "Westerns", "Westerns"),
    ("Reality-TV", "", "Reality TV"),
];

/// The menu's genres for a tab (Reality TV is a Series genre only).
fn genres_for(movie: bool) -> impl Iterator<Item = (&'static str, &'static str)> {
    GENRES.iter().map(move |(g, m, s)| (*g, if movie { *m } else { *s })).filter(|(_, label)| !label.is_empty())
}

fn genre_label(genre: &str, movie: bool) -> &'static str {
    genres_for(movie).find(|(g, _)| *g == genre).map(|(_, l)| l).unwrap_or("")
}

/// The tab and genre last open, from config.json (`movies_place`).
fn saved_place() -> (Tab, Option<&'static str>) {
    let v = mv_core::config::Config::open_default().get("movies_place").unwrap_or_default();
    let tab = match v.get("tab").and_then(|t| t.as_str()) {
        Some("movies") => Tab::Movies,
        Some("series") => Tab::Series,
        _ => Tab::Home,
    };
    let genre = v.get("genre").and_then(|g| g.as_str()).and_then(|g| GENRES.iter().find(|(id, _, _)| *id == g).map(|(id, _, _)| *id));
    (tab, if tab == Tab::Home { None } else { genre })
}

fn save_place(tab: Tab, genre: Option<&'static str>) {
    let tab = match tab {
        Tab::Home => "home",
        Tab::Movies => "movies",
        Tab::Series => "series",
    };
    let _ = mv_core::config::Config::open_default().set("movies_place", serde_json::json!({ "tab": tab, "genre": genre }));
}

pub struct MoviesHome {
    tab: Tab,
    /// The genre picked from the Genres menu (Movies and Series tabs).
    genre: Option<&'static str>,
    genre_menu: bool,
    /// Tabs and genres visited before, for Back.
    places: Vec<Place>,
    /// Scroll position and row pages to put back once the rows of a place Back returned to are in.
    pending_scroll: Option<(Point<Pixels>, RowOffsets)>,
    rows: Vec<Row>,
    cont: Vec<PlaybackState>,
    my_list: Vec<AnimeSearchResult>,
    hero: usize,
    hero_prev: Option<usize>,
    /// Where the hero was painted; the carousel holds still while the pointer is inside.
    hero_bounds: look::BoundsSlot,
    hero_paused: bool,
    offsets: RowOffsets,
    /// Where each row slid from and when, so the slide animates (settled after `nfx::SLIDE`).
    slides: HashMap<&'static str, (usize, Instant)>,
    /// Card bounds and the open hover card.
    hov: nfx::Hovering,
    /// My List is being saved from a hover card.
    saving_list: bool,
    /// The "More Info" modal over the page, and the focus it takes so Escape reaches it.
    detail: Option<Entity<MovieDetail>>,
    detail_focus: FocusHandle,
    search: Entity<InputState>,
    search_gen: u64,
    results: Option<(String, Load<Vec<AnimeSearchResult>>)>,
    expanded: Option<Expanded>,
    home_offset: Option<Point<Pixels>>,
    streams: StreamState,
    lookup: Option<Task<()>>,
    last_play: Option<(String, PlayBase, bool)>,
    focus: FocusHandle,
    /// What had focus before the overlay opened; it gets it back on close (WAI-ARIA dialog pattern).
    return_focus: Option<FocusHandle>,
    /// `common::progress_stamp()` / `common::list_epoch()` when Continue Watching and My List were
    /// read. The page is kept alive between visits, so it rereads them after either changed.
    personal_epoch: Option<((u64, u64), u64)>,
    scroll: ScrollHandle,
    generation: u64,
    _sub: Subscription,
}

fn close_streams(v: &mut MoviesHome, window: &mut Window, cx: &mut Context<MoviesHome>) {
    v.streams = StreamState::Idle;
    v.lookup = None;
    if let Some(f) = v.return_focus.take() {
        window.focus(&f, cx);
    }
}

fn retry_lookup(v: &mut MoviesHome, window: &mut Window, cx: &mut Context<MoviesHome>) {
    if let Some((label, base, movie)) = v.last_play.clone() {
        v.start(label, base, movie, window, cx);
    }
}

impl MoviesHome {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = common::input(window, cx, "Search movies and series");
        let sub = cx.subscribe_in(&search, window, |this, _, ev: &InputEvent, _, cx| match ev {
            InputEvent::PressEnter { .. } => {
                this.search_gen += 1;
                this.run_search(cx);
            }
            InputEvent::Change => this.debounce_search(cx),
            _ => {}
        });
        let (tab, genre) = saved_place();
        let weak = cx.weak_entity();
        let me = cx.entity_id();
        mv_ui::set_local_back(cx, me, move |w, cx| weak.update(cx, |v, cx| v.local_back(w, cx)).unwrap_or(false));
        let mut s = Self {
            tab,
            genre,
            genre_menu: false,
            places: vec![],
            pending_scroll: None,
            rows: vec![],
            cont: vec![],
            my_list: vec![],
            hero: 0,
            hero_prev: None,
            hero_bounds: Default::default(),
            hero_paused: false,
            offsets: Default::default(),
            slides: Default::default(),
            hov: Default::default(),
            saving_list: false,
            detail: None,
            detail_focus: cx.focus_handle(),
            search,
            search_gen: 0,
            results: None,
            expanded: None,
            home_offset: None,
            streams: StreamState::Idle,
            lookup: None,
            last_play: None,
            focus: cx.focus_handle(),
            return_focus: None,
            personal_epoch: None,
            scroll: ScrollHandle::new(),
            generation: 0,
            _sub: sub,
        };
        s.load(cx);
        look::rotate(window, cx, Self::step);
        s
    }

    /// History + My List (movies and series only), reread whenever the player saved progress,
    /// History or My List changed since: a Continue Watching card resumes straight away, so a
    /// stale list would play the wrong episode, and a cleared title stayed in the row.
    fn load_personal(&mut self, cx: &mut Context<Self>) {
        let epoch = (common::progress_stamp(), common::list_epoch());
        if self.personal_epoch == Some(epoch) {
            return;
        }
        self.personal_epoch = Some(epoch);
        fetch(
            cx,
            db(|c| {
                let mut h = media::list_history(c, Some("movie"), None, 20)?;
                h.extend(media::list_history(c, Some("series"), None, 20)?);
                h.retain(|h| !h.completed && h.duration_secs > 0.0);
                h.sort_by_key(|h| std::cmp::Reverse(h.updated_at));
                Ok(h)
            }),
            |v: &mut Self, l| {
                if let Load::Ready(cont) = l {
                    v.cont = cont;
                }
            },
        );
        fetch(
            cx,
            db(|c| {
                let ids: Vec<_> = watchlist::list(c, None)?.into_iter().filter(|e| e.kind == "movie" || e.kind == "series").collect();
                Ok(ids
                    .into_iter()
                    .filter_map(|e| {
                        let t = media::get_title(c, &e.media_id).ok().flatten()?;
                        Some(AnimeSearchResult {
                            id: t.imdb_id.clone().unwrap_or_default(),
                            imdb_id: t.imdb_id,
                            title: t.title,
                            cover_url: t.cover_url,
                            banner_url: t.banner_url,
                            anime_type: Some(if e.kind == "movie" { "Movie" } else { "TV" }.into()),
                            ..Default::default()
                        })
                    })
                    .collect::<Vec<_>>())
            }),
            |v: &mut Self, l| {
                if let Load::Ready(list) = l {
                    v.my_list = list;
                }
            },
        );
    }

    /// A genre page: its Top 10, its popular titles and its best rated (Cinemeta's genre filter).
    #[allow(clippy::type_complexity)]
    fn plan_for(tab: Tab, genre: Option<&'static str>) -> Vec<(&'static str, String, Look, bool, CinemetaCatalog, Option<&'static str>)> {
        let Some(g) = genre.filter(|_| tab != Tab::Home) else { return Self::plan(tab) };
        let movie = tab == Tab::Movies;
        let label = genre_label(g, movie);
        vec![
            ("genre-top10", format!("Top 10 {label} Today"), Look::Top10, movie, CinemetaCatalog::Top, Some(g)),
            ("genre-popular", format!("Popular {label}"), Look::Landscape, movie, CinemetaCatalog::Top, Some(g)),
            ("genre-acclaimed", format!("Critically Acclaimed {label}"), Look::Landscape, movie, CinemetaCatalog::ImdbRating, Some(g)),
        ]
    }

    #[allow(clippy::type_complexity)]
    fn plan(tab: Tab) -> Vec<(&'static str, String, Look, bool, CinemetaCatalog, Option<&'static str>)> {
        // (id, title, look, movie?, catalog, genre)
        let mut v = vec![];
        if tab != Tab::Series {
            v.push(("top10-movies", "Top 10 Movies Today".to_string(), Look::Top10, true, CinemetaCatalog::Top, None));
        }
        if tab != Tab::Movies {
            v.push(("top10-series", "Top 10 Series Today".to_string(), Look::Top10, false, CinemetaCatalog::Top, None));
        }
        if tab != Tab::Series {
            v.push(("trending", "Trending Now".to_string(), Look::Landscape, true, CinemetaCatalog::Popular, None));
            v.push(("acclaimed", "Critically Acclaimed Movies".to_string(), Look::Landscape, true, CinemetaCatalog::ImdbRating, None));
        }
        if tab != Tab::Movies {
            v.push(("binge", "Binge-worthy Series".to_string(), Look::Landscape, false, CinemetaCatalog::Popular, None));
            v.push(("acclaimed-tv", "Critically Acclaimed TV".to_string(), Look::Landscape, false, CinemetaCatalog::ImdbRating, None));
        }
        let ids: [&'static str; 8] = ["g0", "g1", "g2", "g3", "g4", "g5", "g6", "g7"];
        for (i, g) in ROW_GENRES.iter().enumerate() {
            let movie = tab != Tab::Series;
            let noun = if movie { "Movies" } else { "TV" };
            let title = match *g {
                "Animation" => format!("Animated {noun}"),
                "Documentary" => {
                    if movie {
                        "Documentaries".to_string()
                    } else {
                        "Docuseries".to_string()
                    }
                }
                g => format!("{g} {noun}"),
            };
            v.push((ids[i], title, Look::Landscape, movie, CinemetaCatalog::Top, Some(*g)));
        }
        v
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.hero = 0;
        self.hero_prev = None;
        self.offsets.clear();
        self.rows = Self::plan_for(self.tab, self.genre)
            .into_iter()
            .map(|(id, title, look, movie, cat, genre)| {
                let load = fetch_cached(cx, catalog_key(movie, cat, genre), DAY, catalog(movie, cat, genre, 1), move |v: &mut Self, l| {
                    if v.generation == generation {
                        if let Some(row) = v.rows.iter_mut().find(|r| r.id == id) {
                            row.load = l;
                        }
                    }
                });
                Row { id, title, look, src: (movie, cat, genre), load }
            })
            .collect();
        cx.notify();
    }

    fn step(&mut self, window: &Window) -> bool {
        if look::pointer_over(&self.hero_bounds, window) || self.hero_paused || reduced() || !matches!(self.streams, StreamState::Idle) || self.results.is_some() || self.expanded.is_some() || self.detail.is_some() {
            return false;
        }
        self.go_hero(self.hero + 1);
        true
    }

    fn go_hero(&mut self, i: usize) {
        if i != self.hero {
            self.hero_prev = Some(self.hero);
            self.hero = i;
        }
    }

    fn debounce_search(&mut self, cx: &mut Context<Self>) {
        self.search_gen += 1;
        let generation = self.search_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let _ = this.update(cx, |v, cx| {
                if v.search_gen == generation {
                    v.run_search(cx);
                }
            });
        })
        .detach();
    }

    fn run_search(&mut self, cx: &mut Context<Self>) {
        let q = self.search.read(cx).value().trim().to_string();
        if q.is_empty() {
            if self.results.take().is_some() {
                self.restore_home();
            }
            cx.notify();
            return;
        }
        if self.results.as_ref().is_some_and(|(cur, l)| *cur == q && !matches!(l, Load::Failed(_))) {
            return;
        }
        if self.results.is_none() && self.expanded.is_none() {
            self.home_offset = Some(self.scroll.offset());
        }
        self.expanded = None;
        self.results = Some((q.clone(), Load::Loading));
        cx.notify();
        let query = q.clone();
        fetch(
            cx,
            async move {
                let c = cinemeta()?;
                let (movies, series) = futures::join!(c.search_movies(&query), c.search_series(&query));
                // Both failing is an error; no matches is an empty list (shown as "No matches").
                if let (Err(e), Err(_)) = (&movies, &series) {
                    return Err(e.to_string());
                }
                let mut all = movies.unwrap_or_default();
                let extra: Vec<_> = series.unwrap_or_default().into_iter().filter(|s| !all.iter().any(|m| m.id == s.id)).collect();
                all.extend(extra);
                Ok(all)
            },
            move |v: &mut Self, l| {
                if v.results.as_ref().is_some_and(|(cur, _)| *cur == q) {
                    v.results = Some((q, l));
                }
            },
        );
    }

    fn restore_home(&mut self) {
        if let Some(o) = self.home_offset.take() {
            self.scroll.set_offset(o);
        }
    }

    fn back_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.results = None;
        self.expanded = None;
        self.search_gen += 1;
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        self.restore_home();
        cx.notify();
    }

    /// Opens a tab (and genre) and remembers where the user was, for Back and the next launch.
    fn go_to(&mut self, tab: Tab, genre: Option<&'static str>, window: &mut Window, cx: &mut Context<Self>) {
        self.genre_menu = false;
        if self.results.is_none() && self.expanded.is_none() {
            let here = Place { tab: self.tab, genre: self.genre, scroll: self.scroll.offset(), offsets: self.offsets.clone() };
            self.places.push(here);
            if self.places.len() > 30 {
                self.places.remove(0);
            }
        }
        self.tab = tab;
        self.genre = genre;
        self.results = None;
        self.expanded = None;
        self.home_offset = None;
        self.pending_scroll = None;
        // Leaving search for a tab clears the query too (the field kept showing a search whose
        // results were gone), and the new tab starts at its top.
        self.search_gen += 1;
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        self.scroll.set_offset(point(px(0.), px(0.)));
        save_place(tab, genre);
        self.load(cx);
    }

    /// Back on this page: undoes the last step here (menu, More Info, sources, a grid or search,
    /// then the tab / genre visited before, at its scroll position). False when there is none,
    /// and the shell leaves the page.
    fn local_back(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.genre_menu {
            self.genre_menu = false;
        } else if self.detail.is_some() {
            self.close_detail(cx);
        } else if !matches!(self.streams, StreamState::Idle) {
            close_streams(self, window, cx);
        } else if self.results.is_some() || self.expanded.is_some() {
            self.back_home(window, cx);
        } else if let Some(p) = self.places.pop() {
            self.tab = p.tab;
            self.genre = p.genre;
            save_place(p.tab, p.genre);
            self.load(cx);
            // Rows load from the cache within a frame or two; the old scroll position is put
            // back once they are in, or it would be clamped to an empty page.
            self.pending_scroll = Some((p.scroll, p.offsets));
        } else {
            return false;
        }
        self.hov.close();
        cx.notify();
        true
    }

    /// Billboard picks: the first top row's titles that have artwork and a synopsis.
    fn featured(&self) -> Vec<AnimeSearchResult> {
        let src = self.rows.iter().find(|r| r.look == Look::Top10).and_then(|r| r.load.ready());
        src.map(|v| v.iter().filter(|r| r.description.is_some() && r.imdb_id.is_some()).take(6).cloned().collect()).unwrap_or_default()
    }

    fn start(&mut self, label: String, base: PlayBase, movie: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Remember what had focus only when the overlay opens (Try again keeps the first one).
        if matches!(self.streams, StreamState::Idle) {
            self.return_focus = window.focused(cx).filter(|f| *f != self.focus);
        }
        self.streams = StreamState::Searching(label.clone());
        self.last_play = Some((label.clone(), base.clone(), movie));
        window.focus(&self.focus, cx);
        if let Some(imdb) = &base.imdb_id {
            fetch(cx, mark_watching(media_id(if movie { "movie" } else { "series" }, imdb)), |_: &mut Self, _| {});
        }
        self.lookup = Some(streams::find(cx, window, label, base, movie, true, |v: &mut Self, s| {
            v.streams = if matches!(s, StreamState::Picking { .. }) { StreamState::Idle } else { s }
        }));
        cx.notify();
    }

    fn resume_of(&self, r: &AnimeSearchResult) -> Option<&PlaybackState> {
        let id = media_id(kind_of(r), r.imdb_id.as_deref().unwrap_or(&r.id));
        self.cont.iter().find(|h| h.media_id == id)
    }

    /// Hero Play: resumes where the user left off when that is known, otherwise starts at the top.
    fn play(&mut self, r: &AnimeSearchResult, window: &mut Window, cx: &mut Context<Self>) {
        let movie = kind_of(r) == "movie";
        let point = if movie { None } else { r.imdb_id.as_deref().and_then(|id| series_point(id, self.resume_of(r).and_then(|h| h.episode_number))) };
        let (s, e) = point.unwrap_or((1, 1));
        let label = if movie { r.title.clone() } else { format!("{} S{s:02}E{e:02}", r.title) };
        let base = PlayBase { title: label.clone(), imdb_id: r.imdb_id.clone(), mal_id: None, season: (!movie).then_some(s), episode: (!movie).then_some(e) };
        self.start(label, base, movie, window, cx);
    }

    /// Where a Continue Watching card resumes: a movie, or a series episode whose season is known.
    fn resume_point(h: &PlaybackState) -> Option<(PlayBase, String, bool)> {
        let imdb = h.media_id.rsplit(':').next().filter(|id| id.starts_with("tt"))?.to_string();
        if h.kind == "movie" {
            return Some((PlayBase { title: h.title.clone(), imdb_id: Some(imdb), mal_id: None, season: None, episode: None }, h.title.clone(), true));
        }
        let (s, e) = series_point(&imdb, h.episode_number)?;
        let label = format!("{} S{s:02}E{e:02}", h.title);
        Some((PlayBase { title: label.clone(), imdb_id: Some(imdb), mal_id: None, season: Some(s), episode: Some(e) }, label, false))
    }

    fn expand(&mut self, id: &'static str, cx: &mut Context<Self>) {
        let Some((title, src, items)) = self.rows.iter().find(|r| r.id == id).map(|r| (r.title.clone(), r.src, r.load.ready().cloned().unwrap_or_default())) else { return };
        self.home_offset = Some(self.scroll.offset());
        self.expanded = Some(Expanded { id, title, src, done: items.len() < CINEMETA_PAGE, items, page: 1, loading: false, error: None });
        self.scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        let Some(e) = self.expanded.as_mut() else { return };
        if e.loading || e.done {
            return;
        }
        e.loading = true;
        e.error = None;
        let ((movie, cat, genre), page, id) = (e.src, e.page + 1, e.id);
        cx.notify();
        fetch(cx, catalog(movie, cat, genre, page), move |v: &mut Self, l| {
            let Some(e) = v.expanded.as_mut().filter(|e| e.id == id) else { return };
            e.loading = false;
            match l {
                Load::Ready(more) => {
                    e.done = more.len() < CINEMETA_PAGE;
                    e.page = page;
                    let fresh: Vec<_> = more.into_iter().filter(|m| !e.items.iter().any(|x| x.id == m.id)).collect();
                    e.items.extend(fresh);
                }
                Load::Failed(err) => e.error = Some(err),
                Load::Loading => {}
            }
        });
    }

    /// Netflix's billboard keeps one shape at every width (about 0.46 of its width tall).
    fn hero_height(width: f32) -> f32 {
        ((width - EDGE * 2.) * 0.46).clamp(340., 880.)
    }

    fn hero(&self, width: f32, cx: &mut Context<Self>) -> AnyElement {
        let items = self.featured();
        let height = Self::hero_height(width);
        let copy_w = (width - EDGE * 2.) * 0.42;
        if items.is_empty() {
            return nfx::billboard("movies-hero", height, None, None).child(nfx::hero_copy(copy_w).child(look::hero_skeleton())).into_any_element();
        }
        let n = items.len();
        let current = self.hero % n;
        let r = items[current].clone();
        let under = self.hero_prev.map(|p| p % n).filter(|p| *p != current).and_then(|p| background_of(&items[p]));
        let movie = kind_of(&r) == "movie";
        // "Film • Comedy • 2017", as Netflix's billboard line.
        let mut parts = vec![if movie { "Film".to_string() } else { "Series".to_string() }];
        parts.extend(r.genres.clone().unwrap_or_default().into_iter().take(2));
        parts.extend(r.year.map(|y| y.to_string()));
        let meta = nfx::dot_line(parts, 16.).children(look::score(r.score, isz::SM, None));
        // "Resume" only when Play really resumes: a series needs its season (see `play`), or it
        // starts at S1 E1 and the label would be wrong.
        let resume = self.resume_of(&r).is_some_and(|h| movie || r.imdb_id.as_deref().and_then(|id| series_point(id, h.episode_number)).is_some());
        let (play_r, info_r) = (r.clone(), r.clone());
        let paused = self.hero_paused;
        let pause = (!reduced()).then(|| {
            look::glass_button("hero-pause", if paused { ph::PLAY } else { ph::PAUSE }, if paused { "Play slideshow" } else { "Pause slideshow" }, 28.).on_click(cx.listener(|this, _, _, cx| {
                this.hero_paused = !this.hero_paused;
                cx.notify();
            }))
        });
        nfx::billboard(("movies-hero", current), height, background_of(&r), under)
            .child(look::bounds_probe(&self.hero_bounds))
            .child(
                nfx::hero_copy(copy_w)
                    .child(nfx::hero_logo(&r.title, logo_of(&r), (height * 0.26).clamp(80., 200.)))
                    .child(meta)
                    .child(div().text_size(px(18.)).line_height(px(26.)).text_color(Tokens::text_1()).line_clamp(3).child(SharedString::from(r.description.as_deref().map(look::blurb).unwrap_or_default())))
                    .child(
                        div()
                            .flex()
                            .gap(px(10.))
                            .mt(px(2.))
                            .child(nfx::play_pill("hero-play", if resume { "Resume" } else { "Play" }).on_click(cx.listener(move |this, _, w, cx| this.play(&play_r, w, cx))))
                            .child(nfx::info_pill("hero-info", "More Info").on_click(cx.listener(move |this, _, w, cx| this.open_detail(&info_r, w, cx)))),
                    ),
            )
            .child(div().absolute().right(px(28.)).bottom(px(30.)).flex().items_center().gap(px(6.)).children(pause).child(look::dots(cx, n, current, |v: &mut Self, i| v.go_hero(i))))
            .into_any_element()
    }

    /// The title's detail ("More Info").
    fn open_detail(&mut self, r: &AnimeSearchResult, window: &mut Window, cx: &mut Context<Self>) {
        let route = Route::MovieDetail { imdb_id: r.imdb_id.clone().unwrap_or_else(|| r.id.clone()), kind: kind_of(r).into(), title: r.title.clone() };
        self.open_route(&route, window, cx);
    }

    /// A title's detail opens in the modal over the page (a "More Like This" card in it replaces
    /// it); any other route is navigated to.
    fn open_route(&mut self, route: &Route, window: &mut Window, cx: &mut Context<Self>) {
        self.hov.close();
        let Route::MovieDetail { imdb_id, kind, title } = route else {
            navigate(route.clone(), window, cx);
            return;
        };
        let weak = cx.weak_entity();
        let (id, kind, title) = (imdb_id.clone(), kind.clone(), title.clone());
        self.detail = Some(cx.new(|cx| {
            MovieDetail::new_modal(id, kind, title, cx).on_open(move |route, w, cx| {
                let _ = weak.update(cx, |v, cx| v.open_route(route, w, cx));
            })
        }));
        window.focus(&self.detail_focus, cx);
        cx.notify();
    }

    fn close_detail(&mut self, cx: &mut Context<Self>) {
        // Dropping the view cancels its source lookup.
        self.detail = None;
        cx.notify();
    }

    fn in_my_list(&self, r: &AnimeSearchResult) -> bool {
        let id = r.imdb_id.as_deref().unwrap_or(&r.id);
        self.my_list.iter().any(|m| m.imdb_id.as_deref().unwrap_or(&m.id) == id)
    }

    /// My List from a hover card's + / check disc, with the Undo toast of the detail page.
    fn toggle_list(&mut self, r: &AnimeSearchResult, cx: &mut Context<Self>) {
        if self.saving_list {
            return;
        }
        let Some(imdb) = r.imdb_id.clone() else { return };
        let add = !self.in_my_list(r);
        let t = media_title(kind_of(r), &imdb, r, &r.title);
        self.saving_list = true;
        let undo_title = t.clone();
        cx.spawn(async move |this, cx| {
            let res = set_watchlist(t, add).await;
            let _ = this.update(cx, |v, cx| {
                v.saving_list = false;
                match res {
                    // The list row reloads itself: `set_watchlist` moved the list epoch.
                    Ok(before) => announce_list_toggle(cx, add, before, undo_title, |_: &mut Self, _| {}),
                    Err(e) => toast::show(cx, Toast::error(format!("My List could not be updated: {e}"))),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Pointer moved over the page: open the hover card of the card it rests on (after
    /// `POP_DELAY`), keep it while the pointer is on it, close it once the pointer leaves.
    fn track_hover(&mut self, pos: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if self.hov.shown.is_some() && look::pointer_over(&self.hov.popup, window) {
            return;
        }
        let at = self.hov.card_at(pos, self.scroll.bounds());
        if at.is_some() && at == self.hov.shown {
            return;
        }
        if self.hov.shown.is_some() {
            self.hov.shown = None;
            self.hov.popup.set(None);
            cx.notify();
        }
        if at == self.hov.pending {
            return;
        }
        self.hov.pending = at;
        self.hov.generation += 1;
        let Some(key) = at else { return };
        if !matches!(self.streams, StreamState::Idle) {
            return;
        }
        let generation = self.hov.generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(nfx::POP_DELAY).await;
            let _ = this.update(cx, |v, cx| {
                if v.hov.generation == generation && v.hov.pending == Some(key) {
                    v.hov.shown = Some(key);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The title behind a card key, for the hover card.
    fn item_of(&self, key: CardKey) -> Option<AnimeSearchResult> {
        match key.0 {
            "mylist" => self.my_list.get(key.1).cloned(),
            "res" => self.results.as_ref().and_then(|(_, l)| l.ready()).and_then(|v| v.get(key.1)).cloned(),
            "all" => self.expanded.as_ref().and_then(|e| e.items.get(key.1)).cloned(),
            id => self.rows.iter().find(|r| r.id == id).and_then(|r| r.load.ready()).and_then(|v| v.get(key.1)).cloned(),
        }
    }

    /// The open hover card, if any, positioned over its card.
    fn hover_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let key = self.hov.shown?;
        let anchor = self.hov.bounds_of(key)?;
        let page = self.scroll.bounds();
        let generation = self.hov.generation;
        if key.0 == "cont" {
            let h = self.cont.get(key.1)?.clone();
            let imdb = h.media_id.rsplit(':').next().filter(|id| id.starts_with("tt")).map(str::to_string);
            let image = imdb.as_ref().map(|id| format!("https://images.metahub.space/background/medium/{id}/img")).or_else(|| h.cover_url.clone());
            let logo = imdb.as_ref().map(|id| format!("https://images.metahub.space/logo/medium/{id}/img"));
            let resume = Self::resume_point(&h);
            let route = route_for_media(&h.media_id, &h.title);
            let episode = resume.as_ref().and_then(|(b, _, _)| b.season.zip(b.episode)).map(|(s, e)| format!("S{s}:E{e}")).or_else(|| h.episode_number.map(|e| format!("Episode {e}")));
            let total = (h.duration_secs / 60.).round() as u64;
            let watched = (h.position_secs / 60.).round() as u64;
            let actions = div()
                .flex()
                .items_center()
                .gap(px(8.))
                .when_some(resume, |d, (base, label, movie)| d.child(nfx::disc("pop-play", ph::PLAY, "Resume", true).on_click(cx.listener(move |this, _, w, cx| {
                    this.hov.close();
                    this.start(label.clone(), base.clone(), movie, w, cx);
                }))))
                .child(div().flex_1())
                .when_some(route, |d, route| d.child(nfx::disc("pop-info", ph::CARET_DOWN, "More Info", false).on_click(cx.listener(move |this, _, w, cx| this.open_route(&route, w, cx)))));
            let body = div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(actions)
                .when_some(episode, |d, e| d.child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(SharedString::from(e))))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(div().flex_1().h(px(3.)).bg(Tokens::track()).child(div().h_full().w(relative(fraction(&h).clamp(0.02, 1.))).bg(Tokens::accent())))
                        .child(div().text_size(px(13.)).text_color(Tokens::text_2()).child(SharedString::from(format!("{watched} of {total}m")))),
                );
            return Some(nfx::hover_card(&self.hov.popup, generation, anchor, page, image, logo, &h.title, body));
        }
        let r = self.item_of(key)?;
        let movie = kind_of(&r) == "movie";
        let saved = self.in_my_list(&r);
        let (play_r, list_r, info_r) = (r.clone(), r.clone(), r.clone());
        let actions = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(nfx::disc("pop-play", ph::PLAY, if self.resume_of(&r).is_some() { "Resume" } else { "Play" }, true).on_click(cx.listener(move |this, _, w, cx| {
                this.hov.close();
                this.play(&play_r, w, cx);
            })))
            .when(r.imdb_id.is_some(), |d| {
                d.child(nfx::disc("pop-list", if saved { ph::CHECK } else { ph::PLUS }, if saved { "Remove from My List" } else { "Add to My List" }, false).on_click(cx.listener(move |this, _, _, cx| this.toggle_list(&list_r, cx))))
            })
            .child(div().flex_1())
            .child(nfx::disc("pop-info", ph::CARET_DOWN, "More Info", false).on_click(cx.listener(move |this, _, w, cx| this.open_detail(&info_r, w, cx))));
        let meta = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(14.))
            .text_color(Tokens::text_2())
            .child(div().child(if movie { "Film" } else { "Series" }))
            .when_some(r.year, |d, y| d.child(nfx::box_tag(y.to_string())))
            .children(look::score(r.score, isz::XS, None))
            .child(nfx::box_tag("HD"));
        let genres = r.genres.clone().unwrap_or_default().into_iter().take(3).collect::<Vec<_>>();
        let body = div().flex().flex_col().gap(px(10.)).child(actions).child(meta).when(!genres.is_empty(), |d| d.child(nfx::dot_line(genres, 13.)));
        Some(nfx::hover_card(&self.hov.popup, generation, anchor, page, background_of(&r), logo_of(&r), &r.title, body))
    }

    /// "Movies › Comedies  [Genres ▾]" over the billboard of the Movies and Series tabs, as on
    /// Netflix; the menu lists Cinemeta's genres.
    fn genre_header(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.tab == Tab::Home {
            return None;
        }
        let movie = self.tab == Tab::Movies;
        let (tab, open, current) = (self.tab, self.genre_menu, self.genre);
        let noun = if movie { "Movies" } else { "Series" };
        let pill = div()
            .id("genre-pill")
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(36.))
            .pl(px(16.))
            .pr(px(12.))
            .rounded_full()
            .border_1()
            .border_color(Tokens::text_1().alpha(if open { 0.8 } else { 0.4 }))
            .bg(Tokens::base().alpha(0.5))
            .text_size(px(15.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(Tokens::text_1())
            .cursor_pointer()
            .role(Role::Button)
            .aria_label("Genres")
            .aria_expanded(open)
            .pressable()
            .hover(|s| s.border_color(Tokens::text_1().alpha(0.8)))
            .focus_ring()
            .child("Genres")
            .child(icon(if open { ph::CARET_UP.fill() } else { ph::CARET_DOWN.fill() }, Tokens::text_1(), isz::SM))
            .on_click(cx.listener(|this, _, _, cx| {
                this.genre_menu = !this.genre_menu;
                this.hov.close();
                cx.notify();
            }));
        let item = |ix: usize, label: String, genre: Option<&'static str>, cx: &mut Context<Self>| {
            let on = genre == current;
            div()
                .id(("genre-item", ix))
                .flex()
                .items_center()
                .gap(px(10.))
                .h(px(42.))
                .px(px(18.))
                .text_size(px(15.))
                .font_weight(if on { FontWeight::BOLD } else { FontWeight::SEMIBOLD })
                .text_color(if on { Tokens::text_1() } else { Tokens::text_2() })
                .cursor_pointer()
                .role(Role::MenuItemRadio)
                .aria_selected(on)
                .hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
                .focus_ring()
                .child(SharedString::from(label))
                .when(on, |d| d.child(div().flex_1()).child(icon(ph::CHECK.bold(), Tokens::accent(), isz::SM)))
                .on_click(cx.listener(move |this, _, w, cx| {
                    if this.genre == genre {
                        this.genre_menu = false;
                        cx.notify();
                    } else {
                        this.go_to(tab, genre, w, cx);
                    }
                }))
        };
        let menu = open.then(|| {
            let mut list = div()
                .id("genre-menu")
                .w(px(300.))
                .max_h(px(460.))
                .overflow_y_scroll()
                .py(px(8.))
                .rounded(px(radius::SM))
                .bg(Tokens::popover())
                .border_1()
                .border_color(Tokens::border())
                .shadow_lg()
                .occlude()
                .role(Role::Menu)
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.genre_menu = false;
                    cx.notify();
                }))
                .child(item(0, format!("All {noun}"), None, cx));
            for (i, (g, label)) in genres_for(movie).enumerate() {
                list = list.child(item(i + 1, label.to_string(), Some(g), cx));
            }
            deferred(anchored().anchor(Anchor::TopLeft).offset(point(px(0.), px(6.))).snap_to_window_with_margin(px(8.)).child(list)).with_priority(2)
        });
        let title = div()
            .id("genre-title")
            .text_size(px(34.))
            .line_height(px(40.))
            .font_weight(FontWeight::BOLD)
            .text_color(if current.is_some() { Tokens::text_2() } else { Tokens::text_1() })
            .when(current.is_some(), |d| d.cursor_pointer().hover(|s| s.text_color(Tokens::text_1())).on_click(cx.listener(move |this, _, w, cx| this.go_to(tab, None, w, cx))))
            .child(noun);
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .px(px(EDGE))
                .mb(px(16.))
                .child(title)
                .when_some(current, |d, g| {
                    d.child(icon(ph::CARET_RIGHT.bold(), Tokens::text_3(), isz::LG)).child(div().text_size(px(34.)).line_height(px(40.)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child(genre_label(g, movie)))
                })
                .child(div().relative().ml(px(10.)).child(pill).children(menu)),
        )
    }

    /// Brand, tabs and search over the top of the billboard; solid once the page scrolled.
    fn top_bar(&self, cx: &mut Context<Self>) -> Div {
        let scrolled = self.results.is_some() || self.expanded.is_some() || f32::from(self.scroll.offset().y) < -8.0;
        let tab = |id: &'static str, label: &'static str, t: Tab, cur: Tab, cx: &mut Context<Self>| {
            nfx::nav_tab(id, label, t == cur && self.results.is_none() && self.expanded.is_none()).on_click(cx.listener(move |this, _, w, cx| {
                if this.tab != t || this.genre.is_some() || this.results.is_some() || this.expanded.is_some() {
                    this.go_to(t, None, w, cx);
                }
            }))
        };
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(64.))
            .px(px(EDGE))
            .flex()
            .items_center()
            .gap(px(4.))
            .child(nfx::bar_backdrop(scrolled))
            // Cards scrolled under the bar must not take its hovers and clicks; the wheel still scrolls.
            .block_mouse_except_scroll()
            .child(
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .mr(px(18.))
                    .child(icon(glyphs::MOVIES.fill(), Tokens::accent(), isz::LG))
                    .child(div().ramp(ramp::TITLE_3).font_weight(FontWeight::BLACK).text_color(Tokens::accent()).child("MOVIES")),
            )
            .child(tab("tab-home", "Home", Tab::Home, self.tab, cx))
            .child(tab("tab-movies", "Movies", Tab::Movies, self.tab, cx))
            .child(tab("tab-series", "Series", Tab::Series, self.tab, cx))
            .child(div().flex_1())
            .child(div().w(px(280.)).child(common::search_field(&self.search)))
    }

    fn rows(&self, width: f32, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = vec![];
        let (per, card) = nfx::card_fit(width);
        let card_h = card * 9. / 16.;
        // Titles in a Top 10 list wear the "TOP 10" mark in every other row too.
        let top: HashSet<String> = self.rows.iter().filter(|r| r.look == Look::Top10).filter_map(|r| r.load.ready()).flat_map(|v| v.iter().take(10).map(|r| r.id.clone())).collect();
        if !self.cont.is_empty() {
            let cont = &self.cont;
            let hov = &self.hov;
            let make = |i: usize, cx: &mut Context<Self>| -> AnyElement {
                let h = &cont[i];
                let imdb = h.media_id.rsplit(':').next().filter(|id| id.starts_with("tt"));
                let image = imdb.map(|id| format!("https://images.metahub.space/background/medium/{id}/img")).or_else(|| h.cover_url.clone());
                let logo = imdb.map(|id| format!("https://images.metahub.space/logo/medium/{id}/img"));
                let el = nfx::card(hov, ("cont", i), card, image, logo, &h.title, false, None, Some(fraction(h)));
                match (Self::resume_point(h), route_for_media(&h.media_id, &h.title)) {
                    (Some((base, label, movie)), _) => el.on_click(cx.listener(move |this, _, w, cx| {
                        this.hov.close();
                        this.start(label.clone(), base.clone(), movie, w, cx);
                    })),
                    (None, Some(route)) => el.on_click(move |_, w, cx| navigate(route.clone(), w, cx)),
                    (None, None) => el,
                }
                .into_any_element()
            };
            let history: OnClick = std::rc::Rc::new(|_, w, cx| navigate(Route::MediaHistory, w, cx));
            out.push(nfx::row(cx, "cont", "Continue Watching", cont.len(), card, per, self.slide("cont"), make, set_offset, Some(history)).into_any_element());
        }
        if !self.my_list.is_empty() {
            let list: OnClick = std::rc::Rc::new(|_, w, cx| navigate(Route::Favorites, w, cx));
            let make = |i: usize, cx: &mut Context<Self>| self.card(Look::Landscape, ("mylist", i), i, card, &self.my_list[i], top.contains(&self.my_list[i].id), cx);
            out.push(nfx::row(cx, "mylist", "My List", self.my_list.len(), card, per, self.slide("mylist"), make, set_offset, Some(list)).into_any_element());
        }
        for row in &self.rows {
            let top10 = row.look == Look::Top10;
            // Top 10: the numeral and a poster in about a boxart's width.
            let (w, h) = if top10 { (card, card * TOP10_H) } else { (card, card_h) };
            match &row.load {
                Load::Loading => out.push(nfx::skeleton_row(&row.title, w, h, per + 1).into_any_element()),
                // A row that fails or comes back empty is simply left out.
                Load::Failed(_) => {}
                Load::Ready(v) if v.is_empty() => {}
                Load::Ready(v) => {
                    let n = v.len().min(if top10 { 10 } else { 40 });
                    let id = row.id;
                    let make = |i: usize, cx: &mut Context<Self>| self.card(row.look, (id, i), i, card, &v[i], !top10 && top.contains(&v[i].id), cx);
                    let explore: Option<OnClick> = (!top10).then(|| std::rc::Rc::new(cx.listener(move |this, _, _, cx| this.expand(id, cx))) as OnClick);
                    out.push(nfx::row(cx, row.id, row.title.clone(), n, w, per, self.slide(row.id), make, set_offset, explore).into_any_element());
                }
            }
        }
        if !self.rows.is_empty() && self.rows.iter().all(|r| matches!(r.load, Load::Failed(_))) {
            let raw = match &self.rows[0].load {
                Load::Failed(e) => e.clone(),
                _ => String::new(),
            };
            out.push(
                div()
                    .px(px(EDGE))
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(error_view("Check your connection, then try again.").title("Movies could not be loaded").retry(cx.listener(|this, _, _, cx| this.load(cx))))
                    .child(div().w(px(420.)).child(disclosure("movies-error-details", "Details").content(muted(raw))))
                    .into_any_element(),
            );
        }
        out
    }

    fn offset(&self, id: &'static str) -> usize {
        self.offsets.get(id).copied().unwrap_or(0)
    }

    /// (from, to) of a row's slide; `from == to` once the slide has had its time.
    fn slide(&self, id: &'static str) -> (usize, usize) {
        let to = self.offset(id);
        match self.slides.get(id) {
            Some((from, at)) if at.elapsed() < nfx::SLIDE => (*from, to),
            _ => (to, to),
        }
    }

    fn card(&self, look_: Look, key: CardKey, rank: usize, w: f32, r: &AnimeSearchResult, top10: bool, cx: &mut Context<Self>) -> AnyElement {
        let item = r.clone();
        let card = match look_ {
            Look::Top10 => {
                let h = w * TOP10_H;
                look::top10_card(key, rank + 1, r.cover_url.as_deref().map(look::hi_res), h, &r.title).relative().child(self.hov.probe(key))
            }
            Look::Landscape => nfx::card(&self.hov, key, w, background_of(r), logo_of(r), &r.title, top10, None, None),
        };
        card.on_click(cx.listener(move |this, _, w, cx| this.open_detail(&item, w, cx))).into_any_element()
    }

    fn grid(&self, width: f32, items: &[AnimeSearchResult], prefix: &'static str, cx: &mut Context<Self>) -> Div {
        let (_, w) = nfx::card_fit(width);
        div().flex().flex_wrap().gap_x(px(nfx::GAP)).gap_y(px(28.)).children(items.iter().enumerate().map(|(i, r)| self.card(Look::Landscape, (prefix, i), i, w, r, false, cx)))
    }

    fn grid_header(&self, title: String, count: Option<usize>, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .mb(px(20.))
            .child(div().child(btn("back-home", "Back to Home", ButtonKind::Ghost).small().icon(glyphs::BACK).on_click(cx.listener(|this, _, w, cx| this.back_home(w, cx)))))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(10.))
                    .child(div().ramp(ramp::TITLE_3).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child(SharedString::from(title)))
                    .when_some(count, |d, n| d.child(muted(if n == 1 { "1 title".to_string() } else { format!("{n} titles") }))),
            )
    }

    fn search_results(&self, q: &str, load: &Load<Vec<AnimeSearchResult>>, width: f32, cx: &mut Context<Self>) -> Div {
        let body = match load {
            Load::Loading => div().flex().flex_wrap().gap(px(8.)).children((0..12).map(|_| mv_ui::skeleton(px(300.), px(169.), mv_ui::theme::radius::MD))).into_any_element(),
            Load::Failed(e) => error_view(e.clone())
                .title("Search failed")
                .retry(cx.listener(|this, _, _, cx| {
                    this.results = None;
                    this.run_search(cx);
                }))
                .into_any_element(),
            Load::Ready(v) if v.is_empty() => empty_view(ph::MAGNIFYING_GLASS_MINUS, format!("No matches for \"{q}\""), "Check the spelling, or search for the original title.").into_any_element(),
            Load::Ready(v) => self.grid(width, v, "res", cx).into_any_element(),
        };
        div().pt(px(84.)).px(px(EDGE)).child(self.grid_header(format!("Results for \"{q}\""), load.ready().map(Vec::len), cx)).child(body)
    }

    fn expanded_view(&self, e: &Expanded, width: f32, cx: &mut Context<Self>) -> Div {
        div()
            .pt(px(84.))
            .px(px(EDGE))
            .child(self.grid_header(e.title.clone(), Some(e.items.len()), cx))
            .child(self.grid(width, &e.items, "all", cx))
            .when_some(e.error.clone(), |d, err| {
                d.child(div().mt(px(20.)).child(inline_alert(AlertKind::Error, "More titles could not be loaded", err).action("Try again", cx.listener(|this, _, _, cx| this.load_more(cx)))))
            })
            .when(!e.done, |d| {
                d.child(
                    div().mt(px(28.)).flex().justify_center().child(btn("all-more", "Load more", ButtonKind::Secondary).icon(ph::CARET_DOWN).loading(e.loading).on_click(cx.listener(|this, _, _, cx| this.load_more(cx)))),
                )
            })
    }
}

fn set_offset(v: &mut MoviesHome, id: &'static str, n: usize) {
    let from = v.offset(id);
    v.slides.insert(id, (from, Instant::now()));
    v.offsets.insert(id, n);
    v.hov.close();
}

impl Render for MoviesHome {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.load_personal(cx);
        let width = look::page_width(&self.scroll, window);
        // The hover card is placed from last frame's card bounds; this frame's probes refill them.
        let hover = if matches!(self.streams, StreamState::Idle) && self.detail.is_none() { self.hover_card(cx) } else { None };
        self.hov.begin_frame();
        // Back returned to a tab or genre: its scroll position and row pages come back once its
        // rows are in.
        if self.pending_scroll.is_some() && !self.rows.iter().any(|r| matches!(r.load, Load::Loading)) {
            if let Some((at, offsets)) = self.pending_scroll.take() {
                self.offsets = offsets;
                self.scroll.set_offset(at);
            }
        }
        let content = if let Some(e) = &self.expanded {
            div().pb(px(40.)).child(self.expanded_view(e, width, cx))
        } else if let Some((q, load)) = &self.results {
            div().pb(px(40.)).child(self.search_results(q, load, width, cx))
        } else {
            let rows = self.rows(width, cx);
            div().pt(px(72.)).pb(px(40.)).children(self.genre_header(cx)).child(self.hero(width, cx)).child(div().mt(px(36.)).relative().children(rows))
        };
        let panel = streams::panel(&self.streams, cx, close_streams, retry_lookup);
        let weak = cx.weak_entity();
        div()
            .relative()
            .size_full()
            .bg(Tokens::base())
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, w, cx| this.track_hover(ev.position, w, cx)))
            // Netflix closes a hover card on scroll; the bar also turns solid as the page moves.
            .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                this.hov.close();
                cx.notify();
            }))
            .child(mv_ui::scroll_page("movies-home", &self.scroll).child(content))
            .child(self.top_bar(cx))
            .children(hover)
            .children(self.detail.clone().map(|e| {
                let weak = cx.weak_entity();
                detail_modal(&self.detail_focus, e, width, move |_, cx| {
                    let _ = weak.update(cx, |v, cx| v.close_detail(cx));
                })
            }))
            .children(panel.map(|p| {
                look::overlay(&self.focus, p, move |w, cx| {
                    let _ = weak.update(cx, |v, cx| {
                        close_streams(v, w, cx);
                        cx.notify();
                    });
                })
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::{catalog_key, kind_of};
    use mv_media::cinemeta::CinemetaCatalog;
    use mv_media::types::AnimeSearchResult;

    #[test]
    fn catalog_keys_match_the_warm_up() {
        assert_eq!(catalog_key(true, CinemetaCatalog::Top, Some("Action")), "cinemeta:movie:top:Action:1");
        assert_eq!(catalog_key(false, CinemetaCatalog::Popular, None), "cinemeta:series:popular::1");
    }

    #[test]
    fn kind_follows_the_type() {
        let movie = AnimeSearchResult { anime_type: Some("Movie".into()), ..Default::default() };
        assert_eq!(kind_of(&movie), "movie");
        assert_eq!(kind_of(&AnimeSearchResult::default()), "series");
    }
}
