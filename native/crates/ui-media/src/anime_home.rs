//! Anime home, Crunchyroll-style: banner carousel, continue watching and poster rows
//! (airing, this season, top rated, movies, coming soon, genres) with search.
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::media::{self, PlaybackState};
use mv_media::types::AnimeSearchResult;
use mv_ui::{chip, empty_state, error_state, icon, navigate, size, Route, Tokens};

use crate::common::{self, db, fetch, fetch_cached, jikan, route_for_media, Load, DAY};
use crate::look::{self, crunchy, RowOffsets, EDGE};
use crate::streams::{self, PlayBase, StreamState};

#[derive(Clone, Copy)]
enum Feed {
    Airing,
    Season,
    Top(Option<&'static str>),
    Upcoming,
    Genre(u32),
}

impl Feed {
    fn key(self) -> String {
        match self {
            Feed::Airing => "jikan:airing:1".into(),
            Feed::Season => "jikan:season:1".into(),
            Feed::Top(k) => format!("jikan:top:{}:1", k.unwrap_or("")),
            Feed::Upcoming => "jikan:upcoming:1".into(),
            Feed::Genre(g) => genre_key(g),
        }
    }

    async fn load(self) -> Result<Vec<AnimeSearchResult>, String> {
        let j = jikan()?;
        let r = match self {
            Feed::Airing => j.airing_anime(1).await,
            Feed::Season => j.season_anime(None, None, 1).await,
            Feed::Top(kind) => j.top_anime(1, kind).await,
            Feed::Upcoming => j.upcoming_anime(1).await,
            Feed::Genre(g) => j.anime_by_genre(g, 1, None).await,
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
        match mv_core::cache::DataCache::shared().get::<Vec<AnimeSearchResult>>(&feed.key()) {
            Some((_, age)) if age < DAY => {}
            _ => {
                let _ = common::cached(feed.key(), feed.load()).await;
            }
        }
    }
}

/// (row id, title, feed) in display order. The first row also feeds the carousel.
const ROWS: &[(&str, &str, Feed)] = &[
    ("airing", "Top Airing", Feed::Airing),
    ("season", "Popular This Season", Feed::Season),
    ("top", "Top Rated of All Time", Feed::Top(None)),
    ("action", "Action Packed", Feed::Genre(1)),
    ("movies", "Anime Movies", Feed::Top(Some("movie"))),
    ("isekai", "Isekai Adventures", Feed::Genre(62)),
    ("romance", "Romance", Feed::Genre(22)),
    ("upcoming", "Coming Soon", Feed::Upcoming),
];

pub struct AnimeHome {
    rows: Vec<(&'static str, &'static str, Load<Vec<AnimeSearchResult>>)>,
    cont: Vec<PlaybackState>,
    hero: usize,
    offsets: RowOffsets,
    search: Entity<InputState>,
    results: Option<(String, Load<Vec<AnimeSearchResult>>)>,
    streams: StreamState,
    scroll: ScrollHandle,
    _sub: Subscription,
}

pub fn subline(r: &AnimeSearchResult) -> String {
    [r.anime_type.clone(), r.episode_count.filter(|n| *n > 1).map(|n| format!("{n} eps")), r.year.map(|y| y.to_string())].into_iter().flatten().collect::<Vec<_>>().join(" · ")
}

impl AnimeHome {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = common::input(window, cx, "Search anime");
        let sub = cx.subscribe_in(&search, window, |this, _, ev: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.run_search(cx);
            }
        });
        let rows = ROWS
            .iter()
            .map(|&(id, title, feed)| {
                let load = fetch_cached(cx, feed.key(), DAY, feed.load(), move |v: &mut Self, l| {
                    if let Some(row) = v.rows.iter_mut().find(|r| r.0 == id) {
                        row.2 = l;
                    }
                });
                (id, title, load)
            })
            .collect();
        fetch(
            cx,
            db(|c| Ok(media::list_history(c, Some("anime"), None, 30)?.into_iter().filter(|h| !h.completed).collect::<Vec<_>>())),
            |v: &mut Self, l| v.cont = l.ready().cloned().unwrap_or_default(),
        );
        look::rotate(cx, |v: &mut Self| v.hero += 1);
        Self { rows, cont: vec![], hero: 0, offsets: Default::default(), search, results: None, streams: StreamState::Idle, scroll: ScrollHandle::new(), _sub: sub }
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
            async move { jikan()?.search_anime(&query, 1, None).await.map_err(|e| e.to_string()) },
            move |v: &mut Self, l| {
                if v.results.as_ref().is_some_and(|(cur, _)| *cur == q) {
                    v.results = Some((q, l));
                }
            },
        );
    }

    fn watch(&mut self, r: &AnimeSearchResult, window: &mut Window, cx: &mut Context<Self>) {
        let movie = r.anime_type.as_deref() == Some("Movie");
        let label = r.title.clone();
        self.streams = StreamState::Searching(label.clone());
        cx.notify();
        let base = PlayBase {
            title: if movie { r.title.clone() } else { format!("{} - Episode 1", r.title) },
            imdb_id: None,
            mal_id: Some(r.id.clone()),
            season: Some(1),
            episode: Some(1),
        };
        streams::find(cx, window, label, base, movie, true, |v: &mut Self, s| {
            v.streams = if matches!(s, StreamState::Picking { .. }) { StreamState::Idle } else { s }
        });
    }

    /// Carousel picks: airing shows with a wide banner and a synopsis.
    fn featured(&self) -> Vec<AnimeSearchResult> {
        let Some(v) = self.rows.first().and_then(|r| r.2.ready()) else { return vec![] };
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
            let failed = matches!(self.rows.first().map(|r| &r.2), Some(Load::Failed(_)));
            return look::billboard("anime-hero", height, None)
                .when(!failed, |d| {
                    d.child(look::hero_copy(110.).child(div().w(px(420.)).h(px(44.)).rounded(px(6.)).bg(Tokens::surface_2())).child(div().w(px(520.)).h(px(66.)).rounded(px(6.)).bg(Tokens::surface())))
                })
                .into_any_element();
        }
        let current = self.hero % items.len();
        let r = items[current].clone();
        let route = Route::AnimeDetail { mal_id: r.id.clone(), title: r.title.clone() };
        let movie = r.anime_type.as_deref() == Some("Movie");
        let genres = r.genres.clone().unwrap_or_default().into_iter().take(3).collect::<Vec<_>>().join(", ");
        let watch_r = r.clone();
        let n = items.len();
        let arrow = |id: &'static str, ic: &'static str, to: usize, cx: &mut Context<Self>| {
            div()
                .id(id)
                .absolute()
                .top(px(height / 2. - 30.))
                .when(id == "hero-prev", |d| d.left(px(6.)))
                .when(id == "hero-next", |d| d.right(px(6.)))
                .size(px(44.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .hover(|s| s.bg(rgba(0xffffff1a)))
                .child(icon(ic, Tokens::text_1(), 30.))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.hero = to;
                    cx.notify();
                }))
        };
        look::billboard(("anime-hero", current), height, r.banner_url.clone().or_else(|| r.cover_url.clone()))
            .child(
                look::hero_copy(92.)
                    .child(div().text_size(px(40.)).line_height(px(46.)).font_weight(FontWeight::BLACK).text_color(Tokens::text_1()).line_clamp(2).child(SharedString::from(r.title.clone())))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .children(r.score.map(|s| div().flex().items_center().gap(px(4.)).text_size(px(size::MD)).font_weight(FontWeight::BOLD).text_color(crunchy()).child(icon("icons/star.svg", crunchy(), 14.)).child(SharedString::from(format!("{s:.1}")))))
                            .child(look::tag(r.anime_type.clone().unwrap_or_else(|| "TV".into())))
                            .child(div().text_size(px(size::MD)).text_color(Tokens::text_2()).child(SharedString::from(genres))),
                    )
                    .child(div().text_size(px(size::LG)).line_height(px(22.)).text_color(Tokens::text_1()).line_clamp(3).child(SharedString::from(r.description.as_deref().map(look::blurb).unwrap_or_default())))
                    .child(
                        div()
                            .flex()
                            .gap(px(10.))
                            .mt(px(6.))
                            .child(
                                look::cta("hero-watch", if movie { "WATCH NOW" } else { "START WATCHING E1" }, crunchy(), rgb(0x000000))
                                    .rounded(px(0.))
                                    .on_click(cx.listener(move |this, _, w, cx| this.watch(&watch_r, w, cx))),
                            )
                            .child(
                                div()
                                    .id("hero-details")
                                    .h(px(42.))
                                    .px(px(16.))
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .border_2()
                                    .border_color(crunchy())
                                    .text_color(crunchy())
                                    .text_size(px(size::MD))
                                    .font_weight(FontWeight::BOLD)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgba(0xf4752126)))
                                    .child(icon("icons/bookmark.svg", crunchy(), 16.))
                                    .child("DETAILS")
                                    .on_click(move |_, w, cx| navigate(route.clone(), w, cx)),
                            ),
                    ),
            )
            .child(arrow("hero-prev", "icons/chevron-left.svg", (current + n - 1) % n, cx))
            .child(arrow("hero-next", "icons/chevron-right.svg", (current + 1) % n, cx))
            .child(div().absolute().left(px(EDGE)).bottom(px(56.)).child(look::dots(cx, n, current, crunchy(), |v: &mut Self, i| v.hero = i)))
            .into_any_element()
    }

    fn top_bar(&self) -> Div {
        let scrolled = self.results.is_some() || f32::from(self.scroll.offset().y) < -40.0;
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
            .child(div().text_size(px(26.)).font_weight(FontWeight::BLACK).text_color(crunchy()).child("ANIME"))
            .child(div().flex_1())
            .child(div().w(px(300.)).child(Input::new(&self.search).cleanable(true)))
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
                    let p = if h.duration_secs > 0.0 { (h.position_secs / h.duration_secs) as f32 } else { 0.0 };
                    let sub = h.episode_number.map(|n| format!("E{n}"));
                    Some(look::landscape_card(("cont", i), card, h.cover_url.clone(), &h.title, sub, Some(p), crunchy()).on_click(move |_, w, cx| navigate(route.clone(), w, cx)).into_any_element())
                })
                .collect();
            out.push(look::row(cx, "cont", "Continue Watching", self.offset("cont"), look::per_row(width, card, 8.), 8., items, set_offset).into_any_element());
        }
        for (id, title, load) in &self.rows {
            match load {
                Load::Loading => out.push(look::skeleton_row(title, pw, pw * 1.5, 8).into_any_element()),
                Load::Failed(_) => {}
                Load::Ready(v) if v.is_empty() => {}
                Load::Ready(v) => {
                    let items = v.iter().take(30).enumerate().map(|(i, r)| poster(( *id, i), pw, r)).collect();
                    out.push(look::row(cx, id, *title, self.offset(id), look::per_row(width, pw, gap), gap, items, set_offset).into_any_element());
                }
            }
        }
        if self.rows.iter().all(|r| matches!(r.2, Load::Failed(_))) {
            let msg = match &self.rows[0].2 {
                Load::Failed(e) => e.clone(),
                _ => String::new(),
            };
            out.push(div().px(px(EDGE)).child(error_state(format!("Anime could not be loaded. {msg}"))).into_any_element());
        }
        out
    }

    fn search_results(&self, q: &str, load: &Load<Vec<AnimeSearchResult>>, width: f32, cx: &mut Context<Self>) -> Div {
        let per = look::per_row(width, 180., 16.);
        let w = ((width - EDGE * 2.0 - 16.0 * (per as f32 - 1.0)) / per as f32).clamp(150., 230.);
        let body = match load {
            Load::Loading => div().flex().flex_wrap().gap(px(16.)).children((0..12).map(|_| div().w(px(w)).h(px(w * 1.5)).rounded(px(4.)).bg(Tokens::surface_2()))).into_any_element(),
            Load::Failed(e) => empty_state("No results", e.clone()).into_any_element(),
            Load::Ready(v) if v.is_empty() => empty_state("No results", "Try a different title.").into_any_element(),
            Load::Ready(v) => div().flex().flex_wrap().gap_x(px(16.)).gap_y(px(28.)).children(v.iter().enumerate().map(|(i, r)| poster(("res", i), w, r))).into_any_element(),
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

/// Crunchyroll-style poster that opens the anime page.
pub fn poster(id: (&'static str, usize), w: f32, r: &AnimeSearchResult) -> AnyElement {
    let route = Route::AnimeDetail { mal_id: r.id.clone(), title: r.title.clone() };
    let badge = r.next_episode.map(|n| format!("EP {n} SOON"));
    look::poster_card(id, w, r.cover_url.clone(), &r.title, subline(r), r.description.clone(), badge, crunchy()).on_click(move |_, w, cx| navigate(route.clone(), w, cx)).into_any_element()
}

fn set_offset(v: &mut AnimeHome, id: &'static str, n: usize) {
    v.offsets.insert(id, n);
}

impl Render for AnimeHome {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = look::page_width(&self.scroll, window);
        let content = match &self.results {
            Some((q, load)) => div().pb(px(40.)).child(self.search_results(q, load, width, cx)),
            None => {
                let rows = self.rows(width, cx);
                div().pb(px(40.)).child(self.hero(cx)).child(div().mt(px(-30.)).relative().children(rows))
            }
        };
        let panel = streams::panel(&self.streams, cx, |v: &mut Self| v.streams = StreamState::Idle);
        div()
            .relative()
            .size_full()
            .bg(Tokens::base())
            .child(mv_ui::scroll_page("anime-home", &self.scroll).child(content))
            .child(self.top_bar())
            .children(panel.map(look::overlay))
    }
}
