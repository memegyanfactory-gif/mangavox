//! Movie / series detail, Netflix-style: billboard with logo and actions, synopsis with cast,
//! season picker + episode list with thumbnails, and "More like this".
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_media::types::{AnimeEpisode, AnimeSearchResult};
use mv_ui::{chip, empty_state, error_state, icon, navigate, size, Route, Tokens};

use crate::common::{fetch, fetch_cached, in_watchlist, media_id, media_title, muted, record_open, set_watchlist, Load, DAY, EPISODES_TTL};
use crate::look::{self, netflix, EDGE};
use crate::movies_home::{background_of, cinemeta, kind_of, logo_of};
use crate::streams::{self, PlayBase, StreamState};

pub struct MovieDetail {
    imdb_id: String,
    kind: String,
    title: String,
    details: Load<AnimeSearchResult>,
    episodes: Load<Vec<AnimeEpisode>>,
    similar: Vec<AnimeSearchResult>,
    season: Option<u32>,
    in_list: Option<bool>,
    streams: StreamState,
    /// The first source lookup (movie, or the first episode) has been started.
    auto: bool,
    /// (season, episode) whose links are shown.
    sel: Option<(u32, u32)>,
    /// Addon chip selected in the sources panel.
    addon_filter: Option<String>,
    scroll: ScrollHandle,
}

/// Season number from a Stremio video id (`tt123:2:5`).
fn season_of(ep: &AnimeEpisode) -> u32 {
    ep.id.split(':').nth(1).and_then(|s| s.parse().ok()).unwrap_or(1)
}

fn first_season(eps: &[AnimeEpisode]) -> Option<u32> {
    eps.iter().map(season_of).filter(|s| *s > 0).min().or_else(|| eps.first().map(season_of))
}

fn runtime(mins: u32) -> String {
    match (mins / 60, mins % 60) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

impl MovieDetail {
    pub fn new(imdb_id: String, kind: String, title: String, cx: &mut Context<Self>) -> Self {
        let series = kind == "series";
        let (id, t, k) = (imdb_id.clone(), title.clone(), kind.clone());
        let key = format!("cinemeta:meta:{kind}:{imdb_id}");
        let warm = mv_core::cache::DataCache::shared().age(&key).is_some();
        let details = fetch_cached(
            cx,
            key,
            DAY,
            async move {
                let c = cinemeta()?;
                let d = if k == "series" { c.series_details(&id).await } else { c.movie_details(&id).await };
                let d = d.map_err(|e| e.to_string())?.ok_or_else(|| "Title not found.".to_string())?;
                if !warm {
                    let _ = record_open(media_title(&k, &id, &d, &t)).await;
                }
                // "More like this": the top titles of the first genre, minus this one.
                let genre = d.genres.as_ref().and_then(|g| g.first().cloned());
                let similar = match genre {
                    Some(g) if k == "series" => c.series_catalog(Default::default(), 1, Some(&g)).await.unwrap_or_default(),
                    Some(g) => c.movies_catalog(Default::default(), 1, Some(&g)).await.unwrap_or_default(),
                    None => vec![],
                };
                let similar = similar.into_iter().filter(|s| s.imdb_id.as_deref() != Some(id.as_str())).take(18).collect::<Vec<_>>();
                Ok((d, similar))
            },
            |v: &mut Self, l| match l {
                Load::Ready((d, s)) => {
                    v.details = Load::Ready(d);
                    v.similar = s;
                }
                Load::Failed(e) => v.details = Load::Failed(e),
                Load::Loading => {}
            },
        );
        let (details, similar) = match details {
            Load::Ready((d, s)) => {
                fetch(cx, record_open(media_title(&kind, &imdb_id, &d, &title)), |_: &mut Self, _| {});
                (Load::Ready(d), s)
            }
            _ => (Load::Loading, vec![]),
        };
        let episodes = if series {
            let id = imdb_id.clone();
            fetch_cached(cx, format!("cinemeta:episodes:{id}"), EPISODES_TTL, async move { cinemeta()?.series_episodes(&id).await.map_err(|e| e.to_string()) }, |v: &mut Self, l| {
                if let Load::Ready(eps) = &l {
                    v.season = v.season.or_else(|| first_season(eps));
                }
                v.episodes = l;
            })
        } else {
            Load::Ready(Vec::new())
        };
        let season = episodes.ready().and_then(|e| first_season(e));
        fetch(cx, in_watchlist(media_id(&kind, &imdb_id)), |v: &mut Self, l| v.in_list = l.ready().copied());
        Self {
            imdb_id,
            kind,
            title,
            details,
            episodes,
            similar,
            season,
            in_list: None,
            streams: StreamState::Idle,
            auto: false,
            sel: None,
            addon_filter: None,
            scroll: ScrollHandle::new(),
        }
    }

    fn display_title(&self) -> String {
        self.details.ready().map(|d| d.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| self.title.clone())
    }

    /// Play the best link right away when it is already known, otherwise look it up and play.
    fn play(&mut self, season: u32, episode: u32, window: &mut Window, cx: &mut Context<Self>) {
        let movie = self.kind != "series";
        if let Some(req) = streams::best(&self.streams, (!movie).then_some(season), (!movie).then_some(episode)) {
            navigate(Route::Player { request: req }, window, cx);
            return;
        }
        self.lookup(season, episode, true, window, cx);
    }

    /// Find the links for a movie / episode and list them on the page.
    fn lookup(&mut self, season: u32, episode: u32, autoplay: bool, window: &mut Window, cx: &mut Context<Self>) {
        let movie = self.kind != "series";
        self.sel = Some((season, episode));
        self.addon_filter = None;
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
        streams::find(cx, window, label, base, movie, autoplay, |v: &mut Self, s| v.streams = s);
    }

    fn toggle_list(&mut self, cx: &mut Context<Self>) {
        let Some(cur) = self.in_list else { return };
        let fallback = AnimeSearchResult { title: self.title.clone(), ..Default::default() };
        let t = media_title(&self.kind, &self.imdb_id, self.details.ready().unwrap_or(&fallback), &self.title);
        self.in_list = None;
        cx.notify();
        fetch(cx, set_watchlist(t, !cur), move |v: &mut Self, l| v.in_list = Some(l.ready().copied().unwrap_or(cur)));
    }

    fn hero(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let series = self.kind == "series";
        let height = 560.;
        let fallback = AnimeSearchResult { title: self.title.clone(), imdb_id: Some(self.imdb_id.clone()), ..Default::default() };
        let d = self.details.ready().unwrap_or(&fallback);
        let first_season = self.season.unwrap_or(1);
        let first_ep = self.episodes.ready().and_then(|e| e.iter().find(|e| season_of(e) == first_season).map(|e| e.number)).unwrap_or(1);
        let seasons = self.episodes.ready().map(|v| {
            let mut s: Vec<u32> = v.iter().map(season_of).filter(|s| *s > 0).collect();
            s.dedup();
            s.len()
        });
        let meta = look::meta_line(
            d.score.map(|s| (format!("IMDb {s:.1}"), rgb(0xf5c518))),
            vec![
                d.year.map(|y| y.to_string()).unwrap_or_default(),
                if series { seasons.filter(|n| *n > 0).map(|n| if n == 1 { "1 Season".to_string() } else { format!("{n} Seasons") }).unwrap_or_default() } else { d.duration_minutes.map(runtime).unwrap_or_default() },
            ],
        )
        .child(look::tag("HD"))
        .child(look::tag(if series { "Series" } else { "Movie" }));
        let (list_icon, list_label) = match self.in_list {
            Some(true) => ("icons/check.svg", "My List"),
            _ => ("icons/plus.svg", "My List"),
        };
        let play_label = if series { format!("Play S{first_season} E{first_ep}") } else { "Play".to_string() };
        let has_panel = !matches!(self.streams, StreamState::Idle);
        let mut copy = look::hero_copy(70.).when(has_panel, |d| d.max_w(relative(0.5))).child(look::hero_title(&d.title, logo_of(d)));
        if matches!(self.details, Load::Loading) {
            copy = copy.child(div().w(px(280.)).h(px(16.)).rounded(px(4.)).bg(Tokens::surface_2()));
        } else {
            copy = copy.child(meta);
        }
        if let Load::Failed(e) = &self.details {
            copy = copy.child(muted(e.clone()));
        }
        let heading = match (series, self.sel) {
            (true, Some((s, e))) => {
                let name = self.episodes.ready().and_then(|v| v.iter().find(|x| season_of(x) == s && x.number == e)).and_then(|x| x.title.clone());
                format!("S{s}E{e}{}", name.map(|n| format!("  {n}")).unwrap_or_default())
            }
            _ => "Sources".to_string(),
        };
        let panel = streams::sources_panel(&self.streams, netflix(), heading, self.addon_filter.as_deref(), cx, |v: &mut Self, f| v.addon_filter = f);
        look::billboard("movie-hero", height, background_of(d))
            .children(panel.map(|p| div().absolute().right(px(EDGE)).top(px(24.)).bottom(px(24.)).w(px(430.)).child(p)))
            .child(
            copy.child(
                div()
                    .flex()
                    .gap(px(10.))
                    .mt(px(4.))
                    .child(look::cta("play-main", play_label, Tokens::text_1(), rgb(0x000000)).on_click(cx.listener(move |this, _, w, cx| this.play(first_season, first_ep, w, cx))))
                    .child(look::soft("toggle-list", list_icon, list_label).when(self.in_list.is_none(), |d| d.opacity(0.5)).on_click(cx.listener(|this, _, _, cx| this.toggle_list(cx)))),
            ),
        )
    }

    fn about(&self) -> Option<Div> {
        let d = self.details.ready()?;
        let line = |label: &'static str, v: String| {
            div().text_size(px(size::SM)).line_height(px(20.)).child(
                div().flex().flex_wrap().gap(px(4.)).child(div().text_color(Tokens::text_3()).child(label)).child(div().text_color(Tokens::text_1()).child(SharedString::from(v))),
            )
        };
        let join = |v: &Option<Vec<String>>| v.clone().unwrap_or_default().into_iter().take(5).collect::<Vec<_>>().join(", ");
        Some(
            div()
                .flex()
                .gap(px(40.))
                .px(px(EDGE))
                .mb(px(36.))
                .child(div().flex_1().min_w_0().text_size(px(size::LG)).line_height(px(24.)).text_color(Tokens::text_1()).child(SharedString::from(d.description.clone().unwrap_or_else(|| "No overview available.".into()))))
                .child(
                    div()
                        .w(px(320.))
                        .flex_none()
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .when(!join(&d.cast).is_empty(), |x| x.child(line("Cast:", join(&d.cast))))
                        .when(!join(&d.genres).is_empty(), |x| x.child(line("Genres:", join(&d.genres))))
                        .when(!join(&d.directors).is_empty(), |x| x.child(line("Director:", join(&d.directors)))),
                ),
        )
    }

    fn episodes(&self, cx: &mut Context<Self>) -> Div {
        let body = match &self.episodes {
            Load::Loading => div().flex().flex_col().gap(px(8.)).children((0..5).map(|_| div().h(px(110.)).rounded(px(6.)).bg(Tokens::surface()))),
            Load::Failed(e) => error_state(e.clone()).min_h(px(100.)),
            Load::Ready(v) if v.is_empty() => empty_state("No episode list", "Use Play above to look for sources.").min_h(px(100.)),
            Load::Ready(v) => {
                let mut seasons: Vec<u32> = v.iter().map(season_of).collect();
                seasons.sort_unstable();
                seasons.dedup();
                let current = self.season.unwrap_or(seasons[0]);
                let chips = div().flex().flex_wrap().gap(px(8.)).children(seasons.into_iter().map(|s| {
                    chip(("season", s as usize), if s == 0 { "Specials".to_string() } else { format!("Season {s}") }, s == current).on_click(cx.listener(move |this, _, _, cx| {
                        this.season = Some(s);
                        cx.notify();
                    }))
                }));
                let fallback_img = self.details.ready().and_then(background_of);
                let sel = self.sel;
                let rows = v.iter().filter(|e| season_of(e) == current).map(|ep| {
                    let (n, s) = (ep.number, season_of(ep));
                    let group: SharedString = format!("ep-{s}-{n}").into();
                    let thumb = ep.thumbnail.clone().or_else(|| fallback_img.clone());
                    let row = div()
                        .id(("ep", (s * 10000 + n) as usize))
                        .group(group.clone())
                        .flex()
                        .items_center()
                        .gap(px(20.))
                        .px(px(16.))
                        .py(px(16.))
                        .rounded(px(6.))
                        .border_b_1()
                        .border_color(Tokens::border())
                        .cursor_pointer()
                        .hover(|x| x.bg(Tokens::surface()))
                        .child(div().w(px(28.)).flex_none().text_size(px(24.)).text_color(Tokens::text_3()).child(SharedString::from(n.to_string())))
                        .child(
                            div()
                                .relative()
                                .flex_none()
                                .w(px(160.))
                                .h(px(90.))
                                .rounded(px(4.))
                                .overflow_hidden()
                                .bg(Tokens::surface_2())
                                .when_some(thumb, |d, u| d.child(look::pic(u)))
                                .child(
                                    div()
                                        .absolute()
                                        .inset_0()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .opacity(0.)
                                        .group_hover(group.clone(), |s| s.opacity(1.))
                                        .child(div().size(px(36.)).rounded_full().bg(rgba(0x000000a0)).border_1().border_color(Tokens::text_1()).flex().items_center().justify_center().child(icon("icons/play.svg", Tokens::text_1(), 16.))),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).truncate().child(SharedString::from(ep.title.clone().unwrap_or_else(|| format!("Episode {n}")))))
                                .child(muted(ep.air_date.as_deref().map(|d| d.chars().take(10).collect::<String>()).unwrap_or_default())),
                        )
                        .when(sel == Some((s, n)), |d| d.bg(Tokens::surface()))
                        .on_click(cx.listener(move |this, _, w, cx| {
                            // Links show in the panel at the top, like Stremio.
                            this.lookup(s, n, false, w, cx);
                            this.scroll.set_offset(point(px(0.), px(0.)));
                        }));
                    row
                });
                div().flex().flex_col().gap(px(16.)).child(chips).child(div().flex().flex_col().children(rows))
            }
        };
        div()
            .px(px(EDGE))
            .mb(px(40.))
            .child(div().mb(px(14.)).text_size(px(size::XL)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child("Episodes"))
            .child(body)
    }

    fn more_like_this(&self, width: f32) -> Option<Div> {
        if self.similar.is_empty() {
            return None;
        }
        let per = look::per_row(width, 170., 10.);
        let w = ((width - EDGE * 2.0 - 10.0 * (per as f32 - 1.0)) / per as f32).clamp(140., 220.);
        Some(
            div()
                .px(px(EDGE))
                .mb(px(40.))
                .child(div().mb(px(14.)).text_size(px(size::XL)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child("More Like This"))
                .child(div().flex().flex_wrap().gap_x(px(10.)).gap_y(px(20.)).children(self.similar.iter().enumerate().map(|(i, r)| {
                    let route = Route::MovieDetail { imdb_id: r.imdb_id.clone().unwrap_or_else(|| r.id.clone()), kind: kind_of(r).into(), title: r.title.clone() };
                    let sub = [r.year.map(|y| y.to_string()), r.score.map(|s| format!("IMDb {s:.1}"))].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                    look::poster_card(("similar", i), w, r.cover_url.as_deref().map(look::hi_res), &r.title, sub, r.description.clone(), None, netflix())
                        .on_click(move |_, w, cx| navigate(route.clone(), w, cx))
                }))),
        )
    }
}

impl Render for MovieDetail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = look::page_width(&self.scroll, window);
        let series = self.kind == "series";
        // Links load by themselves: the movie at once, a series' first episode once the list is in.
        if !self.auto {
            let first = self.episodes.ready().map(|eps| {
                let s = self.season.unwrap_or(1);
                (s, eps.iter().find(|e| season_of(e) == s).map(|e| e.number).unwrap_or(1))
            });
            match (series, first) {
                (false, _) => {
                    self.auto = true;
                    self.lookup(1, 1, false, window, cx);
                }
                (true, Some((s, e))) => {
                    self.auto = true;
                    self.lookup(s, e, false, window, cx);
                }
                _ => {}
            }
        }

        div()
            .relative()
            .size_full()
            .bg(Tokens::base())
            .child(
                mv_ui::scroll_page("movie-detail", &self.scroll)
                    .child(self.hero(cx))
                    .children(self.about())
                    .when(series, |d| d.child(self.episodes(cx)))
                    .children(self.more_like_this(width)),
            )
    }
}
