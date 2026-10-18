use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;

use anyhow::anyhow;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::manga;
use mv_sources::{MangaSearchResult, SourceInfo};
use mv_ui::{button, page_header, size, ButtonKind, Route, Tokens};

use crate::backend;
use crate::ops;
use crate::ui::{
    cover_src, error_view, manga_card, note_line, search_box, skeleton_cover, skeleton_rows, spawn_cover_downloads, spinner, state_view, tab, tag, width_probe,
    CardInfo, Grid, Load,
};

/// Last source picked in Browse (restored next time).
const SOURCE_KEY: &str = "browseSource";

#[derive(Clone, PartialEq, Debug)]
enum Mode {
    Popular,
    Latest,
    Search(String),
}

/// `source \u{1} source_id` -> (manga id, shown in the library).
type Known = HashMap<String, (String, bool)>;

fn known_key(source: &str, source_id: &str) -> String {
    format!("{source}\u{1}{source_id}")
}

fn load_known() -> anyhow::Result<Known> {
    let hidden = backend::browse_only();
    backend::with_db(|c| {
        Ok(manga::list(c)?.into_iter().map(|m| (known_key(&m.source_plugin, &m.source_id), (m.id.clone(), !hidden.contains(&m.id)))).collect())
    })
}

pub struct BrowsePage {
    sources: Load<Vec<SourceInfo>>,
    selected: Option<String>,
    mode: Mode,
    page: u32,
    results: Load<Vec<MangaSearchResult>>,
    /// Next page is being fetched (infinite scroll).
    loading_more: bool,
    /// The source returned an empty / failing page: stop asking for more.
    exhausted: bool,
    scroll: UniformListScrollHandle,
    covers: HashMap<String, PathBuf>,
    input: Entity<InputState>,
    generation: u64,
    /// Result being opened (`source_id`, title).
    busy: Option<(String, String)>,
    note: Option<String>,
    /// Titles already in the database, for the "In library" marker and instant opening.
    known: Known,
    seen_version: u64,
    width: f32,
    _sub: Subscription,
}

impl BrowsePage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search this source"));
        let sub = cx.subscribe_in(&input, window, |this, input, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                let q = input.read(cx).value().trim().to_string();
                this.mode = if q.is_empty() { Mode::Popular } else { Mode::Search(q) };
                this.fetch(cx);
            }
        });
        cx.spawn(async move |this, cx| {
            let list = mv_ui::bg::run(async { backend::registry().await.list() }).await;
            let _ = this.update(cx, |this, cx| {
                match list {
                    Ok(list) => {
                        let off = backend::disabled_sources();
                        let list: Vec<SourceInfo> = list.into_iter().filter(|s| !off.contains(&s.id)).collect();
                        let last = backend::config_str(SOURCE_KEY).filter(|id| list.iter().any(|s| &s.id == id));
                        this.selected = last.or_else(|| list.first().map(|s| s.id.clone()));
                        this.sources = Load::Ready(list);
                        this.fetch(cx);
                    }
                    Err(e) => this.sources = Load::Failed(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        Self {
            sources: Load::Loading,
            selected: None,
            mode: Mode::Popular,
            page: 1,
            results: Load::Loading,
            loading_more: false,
            exhausted: false,
            scroll: UniformListScrollHandle::new(),
            covers: HashMap::new(),
            input,
            generation: 0,
            busy: None,
            note: None,
            known: HashMap::new(),
            seen_version: u64::MAX,
            width: 0.,
            _sub: sub,
        }
    }

    fn reload_known(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(known)) = mv_ui::bg::blocking(load_known).await {
                let _ = this.update(cx, |this, cx| {
                    this.known = known;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn select_source(&mut self, id: String, cx: &mut Context<Self>) {
        if self.selected.as_deref() == Some(id.as_str()) && self.mode == Mode::Popular {
            return;
        }
        backend::set_config_str(SOURCE_KEY, &id);
        self.selected = Some(id);
        self.mode = Mode::Popular;
        self.fetch(cx);
    }

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.mode = mode;
        self.fetch(cx);
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        let Some(src) = self.selected.clone() else {
            self.results = Load::Ready(vec![]);
            cx.notify();
            return;
        };
        self.generation += 1;
        self.page = 1;
        self.results = Load::Loading;
        self.loading_more = false;
        self.exhausted = false;
        self.note = None;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
        self.request(src, 1, cx);
    }

    /// Fetch the next page and append it (called when the grid nears its end).
    fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.loading_more || self.exhausted || !matches!(self.results, Load::Ready(_)) {
            return;
        }
        let Some(src) = self.selected.clone() else { return };
        self.loading_more = true;
        self.request(src, self.page + 1, cx);
    }

    fn request(&mut self, src: String, page: u32, cx: &mut Context<Self>) {
        let generation = self.generation;
        let mode = self.mode.clone();
        let sid = src.clone();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move {
                let s = backend::source(&sid).await?;
                match mode {
                    Mode::Popular => s.get_popular_manga(page).await,
                    Mode::Latest => s.get_latest_manga(page).await,
                    Mode::Search(q) => s.search_manga(&q, page).await,
                }
                .map_err(|e| anyhow!("{e}"))
            })
            .await
            .and_then(|r| r);
            let covers = match &res {
                Ok(list) => {
                    let urls: Vec<_> = list.iter().filter_map(|r| r.cover_url.clone().map(|u| (u, Some(src.clone())))).collect();
                    mv_ui::bg::blocking(move || backend::resolve_covers(urls)).await.ok()
                }
                Err(_) => None,
            };
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match (res, &mut this.results) {
                    (Ok(list), Load::Ready(cur)) if page > 1 => {
                        // Sources often repeat items across pages; keep the grid unique.
                        let before = cur.len();
                        for it in list {
                            if !cur.iter().any(|c| c.source_id == it.source_id) {
                                cur.push(it);
                            }
                        }
                        this.exhausted = cur.len() == before;
                        this.page = page;
                    }
                    (Ok(list), _) => {
                        this.exhausted = list.is_empty();
                        this.results = Load::Ready(list);
                    }
                    (Err(e), Load::Ready(_)) => {
                        this.exhausted = true;
                        this.note = Some(format!("Could not load more: {e:#}"));
                    }
                    (Err(e), _) => this.results = Load::Failed(format!("{e:#}")),
                }
                this.loading_more = false;
                if let Some((have, missing)) = covers {
                    this.covers.extend(have);
                    spawn_cover_downloads(cx, missing, |t: &mut Self| &mut t.covers);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_result(&mut self, r: MangaSearchResult, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let Some(src) = self.selected.clone() else { return };
        // Already in the database: open it straight away.
        if let Some((manga_id, _)) = self.known.get(&known_key(&src, &r.source_id)) {
            mv_ui::navigate(Route::MangaDetail { manga_id: manga_id.clone(), source_id: Some(src) }, window, cx);
            return;
        }
        self.busy = Some((r.source_id.clone(), r.title.clone()));
        self.note = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let res = mv_ui::bg::run(ops::prepare_from_source(src.clone(), r)).await.and_then(|r| r);
            let _ = this.update_in(cx, |this, w, cx| {
                this.busy = None;
                match res {
                    Ok(m) => {
                        this.known.insert(known_key(&m.source_plugin, &m.source_id), (m.id.clone(), false));
                        mv_ui::navigate(Route::MangaDetail { manga_id: m.id, source_id: Some(src) }, w, cx)
                    }
                    Err(e) => this.note = Some(format!("Could not open: {e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn grid(&self, grid: Grid, cx: &mut Context<Self>) -> AnyElement {
        let Load::Ready(list) = &self.results else { return div().into_any_element() };
        if list.is_empty() {
            let (title, body) = match &self.mode {
                Mode::Search(q) => ("No results", format!("Nothing on this source matches \"{q}\".")),
                _ => ("Nothing here", "This source returned no titles.".to_string()),
            };
            return state_view("icons/search.svg", title, body).into_any_element();
        }
        let rows = grid.rows(list.len());
        // One extra row of skeletons while there may be more to load.
        let footer = !self.exhausted;
        uniform_list(
            "browse-grid",
            rows + footer as usize,
            cx.processor(move |this: &mut Self, range: Range<usize>, _w, cx| {
                if range.end + 2 >= rows {
                    this.load_more(cx);
                }
                let Load::Ready(list) = &this.results else { return vec![] };
                let src = this.selected.clone().unwrap_or_default();
                let entity = cx.entity();
                range
                    .map(|r| {
                        if r >= rows {
                            return grid.row().children((0..grid.cols).map(|_| skeleton_cover(grid)));
                        }
                        grid.row().children(list.iter().skip(r * grid.cols).take(grid.cols).map(|it| {
                            let item = it.clone();
                            let entity = entity.clone();
                            let in_library = this.known.get(&known_key(&src, &it.source_id)).is_some_and(|k| k.1);
                            let busy = this.busy.as_ref().is_some_and(|b| b.0 == it.source_id);
                            let info = CardInfo {
                                title: it.title.clone().into(),
                                cover: cover_src(&this.covers, it.cover_url.as_deref()),
                                in_library,
                                ..Default::default()
                            };
                            manga_card(SharedString::from(format!("br-{}", it.source_id)), info, grid)
                                .when(busy, |d| {
                                    d.child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .left_0()
                                            .size_full()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .bg(Tokens::scrim())
                                            .child(spinner(SharedString::from(format!("spin-{}", it.source_id)), 26.)),
                                    )
                                })
                                .on_click(move |_, w, cx| entity.update(cx, |t, cx| t.open_result(item.clone(), w, cx)))
                        }))
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.scroll)
        .size_full()
        .into_any_element()
    }

    /// Source chips: icon (or initial), name, language and an 18+ marker.
    fn source_picker(&self, list: &[SourceInfo], cx: &mut Context<Self>) -> Div {
        div().flex().flex_wrap().gap(px(8.)).px(px(size::PAGE_X)).mb(px(14.)).children(list.iter().map(|s| {
            let id = s.id.clone();
            let active = self.selected.as_deref() == Some(s.id.as_str());
            let letter: String = s.name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
            div()
                .id(SharedString::from(format!("src-{}", s.id)))
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(36.))
                .pl(px(6.))
                .pr(px(10.))
                .rounded(px(10.))
                .border_1()
                .cursor_pointer()
                .map(|d| {
                    if active {
                        d.bg(Tokens::accent_soft()).border_color(Tokens::accent())
                    } else {
                        d.bg(Tokens::surface()).border_color(Tokens::border()).hover(|s| s.bg(Tokens::hover()))
                    }
                })
                .child(
                    div()
                        .size(px(24.))
                        .flex_none()
                        .rounded(px(6.))
                        .overflow_hidden()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(if active { Tokens::accent() } else { Tokens::surface_2() })
                        .text_size(px(size::XS))
                        .font_weight(FontWeight::BOLD)
                        .text_color(if active { Tokens::base() } else { Tokens::text_2() })
                        .map(|d| match s.icon.clone().filter(|u| u.starts_with("http")) {
                            Some(u) => d.child(img(SharedString::from(u)).size_full().object_fit(ObjectFit::Cover)),
                            None => d.child(letter),
                        }),
                )
                .child(
                    div()
                        .text_size(px(size::SM))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if active { Tokens::text_1() } else { Tokens::text_2() })
                        .child(s.name.clone()),
                )
                .when(!s.lang.is_empty(), |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(s.lang.to_uppercase())))
                .when(s.nsfw, |d| d.child(div().text_size(px(size::XS)).font_weight(FontWeight::BOLD).text_color(Tokens::danger()).child("18+")))
                .on_click(cx.listener(move |this, _, _, cx| this.select_source(id.clone(), cx)))
        }))
    }
}

impl Render for BrowsePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let v = backend::data_version();
        if v != self.seen_version {
            self.seen_version = v;
            self.reload_known(cx);
        }
        let grid = Grid::new(self.width, window);
        let current = match &self.sources {
            Load::Ready(list) => list.iter().find(|s| Some(&s.id) == self.selected.as_ref()).cloned(),
            _ => None,
        };
        let subtitle = current.as_ref().map(|s| SharedString::from(s.name.clone()));
        let mut root = div().size_full().flex().flex_col().pt(px(size::PAGE_Y)).child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(size::PAGE_X))
                .child(div().flex_1().min_w_0().child(page_header("Browse", subtitle)))
                .when(current.is_some(), |d| d.child(div().mb(px(20.)).child(search_box(&self.input)))),
        );
        match &self.sources {
            Load::Loading => return root.child(skeleton_rows(grid, 2)),
            Load::Failed(e) => return root.child(error_view(e.clone())),
            Load::Ready(list) if list.is_empty() => {
                return root.child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .child(state_view("icons/compass.svg", "No sources enabled", "Install or enable a source to start browsing."))
                        .child(button("open-sources", "Open source settings", ButtonKind::Primary).on_click(|_, w, cx| {
                            mv_ui::navigate(Route::Settings { tab: Some("sources".into()) }, w, cx)
                        })),
                )
            }
            Load::Ready(list) => root = root.child(self.source_picker(list, cx)),
        }
        let search = match &self.mode {
            Mode::Search(q) => Some(q.clone()),
            _ => None,
        };
        let tabs = div()
            .flex()
            .items_center()
            .px(px(size::PAGE_X))
            .mb(px(14.))
            .border_b_1()
            .border_color(Tokens::border())
            .child(tab("m-popular", "Popular", None, self.mode == Mode::Popular).on_click(cx.listener(|this, _, _, cx| this.set_mode(Mode::Popular, cx))))
            .child(tab("m-latest", "Latest", None, self.mode == Mode::Latest).on_click(cx.listener(|this, _, _, cx| this.set_mode(Mode::Latest, cx))))
            .when_some(search, |d, q| d.child(tab("m-search", format!("Search: {q}"), None, true)))
            .child(div().flex_1())
            .when(self.loading_more, |d| d.child(div().mr(px(8.)).child(spinner("more-spin", 14.))))
            .when_some(self.busy.clone(), |d, (_, t)| d.child(div().mr(px(8.)).child(note_line(format!("Opening {t}..."), false))))
            .when_some(self.note.clone(), |d, n| d.child(div().mr(px(8.)).child(note_line(n, true))))
            .when(current.as_ref().is_some_and(|s| s.nsfw), |d| d.child(tag("18+")));
        let body: AnyElement = match &self.results {
            Load::Loading => skeleton_rows(grid, 3).into_any_element(),
            Load::Failed(e) => div()
                .flex()
                .flex_col()
                .items_center()
                .child(error_view(e.clone()))
                .child(button("retry", "Try again", ButtonKind::Secondary).on_click(cx.listener(|this, _, _, cx| this.fetch(cx))))
                .into_any_element(),
            Load::Ready(_) => self.grid(grid, cx),
        };
        root.child(tabs).child(div().relative().flex_1().min_h_0().child(width_probe(cx, |t: &mut Self| &mut t.width)).child(body))
    }
}
