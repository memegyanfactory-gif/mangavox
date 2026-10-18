use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::history::{self, ReadingHistory};
use mv_ui::{button, empty_state, error_state, icon_button, page, page_header, ButtonKind, CoverSource, Route, Tokens};

use crate::backend;
use crate::logic;
use crate::ui::{muted, spawn_cover_downloads, Load};

pub struct HistoryPage {
    state: Load<Vec<ReadingHistory>>,
    covers: HashMap<String, PathBuf>,
}

impl HistoryPage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(|| {
                let items = backend::with_db(|c| history::list(c, 200))?;
                let (have, missing) =
                    backend::resolve_covers(items.iter().filter_map(|h| h.cover_url.clone().map(|u| (u, Some(h.source_plugin.clone())))));
                Ok::<_, anyhow::Error>((items, have, missing))
            })
            .await
            .and_then(|r| r);
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
        Self { state: Load::Loading, covers: HashMap::new() }
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
        self.state = Load::Ready(vec![]);
        cx.notify();
        cx.spawn(async move |_, _| {
            let _ = mv_ui::bg::blocking(|| backend::with_db(|c| history::clear(c))).await;
        })
        .detach();
    }
}

impl Render for HistoryPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = chrono::Utc::now().naive_utc();
        let count = match &self.state {
            Load::Ready(i) if !i.is_empty() => Some(SharedString::from(format!("{} recent", i.len()))),
            _ => None,
        };
        let mut header = div().flex().items_center().child(div().flex_1().child(page_header("History", count)));
        if matches!(&self.state, Load::Ready(i) if !i.is_empty()) {
            header = header.child(button("clear", "Clear all", ButtonKind::Ghost).on_click(cx.listener(|this, _, _, cx| this.clear(cx))));
        }
        let body: AnyElement = match &self.state {
            Load::Loading => div().flex().flex_col().gap(px(8.)).children((0..6).map(|_| div().h(px(64.)).rounded(px(10.)).bg(Tokens::surface_2()))).into_any_element(),
            Load::Failed(e) => error_state(e.clone()).into_any_element(),
            Load::Ready(items) if items.is_empty() => empty_state("Nothing read yet", "Chapters you open in the reader show up here.").into_any_element(),
            Load::Ready(items) => div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .children(items.iter().map(|h| {
                    let route = Route::Reader { manga_id: h.manga_id.clone(), chapter_id: h.chapter_id.clone() };
                    let mid = h.manga_id.clone();
                    let ch = h.chapter_number.map(|n| format!("Ch. {}", logic::fmt_number(n))).unwrap_or_else(|| "Oneshot".into());
                    let sub = format!("{ch}  -  page {}  -  {}", h.page + 1, logic::time_ago(&h.read_at, now));
                    let cover = h.cover_url.as_deref().and_then(|u| self.covers.get(u)).cloned().map(CoverSource::File);
                    div()
                        .id(SharedString::from(format!("h-{}", h.id)))
                        .flex()
                        .items_center()
                        .gap(px(14.))
                        .p(px(8.))
                        .rounded(px(10.))
                        .border_1()
                        .border_color(Tokens::border())
                        .bg(Tokens::surface())
                        .cursor_pointer()
                        .hover(|s| s.bg(Tokens::hover()))
                        .on_click(move |_, w, cx| mv_ui::navigate(route.clone(), w, cx))
                        .child(
                            div().w(px(40.)).h(px(56.)).rounded(px(6.)).overflow_hidden().bg(Tokens::surface_2()).flex_none().when_some(cover, |d, c| {
                                d.child(c.into_img().size_full().object_fit(ObjectFit::Cover))
                            }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(div().text_size(px(13.5)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).truncate().child(h.manga_title.clone()))
                                .child(muted(sub)),
                        )
                        .child(button(SharedString::from(format!("resume-{}", h.id)), "Resume", ButtonKind::Secondary))
                        .child(
                            icon_button(SharedString::from(format!("rm-{}", h.id)), "icons/x.svg", true).on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.remove(mid.clone(), cx);
                            })),
                        )
                }))
                .into_any_element(),
        };
        page("history-page").child(header).child(body)
    }
}
