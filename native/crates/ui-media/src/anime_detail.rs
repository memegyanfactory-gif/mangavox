//! Anime detail, Crunchyroll-style: banner with poster, rating, tags and actions,
//! show facts, then an episode grid with thumbnails. Plays via addons.
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;

use mv_media::episode_art;
use mv_media::types::{AnimeEpisode, AnimeSearchResult};
use mv_ui::{bg, button, empty_state, error_state, icon, navigate, size, ButtonKind, Route, Tokens};

use mv_core::cache::DataCache;

use crate::common::{cached, fetch, fetch_cached, in_watchlist, jikan, media_id, media_title, muted, record_open, set_watchlist, Load, DAY, EPISODES_TTL};
use crate::look::{self, crunchy, EDGE};
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
    /// The first source lookup has been started.
    auto: bool,
    /// Episode whose links are shown.
    sel: Option<u32>,
    /// Addon chip selected in the sources panel.
    addon_filter: Option<String>,
    /// Full synopsis shown under the hero.
    synopsis_open: bool,
    scroll: ScrollHandle,
}

/// "in 3 days" / "in 5 hours" until a unix timestamp.
fn airs_in(at: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let s = (at - now).max(0);
    match (s / 86400, s / 3600) {
        (d, _) if d >= 2 => format!("in {d} days"),
        (1, _) => "tomorrow".into(),
        (_, h) if h >= 1 => format!("in {h} hours"),
        _ => "soon".into(),
    }
}

impl AnimeDetail {
    pub fn new(mal_id: String, title: String, cx: &mut Context<Self>) -> Self {
        let (id, t) = (mal_id.clone(), title.clone());
        let key = format!("jikan:anime:{mal_id}");
        let warm = DataCache::shared().age(&key).is_some();
        let details = fetch_cached(
            cx,
            key,
            DAY,
            async move {
                let d = jikan()?.anime_details(&id).await.map_err(|e| e.to_string())?;
                if !warm {
                    let _ = record_open(media_title("anime", &id, &d, &t)).await;
                }
                Ok(d)
            },
            |v: &mut Self, l| v.details = l,
        );
        if let Load::Ready(d) = &details {
            fetch(cx, record_open(media_title("anime", &mal_id, d, &title)), |_: &mut Self, _| {});
        }
        fetch(cx, in_watchlist(media_id("anime", &mal_id)), |v: &mut Self, l| v.in_list = l.ready().copied());
        let mut page = Self {
            mal_id,
            title,
            details,
            episodes: Load::Loading,
            page: 1,
            has_more: false,
            in_list: None,
            streams: StreamState::Idle,
            auto: false,
            sel: None,
            addon_filter: None,
            synopsis_open: false,
            scroll: ScrollHandle::new(),
        };
        page.load_page(1, cx);
        page
    }

    /// Fetch one page of episodes (page 1 replaces the list), then their stills.
    fn load_page(&mut self, page: u32, cx: &mut Context<Self>) {
        let id = self.mal_id.clone();
        let key = format!("jikan:episodes:{id}:{page}");
        // Page 1 comes from the cache at once; it is refetched only when stale.
        if let Some((eps, age)) = DataCache::shared().get::<Vec<AnimeEpisode>>(&key).filter(|_| page == 1) {
            self.has_more = eps.len() >= 100;
            self.episodes = Load::Ready(eps);
            self.fill_art(cx);
            if age < EPISODES_TTL {
                return;
            }
        }
        cx.spawn(async move |this, cx| {
            let res = bg::run(cached(key, async move { jikan()?.anime_episodes(&id, page).await.map_err(|e| e.to_string()) })).await.unwrap_or_else(|e| Err(e.to_string()));
            let _ = this.update(cx, |v, cx| {
                let l: Load<Vec<AnimeEpisode>> = res.into();
                if page == 1 && matches!(l, Load::Failed(_)) && v.episodes.ready().is_some() {
                    return;
                }
                v.has_more = l.ready().is_some_and(|e| e.len() >= 100);
                match (&mut v.episodes, l) {
                    (Load::Ready(cur), Load::Ready(more)) if page > 1 => cur.extend(more),
                    (_, l) if page == 1 => v.episodes = l,
                    _ => {}
                }
                v.fill_art(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Give episodes without a still their own (Kitsu / IMDB), instead of the series banner on every card.
    fn fill_art(&mut self, cx: &mut Context<Self>) {
        let art_key = format!("episode-art:{}", self.mal_id);
        let Load::Ready(eps) = &mut self.episodes else { return };
        let known: HashMap<u32, String> = DataCache::shared().get(&art_key).map(|(m, _)| m).unwrap_or_default();
        for e in eps.iter_mut().filter(|e| e.thumbnail.is_none()) {
            e.thumbnail = known.get(&e.number).cloned();
        }
        let mut missing: Vec<AnimeEpisode> = eps.iter().filter(|e| e.thumbnail.is_none()).cloned().collect();
        if missing.is_empty() {
            return;
        }
        let finished = self.details.ready().filter(|d| d.status.as_deref().is_some_and(|s| s.to_lowercase().contains("finished")));
        let expected = finished.and_then(|d| d.episode_count);
        let id = self.mal_id.clone();
        cx.spawn(async move |this, cx| {
            let Ok(filled) = bg::run(async move {
                episode_art::fill_thumbnails(&id, &mut missing, expected).await;
                missing
            })
            .await
            else {
                return;
            };
            let art: HashMap<u32, String> = filled.into_iter().filter_map(|e| Some((e.number, e.thumbnail?))).collect();
            if !art.is_empty() {
                let mut all = known;
                all.extend(art.clone());
                let _ = bg::blocking(move || DataCache::shared().put(&art_key, &all)).await;
            }
            let _ = this.update(cx, |v, cx| {
                if let Load::Ready(cur) = &mut v.episodes {
                    for e in cur.iter_mut().filter(|e| e.thumbnail.is_none()) {
                        e.thumbnail = art.get(&e.number).cloned();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn is_movie(&self) -> bool {
        self.details.ready().is_some_and(|d| d.anime_type.as_deref() == Some("Movie"))
    }

    fn display_title(&self) -> String {
        self.details.ready().map(|d| d.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| self.title.clone())
    }

    /// Play the best known link at once, otherwise look it up and play.
    fn play(&mut self, episode: u32, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(req) = streams::best(&self.streams, Some(1), Some(episode)) {
            navigate(Route::Player { request: req }, window, cx);
            return;
        }
        self.lookup(episode, true, window, cx);
    }

    /// Find links for an episode and list them on the page.
    fn lookup(&mut self, episode: u32, autoplay: bool, window: &mut Window, cx: &mut Context<Self>) {
        let movie = self.is_movie();
        self.sel = Some(episode);
        self.addon_filter = None;
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
        streams::find(cx, window, label, base, movie, autoplay, |v: &mut Self, s| v.streams = s);
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        self.page += 1;
        self.has_more = false;
        self.load_page(self.page, cx);
    }

    fn toggle_list(&mut self, cx: &mut Context<Self>) {
        let Some(cur) = self.in_list else { return };
        let fallback = AnimeSearchResult { title: self.title.clone(), ..Default::default() };
        let t = media_title("anime", &self.mal_id, self.details.ready().unwrap_or(&fallback), &self.title);
        self.in_list = None;
        cx.notify();
        fetch(cx, set_watchlist(t, !cur), move |v: &mut Self, l| v.in_list = Some(l.ready().copied().unwrap_or(cur)));
    }

    fn hero(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let height = 480.;
        let movie = self.is_movie();
        let first = self.episodes.ready().and_then(|e| e.first().map(|e| e.number)).unwrap_or(1);
        let Load::Ready(d) = &self.details else {
            let failed = match &self.details {
                Load::Failed(e) => Some(e.clone()),
                _ => None,
            };
            return look::billboard("anime-detail-hero", height, None).child(
                look::hero_copy(70.)
                    .child(div().text_size(px(40.)).font_weight(FontWeight::BLACK).text_color(Tokens::text_1()).child(SharedString::from(self.title.clone())))
                    .child(match failed {
                        Some(e) => error_state(e).min_h(px(60.)).into_any_element(),
                        None => div().w(px(420.)).h(px(60.)).rounded(px(6.)).bg(Tokens::surface()).into_any_element(),
                    }),
            );
        };
        let tags = [d.anime_type.clone(), d.status.clone(), d.episode_count.filter(|_| !movie).map(|n| format!("{n} episodes")), d.duration_minutes.map(|m| format!("{m} min"))];
        let (bm_icon, bm_label) = match self.in_list {
            Some(true) => ("icons/bookmark-check.svg", "IN WATCHLIST"),
            _ => ("icons/bookmark-plus.svg", "ADD TO WATCHLIST"),
        };
        let heading = match (movie, self.sel) {
            (false, Some(e)) => {
                let name = self.episodes.ready().and_then(|v| v.iter().find(|x| x.number == e)).and_then(|x| x.title.clone()).filter(|t| !t.is_empty());
                format!("Episode {e}{}", name.map(|n| format!("  {n}")).unwrap_or_default())
            }
            _ => "Sources".to_string(),
        };
        let panel = streams::sources_panel(&self.streams, crunchy(), heading, self.addon_filter.as_deref(), cx, |v: &mut Self, f| v.addon_filter = f);
        look::billboard("anime-detail-hero", height, d.banner_url.clone().or_else(|| d.cover_url.clone()))
            .child(
                look::hero_copy(56.)
                    .w(px(640.))
                    .when(!matches!(self.streams, StreamState::Idle), |d| d.max_w(relative(0.5)))
                    .child(div().text_size(px(40.)).line_height(px(46.)).font_weight(FontWeight::BLACK).text_color(Tokens::text_1()).line_clamp(2).child(SharedString::from(d.title.clone())))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.))
                            .children(d.score.map(|s| {
                                div().flex().items_center().gap(px(4.)).mr(px(4.)).text_size(px(size::LG)).font_weight(FontWeight::BOLD).text_color(crunchy()).child(icon("icons/star.svg", crunchy(), 16.)).child(SharedString::from(format!("{s:.1}")))
                            }))
                            .children(tags.into_iter().flatten().map(look::tag)),
                    )
                    .child(div().text_size(px(size::MD)).text_color(Tokens::text_2()).child(SharedString::from(d.genres.clone().unwrap_or_default().join(", "))))
                    .child(div().text_size(px(size::LG)).line_height(px(22.)).text_color(Tokens::text_1()).line_clamp(3).child(SharedString::from(d.description.as_deref().map(look::blurb).unwrap_or_else(|| "No synopsis available.".into()))))
                    .when(d.description.as_deref().is_some_and(|t| t.len() > look::blurb(t).len() + 40 || t.len() > 260), |c| {
                        c.child(
                            div()
                                .id("synopsis-more")
                                .text_size(px(size::SM))
                                .font_weight(FontWeight::BOLD)
                                .text_color(Tokens::text_2())
                                .cursor_pointer()
                                .hover(|s| s.text_color(crunchy()))
                                .child(if self.synopsis_open { "LESS" } else { "MORE DETAILS" })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.synopsis_open = !this.synopsis_open;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .gap(px(10.))
                            .mt(px(4.))
                            .child(
                                look::cta("play-main", if movie { "WATCH NOW".to_string() } else { format!("START WATCHING E{first}") }, crunchy(), rgb(0x000000))
                                    .rounded(px(0.))
                                    .on_click(cx.listener(move |this, _, w, cx| this.play(first, w, cx))),
                            )
                            .child(
                                div()
                                    .id("toggle-list")
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
                                    .when(self.in_list.is_none(), |d| d.opacity(0.5))
                                    .child(icon(bm_icon, crunchy(), 16.))
                                    .child(bm_label)
                                    .on_click(cx.listener(|this, _, _, cx| this.toggle_list(cx))),
                            ),
                    ),
            )
            .map(|hero| match panel {
                Some(p) => hero.child(div().absolute().right(px(EDGE)).top(px(24.)).bottom(px(24.)).w(px(430.)).child(p)),
                None => hero                .child(
                div()
                    .absolute()
                    .right(px(EDGE))
                    .bottom(px(56.))
                    .w(px(220.))
                    .h(px(330.))
                    .rounded(px(4.))
                    .overflow_hidden()
                    .border_1()
                    .border_color(rgba(0xffffff26))
                    .shadow_lg()
                    .bg(Tokens::surface_2())
                    .when_some(d.cover_url.clone(), |x, u| x.child(look::pic(u))),
            ),
            })
    }

    /// The whole synopsis, when "More details" is open.
    fn synopsis(&self) -> Option<Div> {
        let text = self.details.ready()?.description.clone().filter(|_| self.synopsis_open)?;
        Some(div().px(px(EDGE)).mb(px(28.)).max_w(px(900.)).text_size(px(size::MD)).line_height(px(22.)).text_color(Tokens::text_2()).child(SharedString::from(text)))
    }

    /// Studio / next episode facts under the hero.
    fn facts(&self) -> Option<Div> {
        let d = self.details.ready()?;
        let mut items: Vec<(&str, String)> = vec![];
        if let Some(s) = d.studios.as_ref().filter(|s| !s.is_empty()) {
            items.push(("Studio", s.join(", ")));
        }
        if let (Some(n), Some(at)) = (d.next_episode, d.next_episode_at) {
            items.push(("Next episode", format!("Episode {n} airs {}", airs_in(at))));
        }
        if let Some(y) = d.year {
            items.push(("Year", y.to_string()));
        }
        if let Some(c) = d.cast.as_ref().filter(|c| !c.is_empty()) {
            items.push(("Voices", c.iter().take(4).cloned().collect::<Vec<_>>().join(", ")));
        }
        if items.is_empty() {
            return None;
        }
        Some(div().flex().flex_wrap().gap_x(px(40.)).gap_y(px(12.)).px(px(EDGE)).mb(px(32.)).children(items.into_iter().map(|(k, v)| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(k.to_uppercase()))
                .child(div().text_size(px(size::MD)).text_color(Tokens::text_1()).child(SharedString::from(v)))
        })))
    }

    fn episodes(&self, width: f32, cx: &mut Context<Self>) -> Div {
        let per = look::per_row(width, 260., 16.);
        let w = ((width - EDGE * 2.0 - 16.0 * (per as f32 - 1.0)) / per as f32).clamp(220., 340.);
        let h = w * 9. / 16.;
        let fallback_img = self.details.ready().and_then(|d| d.banner_url.clone().or_else(|| d.cover_url.clone()));
        let count = self.episodes.ready().map(|v| v.len()).unwrap_or(0);
        let body = match &self.episodes {
            Load::Loading => div().flex().flex_wrap().gap(px(16.)).children((0..8).map(|_| div().w(px(w)).h(px(h)).rounded(px(4.)).bg(Tokens::surface_2()))),
            Load::Failed(e) => error_state(e.clone()).min_h(px(100.)),
            Load::Ready(v) if v.is_empty() => empty_state("No episode list", "Use the watch button above to look for sources.").min_h(px(100.)),
            Load::Ready(v) => div().flex().flex_wrap().gap_x(px(16.)).gap_y(px(24.)).children(v.iter().map(|ep| {
                let n = ep.number;
                let group: SharedString = format!("aep-{n}").into();
                let title = ep.title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| format!("Episode {n}"));
                div()
                    .id(("ep", n as usize))
                    .group(group.clone())
                    .w(px(w))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .cursor_pointer()
                    .child(
                        div()
                            .relative()
                            .w(px(w))
                            .when(self.sel == Some(n), |d| d.border_2().border_color(crunchy()))
                            .h(px(h))
                            .rounded(px(4.))
                            .overflow_hidden()
                            .bg(Tokens::surface_2())
                            .when_some(ep.thumbnail.clone().or_else(|| fallback_img.clone()), |d, u| d.child(look::pic(u)))
                            .child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(rgba(0x00000080))
                                    .opacity(0.)
                                    .group_hover(group.clone(), |s| s.opacity(1.))
                                    .child(icon("icons/play.svg", crunchy(), 40.)),
                            )
                            .child(div().absolute().right(px(6.)).bottom(px(6.)).px(px(5.)).rounded(px(3.)).bg(rgba(0x000000c0)).text_size(px(size::XS)).text_color(Tokens::text_1()).child(SharedString::from(format!("E{n}")))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(SharedString::from(format!("Episode {n}"))))
                            .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).line_clamp(2).group_hover(group, |s| s.text_color(crunchy())).child(SharedString::from(title))),
                    )
                    .on_click(cx.listener(move |this, _, w, cx| {
                        // Links show in the panel at the top, like Stremio.
                        this.lookup(n, false, w, cx);
                        this.scroll.set_offset(point(px(0.), px(0.)));
                    }))
            })),
        };
        div()
            .px(px(EDGE))
            .mb(px(40.))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(10.))
                    .mb(px(16.))
                    .pb(px(10.))
                    .border_b_1()
                    .border_color(Tokens::border())
                    .child(div().text_size(px(size::XL)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child("Episodes"))
                    .when(count > 0, |d| d.child(muted(format!("{count}{}", if self.has_more { "+" } else { "" })))),
            )
            .child(body)
            .when(self.has_more, |d| {
                d.child(div().mt(px(20.)).flex().justify_center().child(button("more-eps", "Load more episodes", ButtonKind::Secondary).on_click(cx.listener(|this, _, _, cx| this.load_more(cx)))))
            })
    }
}

impl Render for AnimeDetail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = look::page_width(&self.scroll, window);
        let movie = self.is_movie();
        // Links load by themselves once we know the first episode (or that it is a movie).
        if !self.auto && self.details.ready().is_some() {
            let first = if movie { Some(1) } else { self.episodes.ready().map(|e| e.first().map(|e| e.number).unwrap_or(1)) };
            if let Some(ep) = first {
                self.auto = true;
                self.lookup(ep, false, window, cx);
            }
        }
        div()
            .relative()
            .size_full()
            .bg(Tokens::base())
            .child(
                mv_ui::scroll_page("anime-detail", &self.scroll)
                    .child(self.hero(cx))
                    .children(self.synopsis())
                    .children(self.facts())
                    .when(!movie, |d| d.child(self.episodes(width, cx))),
            )
    }
}
