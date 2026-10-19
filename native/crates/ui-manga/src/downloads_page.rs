//! Downloads: titles with chapters saved for offline reading, plus the live download queue.
use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::chapter::Chapter;
use mv_core::repo::manga::Manga;
use mv_ui::theme::{icon as icon_px, radius};
use mv_ui::toast::{self, Toast};
use mv_ui::{
    btn, confirm_dialog, disclosure, empty_view, error_view, fmt_bytes, glyphs, icon_button, muted, page_title, ph, progress_bar, progress_ring_sized, size, skeleton, skeleton_line, status_tag,
    ButtonKind, CoverSource, Interactive, Route, StatusKind, Tokens,
};

use crate::backend;
use crate::downloads::{downloads, ChapterDl, Downloads, Job};
use crate::logic;
use crate::store;
use crate::ui::{search_box, spawn_cover_downloads, thumb, Load};

struct Title {
    manga: Manga,
    chapters: Vec<Chapter>,
    bytes: u64,
}

pub struct DownloadsPage {
    state: Load<Vec<Title>>,
    covers: HashMap<String, PathBuf>,
    scroll: ScrollHandle,
    query: String,
    input: Entity<InputState>,
    seen_version: u64,
    queue: Entity<Downloads>,
    _subs: Vec<Subscription>,
}

type Loaded = (Vec<Title>, HashMap<String, PathBuf>, Vec<(String, Option<String>)>);

fn dir_size(dir: &str) -> u64 {
    std::fs::read_dir(dir).map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()).unwrap_or(0)
}

fn load() -> anyhow::Result<Loaded> {
    let items = backend::with_db(|c| store::downloaded_titles(c))?;
    let titles: Vec<Title> = items
        .into_iter()
        .map(|(manga, chapters)| {
            let bytes = chapters.iter().filter_map(|c| c.pages_dir.as_deref()).map(dir_size).sum();
            Title { manga, chapters, bytes }
        })
        .collect();
    let (have, missing) = backend::resolve_covers(titles.iter().filter_map(|t| t.manga.cover_path.clone().map(|u| (u, Some(t.manga.source_plugin.clone())))));
    Ok((titles, have, missing))
}

fn chapters_word(n: usize) -> String {
    format!("{n} chapter{}", if n == 1 { "" } else { "s" })
}

impl DownloadsPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search downloads"));
        let sub = cx.subscribe_in(&input, window, |this, input, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::Change) {
                this.query = input.read(cx).value().to_string();
                cx.notify();
            }
        });
        let queue = downloads(cx);
        let watch = cx.observe(&queue, |_, _, cx| cx.notify());
        let mut p = Self {
            state: Load::Loading,
            covers: HashMap::new(),
            scroll: ScrollHandle::new(),
            query: String::new(),
            input,
            seen_version: backend::data_version(),
            queue,
            _subs: vec![sub, watch],
        };
        p.reload(cx);
        p
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(load).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok((items, have, missing)) => {
                        this.covers.extend(have);
                        this.state = Load::Ready(items);
                        spawn_cover_downloads(cx, missing, |t: &mut Self| &mut t.covers);
                    }
                    Err(e) => this.state = Load::Failed(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Read the first unread downloaded chapter (or resume one in progress).
    fn read(&self, manga_id: &str, window: &mut Window, cx: &mut App) {
        let Load::Ready(items) = &self.state else { return };
        let Some(t) = items.iter().find(|t| t.manga.id == manga_id) else { return };
        if let Some((i, _)) = logic::continue_target(&t.chapters) {
            mv_ui::navigate(Route::Reader { manga_id: t.manga.id.clone(), chapter_id: t.chapters[i].id.clone() }, window, cx);
        }
    }

    /// Deleting a title's downloads also removes its Studio work, which cannot be recreated
    /// quickly, so it asks first and says exactly what goes.
    /// NN/g confirmation dialogs: https://www.nngroup.com/articles/confirmation-dialog/
    fn confirm_delete(&mut self, manga_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Load::Ready(items) = &self.state else { return };
        let Some(t) = items.iter().find(|t| t.manga.id == manga_id) else { return };
        let title = t.manga.title.clone();
        let body = format!(
            "{} · {}. The pages and the Studio work made for this title (characters, voices, scripts and recaps) are deleted. This cannot be undone.",
            chapters_word(t.chapters.len()),
            fmt_bytes(t.bytes)
        );
        let weak = cx.entity().downgrade();
        confirm_dialog(format!("Delete downloads for {title}?"), body, "Delete downloads", true)
            .icon(glyphs::DELETE)
            .cancel_label("Keep")
            .on_confirm(move |_, cx| {
                let id = manga_id.clone();
                let _ = weak.update(cx, |this, cx| this.delete(id, cx));
                toast::show(cx, Toast::success(format!("Deleted the downloads of {title}")));
            })
            .open(window, cx);
    }

    fn delete(&mut self, manga_id: String, cx: &mut Context<Self>) {
        let Load::Ready(items) = &mut self.state else { return };
        let Some(pos) = items.iter().position(|t| t.manga.id == manga_id) else { return };
        let t = items.remove(pos);
        let list = t.chapters.iter().map(|c| (manga_id.clone(), c.id.clone())).collect();
        self.queue.update(cx, |d, cx| d.delete(list, true, cx));
        cx.notify();
    }

    fn queue_row(&self, i: usize, job: &Job, st: ChapterDl) -> Stateful<Div> {
        let q = self.queue.clone();
        let cid = job.chapter_id.clone();
        let (frac, word) = match st {
            ChapterDl::Active(f) => (f, format!("{}%", (f * 100.).round())),
            ChapterDl::Queued => (0., "Waiting".to_string()),
        };
        div()
            .id(("q-row", i))
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(36.))
            .px(px(8.))
            .rounded(px(radius::MD))
            .hover(|s| s.bg(Tokens::hover()))
            .child(progress_ring_sized(frac, 16.))
            .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).text_color(Tokens::text_1()).child(format!("{} · {}", job.manga_title, job.label)))
            .child(div().flex_none().text_size(px(size::XS)).text_color(Tokens::text_3()).child(word))
            .child(icon_button(("q-cancel", i), ph::X, "Remove from queue").small().on_click(move |_, _, cx| q.update(cx, |d, cx| d.cancel_job(&cid, cx))))
    }

    fn queue_section(&self, cx: &mut Context<Self>) -> Option<Div> {
        let d = self.queue.read(cx);
        let pending = d.pending();
        if pending.is_empty() {
            return None;
        }
        let paused = d.is_paused();
        let (job, st) = pending[0].clone();
        let frac = match st {
            ChapterDl::Active(f) => f,
            ChapterDl::Queued => 0.,
        };
        let waiting = pending.len() - 1;
        let (q1, q2) = (self.queue.clone(), self.queue.clone());
        let status = match (st, paused) {
            (ChapterDl::Active(_), true) => status_tag(StatusKind::Running).label("Pausing after this chapter"),
            (ChapterDl::Active(_), false) => status_tag(StatusKind::Running).label(format!("{}%", (frac * 100.).round())),
            (ChapterDl::Queued, true) => status_tag(StatusKind::Queued).label("Paused"),
            (ChapterDl::Queued, false) => status_tag(StatusKind::Queued),
        };
        let rest: Vec<Stateful<Div>> = pending.iter().skip(1).enumerate().map(|(i, (j, s))| self.queue_row(i, j, *s)).collect();
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .mb(px(22.))
                .p(px(14.))
                .rounded(px(radius::LG))
                .bg(Tokens::surface())
                .border_1()
                .border_color(Tokens::border())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(progress_ring_sized(frac, icon_px::XL))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(div().truncate().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("Downloading {}", job.manga_title)))
                                .child(muted(if waiting > 0 { format!("{} · {waiting} more in the queue", job.label) } else { job.label.clone() })),
                        )
                        .child(status)
                        .child(
                            btn("dl-pause", if paused { "Resume" } else { "Pause" }, ButtonKind::Ghost)
                                .small()
                                .icon(if paused { ph::PLAY } else { ph::PAUSE })
                                .tooltip(if paused { "Continue with the queue" } else { "Finish this chapter, then wait" })
                                .on_click(move |_, _, cx| q1.update(cx, |d, cx| d.set_paused(!paused, cx))),
                        )
                        .child(btn("dl-cancel-all", "Cancel all", ButtonKind::Ghost).small().icon(ph::X_CIRCLE).on_click(move |_, _, cx| q2.update(cx, |d, cx| d.cancel_all(cx)))),
                )
                .child(progress_bar("dl-queue-bar", frac, !paused))
                .when(waiting > 0, |d| d.child(disclosure("dl-queue-list", format!("Up next ({waiting})")).content(div().flex().flex_col().children(rest)))),
        )
    }

    fn row(&self, t: &Title, cx: &mut Context<Self>) -> Stateful<Div> {
        let m = &t.manga;
        let detail = Route::MangaDetail { manga_id: m.id.clone(), source_id: Some(m.source_plugin.clone()) };
        let cover = m.cover_path.as_deref().and_then(|u| self.covers.get(u)).cloned().map(CoverSource::File);
        let n = t.chapters.len();
        let unread = t.chapters.iter().filter(|c| !c.is_read).count();
        let range = match (t.chapters.first().and_then(|c| c.number), t.chapters.last().and_then(|c| c.number)) {
            (Some(a), Some(b)) if n > 1 => format!("Ch. {}–{}", logic::fmt_number(a), logic::fmt_number(b)),
            (Some(a), _) => format!("Ch. {}", logic::fmt_number(a)),
            _ => String::new(),
        };
        let pending = self.queue.read(cx).pending_for(&m.id);
        let (id, id2, id3) = (m.id.clone(), m.id.clone(), m.id.clone());
        let label = if let Some((i, resume)) = logic::continue_target(&t.chapters) {
            format!("{} {}", if resume { "Resume" } else { "Read" }, logic::chapter_short(&t.chapters[i]))
        } else {
            "Read".into()
        };
        // The whole row reads; the chapter list and delete are explicit buttons.
        div()
            .id(SharedString::from(format!("dl-{}", m.id)))
            .flex()
            .items_center()
            .gap(px(14.))
            .px(px(10.))
            .py(px(10.))
            .rounded(px(radius::LG))
            .role(Role::Link)
            .aria_label(SharedString::from(format!("{label}: {}", m.title)))
            .hover(|s| s.bg(Tokens::hover()))
            .pressable()
            .on_click(cx.listener(move |this, _, w, cx| this.read(&id, w, cx)))
            .child(thumb(cover, 52., 78.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).truncate().child(m.title.clone()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(size::SM))
                            .text_color(Tokens::text_3())
                            .child(chapters_word(n))
                            .when(!range.is_empty(), |d| d.child("·").child(range))
                            .child("·")
                            .child(fmt_bytes(t.bytes))
                            .when(unread > 0, |d| d.child("·").child(div().text_color(Tokens::accent()).child(format!("{unread} unread"))))
                            .when(pending > 0, |d| d.child("·").child(div().text_color(Tokens::accent()).child(format!("{pending} downloading")))),
                    ),
            )
            .child(btn(SharedString::from(format!("dlr-{}", m.id)), label, ButtonKind::Secondary).icon(ph::PLAY).on_click(cx.listener(move |this, _, w, cx| {
                cx.stop_propagation();
                this.read(&id2, w, cx);
            })))
            .child(icon_button(SharedString::from(format!("dlo-{}", m.id)), ph::LIST_NUMBERS, "All chapters").on_click(move |_, w, cx| {
                cx.stop_propagation();
                mv_ui::navigate(detail.clone(), w, cx);
            }))
            .child(icon_button(SharedString::from(format!("dld-{}", m.id)), glyphs::DELETE, "Delete downloads").danger().on_click(cx.listener(move |this, _, w, cx| {
                cx.stop_propagation();
                this.confirm_delete(id3.clone(), w, cx);
            })))
    }
}

fn row_skeleton() -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(14.))
        .px(px(10.))
        .py(px(10.))
        .child(skeleton(px(52.), px(78.), radius::SM))
        .child(div().flex().flex_col().gap(px(8.)).flex_1().child(skeleton_line(px(240.))).child(skeleton_line(px(150.)).h(px(10.))))
}

impl Render for DownloadsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let v = backend::data_version();
        if v != self.seen_version {
            self.seen_version = v;
            self.reload(cx);
        }
        let (count, total_bytes) = match &self.state {
            Load::Ready(i) if !i.is_empty() => (i.iter().map(|t| t.chapters.len()).sum::<usize>(), i.iter().map(|t| t.bytes).sum::<u64>()),
            _ => (0, 0),
        };
        let has_items = count > 0;
        let header = page_title("Downloads")
            .icon(glyphs::DOWNLOADS)
            .when(has_items, |h| h.subtitle(format!("{} · {}", chapters_word(count), fmt_bytes(total_bytes))).action(search_box(&self.input)));
        let body: AnyElement = match &self.state {
            Load::Loading => div().flex().flex_col().gap(px(4.)).children((0..5).map(|_| row_skeleton())).into_any_element(),
            Load::Failed(e) => error_view(e.clone())
                .title("Could not list your downloads")
                .retry(cx.listener(|this, _, _, cx| {
                    this.state = Load::Loading;
                    this.reload(cx);
                    cx.notify();
                }))
                .into_any_element(),
            Load::Ready(items) if items.is_empty() => empty_view(glyphs::DOWNLOADS, "No downloads yet", "Open a title and choose Download to keep its chapters for offline reading.")
                .action(btn("dl-browse", "Go to Library", ButtonKind::Primary).icon(glyphs::LIBRARY).on_click(|_, w, cx| mv_ui::navigate(Route::Library, w, cx)))
                .into_any_element(),
            Load::Ready(items) => {
                let q = self.query.trim().to_lowercase();
                let shown: Vec<&Title> = items.iter().filter(|t| q.is_empty() || t.manga.title.to_lowercase().contains(&q)).collect();
                if shown.is_empty() {
                    empty_view(glyphs::SEARCH, "No matches", "No downloaded title matches this search.")
                        .action(btn("clear-search", "Clear search", ButtonKind::Secondary).icon(ph::X).on_click(cx.listener(|this, _, w, cx| {
                            this.query.clear();
                            this.input.update(cx, |s, cx| s.set_value("", w, cx));
                            cx.notify();
                        })))
                        .into_any_element()
                } else {
                    div().flex().flex_col().children(shown.into_iter().map(|t| self.row(t, cx))).into_any_element()
                }
            }
        };
        div()
            .id("downloads-page")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                div()
                    .w_full()
                    .max_w(px(1040.))
                    .px(px(size::PAGE_X))
                    .py(px(size::PAGE_Y))
                    .child(header)
                    .children(self.queue_section(cx))
                    .child(body)
                    .when(has_items, |d| d.child(div().mt(px(16.)).px(px(10.)).child(muted("Downloaded chapters open without a connection. Click a title to keep reading.")))),
            )
    }
}
