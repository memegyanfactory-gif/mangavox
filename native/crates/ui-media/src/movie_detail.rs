//! Movie / series detail: poster, overview, season + episode picker, play via addons.
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_media::types::{AnimeEpisode, AnimeSearchResult};
use mv_ui::{button, chip, empty_state, error_state, page, section_title, size, ButtonKind, Tokens};

use crate::anime_detail::pill;
use crate::common::{self, cover_of, fetch, in_watchlist, media_id, media_title, muted, record_open, set_watchlist, Load};
use crate::movies_home::cinemeta;
use crate::streams::{self, PlayBase, StreamState};

pub struct MovieDetail {
    imdb_id: String,
    kind: String,
    title: String,
    details: Load<AnimeSearchResult>,
    episodes: Load<Vec<AnimeEpisode>>,
    season: Option<u32>,
    in_list: Option<bool>,
    streams: StreamState,
}

/// Season number from a Stremio video id (`tt123:2:5`).
fn season_of(ep: &AnimeEpisode) -> u32 {
    ep.id.split(':').nth(1).and_then(|s| s.parse().ok()).unwrap_or(1)
}

impl MovieDetail {
    pub fn new(imdb_id: String, kind: String, title: String, cx: &mut Context<Self>) -> Self {
        let series = kind == "series";
        let (id, t, k) = (imdb_id.clone(), title.clone(), kind.clone());
        fetch(
            cx,
            async move {
                let c = cinemeta()?;
                let d = if k == "series" { c.series_details(&id).await } else { c.movie_details(&id).await };
                let d = d.map_err(|e| e.to_string())?.ok_or_else(|| "Title not found.".to_string())?;
                let _ = record_open(media_title(&k, &id, &d, &t)).await;
                Ok(d)
            },
            |v: &mut Self, l| v.details = l,
        );
        let episodes = if series {
            let id = imdb_id.clone();
            fetch(cx, async move { cinemeta()?.series_episodes(&id).await.map_err(|e| e.to_string()) }, |v: &mut Self, l| {
                if let Load::Ready(eps) = &l {
                    v.season = eps.iter().map(season_of).filter(|s| *s > 0).min().or_else(|| eps.first().map(season_of));
                }
                v.episodes = l;
            });
            Load::Loading
        } else {
            Load::Ready(Vec::new())
        };
        fetch(cx, in_watchlist(media_id(&kind, &imdb_id)), |v: &mut Self, l| v.in_list = l.ready().copied());
        Self { imdb_id, kind, title, details: Load::Loading, episodes, season: None, in_list: None, streams: StreamState::Idle }
    }

    fn display_title(&self) -> String {
        self.details.ready().map(|d| d.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| self.title.clone())
    }

    fn play(&mut self, season: u32, episode: u32, window: &mut Window, cx: &mut Context<Self>) {
        let movie = self.kind != "series";
        let name = self.display_title();
        let label = if movie { name.clone() } else { format!("{name} S{season:02}E{episode:02}") };
        self.streams = StreamState::Searching(label.clone());
        cx.notify();
        let base = PlayBase {
            title: label.clone(),
            imdb_id: Some(self.imdb_id.clone()),
            mal_id: None,
            season: (!movie).then_some(season),
            episode: (!movie).then_some(episode),
        };
        streams::find(cx, window, label, base, movie, |v: &mut Self, s| v.streams = s);
    }

    fn toggle_list(&mut self, cx: &mut Context<Self>) {
        let Some(cur) = self.in_list else { return };
        let fallback = AnimeSearchResult { title: self.title.clone(), ..Default::default() };
        let t = media_title(&self.kind, &self.imdb_id, self.details.ready().unwrap_or(&fallback), &self.title);
        self.in_list = None;
        cx.notify();
        fetch(cx, set_watchlist(t, !cur), move |v: &mut Self, l| v.in_list = Some(l.ready().copied().unwrap_or(cur)));
    }
}

impl Render for MovieDetail {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let series = self.kind == "series";
        let panel = streams::panel(&self.streams, cx, |v: &mut Self| v.streams = StreamState::Idle);
        let first_season = self.season.unwrap_or(1);
        let first_ep = self.episodes.ready().and_then(|e| e.iter().find(|e| season_of(e) == first_season).map(|e| e.number)).unwrap_or(1);
        let heading = |t: String| div().text_size(px(size::XXL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(SharedString::from(t));

        let header = match &self.details {
            Load::Loading => div()
                .flex()
                .gap(px(28.))
                .mb(px(24.))
                .child(div().flex_none().w(px(180.)).h(px(260.)).rounded(px(12.)).bg(Tokens::surface_2()))
                .child(div().flex_1().flex().flex_col().gap(px(10.)).child(heading(self.title.clone())).child(muted("Loading..."))),
            Load::Failed(e) => div().mb(px(24.)).child(heading(self.title.clone())).child(error_state(e.clone()).min_h(px(100.))),
            Load::Ready(d) => {
                let meta = [d.score.map(|s| format!("IMDb {s:.1}")), d.year.map(|y| y.to_string()).or_else(|| d.status.clone()), Some(if series { "Series".to_string() } else { "Movie".to_string() })]
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
                            .child(heading(d.title.clone()))
                            .child(muted(meta))
                            .child(div().flex().flex_wrap().gap(px(6.)).children(d.genres.clone().unwrap_or_default().into_iter().map(pill)))
                            .child(div().text_size(px(size::MD)).text_color(Tokens::text_2()).child(SharedString::from(d.description.clone().unwrap_or_else(|| "No overview available.".into()))))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(8.))
                                    .mt(px(4.))
                                    .child(
                                        button("play-main", if series { format!("Play S{first_season} E{first_ep}") } else { "Play".to_string() }, ButtonKind::Primary)
                                            .on_click(cx.listener(move |this, _, w, cx| this.play(first_season, first_ep, w, cx))),
                                    )
                                    .child(common::list_button("toggle-list", self.in_list).on_click(cx.listener(|this, _, _, cx| this.toggle_list(cx)))),
                            ),
                    )
            }
        };

        let episodes = if !series {
            div()
        } else {
            let body = match &self.episodes {
                Load::Loading => div().flex().flex_col().gap(px(6.)).children((0..6).map(|_| div().h(px(44.)).rounded(px(8.)).bg(Tokens::surface_2()))),
                Load::Failed(e) => error_state(e.clone()).min_h(px(100.)),
                Load::Ready(v) if v.is_empty() => empty_state("No episode list", "Use Play above to look for sources.").min_h(px(100.)),
                Load::Ready(v) => {
                    let mut seasons: Vec<u32> = v.iter().map(season_of).collect();
                    seasons.sort_unstable();
                    seasons.dedup();
                    let current = self.season.unwrap_or(seasons[0]);
                    let chips = div().flex().flex_wrap().gap(px(8.)).mb(px(12.)).children(seasons.into_iter().map(|s| {
                        chip(("season", s as usize), if s == 0 { "Specials".to_string() } else { format!("Season {s}") }, s == current).on_click(cx.listener(move |this, _, _, cx| {
                            this.season = Some(s);
                            cx.notify();
                        }))
                    }));
                    let rows = v.iter().filter(|e| season_of(e) == current).map(|ep| {
                        let (n, s) = (ep.number, season_of(ep));
                        div()
                            .id(("ep", (s * 10000 + n) as usize))
                            .flex()
                            .items_center()
                            .gap(px(14.))
                            .px(px(10.))
                            .h(px(40.))
                            .rounded(px(8.))
                            .cursor_pointer()
                            .hover(|x| x.bg(Tokens::hover()))
                            .child(div().w(px(36.)).flex_none().text_size(px(size::SM)).text_color(Tokens::text_3()).child(SharedString::from(n.to_string())))
                            .child(div().flex_1().min_w_0().text_size(px(size::MD)).text_color(Tokens::text_1()).truncate().child(SharedString::from(ep.title.clone().unwrap_or_else(|| format!("Episode {n}")))))
                            .child(muted(ep.air_date.as_deref().map(|d| d.chars().take(10).collect::<String>()).unwrap_or_default()))
                            .on_click(cx.listener(move |this, _, w, cx| this.play(s, n, w, cx)))
                    });
                    div().child(chips).child(div().flex().flex_col().children(rows))
                }
            };
            div().child(section_title("Episodes")).child(body)
        };

        page("movie-detail").child(header).children(panel).child(episodes)
    }
}
