//! Anime detail: cover, synopsis, genres, episodes, play via addons.
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_media::types::{AnimeEpisode, AnimeSearchResult};
use mv_ui::{button, empty_state, error_state, page, section_title, size, ButtonKind, Tokens};

use crate::common::{self, cover_of, fetch, in_watchlist, jikan, media_id, media_title, muted, record_open, set_watchlist, Load};
use crate::streams::{self, PlayBase, StreamState};

pub struct AnimeDetail {
    mal_id: String,
    title: String,
    details: Load<AnimeSearchResult>,
    episodes: Load<Vec<AnimeEpisode>>,
    page: u32,
    has_more: bool,
    in_list: Option<bool>,
    streams: StreamState,
}

impl AnimeDetail {
    pub fn new(mal_id: String, title: String, cx: &mut Context<Self>) -> Self {
        let (id, t) = (mal_id.clone(), title.clone());
        fetch(
            cx,
            async move {
                let d = jikan()?.anime_details(&id).await.map_err(|e| e.to_string())?;
                let _ = record_open(media_title("anime", &id, &d, &t)).await;
                Ok(d)
            },
            |v: &mut Self, l| v.details = l,
        );
        let id = mal_id.clone();
        fetch(
            cx,
            async move { jikan()?.anime_episodes(&id, 1).await.map_err(|e| e.to_string()) },
            |v: &mut Self, l| {
                v.has_more = l.ready().is_some_and(|e| e.len() >= 100);
                v.episodes = l;
            },
        );
        fetch(cx, in_watchlist(media_id("anime", &mal_id)), |v: &mut Self, l| v.in_list = l.ready().copied());
        Self { mal_id, title, details: Load::Loading, episodes: Load::Loading, page: 1, has_more: false, in_list: None, streams: StreamState::Idle }
    }

    fn is_movie(&self) -> bool {
        self.details.ready().is_some_and(|d| d.anime_type.as_deref() == Some("Movie"))
    }

    fn display_title(&self) -> String {
        self.details.ready().map(|d| d.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| self.title.clone())
    }

    fn play(&mut self, episode: u32, window: &mut Window, cx: &mut Context<Self>) {
        let movie = self.is_movie();
        let name = self.display_title();
        let label = if movie { name.clone() } else { format!("{name}, episode {episode}") };
        self.streams = StreamState::Searching(label.clone());
        cx.notify();
        let base = PlayBase {
            title: if movie { name } else { format!("{name} - Episode {episode}") },
            imdb_id: None,
            mal_id: Some(self.mal_id.clone()),
            season: Some(1),
            episode: Some(episode),
        };
        streams::find(cx, window, label, base, movie, |v: &mut Self, s| v.streams = s);
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        self.page += 1;
        self.has_more = false;
        let (id, page) = (self.mal_id.clone(), self.page);
        fetch(
            cx,
            async move { jikan()?.anime_episodes(&id, page).await.map_err(|e| e.to_string()) },
            |v: &mut Self, l| {
                if let (Load::Ready(cur), Load::Ready(more)) = (&mut v.episodes, l) {
                    v.has_more = more.len() >= 100;
                    cur.extend(more);
                }
            },
        );
    }

    fn toggle_list(&mut self, cx: &mut Context<Self>) {
        let Some(cur) = self.in_list else { return };
        let fallback = AnimeSearchResult { title: self.title.clone(), ..Default::default() };
        let t = media_title("anime", &self.mal_id, self.details.ready().unwrap_or(&fallback), &self.title);
        self.in_list = None;
        cx.notify();
        fetch(cx, set_watchlist(t, !cur), move |v: &mut Self, l| v.in_list = Some(l.ready().copied().unwrap_or(cur)));
    }
}

pub fn pill(text: impl Into<SharedString>) -> Div {
    div().px(px(9.)).h(px(22.)).flex().items_center().rounded(px(999.)).border_1().border_color(Tokens::border()).text_size(px(size::XS)).text_color(Tokens::text_2()).child(text.into())
}

impl Render for AnimeDetail {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let movie = self.is_movie();
        let panel = streams::panel(&self.streams, cx, |v: &mut Self| v.streams = StreamState::Idle);
        let first = self.episodes.ready().and_then(|e| e.first().map(|e| e.number)).unwrap_or(1);

        let header = match &self.details {
            Load::Loading => div().flex().gap(px(28.)).mb(px(24.)).child(div().flex_none().w(px(180.)).h(px(260.)).rounded(px(12.)).bg(Tokens::surface_2())).child(
                div().flex_1().flex().flex_col().gap(px(10.)).child(div().text_size(px(size::XXL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(SharedString::from(self.title.clone()))).child(muted("Loading...")),
            ),
            Load::Failed(e) => div().mb(px(24.)).child(div().text_size(px(size::XXL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(SharedString::from(self.title.clone()))).child(error_state(e.clone()).min_h(px(100.))),
            Load::Ready(d) => {
                let meta = [
                    d.score.map(|s| format!("Score {s:.1}")),
                    d.year.map(|y| y.to_string()),
                    d.anime_type.clone(),
                    d.episode_count.map(|n| format!("{n} episodes")),
                    d.status.clone(),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("  ·  ");
                div()
                    .flex()
                    .gap(px(28.))
                    .mb(px(24.))
                    .child(
                        div()
                            .flex_none()
                            .w(px(180.))
                            .h(px(260.))
                            .rounded(px(12.))
                            .overflow_hidden()
                            .bg(Tokens::surface_2())
                            .border_1()
                            .border_color(Tokens::border())
                            .when_some(cover_of(&d.cover_url), |x, c| x.child(c.into_img().size_full().object_fit(ObjectFit::Cover))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(12.))
                            .child(div().text_size(px(size::XXL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(SharedString::from(d.title.clone())))
                            .child(muted(meta))
                            .child(div().flex().flex_wrap().gap(px(6.)).children(d.genres.clone().unwrap_or_default().into_iter().map(pill)))
                            .child(div().text_size(px(size::MD)).text_color(Tokens::text_2()).child(SharedString::from(d.description.clone().unwrap_or_else(|| "No synopsis available.".into()))))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(8.))
                                    .mt(px(4.))
                                    .child(button("play-main", if movie { "Play" } else { "Play episode 1" }, ButtonKind::Primary).on_click(cx.listener(move |this, _, w, cx| this.play(first, w, cx))))
                                    .child(common::list_button("toggle-list", self.in_list).on_click(cx.listener(|this, _, _, cx| this.toggle_list(cx)))),
                            ),
                    )
            }
        };

        let episodes = if movie {
            div()
        } else {
            let body = match &self.episodes {
                Load::Loading => div().flex().flex_col().gap(px(6.)).children((0..6).map(|_| div().h(px(44.)).rounded(px(8.)).bg(Tokens::surface_2()))),
                Load::Failed(e) => error_state(e.clone()).min_h(px(100.)),
                Load::Ready(v) if v.is_empty() => empty_state("No episode list", "Use Play above to look for sources.").min_h(px(100.)),
                Load::Ready(v) => div().flex().flex_col().children(v.iter().map(|ep| {
                    let n = ep.number;
                    div()
                        .id(("ep", n as usize))
                        .flex()
                        .items_center()
                        .gap(px(14.))
                        .px(px(10.))
                        .h(px(40.))
                        .rounded(px(8.))
                        .cursor_pointer()
                        .hover(|s| s.bg(Tokens::hover()))
                        .child(div().w(px(36.)).flex_none().text_size(px(size::SM)).text_color(Tokens::text_3()).child(SharedString::from(n.to_string())))
                        .child(div().flex_1().min_w_0().text_size(px(size::MD)).text_color(Tokens::text_1()).truncate().child(SharedString::from(ep.title.clone().unwrap_or_else(|| format!("Episode {n}")))))
                        .child(muted(ep.air_date.as_deref().map(|d| d.chars().take(10).collect::<String>()).unwrap_or_default()))
                        .on_click(cx.listener(move |this, _, w, cx| this.play(n, w, cx)))
                })),
            };
            div().child(section_title("Episodes")).child(body).when(self.has_more, |d| {
                d.child(div().mt(px(10.)).child(button("more-eps", "Load more episodes", ButtonKind::Secondary).on_click(cx.listener(|this, _, _, cx| this.load_more(cx)))))
            })
        };

        page("anime-detail").child(header).children(panel).child(episodes)
    }
}
