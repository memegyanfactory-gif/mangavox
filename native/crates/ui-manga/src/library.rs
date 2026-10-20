use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::category::{self, LibraryCategory};
use mv_core::repo::manga::{self, Manga};
use mv_ui::theme::icon as icon_px;
use mv_ui::toast::{self, Toast};
use mv_ui::{
    btn, empty_view, error_view, filter_chip, focus_ring_shadow, glyphs, icon_button, menu_button, overflow_menu, page_title, ph, ph_icon, progress_bar, size, tip, ButtonKind, Interactive,
    MenuItem, Ph, Route, Tokens, Weight,
};

use crate::backend;
use crate::categories;
use crate::logic::{self, LibStats, LibraryFilter, SortMode};
use crate::ops;
use crate::store;
use crate::ui::{cover_src, manga_card, search_box, skeleton_rows, spawn_cover_downloads, tab, width_probe, CardInfo, Grid, Load, CARD_GROUP};

type Missing = Vec<(String, Option<String>)>;

/// Config keys for the chosen sort and its direction.
const SORT_KEY: &str = "librarySort";
const REVERSE_KEY: &str = "librarySortReverse";
/// Titles refreshed at the same time during a library update.
const UPDATE_PARALLEL: usize = 3;

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
    /// Sort direction flipped from the mode's natural order.
    reverse: bool,
    /// Selected category tab (`None` = all titles).
    category: Option<String>,
    query: String,
    input: Entity<InputState>,
    scroll: UniformListScrollHandle,
    width: f32,
    seen_version: u64,
    /// Library update in progress: (titles done, titles total).
    updating: Option<(usize, usize)>,
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

/// Leading icon of a filter chip (the check replaces it when selected).
fn filter_icon(f: LibraryFilter) -> Option<Ph> {
    match f {
        LibraryFilter::All => None,
        LibraryFilter::Unread => Some(ph::CIRCLE_DASHED),
        LibraryFilter::Downloaded => Some(glyphs::DOWNLOAD),
        LibraryFilter::Favorites => Some(glyphs::FAVORITES),
        LibraryFilter::Ongoing => Some(ph::CLOCK),
        LibraryFilter::Completed => Some(ph::CHECK_CIRCLE),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
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
            sort: SortMode::from_config(&backend::config_str(SORT_KEY).unwrap_or_default()),
            reverse: backend::config_str(REVERSE_KEY).as_deref() == Some("1"),
            category: None,
            query: String::new(),
            input,
            scroll: UniformListScrollHandle::new(),
            width: 0.,
            seen_version: backend::data_version(),
            updating: None,
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
            let _ = this.update_in(cx, |_, w, cx| match res {
                Ok(Some(chapter_id)) => mv_ui::navigate(Route::Reader { manga_id, chapter_id }, w, cx),
                Ok(None) => mv_ui::navigate(Route::MangaDetail { manga_id, source_id: None }, w, cx),
                Err(e) => toast::show(cx, Toast::error(format!("Could not open the chapter: {e:#}"))),
            });
        })
        .detach();
    }

    fn set_sort(&mut self, sort: SortMode, cx: &mut Context<Self>) {
        self.sort = sort;
        backend::set_config_str(SORT_KEY, sort.config());
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn set_reverse(&mut self, reverse: bool, cx: &mut Context<Self>) {
        self.reverse = reverse;
        backend::set_config_str(REVERSE_KEY, if reverse { "1" } else { "0" });
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn clear_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter = LibraryFilter::All;
        self.category = None;
        self.query.clear();
        self.input.update(cx, |s, cx| s.set_value("", window, cx));
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// Check titles for new chapters: the whole library, the open category, or `only` (a retry).
    /// Mihon library update: <https://mihon.app/docs/faq/library>
    fn update_library(&mut self, only: Option<Vec<String>>, cx: &mut Context<Self>) {
        if self.updating.is_some() {
            return;
        }
        let Load::Ready(d) = &self.state else { return };
        let scope: Option<HashSet<String>> = only.map(|v| v.into_iter().collect()).or_else(|| self.category.as_ref().map(|c| d.members.get(c).cloned().unwrap_or_default()));
        let targets: Vec<(String, usize)> =
            d.items.iter().filter(|m| scope.as_ref().is_none_or(|s| s.contains(&m.id))).map(|m| (m.id.clone(), d.stats.get(&m.id).map_or(0, |s| s.total))).collect();
        if targets.is_empty() {
            return;
        }
        let total = targets.len();
        self.updating = Some((0, total));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (mut done, mut added, mut titles_new) = (0usize, 0usize, 0usize);
            let mut failed: Vec<String> = vec![];
            for chunk in targets.chunks(UPDATE_PARALLEL) {
                let results = futures::future::join_all(chunk.iter().map(|(id, _)| mv_ui::bg::run(ops::refresh_manga(id.clone())))).await;
                for ((id, before), r) in chunk.iter().zip(results) {
                    match r.and_then(|r| r) {
                        Ok((_, cs)) => {
                            let n = cs.len().saturating_sub(*before);
                            if n > 0 {
                                added += n;
                                titles_new += 1;
                            }
                        }
                        Err(_) => failed.push(id.clone()),
                    }
                }
                done += chunk.len();
                let _ = this.update(cx, |t, cx| {
                    t.updating = Some((done, total));
                    cx.notify();
                });
            }
            let _ = this.update(cx, |t, cx| {
                t.updating = None;
                cx.notify();
            });
            let toast = if !failed.is_empty() {
                let weak = this.clone();
                let msg = if added > 0 {
                    format!("{} in {}. {} could not be checked.", plural(added, "new chapter", "new chapters"), plural(titles_new, "title", "titles"), plural(failed.len(), "title", "titles"))
                } else {
                    format!("{} of {total} could not be checked. Their sources did not answer.", plural(failed.len(), "title", "titles"))
                };
                Toast::warning(msg).action("Retry", move |_, cx| {
                    let ids = failed.clone();
                    let _ = weak.update(cx, |t, cx| t.update_library(Some(ids), cx));
                })
            } else if added > 0 {
                Toast::success(format!("{} in {}", plural(added, "new chapter", "new chapters"), plural(titles_new, "title", "titles")))
            } else {
                Toast::success("Your library is up to date").icon(ph::CHECK_CIRCLE)
            };
            cx.update(|cx| toast::show(cx, toast));
        })
        .detach();
    }

    fn visible(&self, d: &Data) -> Vec<usize> {
        let only = self.category.as_ref().map(|c| d.members.get(c).cloned().unwrap_or_default());
        let mut v = logic::filter_library(&d.items, &d.stats, self.filter, only.as_ref(), &self.query, self.sort);
        if self.reverse {
            v.reverse();
        }
        v
    }

    fn grid(&self, d: &Data, grid: Grid, cx: &mut Context<Self>) -> AnyElement {
        if d.items.is_empty() {
            return empty_view(glyphs::LIBRARY, "Your library is empty", "Find something in Browse, open it and choose Add to library.")
                .action(btn("go-browse", "Browse sources", ButtonKind::Primary).icon(glyphs::BROWSE).on_click(|_, w, cx| mv_ui::navigate(Route::Browse, w, cx)))
                .into_any_element();
        }
        let visible = self.visible(d);
        if visible.is_empty() {
            let body = if self.query.trim().is_empty() { "Nothing in this view matches the filter." } else { "No titles match your search." };
            return empty_view(ph::FUNNEL_X, "No matches", body)
                .action(btn("clear-filters", "Clear filters", ButtonKind::Secondary).icon(ph::FUNNEL_X).on_click(cx.listener(|this, _, w, cx| this.clear_filters(w, cx))))
                .into_any_element();
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
                            let read = st.total.saturating_sub(st.unread);
                            let route = Route::MangaDetail { manga_id: m.id.clone(), source_id: Some(m.source_plugin.clone()) };
                            let cid = m.id.clone();
                            // Continue reading: hidden until the card is hovered, shown when it has keyboard focus.
                            // W3C visible controls: https://www.w3.org/WAI/WCAG22/Understanding/focus-visible.html
                            let resume = div()
                                .id(SharedString::from(format!("cont-{cid}")))
                                .absolute()
                                .right(px(6.))
                                .bottom(px(8.))
                                .size(px(30.))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(Tokens::accent())
                                .shadow_md()
                                .opacity(0.)
                                .group_hover(CARD_GROUP, |s| s.opacity(1.))
                                .role(Role::Button)
                                .aria_label("Continue reading")
                                .hover(|s| s.bg(Tokens::accent_hover()))
                                .pressable()
                                .active(|s| s.bg(Tokens::accent_pressed()))
                                .focus_visible(|s| s.opacity(1.).shadow(focus_ring_shadow()))
                                .tooltip(tip("Continue reading"))
                                .child(ph_icon(ph::PLAY, Weight::Fill, Tokens::on_accent(), icon_px::SM))
                                .on_click(cx.listener(move |this, _, w, cx| {
                                    cx.stop_propagation();
                                    this.continue_reading(cid.clone(), w, cx);
                                }));
                            let info = CardInfo {
                                title: m.title.clone().into(),
                                cover: cover_src(&this.covers, m.cover_path.as_deref()),
                                unread: st.unread,
                                downloaded: st.downloaded,
                                in_library: false,
                                favorite: m.is_favorite,
                                progress: (st.total > 0 && read > 0).then(|| read as f32 / st.total as f32),
                                busy: false,
                                action: Some(resume.into_any_element()),
                            };
                            manga_card(SharedString::from(format!("lib-{}", m.id)), info, grid).on_click(move |_, w, cx| mv_ui::navigate(route.clone(), w, cx))
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
            tab("cat-all", None, "All", Some(d.items.len()), self.category.is_none()).on_click(cx.listener(|this, _, _, cx| {
                this.category = None;
                this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
            })),
        );
        for c in &d.categories {
            let id = c.id.clone();
            let n = d.members.get(&c.id).map(|m| m.len()).unwrap_or(0);
            row = row.child(tab(SharedString::from(format!("cat-{}", c.id)), None, c.name.clone(), Some(n), self.category.as_deref() == Some(c.id.as_str())).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.category = Some(id.clone());
                    this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                    cx.notify();
                }),
            ));
        }
        Some(row.child(div().flex_1()).child(icon_button("cat-edit", ph::PENCIL_SIMPLE, "Edit categories").small().on_click(|_, w, cx| categories::open_manage(w, cx))))
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (subtitle, has_items) = match &self.state {
            Load::Ready(d) if !d.items.is_empty() => {
                let unread: usize = d.items.iter().filter_map(|m| d.stats.get(&m.id)).map(|s| s.unread).sum();
                let s = if unread > 0 { format!("{}, {}", plural(d.items.len(), "title", "titles"), plural(unread, "unread chapter", "unread chapters")) } else { plural(d.items.len(), "title", "titles") };
                (Some(s), true)
            }
            _ => (None, false),
        };
        let weak = cx.entity().downgrade();
        let mut sort_items = vec![MenuItem::heading("Sort by")];
        sort_items.extend(SortMode::ALL.map(|m| {
            let w = weak.clone();
            MenuItem::new(m.label(), move |_, cx| {
                let _ = w.update(cx, |t, cx| t.set_sort(m, cx));
            })
            .checked(self.sort == m)
        }));
        let (w, reverse) = (weak.clone(), self.reverse);
        sort_items.push(MenuItem::separator());
        sort_items.push(
            MenuItem::new("Reverse order", move |_, cx| {
                let _ = w.update(cx, |t, cx| t.set_reverse(!reverse, cx));
            })
            .icon(ph::ARROWS_DOWN_UP)
            .checked(self.reverse),
        );
        let in_category = self.category.is_some();
        let update_label = match self.updating {
            Some((done, total)) => format!("Updating {done} of {total}"),
            None => "Update".into(),
        };
        let w2 = weak.clone();
        div().px(px(size::PAGE_X)).child(
            page_title("Library")
                .icon(glyphs::LIBRARY)
                .when_some(subtitle, |h, s| h.subtitle(s))
                .when(has_items, |h| {
                    h.action(search_box(&self.input))
                        .action(menu_button("lib-sort", if self.reverse { ph::SORT_DESCENDING } else { ph::SORT_ASCENDING }, "Sort", sort_items).label(self.sort.label()))
                        .action(
                            btn("lib-update", update_label, ButtonKind::Ghost)
                                .icon(glyphs::REFRESH)
                                .loading(self.updating.is_some())
                                .tooltip(if in_category { "Check this category for new chapters" } else { "Check every title for new chapters" })
                                .on_click(cx.listener(|this, _, _, cx| this.update_library(None, cx))),
                        )
                        .action(overflow_menu(
                            "lib-more",
                            vec![
                                MenuItem::new("Manage categories", categories::open_manage).icon(ph::FOLDER_SIMPLE),
                                MenuItem::new("Update whole library", move |_, cx| {
                                    // Every title, without leaving the category tab the user is on.
                                    let _ = w2.update(cx, |t, cx| {
                                        let all = match &t.state {
                                            Load::Ready(d) => d.items.iter().map(|m| m.id.clone()).collect(),
                                            _ => vec![],
                                        };
                                        t.update_library(Some(all), cx)
                                    });
                                })
                                .icon(glyphs::REFRESH),
                            ],
                        ))
                }),
        )
    }

    fn filter_row(&self, d: &Data, cx: &mut Context<Self>) -> Div {
        let count = |f: LibraryFilter| -> Option<usize> {
            let st = |m: &Manga| d.stats.get(&m.id).copied().unwrap_or_default();
            match f {
                LibraryFilter::Unread => Some(d.items.iter().filter(|m| st(m).unread > 0).count()),
                LibraryFilter::Downloaded => Some(d.items.iter().filter(|m| st(m).downloaded > 0).count()),
                LibraryFilter::Favorites => Some(d.items.iter().filter(|m| m.is_favorite).count()),
                _ => None,
            }
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .px(px(size::PAGE_X))
            .mb(px(16.))
            .children(LibraryFilter::ALL.map(|f| {
                let on = self.filter == f;
                filter_chip(f.label(), f.label(), on)
                    .map(|c| match filter_icon(f) {
                        Some(i) => c.icon(i),
                        None => c,
                    })
                    .map(|c| match count(f) {
                        Some(n) => c.count(n),
                        None => c,
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // A selected chip clicked again goes back to All (filter chips toggle).
                        this.filter = if on && f != LibraryFilter::All { LibraryFilter::All } else { f };
                        this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                        cx.notify();
                    }))
            }))
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
        let mut root = div().size_full().flex().flex_col().pt(px(size::PAGE_Y)).child(self.header(cx));
        if let Some((done, total)) = self.updating {
            root = root.child(div().px(px(size::PAGE_X)).mt(px(-8.)).mb(px(14.)).child(progress_bar("lib-update-bar", done as f32 / total.max(1) as f32, true)));
        }
        let body: AnyElement = match &self.state {
            Load::Loading => skeleton_rows(grid, 3).into_any_element(),
            Load::Failed(e) => error_view(e.clone())
                .title("Could not open your library")
                .retry(cx.listener(|this, _, _, cx| {
                    this.state = Load::Loading;
                    this.reload(cx);
                    cx.notify();
                }))
                .into_any_element(),
            Load::Ready(d) => {
                if let Some(tabs) = self.category_tabs(d, cx) {
                    root = root.child(tabs);
                }
                if !d.items.is_empty() {
                    root = root.child(self.filter_row(d, cx));
                }
                self.grid(d, grid, cx)
            }
        };
        root.child(div().relative().flex_1().min_h_0().child(width_probe(cx, |t: &mut Self| &mut t.width)).child(body))
    }
}
