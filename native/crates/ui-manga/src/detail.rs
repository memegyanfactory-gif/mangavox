use std::ops::Range;
use std::path::PathBuf;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::chapter::{self, Chapter};
use mv_core::repo::manga::{self, Manga};
use mv_ui::{button, error_state, size, ButtonKind, CoverSource, Route, Tokens};

use crate::backend;
use crate::logic;
use crate::ops;
use crate::ui::{muted, Load};

pub struct DetailPage {
    manga_id: String,
    state: Load<(Manga, Vec<Chapter>)>,
    cover: Option<PathBuf>,
    newest_first: bool,
    expanded: bool,
    refreshing: bool,
    confirm_remove: bool,
    note: Option<String>,
    scroll: UniformListScrollHandle,
}

fn load(id: String) -> anyhow::Result<(Manga, Vec<Chapter>, Option<PathBuf>)> {
    backend::with_db(|c| {
        let m = manga::get(c, &id)?.ok_or_else(|| anyhow::anyhow!("This title is not in your library"))?;
        let cs = chapter::list(c, &id)?;
        let cover = m.cover_path.as_deref().and_then(mv_core::covers::cached_cover);
        Ok((m, cs, cover))
    })
}

impl DetailPage {
    pub fn new(manga_id: String, cx: &mut Context<Self>) -> Self {
        let mut p = Self {
            manga_id,
            state: Load::Loading,
            cover: None,
            newest_first: true,
            expanded: false,
            refreshing: false,
            confirm_remove: false,
            note: None,
            scroll: UniformListScrollHandle::new(),
        };
        p.reload(cx);
        p
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let id = self.manga_id.clone();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || load(id)).await.and_then(|r| r);
            let missing = match &res {
                Ok((m, _, None)) => m.cover_path.clone().filter(|u| u.starts_with("http")).map(|u| (u, m.source_plugin.clone())),
                _ => None,
            };
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok((m, cs, cover)) => {
                        this.cover = cover;
                        this.state = Load::Ready((m, cs));
                    }
                    Err(e) => this.state = Load::Failed(format!("{e:#}")),
                }
                cx.notify();
            });
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
                        this.note = Some(format!("{} chapters", cs.len()));
                        this.state = Load::Ready((m, cs));
                    }
                    Err(e) => this.note = Some(format!("Refresh failed: {e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle_favorite(&mut self, cx: &mut Context<Self>) {
        if let Load::Ready((m, _)) = &mut self.state {
            m.is_favorite = !m.is_favorite;
            cx.notify();
        }
        let id = self.manga_id.clone();
        cx.spawn(async move |_, _| {
            let _ = mv_ui::bg::blocking(move || backend::with_db(|c| manga::toggle_favorite(c, &id))).await;
        })
        .detach();
    }

    fn remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.confirm_remove {
            self.confirm_remove = true;
            cx.notify();
            return;
        }
        let id = self.manga_id.clone();
        cx.spawn_in(window, async move |_, cx| {
            let _ = mv_ui::bg::blocking(move || backend::with_db(|c| manga::remove(c, &id))).await;
            let _ = cx.update(|w, cx| mv_ui::navigate(Route::Library, w, cx));
        })
        .detach();
    }

    fn header(&self, m: &Manga, chapters: &[Chapter], cx: &mut Context<Self>) -> Div {
        let cover = self.cover.clone().map(CoverSource::File);
        let target = logic::continue_target(chapters).map(|(i, _)| chapters[i].id.clone());
        let mid = m.id.clone();
        let meta: Vec<String> = [m.author.clone(), m.status.clone()].into_iter().flatten().filter(|s| !s.is_empty()).collect();
        let desc = m.description.clone().unwrap_or_default();
        let long = desc.chars().count() > 280;
        div()
            .flex()
            .gap(px(24.))
            .mb(px(20.))
            .child(
                div().w(px(170.)).h(px(244.)).flex_none().rounded(px(12.)).overflow_hidden().bg(Tokens::surface_2()).border_1().border_color(Tokens::border()).when_some(cover, |d, c| {
                    d.child(c.into_img().size_full().object_fit(ObjectFit::Cover))
                }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(div().text_size(px(size::XXL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(m.title.clone()))
                    .when(!meta.is_empty(), |d| d.child(muted(meta.join("  -  "))))
                    .when(!m.genres.is_empty(), |d| {
                        d.child(div().flex().flex_wrap().gap(px(6.)).children(m.genres.iter().take(12).map(|g| {
                            div().px(px(8.)).py(px(2.)).rounded(px(999.)).border_1().border_color(Tokens::border()).text_size(px(size::XS)).text_color(Tokens::text_2()).child(g.clone())
                        })))
                    })
                    .when(!desc.is_empty(), |d| {
                        let expanded = self.expanded;
                        d.child(
                            div()
                                .id("desc")
                                .text_size(px(size::MD))
                                .text_color(Tokens::text_2())
                                .when(long, |d| d.cursor_pointer().on_click(cx.listener(|this, _, _, cx| {
                                    this.expanded = !this.expanded;
                                    cx.notify();
                                })))
                                .when(!expanded, |d| d.line_clamp(4))
                                .child(desc),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .mt(px(4.))
                            .child(
                                button("read", logic::continue_label(chapters), ButtonKind::Primary).on_click(move |_, w, cx| {
                                    if let Some(cid) = &target {
                                        mv_ui::navigate(Route::Reader { manga_id: mid.clone(), chapter_id: cid.clone() }, w, cx);
                                    }
                                }),
                            )
                            .child(
                                button("fav", if m.is_favorite { "Favorited" } else { "Favorite" }, ButtonKind::Secondary)
                                    .on_click(cx.listener(|this, _, _, cx| this.toggle_favorite(cx))),
                            )
                            .child(
                                button("refresh", if self.refreshing { "Refreshing..." } else { "Refresh chapters" }, ButtonKind::Ghost)
                                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                            )
                            .child(
                                button("remove", if self.confirm_remove { "Click again to remove" } else { "Remove" }, ButtonKind::Ghost)
                                    .when(self.confirm_remove, |d| d.text_color(Tokens::danger()))
                                    .on_click(cx.listener(|this, _, w, cx| this.remove(w, cx))),
                            )
                            .when_some(self.note.clone(), |d, n| d.child(muted(n))),
                    ),
            )
    }

    fn chapter_list(&self, chapters: &[Chapter], cx: &mut Context<Self>) -> impl IntoElement {
        let n = chapters.len();
        let newest = self.newest_first;
        let current = logic::continue_target(chapters).map(|(i, _)| chapters[i].id.clone());
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .mb(px(8.))
                    .child(div().flex_1().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("Chapters ({n})")))
                    .child(
                        button("order", if newest { "Newest first" } else { "Oldest first" }, ButtonKind::Ghost).on_click(cx.listener(|this, _, _, cx| {
                            this.newest_first = !this.newest_first;
                            cx.notify();
                        })),
                    ),
            )
            .child(if n == 0 {
                muted("No chapters yet. Try Refresh chapters.").into_any_element()
            } else {
                uniform_list(
                    "chapters",
                    n,
                    cx.processor(move |this: &mut Self, range: Range<usize>, _w, _cx| {
                        let Load::Ready((m, cs)) = &this.state else { return vec![] };
                        range
                            .map(|row| {
                                let c = if newest { &cs[cs.len() - 1 - row] } else { &cs[row] };
                                let route = Route::Reader { manga_id: m.id.clone(), chapter_id: c.id.clone() };
                                let status = if c.is_read {
                                    "Read".to_string()
                                } else if c.read_progress > 0 {
                                    format!("Page {}", c.read_progress + 1)
                                } else {
                                    String::new()
                                };
                                let is_cur = current.as_deref() == Some(c.id.as_str());
                                div()
                                    .id(SharedString::from(format!("ch-{}", c.id)))
                                    .flex()
                                    .items_center()
                                    .h(px(40.))
                                    .px(px(12.))
                                    .rounded(px(8.))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(Tokens::hover()))
                                    .when(is_cur, |d| d.bg(Tokens::accent_soft()))
                                    .on_click(move |_, w, cx| mv_ui::navigate(route.clone(), w, cx))
                                    .child(
                                        div()
                                            .flex_1()
                                            .truncate()
                                            .text_size(px(size::MD))
                                            .text_color(if c.is_read { Tokens::text_3() } else { Tokens::text_1() })
                                            .child(logic::chapter_label(c)),
                                    )
                                    .child(muted(status))
                            })
                            .collect()
                    }),
                )
                .track_scroll(&self.scroll)
                .h_full()
                .into_any_element()
            })
    }
}

impl Render for DetailPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = div().size_full().flex().flex_col().px(px(size::PAGE_X)).pt(px(size::PAGE_Y));
        match &self.state {
            Load::Loading => root.child(muted("Loading...")),
            Load::Failed(e) => root.child(error_state(e.clone())),
            Load::Ready((m, cs)) => {
                let (m, cs) = (m.clone(), cs.clone());
                root.child(self.header(&m, &cs, cx)).child(self.chapter_list(&cs, cx))
            }
        }
    }
}
