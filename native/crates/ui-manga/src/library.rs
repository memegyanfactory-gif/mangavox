use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::manga::{self, Manga};
use mv_ui::{button, chip, cover_card, empty_state, error_state, page_header, size, ButtonKind, Route};

use crate::backend;
use crate::logic::{self, LibraryFilter, SortMode};
use crate::ui::{cover_src, search_box, skeleton_grid, spawn_cover_downloads, Load, ROW_H};

type Missing = Vec<(String, Option<String>)>;

pub struct LibraryPage {
    state: Load<Vec<Manga>>,
    covers: HashMap<String, PathBuf>,
    filter: LibraryFilter,
    sort: SortMode,
    query: String,
    input: Entity<InputState>,
    _sub: Subscription,
}

fn load() -> anyhow::Result<(Vec<Manga>, HashMap<String, PathBuf>, Missing)> {
    let items = backend::with_db(|c| manga::list(c))?;
    let (have, missing) =
        backend::resolve_covers(items.iter().filter_map(|m| m.cover_path.clone().map(|u| (u, Some(m.source_plugin.clone())))));
    Ok((items, have, missing))
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
            query: String::new(),
            input,
            _sub: sub,
        };
        page.reload(cx);
        page
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(load).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| match res {
                Ok((items, have, missing)) => {
                    this.covers.extend(have);
                    this.state = Load::Ready(items);
                    spawn_cover_downloads(cx, missing, |t: &mut Self| &mut t.covers);
                    cx.notify();
                }
                Err(e) => {
                    this.state = Load::Failed(format!("{e:#}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn grid(&self, cx: &mut Context<Self>, window: &Window) -> AnyElement {
        let Load::Ready(items) = &self.state else { return div().into_any_element() };
        if items.is_empty() {
            return div()
                .flex()
                .flex_col()
                .items_center()
                .child(empty_state("Your library is empty", "Open Browse, pick a source and add something to read."))
                .child(button("go-browse", "Browse sources", ButtonKind::Primary).on_click(|_, w, cx| mv_ui::navigate(Route::Browse, w, cx)))
                .into_any_element();
        }
        let visible = logic::filter_library(items, self.filter, &self.query, self.sort);
        if visible.is_empty() {
            return empty_state("No matches", "Try another filter or search term.").into_any_element();
        }
        // Assumes the expanded 244 px sidebar, so the grid never overflows.
        let width = f32::from(window.viewport_size().width) - 244.0 - size::PAGE_X * 2.0;
        let cols = logic::grid_columns(width);
        let rows = visible.len().div_ceil(cols);
        uniform_list(
            "library-grid",
            rows,
            cx.processor(move |this: &mut Self, range: Range<usize>, _w, _cx| {
                let Load::Ready(items) = &this.state else { return vec![] };
                range
                    .map(|r| {
                        div().flex().gap(px(20.)).h(px(ROW_H)).children(visible.iter().skip(r * cols).take(cols).map(|&i| {
                            let m = &items[i];
                            let unread = m.unread_count.filter(|n| *n > 0).map(|n| SharedString::from(n.to_string()));
                            let sub = m.author.clone().or_else(|| m.status.clone()).unwrap_or_default();
                            let route = Route::MangaDetail { manga_id: m.id.clone(), source_id: Some(m.source_plugin.clone()) };
                            cover_card(format!("lib-{}", m.id), m.title.clone().into(), sub.into(), cover_src(&this.covers, m.cover_path.as_deref()), unread)
                                .on_click(move |_, w, cx| mv_ui::navigate(route.clone(), w, cx))
                        }))
                    })
                    .collect()
            }),
        )
        .h_full()
        .into_any_element()
    }
}

impl Render for LibraryPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = match &self.state {
            Load::Ready(items) => Some(SharedString::from(format!("{} titles", items.len()))),
            _ => None,
        };
        let body: AnyElement = match &self.state {
            Load::Loading => skeleton_grid(12).into_any_element(),
            Load::Failed(e) => error_state(e.clone()).into_any_element(),
            Load::Ready(_) => self.grid(cx, window),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .px(px(size::PAGE_X))
            .pt(px(size::PAGE_Y))
            .child(page_header("Library", count))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .mb(px(16.))
                    .children(LibraryFilter::ALL.map(|f| {
                        chip(f.label(), f.label(), self.filter == f).on_click(cx.listener(move |this, _, _, cx| {
                            this.filter = f;
                            cx.notify();
                        }))
                    }))
                    .child(div().flex_1())
                    .child(search_box(&self.input))
                    .child(button("sort", format!("Sort: {}", self.sort.label()), ButtonKind::Ghost).on_click(cx.listener(|this, _, _, cx| {
                        this.sort = this.sort.toggled();
                        cx.notify();
                    })))
                    .when(matches!(self.state, Load::Failed(_)), |d| {
                        d.child(button("retry", "Retry", ButtonKind::Secondary).on_click(cx.listener(|this, _, _, cx| {
                            this.state = Load::Loading;
                            this.reload(cx);
                            cx.notify();
                        })))
                    }),
            )
            .child(div().flex_1().min_h_0().child(body))
    }
}
