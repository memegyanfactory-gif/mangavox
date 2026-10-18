use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::history::{self, ReadingHistory};
use mv_ui::{button, icon, page_header, size, ButtonKind, CoverSource, Route, Tokens};

use crate::backend;
use crate::logic;
use crate::ui::{error_view, icon_tip, muted, search_box, spawn_cover_downloads, state_view, Load};

pub struct HistoryPage {
    state: Load<Vec<ReadingHistory>>,
    covers: HashMap<String, PathBuf>,
    scroll: ScrollHandle,
    query: String,
    input: Entity<InputState>,
    confirm_clear: bool,
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
            confirm_clear: false,
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

    fn remove(&mut self, manga_id: String, cx: &mut Context<Self>) {
        if let Load::Ready(items) = &mut self.state {
            items.retain(|h| h.manga_id != manga_id);
        }
        cx.notify();
        cx.spawn(async move |_, _| {
            let _ = mv_ui::bg::blocking(move || backend::with_db(|c| history::remove(c, &manga_id))).await;
        })
        .detach();
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        if !self.confirm_clear {
            self.confirm_clear = true;
            cx.notify();
            return;
        }
        self.confirm_clear = false;
        self.state = Load::Ready(vec![]);
        cx.notify();
        cx.spawn(async move |_, _| {
            let _ = mv_ui::bg::blocking(|| backend::with_db(|c| history::clear(c))).await;
        })
        .detach();
    }

    fn row(&self, h: &ReadingHistory, cx: &mut Context<Self>) -> Stateful<Div> {
        let resume = Route::Reader { manga_id: h.manga_id.clone(), chapter_id: h.chapter_id.clone() };
        let resume2 = resume.clone();
        let detail = Route::MangaDetail { manga_id: h.manga_id.clone(), source_id: Some(h.source_plugin.clone()) };
        let mid = h.manga_id.clone();
        let ch = match (h.chapter_number, h.chapter_title.as_deref().map(str::trim).filter(|t| !t.is_empty())) {
            (Some(n), Some(t)) => format!("Ch. {} - {t}", logic::fmt_number(n)),
            (Some(n), None) => format!("Ch. {}", logic::fmt_number(n)),
            (None, Some(t)) => t.to_string(),
            (None, None) => "Oneshot".into(),
        };
        let time = logic::local_time(&h.read_at).map(|t| t.format("%H:%M").to_string()).unwrap_or_default();
        let cover = h.cover_url.as_deref().and_then(|u| self.covers.get(u)).cloned().map(CoverSource::File);
        div()
            .id(SharedString::from(format!("h-{}", h.id)))
            .flex()
            .items_center()
            .gap(px(14.))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(10.))
            .cursor_pointer()
            .hover(|s| s.bg(Tokens::hover()))
            .on_click(move |_, w, cx| mv_ui::navigate(resume.clone(), w, cx))
            .child(
                div()
                    .id(SharedString::from(format!("hc-{}", h.id)))
                    .w(px(44.))
                    .h(px(66.))
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
            .child(icon_tip(SharedString::from(format!("resume-{}", h.id)), "icons/play.svg", "Resume").on_click(move |_, w, cx| {
                cx.stop_propagation();
                mv_ui::navigate(resume2.clone(), w, cx);
            }))
            .child(icon_tip(SharedString::from(format!("rm-{}", h.id)), "icons/trash.svg", "Remove from history").on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.remove(mid.clone(), cx);
            })))
    }
}

impl Render for HistoryPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let v = backend::data_version();
        if v != self.seen_version {
            self.seen_version = v;
            self.reload(cx);
        }
        let has_items = matches!(&self.state, Load::Ready(i) if !i.is_empty());
        let count = match &self.state {
            Load::Ready(i) if !i.is_empty() => Some(SharedString::from(format!("{} titles", i.len()))),
            _ => None,
        };
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(div().flex_1().min_w_0().child(page_header("History", count)))
            .when(has_items, |d| {
                d.child(div().mb(px(20.)).child(search_box(&self.input))).child(
                    div().mb(px(20.)).child(
                        button("clear", if self.confirm_clear { "Click again to clear" } else { "Clear all" }, ButtonKind::Ghost)
                            .when(self.confirm_clear, |d| d.text_color(Tokens::danger()))
                            .on_click(cx.listener(|this, _, _, cx| this.clear(cx))),
                    ),
                )
            });
        let body: AnyElement = match &self.state {
            Load::Loading => div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .children((0..6).map(|_| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(14.))
                        .px(px(10.))
                        .child(div().w(px(44.)).h(px(66.)).rounded(px(6.)).bg(Tokens::surface_2()))
                        .child(div().flex().flex_col().gap(px(6.)).child(div().w(px(220.)).h(px(12.)).rounded(px(4.)).bg(Tokens::surface_2())).child(div().w(px(140.)).h(px(10.)).rounded(px(4.)).bg(Tokens::surface_2())))
                }))
                .into_any_element(),
            Load::Failed(e) => error_view(e.clone()).into_any_element(),
            Load::Ready(items) if items.is_empty() => state_view("icons/clock.svg", "Nothing read yet", "Chapters you open in the reader show up here.").into_any_element(),
            Load::Ready(items) => {
                let q = self.query.trim().to_lowercase();
                let shown: Vec<&ReadingHistory> = items.iter().filter(|h| q.is_empty() || h.manga_title.to_lowercase().contains(&q)).collect();
                if shown.is_empty() {
                    state_view("icons/search.svg", "No matches", "Nothing in your history matches this search.").into_any_element()
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
