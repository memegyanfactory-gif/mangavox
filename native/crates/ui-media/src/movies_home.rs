//! Movies home, Netflix-style: rotating billboard, continue watching, Top 10 rows,
//! trending / acclaimed / genre rows, "See all" grids and search. Data comes from Cinemeta.
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::media::{self, PlaybackState};
use mv_core::repo::watchlist;
use mv_media::cinemeta::{CinemetaCatalog, CinemetaClient};
use mv_media::types::AnimeSearchResult;
use mv_ui::motion::reduced;
use mv_ui::theme::{icon as isz, ramp};
use mv_ui::{btn, chip, disclosure, empty_view, error_view, glyphs, icon, inline_alert, muted, navigate, ph, size, AlertKind, ButtonKind, RampExt, Route, Tokens};

use crate::common::{self, db, fetch, fetch_cached, fraction, mark_watching, media_id, route_for_media, Load, DAY, SEARCH_DEBOUNCE};
use crate::look::{self, Hover, OnClick, RowOffsets, EDGE};
use crate::streams::{self, PlayBase, StreamState};

/// Genre rows shown on the home page, in order.
const ROW_GENRES: &[&str] = &["Action", "Comedy", "Sci-Fi", "Thriller", "Animation", "Horror", "Romance", "Documentary"];

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
    Poster,
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

pub struct MoviesHome {
    tab: Tab,
    rows: Vec<Row>,
    cont: Vec<PlaybackState>,
    my_list: Vec<AnimeSearchResult>,
    hero: usize,
    hero_prev: Option<usize>,
    /// Where the hero was painted; the carousel holds still while the pointer is inside.
    hero_bounds: look::BoundsSlot,
    hero_paused: bool,
    offsets: RowOffsets,
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
        let mut s = Self {
            tab: Tab::Home,
            rows: vec![],
            cont: vec![],
            my_list: vec![],
            hero: 0,
            hero_prev: None,
            hero_bounds: Default::default(),
            hero_paused: false,
            offsets: Default::default(),
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
            v.push(("acclaimed", "Critically Acclaimed Movies".to_string(), Look::Poster, true, CinemetaCatalog::ImdbRating, None));
        }
        if tab != Tab::Movies {
            v.push(("binge", "Binge-worthy Series".to_string(), Look::Landscape, false, CinemetaCatalog::Popular, None));
            v.push(("acclaimed-tv", "Critically Acclaimed TV".to_string(), Look::Poster, false, CinemetaCatalog::ImdbRating, None));
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
        self.rows = Self::plan(self.tab)
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
        if look::pointer_over(&self.hero_bounds, window) || self.hero_paused || reduced() || !matches!(self.streams, StreamState::Idle) || self.results.is_some() || self.expanded.is_some() {
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

    fn hero(&self, cx: &mut Context<Self>) -> AnyElement {
        let items = self.featured();
        let height = 540.;
        if items.is_empty() {
            return look::billboard("movies-hero", height, None, None).child(look::hero_copy(110.).child(look::hero_skeleton())).into_any_element();
        }
        let n = items.len();
        let current = self.hero % n;
        let r = items[current].clone();
        let under = self.hero_prev.map(|p| p % n).filter(|p| *p != current).and_then(|p| background_of(&items[p]));
        let movie = kind_of(&r) == "movie";
        let genres = r.genres.clone().unwrap_or_default().into_iter().take(3).collect::<Vec<_>>().join(" · ");
        let meta = look::meta_line(
            look::score(r.score, isz::MD, Some("IMDb")),
            vec![r.year.map(|y| y.to_string()).unwrap_or_default(), if movie { "Movie".into() } else { "Series".into() }, genres],
        );
        // "Resume" only when Play really resumes: a series needs its season (see `play`), or it
        // starts at S1 E1 and the label would be wrong.
        let resume = self.resume_of(&r).is_some_and(|h| movie || r.imdb_id.as_deref().and_then(|id| series_point(id, h.episode_number)).is_some());
        let (play_r, route) = (r.clone(), Route::MovieDetail { imdb_id: r.imdb_id.clone().unwrap_or_default(), kind: kind_of(&r).into(), title: r.title.clone() });
        let paused = self.hero_paused;
        let pause = (!reduced()).then(|| {
            look::glass_button("hero-pause", if paused { ph::PLAY } else { ph::PAUSE }, if paused { "Play slideshow" } else { "Pause slideshow" }, 28.).on_click(cx.listener(|this, _, _, cx| {
                this.hero_paused = !this.hero_paused;
                cx.notify();
            }))
        });
        look::billboard(("movies-hero", current), height, background_of(&r), under)
            .child(look::bounds_probe(&self.hero_bounds))
            .child(
                look::hero_copy(96.)
                    .child(look::hero_title(&r.title, logo_of(&r)))
                    .child(meta)
                    .child(div().text_size(px(size::LG)).line_height(px(22.)).text_color(Tokens::text_1()).line_clamp(3).child(SharedString::from(r.description.as_deref().map(look::blurb).unwrap_or_default())))
                    .child(
                        div()
                            .flex()
                            .gap(px(10.))
                            .mt(px(4.))
                            .child(look::hero_play("hero-play", if resume { "Resume" } else { "Play" }).on_click(cx.listener(move |this, _, w, cx| this.play(&play_r, w, cx))))
                            .child(look::hero_more("hero-info").on_click(move |_, w, cx| navigate(route.clone(), w, cx))),
                    ),
            )
            .child(div().absolute().right(px(EDGE - 5.)).bottom(px(104.)).flex().items_center().gap(px(6.)).children(pause).child(look::dots(cx, n, current, |v: &mut Self, i| v.go_hero(i))))
            .into_any_element()
    }

    /// Tabs + search, floating over the top of the billboard.
    fn top_bar(&self, cx: &mut Context<Self>) -> Div {
        let scrolled = self.results.is_some() || self.expanded.is_some() || f32::from(self.scroll.offset().y) < -40.0;
        let tab = |id: &'static str, label: &'static str, t: Tab, cur: Tab, cx: &mut Context<Self>| {
            chip(id, label, t == cur).on_click(cx.listener(move |this, _, w, cx| {
                if this.tab != t || this.results.is_some() || this.expanded.is_some() {
                    this.tab = t;
                    this.results = None;
                    this.expanded = None;
                    this.home_offset = None;
                    // Leaving search for a tab clears the query too (the field kept showing a
                    // search whose results were gone), and the new tab starts at its top.
                    this.search_gen += 1;
                    this.search.update(cx, |s, cx| s.set_value("", w, cx));
                    this.scroll.set_offset(point(px(0.), px(0.)));
                    this.load(cx);
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
            .gap(px(10.))
            .map(|d| if scrolled { d.bg(Tokens::base()).border_b_1().border_color(Tokens::border()) } else { d })
            // Cards scrolled under the bar must not take its hovers and clicks; the wheel still scrolls.
            .block_mouse_except_scroll()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .mr(px(12.))
                    .child(icon(glyphs::MOVIES.fill(), Tokens::accent(), isz::XL))
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
        let card = 300.;
        if !self.cont.is_empty() {
            let items = self
                .cont
                .iter()
                .enumerate()
                .filter_map(|(i, h)| {
                    let route = route_for_media(&h.media_id, &h.title)?;
                    let left = look::time_left(h.position_secs, h.duration_secs);
                    let resume = Self::resume_point(h);
                    // "S2 E5 · 12m left" when the season is known, as Netflix's resume line.
                    let episode = match resume.as_ref().and_then(|(b, _, _)| b.season.zip(b.episode)) {
                        Some((s, e)) => Some(format!("S{s} E{e}")),
                        None => h.episode_number.map(|e| format!("E{e}")),
                    };
                    let sub = [episode, left].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                    let imdb = h.media_id.rsplit(':').next().filter(|id| id.starts_with("tt"));
                    let image = imdb.map(|id| format!("https://images.metahub.space/background/medium/{id}/img")).or_else(|| h.cover_url.clone());
                    let hover = if resume.is_some() { Hover::Play } else { Hover::Open };
                    let card_el = look::landscape_card(("cont", i), card, image, &h.title, Some(sub).filter(|s| !s.is_empty()), Some(fraction(h)), hover);
                    let info_route = route.clone();
                    let card_el = match resume {
                        Some((base, label, movie)) => card_el
                            .on_click(cx.listener(move |this, _, w, cx| this.start(label.clone(), base.clone(), movie, w, cx)))
                            .child(div().absolute().top(px(8.)).right(px(8.)).child(look::glass_button(("cont-info", i), ph::INFO, "Details", 28.).on_click(move |_, w, cx| {
                                // The card under the button would resume playback.
                                cx.stop_propagation();
                                navigate(info_route.clone(), w, cx);
                            }))),
                        None => card_el.on_click(move |_, w, cx| navigate(route.clone(), w, cx)),
                    };
                    Some(card_el.into_any_element())
                })
                .collect();
            let history: OnClick = std::rc::Rc::new(|_, w, cx| navigate(Route::MediaHistory, w, cx));
            out.push(look::row(cx, "cont", "Continue Watching", self.offset("cont"), look::per_row(width, card, 8.), 8., items, set_offset, Some(history)).into_any_element());
        }
        if !self.my_list.is_empty() {
            let items = self.my_list.iter().enumerate().map(|(i, r)| self.card(Look::Landscape, ("mylist", i), i, r)).collect();
            let list: OnClick = std::rc::Rc::new(|_, w, cx| navigate(Route::Favorites, w, cx));
            out.push(look::row(cx, "mylist", "My List", self.offset("mylist"), look::per_row(width, card, 8.), 8., items, set_offset, Some(list)).into_any_element());
        }
        for row in &self.rows {
            let (w, h) = match row.look {
                Look::Landscape => (card, card * 9. / 16.),
                Look::Top10 => (217., 210.),
                Look::Poster => (180., 270.),
            };
            match &row.load {
                Load::Loading => out.push(look::skeleton_row(&row.title, w, h, 8).into_any_element()),
                // A row that fails or comes back empty is simply left out.
                Load::Failed(_) => {}
                Load::Ready(v) if v.is_empty() => {}
                Load::Ready(v) => {
                    let top10 = row.look == Look::Top10;
                    let items = v.iter().take(if top10 { 10 } else { 40 }).enumerate().map(|(i, r)| self.card(row.look, (row.id, i), i, r)).collect();
                    let id = row.id;
                    let see_all: Option<OnClick> = (!top10).then(|| std::rc::Rc::new(cx.listener(move |this, _, _, cx| this.expand(id, cx))) as OnClick);
                    out.push(look::row(cx, row.id, row.title.clone(), self.offset(row.id), look::per_row(width, w, 8.), 8., items, set_offset, see_all).into_any_element());
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

    fn card(&self, look_: Look, key: (&'static str, usize), rank: usize, r: &AnimeSearchResult) -> AnyElement {
        let route = Route::MovieDetail { imdb_id: r.imdb_id.clone().unwrap_or_else(|| r.id.clone()), kind: kind_of(r).into(), title: r.title.clone() };
        let card = match look_ {
            Look::Top10 => look::top10_card(key, rank + 1, r.cover_url.as_deref().map(look::hi_res), 210., &r.title),
            Look::Poster => look::poster_card(key, 180., r.cover_url.as_deref().map(look::hi_res), &r.title, r.year.map(|y| y.to_string()).unwrap_or_default(), r.score, r.description.as_deref().map(look::blurb), None),
            Look::Landscape => {
                let sub = r.genres.clone().unwrap_or_default().into_iter().take(2).collect::<Vec<_>>().join(" · ");
                look::landscape_card(key, 300., background_of(r), &r.title, Some(sub).filter(|s| !s.is_empty()), None, Hover::Open)
            }
        };
        card.on_click(move |_, w, cx| navigate(route.clone(), w, cx)).into_any_element()
    }

    fn grid(width: f32, items: &[AnimeSearchResult], prefix: &'static str) -> Div {
        let per = look::per_row(width, 300., 8.);
        let w = ((width - EDGE * 2.0 - 8.0 * (per as f32 - 1.0)) / per as f32).max(200.);
        div().flex().flex_wrap().gap_x(px(8.)).gap_y(px(24.)).children(items.iter().enumerate().map(|(i, r)| {
            let route = Route::MovieDetail { imdb_id: r.imdb_id.clone().unwrap_or_else(|| r.id.clone()), kind: kind_of(r).into(), title: r.title.clone() };
            let sub = [r.year.map(|y| y.to_string()), Some(if kind_of(r) == "movie" { "Movie".to_string() } else { "Series".to_string() })].into_iter().flatten().collect::<Vec<_>>().join(" · ");
            look::landscape_card((prefix, i), w, background_of(r), &r.title, Some(sub), None, Hover::Open).on_click(move |_, w, cx| navigate(route.clone(), w, cx))
        }))
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
            Load::Ready(v) => Self::grid(width, v, "res").into_any_element(),
        };
        div().pt(px(84.)).px(px(EDGE)).child(self.grid_header(format!("Results for \"{q}\""), load.ready().map(Vec::len), cx)).child(body)
    }

    fn expanded_view(&self, e: &Expanded, width: f32, cx: &mut Context<Self>) -> Div {
        div()
            .pt(px(84.))
            .px(px(EDGE))
            .child(self.grid_header(e.title.clone(), Some(e.items.len()), cx))
            .child(Self::grid(width, &e.items, "all"))
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
    v.offsets.insert(id, n);
}

impl Render for MoviesHome {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.load_personal(cx);
        let width = look::page_width(&self.scroll, window);
        let content = if let Some(e) = &self.expanded {
            div().pb(px(40.)).child(self.expanded_view(e, width, cx))
        } else if let Some((q, load)) = &self.results {
            div().pb(px(40.)).child(self.search_results(q, load, width, cx))
        } else {
            let rows = self.rows(width, cx);
            div().pb(px(40.)).child(self.hero(cx)).child(div().mt(px(-70.)).relative().children(rows))
        };
        let panel = streams::panel(&self.streams, cx, close_streams, retry_lookup);
        let weak = cx.weak_entity();
        div()
            .relative()
            .size_full()
            .bg(Tokens::base())
            .child(mv_ui::scroll_page("movies-home", &self.scroll).child(content))
            .child(self.top_bar(cx))
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
