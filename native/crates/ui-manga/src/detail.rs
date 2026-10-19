use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::chapter::Chapter;
use mv_core::repo::manga::{self, Manga};
use mv_ui::{back_target, button, icon, size, ButtonKind, CoverSource, Route, Tokens};

use mv_sources::MangaSearchResult;

use crate::backend::{self, ChapterExtra};
use crate::discover::{self, Recs};
use crate::downloads::{downloads, ChapterDl, Downloads, Job};
use crate::logic::{self, ChapterView};
use crate::ops;
use crate::store;
use crate::tags::MAX_RECS;
use crate::ui::{
    action, cover_src, error_view, icon_tip, manga_card, note_line, select_chip, skeleton_cover, spawn_cover_downloads, spinner, state_view, tool_button, CardInfo, Grid,
    Load,
};

/// Fixed list rows before the chapters: the header and the chapter toolbar.
const LEAD: usize = 2;
const ROW_H: f32 = 58.0;
/// Config key for the chapter order ("1" = newest first; oldest first otherwise, like reading order).
const SORT_KEY: &str = "chapterNewestFirst";
/// Cover cards in the "You may also like" row.
const REC_GRID: Grid = Grid { cols: MAX_RECS, card_w: 116.0 };

pub struct DetailPage {
    manga_id: String,
    /// Downloaded chapter whose delete was clicked once (a second click deletes).
    confirm_delete: Option<String>,
    state: Load<(Manga, Vec<Chapter>)>,
    cover: Option<PathBuf>,
    source_name: Option<String>,
    in_library: bool,
    view: ChapterView,
    /// Chapter indices (into the ascending list) in display order.
    rows: Vec<usize>,
    /// Chapter the Resume / Start button opens.
    current: Option<usize>,
    extras: HashMap<String, ChapterExtra>,
    lang: String,
    expanded: bool,
    refreshing: bool,
    auto_refreshed: bool,
    note: Option<(String, bool)>,
    hovered: Option<String>,
    list: ListState,
    seen_version: u64,
    /// "You may also like" (None until the title's tags are known).
    recs: Option<Load<Recs>>,
    rec_covers: HashMap<String, PathBuf>,
    /// Recommendation being opened (`source_id`).
    rec_busy: Option<String>,
    queue: Entity<Downloads>,
    _watch: Subscription,
}

struct Loaded {
    manga: Manga,
    chapters: Vec<Chapter>,
    cover: Option<PathBuf>,
}

fn load(id: String, lang: String) -> anyhow::Result<Loaded> {
    backend::with_db(|c| {
        let m = manga::get(c, &id)?.ok_or_else(|| anyhow::anyhow!("This title is no longer in the database"))?;
        let cs = store::chapters(c, &id, &lang)?;
        let cover = m.cover_path.as_deref().and_then(mv_core::covers::cached_cover);
        Ok(Loaded { manga: m, chapters: cs, cover })
    })
}

impl DetailPage {
    pub fn new(manga_id: String, cx: &mut Context<Self>) -> Self {
        let queue = downloads(cx);
        let watch = cx.observe(&queue, |_, _, cx| cx.notify());
        let newest_first = backend::config_str(SORT_KEY).as_deref() == Some("1");
        let mut p = Self {
            confirm_delete: None,
            in_library: !backend::browse_only().contains(&manga_id),
            extras: backend::extras(&manga_id).unwrap_or_default(),
            manga_id,
            state: Load::Loading,
            cover: None,
            source_name: None,
            view: ChapterView { newest_first, ..Default::default() },
            rows: vec![],
            current: None,
            lang: backend::preferred_language(),
            expanded: false,
            refreshing: false,
            auto_refreshed: false,
            note: None,
            hovered: None,
            list: ListState::new(0, ListAlignment::Top, px(400.)),
            seen_version: backend::data_version(),
            recs: None,
            rec_covers: HashMap::new(),
            rec_busy: None,
            queue,
            _watch: watch,
        };
        p.reload(cx);
        p
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let (id, lang) = (self.manga_id.clone(), self.lang.clone());
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || load(id, lang)).await.and_then(|r| r);
            let (missing, source) = match &res {
                Ok(l) => (
                    if l.cover.is_none() { l.manga.cover_path.clone().filter(|u| u.starts_with("http")).map(|u| (u, l.manga.source_plugin.clone())) } else { None },
                    Some(l.manga.source_plugin.clone()),
                ),
                _ => (None, None),
            };
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(l) => {
                        let empty = l.chapters.is_empty();
                        if l.cover.is_some() {
                            this.cover = l.cover;
                        }
                        this.in_library = !backend::browse_only().contains(&this.manga_id);
                        this.set_data(l.manga, l.chapters);
                        this.maybe_load_recs(cx);
                        // A title without chapters has never been synced: fetch them once.
                        if empty && !this.auto_refreshed {
                            this.auto_refreshed = true;
                            this.refresh(cx);
                        }
                    }
                    Err(e) => this.state = Load::Failed(format!("{e:#}")),
                }
                cx.notify();
            });
            if let Some(src) = source {
                let name = mv_ui::bg::run(async move { backend::registry().await.get(&src).map(|s| s.info().name) }).await.ok().flatten();
                let _ = this.update(cx, |this, cx| {
                    this.source_name = name;
                    cx.notify();
                });
            }
            if let Some((url, src)) = missing {
                let p = mv_ui::bg::run(backend::ensure_cover(url, Some(src))).await.ok().flatten();
                let _ = this.update(cx, |this, cx| {
                    this.cover = p;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn set_data(&mut self, m: Manga, cs: Vec<Chapter>) {
        self.state = Load::Ready((m, cs));
        self.sync_rows();
        self.list.remeasure_items(0..LEAD.min(self.list.item_count()));
    }

    /// Recompute the visible chapters and tell the list how many rows it has.
    fn sync_rows(&mut self) {
        (self.rows, self.current) = match &self.state {
            Load::Ready((_, cs)) => (logic::visible_chapters(cs, self.view), logic::continue_target(cs).map(|(i, _)| i)),
            _ => (vec![], None),
        };
        // header + toolbar + (chapters or one empty-state row) + bottom spacer for the Resume button
        let want = LEAD + self.rows.len().max(1) + 1;
        let have = self.list.item_count();
        if have < LEAD {
            self.list.reset(want);
        } else if have != want {
            self.list.splice(LEAD..have, want - LEAD);
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        self.note = None;
        cx.notify();
        let id = self.manga_id.clone();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(ops::refresh_manga(id)).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.refreshing = false;
                match res {
                    Ok((m, cs)) => {
                        let before = match &this.state {
                            Load::Ready((_, old)) => old.len(),
                            _ => 0,
                        };
                        let added = cs.len().saturating_sub(before);
                        this.note = Some((if added > 0 { format!("{added} new chapters") } else { "Up to date".into() }, false));
                        this.extras = backend::extras(&this.manga_id).unwrap_or_default();
                        this.seen_version = backend::data_version();
                        this.set_data(m, cs);
                        this.maybe_load_recs(cx);
                    }
                    Err(e) => this.note = Some((format!("Refresh failed: {e:#}"), true)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Start the recommendations once the title has tags (fresh titles get them on refresh).
    fn maybe_load_recs(&mut self, cx: &mut Context<Self>) {
        if self.recs.is_some() {
            return;
        }
        if matches!(&self.state, Load::Ready((m, _)) if !m.genres.is_empty()) {
            self.load_recs(cx);
        }
    }

    fn load_recs(&mut self, cx: &mut Context<Self>) {
        let Load::Ready((m, _)) = &self.state else { return };
        let (src, sid, title, genres) = (m.source_plugin.clone(), m.source_id.clone(), m.title.clone(), m.genres.clone());
        self.recs = Some(Load::Loading);
        self.list.remeasure_items(0..1.min(self.list.item_count()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(discover::recommendations(src, sid, title, genres)).await.and_then(|r| r);
            let covers = match &res {
                Ok(r) => {
                    let urls: Vec<_> = r.items.iter().filter_map(|i| i.cover_url.clone().map(|u| (u, Some(r.source_id.clone())))).collect();
                    mv_ui::bg::blocking(move || backend::resolve_covers(urls)).await.ok()
                }
                Err(_) => None,
            };
            let _ = this.update(cx, |this, cx| {
                this.recs = Some(match res {
                    Ok(r) => Load::Ready(r),
                    Err(e) => Load::Failed(format!("{e:#}")),
                });
                if let Some((have, missing)) = covers {
                    this.rec_covers.extend(have);
                    spawn_cover_downloads(cx, missing, |t: &mut Self| &mut t.rec_covers);
                }
                this.list.remeasure_items(0..1.min(this.list.item_count()));
                cx.notify();
            });
        })
        .detach();
    }

    /// Open a recommended title (straight away when it is already in the database).
    fn open_rec(&mut self, source_id: String, r: MangaSearchResult, window: &mut Window, cx: &mut Context<Self>) {
        if self.rec_busy.is_some() {
            return;
        }
        self.rec_busy = Some(r.source_id.clone());
        self.note = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let (src, rid) = (source_id.clone(), r.source_id.clone());
            let known = mv_ui::bg::blocking(move || backend::with_db(|c| manga::find_by_source(c, &src, &rid))).await.ok().and_then(|r| r.ok()).flatten();
            let res = match known {
                Some(m) => Ok(m),
                None => mv_ui::bg::run(ops::prepare_from_source(source_id.clone(), r)).await.and_then(|r| r),
            };
            let _ = this.update_in(cx, |this, w, cx| {
                this.rec_busy = None;
                match res {
                    Ok(m) => mv_ui::navigate(Route::MangaDetail { manga_id: m.id, source_id: Some(source_id) }, w, cx),
                    Err(e) => this.note = Some((format!("Could not open: {e:#}"), true)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// "You may also like": 5-8 titles sharing this one's main tags.
    fn recs_section(&self, cx: &mut Context<Self>) -> Option<Div> {
        let recs = self.recs.as_ref()?;
        let heading = |sub: Option<String>| {
            div()
                .flex()
                .items_baseline()
                .gap(px(10.))
                .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("You may also like"))
                .when_some(sub, |d, s| d.child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(s)))
        };
        let body: AnyElement = match recs {
            Load::Loading => div().flex().gap(px(12.)).overflow_hidden().children((0..6).map(|_| skeleton_cover(REC_GRID))).into_any_element(),
            Load::Failed(e) => div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(note_line(format!("Could not load recommendations: {e}"), true))
                .child(tool_button("recs-retry", "icons/refresh-cw.svg", "Try again", false).on_click(cx.listener(|this, _, _, cx| this.load_recs(cx))))
                .into_any_element(),
            Load::Ready(r) if r.items.is_empty() => {
                note_line(if r.basis.is_empty() { "No similar titles: none of this title's tags can be searched." } else { "No similar titles found." }, false).into_any_element()
            }
            // One horizontal row (scrolls sideways) so the chapters stay close to the top.
            Load::Ready(r) => div()
                .id("recs-row")
                .flex()
                .overflow_x_scroll()
                .gap(px(12.))
                .children(r.items.iter().map(|it| {
                    let item = it.clone();
                    let src = r.source_id.clone();
                    let busy = self.rec_busy.as_deref() == Some(it.source_id.as_str());
                    let info = CardInfo { title: it.title.clone().into(), cover: cover_src(&self.rec_covers, it.cover_url.as_deref()), ..Default::default() };
                    manga_card(SharedString::from(format!("rec-{}", it.source_id)), info, REC_GRID)
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
                                    .child(spinner(SharedString::from(format!("rec-spin-{}", it.source_id)), 22.)),
                            )
                        })
                        .on_click(cx.listener(move |this, _, w, cx| this.open_rec(src.clone(), item.clone(), w, cx)))
                }))
                .into_any_element(),
        };
        let sub = match recs {
            Load::Ready(r) if !r.basis.is_empty() => {
                let own = matches!(&self.state, Load::Ready((m, _)) if m.source_plugin == r.source_id);
                let from = if own { String::new() } else { format!(" on {}", r.source_name) };
                Some(format!("Tagged {}{from}", r.basis.join(" + ")))
            }
            _ => None,
        };
        Some(div().flex().flex_col().gap(px(10.)).px(px(size::PAGE_X)).pb(px(18.)).child(heading(sub)).child(body))
    }

    fn toggle_favorite(&mut self, cx: &mut Context<Self>) {
        if let Load::Ready((m, _)) = &mut self.state {
            m.is_favorite = !m.is_favorite;
            cx.notify();
        }
        let id = self.manga_id.clone();
        cx.spawn(async move |_, _| {
            let _ = mv_ui::bg::blocking(move || backend::with_db(|c| manga::toggle_favorite(c, &id))).await;
            backend::bump_data();
        })
        .detach();
    }

    fn toggle_library(&mut self, cx: &mut Context<Self>) {
        self.in_library = !self.in_library;
        backend::set_in_library(&self.manga_id, self.in_library);
        self.seen_version = backend::data_version();
        self.note = Some((if self.in_library { "Added to library".into() } else { "Removed from library".into() }, false));
        cx.notify();
    }

    /// Mark chapters read / unread, updating the list immediately.
    fn set_read(&mut self, ids: Vec<String>, read: bool, cx: &mut Context<Self>) {
        if let Load::Ready((_, cs)) = &mut self.state {
            for c in cs.iter_mut().filter(|c| ids.contains(&c.id)) {
                c.is_read = read;
                if !read {
                    c.read_progress = 0;
                }
            }
        }
        self.sync_rows();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let _ = mv_ui::bg::blocking(move || backend::with_db(|c| store::set_read(c, &ids, read))).await;
            let _ = this.update(cx, |this, _| this.seen_version = backend::data_version());
        })
        .detach();
    }

    fn set_view(&mut self, view: ChapterView, cx: &mut Context<Self>) {
        if view.newest_first != self.view.newest_first {
            backend::set_config_str(SORT_KEY, if view.newest_first { "1" } else { "0" });
        }
        self.view = view;
        self.sync_rows();
        cx.notify();
    }

    /// Queue chapters (by id) that are not downloaded yet, in reading order.
    fn download(&mut self, ids: HashSet<String>, cx: &mut Context<Self>) {
        let Load::Ready((m, cs)) = &self.state else { return };
        let jobs: Vec<Job> = cs
            .iter()
            .filter(|c| ids.contains(&c.id) && c.download_status != "complete")
            .map(|c| Job { manga_id: m.id.clone(), manga_title: m.title.clone(), chapter_id: c.id.clone(), label: logic::chapter_short(c) })
            .collect();
        self.queue.update(cx, |d, cx| d.enqueue(jobs, cx));
    }

    fn delete_download(&mut self, chapter_id: String, cx: &mut Context<Self>) {
        if self.confirm_delete.as_deref() != Some(chapter_id.as_str()) {
            self.confirm_delete = Some(chapter_id);
            return cx.notify();
        }
        self.confirm_delete = None;
        if let Load::Ready((_, cs)) = &mut self.state {
            if let Some(c) = cs.iter_mut().find(|c| c.id == chapter_id) {
                c.download_status = "none".into();
            }
        }
        self.sync_rows();
        let mid = self.manga_id.clone();
        self.queue.update(cx, |d, cx| d.delete(vec![(mid, chapter_id)], false, cx));
        cx.notify();
    }

    fn render_item(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        if !matches!(self.state, Load::Ready(_)) {
            return div().into_any_element();
        }
        // Borrow the data out of `self` for the duration of the row (no per-row clone of the list).
        let state = std::mem::replace(&mut self.state, Load::Loading);
        let Load::Ready((m, cs)) = &state else { unreachable!() };
        let el = match ix {
            0 => self.header(m, cx).into_any_element(),
            1 => self.toolbar(m, cs, cx).into_any_element(),
            _ => {
                let r = ix - LEAD;
                match self.rows.get(r) {
                    Some(&i) if i < cs.len() => self.chapter_row(m, cs, i, cx).into_any_element(),
                    None if r == 0 => self.empty_chapters(cs, cx).into_any_element(),
                    _ => div().h(px(96.)).into_any_element(),
                }
            }
        };
        self.state = state;
        // List items size to their content unless told otherwise: stretch every row edge to edge.
        div().w_full().child(el).into_any_element()
    }

    fn header(&self, m: &Manga, cx: &mut Context<Self>) -> Div {
        let cover = self.cover.clone().map(CoverSource::File);
        let backdrop = self.cover.clone().map(CoverSource::File);
        let desc = logic::clean_description(m.description.as_deref().unwrap_or_default());
        let long = desc.chars().count() > 260 || desc.lines().count() > 3;
        let web = logic::web_url(&m.source_plugin, &m.source_id);
        let mut people: Vec<String> = vec![];
        if let Some(a) = m.author.clone().filter(|s| !s.is_empty()) {
            people.push(a);
        }
        if let Some(a) = m.artist.clone().filter(|s| !s.is_empty() && Some(s) != m.author.as_ref()) {
            people.push(a);
        }
        let status = m.status.clone().filter(|s| !s.is_empty());
        let source = self.source_name.clone().unwrap_or_else(|| m.source_plugin.clone());

        let top = div()
            .relative()
            .w_full()
            .overflow_hidden()
            // Edge-to-edge backdrop: the cover stretched across the whole page width, faded into the page.
            .when_some(backdrop, |d, c| d.child(c.into_img().absolute().top_0().left_0().size_full().object_fit(ObjectFit::Cover).opacity(0.3)))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .bg(linear_gradient(90., linear_color_stop(rgba(0x0b0d10cc), 0.), linear_color_stop(rgba(0x0b0d1033), 0.8))),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .bg(linear_gradient(180., linear_color_stop(rgba(0x0b0d1040), 0.), linear_color_stop(Tokens::base(), 1.))),
            )
            .when_some(back_label(cx), |d, label| d.child(div().relative().flex().px(px(size::PAGE_X - 10.)).pt(px(14.)).child(back_pill(label))))
            .child(
                div()
                    .relative()
                    .flex()
                    .items_end()
                    .gap(px(28.))
                    .px(px(size::PAGE_X))
                    .pt(px(20.))
                    .pb(px(22.))
                    .child(
                        div()
                            .w(px(168.))
                            .h(px(252.))
                            .flex_none()
                            .rounded(px(10.))
                            .overflow_hidden()
                            .bg(Tokens::surface_2())
                            .border_1()
                            .border_color(Tokens::border())
                            .shadow_lg()
                            .map(|d| match cover {
                                Some(c) => d.child(c.into_img().size_full().object_fit(ObjectFit::Cover)),
                                None => d.flex().items_center().justify_center().child(icon("icons/book-open.svg", rgba(0xffffff1f), 36.)),
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(px(size::XXL))
                                    .line_height(px(34.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(Tokens::text_1())
                                    .line_clamp(3)
                                    .child(m.title.clone()),
                            )
                            .when(!people.is_empty(), |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .text_size(px(size::MD))
                                        .text_color(Tokens::text_2())
                                        .child(icon("icons/user.svg", Tokens::text_3(), 14.))
                                        .child(people.join(", ")),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .text_size(px(size::SM))
                                    .text_color(Tokens::text_3())
                                    .child(icon(status_icon(status.as_deref()), Tokens::text_3(), 14.))
                                    .child(status.unwrap_or_else(|| "Unknown status".into()))
                                    .child("·")
                                    .child(source)
                                    .when(self.lang != "all", |d| d.child("·").child(self.lang.to_uppercase())),
                            ),
                    ),
            );

        let actions = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(size::PAGE_X - 10.))
            .child(
                action("act-lib", if self.in_library { "icons/bookmark-check.svg" } else { "icons/bookmark-plus.svg" }, if self.in_library { "In library" } else { "Add to library" }, self.in_library)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_library(cx))),
            )
            .child(
                action("act-fav", "icons/heart.svg", "Favorite", m.is_favorite)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_favorite(cx))),
            )
            .child(
                action("act-refresh", "icons/refresh-cw.svg", if self.refreshing { "Refreshing" } else { "Refresh" }, false)
                    .when(self.refreshing, |d| d.opacity(0.6))
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
            .when_some(web, |d, url| {
                d.child(action("act-web", "icons/globe.svg", "Open in browser", false).on_click(move |_, _, cx| cx.open_url(&url)))
            })
            .child(div().flex_1())
            .when_some(self.note.clone(), |d, (n, err)| d.child(div().mr(px(10.)).child(note_line(n, err))));

        let expanded = self.expanded;
        let about = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .px(px(size::PAGE_X))
            .pt(px(12.))
            .pb(px(16.))
            .when(!desc.is_empty(), |d| {
                d.child(
                    div()
                        .id("desc")
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .max_w(px(860.))
                        .when(long, |d| {
                            d.cursor_pointer().on_click(cx.listener(|this, _, _, cx| {
                                this.expanded = !this.expanded;
                                this.list.remeasure_items(0..1);
                                cx.notify();
                            }))
                        })
                        .child(div().text_size(px(size::MD)).line_height(px(21.)).text_color(Tokens::text_2()).when(!expanded, |d| d.line_clamp(3)).child(desc))
                        .when(long, |d| {
                            d.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .text_size(px(size::SM))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(Tokens::accent())
                                    .child(if expanded { "Show less" } else { "Show more" })
                                    .child(icon(if expanded { "icons/chevron-up.svg" } else { "icons/chevron-down.svg" }, Tokens::accent(), 14.)),
                            )
                        }),
                )
            })
            .when(!m.genres.is_empty(), |d| {
                d.child(div().flex().flex_wrap().gap(px(6.)).children(m.genres.iter().take(16).enumerate().map(|(i, g)| {
                    let (src, name) = (m.source_plugin.clone(), g.clone());
                    select_chip(SharedString::from(format!("genre-{i}")), g.clone(), false)
                        .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Browse this tag").build(w, cx))
                        .on_click(move |_, w, cx| {
                            discover::request_tag(&src, &name);
                            mv_ui::navigate(Route::Browse, w, cx);
                        })
                })))
            });

        div().flex().flex_col().child(top).child(actions).child(about).children(self.recs_section(cx))
    }

    /// Chapter section header: counts, the download button, then filters / order / mark all.
    fn toolbar(&self, m: &Manga, cs: &[Chapter], cx: &mut Context<Self>) -> Div {
        let v = self.view;
        let all_read = !cs.is_empty() && cs.iter().all(|c| c.is_read);
        let unread = cs.iter().filter(|c| !c.is_read).count();
        let downloaded = cs.iter().filter(|c| c.download_status == "complete").count();
        let queue = self.queue.read(cx);
        let pending = queue.pending_for(&m.id);
        let missing: Vec<&Chapter> = cs.iter().filter(|c| c.download_status != "complete" && queue.status(&c.id).is_none()).collect();
        let missing_unread = missing.iter().filter(|c| !c.is_read).count();

        let mut stats = vec![format!("{} chapter{}", cs.len(), if cs.len() == 1 { "" } else { "s" })];
        if unread > 0 && unread < cs.len() {
            stats.push(format!("{unread} unread"));
        }
        if downloaded > 0 {
            stats.push(format!("{downloaded} downloaded"));
        }

        let download: AnyElement = if pending > 0 {
            let mid = m.id.clone();
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(34.))
                .pl(px(12.))
                .pr(px(4.))
                .rounded(px(9.))
                .bg(Tokens::accent_soft())
                .text_size(px(size::SM))
                .font_weight(FontWeight::MEDIUM)
                .text_color(Tokens::accent())
                .child(spinner("dl-spin", 14.))
                .child(format!("Downloading · {pending} left"))
                .child(
                    div()
                        .id("dl-cancel-title")
                        .px(px(8.))
                        .h(px(26.))
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .text_color(Tokens::text_2())
                        .hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
                        .child("Cancel")
                        .on_click(cx.listener(move |this, _, _, cx| this.queue.update(cx, |d, cx| d.cancel_manga(&mid, cx)))),
                )
                .into_any_element()
        } else if missing.is_empty() && !cs.is_empty() {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .h(px(34.))
                .px(px(12.))
                .rounded(px(9.))
                .border_1()
                .border_color(Tokens::border())
                .text_size(px(size::SM))
                .text_color(Tokens::text_2())
                .child(icon("icons/circle-check.svg", Tokens::accent(), 15.))
                .child("All chapters downloaded")
                .into_any_element()
        } else if missing.is_empty() {
            div().into_any_element()
        } else {
            let all: HashSet<String> = missing.iter().map(|c| c.id.clone()).collect();
            let unread_ids: HashSet<String> = missing.iter().filter(|c| !c.is_read).map(|c| c.id.clone()).collect();
            let n = missing.len();
            let label = if downloaded == 0 { format!("Download all {n} chapters") } else { format!("Download {n} more") };
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .when(missing_unread > 0 && missing_unread < n, |d| {
                    d.child(
                        button("dl-unread", format!("Unread ({missing_unread})"), ButtonKind::Ghost)
                            .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Download only unread chapters").build(w, cx))
                            .on_click(cx.listener(move |this, _, _, cx| this.download(unread_ids.clone(), cx))),
                    )
                })
                .child(
                    button("dl-all", label, ButtonKind::Primary)
                        .h(px(34.))
                        .px(px(14.))
                        .child(icon("icons/download.svg", Tokens::base(), 15.))
                        .flex_row_reverse()
                        .on_click(cx.listener(move |this, _, _, cx| this.download(all.clone(), cx))),
                )
                .into_any_element()
        };

        let filters = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .h(px(44.))
            .border_b_1()
            .border_color(Tokens::border())
            .child(tool_button("f-unread", "icons/eye-off.svg", "Unread", v.unread_only).on_click(cx.listener(move |this, _, _, cx| {
                this.set_view(ChapterView { unread_only: !v.unread_only, ..v }, cx)
            })))
            .child(tool_button("f-dl", "icons/folder-down.svg", "Downloaded", v.downloaded_only).on_click(cx.listener(move |this, _, _, cx| {
                this.set_view(ChapterView { downloaded_only: !v.downloaded_only, ..v }, cx)
            })))
            .child(
                tool_button(
                    "sort",
                    if v.newest_first { "icons/arrow-down-wide-narrow.svg" } else { "icons/arrow-up-narrow-wide.svg" },
                    if v.newest_first { "Newest first" } else { "Oldest first" },
                    false,
                )
                .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Reverse the chapter order").build(w, cx))
                .on_click(cx.listener(move |this, _, _, cx| this.set_view(ChapterView { newest_first: !v.newest_first, ..v }, cx))),
            )
            .child(div().flex_1())
            .when(self.refreshing, |d| d.child(div().mr(px(6.)).child(spinner("refresh-spin", 16.))))
            .when(!cs.is_empty(), |d| {
                let ids: Vec<String> = cs.iter().map(|c| c.id.clone()).collect();
                d.child(
                    tool_button("mark-all", if all_read { "icons/rotate-ccw.svg" } else { "icons/check-check.svg" }, if all_read { "Mark all unread" } else { "Mark all read" }, false)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_read(ids.clone(), !all_read, cx))),
                )
            });

        div()
            .w_full()
            .flex()
            .flex_col()
            .px(px(size::PAGE_X))
            .pt(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .pb(px(6.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .gap(px(10.))
                            .child(div().text_size(px(size::XL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Chapters"))
                            .child(div().truncate().text_size(px(size::SM)).text_color(Tokens::text_3()).child(stats.join("  ·  "))),
                    )
                    .child(download),
            )
            .child(filters)
    }

    fn empty_chapters(&self, cs: &[Chapter], cx: &mut Context<Self>) -> Div {
        if self.refreshing && cs.is_empty() {
            return div().flex().items_center().justify_center().gap(px(10.)).h(px(160.)).child(spinner("chapters-spin", 18.)).child(note_line("Fetching chapters...", false));
        }
        if cs.is_empty() {
            return div()
                .flex()
                .flex_col()
                .items_center()
                .child(state_view("icons/book-open.svg", "No chapters yet", "The source has not listed any readable chapters."))
                .child(button("refresh-empty", "Check again", ButtonKind::Secondary).on_click(cx.listener(|this, _, _, cx| this.refresh(cx))));
        }
        div().child(state_view("icons/list-filter.svg", "No chapters match", "Turn off the Unread or Downloaded filter to see every chapter."))
    }

    fn chapter_row(&self, m: &Manga, cs: &[Chapter], i: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let c = &cs[i];
        let current = self.current == Some(i);
        let route = Route::Reader { manga_id: m.id.clone(), chapter_id: c.id.clone() };
        let extra = self.extras.get(&c.source_id).cloned().unwrap_or_default();
        let mut meta: Vec<String> = vec![];
        if let Some(d) = extra.date {
            meta.push(d);
        }
        if let Some(g) = extra.group {
            meta.push(g);
        }
        if !logic::lang_matches(&c.language, &self.lang) {
            meta.push(c.language.to_uppercase());
        }
        let progress = (!c.is_read && c.read_progress > 0).then(|| format!("Page {}", c.read_progress + 1));
        let downloaded = c.download_status == "complete";
        let dl = self.queue.read(cx).status(&c.id);
        let hovered = self.hovered.as_deref() == Some(c.id.as_str());
        let (hid, rid) = (c.id.clone(), c.id.clone());
        let read = c.is_read;
        let title_color = if read { Tokens::text_3() } else { Tokens::text_1() };
        div()
            .id(SharedString::from(format!("ch-{}", c.id)))
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(ROW_H))
            .mx(px(size::PAGE_X - 10.))
            .px(px(10.))
            .rounded(px(8.))
            .cursor_pointer()
            .hover(|s| s.bg(Tokens::hover()))
            .on_hover(cx.listener(move |this, h: &bool, _, cx| {
                if *h {
                    this.hovered = Some(hid.clone());
                } else if this.hovered.as_deref() == Some(hid.as_str()) {
                    this.hovered = None;
                }
                cx.notify();
            }))
            .on_click(move |_, w, cx| mv_ui::navigate(route.clone(), w, cx))
            .child(div().w(px(3.)).h(px(28.)).rounded(px(2.)).flex_none().when(current, |d| d.bg(Tokens::accent())))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(size::MD))
                            .font_weight(if read { FontWeight::NORMAL } else { FontWeight::MEDIUM })
                            .text_color(title_color)
                            .child(logic::chapter_label(c)),
                    )
                    .when(!meta.is_empty() || progress.is_some(), |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .text_size(px(size::XS))
                                .text_color(if read { rgba(0x7d859099) } else { Tokens::text_3() })
                                .when(!meta.is_empty(), |d| d.child(div().truncate().child(meta.join("  ·  "))))
                                .when_some(progress, |d, p| {
                                    d.when(!meta.is_empty(), |d| d.child("·")).child(div().flex_none().text_color(Tokens::accent()).child(p))
                                }),
                        )
                    }),
            )
            .when(hovered, |d| {
                d.child(
                    icon_tip(SharedString::from(format!("mr-{}", c.id)), if read { "icons/rotate-ccw.svg" } else { "icons/check.svg" }, if read { "Mark unread" } else { "Mark read" })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.set_read(vec![rid.clone()], !read, cx);
                        })),
                )
            })
            .child(self.download_control(c, downloaded, dl, cx))
    }

    /// Per-chapter download state at the end of a row (Tachiyomi style): download, queued,
    /// progress, or downloaded (click to delete).
    fn download_control(&self, c: &Chapter, downloaded: bool, dl: Option<ChapterDl>, cx: &mut Context<Self>) -> AnyElement {
        let id = SharedString::from(format!("dlc-{}", c.id));
        let base = |id: SharedString| {
            div().id(id).flex().flex_none().items_center().justify_center().gap(px(4.)).h(px(32.)).min_w(px(32.)).rounded(px(8.)).cursor_pointer().hover(|s| s.bg(Tokens::hover()))
        };
        match dl {
            Some(ChapterDl::Active(f)) => div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .h(px(32.))
                .min_w(px(32.))
                .child(spinner(SharedString::from(format!("dls-{}", c.id)), 15.))
                .child(div().text_size(px(size::XS)).text_color(Tokens::accent()).child(format!("{}%", (f * 100.).round())))
                .into_any_element(),
            Some(ChapterDl::Queued) => base(id)
                .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Queued for download").build(w, cx))
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(icon("icons/clock.svg", Tokens::text_3(), 15.))
                .into_any_element(),
            None if downloaded => {
                let cid = c.id.clone();
                let confirming = self.confirm_delete.as_deref() == Some(c.id.as_str());
                base(id)
                    .tooltip(move |w, cx| {
                        let tip = if confirming { "Click again to delete the pages and this chapter's Studio work" } else { "Downloaded · click to delete" };
                        gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx)
                    })
                    .when(confirming, |d| d.px(px(8.)).bg(rgba(0xf8717124)).child(icon("icons/trash.svg", Tokens::danger(), 15.)).child(div().text_size(px(size::XS)).text_color(Tokens::danger()).child("Delete?")))
                    .when(!confirming, |d| d.child(icon("icons/circle-check.svg", Tokens::accent(), 16.)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.delete_download(cid.clone(), cx);
                    }))
                    .into_any_element()
            }
            None => {
                let cid = c.id.clone();
                base(id)
                    .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Download").build(w, cx))
                    .child(icon("icons/arrow-down-to-line.svg", Tokens::text_3(), 16.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.download(HashSet::from([cid.clone()]), cx);
                    }))
                    .into_any_element()
            }
        }
    }

    /// Floating Resume / Start button (always reachable while scrolling the chapter list).
    fn fab(&self, m: &Manga, cs: &[Chapter]) -> Option<Stateful<Div>> {
        let (i, resume) = logic::continue_target(cs)?;
        let route = Route::Reader { manga_id: m.id.clone(), chapter_id: cs[i].id.clone() };
        let label = format!("{} {}", if resume { "Resume" } else { "Start" }, logic::chapter_short(&cs[i]));
        Some(
            div()
                .id("fab")
                .absolute()
                .bottom(px(24.))
                .right(px(size::PAGE_X))
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(44.))
                .px(px(18.))
                .rounded(px(999.))
                .bg(Tokens::accent())
                .shadow_lg()
                .cursor_pointer()
                .hover(|s| s.opacity(0.92))
                .text_size(px(size::MD))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(Tokens::base())
                .child(icon("icons/play.svg", Tokens::base(), 16.))
                .child(label)
                .on_click(move |_, w, cx| mv_ui::navigate(route.clone(), w, cx)),
        )
    }
}

fn status_icon(status: Option<&str>) -> &'static str {
    let s = status.unwrap_or("").to_lowercase();
    if s.contains("complete") || s.contains("finished") {
        "icons/circle-check.svg"
    } else if s.contains("hiatus") || s.contains("cancel") {
        "icons/circle-pause.svg"
    } else {
        "icons/clock.svg"
    }
}

impl Render for DetailPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Read state changed in the reader (or elsewhere) while this page was in history.
        let v = backend::data_version();
        if v != self.seen_version {
            self.seen_version = v;
            self.lang = backend::preferred_language();
            self.extras = backend::extras(&self.manga_id).unwrap_or_default();
            self.reload(cx);
        }
        let root = div().size_full().relative();
        match &self.state {
            Load::Loading => root.child(
                div()
                    .flex()
                    .gap(px(24.))
                    .px(px(size::PAGE_X))
                    .pt(px(32.))
                    .child(div().w(px(150.)).h(px(225.)).rounded(px(10.)).bg(Tokens::surface_2()))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .justify_end()
                            .gap(px(10.))
                            .child(div().w(px(320.)).h(px(24.)).rounded(px(6.)).bg(Tokens::surface_2()))
                            .child(div().w(px(180.)).h(px(12.)).rounded(px(4.)).bg(Tokens::surface_2())),
                    ),
            ),
            Load::Failed(e) => root.child(error_view(e.clone())),
            Load::Ready((m, cs)) => {
                let fab = self.fab(m, cs);
                root.child(list(self.list.clone(), cx.processor(|this: &mut Self, ix: usize, _w, cx| this.render_item(ix, cx))).size_full())
                    .when_some(fab, |d, f| d.child(f))
            }
        }
    }
}

/// "Back to Library" style label for wherever the user opened this manga from.
fn back_label(cx: &App) -> Option<SharedString> {
    let label = match back_target(cx)? {
        Route::Browse => "Back to Browse",
        Route::Library => "Back to Library",
        Route::MangaHistory => "Back to History",
        Route::MangaDownloads => "Back to Downloads",
        Route::Settings { .. } => "Back to Settings",
        _ => "Back",
    };
    Some(label.into())
}

fn back_pill(label: SharedString) -> Stateful<Div> {
    div()
        .id("detail-back")
        .flex()
        .items_center()
        .gap(px(6.))
        .w_auto()
        .flex_none()
        .h(px(30.))
        .px(px(10.))
        .rounded(px(8.))
        .cursor_pointer()
        .text_size(px(size::SM))
        .text_color(Tokens::text_2())
        .bg(rgba(0x0000004d))
        .hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
        .child(icon("icons/arrow-left.svg", Tokens::text_2(), 15.))
        .child(label)
        .on_click(|_, w, cx| mv_ui::navigate(Route::Back, w, cx))
}
