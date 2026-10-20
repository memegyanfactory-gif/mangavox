use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::category::{self, LibraryCategory};
use mv_core::repo::chapter::Chapter;
use mv_core::repo::manga::{self, Manga};
use mv_sources::MangaSearchResult;
use mv_ui::theme::{icon as icon_px, radius, ramp};
use mv_ui::toast::{self, Toast};
use mv_ui::{
    back_target, btn, chip, empty_view, error_view, filter_chip, glyphs, icon_button, icon_ph, inline_alert, muted, ph, ph_icon, progress_ring_sized, section_header, size, skeleton,
    skeleton_line, status_tag, tip, AlertKind, ButtonKind, CoverSource, Interactive, RampExt, Route, StatusKind, Tokens, Weight,
};

use crate::backend::{self, ChapterExtra};
use crate::categories;
use crate::discover::{self, Recs};
use crate::downloads::{downloads, ChapterDl, Downloads, Job};
use crate::logic::{self, ChapterView};
use crate::ops;
use crate::store;
use crate::tags::MAX_RECS;
use crate::ui::{action_tile, cover_src, fill_menu, icon_trigger, manga_card, skeleton_cover, spawn_cover_downloads, CardInfo, Entry, Grid, Load};

/// Fixed list rows before the chapters: the header and the chapter toolbar.
const LEAD: usize = 2;
const ROW_H: f32 = 58.0;
/// Config key for the chapter order ("1" = newest first; oldest first otherwise, like reading order).
const SORT_KEY: &str = "chapterNewestFirst";
/// Cover cards in the "You may also like" row.
const REC_GRID: Grid = Grid { cols: MAX_RECS, card_w: 116.0 };
/// A deleted download keeps its files until its Undo toast has had its time.
const UNDO_WINDOW: Duration = Duration::from_millis(6_500);

pub struct DetailPage {
    manga_id: String,
    state: Load<(Manga, Vec<Chapter>)>,
    cover: Option<PathBuf>,
    source_name: Option<String>,
    in_library: bool,
    /// Every library category, and the ones this title is in.
    cats: Vec<LibraryCategory>,
    member: Vec<String>,
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
    /// Downloads deleted in the UI whose files go after the Undo window.
    pending_delete: HashSet<String>,
    /// Bumped on each toggle so the action tile replays its pop.
    fav_pop: u64,
    lib_pop: u64,
    list: ListState,
    seen_version: u64,
    /// "You may also like" (None until the title's tags are known).
    recs: Option<Load<Recs>>,
    rec_covers: HashMap<String, PathBuf>,
    recs_scroll: ScrollHandle,
    /// Recommendation being opened (`source_id`).
    rec_busy: Option<String>,
    queue: Entity<Downloads>,
    _watch: Subscription,
}

struct Loaded {
    manga: Manga,
    chapters: Vec<Chapter>,
    cover: Option<PathBuf>,
    cats: Vec<LibraryCategory>,
    member: Vec<String>,
}

fn load(id: String, lang: String) -> anyhow::Result<Loaded> {
    backend::with_db(|c| {
        let m = manga::get(c, &id)?.ok_or_else(|| anyhow::anyhow!("This title is no longer in the database"))?;
        let cs = store::chapters(c, &id, &lang)?;
        let cover = m.cover_path.as_deref().and_then(mv_core::covers::cached_cover);
        let cats = category::list(c)?;
        let member = category::manga_category_map(c)?.remove(&id).unwrap_or_default();
        Ok(Loaded { manga: m, chapters: cs, cover, cats, member })
    })
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

impl DetailPage {
    pub fn new(manga_id: String, cx: &mut Context<Self>) -> Self {
        let queue = downloads(cx);
        let watch = cx.observe(&queue, |_, _, cx| cx.notify());
        let newest_first = backend::config_str(SORT_KEY).as_deref() == Some("1");
        let mut p = Self {
            in_library: !backend::browse_only().contains(&manga_id),
            extras: backend::extras(&manga_id).unwrap_or_default(),
            manga_id,
            state: Load::Loading,
            cover: None,
            source_name: None,
            cats: vec![],
            member: vec![],
            view: ChapterView { newest_first, ..Default::default() },
            rows: vec![],
            current: None,
            lang: backend::preferred_language(),
            expanded: false,
            refreshing: false,
            auto_refreshed: false,
            pending_delete: HashSet::new(),
            fav_pop: 0,
            lib_pop: 0,
            list: ListState::new(0, ListAlignment::Top, px(400.)),
            seen_version: backend::data_version(),
            recs: None,
            rec_covers: HashMap::new(),
            recs_scroll: ScrollHandle::new(),
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
                        this.cats = l.cats;
                        this.member = l.member;
                        this.set_data(l.manga, l.chapters);
                        this.maybe_load_recs(cx);
                        // A title without chapters has never been synced: fetch them once.
                        if empty && !this.auto_refreshed {
                            this.auto_refreshed = true;
                            this.refresh(true, cx);
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

    fn set_data(&mut self, m: Manga, mut cs: Vec<Chapter>) {
        // A download deleted in the UI stays deleted while its Undo is still offered.
        for c in cs.iter_mut().filter(|c| self.pending_delete.contains(&c.id)) {
            c.download_status = "none".into();
        }
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

    /// Re-read chapters from the source. `quiet` (the first sync of a new title) only reports
    /// failures; a refresh the user asked for always says what happened.
    fn refresh(&mut self, quiet: bool, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        self.list.remeasure_items(0..LEAD.min(self.list.item_count()));
        cx.notify();
        let id = self.manga_id.clone();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(ops::refresh_manga(id)).await.and_then(|r| r);
            let weak = this.clone();
            let _ = this.update(cx, |this, cx| {
                this.refreshing = false;
                match res {
                    Ok((m, cs)) => {
                        let before = match &this.state {
                            Load::Ready((_, old)) => old.len(),
                            _ => 0,
                        };
                        let added = cs.len().saturating_sub(before);
                        if !quiet {
                            toast::show(cx, if added > 0 { Toast::success(plural(added, "new chapter", "new chapters")) } else { Toast::success("Up to date: no new chapters").icon(ph::CHECK_CIRCLE) });
                        }
                        this.extras = backend::extras(&this.manga_id).unwrap_or_default();
                        this.seen_version = backend::data_version();
                        this.set_data(m, cs);
                        this.maybe_load_recs(cx);
                    }
                    Err(e) => toast::show(
                        cx,
                        Toast::error(format!("Could not refresh: {e:#}")).action("Retry", move |_, cx| {
                            let _ = weak.update(cx, |t, cx| t.refresh(false, cx));
                        }),
                    ),
                }
                this.list.remeasure_items(0..LEAD.min(this.list.item_count()));
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
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let (src, rid) = (source_id.clone(), r.source_id.clone());
            let title = r.title.clone();
            let known = mv_ui::bg::blocking(move || backend::with_db(|c| manga::find_by_source(c, &src, &rid))).await.ok().and_then(|r| r.ok()).flatten();
            let res = match known {
                Some(m) => Ok(m),
                None => mv_ui::bg::run(ops::prepare_from_source(source_id.clone(), r)).await.and_then(|r| r),
            };
            let _ = this.update_in(cx, |this, w, cx| {
                this.rec_busy = None;
                match res {
                    Ok(m) => mv_ui::navigate(Route::MangaDetail { manga_id: m.id, source_id: Some(source_id) }, w, cx),
                    Err(e) => toast::show(cx, Toast::error(format!("Could not open {title}: {e:#}"))),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Scroll the recommendations row by about one screen of cards (`dir` -1 or 1).
    fn scroll_recs(&mut self, dir: f32, cx: &mut Context<Self>) {
        let step = (REC_GRID.card_w + 12.) * 4.;
        let o = self.recs_scroll.offset();
        let max = self.recs_scroll.max_offset().x;
        let x = (o.x - px(step * dir)).min(px(0.)).max(-max);
        self.recs_scroll.set_offset(point(x, o.y));
        cx.notify();
    }

    /// "You may also like": 5-8 titles sharing this one's main tags.
    /// `m` is the open title (the page state is lent out while a list row renders).
    fn recs_section(&self, m: &Manga, cx: &mut Context<Self>) -> Option<Div> {
        let recs = self.recs.as_ref()?;
        let sub = match recs {
            Load::Ready(r) if !r.basis.is_empty() => {
                let from = if m.source_plugin == r.source_id { String::new() } else { format!(" on {}", r.source_name) };
                Some(format!("Tagged {}{from}", r.basis.join(" + ")))
            }
            _ => None,
        };
        let scrollable = matches!(recs, Load::Ready(r) if r.items.len() > 4);
        // Scroll position from the last frame (offset runs from 0 to -max).
        let (off, max) = (-f32::from(self.recs_scroll.offset().x), f32::from(self.recs_scroll.max_offset().x));
        let at_start = off <= 1.;
        let at_end = max > 0. && off >= max - 1.;
        let heading = section_header("You may also like", Some(ph::SPARKLE))
            .mb(px(10.))
            .when_some(sub, |h, s| h.trailing(muted(s).mr(px(8.))))
            .when(scrollable, |h| {
                h.trailing(icon_button("recs-prev", ph::CARET_LEFT, "Scroll back").small().map(|b| if at_start { b.disabled("") } else { b }).on_click(cx.listener(|this, _, _, cx| this.scroll_recs(-1., cx))))
                    .trailing(icon_button("recs-next", ph::CARET_RIGHT, "Scroll for more").small().map(|b| if at_end { b.disabled("") } else { b }).on_click(cx.listener(|this, _, _, cx| this.scroll_recs(1., cx))))
            });
        let body: AnyElement = match recs {
            Load::Loading => div().flex().gap(px(12.)).overflow_hidden().children((0..6).map(|_| skeleton_cover(REC_GRID))).into_any_element(),
            Load::Failed(e) => inline_alert(AlertKind::Warning, "Could not load recommendations", e.clone()).action("Try again", cx.listener(|this, _, _, cx| this.load_recs(cx))).into_any_element(),
            Load::Ready(r) if r.items.is_empty() => {
                muted(if r.basis.is_empty() { "No similar titles: none of this title's tags can be searched." } else { "No similar titles found." }).into_any_element()
            }
            // One horizontal row (scrolls sideways) so the chapters stay close to the top; the
            // right edge fades out and arrows page through it.
            // NN/g carousels: https://www.nngroup.com/articles/designing-effective-carousels/
            Load::Ready(r) => div()
                .relative()
                .child(
                    div().id("recs-row").flex().overflow_x_scroll().track_scroll(&self.recs_scroll).gap(px(12.)).children(r.items.iter().map(|it| {
                        let item = it.clone();
                        let src = r.source_id.clone();
                        let busy = self.rec_busy.as_deref() == Some(it.source_id.as_str());
                        let info = CardInfo { title: it.title.clone().into(), cover: cover_src(&self.rec_covers, it.cover_url.as_deref()), busy, ..Default::default() };
                        manga_card(SharedString::from(format!("rec-{}", it.source_id)), info, REC_GRID).on_click(cx.listener(move |this, _, w, cx| this.open_rec(src.clone(), item.clone(), w, cx)))
                    })),
                )
                // The fade says "more this way"; it goes once the row is scrolled to its end.
                .when(scrollable && !at_end, |d| d.child(div().absolute().top_0().right_0().h_full().w(px(56.)).bg(Tokens::overlay_gradient(90.))))
                .into_any_element(),
        };
        Some(div().flex().flex_col().px(px(size::PAGE_X)).pb(px(18.)).child(heading).child(body))
    }

    fn toggle_favorite(&mut self, cx: &mut Context<Self>) {
        let Load::Ready((m, _)) = &mut self.state else { return };
        m.is_favorite = !m.is_favorite;
        let now = m.is_favorite;
        self.fav_pop += 1;
        // A favourite lives in the library: favouriting a browse-only title adds it.
        if now && !self.in_library {
            self.in_library = true;
            self.lib_pop += 1;
            backend::set_in_library(&self.manga_id, true);
            self.seen_version = backend::data_version();
            toast::show(cx, Toast::success("Added to your library and Favorites").icon(glyphs::FAVORITES));
        }
        self.list.remeasure_items(0..1.min(self.list.item_count()));
        cx.notify();
        let id = self.manga_id.clone();
        cx.spawn(async move |_, _| {
            let _ = mv_ui::bg::blocking(move || backend::with_db(|c| manga::toggle_favorite(c, &id))).await;
            backend::bump_data();
        })
        .detach();
    }

    fn set_library(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.in_library == on {
            return;
        }
        self.in_library = on;
        self.lib_pop += 1;
        backend::set_in_library(&self.manga_id, on);
        self.seen_version = backend::data_version();
        let weak = cx.entity().downgrade();
        let t = if on {
            let t = Toast::success("Added to your library").icon(glyphs::SAVED);
            if self.cats.is_empty() {
                t
            } else {
                let (id, title) = (self.manga_id.clone(), self.title());
                t.action("Set categories", move |w, cx| categories::open_assign(id.clone(), title.clone(), w, cx))
            }
        } else {
            Toast::info("Removed from your library").action("Undo", move |_, cx| {
                let _ = weak.update(cx, |t, cx| t.set_library(true, cx));
            })
        };
        toast::show(cx, t);
        self.list.remeasure_items(0..1.min(self.list.item_count()));
        cx.notify();
    }

    fn title(&self) -> String {
        match &self.state {
            Load::Ready((m, _)) => m.title.clone(),
            _ => String::new(),
        }
    }

    /// Mark chapters read / unread, updating the list immediately. Bulk marks offer Undo, which
    /// puts back each chapter's read flag and saved page.
    /// NN/g, undo over confirmation: https://www.nngroup.com/articles/confirmation-dialog/
    fn set_read(&mut self, ids: Vec<String>, read: bool, cx: &mut Context<Self>) {
        let mut prev: Vec<(String, bool, i64)> = vec![];
        if let Load::Ready((_, cs)) = &mut self.state {
            for c in cs.iter_mut().filter(|c| ids.contains(&c.id)) {
                prev.push((c.id.clone(), c.is_read, c.read_progress));
                c.is_read = read;
                if !read {
                    c.read_progress = 0;
                }
            }
        }
        self.sync_rows();
        if prev.len() > 1 {
            let weak = cx.entity().downgrade();
            let msg = format!("Marked {} as {}", plural(prev.len(), "chapter", "chapters"), if read { "read" } else { "unread" });
            let undo = prev.clone();
            toast::show(
                cx,
                Toast::success(msg).action("Undo", move |_, cx| {
                    let p = undo.clone();
                    let _ = weak.update(cx, |t, cx| t.restore_read(p, cx));
                }),
            );
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let _ = mv_ui::bg::blocking(move || backend::with_db(|c| store::set_read(c, &ids, read))).await;
            let _ = this.update(cx, |this, _| this.seen_version = backend::data_version());
        })
        .detach();
    }

    fn restore_read(&mut self, prev: Vec<(String, bool, i64)>, cx: &mut Context<Self>) {
        if let Load::Ready((_, cs)) = &mut self.state {
            for (id, read, progress) in &prev {
                if let Some(c) = cs.iter_mut().find(|c| &c.id == id) {
                    c.is_read = *read;
                    c.read_progress = *progress;
                }
            }
        }
        self.sync_rows();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let _ = mv_ui::bg::blocking(move || backend::with_db(|c| store::restore_read(c, &prev))).await;
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

    fn jobs(&self, ids: &HashSet<String>) -> Vec<Job> {
        let Load::Ready((m, cs)) = &self.state else { return vec![] };
        cs.iter()
            .filter(|c| ids.contains(&c.id) && c.download_status != "complete")
            .map(|c| Job { manga_id: m.id.clone(), manga_title: m.title.clone(), chapter_id: c.id.clone(), label: logic::chapter_short(c) })
            .collect()
    }

    /// Queue chapters (by id) that are not downloaded yet, in reading order.
    fn download(&mut self, ids: HashSet<String>, cx: &mut Context<Self>) {
        // A chapter whose delete is still waiting out its Undo window has its files: keep them
        // (the same as Undo) instead of downloading it again over a delete that is about to run.
        let kept: Vec<String> = ids.iter().filter(|id| self.pending_delete.remove(*id)).cloned().collect();
        if !kept.is_empty() {
            if let Load::Ready((_, cs)) = &mut self.state {
                for c in cs.iter_mut().filter(|c| kept.contains(&c.id)) {
                    c.download_status = "complete".into();
                }
            }
            self.sync_rows();
            cx.notify();
        }
        let jobs = self.jobs(&ids);
        self.queue.update(cx, |d, cx| d.enqueue(jobs, cx));
    }

    /// Delete a chapter's download (pages and its Studio work) with Undo: the row changes at
    /// once, the files go when the Undo window closes. An Undo that arrives later downloads the
    /// chapter again. Mihon download indicator (delete from a menu, never on the status icon):
    /// https://github.com/mihonapp/mihon/blob/main/app/src/main/java/eu/kanade/presentation/manga/components/ChapterDownloadIndicator.kt
    fn delete_download(&mut self, chapter_id: String, cx: &mut Context<Self>) {
        let Load::Ready((m, cs)) = &mut self.state else { return };
        let Some(c) = cs.iter_mut().find(|c| c.id == chapter_id) else { return };
        c.download_status = "none".into();
        let job = Job { manga_id: m.id.clone(), manga_title: m.title.clone(), chapter_id: c.id.clone(), label: logic::chapter_short(c) };
        self.pending_delete.insert(chapter_id.clone());
        self.sync_rows();
        cx.notify();
        let undone = Rc::new(Cell::new(false));
        let committed = Rc::new(Cell::new(false));
        {
            let (undone, committed, queue, weak) = (undone.clone(), committed.clone(), self.queue.clone(), cx.entity().downgrade());
            let (mid, cid) = (job.manga_id.clone(), job.chapter_id.clone());
            cx.spawn(async move |_, cx| {
                cx.background_executor().timer(UNDO_WINDOW).await;
                // Downloading it again from the page cancels the delete too (the page is gone: go ahead).
                let still_wanted = weak.update(cx, |t, _| t.pending_delete.contains(&cid)).unwrap_or(true);
                if !undone.get() && still_wanted {
                    committed.set(true);
                    let id = cid.clone();
                    queue.update(cx, |d, cx| d.delete(vec![(mid, cid)], false, cx));
                    let _ = weak.update(cx, |t, _| t.pending_delete.remove(&id));
                }
            })
            .detach();
        }
        let (weak, queue) = (cx.entity().downgrade(), self.queue.clone());
        let label = job.label.clone();
        toast::show(
            cx,
            Toast::success(format!("Deleted the download of {label}")).action("Undo", move |_, cx| {
                if committed.get() {
                    // Too late to keep the files: fetch the chapter again instead.
                    let j = job.clone();
                    queue.update(cx, |d, cx| d.enqueue(vec![j], cx));
                    return;
                }
                undone.set(true);
                let id = job.chapter_id.clone();
                let _ = weak.update(cx, |t, cx| {
                    t.pending_delete.remove(&id);
                    if let Load::Ready((_, cs)) = &mut t.state {
                        if let Some(c) = cs.iter_mut().find(|c| c.id == id) {
                            c.download_status = "complete".into();
                        }
                    }
                    t.sync_rows();
                    cx.notify();
                });
            }),
        );
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
                    Some(&i) if i < cs.len() => self.chapter_row(m, cs, i, cx),
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
        let base = Tokens::base();

        let top = div()
            .relative()
            .w_full()
            .overflow_hidden()
            // Edge-to-edge backdrop: the cover stretched across the whole page width, faded into the page.
            .when_some(backdrop, |d, c| d.child(c.into_img().absolute().top_0().left_0().size_full().object_fit(ObjectFit::Cover).opacity(0.3)))
            .child(div().absolute().top_0().left_0().size_full().bg(linear_gradient(90., linear_color_stop(base.alpha(0.8), 0.), linear_color_stop(base.alpha(0.2), 0.8))))
            .child(div().absolute().top_0().left_0().size_full().bg(linear_gradient(180., linear_color_stop(base.alpha(0.25), 0.), linear_color_stop(base, 1.))))
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
                            .rounded(px(radius::LG))
                            .overflow_hidden()
                            .bg(Tokens::surface_2())
                            .border_1()
                            .border_color(Tokens::border())
                            .shadow_lg()
                            .map(|d| match cover {
                                Some(c) => d.child(c.into_img().size_full().object_fit(ObjectFit::Cover)),
                                None => d.flex().items_center().justify_center().child(ph_icon(glyphs::MANGA, Weight::Duotone, Tokens::text_3(), icon_px::EMPTY)),
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
                            .child(div().ramp(ramp::TITLE).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).line_clamp(3).child(m.title.clone()))
                            .when(!people.is_empty(), |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .text_size(px(size::MD))
                                        .text_color(Tokens::text_2())
                                        .child(icon_ph(ph::USER, Tokens::text_3(), icon_px::SM))
                                        .child(people.join(", ")),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(6.))
                                    .text_size(px(size::SM))
                                    .text_color(Tokens::text_3())
                                    .child(icon_ph(status_icon(status.as_deref()), Tokens::text_3(), icon_px::SM))
                                    .child(status.unwrap_or_else(|| "Unknown status".into()))
                                    .child("·")
                                    .child(icon_ph(glyphs::SOURCES, Tokens::text_3(), icon_px::SM))
                                    .child(source)
                                    .when(self.lang != "all", |d| d.child("·").child(icon_ph(glyphs::LANGUAGE, Tokens::text_3(), icon_px::SM)).child(self.lang.to_uppercase())),
                            ),
                    ),
            );

        let cat_label: SharedString = match self.member.len() {
            0 => "Categories".into(),
            1 => self.cats.iter().find(|c| c.id == self.member[0]).map(|c| c.name.clone()).unwrap_or_else(|| "1 category".into()).into(),
            n => format!("{n} categories").into(),
        };
        let (cat_id, cat_title) = (self.manga_id.clone(), m.title.clone());
        let actions = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(size::PAGE_X - 10.))
            .child(
                action_tile("act-lib", glyphs::SAVED, if self.in_library { "In library" } else { "Add to library" }, self.in_library, Tokens::accent(), false, self.lib_pop)
                    .aria_toggled(if self.in_library { Toggled::True } else { Toggled::False })
                    .tooltip(tip(if self.in_library { "Remove from your library" } else { "Add to your library" }))
                    .on_click(cx.listener(|this, _, _, cx| this.set_library(!this.in_library, cx))),
            )
            .child(
                action_tile("act-fav", glyphs::FAVORITES, if m.is_favorite { "Favorited" } else { "Favorite" }, m.is_favorite, Tokens::favorite(), false, self.fav_pop)
                    .aria_toggled(if m.is_favorite { Toggled::True } else { Toggled::False })
                    .tooltip(tip(if m.is_favorite { "Remove from Favorites" } else { "Pin to Favorites in your library" }))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_favorite(cx))),
            )
            .when(self.in_library, |d| {
                d.child(
                    action_tile("act-cat", ph::FOLDER_SIMPLE, cat_label, !self.member.is_empty(), Tokens::accent(), false, 0)
                        .tooltip(tip("Choose the library categories for this title"))
                        .on_click(move |_, w, cx| categories::open_assign(cat_id.clone(), cat_title.clone(), w, cx)),
                )
            })
            .child(
                action_tile("act-refresh", glyphs::REFRESH, if self.refreshing { "Refreshing" } else { "Refresh" }, false, Tokens::accent(), self.refreshing, 0)
                    .tooltip(tip("Check the source for new chapters"))
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(false, cx))),
            )
            .when_some(web, |d, url| {
                let open = url.clone();
                d.child(action_tile("act-web", ph::GLOBE, "Open in browser", false, Tokens::accent(), false, 0).tooltip(tip(url)).on_click(move |_, _, cx| cx.open_url(&open)))
            });

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
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap(px(4.))
                        .max_w(px(860.))
                        .child(div().text_size(px(size::MD)).line_height(px(21.)).text_color(Tokens::text_2()).when(!expanded, |d| d.line_clamp(3)).child(desc))
                        .when(long, |d| {
                            d.child(
                                btn("desc-more", if expanded { "Show less" } else { "Show more" }, ButtonKind::Ghost)
                                    .small()
                                    .trailing_icon(if expanded { ph::CARET_UP } else { ph::CARET_DOWN })
                                    .text_color(Tokens::accent())
                                    .ml(px(-10.))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.expanded = !this.expanded;
                                        this.list.remeasure_items(0..1);
                                        cx.notify();
                                    })),
                            )
                        }),
                )
            })
            .when(!m.genres.is_empty(), |d| {
                d.child(div().flex().flex_wrap().gap(px(6.)).children(m.genres.iter().take(16).enumerate().map(|(i, g)| {
                    let (src, name) = (m.source_plugin.clone(), g.clone());
                    chip(SharedString::from(format!("genre-{i}")), g.clone(), false).tooltip(tip(format!("Browse titles tagged {g}"))).on_click(move |_, w, cx| {
                        discover::request_tag(&src, &name);
                        mv_ui::navigate(Route::Browse, w, cx);
                    })
                })))
            });

        div().flex().flex_col().child(top).child(actions).child(about).children(self.recs_section(m, cx))
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

        let mut stats = vec![plural(cs.len(), "chapter", "chapters")];
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
                .gap(px(6.))
                .child(status_tag(StatusKind::Running).label(format!("Downloading · {pending} left")))
                .child(btn("dl-cancel-title", "Cancel", ButtonKind::Ghost).small().icon(ph::X).on_click(cx.listener(move |this, _, _, cx| this.queue.update(cx, |d, cx| d.cancel_manga(&mid, cx)))))
                .into_any_element()
        } else if missing.is_empty() && !cs.is_empty() {
            status_tag(StatusKind::Done).label("All chapters downloaded").into_any_element()
        } else if missing.is_empty() {
            div().into_any_element()
        } else {
            let all: HashSet<String> = missing.iter().map(|c| c.id.clone()).collect();
            let unread_ids: HashSet<String> = missing.iter().filter(|c| !c.is_read).map(|c| c.id.clone()).collect();
            let n = missing.len();
            let label = if downloaded == 0 { format!("Download all {n}") } else { format!("Download {n} more") };
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .when(missing_unread > 0 && missing_unread < n, |d| {
                    d.child(
                        btn("dl-unread", format!("Unread ({missing_unread})"), ButtonKind::Ghost)
                            .icon(ph::ARROW_LINE_DOWN)
                            .tooltip("Download only the chapters you have not read")
                            .on_click(cx.listener(move |this, _, _, cx| this.download(unread_ids.clone(), cx))),
                    )
                })
                .child(btn("dl-all", label, ButtonKind::Secondary).icon(glyphs::DOWNLOAD).tooltip("Save for offline reading").on_click(cx.listener(move |this, _, _, cx| this.download(all.clone(), cx))))
                .into_any_element()
        };

        let filters = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(48.))
            .border_b_1()
            .border_color(Tokens::border())
            .child(filter_chip("f-unread", "Hide read", v.unread_only).icon(ph::EYE_SLASH).on_click(cx.listener(move |this, _, _, cx| {
                this.set_view(ChapterView { unread_only: !v.unread_only, ..v }, cx)
            })))
            .child(filter_chip("f-dl", "Downloaded", v.downloaded_only).icon(glyphs::DOWNLOAD).on_click(cx.listener(move |this, _, _, cx| {
                this.set_view(ChapterView { downloaded_only: !v.downloaded_only, ..v }, cx)
            })))
            .child(
                btn("sort", if v.newest_first { "Newest first" } else { "Oldest first" }, ButtonKind::Ghost)
                    .small()
                    .icon(if v.newest_first { ph::SORT_DESCENDING } else { ph::SORT_ASCENDING })
                    .tooltip("Reverse the chapter order")
                    .on_click(cx.listener(move |this, _, _, cx| this.set_view(ChapterView { newest_first: !v.newest_first, ..v }, cx))),
            )
            .child(div().flex_1())
            .when(!cs.is_empty(), |d| {
                let ids: Vec<String> = cs.iter().filter(|c| c.is_read == all_read).map(|c| c.id.clone()).collect();
                d.child(
                    btn("mark-all", if all_read { "Mark all unread" } else { "Mark all read" }, ButtonKind::Ghost)
                        .small()
                        .icon(if all_read { ph::ARROW_COUNTER_CLOCKWISE } else { ph::CHECKS })
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
                            .items_center()
                            .gap(px(10.))
                            .child(ph_icon(ph::LIST_NUMBERS, Weight::Regular, Tokens::text_2(), icon_px::LG))
                            .child(div().ramp(ramp::SUBTITLE).text_color(Tokens::text_1()).child("Chapters"))
                            .child(div().truncate().text_size(px(size::SM)).text_color(Tokens::text_3()).child(stats.join("  ·  "))),
                    )
                    .child(download),
            )
            .child(filters)
    }

    fn empty_chapters(&self, cs: &[Chapter], cx: &mut Context<Self>) -> Div {
        if self.refreshing && cs.is_empty() {
            return div().flex().flex_col().pt(px(4.)).children((0..6).map(|_| chapter_skeleton()));
        }
        if cs.is_empty() {
            return div().child(
                empty_view(glyphs::MANGA, "No chapters yet", "The source has not listed any readable chapters.")
                    .action(btn("refresh-empty", "Check again", ButtonKind::Secondary).icon(glyphs::REFRESH).on_click(cx.listener(|this, _, _, cx| this.refresh(false, cx)))),
            );
        }
        let v = self.view;
        div().child(
            empty_view(ph::FUNNEL_X, "No chapters match", "Hide read or Downloaded is hiding every chapter.").action(btn("clear-ch-filters", "Show all chapters", ButtonKind::Secondary).icon(ph::FUNNEL_X).on_click(
                cx.listener(move |this, _, _, cx| this.set_view(ChapterView { unread_only: false, downloaded_only: false, ..v }, cx)),
            )),
        )
    }

    /// The actions a chapter row offers in its menu and on right-click.
    fn row_entries(&self, cs: &[Chapter], i: usize, dl: Option<ChapterDl>, cx: &mut Context<Self>) -> Vec<Entry> {
        let c = &cs[i];
        let weak = cx.entity().downgrade();
        let mut v = vec![];
        {
            let (w, id, read) = (weak.clone(), c.id.clone(), !c.is_read);
            v.push(
                Entry::new(if c.is_read { "Mark as unread" } else { "Mark as read" }, move |_, cx| {
                    let _ = w.update(cx, |t, cx| t.set_read(vec![id.clone()], read, cx));
                })
                .icon(if c.is_read { ph::ARROW_COUNTER_CLOCKWISE } else { ph::CHECK }),
            );
        }
        // Earlier in reading order (the list is ascending), still unread.
        let earlier: Vec<String> = cs[..i].iter().filter(|c| !c.is_read).map(|c| c.id.clone()).collect();
        if !earlier.is_empty() {
            let (w, n) = (weak.clone(), earlier.len());
            v.push(
                Entry::new("Mark previous as read", move |_, cx| {
                    let ids = earlier.clone();
                    let _ = w.update(cx, |t, cx| t.set_read(ids, true, cx));
                })
                .icon(ph::CHECKS)
                .hint(plural(n, "chapter", "chapters")),
            );
        }
        v.push(Entry::Separator);
        let id = c.id.clone();
        let downloaded = c.download_status == "complete";
        match dl {
            Some(_) => {
                let q = self.queue.clone();
                v.push(Entry::new("Cancel download", move |_, cx| q.update(cx, |d, cx| d.cancel_job(&id, cx))).icon(ph::X));
            }
            None if downloaded => {
                let w = weak.clone();
                v.push(
                    Entry::new("Delete download", move |_, cx| {
                        let _ = w.update(cx, |t, cx| t.delete_download(id.clone(), cx));
                    })
                    .icon(glyphs::DELETE)
                    .hint("Pages and this chapter's Studio work")
                    .danger(),
                );
            }
            None => {
                let w = weak.clone();
                v.push(
                    Entry::new("Download", move |_, cx| {
                        let _ = w.update(cx, |t, cx| t.download(HashSet::from([id.clone()]), cx));
                    })
                    .icon(glyphs::DOWNLOAD),
                );
            }
        }
        v
    }

    fn chapter_row(&self, m: &Manga, cs: &[Chapter], i: usize, cx: &mut Context<Self>) -> AnyElement {
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
        let dl = self.queue.read(cx).status(&c.id);
        let rid = c.id.clone();
        let read = c.is_read;
        let label = logic::chapter_label(c);
        let entries = self.row_entries(cs, i, dl, cx);
        let menu_entries = entries.clone();
        div()
            .id(SharedString::from(format!("ch-{}", c.id)))
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(ROW_H))
            .mx(px(size::PAGE_X - 10.))
            .px(px(10.))
            .rounded(px(radius::MD))
            .group("ch-row")
            .role(Role::Link)
            .aria_label(SharedString::from(label.clone()))
            .hover(|s| s.bg(Tokens::hover()))
            .pressable()
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
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .min_w_0()
                            // Read is marked by a check as well as the dimmer title (WCAG 1.4.1).
                            .when(read, |d| d.child(ph_icon(ph::CHECK, Weight::Bold, Tokens::text_3(), icon_px::XS)))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(size::MD))
                                    .font_weight(if read { FontWeight::NORMAL } else { FontWeight::MEDIUM })
                                    .text_color(if read { Tokens::text_3() } else { Tokens::text_1() })
                                    .child(label),
                            ),
                    )
                    .when(!meta.is_empty() || progress.is_some(), |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .text_size(px(size::XS))
                                .text_color(Tokens::text_3())
                                .when(!meta.is_empty(), |d| d.child(div().truncate().child(meta.join("  ·  "))))
                                .when_some(progress, |d, p| d.when(!meta.is_empty(), |d| d.child("·")).child(div().flex_none().text_color(Tokens::accent()).child(p))),
                        )
                    }),
            )
            // Row controls appear with the pointer, and on their own when Tab reaches them (the
            // opacity sits on each control so keyboard focus can lift it; an open menu keeps
            // its trigger shown). WCAG 2.4.7: https://www.w3.org/WAI/WCAG22/Understanding/focus-visible.html
            .child(
                icon_button(SharedString::from(format!("mr-{}", c.id)), if read { ph::ARROW_COUNTER_CLOCKWISE } else { ph::CHECK }, if read { "Mark as unread" } else { "Mark as read" })
                    .small()
                    .opacity(0.)
                    .group_hover("ch-row", |s| s.opacity(1.))
                    .in_focus(|s| s.opacity(1.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.set_read(vec![rid.clone()], !read, cx);
                    })),
            )
            .child(
                icon_trigger(SharedString::from(format!("chm-{}", c.id)), icon_ph(ph::DOTS_THREE_VERTICAL, Tokens::text_2(), icon_px::MD), "More actions")
                    .opacity(0.)
                    .group_hover("ch-row", |s| s.opacity(1.))
                    .in_focus(|s| s.opacity(1.))
                    .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| fill_menu(menu.min_w(px(240.)), &menu_entries)),
            )
            .child(self.download_control(c, dl, cx))
            .context_menu(move |menu, _, _| fill_menu(menu.min_w(px(240.)), &entries))
            .into_any_element()
    }

    /// Per-chapter download state at the end of a row (Mihon's indicator): download, a flat ring
    /// while queued, a filling ring while downloading, a filled check when saved and a red alert
    /// when it failed. The saved check opens a menu with Delete; it never deletes by itself.
    fn download_control(&self, c: &Chapter, dl: Option<ChapterDl>, cx: &mut Context<Self>) -> AnyElement {
        let id = SharedString::from(format!("dlc-{}", c.id));
        let cid = c.id.clone();
        let q = self.queue.clone();
        let cancel = move |label: &'static str| {
            let (q, cid) = (q.clone(), cid.clone());
            vec![Entry::new(label, move |_, cx| q.update(cx, |d, cx| d.cancel_job(&cid, cx))).icon(ph::X)]
        };
        let error = self.queue.read(cx).error(&c.id).map(str::to_string);
        match dl {
            Some(ChapterDl::Active(f)) => {
                let entries = cancel("Cancel download");
                icon_trigger(id, progress_ring_sized(f, 18.), format!("Downloading · {}%", (f * 100.).round()))
                    .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| fill_menu(menu.min_w(px(200.)), &entries))
                    .into_any_element()
            }
            Some(ChapterDl::Queued) => {
                let entries = cancel("Remove from queue");
                icon_trigger(id, progress_ring_sized(0., 18.), "Queued for download")
                    .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| fill_menu(menu.min_w(px(200.)), &entries))
                    .into_any_element()
            }
            None if c.download_status == "complete" => {
                let w = cx.entity().downgrade();
                let did = c.id.clone();
                let entries = vec![
                    Entry::Heading("Saved for offline reading".into()),
                    Entry::new("Delete download", move |_, cx| {
                        let _ = w.update(cx, |t, cx| t.delete_download(did.clone(), cx));
                    })
                    .icon(glyphs::DELETE)
                    .hint("Pages and this chapter's Studio work")
                    .danger(),
                ];
                icon_trigger(id, ph_icon(ph::CHECK_CIRCLE, Weight::Fill, Tokens::accent(), icon_px::LG), "Downloaded")
                    .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| fill_menu(menu.min_w(px(220.)), &entries))
                    .into_any_element()
            }
            None => {
                let did = c.id.clone();
                let (icon, tip_text, color) = match &error {
                    Some(e) => (ph::WARNING_CIRCLE, format!("Download failed: {e}. Click to try again."), Tokens::danger()),
                    None => (ph::ARROW_LINE_DOWN, "Download chapter".to_string(), Tokens::text_3()),
                };
                icon_button(id, icon, tip_text)
                    .color(color)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.download(HashSet::from([did.clone()]), cx);
                    }))
                    .into_any_element()
            }
        }
    }

    /// Floating Resume / Start button (always reachable while scrolling the chapter list).
    fn fab(&self, m: &Manga, cs: &[Chapter]) -> Option<impl IntoElement> {
        let (i, resume) = logic::continue_target(cs)?;
        let route = Route::Reader { manga_id: m.id.clone(), chapter_id: cs[i].id.clone() };
        let label = format!("{} {}", if resume { "Resume" } else { "Start" }, logic::chapter_short(&cs[i]));
        Some(
            btn("fab", label, ButtonKind::Primary)
                .large()
                .icon(ph::PLAY.fill())
                .absolute()
                .bottom(px(24.))
                .right(px(size::PAGE_X))
                .rounded(px(radius::PILL))
                .shadow_lg()
                .on_click(move |_, w, cx| mv_ui::navigate(route.clone(), w, cx)),
        )
    }
}

/// A chapter row while chapters are being fetched.
fn chapter_skeleton() -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .h(px(ROW_H))
        .mx(px(size::PAGE_X - 10.))
        .px(px(10.))
        .child(div().w(px(3.)))
        .child(div().flex().flex_col().gap(px(8.)).flex_1().child(skeleton_line(px(180.))).child(skeleton_line(px(110.)).h(px(10.))))
        .child(skeleton(px(20.), px(20.), radius::PILL))
}

/// The page while the title loads, shaped like the real header, actions and chapter rows so
/// nothing jumps when the data arrives. NN/g skeleton screens: https://www.nngroup.com/articles/skeleton-screens/
fn page_skeleton() -> Div {
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_end()
                .gap(px(28.))
                .px(px(size::PAGE_X))
                .pt(px(20.))
                .pb(px(22.))
                .child(skeleton(px(168.), px(252.), radius::LG))
                .child(div().flex().flex_col().gap(px(12.)).flex_1().child(skeleton_line(px(360.)).h(px(28.))).child(skeleton_line(px(200.))).child(skeleton_line(px(260.)))),
        )
        .child(div().flex().gap(px(4.)).px(px(size::PAGE_X - 10.)).children((0..4).map(|_| skeleton(px(88.), px(60.), radius::LG))))
        .child(div().px(px(size::PAGE_X)).pt(px(16.)).pb(px(12.)).child(mv_ui::skeleton_text(3).max_w(px(860.))))
        .children((0..6).map(|_| chapter_skeleton()))
}

fn status_icon(status: Option<&str>) -> mv_ui::Ph {
    let s = status.unwrap_or("").to_lowercase();
    if s.contains("complete") || s.contains("finished") {
        ph::CHECK_CIRCLE
    } else if s.contains("hiatus") || s.contains("cancel") {
        ph::PAUSE_CIRCLE
    } else {
        ph::CLOCK
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
            Load::Loading => root.child(page_skeleton()),
            Load::Failed(e) => root.child(error_view(e.clone()).title("Could not open this title").retry(cx.listener(|this, _, _, cx| {
                this.state = Load::Loading;
                this.reload(cx);
                cx.notify();
            }))),
            Load::Ready((m, cs)) => {
                let fab = self.fab(m, cs);
                root.child(list(self.list.clone(), cx.processor(|this: &mut Self, ix: usize, _w, cx| this.render_item(ix, cx))).size_full()).children(fab)
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

/// Back control on a glass pill so it reads over the cover backdrop.
fn back_pill(label: SharedString) -> impl IntoElement {
    btn("detail-back", label, ButtonKind::Ghost)
        .small()
        .icon(glyphs::BACK)
        .kbd("alt-left")
        .bg(Tokens::glass())
        .text_color(Tokens::text_1())
        .on_click(|_, w, cx| mv_ui::navigate(Route::Back, w, cx))
}
