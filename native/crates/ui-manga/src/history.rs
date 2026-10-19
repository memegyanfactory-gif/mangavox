use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::history::{self, ReadingHistory};
use mv_ui::theme::radius;
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, confirm_dialog, empty_view, error_view, glyphs, icon_button, muted, page_title, ph, size, skeleton, skeleton_line, ButtonKind, CoverSource, Interactive, Route, Tokens};

use crate::backend;
use crate::logic;
use crate::ui::{search_box, spawn_cover_downloads, thumb, Load};

/// A removed entry stays in the database until its Undo toast has had its time.
const UNDO_WINDOW: Duration = Duration::from_millis(6_500);

pub struct HistoryPage {
    state: Load<Vec<ReadingHistory>>,
    covers: HashMap<String, PathBuf>,
    scroll: ScrollHandle,
    query: String,
    input: Entity<InputState>,
    /// Entries removed in the UI (manga ids) whose delete waits for the Undo window.
    hidden: HashSet<String>,
    seen_version: u64,
    _sub: Subscription,
}

type Loaded = (Vec<ReadingHistory>, HashMap<String, PathBuf>, Vec<(String, Option<String>)>);

fn load() -> anyhow::Result<Loaded> {
    let items = backend::with_db(|c| history::list(c, 200))?;
    let (have, missing) = backend::resolve_covers(items.iter().filter_map(|h| h.cover_url.clone().map(|u| (u, Some(h.source_plugin.clone())))));
    Ok((items, have, missing))
}

impl HistoryPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search history"));
        let sub = cx.subscribe_in(&input, window, |this, input, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::Change) {
                this.query = input.read(cx).value().to_string();
                cx.notify();
            }
        });
        let mut p = Self {
            state: Load::Loading,
            covers: HashMap::new(),
            scroll: ScrollHandle::new(),
            query: String::new(),
            input,
            hidden: HashSet::new(),
            seen_version: backend::data_version(),
            _sub: sub,
        };
        p.reload(cx);
        p
    }

    /// Re-read history; the current list stays visible until the new one arrives.
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

    /// Hide the entry now and delete it after the Undo window. An Undo that comes later puts it
    /// back at its chapter and page (as just read).
    /// NN/g, undo for routine removals: https://www.nngroup.com/articles/confirmation-dialog/
    fn remove(&mut self, h: &ReadingHistory, cx: &mut Context<Self>) {
        let mid = h.manga_id.clone();
        self.hidden.insert(mid.clone());
        cx.notify();
        let undone = Rc::new(Cell::new(false));
        let committed = Rc::new(Cell::new(false));
        {
            let (undone, committed, mid) = (undone.clone(), committed.clone(), mid.clone());
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(UNDO_WINDOW).await;
                if undone.get() {
                    return;
                }
                committed.set(true);
                let id = mid.clone();
                let _ = mv_ui::bg::blocking(move || backend::with_db(|c| history::remove(c, &id))).await;
                let _ = this.update(cx, |t, _| {
                    t.hidden.remove(&mid);
                    if let Load::Ready(items) = &mut t.state {
                        items.retain(|h| h.manga_id != mid);
                    }
                });
            })
            .detach();
        }
        let weak = cx.entity().downgrade();
        let (chapter, page) = (h.chapter_id.clone(), h.page);
        toast::show(
            cx,
            Toast::success(format!("Removed {} from history", h.manga_title)).action("Undo", move |_, cx| {
                undone.set(true);
                let (m, c, weak) = (mid.clone(), chapter.clone(), weak.clone());
                if !committed.get() {
                    let _ = weak.update(cx, |t, cx| {
                        t.hidden.remove(&m);
                        cx.notify();
                    });
                    return;
                }
                // Already deleted: write the entry back, then re-read the list.
                cx.spawn(async move |cx| {
                    let _ = mv_ui::bg::blocking(move || backend::with_db(|conn| history::add(conn, &m, &c, page))).await;
                    backend::bump_data();
                    let _ = weak.update(cx, |_, cx| cx.notify());
                })
                .detach();
            }),
        );
    }

    fn clear(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        confirm_dialog(
            "Clear reading history?",
            format!("{} leave History. Your library, downloads and reading progress stay as they are.", if n == 1 { "1 title".to_string() } else { format!("All {n} titles") }),
            "Clear history",
            true,
        )
        .icon(ph::BROOM)
        .cancel_label("Keep history")
        .on_confirm(move |_, cx| {
            let _ = weak.update(cx, |t, cx| {
                t.state = Load::Ready(vec![]);
                cx.notify();
                cx.spawn(async move |_, _| {
                    let _ = mv_ui::bg::blocking(|| backend::with_db(|c| history::clear(c))).await;
                    backend::bump_data();
                })
                .detach();
            });
            toast::show(cx, Toast::success("Reading history cleared"));
        })
        .open(window, cx);
    }

    fn row(&self, h: &ReadingHistory, cx: &mut Context<Self>) -> Stateful<Div> {
        let resume = Route::Reader { manga_id: h.manga_id.clone(), chapter_id: h.chapter_id.clone() };
        let resume2 = resume.clone();
        let detail = Route::MangaDetail { manga_id: h.manga_id.clone(), source_id: Some(h.source_plugin.clone()) };
        let entry = h.clone();
        let ch = match (h.chapter_number, h.chapter_title.as_deref().map(str::trim).filter(|t| !t.is_empty())) {
            (Some(n), Some(t)) => format!("Ch. {} - {t}", logic::fmt_number(n)),
            (Some(n), None) => format!("Ch. {}", logic::fmt_number(n)),
            (None, Some(t)) => t.to_string(),
            (None, None) => "Oneshot".into(),
        };
        let time = logic::local_time(&h.read_at).map(|t| t.format("%H:%M").to_string()).unwrap_or_default();
        let cover = h.cover_url.as_deref().and_then(|u| self.covers.get(u)).cloned().map(CoverSource::File);
        // The whole row resumes reading; details and removal are explicit buttons.
        // NN/g, visible affordances: https://www.nngroup.com/articles/tooltip-guidelines/
        div()
            .id(SharedString::from(format!("h-{}", h.id)))
            .flex()
            .items_center()
            .gap(px(14.))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(radius::LG))
            .role(Role::Link)
            .aria_label(SharedString::from(format!("Resume {} {ch}", h.manga_title)))
            .hover(|s| s.bg(Tokens::hover()))
            .pressable()
            .on_click(move |_, w, cx| mv_ui::navigate(resume.clone(), w, cx))
            .child(thumb(cover, 44., 66.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).truncate().child(h.manga_title.clone()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(size::SM))
                            .text_color(Tokens::text_3())
                            .child(div().truncate().child(ch))
                            .child("·")
                            .child(div().flex_none().text_color(Tokens::accent()).child(format!("Page {}", h.page + 1)))
                            .when(!time.is_empty(), |d| d.child("·").child(div().flex_none().child(time))),
                    ),
            )
            .child(icon_button(SharedString::from(format!("resume-{}", h.id)), ph::PLAY, "Resume").on_click(move |_, w, cx| {
                cx.stop_propagation();
                mv_ui::navigate(resume2.clone(), w, cx);
            }))
            .child(icon_button(SharedString::from(format!("info-{}", h.id)), ph::INFO, "Title details and chapters").on_click(move |_, w, cx| {
                cx.stop_propagation();
                mv_ui::navigate(detail.clone(), w, cx);
            }))
            .child(icon_button(SharedString::from(format!("rm-{}", h.id)), ph::X, "Remove from history").on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.remove(&entry, cx);
            })))
    }
}

fn row_skeleton() -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(14.))
        .px(px(10.))
        .py(px(8.))
        .child(skeleton(px(44.), px(66.), radius::SM))
        .child(div().flex().flex_col().gap(px(8.)).flex_1().child(skeleton_line(px(220.))).child(skeleton_line(px(140.)).h(px(10.))))
}

impl Render for HistoryPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let v = backend::data_version();
        if v != self.seen_version {
            self.seen_version = v;
            self.reload(cx);
        }
        let visible: Vec<&ReadingHistory> = match &self.state {
            Load::Ready(i) => i.iter().filter(|h| !self.hidden.contains(&h.manga_id)).collect(),
            _ => vec![],
        };
        let n = visible.len();
        let has_items = n > 0;
        let header = page_title("History")
            .icon(glyphs::HISTORY)
            .when(has_items, |h| h.subtitle(if n == 1 { "1 title".to_string() } else { format!("{n} titles") }))
            .when(has_items, |h| {
                h.action(search_box(&self.input))
                    .action(btn("clear", "Clear history", ButtonKind::Ghost).icon(ph::BROOM).tooltip("Remove every title from History").on_click(cx.listener(move |this, _, w, cx| this.clear(n, w, cx))))
            });
        let body: AnyElement = match &self.state {
            Load::Loading => div().flex().flex_col().gap(px(4.)).children((0..6).map(|_| row_skeleton())).into_any_element(),
            Load::Failed(e) => error_view(e.clone())
                .title("Could not open History")
                .retry(cx.listener(|this, _, _, cx| {
                    this.state = Load::Loading;
                    this.reload(cx);
                    cx.notify();
                }))
                .into_any_element(),
            Load::Ready(_) if !has_items => empty_view(glyphs::HISTORY, "Nothing read yet", "Chapters you open in the reader show up here, so you can pick up where you left off.")
                .action(btn("go-library", "Open Library", ButtonKind::Primary).icon(glyphs::LIBRARY).on_click(|_, w, cx| mv_ui::navigate(Route::Library, w, cx)))
                .into_any_element(),
            Load::Ready(_) => {
                let q = self.query.trim().to_lowercase();
                let shown: Vec<&ReadingHistory> = visible.iter().copied().filter(|h| q.is_empty() || h.manga_title.to_lowercase().contains(&q)).collect();
                if shown.is_empty() {
                    empty_view(glyphs::SEARCH, "No matches", "Nothing in your history matches this search.")
                        .action(btn("clear-search", "Clear search", ButtonKind::Secondary).icon(ph::X).on_click(cx.listener(|this, _, w, cx| {
                            this.query.clear();
                            this.input.update(cx, |s, cx| s.set_value("", w, cx));
                            cx.notify();
                        })))
                        .into_any_element()
                } else {
                    let today = chrono::Local::now().date_naive();
                    let mut col = div().flex().flex_col();
                    let mut last_day = None;
                    let mut first = true;
                    for h in shown {
                        let day = logic::local_time(&h.read_at).map(|t| t.date());
                        if first || day != last_day {
                            let label = day.map(|d| logic::day_label(d, today)).unwrap_or_else(|| "Earlier".into());
                            col = col.child(
                                div()
                                    .px(px(10.))
                                    .pt(px(if first { 4. } else { 18. }))
                                    .pb(px(6.))
                                    .text_size(px(size::SM))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(Tokens::text_2())
                                    .child(label),
                            );
                            last_day = day;
                            first = false;
                        }
                        col = col.child(self.row(h, cx));
                    }
                    col.into_any_element()
                }
            }
        };
        div()
            .id("history-page")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                div()
                    .w_full()
                    .max_w(px(980.))
                    .px(px(size::PAGE_X))
                    .py(px(size::PAGE_Y))
                    .child(header)
                    .child(body)
                    .when(has_items, |d| d.child(div().mt(px(16.)).px(px(10.)).child(muted("History keeps the last chapter you read of each title.")))),
            )
    }
}
