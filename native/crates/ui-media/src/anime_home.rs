//! Anime home: hero, continue watching, airing / top / this-season rails, search.
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::media::{self, PlaybackState};
use mv_media::types::AnimeSearchResult;
use mv_ui::{button, chip, cover_card, navigate, page, page_header, section_title, size, ButtonKind, Route, Tokens};

use crate::common::{self, cover_of, db, fetch, grid, jikan, muted, on_open, rail, route_for_media, Load};
use crate::streams::{self, PlayBase, StreamState};

pub struct AnimeHome {
    airing: Load<Vec<AnimeSearchResult>>,
    top: Load<Vec<AnimeSearchResult>>,
    season: Load<Vec<AnimeSearchResult>>,
    cont: Load<Vec<PlaybackState>>,
    search: Entity<InputState>,
    results: Option<(String, Load<Vec<AnimeSearchResult>>)>,
    streams: StreamState,
    _sub: Subscription,
}

impl AnimeHome {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = common::input(window, cx, "Search anime");
        let sub = cx.subscribe_in(&search, window, |this, _, ev: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.run_search(cx);
            }
        });
        fetch(cx, async { jikan()?.airing_anime(1).await.map_err(|e| e.to_string()) }, |v: &mut Self, l| v.airing = l);
        fetch(cx, async { jikan()?.top_anime(1, None).await.map_err(|e| e.to_string()) }, |v: &mut Self, l| v.top = l);
        fetch(cx, async { jikan()?.season_anime(None, None, 1).await.map_err(|e| e.to_string()) }, |v: &mut Self, l| v.season = l);
        fetch(
            cx,
            db(|c| Ok(media::list_history(c, Some("anime"), None, 30)?.into_iter().filter(|h| !h.completed).collect::<Vec<_>>())),
            |v: &mut Self, l| v.cont = l,
        );
        Self { airing: Load::Loading, top: Load::Loading, season: Load::Loading, cont: Load::Loading, search, results: None, streams: StreamState::Idle, _sub: sub }
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
        streams::find(cx, window, label, base, movie, |v: &mut Self, s| v.streams = s);
    }

    fn hero(&self, cx: &mut Context<Self>) -> Div {
        let shell = || div().flex().gap(px(28.)).p(px(24.)).mb(px(24.)).h(px(280.)).rounded(px(16.)).bg(Tokens::surface()).border_1().border_color(Tokens::border()).overflow_hidden();
        let Load::Ready(items) = &self.airing else {
            return shell().child(div().flex_1()).child(div().w(px(176.)).h_full().rounded(px(10.)).bg(Tokens::surface_2()));
        };
        let Some(r) = items.iter().find(|r| r.description.is_some() && r.cover_url.is_some()).or(items.first()) else {
            return div();
        };
        let (watch_r, route) = (r.clone(), Route::AnimeDetail { mal_id: r.id.clone(), title: r.title.clone() });
        let meta = [r.score.map(|s| format!("Score {s:.1}")), r.year.map(|y| y.to_string()), r.anime_type.clone(), r.episode_count.map(|n| format!("{n} episodes"))]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("  ·  ");
        shell()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(px(10.))
                    .child(div().text_size(px(size::XS)).text_color(Tokens::accent()).font_weight(FontWeight::SEMIBOLD).child("AIRING NOW"))
                    .child(div().text_size(px(size::XXL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).line_clamp(2).child(SharedString::from(r.title.clone())))
                    .child(muted(meta))
                    .child(div().text_size(px(size::MD)).text_color(Tokens::text_2()).line_clamp(3).child(SharedString::from(r.description.clone().unwrap_or_default())))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .mt(px(6.))
                            .child(button("hero-watch", "Watch", ButtonKind::Primary).on_click(cx.listener(move |this, _, w, cx| this.watch(&watch_r, w, cx))))
                            .child(button("hero-details", "Details", ButtonKind::Secondary).on_click(move |_, w, cx| navigate(route.clone(), w, cx))),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(176.))
                    .h_full()
                    .rounded(px(10.))
                    .overflow_hidden()
                    .bg(Tokens::surface_2())
                    .when_some(cover_of(&r.cover_url), |d, c| d.child(c.into_img().size_full().object_fit(ObjectFit::Cover))),
            )
    }

    fn continue_row(&self) -> Option<Div> {
        let items = self.cont.ready().filter(|v| !v.is_empty())?;
        let cards = items.iter().enumerate().filter_map(|(i, h)| {
            let route = route_for_media(&h.media_id, &h.title)?;
            let pct = if h.duration_secs > 0.0 { (h.position_secs / h.duration_secs * 100.0).round() as i64 } else { 0 };
            let sub = match h.episode_number {
                Some(n) => format!("Episode {n}  ·  {pct}%"),
                None => format!("{pct}%"),
            };
            Some(div().flex_none().child(on_open(cover_card(("cont", i), h.title.clone().into(), sub.into(), cover_of(&h.cover_url), None), route)))
        });
        Some(
            div().mb(px(20.)).child(section_title("Continue watching")).child(div().id("cont-row").flex().gap(px(16.)).pb(px(8.)).overflow_x_scroll().children(cards)),
        )
    }
}

impl Render for AnimeHome {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let panel = streams::panel(&self.streams, cx, |v: &mut Self| v.streams = StreamState::Idle);
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
                .child(grid("anime-results", load)),
            None => div()
                .child(self.hero(cx))
                .children(panel)
                .children(self.continue_row())
                .child(rail("airing", "Airing now", &self.airing))
                .child(rail("top", "Top anime", &self.top))
                .child(rail("season", "This season", &self.season)),
        };
        page("anime-home")
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .child(page_header("Anime", None))
                    .child(div().w(px(280.)).child(Input::new(&self.search))),
            )
            .child(body)
    }
}
