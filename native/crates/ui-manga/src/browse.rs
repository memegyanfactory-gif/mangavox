use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;

use anyhow::anyhow;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_sources::{MangaSearchResult, SourceInfo};
use mv_ui::{button, chip, cover_card, empty_state, error_state, page_header, size, ButtonKind, Route, Tokens};

use crate::backend;
use crate::logic;
use crate::ops;
use crate::ui::{cover_src, muted, search_box, skeleton_grid, spawn_cover_downloads, Load, ROW_H};

#[derive(Clone, PartialEq, Debug)]
enum Mode {
    Popular,
    Latest,
    Search(String),
}

pub struct BrowsePage {
    sources: Load<Vec<SourceInfo>>,
    selected: Option<String>,
    mode: Mode,
    page: u32,
    results: Load<Vec<MangaSearchResult>>,
    covers: HashMap<String, PathBuf>,
    input: Entity<InputState>,
    generation: u64,
    busy: Option<String>,
    note: Option<String>,
    _sub: Subscription,
}

impl BrowsePage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search this source"));
        let sub = cx.subscribe_in(&input, window, |this, input, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                let q = input.read(cx).value().trim().to_string();
                this.mode = if q.is_empty() { Mode::Popular } else { Mode::Search(q) };
                this.page = 1;
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
                        this.selected = list.first().map(|s| s.id.clone());
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
            covers: HashMap::new(),
            input,
            generation: 0,
            busy: None,
            note: None,
            _sub: sub,
        }
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        let Some(src) = self.selected.clone() else {
            self.results = Load::Ready(vec![]);
            cx.notify();
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        self.results = Load::Loading;
        self.note = None;
        cx.notify();
        let (mode, page) = (self.mode.clone(), self.page);
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
                match res {
                    Ok(list) => this.results = Load::Ready(list),
                    Err(e) => this.results = Load::Failed(format!("{e:#}")),
                }
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
        self.busy = Some(r.title.clone());
        self.note = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let res = mv_ui::bg::run(ops::prepare_from_source(src.clone(), r)).await.and_then(|r| r);
            let _ = this.update_in(cx, |this, w, cx| {
                this.busy = None;
                match res {
                    Ok(m) => mv_ui::navigate(Route::MangaDetail { manga_id: m.id, source_id: Some(src) }, w, cx),
                    Err(e) => this.note = Some(format!("Could not add: {e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn grid(&self, cx: &mut Context<Self>, window: &Window) -> AnyElement {
        let Load::Ready(list) = &self.results else { return div().into_any_element() };
        if list.is_empty() {
            return empty_state("Nothing here", "No results on this page.").into_any_element();
        }
        let width = f32::from(window.viewport_size().width) - 244.0 - size::PAGE_X * 2.0;
        let cols = logic::grid_columns(width);
        uniform_list(
            "browse-grid",
            list.len().div_ceil(cols),
            cx.processor(move |this: &mut Self, range: Range<usize>, _w, cx| {
                let Load::Ready(list) = &this.results else { return vec![] };
                let entity = cx.entity();
                range
                    .map(|r| {
                        div().flex().gap(px(20.)).h(px(ROW_H)).children(list.iter().skip(r * cols).take(cols).map(|it| {
                            let item = it.clone();
                            let entity = entity.clone();
                            cover_card(
                                format!("br-{}", it.source_id),
                                it.title.clone().into(),
                                it.author.clone().or_else(|| it.status.clone()).unwrap_or_default().into(),
                                cover_src(&this.covers, it.cover_url.as_deref()),
                                None,
                            )
                            .on_click(move |_, w, cx| entity.update(cx, |t, cx| t.open_result(item.clone(), w, cx)))
                        }))
                    })
                    .collect()
            }),
        )
        .h_full()
        .into_any_element()
    }
}

impl Render for BrowsePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let subtitle = self.busy.as_ref().map(|t| SharedString::from(format!("Adding {t}...")));
        let mut root = div().size_full().flex().flex_col().px(px(size::PAGE_X)).pt(px(size::PAGE_Y)).child(page_header("Browse", subtitle));
        match &self.sources {
            Load::Loading => return root.child(skeleton_grid(8)),
            Load::Failed(e) => return root.child(error_state(e.clone())),
            Load::Ready(list) if list.is_empty() => {
                return root.child(empty_state("No sources enabled", "Enable or install a source in Settings, Sources."))
            }
            Load::Ready(list) => {
                root = root.child(div().flex().flex_wrap().gap(px(8.)).mb(px(12.)).children(list.iter().map(|s| {
                    let id = s.id.clone();
                    let label = if s.nsfw { format!("{} 18+", s.name) } else { s.name.clone() };
                    chip(SharedString::from(format!("src-{}", s.id)), label, self.selected.as_deref() == Some(s.id.as_str())).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.selected = Some(id.clone());
                            this.mode = Mode::Popular;
                            this.page = 1;
                            this.fetch(cx);
                        },
                    ))
                })));
            }
        }
        let tab = |id: &'static str, label: &'static str, m: Mode, active: bool, cx: &mut Context<Self>| {
            chip(id, label, active).on_click(cx.listener(move |this, _, _, cx| {
                this.mode = m.clone();
                this.page = 1;
                this.fetch(cx);
            }))
        };
        let popular = self.mode == Mode::Popular;
        let latest = self.mode == Mode::Latest;
        let page = self.page;
        let has_results = matches!(&self.results, Load::Ready(l) if !l.is_empty());
        let body: AnyElement = match &self.results {
            Load::Loading => skeleton_grid(12).into_any_element(),
            Load::Failed(e) => error_state(e.clone()).into_any_element(),
            Load::Ready(_) => self.grid(cx, window),
        };
        root.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .mb(px(16.))
                .child(tab("m-popular", "Popular", Mode::Popular, popular, cx))
                .child(tab("m-latest", "Latest", Mode::Latest, latest, cx))
                .when_some(
                    match &self.mode {
                        Mode::Search(q) => Some(format!("Results for \"{q}\"")),
                        _ => None,
                    },
                    |d, t| d.child(muted(t)),
                )
                .when_some(self.note.clone(), |d, n| d.child(div().text_size(px(size::SM)).text_color(Tokens::danger()).child(n)))
                .child(div().flex_1())
                .child(search_box(&self.input)),
        )
        .child(div().flex_1().min_h_0().child(body))
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(12.))
                .py(px(10.))
                .child(button("prev", "Previous", ButtonKind::Ghost).when(page <= 1, |d| d.opacity(0.4)).on_click(cx.listener(|this, _, _, cx| {
                    if this.page > 1 {
                        this.page -= 1;
                        this.fetch(cx);
                    }
                })))
                .child(muted(format!("Page {page}")))
                .child(button("next", "Next", ButtonKind::Ghost).when(!has_results, |d| d.opacity(0.4)).on_click(cx.listener(|this, _, _, cx| {
                    if matches!(&this.results, Load::Ready(l) if !l.is_empty()) {
                        this.page += 1;
                        this.fetch(cx);
                    }
                }))),
        )
    }
}
