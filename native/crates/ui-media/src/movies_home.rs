//! Movies home, Netflix-style: rotating billboard, continue watching, Top 10 rows,
//! trending / acclaimed / genre rows and search. Data comes from Cinemeta.
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::media::{self, PlaybackState};
use mv_core::repo::watchlist;
use mv_media::cinemeta::{CinemetaCatalog, CinemetaClient};
use mv_media::types::AnimeSearchResult;
use mv_ui::{chip, empty_state, error_state, navigate, size, Route, Tokens};

use crate::common::{self, db, fetch, route_for_media, Load};
use crate::look::{self, netflix, RowOffsets, EDGE};
use crate::streams::{self, PlayBase, StreamState};

/// Genre rows shown on the home page, in order.
const ROW_GENRES: &[&str] = &["Action", "Comedy", "Sci-Fi", "Thriller", "Animation", "Horror", "Romance", "Documentary"];

pub fn cinemeta() -> Result<CinemetaClient, String> {
    CinemetaClient::new().map_err(|e| e.to_string())
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
    load: Load<Vec<AnimeSearchResult>>,
}

pub struct MoviesHome {
    tab: Tab,
    rows: Vec<Row>,
    cont: Vec<PlaybackState>,
    my_list: Vec<AnimeSearchResult>,
    hero: usize,
    offsets: RowOffsets,
    search: Entity<InputState>,
    results: Option<(String, Load<Vec<AnimeSearchResult>>)>,
    streams: StreamState,
    scroll: ScrollHandle,
    generation: u64,
    _sub: Subscription,
}

impl MoviesHome {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = common::input(window, cx, "Titles, genres");
        let sub = cx.subscribe_in(&search, window, |this, _, ev: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.run_search(cx);
            }
        });
        let mut s = Self {
            tab: Tab::Home,
            rows: vec![],
            cont: vec![],
            my_list: vec![],
            hero: 0,
            offsets: Default::default(),
            search,
            results: None,
            streams: StreamState::Idle,
            scroll: ScrollHandle::new(),
            generation: 0,
            _sub: sub,
        };
        s.load(cx);
        s.load_personal(cx);
        look::rotate(cx, |v: &mut Self| v.hero += 1);
        s
    }

    /// History + My List (movies and series only).
    fn load_personal(&mut self, cx: &mut Context<Self>) {
        fetch(
            cx,
            db(|c| {
                let mut h = media::list_history(c, Some("movie"), None, 20)?;
                h.extend(media::list_history(c, Some("series"), None, 20)?);
                h.retain(|h| !h.completed && h.duration_secs > 0.0);
                h.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
                Ok(h)
            }),
            |v: &mut Self, l| v.cont = l.ready().cloned().unwrap_or_default(),
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
            |v: &mut Self, l| v.my_list = l.ready().cloned().unwrap_or_default(),
        );
    }

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
                "Documentary" => if movie { "Documentaries".to_string() } else { "Docuseries".to_string() },
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
        self.offsets.clear();
        self.rows = Self::plan(self.tab)
            .into_iter()
            .map(|(id, title, look, movie, catalog, genre)| {
                fetch(
                    cx,
                    async move {
                        let c = cinemeta()?;
                        let r = if movie { c.movies_catalog(catalog, 1, genre).await } else { c.series_catalog(catalog, 1, genre).await };
                        r.map_err(|e| e.to_string())
                    },
                    move |v: &mut Self, l| {
                        if v.generation == generation {
                            if let Some(row) = v.rows.iter_mut().find(|r| r.id == id) {
                                row.load = l;
                            }
                        }
                    },
                );
                Row { id, title, look, load: Load::Loading }
            })
            .collect();
        cx.notify();
    }

    fn run_search(&mut self, cx: &mut Context<Self>) {
        let q = self.search.read(cx).value().trim().to_string();
        if q.is_empty() {
            self.results = None;
            cx.notify();
            return;
        }
        self.results = Some((q.clone(), Load::Loading));
        cx.notify();
        let query = q.clone();
        fetch(
            cx,
            async move {
                let c = cinemeta()?;
                let (movies, series) = futures::join!(c.search_movies(&query), c.search_series(&query));
                let mut all = movies.unwrap_or_default();
                let series = series.unwrap_or_default();
                if all.is_empty() && series.is_empty() {
                    return Err("Nothing matched your search.".to_string());
                }
                let extra: Vec<_> = series.into_iter().filter(|s| !all.iter().any(|m| m.id == s.id)).collect();
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

    /// Billboard picks: the first top row's titles that have artwork and a synopsis.
    fn featured(&self) -> Vec<AnimeSearchResult> {
        let src = self.rows.iter().find(|r| r.look == Look::Top10).and_then(|r| r.load.ready());
        src.map(|v| v.iter().filter(|r| r.description.is_some() && r.imdb_id.is_some()).take(6).cloned().collect()).unwrap_or_default()
    }

    fn play(&mut self, r: &AnimeSearchResult, window: &mut Window, cx: &mut Context<Self>) {
        let movie = kind_of(r) == "movie";
        let label = if movie { r.title.clone() } else { format!("{} S01E01", r.title) };
        self.streams = StreamState::Searching(label.clone());
        cx.notify();
        let base = PlayBase { title: label.clone(), imdb_id: r.imdb_id.clone(), mal_id: None, season: (!movie).then_some(1), episode: (!movie).then_some(1) };
        streams::find(cx, window, label, base, movie, true, |v: &mut Self, s| {
            v.streams = if matches!(s, StreamState::Picking { .. }) { StreamState::Idle } else { s }
        });
    }

    fn hero(&self, cx: &mut Context<Self>) -> AnyElement {
        let items = self.featured();
        let height = 540.;
        let Some(r) = (!items.is_empty()).then(|| items[self.hero % items.len()].clone()) else {
            return look::billboard("movies-hero", height, None)
                .child(look::hero_copy(110.).child(div().w(px(380.)).h(px(44.)).rounded(px(6.)).bg(Tokens::surface_2())).child(div().w(px(500.)).h(px(60.)).rounded(px(6.)).bg(Tokens::surface())))
                .into_any_element();
        };
        let current = self.hero % items.len();
        let movie = kind_of(&r) == "movie";
        let genres = r.genres.clone().unwrap_or_default().into_iter().take(3).collect::<Vec<_>>().join(" · ");
        let meta = look::meta_line(
            r.score.map(|s| (format!("IMDb {s:.1}"), rgb(0xf5c518))),
            vec![r.year.map(|y| y.to_string()).unwrap_or_default(), if movie { "Movie".into() } else { "Series".into() }, genres],
        );
        let (play_r, route) = (r.clone(), Route::MovieDetail { imdb_id: r.imdb_id.clone().unwrap_or_default(), kind: kind_of(&r).into(), title: r.title.clone() });
        look::billboard(("movies-hero", current), height, background_of(&r))
            .child(
                look::hero_copy(96.)
                    .child(look::hero_title(&r.title, logo_of(&r)))
                    .child(meta)
                    .child(div().text_size(px(size::LG)).line_height(px(22.)).text_color(Tokens::text_1()).line_clamp(3).child(SharedString::from(r.description.clone().unwrap_or_default())))
                    .child(
                        div()
                            .flex()
                            .gap(px(10.))
                            .mt(px(4.))
                            .child(look::cta("hero-play", "Play", Tokens::text_1(), rgb(0x000000)).on_click(cx.listener(move |this, _, w, cx| this.play(&play_r, w, cx))))
                            .child(look::soft("hero-info", "icons/info.svg", "More Info").on_click(move |_, w, cx| navigate(route.clone(), w, cx))),
                    ),
            )
            .child(div().absolute().right(px(EDGE)).bottom(px(110.)).child(look::dots(cx, items.len(), current, netflix(), |v: &mut Self, i| v.hero = i)))
            .into_any_element()
    }

    /// Tabs + search, floating over the top of the billboard.
    fn top_bar(&self, cx: &mut Context<Self>) -> Div {
        let scrolled = self.results.is_some() || f32::from(self.scroll.offset().y) < -40.0;
        let tab = |id: &'static str, label: &'static str, t: Tab, cur: Tab, cx: &mut Context<Self>| {
            div()
                .id(id)
                .text_size(px(size::LG))
                .cursor_pointer()
                .text_color(if t == cur { Tokens::text_1() } else { Tokens::text_2() })
                .when(t == cur, |d| d.font_weight(FontWeight::BOLD))
                .hover(|s| s.text_color(Tokens::text_1()))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.tab != t {
                        this.tab = t;
                        this.results = None;
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
            .gap(px(22.))
            .map(|d| if scrolled { d.bg(Tokens::base()).border_b_1().border_color(Tokens::border()) } else { d })
            .child(div().text_size(px(26.)).font_weight(FontWeight::BLACK).text_color(netflix()).child("MOVIES"))
            .child(tab("tab-home", "Home", Tab::Home, self.tab, cx))
            .child(tab("tab-movies", "Movies", Tab::Movies, self.tab, cx))
            .child(tab("tab-series", "Series", Tab::Series, self.tab, cx))
            .child(div().flex_1())
            .child(div().w(px(280.)).child(Input::new(&self.search).cleanable(true)))
    }

    fn rows(&self, width: f32, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = vec![];
        let card = 300.;
        let accent = netflix();
        if !self.cont.is_empty() {
            let items = self
                .cont
                .iter()
                .enumerate()
                .filter_map(|(i, h)| {
                    let route = route_for_media(&h.media_id, &h.title)?;
                    let p = (h.position_secs / h.duration_secs) as f32;
                    let sub = h.episode_number.map(|e| format!("Episode {e}"));
                    let imdb = h.media_id.rsplit(':').next().filter(|id| id.starts_with("tt"));
                    let image = imdb.map(|id| format!("https://images.metahub.space/background/medium/{id}/img")).or_else(|| h.cover_url.clone());
                    Some(look::landscape_card(("cont", i), card, image, &h.title, sub, Some(p), accent).on_click(move |_, w, cx| navigate(route.clone(), w, cx)).into_any_element())
                })
                .collect();
            out.push(look::row(cx, "cont", "Continue Watching", self.offset("cont"), look::per_row(width, card, 8.), 8., items, set_offset).into_any_element());
        }
        if !self.my_list.is_empty() {
            let items = self.my_list.iter().enumerate().map(|(i, r)| self.card(Look::Landscape, ("mylist", i), i, r)).collect();
            out.push(look::row(cx, "mylist", "My List", self.offset("mylist"), look::per_row(width, card, 8.), 8., items, set_offset).into_any_element());
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
                    let take = if row.look == Look::Top10 { 10 } else { 30 };
                    let items = v.iter().take(take).enumerate().map(|(i, r)| self.card(row.look, (row.id, i), i, r)).collect();
                    out.push(look::row(cx, row.id, row.title.clone(), self.offset(row.id), look::per_row(width, w, 8.), 8., items, set_offset).into_any_element());
                }
            }
        }
        if self.rows.iter().all(|r| matches!(r.load, Load::Failed(_))) {
            out.push(div().px(px(EDGE)).child(error_state("Movies could not be loaded. Check your connection and try again.")).into_any_element());
        }
        out
    }

    fn offset(&self, id: &'static str) -> usize {
        self.offsets.get(id).copied().unwrap_or(0)
    }

    fn card(&self, look_: Look, id: (&'static str, usize), rank: usize, r: &AnimeSearchResult) -> AnyElement {
        let route = Route::MovieDetail { imdb_id: r.imdb_id.clone().unwrap_or_else(|| r.id.clone()), kind: kind_of(r).into(), title: r.title.clone() };
        let card = match look_ {
            Look::Top10 => look::top10_card(id, rank + 1, r.cover_url.as_deref().map(look::hi_res), 210.),
            Look::Poster => look::poster_card(
                id,
                180.,
                r.cover_url.as_deref().map(look::hi_res),
                &r.title,
                [r.year.map(|y| y.to_string()), r.score.map(|s| format!("IMDb {s:.1}"))].into_iter().flatten().collect::<Vec<_>>().join(" · "),
                r.description.clone(),
                None,
                netflix(),
            ),
            Look::Landscape => {
                let sub = r.genres.clone().unwrap_or_default().into_iter().take(2).collect::<Vec<_>>().join(" · ");
                look::landscape_card(id, 300., background_of(r), &r.title, Some(sub).filter(|s| !s.is_empty()), None, netflix())
            }
        };
        card.on_click(move |_, w, cx| navigate(route.clone(), w, cx)).into_any_element()
    }

    fn search_results(&self, q: &str, load: &Load<Vec<AnimeSearchResult>>, width: f32, cx: &mut Context<Self>) -> Div {
        let body = match load {
            Load::Loading => div().flex().flex_wrap().gap(px(8.)).children((0..12).map(|_| div().w(px(300.)).h(px(169.)).rounded(px(6.)).bg(Tokens::surface_2()))).into_any_element(),
            Load::Failed(e) => empty_state("No results", e.clone()).into_any_element(),
            Load::Ready(v) => {
                let per = look::per_row(width, 300., 8.);
                let w = ((width - EDGE * 2.0 - 8.0 * (per as f32 - 1.0)) / per as f32).max(200.);
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x(px(8.))
                    .gap_y(px(24.))
                    .children(v.iter().enumerate().map(|(i, r)| {
                        let route = Route::MovieDetail { imdb_id: r.imdb_id.clone().unwrap_or_else(|| r.id.clone()), kind: kind_of(r).into(), title: r.title.clone() };
                        let sub = [r.year.map(|y| y.to_string()), Some(if kind_of(r) == "movie" { "Movie".to_string() } else { "Series".to_string() })].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                        look::landscape_card(("res", i), w, background_of(r), &r.title, Some(sub), None, netflix()).on_click(move |_, w, cx| navigate(route.clone(), w, cx))
                    }))
                    .into_any_element()
            }
        };
        div()
            .pt(px(84.))
            .px(px(EDGE))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .mb(px(18.))
                    .child(div().text_size(px(size::XL)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child(SharedString::from(format!("Results for \"{q}\""))))
                    .child(chip("clear-search", "Clear", false).on_click(cx.listener(|this, _, w, cx| {
                        this.results = None;
                        this.search.update(cx, |s, cx| s.set_value("", w, cx));
                        cx.notify();
                    }))),
            )
            .child(body)
    }
}

fn set_offset(v: &mut MoviesHome, id: &'static str, n: usize) {
    v.offsets.insert(id, n);
}

impl Render for MoviesHome {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = look::page_width(&self.scroll, window);
        let content = match &self.results {
            Some((q, load)) => div().pb(px(40.)).child(self.search_results(q, load, width, cx)),
            None => {
                let rows = self.rows(width, cx);
                div().pb(px(40.)).child(self.hero(cx)).child(div().mt(px(-70.)).relative().children(rows))
            }
        };
        let panel = streams::panel(&self.streams, cx, |v: &mut Self| v.streams = StreamState::Idle);
        div()
            .relative()
            .size_full()
            .bg(Tokens::base())
            .child(mv_ui::scroll_page("movies-home", &self.scroll).child(content))
            .child(self.top_bar(cx))
            .children(panel.map(look::overlay))
    }
}
