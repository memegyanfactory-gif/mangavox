//! Movies home: Cinemeta catalogs, genre chips and search.
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::*;
use mv_media::cinemeta::{CinemetaCatalog, CinemetaClient};
use mv_media::types::AnimeSearchResult;
use mv_ui::{chip, page, page_header, section_title};

use crate::common::{self, fetch, grid, rail, Load};

pub const GENRES: &[&str] = &[
    "Action", "Adventure", "Animation", "Comedy", "Crime", "Documentary", "Drama", "Family", "Fantasy", "History", "Horror", "Mystery", "Romance", "Sci-Fi",
    "Thriller", "War", "Western",
];

pub fn cinemeta() -> Result<CinemetaClient, String> {
    CinemetaClient::new().map_err(|e| e.to_string())
}

pub struct MoviesHome {
    movies: Load<Vec<AnimeSearchResult>>,
    series: Load<Vec<AnimeSearchResult>>,
    genre: Option<&'static str>,
    search: Entity<InputState>,
    results: Option<(String, Load<Vec<AnimeSearchResult>>)>,
    _sub: Subscription,
}

impl MoviesHome {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = common::input(window, cx, "Search movies and series");
        let sub = cx.subscribe_in(&search, window, |this, _, ev: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.run_search(cx);
            }
        });
        let mut s = Self { movies: Load::Loading, series: Load::Loading, genre: None, search, results: None, _sub: sub };
        s.load_catalogs(cx);
        s
    }

    fn load_catalogs(&mut self, cx: &mut Context<Self>) {
        self.movies = Load::Loading;
        self.series = Load::Loading;
        let genre = self.genre;
        fetch(
            cx,
            async move { cinemeta()?.movies_catalog(CinemetaCatalog::Top, 1, genre).await.map_err(|e| e.to_string()) },
            move |v: &mut Self, l| {
                if v.genre == genre {
                    v.movies = l;
                }
            },
        );
        fetch(
            cx,
            async move { cinemeta()?.series_catalog(CinemetaCatalog::Top, 1, genre).await.map_err(|e| e.to_string()) },
            move |v: &mut Self, l| {
                if v.genre == genre {
                    v.series = l;
                }
            },
        );
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
                    return Err("Search failed or returned nothing.".to_string());
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
}

impl Render for MoviesHome {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chips = div()
            .flex()
            .flex_wrap()
            .gap(px(8.))
            .mb(px(20.))
            .child(chip("genre-all", "All", self.genre.is_none()).on_click(cx.listener(|this, _, _, cx| {
                this.genre = None;
                this.load_catalogs(cx);
                cx.notify();
            })))
            .children(GENRES.iter().map(|g| {
                let g: &'static str = g;
                chip(g, g, self.genre == Some(g)).on_click(cx.listener(move |this, _, _, cx| {
                    this.genre = Some(g);
                    this.load_catalogs(cx);
                    cx.notify();
                }))
            }));
        let body = match &self.results {
            Some((q, load)) => div()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(section_title(format!("Results for \"{q}\"")))
                        .child(chip("clear-search", "Clear", false).on_click(cx.listener(|this, _, w, cx| {
                            this.results = None;
                            this.search.update(cx, |s, cx| s.set_value("", w, cx));
                            cx.notify();
                        }))),
                )
                .child(grid("movie-results", load)),
            None => {
                let suffix = self.genre.map(|g| format!(" - {g}")).unwrap_or_default();
                div().child(chips).child(rail("top-movies", &format!("Top movies{suffix}"), &self.movies)).child(rail("top-series", &format!("Top series{suffix}"), &self.series))
            }
        };
        page("movies-home")
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .child(page_header("Movies and series", None))
                    .child(div().w(px(300.)).child(Input::new(&self.search))),
            )
            .child(body)
    }
}
