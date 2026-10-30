//! Anime home, Crunchyroll-style: banner carousel, continue watching and poster rows
//! (airing, this season, top rated, movies, coming soon, genres), "See all" grids and search.
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::cache::DataCache;
use mv_core::repo::media::{self, PlaybackState};
use mv_media::types::AnimeSearchResult;
use mv_ui::motion::reduced;
use mv_ui::theme::{icon as isz, ramp};
use mv_ui::{btn, disclosure, empty_view, error_view, glyphs, icon, inline_alert, muted, navigate, ph, AlertKind, ButtonKind, RampExt, Route, Tokens};

use crate::common::{self, db, fetch, fetch_cached, fraction, jikan, mark_watching, media_id, route_for_media, subline, Load, DAY, SEARCH_DEBOUNCE};
use crate::look::{self, Hover, OnClick, RowOffsets, EDGE};
use crate::streams::{self, PlayBase, StreamState};

/// Jikan pages hold 25 titles.
const JIKAN_PAGE: usize = 25;

#[derive(Clone, Copy)]
pub(crate) enum Feed {
    Airing,
    Season,
    Top(Option<&'static str>),
    Upcoming,
    Genre(u32),
}

impl Feed {
    pub(crate) fn key(self) -> String {
        match self {
            Feed::Airing => "jikan:airing:1".into(),
            Feed::Season => "jikan:season:1".into(),
            Feed::Top(k) => format!("jikan:top:{}:1", k.unwrap_or("")),
            Feed::Upcoming => "jikan:upcoming:1".into(),
            Feed::Genre(g) => genre_key(g),
        }
    }

    pub(crate) async fn load(self, page: u32) -> Result<Vec<AnimeSearchResult>, String> {
        let j = jikan()?;
        let r = match self {
            Feed::Airing => j.airing_anime(page).await,
            Feed::Season => j.season_anime(None, None, page).await,
            Feed::Top(kind) => j.top_anime(page, kind).await,
            Feed::Upcoming => j.upcoming_anime(page).await,
            Feed::Genre(g) => j.anime_by_genre(g, page, None).await,
        };
        r.map_err(|e| e.to_string())
    }
}

pub fn genre_key(mal_genre: u32) -> String {
    format!("jikan:genre:{mal_genre}:1")
}

/// Fill the cache for the home rows, in display order (Jikan is rate limited).
pub async fn prefetch() {
    for &(_, _, feed) in ROWS {
        match DataCache::shared().get::<Vec<AnimeSearchResult>>(&feed.key()) {
            Some((_, age)) if age < DAY => {}
            _ => {
                let _ = common::cached(feed.key(), feed.load(1)).await;
            }
        }
    }
}

/// (row id, title, feed) in display order. The first row also feeds the carousel.
pub(crate) const ROWS: &[(&str, &str, Feed)] = &[
    ("airing", "Top Airing", Feed::Airing),
    ("season", "Popular This Season", Feed::Season),
    ("top", "Top Rated of All Time", Feed::Top(None)),
    ("action", "Action Packed", Feed::Genre(1)),
    ("movies", "Anime Movies", Feed::Top(Some("movie"))),
    ("isekai", "Isekai Adventures", Feed::Genre(62)),
    ("romance", "Romance", Feed::Genre(22)),
    ("upcoming", "Coming Soon", Feed::Upcoming),
];

/// A row opened with "See all": every title of the feed, a page at a time.
struct Expanded {
    id: &'static str,
    title: &'static str,
    feed: Feed,
    items: Vec<AnimeSearchResult>,
    page: u32,
    loading: bool,
    done: bool,
    error: Option<String>,
}

pub struct AnimeHome {
    rows: Vec<(&'static str, &'static str, Feed, Load<Vec<AnimeSearchResult>>)>,
    cont: Vec<PlaybackState>,
    hero: usize,
    /// Slide shown before the current one, drawn under it while the new art fades in.
    hero_prev: Option<usize>,
    /// Where the hero was painted; the carousel holds still while the pointer is inside.
    hero_bounds: look::BoundsSlot,
    hero_paused: bool,
    offsets: RowOffsets,
    search: Entity<InputState>,
    search_gen: u64,
    results: Option<(String, Load<Vec<AnimeSearchResult>>)>,
    expanded: Option<Expanded>,
    /// Scroll position of the home rows while a grid is open, restored on Back.
    home_offset: Option<Point<Pixels>>,
    streams: StreamState,
    /// The running source lookup; dropping it cancels the lookup.
    lookup: Option<Task<()>>,
    /// Last lookup, for Try again.
    last_play: Option<(String, PlayBase, bool)>,
    /// The source overlay takes focus when it opens, so Escape reaches it.
    focus: FocusHandle,
    /// What had focus before the overlay opened; it gets it back on close (WAI-ARIA dialog pattern).
    return_focus: Option<FocusHandle>,
    /// `common::progress_stamp()` when Continue Watching was read. The page is kept alive between
    /// visits, so it rereads the history after the player saved progress or History changed.
    watch_epoch: Option<(u64, u64)>,
    scroll: ScrollHandle,
    _sub: Subscription,
}

/// Anime movies are recorded with episode 1 too, so the type comes from the cached details.
fn is_movie(mal: &str) -> bool {
    DataCache::shared().get::<AnimeSearchResult>(&format!("jikan:anime:{mal}")).is_some_and(|(d, _)| d.anime_type.as_deref() == Some("Movie"))
}

fn close_streams(v: &mut AnimeHome, window: &mut Window, cx: &mut Context<AnimeHome>) {
    v.streams = StreamState::Idle;
    v.lookup = None;
    if let Some(f) = v.return_focus.take() {
        window.focus(&f, cx);
    }
}

fn retry_lookup(v: &mut AnimeHome, window: &mut Window, cx: &mut Context<AnimeHome>) {
    if let Some((label, base, movie)) = v.last_play.clone() {
        v.start(label, base, movie, window, cx);
    }
}

impl AnimeHome {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = common::input(window, cx, "Search anime");
        let sub = cx.subscribe_in(&search, window, |this, _, ev: &InputEvent, _, cx| match ev {
            InputEvent::PressEnter { .. } => {
                this.search_gen += 1;
                this.run_search(cx);
            }
            InputEvent::Change => this.debounce_search(cx),
            _ => {}
        });
        look::rotate(window, cx, Self::step);
        let mut s = Self {
            rows: vec![],
            cont: vec![],
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
            watch_epoch: None,
            scroll: ScrollHandle::new(),
            _sub: sub,
        };
        s.load_rows(cx);
        s
    }

    /// Continue Watching (and the hero's Resume), reread whenever the player saved progress or
    /// History removed or cleared entries since: a card resumes straight away, so a stale list
    /// would play the wrong episode, and a cleared title stayed in the row.
    fn refresh_cont(&mut self, cx: &mut Context<Self>) {
        let epoch = common::progress_stamp();
        if self.watch_epoch == Some(epoch) {
            return;
        }
        self.watch_epoch = Some(epoch);
        fetch(
            cx,
            db(|c| Ok(media::list_history(c, Some("anime"), None, 30)?.into_iter().filter(|h| !h.completed).collect::<Vec<_>>())),
            |v: &mut Self, l| {
                if let Load::Ready(cont) = l {
                    v.cont = cont;
                }
            },
        );
    }

    fn load_rows(&mut self, cx: &mut Context<Self>) {
        self.rows = ROWS
            .iter()
            .map(|&(id, title, feed)| {
                let load = fetch_cached(cx, feed.key(), DAY, feed.load(1), move |v: &mut Self, l| {
                    if let Some(row) = v.rows.iter_mut().find(|r| r.0 == id) {
                        row.3 = l;
                    }
                });
                (id, title, feed, load)
            })
            .collect();
        cx.notify();
    }

    /// Carousel tick: advances unless the hero is hovered or paused, an overlay or grid is open,
    /// or motion is reduced (WCAG 2.2.2: moving content can be paused).
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
            async move { jikan()?.search_anime(&query, 1, None).await.map_err(|e| e.to_string()) },
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

    /// Leaves search results or a See all grid for the home rows.
    fn back_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.results = None;
        self.expanded = None;
        self.search_gen += 1;
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        self.restore_home();
        cx.notify();
    }

    /// Starts a source lookup that plays the best link; the overlay shows progress and errors.
    fn start(&mut self, label: String, base: PlayBase, movie: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Remember what had focus only when the overlay opens (Try again keeps the first one).
        if matches!(self.streams, StreamState::Idle) {
            self.return_focus = window.focused(cx).filter(|f| *f != self.focus);
        }
        self.streams = StreamState::Searching(label.clone());
        self.last_play = Some((label.clone(), base.clone(), movie));
        window.focus(&self.focus, cx);
        if let Some(mal) = &base.mal_id {
            fetch(cx, mark_watching(media_id("anime", mal)), |_: &mut Self, _| {});
        }
        self.lookup = Some(streams::find(cx, window, label, base, movie, true, |v: &mut Self, s| {
            v.streams = if matches!(s, StreamState::Picking { .. }) { StreamState::Idle } else { s }
        }));
        cx.notify();
    }

    /// Hero action: resumes the episode in progress when there is one, otherwise episode 1.
    fn watch(&mut self, r: &AnimeSearchResult, window: &mut Window, cx: &mut Context<Self>) {
        let movie = r.anime_type.as_deref() == Some("Movie");
        let ep = if movie { 1 } else { self.resume_of(&r.id).and_then(|h| h.episode_number).map_or(1, |n| n.max(1) as u32) };
        let base = PlayBase {
            title: if movie { r.title.clone() } else { format!("{} - Episode {ep}", r.title) },
            imdb_id: None,
            mal_id: Some(r.id.clone()),
            season: Some(1),
            episode: Some(ep),
        };
        self.start(r.title.clone(), base, movie, window, cx);
    }

    /// Continue Watching: plays the saved episode straight away (the player resumes the position).
    fn resume(&mut self, h: &PlaybackState, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mal) = h.media_id.strip_prefix("anime:mal:") else { return };
        let movie = is_movie(mal);
        let ep = h.episode_number.map_or(1, |n| n.max(1) as u32);
        let base = PlayBase {
            title: if movie { h.title.clone() } else { format!("{} - Episode {ep}", h.title) },
            imdb_id: None,
            mal_id: Some(mal.to_string()),
            season: Some(1),
            episode: Some(ep),
        };
        let label = if movie { h.title.clone() } else { format!("{}, episode {ep}", h.title) };
        self.start(label, base, movie, window, cx);
    }

    fn resume_of(&self, mal: &str) -> Option<&PlaybackState> {
        let id = media_id("anime", mal);
        self.cont.iter().find(|h| h.media_id == id)
    }

    fn expand(&mut self, id: &'static str, cx: &mut Context<Self>) {
        let Some((title, feed, items)) = self.rows.iter().find(|r| r.0 == id).map(|r| (r.1, r.2, r.3.ready().cloned().unwrap_or_default())) else { return };
        self.home_offset = Some(self.scroll.offset());
        self.expanded = Some(Expanded { id, title, feed, done: items.len() < JIKAN_PAGE, items, page: 1, loading: false, error: None });
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
        let (feed, page, id) = (e.feed, e.page + 1, e.id);
        cx.notify();
        fetch(cx, feed.load(page), move |v: &mut Self, l| {
            let Some(e) = v.expanded.as_mut().filter(|e| e.id == id) else { return };
            e.loading = false;
            match l {
                Load::Ready(more) => {
                    e.done = more.len() < JIKAN_PAGE;
                    e.page = page;
                    let fresh: Vec<_> = more.into_iter().filter(|m| !e.items.iter().any(|x| x.id == m.id)).collect();
                    e.items.extend(fresh);
                }
                Load::Failed(err) => e.error = Some(err),
                Load::Loading => {}
            }
        });
    }

    /// Carousel picks: airing shows with a wide banner and a synopsis.
    fn featured(&self) -> Vec<AnimeSearchResult> {
        let Some(v) = self.rows.first().and_then(|r| r.3.ready()) else { return vec![] };
        let mut picks: Vec<_> = v.iter().filter(|r| r.banner_url.is_some() && r.description.is_some()).take(6).cloned().collect();
        if picks.is_empty() {
            picks = v.iter().take(5).cloned().collect();
        }
        picks
    }

    fn hero(&self, cx: &mut Context<Self>) -> AnyElement {
        let height = 520.;
        let items = self.featured();
        if items.is_empty() {
            let failed = matches!(self.rows.first().map(|r| &r.3), Some(Load::Failed(_)));
            return look::billboard("anime-hero", height, None, None).when(!failed, |d| d.child(look::hero_copy(110.).child(look::hero_skeleton()))).into_any_element();
        }
        let n = items.len();
        let current = self.hero % n;
        let r = items[current].clone();
        let art = |r: &AnimeSearchResult| r.banner_url.clone().or_else(|| r.cover_url.clone());
        let under = self.hero_prev.map(|p| p % n).filter(|p| *p != current).and_then(|p| art(&items[p]));
        let route = Route::AnimeDetail { mal_id: r.id.clone(), title: r.title.clone() };
        let movie = r.anime_type.as_deref() == Some("Movie");
        let genres = r.genres.clone().unwrap_or_default().into_iter().take(3).collect::<Vec<_>>().join(", ");
        let resume = self.resume_of(&r.id);
        let play_label = match (movie, resume.and_then(|h| h.episode_number)) {
            (true, _) if resume.is_some() => "Resume".to_string(),
            (true, _) => "Watch now".to_string(),
            (false, Some(e)) => format!("Resume E{e}"),
            (false, None) => "Start watching E1".to_string(),
        };
        let watch_r = r.clone();
        let arrow = |id: &'static str, left: bool, to: usize, cx: &mut Context<Self>| {
            div().absolute().top(px(height / 2. - 30.)).when(left, |d| d.left(px(6.))).when(!left, |d| d.right(px(6.))).child(
                look::glass_button(id, if left { ph::CARET_LEFT } else { ph::CARET_RIGHT }, if left { "Previous title" } else { "Next title" }, 44.).on_click(cx.listener(move |this, _, _, cx| {
                    this.go_hero(to);
                    cx.notify();
                })),
            )
        };
        let paused = self.hero_paused;
        let pause = (!reduced()).then(|| {
            look::glass_button("hero-pause", if paused { ph::PLAY } else { ph::PAUSE }, if paused { "Play slideshow" } else { "Pause slideshow" }, 28.).on_click(cx.listener(|this, _, _, cx| {
                this.hero_paused = !this.hero_paused;
                cx.notify();
            }))
        });
        look::billboard(("anime-hero", current), height, art(&r), under)
            .child(look::bounds_probe(&self.hero_bounds))
            .child(
                look::hero_copy(92.)
                    .child(div().ramp(ramp::TITLE_LARGE).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).line_clamp(2).child(SharedString::from(r.title.clone())))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .children(look::score(r.score, isz::MD, None))
                            .child(look::meta_tag(r.anime_type.clone().unwrap_or_else(|| "TV".into())))
                            .child(div().text_size(px(mv_ui::size::MD)).text_color(Tokens::text_2()).child(SharedString::from(genres))),
                    )
                    .child(div().text_size(px(mv_ui::size::LG)).line_height(px(22.)).text_color(Tokens::text_1()).line_clamp(3).child(SharedString::from(r.description.as_deref().map(look::blurb).unwrap_or_default())))
                    .child(
                        div()
                            .flex()
                            .gap(px(10.))
                            .mt(px(6.))
                            .child(look::hero_play("hero-watch", play_label).on_click(cx.listener(move |this, _, w, cx| this.watch(&watch_r, w, cx))))
                            .child(look::hero_more("hero-details").on_click(move |_, w, cx| navigate(route.clone(), w, cx))),
                    ),
            )
            .child(arrow("hero-prev", true, (current + n - 1) % n, cx))
            .child(arrow("hero-next", false, (current + 1) % n, cx))
            .child(div().absolute().left(px(EDGE - 5.)).bottom(px(50.)).flex().items_center().gap(px(6.)).children(pause).child(look::dots(cx, n, current, |v: &mut Self, i| v.go_hero(i))))
            .into_any_element()
    }

    fn top_bar(&self) -> Div {
        let scrolled = self.results.is_some() || self.expanded.is_some() || f32::from(self.scroll.offset().y) < -40.0;
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(64.))
            .px(px(EDGE))
            .flex()
            .items_center()
            .gap(px(14.))
            .map(|d| if scrolled { d.bg(Tokens::base()).border_b_1().border_color(Tokens::border()) } else { d })
            // Cards scrolled under the bar must not take its hovers and clicks; the wheel still scrolls.
            .block_mouse_except_scroll()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(icon(glyphs::ANIME.fill(), Tokens::accent(), isz::XL))
                    .child(div().ramp(ramp::TITLE_3).font_weight(FontWeight::BLACK).text_color(Tokens::accent()).child("ANIME")),
            )
            .child(div().flex_1())
            .child(div().w(px(300.)).child(common::search_field(&self.search)))
    }

    fn offset(&self, id: &'static str) -> usize {
        self.offsets.get(id).copied().unwrap_or(0)
    }

    fn rows(&self, width: f32, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = vec![];
        let (pw, gap) = (190., 16.);
        if !self.cont.is_empty() {
            let card = 300.;
            let items = self
                .cont
                .iter()
                .enumerate()
                .filter_map(|(i, h)| {
                    let route = route_for_media(&h.media_id, &h.title)?;
                    let left = look::time_left(h.position_secs, h.duration_secs);
                    let sub = [h.episode_number.map(|n| format!("E{n}")), left].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                    let h2 = h.clone();
                    let info = div().absolute().top(px(8.)).right(px(8.)).child(look::glass_button(("cont-info", i), ph::INFO, "Details", 28.).on_click(move |_, w, cx| {
                        // The card under the button would resume playback.
                        cx.stop_propagation();
                        navigate(route.clone(), w, cx);
                    }));
                    Some(
                        look::landscape_card(("cont", i), card, h.cover_url.clone(), &h.title, Some(sub).filter(|s| !s.is_empty()), Some(fraction(h)), Hover::Play)
                            .on_click(cx.listener(move |this, _, w, cx| this.resume(&h2, w, cx)))
                            .child(info)
                            .into_any_element(),
                    )
                })
                .collect();
            let history: OnClick = std::rc::Rc::new(|_, w, cx| navigate(Route::MediaHistory, w, cx));
            out.push(look::row(cx, "cont", "Continue Watching", self.offset("cont"), look::per_row(width, card, 8.), 8., items, set_offset, Some(history)).into_any_element());
        }
        for (id, title, _, load) in &self.rows {
            let (id, title): (&'static str, &'static str) = (*id, *title);
            match load {
                Load::Loading => out.push(look::skeleton_row(title, pw, pw * 1.5, 8).into_any_element()),
                Load::Failed(_) => {}
                Load::Ready(v) if v.is_empty() => {}
                Load::Ready(v) => {
                    let items = v.iter().enumerate().map(|(i, r)| poster((id, i), pw, r)).collect();
                    let see_all: OnClick = std::rc::Rc::new(cx.listener(move |this, _, _, cx| this.expand(id, cx)));
                    out.push(look::row(cx, id, title, self.offset(id), look::per_row(width, pw, gap), gap, items, set_offset, Some(see_all)).into_any_element());
                }
            }
        }
        if !self.rows.is_empty() && self.rows.iter().all(|r| matches!(r.3, Load::Failed(_))) {
            let raw = match &self.rows[0].3 {
                Load::Failed(e) => e.clone(),
                _ => String::new(),
            };
            out.push(
                div()
                    .px(px(EDGE))
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(error_view("Check your connection, then try again.").title("Anime could not be loaded").retry(cx.listener(|this, _, _, cx| this.load_rows(cx))))
                    .child(div().w(px(420.)).child(disclosure("anime-error-details", "Details").content(muted(raw))))
                    .into_any_element(),
            );
        }
        out
    }

    fn grid_width(width: f32) -> f32 {
        let per = look::per_row(width, 180., 16.);
        ((width - EDGE * 2.0 - 16.0 * (per as f32 - 1.0)) / per as f32).clamp(150., 230.)
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
        let w = Self::grid_width(width);
        let body = match load {
            Load::Loading => look::skeleton_posters(12, w).into_any_element(),
            Load::Failed(e) => error_view(e.clone())
                .title("Search failed")
                .retry(cx.listener(|this, _, _, cx| {
                    this.results = None;
                    this.run_search(cx);
                }))
                .into_any_element(),
            Load::Ready(v) if v.is_empty() => empty_view(ph::MAGNIFYING_GLASS_MINUS, format!("No matches for \"{q}\""), "Check the spelling, or try the English or Japanese title.").into_any_element(),
            Load::Ready(v) => div().flex().flex_wrap().gap_x(px(16.)).gap_y(px(28.)).children(v.iter().enumerate().map(|(i, r)| poster(("res", i), w, r))).into_any_element(),
        };
        div().pt(px(84.)).px(px(EDGE)).child(self.grid_header(format!("Results for \"{q}\""), load.ready().map(Vec::len), cx)).child(body)
    }

    fn expanded_view(&self, e: &Expanded, width: f32, cx: &mut Context<Self>) -> Div {
        let w = Self::grid_width(width);
        div()
            .pt(px(84.))
            .px(px(EDGE))
            .child(self.grid_header(e.title.to_string(), Some(e.items.len()), cx))
            .child(div().flex().flex_wrap().gap_x(px(16.)).gap_y(px(28.)).children(e.items.iter().enumerate().map(|(i, r)| poster(("all", i), w, r))))
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

/// Crunchyroll-style poster that opens the anime page.
pub fn poster(key: (&'static str, usize), w: f32, r: &AnimeSearchResult) -> AnyElement {
    let route = Route::AnimeDetail { mal_id: r.id.clone(), title: r.title.clone() };
    let corner = r.next_episode.map(|n| format!("E{n} SOON"));
    look::poster_card(key, w, r.cover_url.clone(), &r.title, subline(r), r.score, r.description.as_deref().map(look::blurb), corner).on_click(move |_, w, cx| navigate(route.clone(), w, cx)).into_any_element()
}

fn set_offset(v: &mut AnimeHome, id: &'static str, n: usize) {
    v.offsets.insert(id, n);
}

impl Render for AnimeHome {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh_cont(cx);
        let width = look::page_width(&self.scroll, window);
        let content = if let Some(e) = &self.expanded {
            div().pb(px(40.)).child(self.expanded_view(e, width, cx))
        } else if let Some((q, load)) = &self.results {
            div().pb(px(40.)).child(self.search_results(q, load, width, cx))
        } else {
            let rows = self.rows(width, cx);
            div().pb(px(40.)).child(self.hero(cx)).child(div().mt(px(-30.)).relative().children(rows))
        };
        let panel = streams::panel(&self.streams, cx, close_streams, retry_lookup);
        let weak = cx.weak_entity();
        div()
            .relative()
            .size_full()
            .bg(Tokens::base())
            .child(mv_ui::scroll_page("anime-home", &self.scroll).child(content))
            .child(self.top_bar())
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
