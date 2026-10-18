use std::path::PathBuf;

use anyhow::{anyhow, bail, Result};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::chapter::{self, Chapter};
use mv_core::repo::manga;
use mv_ui::{button, chip, size, ButtonKind, Route, Tokens};

use crate::backend;
use crate::logic;
use crate::store;
use crate::ui::{muted, Load};

const MAX_INFLIGHT: usize = 3;
const STRIP_WIDTH: f32 = 860.0;

#[derive(Clone)]
enum PageState {
    Idle,
    Loading,
    Ready(PathBuf),
    Failed(String),
}

struct PageEntry {
    url: String,
    state: PageState,
}

struct Loaded {
    manga_title: String,
    source: String,
    chapters: Vec<Chapter>,
    chapter: Chapter,
    pages: Vec<PageEntry>,
    start: usize,
}

fn local_pages(ch: &Chapter) -> Vec<PathBuf> {
    if ch.download_status != "complete" {
        return vec![];
    }
    let Some(dir) = ch.pages_dir.as_deref() else { return vec![] };
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()).is_some_and(|e| ["jpg", "jpeg", "png", "webp", "gif"].contains(&e.to_lowercase().as_str()))
                && std::fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false)
        })
        .collect();
    v.sort();
    v
}

async fn load_chapter(chapter_id: String) -> Result<Loaded> {
    let cid = chapter_id.clone();
    let (m, chapters, ch) = mv_ui::bg::blocking(move || {
        backend::with_db(|c| {
            let ch = chapter::get(c, &cid)?.ok_or_else(|| anyhow!("Chapter not found"))?;
            let m = manga::get(c, &ch.manga_id)?.ok_or_else(|| anyhow!("Manga not found"))?;
            let list = chapter::list(c, &m.id)?;
            Ok((m, list, ch))
        })
    })
    .await??;
    let source = if ch.source_plugin.is_empty() { m.source_plugin.clone() } else { ch.source_plugin.clone() };
    let local = local_pages(&ch);
    let pages: Vec<PageEntry> = if !local.is_empty() {
        local.into_iter().map(|p| PageEntry { url: String::new(), state: PageState::Ready(p) }).collect()
    } else {
        let src = backend::source(&source).await?;
        let urls = src.get_page_urls(&ch.source_id).await.map_err(|e| anyhow!("{e}"))?;
        if urls.is_empty() {
            bail!("The source returned no pages for this chapter");
        }
        urls.into_iter().map(|url| PageEntry { url, state: PageState::Idle }).collect()
    };
    let start = if ch.read_progress > 0 && !ch.is_read { logic::clamp_page(ch.read_progress, pages.len()) } else { 0 };
    Ok(Loaded { manga_title: m.title, source, chapters, chapter: ch, pages, start })
}

pub struct ReaderPage {
    manga_id: String,
    title: String,
    source: String,
    chapters: Vec<Chapter>,
    chapter: Option<Chapter>,
    pages: Load<Vec<PageEntry>>,
    current: usize,
    strip: bool,
    inflight: usize,
    generation: u64,
    focus: FocusHandle,
    scroll: ScrollHandle,
}

impl ReaderPage {
    pub fn new(manga_id: String, chapter_id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut p = Self {
            manga_id,
            title: String::new(),
            source: String::new(),
            chapters: vec![],
            chapter: None,
            pages: Load::Loading,
            current: 0,
            strip: mv_core::config::Config::open_default().get("readerMode").and_then(|v| v.as_str().map(|s| s == "vertical")).unwrap_or(false),
            inflight: 0,
            generation: 0,
            focus,
            scroll: ScrollHandle::new(),
        };
        p.open_chapter(chapter_id, cx);
        p
    }

    fn open_chapter(&mut self, chapter_id: String, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.pages = Load::Loading;
        self.inflight = 0;
        self.current = 0;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(load_chapter(chapter_id)).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match res {
                    Ok(l) => {
                        this.title = l.manga_title;
                        this.source = l.source;
                        this.chapters = l.chapters;
                        this.current = l.start;
                        this.chapter = Some(l.chapter);
                        this.pages = Load::Ready(l.pages);
                        this.scroll.set_offset(point(px(0.), px(0.)));
                        this.save(cx);
                        this.pump(cx);
                    }
                    Err(e) => this.pages = Load::Failed(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn total(&self) -> usize {
        match &self.pages {
            Load::Ready(p) => p.len(),
            _ => 0,
        }
    }

    /// Persist progress, history and read state off the UI thread.
    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(ch) = &self.chapter else { return };
        let (mid, cid, page, total) = (self.manga_id.clone(), ch.id.clone(), self.current, self.total());
        if total == 0 {
            return;
        }
        cx.spawn(async move |_, _| {
            let _ = mv_ui::bg::blocking(move || backend::with_db(|c| store::save_progress(c, &mid, &cid, page, total))).await;
        })
        .detach();
    }

    /// Start downloads for pages near the current one, keeping a few in flight.
    fn pump(&mut self, cx: &mut Context<Self>) {
        let total = self.total();
        let (behind, ahead) = if self.strip { (1, 6) } else { (1, 4) };
        for i in logic::preload_window(self.current, total, behind, ahead) {
            if self.inflight >= MAX_INFLIGHT {
                break;
            }
            let Load::Ready(pages) = &mut self.pages else { return };
            if !matches!(pages[i].state, PageState::Idle) {
                continue;
            }
            pages[i].state = PageState::Loading;
            self.inflight += 1;
            let (url, src, generation) = (pages[i].url.clone(), self.source.clone(), self.generation);
            cx.spawn(async move |this, cx| {
                let res = mv_ui::bg::run(backend::download_page(url, Some(src))).await.and_then(|r| r);
                let _ = this.update(cx, |this, cx| {
                    if this.generation != generation {
                        return;
                    }
                    this.inflight = this.inflight.saturating_sub(1);
                    if let Load::Ready(pages) = &mut this.pages {
                        pages[i].state = match res {
                            Ok(p) => PageState::Ready(p),
                            Err(e) => PageState::Failed(format!("{e:#}")),
                        };
                    }
                    this.pump(cx);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn goto(&mut self, page: usize, cx: &mut Context<Self>) {
        let page = page.min(self.total().saturating_sub(1));
        if page == self.current {
            return;
        }
        self.current = page;
        self.save(cx);
        self.pump(cx);
        cx.notify();
    }

    fn chapter_index(&self) -> Option<usize> {
        let id = &self.chapter.as_ref()?.id;
        self.chapters.iter().position(|c| &c.id == id)
    }

    fn step_chapter(&mut self, dir: i32, cx: &mut Context<Self>) -> bool {
        let Some(i) = self.chapter_index() else { return false };
        let j = i as i32 + dir;
        if j < 0 || j as usize >= self.chapters.len() {
            return false;
        }
        if dir > 0 {
            // leaving forward counts as finished
            if let Some(ch) = &self.chapter {
                let (mid, cid, total) = (self.manga_id.clone(), ch.id.clone(), self.total());
                cx.spawn(async move |_, _| {
                    let _ = mv_ui::bg::blocking(move || backend::with_db(|c| store::save_progress(c, &mid, &cid, total.saturating_sub(1), total))).await;
                })
                .detach();
            }
        }
        let id = self.chapters[j as usize].id.clone();
        self.open_chapter(id, cx);
        true
    }

    fn next(&mut self, cx: &mut Context<Self>) {
        if self.strip {
            self.scroll_by(0.85, cx);
        } else if self.current + 1 < self.total() {
            self.goto(self.current + 1, cx);
        } else {
            self.step_chapter(1, cx);
        }
    }

    fn prev(&mut self, cx: &mut Context<Self>) {
        if self.strip {
            self.scroll_by(-0.85, cx);
        } else if self.current > 0 {
            self.goto(self.current - 1, cx);
        } else {
            self.step_chapter(-1, cx);
        }
    }

    fn scroll_by(&mut self, viewport_fraction: f32, cx: &mut Context<Self>) {
        let h = self.scroll.bounds().size.height;
        let o = self.scroll.offset();
        let y = (o.y - h * viewport_fraction).min(px(0.));
        self.scroll.set_offset(point(o.x, y));
        self.sync_strip_page(cx);
        cx.notify();
    }

    fn sync_strip_page(&mut self, cx: &mut Context<Self>) {
        let top = self.scroll.top_item();
        if top != self.current && top < self.total() {
            self.goto(top, cx);
        }
    }

    fn set_mode(&mut self, strip: bool, cx: &mut Context<Self>) {
        if self.strip == strip {
            return;
        }
        self.strip = strip;
        let _ = mv_core::config::Config::open_default().set("readerMode", serde_json::json!(if strip { "vertical" } else { "paged" }));
        self.scroll.scroll_to_item(self.current);
        self.pump(cx);
        cx.notify();
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        match k.key.as_str() {
            "escape" => mv_ui::navigate(Route::Back, window, cx),
            "right" | "pagedown" | "down" => self.next(cx),
            "space" if k.modifiers.shift => self.prev(cx),
            "space" => self.next(cx),
            "left" | "pageup" | "up" => self.prev(cx),
            "home" => self.goto(0, cx),
            "end" => self.goto(self.total().saturating_sub(1), cx),
            "m" => self.set_mode(!self.strip, cx),
            "n" => {
                self.step_chapter(1, cx);
            }
            "p" => {
                self.step_chapter(-1, cx);
            }
            _ => {}
        }
    }

    fn retry(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Load::Ready(pages) = &mut self.pages {
            if matches!(pages[i].state, PageState::Failed(_)) {
                pages[i].state = PageState::Idle;
            }
        }
        self.pump(cx);
        cx.notify();
    }

    fn page_view(&self, i: usize, fit_width: bool, cx: &mut Context<Self>) -> AnyElement {
        let Load::Ready(pages) = &self.pages else { return div().into_any_element() };
        match &pages[i].state {
            PageState::Ready(p) => {
                let im = img(p.clone());
                if fit_width {
                    im.w_full().into_any_element()
                } else {
                    im.size_full().object_fit(ObjectFit::Contain).into_any_element()
                }
            }
            PageState::Failed(e) => div()
                .id(SharedString::from(format!("retry-{i}")))
                .size_full()
                .min_h(px(240.))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .cursor_pointer()
                .child(div().text_size(px(size::MD)).text_color(Tokens::danger()).child(format!("Page {} failed to load", i + 1)))
                .child(muted(e.clone()))
                .child(muted("Click to retry"))
                .on_click(cx.listener(move |this, _, _, cx| this.retry(i, cx)))
                .into_any_element(),
            _ => div().size_full().min_h(px(240.)).flex().items_center().justify_center().child(muted(format!("Loading page {}...", i + 1))).into_any_element(),
        }
    }

    fn top_bar(&self, cx: &mut Context<Self>) -> Div {
        let idx = self.chapter_index();
        let has_prev = idx.is_some_and(|i| i > 0);
        let has_next = idx.is_some_and(|i| i + 1 < self.chapters.len());
        let label = self.chapter.as_ref().map(logic::chapter_label).unwrap_or_default();
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(44.))
            .px(px(16.))
            .flex_none()
            .border_b_1()
            .border_color(Tokens::border())
            .bg(Tokens::sidebar())
            .child(button("close", "Close", ButtonKind::Ghost).on_click(|_, w, cx| mv_ui::navigate(Route::Back, w, cx)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .gap(px(8.))
                    .child(div().truncate().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(self.title.clone()))
                    .child(div().flex_none().child(muted(label))),
            )
            .child(muted(if self.total() > 0 { format!("{} / {}", self.current + 1, self.total()) } else { String::new() }))
            .child(button("prev-ch", "Prev chapter", ButtonKind::Ghost).when(!has_prev, |d| d.opacity(0.4)).on_click(cx.listener(|this, _, _, cx| {
                this.step_chapter(-1, cx);
            })))
            .child(button("next-ch", "Next chapter", ButtonKind::Ghost).when(!has_next, |d| d.opacity(0.4)).on_click(cx.listener(|this, _, _, cx| {
                this.step_chapter(1, cx);
            })))
            .child(chip("mode-paged", "Paged", !self.strip).on_click(cx.listener(|this, _, _, cx| this.set_mode(false, cx))))
            .child(chip("mode-strip", "Strip", self.strip).on_click(cx.listener(|this, _, _, cx| this.set_mode(true, cx))))
    }
}

impl Render for ReaderPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = match &self.pages {
            Load::Loading => div().size_full().flex().items_center().justify_center().child(muted("Loading chapter...")).into_any_element(),
            Load::Failed(e) => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .child(div().text_size(px(size::LG)).text_color(Tokens::text_1()).child("Could not open this chapter"))
                .child(muted(e.clone()))
                .child(button("reload", "Try again", ButtonKind::Secondary).on_click(cx.listener(|this, _, _, cx| {
                    if let Some(c) = this.chapter.as_ref().map(|c| c.id.clone()) {
                        this.open_chapter(c, cx);
                    }
                })))
                .into_any_element(),
            Load::Ready(pages) if self.strip => {
                let n = pages.len();
                div()
                    .id("strip")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                        this.sync_strip_page(cx);
                        cx.notify();
                    }))
                    .child(div().flex().flex_col().items_center().children((0..n).map(|i| {
                        div().w_full().max_w(px(STRIP_WIDTH)).child(self.page_view(i, true, cx))
                    })))
                    .into_any_element()
            }
            Load::Ready(_) => {
                let i = self.current;
                div()
                    .id("paged")
                    .size_full()
                    .relative()
                    .child(self.page_view(i, false, cx))
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .w(px(120.))
                            .id("hit-prev")
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| this.prev(cx))),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right_0()
                            .w(px(120.))
                            .id("hit-next")
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| this.next(cx))),
                    )
                    .into_any_element()
            }
        };
        div()
            .id("reader")
            .key_context("Reader")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| this.on_key(ev, w, cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x000000))
            .child(self.top_bar(cx))
            .child(div().flex_1().min_h_0().child(body))
    }
}
