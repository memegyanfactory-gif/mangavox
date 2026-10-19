use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::chapter::{self, Chapter};
use mv_core::repo::manga;
use mv_ui::{button, icon, size, ButtonKind, Route, Tokens};

use crate::backend;
use crate::logic::{self, Fit, ReaderMode};
use crate::store;
use crate::ui::{muted, spinner, Load};

const MAX_INFLIGHT: usize = 3;
const STRIP_WIDTH: f32 = 860.0;
/// Overlay hides after this long without activity near the bars.
const OVERLAY_IDLE: Duration = Duration::from_millis(2600);
/// Mouse within this distance of the top / bottom edge reveals the overlay.
const EDGE_TOP: f32 = 72.0;
const EDGE_BOTTOM: f32 = 120.0;

#[derive(Clone)]
enum PageState {
    Idle,
    Loading,
    /// The file and its width / height ratio (when the header could be read).
    Ready(PathBuf, Option<f32>),
    Failed(String),
}

/// Width / height of an image file, read from its header only.
fn aspect(path: &Path) -> Option<f32> {
    let s = imagesize::size(path).ok()?;
    (s.width > 0 && s.height > 0).then(|| s.width as f32 / s.height as f32)
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

async fn load_chapter(chapter_id: String, at_end: bool) -> Result<Loaded> {
    let cid = chapter_id.clone();
    let lang = backend::preferred_language();
    let (m, chapters, ch) = mv_ui::bg::blocking(move || {
        backend::with_db(|c| {
            let ch = chapter::get(c, &cid)?.ok_or_else(|| anyhow!("Chapter not found"))?;
            let m = manga::get(c, &ch.manga_id)?.ok_or_else(|| anyhow!("Manga not found"))?;
            let mut list = store::chapters(c, &m.id, &lang)?;
            // The open chapter is always part of the list, even in another language.
            if !list.iter().any(|x| x.id == ch.id) {
                list = chapter::list(c, &m.id)?;
            }
            Ok((m, list, ch))
        })
    })
    .await??;
    let source = if ch.source_plugin.is_empty() { m.source_plugin.clone() } else { ch.source_plugin.clone() };
    let local = local_pages(&ch);
    let pages: Vec<PageEntry> = if !local.is_empty() {
        local.into_iter().map(|p| PageEntry { url: String::new(), state: PageState::Ready(p.clone(), aspect(&p)) }).collect()
    } else {
        let src = backend::source(&source).await?;
        let urls = src.get_page_urls(&ch.source_id).await.map_err(|e| anyhow!("{e}"))?;
        if urls.is_empty() {
            bail!("The source returned no pages for this chapter");
        }
        urls.into_iter().map(|url| PageEntry { url, state: PageState::Idle }).collect()
    };
    let start = if at_end {
        pages.len().saturating_sub(1)
    } else if ch.read_progress > 0 && !ch.is_read {
        logic::clamp_page(ch.read_progress, pages.len())
    } else {
        0
    };
    Ok(Loaded { manga_title: m.title, source, chapters, chapter: ch, pages, start })
}

/// Records an element's window bounds during prepaint (for click / drag math).
fn bounds_probe(cell: Rc<Cell<Bounds<Pixels>>>) -> impl IntoElement {
    canvas(move |b, _, _| cell.set(b), |_, _, _, _| {}).absolute().top_0().left_0().size_full()
}

pub struct ReaderPage {
    manga_id: String,
    title: String,
    source: String,
    chapters: Vec<Chapter>,
    chapter: Option<Chapter>,
    pages: Load<Vec<PageEntry>>,
    current: usize,
    mode: ReaderMode,
    fit: Fit,
    /// Paged mode: showing the "next / previous chapter" card instead of a page.
    transition: Option<i32>,
    inflight: usize,
    generation: u64,
    focus: FocusHandle,
    /// Long-strip scroll position (one child per page).
    strip_scroll: ScrollHandle,
    /// Paged mode scroll (fit width / original size pages can be taller than the window).
    page_scroll: ScrollHandle,
    overlay: bool,
    last_activity: Instant,
    over_bars: bool,
    hide_task: Option<Task<()>>,
    root_bounds: Rc<Cell<Bounds<Pixels>>>,
    slider_bounds: Rc<Cell<Bounds<Pixels>>>,
    dragging: bool,
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
            mode: ReaderMode::from_config(&backend::config_str("readerMode").unwrap_or_default()),
            fit: Fit::from_config(&backend::config_str("readerFit").unwrap_or_default()),
            transition: None,
            inflight: 0,
            generation: 0,
            focus,
            strip_scroll: ScrollHandle::new(),
            page_scroll: ScrollHandle::new(),
            overlay: true,
            last_activity: Instant::now(),
            over_bars: false,
            hide_task: None,
            root_bounds: Rc::new(Cell::new(Bounds::default())),
            slider_bounds: Rc::new(Cell::new(Bounds::default())),
            dragging: false,
        };
        p.open_chapter(chapter_id, false, cx);
        p.show_overlay(cx);
        p
    }

    fn strip(&self) -> bool {
        self.mode == ReaderMode::Strip
    }

    fn open_chapter(&mut self, chapter_id: String, at_end: bool, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.pages = Load::Loading;
        self.inflight = 0;
        self.current = 0;
        self.transition = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(load_chapter(chapter_id, at_end)).await.and_then(|r| r);
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
                        this.page_scroll.set_offset(point(px(0.), px(0.)));
                        this.strip_scroll.set_offset(point(px(0.), px(0.)));
                        if this.current > 0 {
                            this.strip_scroll.scroll_to_top_of_item(this.current);
                        }
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
        let (behind, ahead) = if self.strip() { (1, 6) } else { (1, 4) };
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
                let res = mv_ui::bg::run(async move { backend::download_page(url, Some(src)).await.map(|p| { let r = aspect(&p); (p, r) }) }).await.and_then(|r| r);
                let _ = this.update(cx, |this, cx| {
                    if this.generation != generation {
                        return;
                    }
                    this.inflight = this.inflight.saturating_sub(1);
                    if let Load::Ready(pages) = &mut this.pages {
                        pages[i].state = match res {
                            Ok((p, r)) => PageState::Ready(p, r),
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
        self.transition = None;
        if page == self.current {
            cx.notify();
            return;
        }
        self.current = page;
        self.page_scroll.set_offset(point(px(0.), px(0.)));
        self.save(cx);
        self.pump(cx);
        cx.notify();
    }

    /// Jump to a page from the slider / Home / End (also scrolls the strip).
    fn jump(&mut self, page: usize, cx: &mut Context<Self>) {
        if self.strip() {
            self.strip_scroll.scroll_to_top_of_item(page.min(self.total().saturating_sub(1)));
        }
        self.goto(page, cx);
    }

    fn chapter_index(&self) -> Option<usize> {
        let id = &self.chapter.as_ref()?.id;
        self.chapters.iter().position(|c| &c.id == id)
    }

    fn neighbour(&self, dir: i32) -> Option<&Chapter> {
        let i = self.chapter_index()?;
        logic::step_chapter_index(&self.chapters, i, dir).map(|j| &self.chapters[j])
    }

    fn step_chapter(&mut self, dir: i32, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.neighbour(dir).map(|c| c.id.clone()) else { return false };
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
        // Going back opens the previous chapter at its last page, like turning pages backwards.
        self.open_chapter(id, dir < 0 && !self.strip(), cx);
        true
    }

    fn next(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.pages, Load::Ready(_)) {
            return;
        }
        if self.strip() {
            self.scroll_by(0.85, cx);
        } else if let Some(t) = self.transition {
            if t > 0 {
                if !self.step_chapter(1, cx) {
                    cx.notify();
                }
            } else {
                self.goto(self.current, cx);
            }
        } else if self.current + 1 < self.total() {
            self.goto(self.current + 1, cx);
        } else {
            self.transition = Some(1);
            cx.notify();
        }
    }

    fn prev(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.pages, Load::Ready(_)) {
            return;
        }
        if self.strip() {
            self.scroll_by(-0.85, cx);
        } else if let Some(t) = self.transition {
            if t < 0 {
                self.step_chapter(-1, cx);
            } else {
                self.goto(self.current, cx);
            }
        } else if self.current > 0 {
            self.goto(self.current - 1, cx);
        } else {
            self.transition = Some(-1);
            cx.notify();
        }
    }

    fn scroll_by(&mut self, viewport_fraction: f32, cx: &mut Context<Self>) {
        let h = self.strip_scroll.bounds().size.height;
        let o = self.strip_scroll.offset();
        let max = self.strip_scroll.max_offset().y;
        let y = (o.y - h * viewport_fraction).min(px(0.)).max(-max);
        self.strip_scroll.set_offset(point(o.x, y));
        self.sync_strip_page(cx);
        cx.notify();
    }

    fn sync_strip_page(&mut self, cx: &mut Context<Self>) {
        let top = self.strip_scroll.top_item().min(self.total().saturating_sub(1));
        if top != self.current {
            self.goto(top, cx);
        }
    }

    fn set_mode(&mut self, mode: ReaderMode, cx: &mut Context<Self>) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.transition = None;
        backend::set_config_str("readerMode", mode.config());
        if mode == ReaderMode::Strip {
            self.strip_scroll.scroll_to_top_of_item(self.current);
        }
        self.pump(cx);
        cx.notify();
    }

    fn set_fit(&mut self, fit: Fit, cx: &mut Context<Self>) {
        self.fit = fit;
        backend::set_config_str("readerFit", fit.config());
        self.page_scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    // ------------------------------------------------------------ overlay

    fn show_overlay(&mut self, cx: &mut Context<Self>) {
        self.last_activity = Instant::now();
        if !self.overlay {
            self.overlay = true;
            cx.notify();
        }
        if self.hide_task.is_none() {
            self.hide_task = Some(cx.spawn(async move |this, cx| loop {
                cx.background_executor().timer(Duration::from_millis(400)).await;
                let done = this
                    .update(cx, |t, cx| {
                        let keep = t.over_bars || t.dragging || !matches!(t.pages, Load::Ready(_)) || t.transition.is_some();
                        if keep {
                            t.last_activity = Instant::now();
                        }
                        if t.last_activity.elapsed() >= OVERLAY_IDLE {
                            t.overlay = false;
                            t.hide_task = None;
                            cx.notify();
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }));
        }
    }

    fn hide_overlay(&mut self, cx: &mut Context<Self>) {
        self.overlay = false;
        self.over_bars = false;
        self.hide_task = None;
        cx.notify();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.dragging {
            if ev.pressed_button == Some(MouseButton::Left) {
                self.slide_to(ev.position.x, cx);
            } else {
                self.dragging = false;
            }
        }
        let b = self.root_bounds.get();
        let y = ev.position.y - b.top();
        if y < px(EDGE_TOP) || y > b.size.height - px(EDGE_BOTTOM) {
            self.show_overlay(cx);
        } else if self.overlay {
            // keep the timer honest while the pointer moves around the bars
            if self.over_bars {
                self.last_activity = Instant::now();
            }
        }
    }

    /// Click on the page: left / right thirds turn pages (mirrored for right-to-left), the middle
    /// toggles the overlay. The overlay bars sit above this area and swallow their own clicks.
    fn on_page_click(&mut self, ev: &ClickEvent, cx: &mut Context<Self>) {
        let b = self.root_bounds.get();
        let zone = if self.strip() { 0 } else { logic::tap_zone(f32::from(ev.position().x - b.left()), f32::from(b.size.width), self.mode) };
        match zone {
            -1 => self.prev(cx),
            1 => self.next(cx),
            _ => {
                if self.overlay {
                    self.hide_overlay(cx)
                } else {
                    self.show_overlay(cx)
                }
            }
        }
    }

    fn slide_to(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let b = self.slider_bounds.get();
        let total = self.total();
        if total == 0 || b.size.width <= px(0.) {
            return;
        }
        let mut f = (f32::from(x - b.left()) / f32::from(b.size.width)).clamp(0., 1.);
        if self.mode == ReaderMode::Rtl {
            // right-to-left: the right end of the slider is page 1
            f = 1. - f;
        }
        let page = (f * (total - 1) as f32).round() as usize;
        if page != self.current || self.transition.is_some() {
            self.jump(page, cx);
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let rtl = self.mode == ReaderMode::Rtl;
        match k.key.as_str() {
            "escape" => mv_ui::navigate(Route::Back, window, cx),
            "right" if rtl => self.prev(cx),
            "left" if rtl => self.next(cx),
            "right" | "pagedown" | "down" => self.next(cx),
            "space" if k.modifiers.shift => self.prev(cx),
            "space" => self.next(cx),
            "left" | "pageup" | "up" => self.prev(cx),
            "home" => self.jump(0, cx),
            "end" => self.jump(self.total().saturating_sub(1), cx),
            "m" => {
                let i = ReaderMode::ALL.iter().position(|m| *m == self.mode).unwrap_or(0);
                self.set_mode(ReaderMode::ALL[(i + 1) % ReaderMode::ALL.len()], cx);
                self.show_overlay(cx);
            }
            "f" => {
                let i = Fit::ALL.iter().position(|f| *f == self.fit).unwrap_or(0);
                self.set_fit(Fit::ALL[(i + 1) % Fit::ALL.len()], cx);
                self.show_overlay(cx);
            }
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

    // ------------------------------------------------------------ views

    fn page_view(&self, i: usize, fit: Fit, strip: bool, cx: &mut Context<Self>) -> AnyElement {
        let Load::Ready(pages) = &self.pages else { return div().into_any_element() };
        match &pages[i].state {
            PageState::Ready(p, ratio) => {
                let im = img(p.clone());
                match (strip, fit) {
                    // A relative width leaves the image box at the file's own pixel height, which
                    // shows as black gaps once the page is scaled; size the box from the ratio instead.
                    (true, _) | (false, Fit::Width) => match ratio {
                        Some(r) => div().w_full().aspect_ratio(*r).child(im.size_full()).into_any_element(),
                        None => im.w_full().into_any_element(),
                    },
                    // Fit the whole page in the window: the container has a definite size
                    // (absolutely positioned), so tall pages scale down instead of being cut off.
                    (false, Fit::Screen) => im.size_full().object_fit(ObjectFit::Contain).into_any_element(),
                    (false, Fit::Original) => im.into_any_element(),
                }
            }
            PageState::Failed(e) => div()
                .id(SharedString::from(format!("retry-{i}")))
                .w_full()
                .h(px(if strip { 420. } else { 280. }))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .cursor_pointer()
                .child(icon("icons/triangle-alert.svg", Tokens::danger(), 22.))
                .child(div().text_size(px(size::MD)).text_color(Tokens::text_1()).child(format!("Page {} failed to load", i + 1)))
                .child(div().max_w(px(480.)).text_center().child(muted(e.clone())))
                .child(button(SharedString::from(format!("retry-btn-{i}")), "Retry", ButtonKind::Secondary))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.retry(i, cx)
                }))
                .into_any_element(),
            _ => div()
                .w_full()
                .h(px(if strip { STRIP_WIDTH * 1.4 } else { 280. }))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .child(spinner(SharedString::from(format!("pg-spin-{i}")), 22.))
                .child(muted(format!("Page {}", i + 1)))
                .into_any_element(),
        }
    }

    /// "Finished / Next" card between chapters (paged mode and the end of the strip).
    fn transition_card(&self, dir: i32, cx: &mut Context<Self>) -> Div {
        let cur = self.chapter.as_ref().map(logic::chapter_label).unwrap_or_default();
        let other = self.neighbour(dir).map(logic::chapter_label);
        let (k1, k2) = if dir > 0 { ("Finished", "Next") } else { ("Current", "Previous") };
        let line = |k: &'static str, v: String, strong: bool| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(k))
                .child(div().text_size(px(size::LG)).font_weight(if strong { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }).text_color(if strong { Tokens::text_1() } else { Tokens::text_2() }).child(v))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .w(px(380.))
            .p(px(24.))
            .rounded(px(14.))
            .bg(Tokens::surface())
            .border_1()
            .border_color(Tokens::border())
            .child(line(k1, cur, false))
            .child(match other {
                Some(o) => line(k2, o, true).into_any_element(),
                None => div()
                    .text_size(px(size::MD))
                    .text_color(Tokens::text_2())
                    .child(if dir > 0 { "There is no next chapter." } else { "There is no previous chapter." })
                    .into_any_element(),
            })
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .when(self.neighbour(dir).is_some(), |d| {
                        d.child(
                            button(SharedString::from(format!("tr-go-{dir}")), if dir > 0 { "Read next chapter" } else { "Read previous chapter" }, ButtonKind::Primary)
                                .occlude()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.step_chapter(dir, cx);
                                })),
                        )
                    })
                    .when(self.neighbour(dir).is_none(), |d| {
                        d.child(button("tr-back", "Back to the manga", ButtonKind::Secondary).occlude().on_click(|_, w, cx| {
                            cx.stop_propagation();
                            mv_ui::navigate(Route::Back, w, cx)
                        }))
                    }),
            )
    }

    fn top_bar(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let label = self.chapter.as_ref().map(logic::chapter_label).unwrap_or_default();
        div()
            .id("reader-top")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(56.))
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(12.))
            .bg(rgba(0x0b0d10f0))
            .border_b_1()
            .border_color(Tokens::border())
            // The bar owns every click inside it: nothing reaches the page-turn area below.
            .occlude()
            .on_hover(cx.listener(|this, h: &bool, _, _| this.over_bars = *h))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| this.on_mouse_move(ev, cx)))
            .child(back_button("reader-back", true))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(self.title.clone()))
                    .child(div().truncate().text_size(px(size::SM)).text_color(Tokens::text_3()).child(label)),
            )
            .when(self.total() > 0, |d| {
                d.child(div().flex_none().text_size(px(size::SM)).text_color(Tokens::text_2()).child(format!("{} / {}", self.current + 1, self.total())))
            })
    }

    fn bottom_bar(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let total = self.total();
        let has_prev = self.neighbour(-1).is_some();
        let has_next = self.neighbour(1).is_some();
        let frac = if total > 1 { self.current as f32 / (total - 1) as f32 } else { 1. };
        let rtl = self.mode == ReaderMode::Rtl;
        let seg = |id: &'static str, label: &'static str, active: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .h(px(26.))
                .px(px(10.))
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(px(size::XS))
                .font_weight(FontWeight::MEDIUM)
                .map(|d| if active { d.bg(Tokens::active()).text_color(Tokens::text_1()) } else { d.text_color(Tokens::text_3()).hover(|s| s.text_color(Tokens::text_1())) })
                .child(label)
        };
        let group = || div().flex().gap(px(2.)).p(px(2.)).rounded(px(8.)).bg(rgba(0x00000066)).border_1().border_color(Tokens::border());
        let chapter_btn = |id: &'static str, path: &'static str, tip: &'static str, enabled: bool| {
            div()
                .id(id)
                .size(px(34.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(999.))
                .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(Tokens::hover())))
                .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx))
                .child(icon(path, if enabled { Tokens::text_1() } else { rgba(0x7d859066) }, 18.))
        };
        let slider_bounds = self.slider_bounds.clone();
        let track = div()
            .id("slider")
            .relative()
            .flex_1()
            .h(px(24.))
            .flex()
            .items_center()
            .cursor_pointer()
            .child(bounds_probe(slider_bounds))
            .child(
                div()
                    .w_full()
                    .h(px(4.))
                    .flex()
                    .when(rtl, |d| d.justify_end())
                    .rounded(px(2.))
                    .bg(rgba(0xffffff26))
                    .child(div().h_full().rounded(px(2.)).bg(Tokens::accent()).w(relative(frac))),
            )
            .child(
                div()
                    .absolute()
                    .top(px(5.))
                    .size(px(14.))
                    .rounded(px(999.))
                    .bg(Tokens::text_1())
                    .shadow_md()
                    .map(|d| if rtl { d.right(relative(frac)).mr(px(-7.)) } else { d.left(relative(frac)).ml(px(-7.)) }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.dragging = true;
                    this.slide_to(ev.position.x, cx);
                }),
            );
        let (left_btn, right_btn) = if rtl {
            (chapter_btn("next-ch", "icons/skip-back.svg", "Next chapter (N)", has_next), chapter_btn("prev-ch", "icons/skip-forward.svg", "Previous chapter (P)", has_prev))
        } else {
            (chapter_btn("prev-ch", "icons/skip-back.svg", "Previous chapter (P)", has_prev), chapter_btn("next-ch", "icons/skip-forward.svg", "Next chapter (N)", has_next))
        };
        let (left_dir, right_dir) = if rtl { (1, -1) } else { (-1, 1) };
        let strip = self.strip();
        div()
            .id("reader-bottom")
            .absolute()
            .bottom(px(16.))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .px(px(16.))
            .child(
                div()
                    .id("reader-bottom-panel")
                    .w_full()
                    .max_w(px(760.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(px(14.))
                    .bg(rgba(0x14181cf2))
                    .border_1()
                    .border_color(Tokens::border())
                    .shadow_lg()
                    .occlude()
                    .on_hover(cx.listener(|this, h: &bool, _, _| this.over_bars = *h))
                    // the panel occludes the root, so slider drags are tracked here as well
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| this.on_mouse_move(ev, cx)))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.dragging = false))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(left_btn.on_click(cx.listener(move |this, _, _, cx| {
                                this.step_chapter(left_dir, cx);
                            })))
                            .child(div().w(px(28.)).flex_none().text_right().text_size(px(size::SM)).text_color(Tokens::text_2()).child(if total > 0 { (self.current + 1).to_string() } else { "-".into() }))
                            .child(track)
                            .child(div().w(px(28.)).flex_none().text_size(px(size::SM)).text_color(Tokens::text_2()).child(if total > 0 { total.to_string() } else { "-".into() }))
                            .child(right_btn.on_click(cx.listener(move |this, _, _, cx| {
                                this.step_chapter(right_dir, cx);
                            }))),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(group().children(ReaderMode::ALL.map(|m| {
                                let id = match m {
                                    ReaderMode::Ltr => "mode-ltr",
                                    ReaderMode::Rtl => "mode-rtl",
                                    ReaderMode::Strip => "mode-strip",
                                };
                                seg(id, m.label(), self.mode == m).on_click(cx.listener(move |this, _, _, cx| this.set_mode(m, cx)))
                            })))
                            .when(!strip, |d| {
                                d.child(group().children(Fit::ALL.map(|f| {
                                    let id = match f {
                                        Fit::Screen => "fit-screen",
                                        Fit::Width => "fit-width",
                                        Fit::Original => "fit-original",
                                    };
                                    seg(id, f.label(), self.fit == f).on_click(cx.listener(move |this, _, _, cx| this.set_fit(f, cx)))
                                })))
                            })
                            .child(div().flex_1())
                            .child(muted("Esc back  ·  M mode  ·  F fit")),
                    ),
            )
    }
}

/// Back control: reacts on mouse *down* so it can never lose the click to a re-render or to the
/// page-turn area, and stops the event from reaching anything below it.
fn back_button(id: &'static str, label: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(36.))
        .px(px(10.))
        .rounded(px(8.))
        .cursor_pointer()
        .text_size(px(size::SM))
        .font_weight(FontWeight::MEDIUM)
        .text_color(Tokens::text_1())
        .hover(|s| s.bg(Tokens::hover()))
        .occlude()
        .child(icon("icons/arrow-left.svg", Tokens::text_1(), 18.))
        .when(label, |d| d.child("Back"))
        .on_mouse_down(MouseButton::Left, |_, w, cx| {
            cx.stop_propagation();
            mv_ui::navigate(Route::Back, w, cx);
        })
}

impl Render for ReaderPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strip = self.strip();
        let body: AnyElement = match &self.pages {
            Load::Loading => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(12.))
                .child(spinner("chapter-spin", 28.))
                .child(muted("Loading chapter..."))
                .into_any_element(),
            Load::Failed(e) => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .child(icon("icons/triangle-alert.svg", Tokens::danger(), 28.))
                .child(div().text_size(px(size::LG)).text_color(Tokens::text_1()).child("Could not open this chapter"))
                .child(div().max_w(px(520.)).text_center().child(muted(e.clone())))
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(button("reload", "Try again", ButtonKind::Primary).on_click(cx.listener(|this, _, _, cx| {
                            if let Some(c) = this.chapter.as_ref().map(|c| c.id.clone()) {
                                this.open_chapter(c, false, cx);
                            }
                        })))
                        .child(button("fail-back", "Back", ButtonKind::Secondary).on_click(|_, w, cx| mv_ui::navigate(Route::Back, w, cx))),
                )
                .into_any_element(),
            Load::Ready(pages) if strip => {
                let n = pages.len();
                div()
                    .id("strip")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.strip_scroll)
                    .flex()
                    .flex_col()
                    .items_center()
                    .on_scroll_wheel(cx.listener(|_, _, window, cx| {
                        // after the scroll has been applied
                        cx.defer_in(window, |this, _, cx| this.sync_strip_page(cx));
                    }))
                    .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| this.on_page_click(ev, cx)))
                    // one child per page, so `top_item` / `scroll_to_top_of_item` map to pages
                    .children((0..n).map(|i| div().w_full().max_w(px(STRIP_WIDTH)).flex_none().child(self.page_view(i, Fit::Width, true, cx))))
                    .child(div().flex_none().flex().justify_center().py(px(48.)).pb(px(160.)).child(self.transition_card(1, cx)))
                    .into_any_element()
            }
            Load::Ready(_) => {
                let content: AnyElement = match self.transition {
                    Some(dir) => div().size_full().flex().items_center().justify_center().child(self.transition_card(dir, cx)).into_any_element(),
                    None => {
                        let page = self.page_view(self.current, self.fit, false, cx);
                        match self.fit {
                            Fit::Screen => div().size_full().child(page).into_any_element(),
                            Fit::Width => div().flex_none().w_full().max_w(px(1100.)).child(page).into_any_element(),
                            Fit::Original => div().flex_none().child(page).into_any_element(),
                        }
                    }
                };
                div()
                    .id("paged")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .when(self.fit != Fit::Screen || self.transition.is_some(), |d| d.overflow_y_scroll())
                    .track_scroll(&self.page_scroll)
                    .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| this.on_page_click(ev, cx)))
                    .child(content)
                    .into_any_element()
            }
        };
        let overlay = self.overlay;
        div()
            .id("reader")
            .key_context("Reader")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| this.on_key(ev, w, cx)))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| this.on_mouse_move(ev, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.dragging = false))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, _| this.dragging = false))
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(rgb(0x000000))
            .child(bounds_probe(self.root_bounds.clone()))
            .child(body)
            .when(overlay, |d| d.child(self.top_bar(cx)).child(self.bottom_bar(cx)))
            // With the overlay hidden a small back button stays in the corner for mouse users.
            .when(!overlay, |d| d.child(div().absolute().top(px(10.)).left(px(10.)).opacity(0.55).hover(|s| s.opacity(1.)).rounded(px(8.)).bg(rgba(0x000000a0)).child(back_button("reader-back-mini", false))))
    }
}
