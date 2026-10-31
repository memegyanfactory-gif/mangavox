//! Studio › Suggestions: what manga recap viewers watch right now and what to recap next. A crawl
//! of public YouTube recap searches ([`mv_youtube::trends`]) ranks the manga being recapped by
//! demand, growth and competition; each manga is looked up in the user's installed sources so it
//! opens like it does from Browse (read, download, Studio). The channel meter scores the user's
//! channel against the niche and lists the fixes worth the most.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_sources::MangaSearchResult;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, empty_view, error_view, icon_ph, muted, page_title, ph, section_label, segmented, size, skeleton, spinner, tag, ButtonKind, Route, Tokens, Tone};
use mv_youtube::trends::{self, ChannelPage, TrendManga, TrendVideo, Trends};
use ui_manga::backend;

use crate::page::{card, card_title, column};

/// The channel the meter scores (`@handle`, link or channel id).
const CHANNEL_KEY: &str = "studio.suggestChannel";
/// A crawl older than this is redone when the page opens.
const STALE_SECS: i64 = 6 * 3600;
/// Best picks looked up in the sources straight away (the rest on request).
const AUTO_MATCH: usize = 10;
/// When the niche median is not known yet (no crawl), what a typical recap gets.
const DEFAULT_NICHE_VIEWS: u64 = 100_000;

fn cache_file() -> PathBuf {
    mv_core::paths::cache().join("studio-suggestions.json")
}

fn load_cache() -> Option<Trends> {
    serde_json::from_str(&std::fs::read_to_string(cache_file()).ok()?).ok()
}

fn save_cache(t: &Trends) {
    let f = cache_file();
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(f, serde_json::to_string(t).unwrap_or_default());
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// "1.2M", "34K", "950".
fn compact(n: f64) -> String {
    match n {
        n if n >= 1e6 => format!("{:.1}M", n / 1e6),
        n if n >= 1e4 => format!("{:.0}K", n / 1e3),
        n if n >= 1e3 => format!("{:.1}K", n / 1e3),
        n => format!("{n:.0}"),
    }
}

fn ago(hours: f64) -> String {
    match hours {
        h if h < 1.0 => "just now".into(),
        h if h < 24.0 => format!("{h:.0}h ago"),
        h if h < 24.0 * 14.0 => format!("{:.0}d ago", h / 24.0),
        h if h < 24.0 * 60.0 => format!("{:.0}w ago", h / 24.0 / 7.0),
        h => format!("{:.0}mo ago", h / 24.0 / 30.0),
    }
}

/// Label and tone of a 0-100 opportunity score.
fn grade(score: u8) -> (&'static str, Tone) {
    match score {
        80.. => ("Hot pick", Tone::Success),
        60..=79 => ("Strong", Tone::Accent),
        40..=59 => ("Worth a try", Tone::Warning),
        _ => ("Crowded", Tone::Neutral),
    }
}

/// A manga found in one of the user's installed sources.
#[derive(Clone)]
struct Hit {
    source_id: String,
    source_name: String,
    result: MangaSearchResult,
}

#[derive(Clone)]
enum Found {
    Searching,
    Hits(Vec<Hit>),
    /// No manga source is installed.
    NoSources,
}

/// Searches every installed source for `names` (the manga's name first, then its other names) and
/// keeps each source's closest title when it really is the same manga.
async fn find_in_sources(names: Vec<String>) -> Found {
    let reg = backend::registry().await;
    let sources = backend::installed_sources(reg.list());
    if sources.is_empty() {
        return Found::NoSources;
    }
    let searches = sources.into_iter().filter_map(|info| reg.get(&info.id).map(|s| (info, s))).map(|(info, src)| {
        let names = names.clone();
        async move {
            for n in names.iter().take(2) {
                let Some(Ok(list)) = backend::within(20, src.search_manga(n, 1)).await else { continue };
                let best = list
                    .into_iter()
                    .map(|r| (names.iter().map(|x| trends::similarity(&r.title, x)).fold(0.0, f64::max), r))
                    .filter(|(s, _)| *s >= 0.8)
                    .max_by(|a, b| a.0.total_cmp(&b.0));
                if let Some((_, result)) = best {
                    return Some(Hit { source_id: info.id.clone(), source_name: info.name.clone(), result });
                }
            }
            None
        }
    });
    Found::Hits(futures::future::join_all(searches).await.into_iter().flatten().collect())
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Picks,
    Growing,
    New,
}

pub struct SuggestionsPage {
    trends: Option<Result<Trends, String>>,
    crawling: bool,
    progress: Arc<Mutex<String>>,
    tab: Tab,
    /// By manga name (normalized).
    found: HashMap<String, Found>,
    /// Source result being opened.
    opening: Option<String>,
    channel_input: Entity<InputState>,
    channel: Option<Result<ChannelPage, String>>,
    channel_busy: bool,
    scroll: ScrollHandle,
    _sub: Subscription,
}

impl SuggestionsPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let saved = backend::config_str(CHANNEL_KEY).filter(|s| !s.trim().is_empty());
        let connected = crate::hub::hub(cx).read(cx).channel.as_ref().map(|c| c.handle.clone()).filter(|h| !h.is_empty());
        let start = saved.or(connected).unwrap_or_default();
        let channel_input = cx.new(|cx| InputState::new(window, cx).placeholder("@yourchannel or channel link").default_value(start.clone()));
        let sub = cx.subscribe(&channel_input, |this, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.analyze(cx);
            }
        });
        let mut p = Self {
            trends: None,
            crawling: false,
            progress: Arc::default(),
            tab: Tab::Picks,
            found: HashMap::new(),
            opening: None,
            channel_input,
            channel: None,
            channel_busy: false,
            scroll: ScrollHandle::new(),
            _sub: sub,
        };
        match load_cache() {
            Some(t) => {
                let stale = now() - t.fetched_at > STALE_SECS;
                p.trends = Some(Ok(t));
                p.match_top(cx);
                if stale {
                    p.crawl(cx);
                }
            }
            None => p.crawl(cx),
        }
        if !start.is_empty() {
            p.analyze(cx);
        }
        p
    }

    fn crawl(&mut self, cx: &mut Context<Self>) {
        if self.crawling {
            return;
        }
        self.crawling = true;
        *self.progress.lock().unwrap_or_else(|e| e.into_inner()) = "Searching YouTube".into();
        cx.notify();
        let progress = self.progress.clone();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move {
                trends::crawl(move |line| {
                    *progress.lock().unwrap_or_else(|e| e.into_inner()) = line;
                })
                .await
            })
            .await
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.crawling = false;
                match res {
                    Ok(t) => {
                        save_cache(&t);
                        this.trends = Some(Ok(t));
                        this.match_top(cx);
                    }
                    // A failed refresh keeps the last list.
                    Err(e) => match &this.trends {
                        Some(Ok(_)) => toast::show(cx, Toast::error(format!("Could not refresh suggestions: {e:#}"))),
                        _ => this.trends = Some(Err(format!("{e:#}"))),
                    },
                }
                cx.notify();
            });
        })
        .detach();
        // Redraw the progress line while the crawl runs.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(400)).await;
            let running = this
                .update(cx, |this, cx| {
                    cx.notify();
                    this.crawling
                })
                .unwrap_or(false);
            if !running {
                break;
            }
        })
        .detach();
    }

    /// Looks the best picks up in the sources, one manga at a time (every source in parallel).
    fn match_top(&mut self, cx: &mut Context<Self>) {
        let Some(Ok(t)) = &self.trends else { return };
        let todo: Vec<TrendManga> = t.manga.iter().take(AUTO_MATCH).filter(|m| !self.found.contains_key(&trends::normalize(&m.name))).cloned().collect();
        for m in &todo {
            self.found.insert(trends::normalize(&m.name), Found::Searching);
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            for m in todo {
                let names: Vec<String> = std::iter::once(m.name.clone()).chain(m.aliases.iter().cloned()).collect();
                let found = mv_ui::bg::run(find_in_sources(names)).await.unwrap_or(Found::Hits(vec![]));
                let stop = this
                    .update(cx, |this, cx| {
                        this.found.insert(trends::normalize(&m.name), found);
                        cx.notify();
                    })
                    .is_err();
                if stop {
                    break;
                }
            }
        })
        .detach();
    }

    fn find_one(&mut self, name: String, aliases: Vec<String>, cx: &mut Context<Self>) {
        let key = trends::normalize(&name);
        if matches!(self.found.get(&key), Some(Found::Searching)) {
            return;
        }
        self.found.insert(key.clone(), Found::Searching);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let names: Vec<String> = std::iter::once(name).chain(aliases).collect();
            let found = mv_ui::bg::run(find_in_sources(names)).await.unwrap_or(Found::Hits(vec![]));
            let _ = this.update(cx, |this, cx| {
                this.found.insert(key, found);
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens a source result the way Browse does: details and chapters synced, then the manga page.
    fn open_hit(&mut self, hit: Hit, window: &mut Window, cx: &mut Context<Self>) {
        if self.opening.is_some() {
            return;
        }
        self.opening = Some(hit.result.source_id.clone());
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let title = hit.result.title.clone();
            let src = hit.source_id.clone();
            let res = mv_ui::bg::run(ui_manga::ops::prepare_from_source(src.clone(), hit.result)).await.and_then(|r| r);
            let _ = this.update_in(cx, |this, w, cx| {
                this.opening = None;
                match res {
                    Ok(m) => mv_ui::navigate(Route::MangaDetail { manga_id: m.id, source_id: Some(src) }, w, cx),
                    Err(e) => toast::show(cx, Toast::error(format!("Could not open {title}: {e:#}"))),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn analyze(&mut self, cx: &mut Context<Self>) {
        let input = self.channel_input.read(cx).value().trim().to_string();
        if input.is_empty() || self.channel_busy {
            return;
        }
        backend::set_config_str(CHANNEL_KEY, &input);
        self.channel_busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move { trends::fetch_channel(&input).await }).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.channel_busy = false;
                this.channel = Some(res.map_err(|e| format!("{e:#}")));
                cx.notify();
            });
        })
        .detach();
    }

    // ------------------------------------------------------------------------- pieces

    fn niche_views(&self) -> u64 {
        match &self.trends {
            Some(Ok(t)) if t.niche_median_views > 0 => t.niche_median_views,
            _ => DEFAULT_NICHE_VIEWS,
        }
    }

    fn meter_card(&self, cx: &mut Context<Self>) -> Div {
        let input_row = div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .child(div().flex_1().min_w_0().child(Input::new(&self.channel_input).cleanable(true).prefix(icon_ph(ph::YOUTUBE_LOGO, Tokens::text_3(), icon_px::SM))))
            .child(btn("analyze", if self.channel_busy { "Checking" } else { "Check channel" }, ButtonKind::Secondary).icon(ph::GAUGE).loading(self.channel_busy).on_click(cx.listener(|this, _, _, cx| this.analyze(cx))));
        let mut c = card().child(card_title(ph::GAUGE, "Channel meter")).child(input_row);
        match &self.channel {
            None if self.channel_busy => c = c.child(div().flex().items_center().gap(px(space::SM)).child(spinner("meter-spin", Tokens::text_3(), 14.)).child(muted("Reading your channel…"))),
            None => c = c.child(muted("Enter your channel to see how ready it is to grow, and what to fix first.")),
            Some(Err(e)) => c = c.child(div().text_size(px(size::SM)).text_color(Tokens::danger()).child(e.clone())),
            Some(Ok(page)) => {
                let s = trends::score_channel(page, self.niche_views());
                let tone = match s.score {
                    0..=39 => Tokens::danger(),
                    40..=69 => Tokens::warning(),
                    _ => Tokens::success(),
                };
                let stats = [
                    page.subscribers.map(|n| format!("{} subscribers", compact(n as f64))),
                    Some(format!("{} videos", page.video_count.unwrap_or(page.videos.len() as u64))),
                    Some(format!("Typical recap in the niche: {} views", compact(self.niche_views() as f64))),
                ];
                let gauge = div()
                    .flex()
                    .items_center()
                    .gap(px(space::LG))
                    .child(crate::page::avatar(&page.avatar, 56.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .flex_1()
                            .min_w_0()
                            .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(page.name.clone()))
                            .child(muted(stats.into_iter().flatten().collect::<Vec<_>>().join(" · "))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_end()
                            .child(div().text_size(px(40.)).font_weight(FontWeight::BOLD).text_color(tone).child(format!("{}", s.score)))
                            .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(format!("/ 100 · {}", s.level))),
                    );
                let bar = |frac: f32, color: Rgba| div().h(px(8.)).w_full().rounded_full().bg(Tokens::active()).child(div().h_full().rounded_full().bg(color).w(relative(frac.clamp(0.0, 1.0))));
                let parts = div().flex().flex_col().gap(px(6.)).children(s.parts.iter().map(|(name, got, of)| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .child(div().w(px(110.)).flex_none().text_size(px(size::SM)).text_color(Tokens::text_2()).child(name.clone()))
                        .child(div().flex_1().child(bar(*got as f32 / *of as f32, Tokens::accent())))
                        .child(div().w(px(48.)).flex_none().text_size(px(size::SM)).text_color(Tokens::text_3()).child(format!("{got}/{of}")))
                }));
                let tips = div().flex().flex_col().gap(px(8.)).children(s.tips.iter().take(5).map(|t| {
                    div()
                        .flex()
                        .items_start()
                        .gap(px(space::SM))
                        .child(div().flex_none().child(tag(format!("+{}", t.points), Tone::Success)))
                        .child(div().flex_1().min_w_0().text_size(px(size::SM)).line_height(px(19.)).text_color(Tokens::text_1()).child(t.text.clone()))
                }));
                c = c.child(gauge).child(bar(s.score as f32 / 100.0, tone)).child(parts);
                if !s.tips.is_empty() {
                    c = c.child(section_label("Do these first")).child(tips);
                }
            }
        }
        c
    }

    fn sources_row(&self, key_id: &str, name: &str, aliases: &[String], cx: &mut Context<Self>) -> AnyElement {
        let key = trends::normalize(name);
        match self.found.get(&key) {
            None => {
                let (n, a) = (name.to_string(), aliases.to_vec());
                btn(SharedString::from(format!("find-{key_id}")), "Find in my sources", ButtonKind::Ghost)
                    .small()
                    .icon(ph::MAGNIFYING_GLASS)
                    .on_click(cx.listener(move |this, _, _, cx| this.find_one(n.clone(), a.clone(), cx)))
                    .into_any_element()
            }
            Some(Found::Searching) => div().flex().items_center().gap(px(6.)).child(spinner(SharedString::from(format!("find-spin-{key_id}")), Tokens::text_3(), 14.)).child(muted("Looking in your sources…")).into_any_element(),
            Some(Found::NoSources) => div()
                .flex()
                .items_center()
                .gap(px(space::SM))
                .child(muted("No manga source installed."))
                .child(btn(SharedString::from(format!("add-src-{key_id}")), "Add a source", ButtonKind::Ghost).small().icon(ph::PLUS).on_click(|_, w, cx| mv_ui::navigate(Route::Settings { tab: Some("sources".into()) }, w, cx)))
                .into_any_element(),
            Some(Found::Hits(h)) if h.is_empty() => div()
                .flex()
                .items_center()
                .gap(px(space::SM))
                .child(muted("Not in your sources yet: add MangaDex or an extension that has it."))
                .child(btn(SharedString::from(format!("add-src-{key_id}")), "Sources", ButtonKind::Ghost).small().icon(ph::PLUS).on_click(|_, w, cx| mv_ui::navigate(Route::Settings { tab: Some("sources".into()) }, w, cx)))
                .into_any_element(),
            Some(Found::Hits(hits)) => div()
                .flex()
                .flex_wrap()
                .gap(px(space::SM))
                .children(hits.iter().enumerate().map(|(i, h)| {
                    let hit = h.clone();
                    let busy = self.opening.as_deref() == Some(h.result.source_id.as_str());
                    btn(SharedString::from(format!("open-{key_id}-{i}")), if busy { "Opening".to_string() } else { format!("Open on {}", h.source_name) }, ButtonKind::Primary)
                        .small()
                        .icon(ph::BOOK_OPEN)
                        .loading(busy)
                        .tooltip(format!("{} · read, download or make a recap", h.result.title))
                        .on_click(cx.listener(move |this, _, w, cx| this.open_hit(hit.clone(), w, cx)))
                }))
                .into_any_element(),
        }
    }

    fn thumb(url: String, w: f32, h: f32) -> Div {
        div().flex_none().w(px(w)).h(px(h)).rounded(px(radius::SM)).overflow_hidden().bg(Tokens::well()).child(img(SharedString::from(url)).size_full().object_fit(ObjectFit::Cover))
    }

    fn video_chip(v: &TrendVideo, i: usize, prefix: &str) -> Stateful<Div> {
        let url = v.url();
        div()
            .id(SharedString::from(format!("{prefix}-vid-{i}")))
            .flex()
            .flex_col()
            .gap(px(4.))
            .w(px(176.))
            .cursor_pointer()
            .on_click(move |_, _, cx| cx.open_url(&url))
            .child(Self::thumb(v.thumbnail(), 176., 99.))
            .child(div().text_size(px(size::SM)).text_color(Tokens::text_1()).line_clamp(2).child(v.title.clone()))
            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{} · {} views · {}", v.channel, compact(v.views as f64), ago(v.age_hours))))
    }

    fn manga_card(&self, t: &Trends, rank: usize, m: &TrendManga, cx: &mut Context<Self>) -> Div {
        let (label, tone) = grade(m.score);
        let key_id = format!("m{rank}");
        let stats = format!(
            "{} views/day across recaps · {} channel{} · newest {} · {:.0}% of views from this week",
            compact(m.demand),
            m.channels,
            if m.channels == 1 { "" } else { "s" },
            ago(m.newest_hours),
            m.growth * 100.0
        );
        let why = match (m.channels, m.growth >= 0.6) {
            (1, true) => "Rising fast and only one channel covers it: get in early.",
            (1, false) => "Steady views and little competition.",
            (2..=3, true) => "Growing this week with few channels on it.",
            (_, true) => "Hot right now, but several channels already recap it: make yours stand out.",
            _ => "Popular, with competition: a fresh angle or a later arc can still win.",
        };
        let cover = if m.cover.is_empty() { None } else { Some(Self::thumb(m.cover.clone(), 84., 120.)) };
        let videos: Vec<&TrendVideo> = m.videos.iter().filter_map(|id| t.video(id)).take(4).collect();
        card()
            .child(
                div()
                    .flex()
                    .gap(px(space::LG))
                    .children(cover)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(space::SM))
                                    .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(format!("#{}", rank + 1)))
                                    .child(div().min_w_0().truncate().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(m.name.clone())),
                            )
                            .when(!m.aliases.is_empty(), |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).truncate().child(format!("Also: {}", m.aliases.iter().take(3).cloned().collect::<Vec<_>>().join(" · ")))))
                            .child(muted(stats))
                            .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(why))
                            .child(self.sources_row(&key_id, &m.name, &m.aliases, cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_end()
                            .gap(px(4.))
                            .flex_none()
                            .child(div().text_size(px(28.)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child(format!("{}", m.score)))
                            .child(tag(label, tone)),
                    ),
            )
            .when(!videos.is_empty(), |d| {
                d.child(section_label("Recaps of it on YouTube")).child(div().flex().flex_wrap().gap(px(space::MD)).children(videos.iter().enumerate().map(|(i, v)| Self::video_chip(v, i, &key_id))))
            })
    }

    fn video_row(&self, i: usize, v: &TrendVideo, prefix: &str, cx: &mut Context<Self>) -> Div {
        let url = v.url();
        let manga = v.manga.as_ref();
        card().child(
            div()
                .flex()
                .gap(px(space::LG))
                .child(div().id(SharedString::from(format!("{prefix}-thumb-{i}"))).cursor_pointer().on_click(move |_, _, cx| cx.open_url(&url)).child(Self::thumb(v.thumbnail(), 192., 108.)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).line_clamp(2).child(v.title.clone()))
                        .child(muted(format!("{} · {} views · {} · {}", v.channel, compact(v.views as f64), ago(v.age_hours), if v.length_secs > 0 { format!("{} min", v.length_secs / 60) } else { "long video".into() })))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::SM))
                                .child(tag(format!("{}/day", compact(v.velocity())), Tone::Success))
                                .map(|d| match manga {
                                    Some(m) => d.child(tag(format!("Manga: {}", m.name), Tone::Accent)),
                                    None => d.child(tag("Manga not named in the video", Tone::Neutral)),
                                }),
                        )
                        .when_some(manga, |d, m| d.child(self.sources_row(&format!("{prefix}{i}"), &m.name, &m.aliases, cx))),
                ),
        )
    }

    fn body(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = match &self.trends {
            None => return div().flex().flex_col().gap(px(space::MD)).children((0..3).map(|_| skeleton(relative(1.), px(150.), radius::LG))).into_any_element(),
            Some(Err(e)) => {
                return error_view(e.clone()).title("Could not read YouTube").retry(cx.listener(|this, _, _, cx| this.crawl(cx))).into_any_element();
            }
            Some(Ok(t)) => t,
        };
        match self.tab {
            Tab::Picks if t.manga.is_empty() => empty_view(ph::LIGHTBULB, "No manga named yet", "The recaps found did not name their manga. Refresh in a while.").into_any_element(),
            Tab::Picks => div().flex().flex_col().gap(px(space::MD)).children(t.manga.iter().take(30).enumerate().map(|(i, m)| self.manga_card(t, i, m, cx))).into_any_element(),
            Tab::Growing | Tab::New => {
                let (hours, prefix) = if self.tab == Tab::Growing { (24.0 * 7.0, "g") } else { (72.0, "n") };
                let list = t.rising(hours);
                if list.is_empty() {
                    return empty_view(ph::TREND_UP, "Nothing new yet", "No recap from this window was found. Refresh to look again.").into_any_element();
                }
                div().flex().flex_col().gap(px(space::MD)).children(list.into_iter().take(30).enumerate().map(|(i, v)| self.video_row(i, v, prefix, cx))).into_any_element()
            }
        }
    }
}

impl Render for SuggestionsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let updated = match (&self.trends, self.crawling) {
            (_, true) => self.progress.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            (Some(Ok(t)), false) => format!("Updated {}", ago((now() - t.fetched_at) as f64 / 3600.0)),
            _ => String::new(),
        };
        let refresh = btn("refresh", "Refresh", ButtonKind::Secondary).icon(ph::ARROW_CLOCKWISE).loading(self.crawling).on_click(cx.listener(|this, _, _, cx| this.crawl(cx)));
        let tab_ix = match self.tab {
            Tab::Picks => 0,
            Tab::Growing => 1,
            Tab::New => 2,
        };
        let tabs = segmented(
            "suggest-tabs",
            [(Some(ph::LIGHTBULB), "Best picks"), (Some(ph::TREND_UP), "Fastest growing"), (Some(ph::SPARKLE), "New & rising")],
            tab_ix,
        )
        .on_change(cx.listener(|this, ix: &usize, _, cx| {
            this.tab = match ix {
                0 => Tab::Picks,
                1 => Tab::Growing,
                _ => Tab::New,
            };
            cx.notify();
        }));
        let hint = match self.tab {
            Tab::Picks => "Manga being recapped right now, ranked for a new recap: views per day, growth this week, and how few channels cover it.",
            Tab::Growing => "Recaps from the last 7 days gaining views fastest: study their titles, thumbnails and length.",
            Tab::New => "Recaps from the last 3 days already pulling views: the newest wave to ride.",
        };
        let content = div()
            .flex()
            .flex_col()
            .gap(px(space::LG))
            .p(px(space::XL))
            .max_w(px(1100.))
            .w_full()
            .mx_auto()
            .child(
                page_title("Suggestions")
                    .icon(ph::LIGHTBULB)
                    .subtitle("What manga recap viewers watch right now, and what to recap next")
                    .action(div().flex().items_center().gap(px(space::SM)).child(muted(updated)).child(refresh)),
            )
            .child(self.meter_card(cx))
            .child(div().flex().flex_col().gap(px(space::SM)).child(tabs).child(muted(hint)))
            .child(self.body(cx));
        div()
            .size_full()
            .p(px(6.))
            .bg(Tokens::canvas())
            .child(column(div().size_full()).child(mv_ui::scroll_page("suggestions", &self.scroll).child(content)))
    }
}

#[cfg(test)]
mod tests {
    use super::{ago, compact, grade};

    #[test]
    fn formats() {
        assert_eq!(compact(1_234_567.0), "1.2M");
        assert_eq!(compact(45_600.0), "46K");
        assert_eq!(compact(1_500.0), "1.5K");
        assert_eq!(compact(950.0), "950");
        assert_eq!(ago(5.0), "5h ago");
        assert_eq!(ago(72.0), "3d ago");
        assert_eq!(grade(85).0, "Hot pick");
        assert_eq!(grade(10).0, "Crowded");
    }
}
