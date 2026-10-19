//! Downloads: titles with chapters saved for offline reading, plus the live download queue.
use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::chapter::Chapter;
use mv_core::repo::manga::Manga;
use mv_ui::{button, icon, page_header, size, ButtonKind, CoverSource, Route, Tokens};

use crate::backend;
use crate::downloads::{downloads, ChapterDl, Downloads};
use crate::logic;
use crate::store;
use crate::ui::{error_view, icon_tip, muted, search_box, spawn_cover_downloads, state_view, Load};

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
    /// Title whose Delete was clicked once (a second click deletes).
    confirm_delete: Option<String>,
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

fn fmt_bytes(b: u64) -> String {
    let mb = b as f64 / (1024. * 1024.);
    if mb >= 1024. {
        format!("{:.1} GB", mb / 1024.)
    } else {
        format!("{mb:.0} MB")
    }
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
            confirm_delete: None,
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
    fn read(&self, t: &Title, window: &mut Window, cx: &mut App) {
        if let Some((i, _)) = logic::continue_target(&t.chapters) {
            mv_ui::navigate(Route::Reader { manga_id: t.manga.id.clone(), chapter_id: t.chapters[i].id.clone() }, window, cx);
        }
    }

    fn delete(&mut self, manga_id: String, cx: &mut Context<Self>) {
        if self.confirm_delete.as_deref() != Some(manga_id.as_str()) {
            self.confirm_delete = Some(manga_id);
            cx.notify();
            return;
        }
        self.confirm_delete = None;
        let Load::Ready(items) = &mut self.state else { return };
        let Some(pos) = items.iter().position(|t| t.manga.id == manga_id) else { return };
        let t = items.remove(pos);
        let list = t.chapters.iter().map(|c| (manga_id.clone(), c.id.clone())).collect();
        self.queue.update(cx, |d, cx| d.delete(list, true, cx));
        cx.notify();
    }

    fn queue_section(&self, cx: &mut Context<Self>) -> Option<Div> {
        let pending = self.queue.read(cx).pending();
        if pending.is_empty() {
            return None;
        }
        let (job, st) = pending[0].clone();
        let frac = match st {
            ChapterDl::Active(f) => f,
            ChapterDl::Queued => 0.,
        };
        let waiting = pending.len() - 1;
        let q = self.queue.clone();
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .mb(px(22.))
                .p(px(14.))
                .rounded(px(12.))
                .bg(Tokens::surface())
                .border_1()
                .border_color(Tokens::border())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(icon("icons/download.svg", Tokens::accent(), 18.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(div().truncate().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("Downloading {}", job.manga_title)))
                                .child(muted(if waiting > 0 { format!("{} · {}% · {waiting} more in queue", job.label, (frac * 100.).round()) } else { format!("{} · {}%", job.label, (frac * 100.).round()) })),
                        )
                        .child(button("dl-cancel-all", "Cancel all", ButtonKind::Ghost).on_click(move |_, _, cx| q.update(cx, |d, cx| d.cancel_all(cx)))),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(4.))
                        .rounded(px(2.))
                        .bg(Tokens::active())
                        .child(div().h_full().rounded(px(2.)).bg(Tokens::accent()).w(relative(frac.clamp(0., 1.)))),
                ),
        )
    }

    fn row(&self, t: &Title, cx: &mut Context<Self>) -> Stateful<Div> {
        let m = &t.manga;
        let detail = Route::MangaDetail { manga_id: m.id.clone(), source_id: Some(m.source_plugin.clone()) };
        let detail2 = detail.clone();
        let cover = m.cover_path.as_deref().and_then(|u| self.covers.get(u)).cloned().map(CoverSource::File);
        let n = t.chapters.len();
        let unread = t.chapters.iter().filter(|c| !c.is_read).count();
        let range = match (t.chapters.first().and_then(|c| c.number), t.chapters.last().and_then(|c| c.number)) {
            (Some(a), Some(b)) if n > 1 => format!("Ch. {}–{}", logic::fmt_number(a), logic::fmt_number(b)),
            (Some(a), _) => format!("Ch. {}", logic::fmt_number(a)),
            _ => String::new(),
        };
        let pending = self.queue.read(cx).pending_for(&m.id);
        let mid = m.id.clone();
        let confirming = self.confirm_delete.as_deref() == Some(m.id.as_str());
        let id = m.id.clone();
        let label = if let Some((i, resume)) = logic::continue_target(&t.chapters) {
            format!("{} {}", if resume { "Resume" } else { "Read" }, logic::chapter_short(&t.chapters[i]))
        } else {
            "Read".into()
        };
        div()
            .id(SharedString::from(format!("dl-{}", m.id)))
            .flex()
            .items_center()
            .gap(px(14.))
            .px(px(10.))
            .py(px(10.))
            .rounded(px(10.))
            .cursor_pointer()
            .hover(|s| s.bg(Tokens::hover()))
            .on_click(cx.listener(move |this, _, w, cx| {
                if let Load::Ready(items) = &this.state {
                    if let Some(t) = items.iter().find(|t| t.manga.id == id) {
                        this.read(t, w, cx);
                    }
                }
            }))
            .child(
                div()
                    .id(SharedString::from(format!("dlc-{}", m.id)))
                    .w(px(52.))
                    .h(px(78.))
                    .flex_none()
                    .rounded(px(6.))
                    .overflow_hidden()
                    .bg(Tokens::surface_2())
                    .border_1()
                    .border_color(Tokens::border())
                    .hover(|s| s.border_color(rgba(0xffffff40)))
                    .map(|d| match cover {
                        Some(c) => d.child(c.into_img().size_full().object_fit(ObjectFit::Cover)),
                        None => d.flex().items_center().justify_center().child(icon("icons/book-open.svg", rgba(0xffffff1f), 18.)),
                    })
                    .on_click(move |_, w, cx| {
                        cx.stop_propagation();
                        mv_ui::navigate(detail.clone(), w, cx);
                    }),
            )
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
                            .child(format!("{n} chapter{}", if n == 1 { "" } else { "s" }))
                            .when(!range.is_empty(), |d| d.child("·").child(range))
                            .child("·")
                            .child(fmt_bytes(t.bytes))
                            .when(unread > 0, |d| d.child("·").child(div().text_color(Tokens::accent()).child(format!("{unread} unread"))))
                            .when(pending > 0, |d| d.child("·").child(div().text_color(Tokens::accent()).child(format!("{pending} downloading")))),
                    ),
            )
            .child(
                button(SharedString::from(format!("dlr-{}", m.id)), label, ButtonKind::Secondary)
                    .child(icon("icons/play.svg", Tokens::text_1(), 13.))
                    .flex_row_reverse(),
            )
            .child(icon_tip(SharedString::from(format!("dlo-{}", m.id)), "icons/list.svg", "All chapters").on_click(move |_, w, cx| {
                cx.stop_propagation();
                mv_ui::navigate(detail2.clone(), w, cx);
            }))
            .child(
                div()
                    .id(SharedString::from(format!("dld-{}", m.id)))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .h(px(32.))
                    .min_w(px(32.))
                    .px(px(if confirming { 10. } else { 0. }))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_size(px(size::SM))
                    .text_color(Tokens::danger())
                    .when(confirming, |d| d.bg(rgba(0xf8717124)))
                    .hover(|s| s.bg(Tokens::hover()))
                    .child(icon("icons/trash.svg", if confirming { Tokens::danger() } else { Tokens::text_2() }, 16.))
                    .when(confirming, |d| d.child("Delete pages + Studio work?"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.delete(mid.clone(), cx);
                    })),
            )
    }
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
        let subtitle = has_items.then(|| SharedString::from(format!("{count} chapters · {}", fmt_bytes(total_bytes))));
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(div().flex_1().min_w_0().child(page_header("Downloads", subtitle)))
            .when(has_items, |d| d.child(div().mb(px(20.)).child(search_box(&self.input))));
        let body: AnyElement = match &self.state {
            Load::Loading => div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .children((0..5).map(|_| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(14.))
                        .px(px(10.))
                        .child(div().w(px(52.)).h(px(78.)).rounded(px(6.)).bg(Tokens::surface_2()))
                        .child(div().flex().flex_col().gap(px(6.)).child(div().w(px(240.)).h(px(12.)).rounded(px(4.)).bg(Tokens::surface_2())).child(div().w(px(150.)).h(px(10.)).rounded(px(4.)).bg(Tokens::surface_2())))
                }))
                .into_any_element(),
            Load::Failed(e) => error_view(e.clone()).into_any_element(),
            Load::Ready(items) if items.is_empty() => div()
                .flex()
                .flex_col()
                .items_center()
                .child(state_view("icons/download.svg", "No downloads yet", "Open a title and choose Download to keep its chapters for offline reading."))
                .child(
                    button("dl-browse", "Go to Library", ButtonKind::Primary)
                        .child(icon("icons/book-open.svg", Tokens::base(), 14.))
                        .on_click(|_, w, cx| mv_ui::navigate(Route::Library, w, cx)),
                )
                .into_any_element(),
            Load::Ready(items) => {
                let q = self.query.trim().to_lowercase();
                let shown: Vec<&Title> = items.iter().filter(|t| q.is_empty() || t.manga.title.to_lowercase().contains(&q)).collect();
                if shown.is_empty() {
                    state_view("icons/search.svg", "No matches", "No downloaded title matches this search.").into_any_element()
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
