use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;

use anyhow::anyhow;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::manga;
use mv_sources::{MangaSearchResult, SourceInfo, Tag};
use mv_ui::{button, page_header, size, ButtonKind, Route, Tokens};

use crate::backend;
use crate::discover::{self, SourceTags, TagRequest, TagTarget};
use crate::ops;
use crate::tags;
use crate::ui::{
    cover_src, error_view, manga_card, note_line, removable_chip, search_box, select_chip, skeleton_cover, skeleton_rows, spawn_cover_downloads, spinner,
    state_view, tab, tag, tool_button, width_probe, CardInfo, Grid, Load,
};

/// Last source picked in Browse (restored next time).
const SOURCE_KEY: &str = "browseSource";

#[derive(Clone, PartialEq, Debug)]
enum Mode {
    Popular,
    Latest,
    Search(String),
    /// Titles carrying all of these tags.
    Tags(Vec<Tag>),
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
    /// Tag list of `tags_for` (the selected source once the panel has been opened).
    tags: Load<SourceTags>,
    tags_for: Option<String>,
    tags_open: bool,
    /// A genre clicked on a manga page is being matched to a tag.
    resolving: Option<String>,
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
            tags: Load::Loading,
            tags_for: None,
            tags_open: false,
            resolving: None,
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

    fn selected_tags(&self) -> &[Tag] {
        match &self.mode {
            Mode::Tags(v) => v,
            _ => &[],
        }
    }

    /// Load (cached) tags for the selected source unless they are already there.
    fn load_tags(&mut self, cx: &mut Context<Self>) {
        let Some(src) = self.selected.clone() else { return };
        if self.tags_for.as_deref() == Some(src.as_str()) {
            return;
        }
        self.tags_for = Some(src.clone());
        self.tags = Load::Loading;
        cx.spawn(async move |this, cx| {
            let sid = src.clone();
            let res = mv_ui::bg::run(async move { discover::tag_list(&sid).await }).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if this.tags_for.as_deref() != Some(src.as_str()) {
                    return;
                }
                this.tags = match res {
                    Ok(t) => Load::Ready(t),
                    Err(e) => Load::Failed(format!("{e:#}")),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Select / deselect a tag. Sources that cannot combine tags switch to the clicked one.
    fn toggle_tag(&mut self, tag: Tag, cx: &mut Context<Self>) {
        let multi = matches!(&self.tags, Load::Ready(st) if st.multi);
        let mut sel = self.selected_tags().to_vec();
        if let Some(i) = sel.iter().position(|t| t.id == tag.id) {
            sel.remove(i);
        } else if multi {
            sel.push(tag);
        } else {
            sel = vec![tag];
        }
        self.set_mode(if sel.is_empty() { Mode::Popular } else { Mode::Tags(sel) }, cx);
    }

    fn remove_tag(&mut self, id: &str, cx: &mut Context<Self>) {
        let sel: Vec<Tag> = self.selected_tags().iter().filter(|t| t.id != id).cloned().collect();
        self.set_mode(if sel.is_empty() { Mode::Popular } else { Mode::Tags(sel) }, cx);
    }

    /// A genre chip on a manga page: find the matching tag (own source, else MangaDex) and show it.
    fn open_tag_request(&mut self, req: TagRequest, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.resolving = Some(req.name.clone());
        self.results = Load::Loading;
        self.note = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let target = mv_ui::bg::run(discover::resolve_tag(req)).await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.resolving = None;
                let (src, mode) = match target {
                    Ok(TagTarget::Tag { source_id, tag }) => {
                        this.tags_open = true;
                        (Some(source_id), Mode::Tags(vec![tag]))
                    }
                    Ok(TagTarget::Search { source_id, query }) => (Some(source_id), Mode::Search(query)),
                    Err(e) => {
                        this.note = Some(format!("Could not open tag: {e:#}"));
                        (None, Mode::Popular)
                    }
                };
                if let Some(id) = src {
                    backend::set_config_str(SOURCE_KEY, &id);
                    this.selected = Some(id);
                }
                this.set_mode(mode, cx);
            });
        })
        .detach();
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
                    Mode::Tags(t) => {
                        let ids: Vec<String> = t.iter().map(|t| t.id.clone()).collect();
                        s.get_manga_by_tags(&ids, page).await
                    }
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
                Mode::Tags(t) => {
                    let names: Vec<&str> = t.iter().map(|t| t.name.as_str()).collect();
                    ("No titles", format!("Nothing on this source is tagged {}.", names.join(" + ")))
                }
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

    /// Selected tags as removable chips, with Clear all.
    fn selected_row(&self, cx: &mut Context<Self>) -> Option<Div> {
        let sel = self.selected_tags();
        if sel.is_empty() {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.))
                .px(px(size::PAGE_X))
                .mb(px(12.))
                .child(div().mr(px(2.)).text_size(px(size::SM)).text_color(Tokens::text_3()).child(if sel.len() > 1 { "Tagged with all of" } else { "Tagged" }))
                .children(sel.iter().map(|t| {
                    let id = t.id.clone();
                    removable_chip(SharedString::from(format!("sel-{}", t.id)), t.name.clone()).on_click(cx.listener(move |this, _, _, cx| this.remove_tag(&id, cx)))
                }))
                .child(
                    div()
                        .id("tags-clear")
                        .ml(px(4.))
                        .text_size(px(size::SM))
                        .text_color(Tokens::accent())
                        .cursor_pointer()
                        .hover(|s| s.text_color(Tokens::text_1()))
                        .child("Clear all")
                        .on_click(cx.listener(|this, _, _, cx| this.set_mode(Mode::Popular, cx))),
                ),
        )
    }

    /// The Tags area: the source's tags in groups (Genres, Themes, ...), multi-select when the
    /// source can combine tags.
    fn tag_panel(&self, current: Option<&SourceInfo>, cx: &mut Context<Self>) -> Div {
        let wrap = div().flex().flex_col().gap(px(8.)).mx(px(size::PAGE_X)).mb(px(12.)).p(px(12.)).rounded(px(10.)).bg(Tokens::surface()).border_1().border_color(Tokens::border());
        let source_name = current.map(|s| s.name.clone()).unwrap_or_else(|| "This source".into());
        match &self.tags {
            Load::Loading => wrap.child(div().flex().items_center().gap(px(8.)).child(spinner("tags-spin", 14.)).child(note_line("Loading tags...", false))),
            Load::Failed(e) => wrap.child(note_line(format!("Could not load tags: {e}"), true)).child(
                div().flex().child(button("tags-retry", "Try again", ButtonKind::Secondary).on_click(cx.listener(|this, _, _, cx| {
                    this.tags_for = None;
                    this.load_tags(cx);
                    cx.notify();
                }))),
            ),
            Load::Ready(st) if st.tags.is_empty() => {
                let fallback = discover::FALLBACK_SOURCE;
                let can_switch = self.selected.as_deref() != Some(fallback)
                    && matches!(&self.sources, Load::Ready(list) if list.iter().any(|s| s.id == fallback));
                wrap.child(note_line(format!("{source_name} does not offer tag browsing."), false)).when(can_switch, |d| {
                    d.child(div().flex().child(button("tags-md", "Browse MangaDex tags", ButtonKind::Secondary).on_click(cx.listener(move |this, _, _, cx| {
                        this.select_source(fallback.to_string(), cx);
                    }))))
                })
            }
            Load::Ready(st) => {
                let selected: Vec<String> = self.selected_tags().iter().map(|t| t.id.clone()).collect();
                let hint = if st.multi { "Pick tags to narrow the results: titles must have all of them." } else { "Pick a tag to browse it (this source filters by one tag at a time)." };
                let groups = tags::group_tags(&st.tags);
                let show_labels = groups.len() > 1;
                wrap.child(note_line(hint, false)).child(
                    div().id("tag-panel").max_h(px(216.)).overflow_y_scroll().flex().flex_col().gap(px(10.)).children(groups.into_iter().map(|(group, list)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .when(show_labels, |d| {
                                d.child(div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_3()).child(group.to_uppercase()))
                            })
                            .child(div().flex().flex_wrap().gap(px(6.)).children(list.into_iter().map(|t| {
                                let active = selected.contains(&t.id);
                                let label = t.name.clone();
                                select_chip(SharedString::from(format!("tg-{}", t.id)), label, active)
                                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_tag(t.clone(), cx)))
                            })))
                    })),
                )
            }
        }
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
        if matches!(self.sources, Load::Ready(_)) {
            if let Some(req) = discover::take_request() {
                let page = cx.entity();
                cx.defer(move |cx| page.update(cx, |this, cx| this.open_tag_request(req, cx)));
            }
        }
        if self.tags_open && self.selected.is_some() && self.tags_for != self.selected {
            self.load_tags(cx);
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
            .when(!self.selected_tags().is_empty(), |d| d.child(tab("m-tags", "Tags", Some(self.selected_tags().len()), true)))
            .child(div().flex_1())
            .when_some(self.resolving.clone(), |d, n| d.child(div().mr(px(8.)).child(note_line(format!("Finding \"{n}\"..."), false))))
            .when(self.loading_more, |d| d.child(div().mr(px(8.)).child(spinner("more-spin", 14.))))
            .when_some(self.busy.clone(), |d, (_, t)| d.child(div().mr(px(8.)).child(note_line(format!("Opening {t}..."), false))))
            .when_some(self.note.clone(), |d, n| d.child(div().mr(px(8.)).child(note_line(n, true))))
            .when(current.as_ref().is_some_and(|s| s.nsfw), |d| d.child(tag("18+")))
            .child(
                div().ml(px(8.)).child(
                    tool_button("tags-toggle", "icons/tags.svg", if self.tags_open { "Hide tags" } else { "Tags" }, self.tags_open).on_click(cx.listener(|this, _, _, cx| {
                        this.tags_open = !this.tags_open;
                        cx.notify();
                    })),
                ),
            );
        let selected_row = self.selected_row(cx);
        let panel = self.tags_open.then(|| self.tag_panel(current.as_ref(), cx));
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
        root.child(tabs).children(panel).children(selected_row).child(div().relative().flex_1().min_h_0().child(width_probe(cx, |t: &mut Self| &mut t.width)).child(body))
    }
}
