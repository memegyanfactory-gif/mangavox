use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::category::{self, LibraryCategory};
use mv_core::repo::manga::{self, Manga};
use mv_ui::{button, chip, icon, page_header, size, ButtonKind, Route, Tokens};

use crate::backend;
use crate::logic::{self, LibStats, LibraryFilter, SortMode};
use crate::store;
use crate::ui::{cover_src, error_view, manga_card, search_box, skeleton_rows, spawn_cover_downloads, state_view, tab, tool_button, width_probe, CardInfo, Grid, Load};

type Missing = Vec<(String, Option<String>)>;

struct Data {
    items: Vec<Manga>,
    stats: HashMap<String, LibStats>,
    categories: Vec<LibraryCategory>,
    /// category id -> manga ids
    members: HashMap<String, HashSet<String>>,
}

pub struct LibraryPage {
    state: Load<Data>,
    covers: HashMap<String, PathBuf>,
    filter: LibraryFilter,
    sort: SortMode,
    /// Selected category tab (`None` = all titles).
    category: Option<String>,
    query: String,
    input: Entity<InputState>,
    scroll: UniformListScrollHandle,
    width: f32,
    hovered: Option<String>,
    seen_version: u64,
    note: Option<String>,
    _sub: Subscription,
}

fn load() -> anyhow::Result<(Data, HashMap<String, PathBuf>, Missing)> {
    let lang = backend::preferred_language();
    let hidden = backend::browse_only();
    let (items, stats, categories, map) = backend::with_db(|c| {
        let items: Vec<Manga> = manga::list(c)?.into_iter().filter(|m| !hidden.contains(&m.id)).collect();
        Ok((items, store::library_stats(c, &lang)?, category::list(c)?, category::manga_category_map(c)?))
    })?;
    let mut members: HashMap<String, HashSet<String>> = HashMap::new();
    for (mid, cats) in map {
        for cat in cats {
            members.entry(cat).or_default().insert(mid.clone());
        }
    }
    let (have, missing) =
        backend::resolve_covers(items.iter().filter_map(|m| m.cover_path.clone().map(|u| (u, Some(m.source_plugin.clone())))));
    Ok((Data { items, stats, categories, members }, have, missing))
}

impl LibraryPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search library"));
        let sub = cx.subscribe_in(&input, window, |this, input, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::Change) {
                this.query = input.read(cx).value().to_string();
                cx.notify();
            }
        });
        let mut page = Self {
            state: Load::Loading,
            covers: HashMap::new(),
            filter: LibraryFilter::All,
            sort: SortMode::Recent,
            category: None,
            query: String::new(),
            input,
            scroll: UniformListScrollHandle::new(),
            width: 0.,
            hovered: None,
            seen_version: backend::data_version(),
            note: None,
            _sub: sub,
        };
        page.reload(cx);
        page
    }

    /// Re-read the library. The current grid stays on screen until the new data arrives, so a
    /// refresh after coming back from the reader keeps the scroll position.
    fn reload(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(load).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok((data, have, missing)) => {
                        this.covers.extend(have);
                        if this.category.as_ref().is_some_and(|c| !data.categories.iter().any(|x| &x.id == c)) {
                            this.category = None;
                        }
                        this.state = Load::Ready(data);
                        spawn_cover_downloads(cx, missing, |t: &mut Self| &mut t.covers);
                    }
                    Err(e) => this.state = Load::Failed(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Open the chapter "Continue reading" points at (or the detail page when there is none).
    fn continue_reading(&mut self, manga_id: String, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let mid = manga_id.clone();
            let lang = backend::preferred_language();
            let res = mv_ui::bg::blocking(move || backend::with_db(|c| store::continue_chapter(c, &mid, &lang))).await.and_then(|r| r);
            let _ = this.update_in(cx, |this, w, cx| match res {
                Ok(Some(chapter_id)) => mv_ui::navigate(Route::Reader { manga_id, chapter_id }, w, cx),
                Ok(None) => mv_ui::navigate(Route::MangaDetail { manga_id, source_id: None }, w, cx),
                Err(e) => {
                    this.note = Some(format!("Could not open: {e:#}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn visible(&self, d: &Data) -> Vec<usize> {
        let only = self.category.as_ref().map(|c| d.members.get(c).cloned().unwrap_or_default());
        logic::filter_library(&d.items, &d.stats, self.filter, only.as_ref(), &self.query, self.sort)
    }

    fn grid(&self, d: &Data, grid: Grid, cx: &mut Context<Self>) -> AnyElement {
        if d.items.is_empty() {
            return div()
                .flex()
                .flex_col()
                .items_center()
                .child(state_view("icons/library.svg", "Your library is empty", "Find something in Browse, open it and choose Add to library."))
                .child(
                    button("go-browse", "Browse sources", ButtonKind::Primary)
                        .child(icon("icons/compass.svg", Tokens::base(), 14.))
                        .on_click(|_, w, cx| mv_ui::navigate(Route::Browse, w, cx)),
                )
                .into_any_element();
        }
        let visible = self.visible(d);
        if visible.is_empty() {
            let body = if self.query.trim().is_empty() { "Nothing matches this filter." } else { "No titles match your search." };
            return state_view("icons/search.svg", "No matches", body).into_any_element();
        }
        let rows = grid.rows(visible.len());
        uniform_list(
            "library-grid",
            rows,
            cx.processor(move |this: &mut Self, range: Range<usize>, _w, cx| {
                let Load::Ready(d) = &this.state else { return vec![] };
                range
                    .map(|r| {
                        grid.row().children(visible.iter().skip(r * grid.cols).take(grid.cols).map(|&i| {
                            let m = &d.items[i];
                            let st = d.stats.get(&m.id).copied().unwrap_or_default();
                            let info = CardInfo {
                                title: m.title.clone().into(),
                                cover: cover_src(&this.covers, m.cover_path.as_deref()),
                                unread: st.unread,
                                downloaded: st.downloaded,
                                in_library: false,
                            };
                            let route = Route::MangaDetail { manga_id: m.id.clone(), source_id: Some(m.source_plugin.clone()) };
                            let (hid, cid) = (m.id.clone(), m.id.clone());
                            let hovered = this.hovered.as_deref() == Some(m.id.as_str());
                            manga_card(SharedString::from(format!("lib-{}", m.id)), info, grid)
                                .on_hover(cx.listener(move |this, h: &bool, _, cx| {
                                    if *h {
                                        this.hovered = Some(hid.clone());
                                    } else if this.hovered.as_deref() == Some(hid.as_str()) {
                                        this.hovered = None;
                                    }
                                    cx.notify();
                                }))
                                .on_click(move |_, w, cx| mv_ui::navigate(route.clone(), w, cx))
                                .when(hovered, |d| {
                                    d.child(
                                        div()
                                            .id(SharedString::from(format!("cont-{}", cid)))
                                            .absolute()
                                            .top(px(6.))
                                            .right(px(6.))
                                            .size(px(32.))
                                            .rounded(px(999.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .bg(Tokens::accent())
                                            .shadow_md()
                                            .cursor_pointer()
                                            .hover(|s| s.opacity(0.9))
                                            .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Continue reading").build(w, cx))
                                            .child(icon("icons/play.svg", Tokens::base(), 15.))
                                            .on_click(cx.listener(move |this, _, w, cx| {
                                                cx.stop_propagation();
                                                this.continue_reading(cid.clone(), w, cx);
                                            })),
                                    )
                                })
                        }))
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.scroll)
        .size_full()
        .into_any_element()
    }

    fn category_tabs(&self, d: &Data, cx: &mut Context<Self>) -> Option<Div> {
        if d.categories.is_empty() {
            return None;
        }
        let mut row = div().flex().items_center().px(px(size::PAGE_X)).border_b_1().border_color(Tokens::border()).mb(px(12.)).child(
            tab("cat-all", "All", Some(d.items.len()), self.category.is_none()).on_click(cx.listener(|this, _, _, cx| {
                this.category = None;
                this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
            })),
        );
        for c in &d.categories {
            let id = c.id.clone();
            let n = d.members.get(&c.id).map(|m| m.len()).unwrap_or(0);
            row = row.child(tab(SharedString::from(format!("cat-{}", c.id)), c.name.clone(), Some(n), self.category.as_deref() == Some(c.id.as_str())).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.category = Some(id.clone());
                    this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                    cx.notify();
                }),
            ));
        }
        Some(row)
    }
}

impl Render for LibraryPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Progress changed elsewhere (reader, detail page): refresh counts in place.
        let v = backend::data_version();
        if v != self.seen_version {
            self.seen_version = v;
            self.reload(cx);
        }
        let grid = Grid::new(self.width, window);
        let subtitle = match &self.state {
            Load::Ready(d) if !d.items.is_empty() => {
                let unread: usize = d.items.iter().filter_map(|m| d.stats.get(&m.id)).map(|s| s.unread).sum();
                Some(SharedString::from(if unread > 0 { format!("{} titles, {unread} unread chapters", d.items.len()) } else { format!("{} titles", d.items.len()) }))
            }
            _ => None,
        };
        let has_items = matches!(&self.state, Load::Ready(d) if !d.items.is_empty());
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(size::PAGE_X))
            .child(div().flex_1().min_w_0().child(page_header("Library", subtitle)))
            .when(has_items, |d| {
                d.child(div().mb(px(20.)).child(search_box(&self.input))).child(
                    div().mb(px(20.)).child(
                        tool_button("sort", "icons/arrow-down-wide-narrow.svg", self.sort.label(), false)
                            .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Change sort order").build(w, cx))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.sort = this.sort.toggled();
                                this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                                cx.notify();
                            })),
                    ),
                )
            });
        let mut root = div().size_full().flex().flex_col().pt(px(size::PAGE_Y)).child(header);
        let body: AnyElement = match &self.state {
            Load::Loading => skeleton_rows(grid, 3).into_any_element(),
            Load::Failed(e) => div()
                .flex()
                .flex_col()
                .items_center()
                .child(error_view(e.clone()))
                .child(button("retry", "Try again", ButtonKind::Secondary).on_click(cx.listener(|this, _, _, cx| {
                    this.state = Load::Loading;
                    this.reload(cx);
                    cx.notify();
                })))
                .into_any_element(),
            Load::Ready(d) => {
                if let Some(tabs) = self.category_tabs(d, cx) {
                    root = root.child(tabs);
                }
                if !d.items.is_empty() {
                    root = root.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.))
                            .px(px(size::PAGE_X))
                            .mb(px(16.))
                            .children(LibraryFilter::ALL.map(|f| {
                                chip(f.label(), f.label(), self.filter == f).on_click(cx.listener(move |this, _, _, cx| {
                                    this.filter = f;
                                    this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                                    cx.notify();
                                }))
                            }))
                            .when_some(self.note.clone(), |d, n| d.child(crate::ui::note_line(n, true))),
                    );
                }
                self.grid(d, grid, cx)
            }
        };
        root.child(div().relative().flex_1().min_h_0().child(width_probe(cx, |t: &mut Self| &mut t.width)).child(body))
    }
}
