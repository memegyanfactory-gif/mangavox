use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::chapter::{self, Chapter};
use mv_core::repo::manga;
use mv_ui::theme::{icon as icon_px, radius, ramp};
use mv_ui::{
    btn, error_view, glyphs, icon_button, icon_ph, icon_tag, kbd, muted, ph, ph_icon, progress_ring_sized, segmented, shimmer_text, size, spinner, tip, tip_kbd, ButtonKind, Ph,
    RampExt, Route, Tokens, Tone, Weight,
};

use mv_ui::Interactive as _;

use crate::backend;
use crate::downloads::{downloads, ChapterDl, Downloads, Job};
use crate::logic::{self, Fit, ReaderMode};
use crate::narrator::Narrator;
use crate::recap::{self, Narration, RecapOutputs};
use crate::store;
use crate::ui::{fill_menu, icon_trigger, menu_trigger, Entry, Load};

const MAX_INFLIGHT: usize = 3;
const STRIP_WIDTH: f32 = 860.0;
/// Widest a fit-width page gets in paged mode.
const PAGED_WIDTH: f32 = 1100.0;
/// Overlay hides after this long without activity near the bars.
const OVERLAY_IDLE: Duration = Duration::from_millis(2600);
/// Mouse within this distance of the top / bottom edge reveals the overlay.
const EDGE_TOP: f32 = 72.0;
const EDGE_BOTTOM: f32 = 140.0;
/// Paged-mode zoom range and button step.
const MAX_ZOOM: f32 = 4.0;
const ZOOM_STEP: f32 = 1.25;
/// Auto-scroll frame time (about 60 fps, smooth enough for a slow strip scroll).
const AUTO_FRAME: Duration = Duration::from_millis(16);
/// Auto-scroll waits this long at the end of a strip or a tall page before moving on.
const AUTO_END_HOLD: f32 = 1.5;
/// Read aloud follows the narration again this long after the user last scrolled by hand.
const REFOLLOW: Duration = Duration::from_secs(6);

/// Chapter whose reader starts reading aloud as soon as it opens (set by the manga page).
static PENDING_ALOUD: Mutex<Option<String>> = Mutex::new(None);

/// Opens a chapter in the reader with Studio's narration playing.
pub(crate) fn open_read_aloud(manga_id: String, chapter_id: String, window: &mut Window, cx: &mut App) {
    *PENDING_ALOUD.lock().unwrap_or_else(|e| e.into_inner()) = Some(chapter_id.clone());
    mv_ui::navigate(Route::Reader { manga_id, chapter_id }, window, cx);
}

/// Plays a chapter's recap video in the app's player.
pub(crate) fn watch_recap(title: String, video: &Path, window: &mut Window, cx: &mut App) {
    let request = mv_ui::nav::PlayRequest { title, source: video.display().to_string(), file_idx: None, imdb_id: None, mal_id: None, season: None, episode: None };
    mv_ui::navigate(Route::Player { request }, window, cx);
}

/// Read aloud: Studio's narration plays and the reader follows the passage being read.
struct Aloud {
    narration: Narration,
    player: Narrator,
    /// Page file names in the reader's order (to match Studio's pages).
    files: Vec<String>,
    /// The reader follows the narration; scrolling by hand lets go of it for a while.
    follow: bool,
    touched: Instant,
    _task: Task<()>,
}

#[derive(Clone)]
enum PageState {
    Idle,
    Loading,
    /// The file and its pixel size (when the header could be read).
    Ready(PathBuf, Option<(f32, f32)>),
    Failed(String),
}

/// Pixel width and height of an image file, read from its header only.
fn dims(path: &Path) -> Option<(f32, f32)> {
    let s = imagesize::size(path).ok()?;
    (s.width > 0 && s.height > 0).then_some((s.width as f32, s.height as f32))
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
    recap: Option<RecapOutputs>,
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
    let (m, chapters, ch, alternates, recap) = mv_ui::bg::blocking(move || {
        backend::with_db(|c| {
            let ch = chapter::get(c, &cid)?.ok_or_else(|| anyhow!("Chapter not found"))?;
            let m = manga::get(c, &ch.manga_id)?.ok_or_else(|| anyhow!("Manga not found"))?;
            let all = chapter::list(c, &m.id)?;
            let mut list = store::chapters(c, &m.id, &lang)?;
            // The open chapter is always part of the list, even in another language.
            if !list.iter().any(|x| x.id == ch.id) {
                list = all.clone();
            }
            let alternates = backend::alternate_sources(&all, &ch);
            let recap = recap::find(&m.id, &ch.id);
            Ok((m, list, ch, alternates, recap))
        })
    })
    .await??;
    let mut source = if ch.source_plugin.is_empty() { m.source_plugin.clone() } else { ch.source_plugin.clone() };
    let local = local_pages(&ch);
    let pages: Vec<PageEntry> = if !local.is_empty() {
        local.into_iter().map(|p| PageEntry { url: String::new(), state: PageState::Ready(p.clone(), dims(&p)) }).collect()
    } else {
        let tries = std::iter::once((source.clone(), ch.source_id.clone())).chain(alternates).collect();
        let (used, urls) = backend::page_urls(tries).await?;
        source = used;
        urls.into_iter().map(|url| PageEntry { url, state: PageState::Idle }).collect()
    };
    let start = if at_end {
        pages.len().saturating_sub(1)
    } else if ch.read_progress > 0 && !ch.is_read {
        logic::clamp_page(ch.read_progress, pages.len())
    } else {
        0
    };
    Ok(Loaded { manga_title: m.title, source, chapters, chapter: ch, pages, start, recap })
}

/// Records an element's window bounds during prepaint (for click / drag math).
fn bounds_probe(cell: Rc<Cell<Bounds<Pixels>>>) -> impl IntoElement {
    canvas(move |b, _, _| cell.set(b), |_, _, _, _| {}).absolute().top_0().left_0().size_full()
}

fn mode_icon(m: ReaderMode) -> Ph {
    match m {
        ReaderMode::Ltr => ph::ARROW_LINE_RIGHT,
        ReaderMode::Rtl => ph::ARROW_LINE_LEFT,
        ReaderMode::Strip => ph::ROWS,
    }
}

fn fit_icon(f: Fit) -> Ph {
    match f {
        Fit::Screen => ph::FRAME_CORNERS,
        Fit::Width => ph::ARROWS_LEFT_RIGHT,
        Fit::Original => ph::ARROWS_OUT_SIMPLE,
    }
}

/// Every reader shortcut, for the help panel (key, what it does).
const SHORTCUTS: &[(&str, &str)] = &[
    ("right", "Next page (previous in right-to-left)"),
    ("left", "Previous page (next in right-to-left)"),
    ("space", "Next page; Shift+Space goes back"),
    ("home", "First page"),
    ("end", "Last page"),
    ("n", "Next chapter"),
    ("p", "Previous chapter"),
    ("a", "Start or stop auto-scroll"),
    ("r", "Read aloud (chapters Studio narrated)"),
    ("k", "Pause or play the narration"),
    ("[", "Auto-scroll slower"),
    ("]", "Auto-scroll faster"),
    ("m", "Change reading mode"),
    ("f", "Change page fit"),
    ("=", "Zoom in (or Ctrl + mouse wheel, or double-click the middle)"),
    ("-", "Zoom out"),
    ("0", "Reset zoom"),
    ("f11", "Full screen"),
    ("?", "Show these shortcuts"),
    ("escape", "Close, unzoom or go back"),
];

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
    /// Paged mode scroll (fit width / original size / zoomed pages can be larger than the window).
    page_scroll: ScrollHandle,
    overlay: bool,
    last_activity: Instant,
    over_bars: bool,
    hide_task: Option<Task<()>>,
    root_bounds: Rc<Cell<Bounds<Pixels>>>,
    slider_bounds: Rc<Cell<Bounds<Pixels>>>,
    dragging: bool,
    /// Paged-mode zoom, 1 = the chosen fit.
    zoom: f32,
    /// Drag-to-pan while zoomed: pointer position and scroll offset at the press.
    pan: Option<(Point<Pixels>, Point<Pixels>)>,
    /// The press became a drag, so its click must not turn the page.
    panned: bool,
    show_keys: bool,
    /// The chapter list menu is open (its pointer is outside the bars, so they must not hide).
    menu_open: bool,
    /// Keyboard focus is on a control inside the reader (a bar button reached with Tab); hiding
    /// the bars would drop that focus and with it every reader shortcut.
    focus_inside: bool,
    queue: Entity<Downloads>,
    /// The open chapter was queued or downloading at the last queue change.
    dl_pending: bool,
    /// Auto-scroll is on: the strip (or a tall page) scrolls by itself and pages turn on a timer.
    auto: bool,
    /// Seconds per screen (long strip, tall pages) or per page (paged modes).
    auto_secs: f32,
    auto_task: Option<Task<()>>,
    /// Paged: time on the current page; strip: time held at the end of the chapter.
    auto_elapsed: f32,
    /// The custom timer field, while it is open.
    auto_custom: Option<(Entity<InputState>, Subscription)>,
    /// The chapter the reader was opened on (its route, to tell when it is the page on screen).
    route_chapter: String,
    /// Studio's recap of the open chapter, when it made one.
    recap: Option<RecapOutputs>,
    aloud: Option<Aloud>,
    aloud_loading: bool,
    aloud_bar: Rc<Cell<Bounds<Pixels>>>,
    _watch: Subscription,
}

impl ReaderPage {
    pub fn new(manga_id: String, chapter_id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let queue = downloads(cx);
        let watch = cx.observe(&queue, |this, q, cx| {
            let pending = this.chapter.as_ref().is_some_and(|c| q.read(cx).status(&c.id).is_some());
            if this.dl_pending && !pending {
                this.refresh_download_state(cx);
            }
            this.dl_pending = pending;
            cx.notify();
        });
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
            zoom: 1.0,
            pan: None,
            panned: false,
            show_keys: false,
            menu_open: false,
            focus_inside: false,
            queue,
            dl_pending: false,
            auto: false,
            auto_secs: backend::config_str("readerAutoScrollSecs").and_then(|s| logic::parse_auto_secs(&s)).unwrap_or(logic::AUTO_DEFAULT),
            auto_task: None,
            auto_elapsed: 0.,
            auto_custom: None,
            route_chapter: chapter_id.clone(),
            recap: None,
            aloud: None,
            aloud_loading: false,
            aloud_bar: Rc::new(Cell::new(Bounds::default())),
            _watch: watch,
        };
        p.open_chapter(chapter_id, false, cx);
        p.show_overlay(cx);
        p
    }

    fn strip(&self) -> bool {
        self.mode == ReaderMode::Strip
    }

    /// Kit controls do not take focus on a mouse press, so every bar control hands it back to
    /// the reader explicitly to keep its keyboard shortcuts working.
    fn refocus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
    }

    fn open_chapter(&mut self, chapter_id: String, at_end: bool, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.pages = Load::Loading;
        self.inflight = 0;
        self.current = 0;
        self.transition = None;
        self.zoom = 1.0;
        // The narration belongs to the chapter that was open.
        self.aloud = None;
        self.recap = None;
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
                        this.dl_pending = this.queue.read(cx).status(&l.chapter.id).is_some();
                        let pending = {
                            let mut p = PENDING_ALOUD.lock().unwrap_or_else(|e| e.into_inner());
                            if p.as_deref() == Some(l.chapter.id.as_str()) {
                                p.take().is_some()
                            } else {
                                false
                            }
                        };
                        this.recap = l.recap;
                        this.chapter = Some(l.chapter);
                        this.pages = Load::Ready(l.pages);
                        this.page_scroll.set_offset(point(px(0.), px(0.)));
                        this.strip_scroll.set_offset(point(px(0.), px(0.)));
                        if this.current > 0 {
                            this.strip_scroll.scroll_to_top_of_item(this.current);
                        }
                        this.save(cx);
                        this.pump(cx);
                        if pending {
                            this.start_aloud(cx);
                        }
                    }
                    Err(e) => this.pages = Load::Failed(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-read the open chapter's download state after its download finished.
    fn refresh_download_state(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.chapter.as_ref().map(|c| c.id.clone()) else { return };
        cx.spawn(async move |this, cx| {
            let cid = id.clone();
            let fresh = mv_ui::bg::blocking(move || backend::with_db(|c| chapter::get(c, &cid))).await.ok().and_then(|r| r.ok()).flatten();
            let _ = this.update(cx, |this, cx| {
                if let (Some(f), Some(c)) = (fresh, this.chapter.as_mut().filter(|c| c.id == id)) {
                    c.download_status = f.download_status;
                    c.pages_dir = f.pages_dir;
                    cx.notify();
                }
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
                let res = mv_ui::bg::run(async move { backend::download_page(url, Some(src)).await.map(|p| { let d = dims(&p); (p, d) }) }).await.and_then(|r| r);
                let _ = this.update(cx, |this, cx| {
                    if this.generation != generation {
                        return;
                    }
                    this.inflight = this.inflight.saturating_sub(1);
                    if let Load::Ready(pages) = &mut this.pages {
                        pages[i].state = match res {
                            Ok((p, d)) => PageState::Ready(p, d),
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
        self.zoom = 1.0;
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
        self.aloud_from_page(cx);
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
            self.aloud_from_page(cx);
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
            self.aloud_from_page(cx);
        } else {
            self.transition = Some(-1);
            cx.notify();
        }
    }

    fn scroll_by(&mut self, viewport_fraction: f32, cx: &mut Context<Self>) {
        self.aloud_touched();
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
        self.zoom = 1.0;
        backend::set_config_str("readerMode", mode.config());
        if mode == ReaderMode::Strip {
            self.strip_scroll.scroll_to_top_of_item(self.current);
        }
        self.pump(cx);
        cx.notify();
    }

    fn set_fit(&mut self, fit: Fit, cx: &mut Context<Self>) {
        self.fit = fit;
        self.zoom = 1.0;
        backend::set_config_str("readerFit", fit.config());
        self.page_scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    // ------------------------------------------------------------ zoom

    /// Size of the current page box at zoom 1 for the chosen fit (window pixels).
    fn zoom_base(&self) -> (f32, f32) {
        let b = self.root_bounds.get();
        let (vw, vh) = (f32::from(b.size.width).max(1.), f32::from(b.size.height).max(1.));
        let px_dims = match &self.pages {
            Load::Ready(p) => match p.get(self.current).map(|e| &e.state) {
                Some(PageState::Ready(_, d)) => *d,
                _ => None,
            },
            _ => None,
        };
        match (self.fit, px_dims) {
            (Fit::Screen, _) => (vw, vh),
            (Fit::Width, Some((w, h))) => {
                let bw = vw.min(PAGED_WIDTH);
                (bw, bw * h / w)
            }
            (Fit::Width, None) => (vw.min(PAGED_WIDTH), vh),
            (Fit::Original, Some(d)) => d,
            (Fit::Original, None) => (vw, vh),
        }
    }

    /// Zoom paged mode to `z`, keeping the point under `anchor` (window coordinates; the centre
    /// when `None`) where it is. Mihon reader settings (double tap to zoom): https://mihon.app/docs/guides/reader-settings
    fn set_zoom(&mut self, z: f32, anchor: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let z = z.clamp(1., MAX_ZOOM);
        if self.strip() || self.transition.is_some() || (z - self.zoom).abs() < 0.001 {
            return;
        }
        let b = self.root_bounds.get();
        let p = anchor.map(|a| a - b.origin).unwrap_or(point(b.size.width / 2., b.size.height / 2.));
        let (bw, bh) = self.zoom_base();
        let (z0, off) = (self.zoom, self.page_scroll.offset());
        let axis = |p: Pixels, off: Pixels, base: f32, view: Pixels| -> Pixels {
            let (p, off, v) = (f32::from(p), f32::from(off), f32::from(view));
            let (s0, s1) = (base * z0, base * z);
            let (o0, o1) = (((v - s0) / 2.).max(0.), ((v - s1) / 2.).max(0.));
            let c = (p - off - o0) / z0;
            px((p - o1 - c * z).min(0.).max(-(s1 - v).max(0.)))
        };
        self.page_scroll.set_offset(point(axis(p.x, off.x, bw, b.size.width), axis(p.y, off.y, bh, b.size.height)));
        self.zoom = z;
        cx.notify();
    }

    fn zoom_by(&mut self, factor: f32, anchor: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let z = self.zoom * factor;
        // Snap back to exactly 1 near the bottom of the range.
        self.set_zoom(if z < 1.05 { 1. } else { z }, anchor, cx);
    }

    // ------------------------------------------------------------ auto-scroll

    fn toggle_auto(&mut self, cx: &mut Context<Self>) {
        if self.auto {
            self.stop_auto(None, cx);
        } else {
            self.start_auto(cx);
        }
    }

    fn start_auto(&mut self, cx: &mut Context<Self>) {
        if self.auto {
            return;
        }
        // The timer and the narration would fight over the scroll.
        self.aloud = None;
        self.auto = true;
        self.auto_elapsed = 0.;
        // Reading starts: the bars get out of the way.
        self.hide_overlay(cx);
        self.auto_task = Some(cx.spawn(async move |this, cx| {
            let mut last = Instant::now();
            loop {
                cx.background_executor().timer(AUTO_FRAME).await;
                let now = Instant::now();
                // A stalled frame (window dragged, PC busy) must not jump a whole screen.
                let dt = now.duration_since(last).as_secs_f32().min(0.1);
                last = now;
                if this.update(cx, |t, cx| t.auto_tick(dt, cx)).is_err() {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn stop_auto(&mut self, reason: Option<&str>, cx: &mut Context<Self>) {
        self.auto = false;
        self.auto_task = None;
        self.auto_elapsed = 0.;
        if let Some(r) = reason {
            mv_ui::toast::show(cx, mv_ui::toast::Toast::info(r.to_string()).icon(ph::PAUSE_CIRCLE));
        }
        cx.notify();
    }

    fn set_auto_secs(&mut self, secs: f32, cx: &mut Context<Self>) {
        self.auto_secs = logic::clamp_auto_secs(secs);
        self.auto_elapsed = self.auto_elapsed.min(self.auto_secs);
        backend::set_config_str("readerAutoScrollSecs", &self.auto_secs.to_string());
        cx.notify();
    }

    /// Opens the field for a timer of your own (seconds, or minutes:seconds).
    fn open_custom_timer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.auto_secs.to_string();
        let input = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("Seconds, e.g. 6 or 1:30");
            s.set_value(current, window, cx);
            s
        });
        let sub = cx.subscribe_in(&input, window, |this: &mut Self, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.apply_custom_timer(window, cx);
            }
        });
        window.focus(&input.focus_handle(cx), cx);
        self.auto_custom = Some((input, sub));
        self.show_overlay(cx);
        cx.notify();
    }

    fn close_custom_timer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.auto_custom = None;
        self.refocus(window, cx);
        cx.notify();
    }

    fn apply_custom_timer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((input, _)) = &self.auto_custom else { return };
        let text = input.read(cx).value().to_string();
        match logic::parse_auto_secs(&text) {
            Some(s) => {
                self.close_custom_timer(window, cx);
                self.set_auto_secs(s, cx);
                self.start_auto(cx);
            }
            None => mv_ui::toast::show(cx, mv_ui::toast::Toast::warning("Type a time in seconds, like 6, 7.5 or 1:30")),
        }
    }

    /// Something else has the reader's attention: the timer waits.
    fn auto_paused(&self) -> bool {
        self.dragging || self.menu_open || self.show_keys || self.auto_custom.is_some() || self.pan.is_some() || !matches!(self.pages, Load::Ready(_)) || (!self.strip() && self.zoom > 1.)
    }

    /// One auto-scroll frame. Long strip: one screen per `auto_secs`, then on to the next
    /// chapter. Paged: a page taller than the window scrolls the same way first; a page that fits
    /// turns after `auto_secs`.
    fn auto_tick(&mut self, dt: f32, cx: &mut Context<Self>) {
        if self.auto_paused() {
            return;
        }
        let strip = self.strip();
        let handle = if strip { self.strip_scroll.clone() } else { self.page_scroll.clone() };
        let h = f32::from(handle.bounds().size.height);
        let max = f32::from(handle.max_offset().y);
        let y = -f32::from(handle.offset().y);
        let scrolls = (strip || self.transition.is_none()) && max > 1. && h > 0.;
        if scrolls && y < max - 0.5 {
            let o = handle.offset();
            let ny = (y + logic::auto_step(h, self.auto_secs, dt)).min(max);
            handle.set_offset(point(o.x, px(-ny)));
            if strip {
                self.sync_strip_page(cx);
            }
            cx.notify();
            return;
        }
        // At the end of the strip or the page: wait, then move on.
        self.auto_elapsed += dt;
        let wait = if scrolls { AUTO_END_HOLD.min(self.auto_secs) } else if strip { AUTO_END_HOLD } else { self.auto_secs };
        if self.auto_elapsed < wait {
            if !strip {
                // the countdown line under the page
                cx.notify();
            }
            return;
        }
        self.auto_elapsed = 0.;
        if strip || self.transition == Some(1) {
            if !self.step_chapter(1, cx) {
                self.stop_auto(Some("Auto-scroll stopped: this is the last chapter"), cx);
            }
            return;
        }
        self.next(cx);
    }

    fn auto_control(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.auto;
        let secs = self.auto_secs;
        let unit = if self.strip() { "per screen" } else { "per page" };
        let toggle = icon_button("auto-scroll", if on { ph::PAUSE } else { ph::PLAY }, if on { "Stop auto-scroll" } else { "Auto-scroll" }).small().selected(on).kbd("a").on_click(cx.listener(|this, _, w, cx| {
            this.refocus(w, cx);
            this.toggle_auto(cx);
        }));
        let weak = cx.entity().downgrade();
        let mut entries: Vec<Entry> = vec![Entry::Heading(format!("Auto-scroll timer · {unit}").into())];
        for (name, p) in ["Fast", "Quick", "Normal", "Relaxed", "Slow"].into_iter().zip(logic::AUTO_PRESETS) {
            let w = weak.clone();
            entries.push(
                Entry::new(name, move |window, cx| {
                    let _ = w.update(cx, |t, cx| {
                        t.set_auto_secs(p, cx);
                        t.refocus(window, cx);
                        t.start_auto(cx);
                    });
                })
                .hint(logic::fmt_auto_secs(p))
                .checked((secs - p).abs() < 0.01),
            );
        }
        let custom = !logic::AUTO_PRESETS.iter().any(|p| (secs - p).abs() < 0.01);
        let w = weak.clone();
        entries.push(Entry::Separator);
        let custom_entry = Entry::new("Custom timer…", move |window, cx| {
            let _ = w.update(cx, |t, cx| t.open_custom_timer(window, cx));
        })
        .checked(custom);
        entries.push(if custom { custom_entry.hint(logic::fmt_auto_secs(secs)) } else { custom_entry });
        let w = cx.entity().downgrade();
        let accent = if on { Tokens::accent() } else { Tokens::text_2() };
        let menu = menu_trigger(
            div()
                .id("auto-speed")
                .flex()
                .flex_none()
                .items_center()
                .gap(px(4.))
                .h(px(28.))
                .px(px(8.))
                .rounded(px(radius::MD))
                .text_size(px(size::SM))
                .text_color(accent)
                .hover(|s| s.bg(Tokens::hover()))
                .pressable()
                .child(icon_ph(ph::TIMER, accent, icon_px::SM))
                .child(logic::fmt_auto_secs(secs))
                .child(icon_ph(ph::CARET_UP, Tokens::text_3(), icon_px::XS)),
            format!("Auto-scroll timer: {} {unit}", logic::fmt_auto_secs(secs)),
        )
        .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| fill_menu(menu.min_w(px(240.)), &entries))
        .on_open_change(move |open, window, cx| {
            let _ = w.update(cx, |t, cx| {
                t.menu_open = *open;
                if *open {
                    t.show_overlay(cx);
                } else if t.auto_custom.is_none() {
                    t.refocus(window, cx);
                }
            });
        });
        let field = self.auto_custom.as_ref().map(|(input, _)| {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(div().w(px(150.)).child(Input::new(input).small()))
                .child(btn("auto-custom-ok", "Set", ButtonKind::Primary).small().on_click(cx.listener(|this, _, w, cx| this.apply_custom_timer(w, cx))))
                .child(icon_button("auto-custom-x", ph::X, "Cancel").small().on_click(cx.listener(|this, _, w, cx| this.close_custom_timer(w, cx))))
        });
        div().flex().items_center().gap(px(2.)).child(toggle).child(menu).children(field)
    }

    // ------------------------------------------------------------ read aloud

    fn toggle_aloud(&mut self, cx: &mut Context<Self>) {
        if self.aloud.is_some() {
            self.aloud = None;
            cx.notify();
        } else {
            self.start_aloud(cx);
        }
    }

    /// Loads the chapter's narration and starts it at the page on screen.
    fn start_aloud(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.recap.as_ref().filter(|r| r.can_read_aloud()).map(|r| r.dir.clone()) else { return };
        if self.aloud.is_some() || self.aloud_loading {
            return;
        }
        self.aloud_loading = true;
        let generation = self.generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || Narration::load(&dir).map(|n| (n, dir.join("audio")))).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.aloud_loading = false;
                if this.generation == generation {
                    match res {
                        Ok((n, audio)) => this.begin_aloud(n, audio, cx),
                        Err(e) => mv_ui::toast::show(cx, mv_ui::toast::Toast::error(format!("Could not read this chapter aloud: {e:#}"))),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn begin_aloud(&mut self, n: Narration, audio: PathBuf, cx: &mut Context<Self>) {
        let Load::Ready(pages) = &self.pages else { return };
        let files: Vec<String> = pages
            .iter()
            .map(|p| match &p.state {
                PageState::Ready(path, _) => path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default(),
                _ => String::new(),
            })
            .collect();
        self.stop_auto(None, cx);
        // From the start, or from the page the reader is on.
        let at = if self.current == 0 { 0 } else { n.page_start(n.studio_page(self.current, &files)).unwrap_or(0) };
        let player = Narrator::start(audio, n.clips.clone(), at);
        let task = cx.spawn(async move |this, cx| {
            let mut last = Instant::now();
            loop {
                cx.background_executor().timer(AUTO_FRAME).await;
                let now = Instant::now();
                let dt = now.duration_since(last).as_secs_f32().min(0.1);
                last = now;
                if this.update(cx, |t, cx| t.aloud_tick(dt, cx)).is_err() {
                    break;
                }
            }
        });
        self.aloud = Some(Aloud { narration: n, player, files, follow: true, touched: Instant::now(), _task: task });
        self.transition = None;
        self.hide_overlay(cx);
    }

    /// The reader is the page on screen (it stays alive in history after the user leaves).
    fn on_screen(&self, cx: &App) -> bool {
        matches!(mv_ui::current_route(cx), Some(Route::Reader { manga_id, chapter_id }) if manga_id == self.manga_id && chapter_id == self.route_chapter)
    }

    /// Something else has the reader's attention: following waits.
    fn aloud_held(&self) -> bool {
        self.dragging || self.menu_open || self.pan.is_some() || self.zoom > 1. || self.show_keys || !matches!(self.pages, Load::Ready(_))
    }

    /// One Read aloud frame: keep the passage being read on screen.
    fn aloud_tick(&mut self, dt: f32, cx: &mut Context<Self>) {
        let here = self.on_screen(cx);
        let held = self.aloud_held();
        let Some(a) = &mut self.aloud else { return };
        if a.player.failed() {
            self.aloud = None;
            mv_ui::toast::show(cx, mv_ui::toast::Toast::error("Could not play the narration. Check that a speaker or headphones are connected."));
            cx.notify();
            return;
        }
        if !here {
            // Left the reader: the narration must not keep talking from history.
            if a.player.playing() {
                a.player.pause();
            }
            return;
        }
        if !a.follow && a.touched.elapsed() >= REFOLLOW {
            a.follow = true;
        }
        let target = (a.follow && !held).then(|| a.narration.focus(a.player.position())).flatten().map(|(p, y)| (a.narration.reader_page(p, &a.files), y));
        let playing = a.player.playing();
        if let Some((page, frac)) = target {
            self.follow_to(page, frac, dt, cx);
        }
        if playing {
            cx.notify();
        }
    }

    /// Eases the view towards point `frac` (0..1 of its height) of reader page `page`.
    fn follow_to(&mut self, page: usize, frac: f32, dt: f32, cx: &mut Context<Self>) {
        let (handle, top, page_h) = if self.strip() {
            let h = self.strip_scroll.clone();
            let Some(item) = h.bounds_for_item(page) else { return };
            // Child bounds are laid out without the scroll offset.
            (h.clone(), f32::from(item.origin.y - h.bounds().origin.y), f32::from(item.size.height))
        } else {
            if self.transition.is_some() || page != self.current {
                self.goto(page, cx);
            }
            let h = self.page_scroll.clone();
            let content = f32::from(h.max_offset().y + h.bounds().size.height);
            (h, 0., content)
        };
        let (view_h, max) = (f32::from(handle.bounds().size.height), f32::from(handle.max_offset().y));
        if view_h <= 0. || page_h <= 0. {
            return;
        }
        let o = handle.offset();
        let cur = -f32::from(o.y);
        let next = recap::ease(cur, recap::scroll_target(top, page_h, view_h, frac).clamp(0., max.max(0.)), dt);
        if (next - cur).abs() > 0.01 {
            handle.set_offset(point(o.x, px(-next)));
            if self.strip() {
                self.sync_strip_page(cx);
            }
            cx.notify();
        }
    }

    /// The user scrolled by hand: stop following for a while (the narration plays on).
    fn aloud_touched(&mut self) {
        if let Some(a) = &mut self.aloud {
            a.follow = false;
            a.touched = Instant::now();
        }
    }

    /// The user turned or jumped to a page: the narration moves there too.
    fn aloud_from_page(&mut self, cx: &mut Context<Self>) {
        let current = self.current;
        let Some(a) = &mut self.aloud else { return };
        if let Some(ms) = a.narration.page_start(a.narration.studio_page(current, &a.files)) {
            a.player.seek(ms);
        }
        a.follow = true;
        cx.notify();
    }

    fn aloud_seek(&mut self, ms: u64, cx: &mut Context<Self>) {
        if let Some(a) = &mut self.aloud {
            a.player.seek(ms.min(a.narration.total_ms));
            a.follow = true;
            cx.notify();
        }
    }

    /// Previous (-1) or next (1) passage; back restarts the passage when it is under way.
    fn aloud_step(&mut self, dir: i32, cx: &mut Context<Self>) {
        let Some(a) = &self.aloud else { return };
        let pos = a.player.position();
        let beats = &a.narration.beats;
        let Some(i) = a.narration.beat_at(pos) else { return };
        let ms = if dir > 0 {
            match beats.get(i + 1) {
                Some(b) => b.start_ms,
                None => return,
            }
        } else if pos > beats[i].start_ms + 2000 || i == 0 {
            beats[i].start_ms
        } else {
            beats[i - 1].start_ms
        };
        self.aloud_seek(ms, cx);
    }

    fn aloud_play_pause(&mut self, cx: &mut Context<Self>) {
        if let Some(a) = &mut self.aloud {
            if a.player.playing() {
                a.player.pause();
            } else {
                a.player.play();
                a.follow = true;
            }
            cx.notify();
        }
    }

    /// Recap actions in the top bar: watch Studio's video, read the chapter aloud.
    fn recap_buttons(&self, cx: &mut Context<Self>) -> Option<Div> {
        let r = self.recap.as_ref()?;
        let video = r.video.clone();
        let title = format!("{} · {}", self.title, self.chapter.as_ref().map(logic::chapter_label).unwrap_or_default());
        let on = self.aloud.is_some();
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(4.))
                .when_some(video, |d, v| {
                    d.child(btn("recap-watch", "Watch recap", ButtonKind::Ghost).small().icon(ph::MONITOR_PLAY).tooltip("Play the recap video Studio made of this chapter").on_click(move |_, w, cx| {
                        watch_recap(title.clone(), &v, w, cx);
                    }))
                })
                .when(r.can_read_aloud(), |d| {
                    d.child(
                        btn("recap-aloud", if on { "Stop reading" } else { "Read aloud" }, if on { ButtonKind::Secondary } else { ButtonKind::Primary })
                            .small()
                            .icon(if on { ph::SPEAKER_X } else { ph::SPEAKER_HIGH })
                            .loading(self.aloud_loading)
                            .kbd("r")
                            .tooltip("Play the chapter's narration while the pages follow along")
                            .on_click(cx.listener(|this, _, w, cx| {
                                this.refocus(w, cx);
                                this.toggle_aloud(cx);
                            })),
                    )
                }),
        )
    }

    /// The Read aloud player: stays on screen while the bars hide.
    fn aloud_player(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let a = self.aloud.as_ref()?;
        let (pos, total) = (a.player.position(), a.narration.total_ms.max(1));
        let ended = a.player.ended();
        let playing = a.player.playing();
        let caption = a.narration.beat_at(pos).map(|i| a.narration.beats[i].text.clone()).unwrap_or_default();
        let frac = (pos as f32 / total as f32).clamp(0., 1.);
        let bar = self.aloud_bar.clone();
        let follow = a.follow;
        let play_icon = if ended {
            ph::ARROW_COUNTER_CLOCKWISE
        } else if playing {
            ph::PAUSE
        } else {
            ph::PLAY
        };
        let play_tip = if ended {
            "Play again"
        } else if playing {
            "Pause"
        } else {
            "Play"
        };
        Some(
            div()
                .id("aloud")
                .absolute()
                .left_0()
                .right_0()
                .bottom(px(if self.overlay { 140. } else { 16. }))
                .flex()
                .justify_center()
                .px(px(16.))
                .child(
                    div()
                        .id("aloud-panel")
                        .w_full()
                        .max_w(px(720.))
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .px(px(12.))
                        .py(px(8.))
                        .rounded(px(radius::XL))
                        .bg(Tokens::glass_strong())
                        .border_1()
                        .border_color(Tokens::glass_border())
                        .shadow_lg()
                        .occlude()
                        .on_hover(cx.listener(|this, h: &bool, _, _| this.over_bars = *h))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(ph_icon(ph::WAVEFORM, Weight::Bold, Tokens::accent(), icon_px::MD))
                                .child(icon_button("aloud-prev", ph::SKIP_BACK, "Previous passage").small().on_click(cx.listener(|this, _, w, cx| {
                                    this.refocus(w, cx);
                                    this.aloud_step(-1, cx);
                                })))
                                .child(icon_button("aloud-play", play_icon, play_tip).kbd("k").selected(playing).on_click(cx.listener(|this, _, w, cx| {
                                    this.refocus(w, cx);
                                    this.aloud_play_pause(cx);
                                })))
                                .child(icon_button("aloud-next", ph::SKIP_FORWARD, "Next passage").small().on_click(cx.listener(|this, _, w, cx| {
                                    this.refocus(w, cx);
                                    this.aloud_step(1, cx);
                                })))
                                .child(div().flex_none().w(px(44.)).text_right().text_size(px(size::SM)).text_color(Tokens::text_2()).child(recap::fmt_ms(pos)))
                                .child(
                                    div()
                                        .id("aloud-bar")
                                        .relative()
                                        .flex_1()
                                        .h(px(20.))
                                        .flex()
                                        .items_center()
                                        .cursor_pointer()
                                        .child(bounds_probe(bar.clone()))
                                        .child(div().w_full().h(px(4.)).rounded_full().bg(Tokens::track()).child(div().h_full().rounded_full().bg(Tokens::accent()).w(relative(frac))))
                                        // A whole click (not a press), so a press that changed the
                                        // layout elsewhere can never land here as a seek.
                                        .on_click(cx.listener(move |this, ev: &ClickEvent, w, cx| {
                                            cx.stop_propagation();
                                            this.refocus(w, cx);
                                            let b = bar.get();
                                            if b.size.width > px(0.) {
                                                let f = (f32::from(ev.position().x - b.left()) / f32::from(b.size.width)).clamp(0., 1.);
                                                this.aloud_seek((f * total as f32) as u64, cx);
                                            }
                                        })),
                                )
                                .child(div().flex_none().w(px(44.)).text_size(px(size::SM)).text_color(Tokens::text_3()).child(recap::fmt_ms(total)))
                                .child(icon_button("aloud-close", ph::X, "Stop reading aloud").small().kbd("r").on_click(cx.listener(|this, _, w, cx| {
                                    this.refocus(w, cx);
                                    this.aloud = None;
                                    cx.notify();
                                }))),
                        )
                        // A fixed two-line caption row, so the controls above never move as
                        // passages change; the follow buttons sit at its end.
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .h(px(40.))
                                .px(px(4.))
                                .child(div().flex_1().min_w_0().line_clamp(2).text_size(px(size::SM)).text_color(Tokens::text_1()).child(caption))
                                .when(!follow, |d| {
                                    d.child(btn("aloud-here", "Read from here", ButtonKind::Ghost).small().icon(ph::BOOK_OPEN).tooltip("Move the narration to the page on screen").on_click(cx.listener(|this, _, w, cx| {
                                        this.refocus(w, cx);
                                        this.aloud_from_page(cx);
                                    })))
                                    .child(btn("aloud-follow", "Back to narration", ButtonKind::Primary).small().icon(ph::ARROW_COUNTER_CLOCKWISE).on_click(cx.listener(|this, _, w, cx| {
                                        this.refocus(w, cx);
                                        if let Some(a) = &mut this.aloud {
                                            a.follow = true;
                                        }
                                        cx.notify();
                                    })))
                                }),
                        ),
                ),
        )
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
                        let keep = t.over_bars || t.dragging || t.show_keys || t.menu_open || t.focus_inside || t.auto_custom.is_some() || !matches!(t.pages, Load::Ready(_)) || t.transition.is_some();
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
        self.menu_open = false;
        self.hide_task = None;
        cx.notify();
    }

    fn toggle_overlay(&mut self, cx: &mut Context<Self>) {
        if self.overlay {
            self.hide_overlay(cx)
        } else {
            self.show_overlay(cx)
        }
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.dragging {
            if ev.pressed_button == Some(MouseButton::Left) {
                self.slide_to(ev.position.x, cx);
            } else {
                self.dragging = false;
            }
        }
        if let Some((start, off)) = self.pan {
            if ev.pressed_button == Some(MouseButton::Left) {
                let d = ev.position - start;
                if f32::from(d.x).abs() + f32::from(d.y).abs() > 3. {
                    self.panned = true;
                }
                self.page_scroll.set_offset(off + d);
                cx.notify();
            } else {
                self.pan = None;
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
    /// toggles the overlay, and a double-click in the middle zooms in on that spot. While zoomed a
    /// drag pans and a click only toggles the overlay. The bars swallow their own clicks.
    fn on_page_click(&mut self, ev: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.refocus(window, cx);
        if std::mem::take(&mut self.panned) {
            return;
        }
        let b = self.root_bounds.get();
        let zoomed = self.zoom > 1.;
        let zone = if self.strip() || zoomed { 0 } else { logic::tap_zone(f32::from(ev.position().x - b.left()), f32::from(b.size.width), self.mode) };
        if ev.click_count() == 2 && zone == 0 && !self.strip() && self.transition.is_none() {
            // The first click of the pair toggled the overlay: put it back, then zoom.
            self.toggle_overlay(cx);
            self.set_zoom(if zoomed { 1. } else { 2. }, ev.mouse_position(), cx);
            return;
        }
        match zone {
            -1 => self.prev(cx),
            1 => self.next(cx),
            _ => self.toggle_overlay(cx),
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
        // A bar control reached with Tab takes Space as its own click (GPUI fires it on key up),
        // so the page must not turn as well.
        let on_control = !self.focus.is_focused(window);
        match k.key.as_str() {
            "space" if on_control => {}
            // While reading aloud, Space is play / pause like any audio player.
            "space" if self.aloud.is_some() => self.aloud_play_pause(cx),
            "k" => self.aloud_play_pause(cx),
            "r" => self.toggle_aloud(cx),
            "escape" if self.show_keys => {
                self.show_keys = false;
                cx.notify();
            }
            "escape" if self.zoom > 1. => self.set_zoom(1., None, cx),
            "escape" => mv_ui::navigate(Route::Back, window, cx),
            "right" if rtl => self.prev(cx),
            "left" if rtl => self.next(cx),
            "right" | "pagedown" | "down" => self.next(cx),
            "space" if k.modifiers.shift => self.prev(cx),
            "space" => self.next(cx),
            "left" | "pageup" | "up" => self.prev(cx),
            "home" => self.jump(0, cx),
            "end" => self.jump(self.total().saturating_sub(1), cx),
            "a" => self.toggle_auto(cx),
            "[" => self.set_auto_secs(logic::auto_nudge(self.auto_secs, -1), cx),
            "]" => self.set_auto_secs(logic::auto_nudge(self.auto_secs, 1), cx),
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
            "=" | "+" => self.zoom_by(ZOOM_STEP, None, cx),
            "-" => self.zoom_by(1. / ZOOM_STEP, None, cx),
            "0" => self.set_zoom(1., None, cx),
            "f11" => window.toggle_fullscreen(),
            "?" => self.toggle_keys(cx),
            "/" if k.modifiers.shift => self.toggle_keys(cx),
            _ => {}
        }
    }

    fn toggle_keys(&mut self, cx: &mut Context<Self>) {
        self.show_keys = !self.show_keys;
        if self.show_keys {
            self.show_overlay(cx);
        }
        cx.notify();
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

    fn download_chapter(&mut self, cx: &mut Context<Self>) {
        let Some(c) = &self.chapter else { return };
        let job = Job { manga_id: self.manga_id.clone(), manga_title: self.title.clone(), chapter_id: c.id.clone(), label: logic::chapter_short(c) };
        self.dl_pending = true;
        self.queue.update(cx, |d, cx| d.enqueue(vec![job], cx));
    }

    // ------------------------------------------------------------ views

    fn page_view(&self, i: usize, fit: Fit, strip: bool, zoomed: bool, cx: &mut Context<Self>) -> AnyElement {
        let Load::Ready(pages) = &self.pages else { return div().into_any_element() };
        match &pages[i].state {
            PageState::Ready(p, d) => {
                let im = img(p.clone());
                let ratio = d.map(|(w, h)| w / h);
                match (strip, fit) {
                    // A relative width leaves the image box at the file's own pixel height, which
                    // shows as black gaps once the page is scaled; size the box from the ratio instead.
                    (true, _) | (false, Fit::Width) => match ratio {
                        Some(r) => div().w_full().aspect_ratio(r).child(im.size_full()).into_any_element(),
                        None => im.w_full().into_any_element(),
                    },
                    // Fit the whole page in its box: the box has a definite size, so tall pages
                    // scale down instead of being cut off.
                    (false, Fit::Screen) => im.size_full().object_fit(ObjectFit::Contain).into_any_element(),
                    (false, Fit::Original) if zoomed => im.size_full().into_any_element(),
                    (false, Fit::Original) => im.into_any_element(),
                }
            }
            PageState::Failed(e) => div()
                .id(SharedString::from(format!("retry-{i}")))
                .w_full()
                .h(px(if strip { 420. } else { 320. }))
                .flex()
                .items_center()
                .justify_center()
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.retry(i, cx)
                }))
                .child(error_view(e.clone()).title(format!("Page {} did not load", i + 1)).compact().retry(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.retry(i, cx)
                })))
                .into_any_element(),
            _ => div()
                .w_full()
                .h(px(if strip { STRIP_WIDTH * 1.4 } else { 280. }))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .child(spinner(SharedString::from(format!("pg-spin-{i}")), Tokens::text_3(), icon_px::XL))
                .child(muted(format!("Page {}", i + 1)))
                .into_any_element(),
        }
    }

    /// "Finished / Next" card between chapters (paged mode and the end of the strip), with the
    /// next chapter's offline state and a warning when chapter numbers skip.
    /// Mihon chapter transitions: https://mihon.app/docs/guides/reader-settings
    fn transition_card(&self, dir: i32, cx: &mut Context<Self>) -> Div {
        let cur = self.chapter.as_ref();
        let other = self.neighbour(dir);
        let (k1, k2, i1, i2) = if dir > 0 { ("Finished", "Next", ph::CHECK_CIRCLE, ph::SKIP_FORWARD) } else { ("Current", "Previous", ph::BOOK_OPEN, ph::SKIP_BACK) };
        let line = |icon: Ph, k: &'static str, v: String, strong: bool| {
            div()
                .flex()
                .items_start()
                .gap(px(12.))
                .child(div().pt(px(2.)).child(ph_icon(icon, if strong { Weight::Fill } else { Weight::Regular }, if strong { Tokens::accent() } else { Tokens::text_3() }, icon_px::LG)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .min_w_0()
                        .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(k))
                        .child(div().ramp(if strong { ramp::SUBTITLE_2 } else { ramp::BODY }).text_color(if strong { Tokens::text_1() } else { Tokens::text_2() }).child(v)),
                )
        };
        let gap = match (cur, other) {
            (Some(a), Some(b)) => logic::missing_between(a.number, b.number),
            _ => 0,
        };
        let offline = other.map(|c| c.download_status == "complete");
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .w(px(400.))
            .p(px(24.))
            .rounded(px(radius::XL))
            .bg(Tokens::surface())
            .border_1()
            .border_color(Tokens::border())
            .shadow_lg()
            .child(line(i1, k1, cur.map(logic::chapter_label).unwrap_or_default(), false))
            .child(match other {
                Some(o) => line(i2, k2, logic::chapter_label(o), true).into_any_element(),
                None => div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .text_size(px(size::MD))
                    .text_color(Tokens::text_2())
                    .child(icon_ph(ph::INFO, Tokens::text_3(), icon_px::LG))
                    .child(if dir > 0 { "There is no next chapter yet." } else { "This is the first chapter." })
                    .into_any_element(),
            })
            .when(other.is_some(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(6.))
                        .child(match offline {
                            Some(true) => icon_tag(glyphs::DOWNLOAD, "Downloaded", Tone::Success),
                            _ => icon_tag(ph::GLOBE, "Reads online", Tone::Neutral),
                        })
                        .when(gap > 0, |d| d.child(icon_tag(ph::WARNING, if gap == 1 { "1 chapter missing".to_string() } else { format!("{gap} chapters missing") }, Tone::Warning))),
                )
            })
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .when(other.is_some(), |d| {
                        d.child(
                            btn(SharedString::from(format!("tr-go-{dir}")), if dir > 0 { "Read next chapter" } else { "Read previous chapter" }, ButtonKind::Primary)
                                .icon(if dir > 0 { ph::ARROW_RIGHT } else { ph::ARROW_LEFT })
                                .kbd(if dir > 0 { "n" } else { "p" })
                                .occlude()
                                .on_click(cx.listener(move |this, _, w, cx| {
                                    cx.stop_propagation();
                                    this.refocus(w, cx);
                                    this.step_chapter(dir, cx);
                                })),
                        )
                    })
                    .when(other.is_none(), |d| {
                        d.child(btn("tr-back", "Back to the manga", ButtonKind::Secondary).icon(glyphs::BACK).occlude().on_click(|_, w, cx| {
                            cx.stop_propagation();
                            mv_ui::navigate(Route::Back, w, cx)
                        }))
                    }),
            )
    }

    fn chapter_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        let cur = self.chapter.as_ref().map(|c| c.id.clone());
        let entries: Vec<Entry> = std::iter::once(Entry::Heading(format!("{} chapters", self.chapters.len()).into()))
            .chain(self.chapters.iter().rev().map(|c| {
                let (w, id) = (weak.clone(), c.id.clone());
                let hint = if c.is_read {
                    Some("Read".to_string())
                } else if c.read_progress > 0 {
                    Some(format!("Page {}", c.read_progress + 1))
                } else {
                    None
                };
                let e = Entry::new(logic::chapter_label(c), move |window, cx| {
                    let _ = w.update(cx, |t, cx| {
                        t.open_chapter(id.clone(), false, cx);
                        t.refocus(window, cx);
                    });
                })
                .checked(cur.as_deref() == Some(c.id.as_str()));
                match hint {
                    Some(h) => e.hint(h),
                    None => e,
                }
            }))
            .collect();
        let w = cx.entity().downgrade();
        icon_trigger("reader-chapters", icon_ph(ph::LIST_NUMBERS, Tokens::text_1(), icon_px::MD), "Chapters")
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| fill_menu(menu.min_w(px(300.)).max_h(px(440.)).scrollable(true), &entries))
            .on_open_change(move |open, window, cx| {
                let _ = w.update(cx, |t, cx| {
                    t.menu_open = *open;
                    if *open {
                        t.show_overlay(cx);
                    } else {
                        t.refocus(window, cx);
                    }
                });
            })
    }

    fn download_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(c) = &self.chapter else { return div().into_any_element() };
        match self.queue.read(cx).status(&c.id) {
            Some(st) => {
                let f = match st {
                    ChapterDl::Active(f) => f,
                    ChapterDl::Queued => 0.,
                };
                let label = if matches!(st, ChapterDl::Queued) { "Queued for download".to_string() } else { format!("Downloading · {}%", (f * 100.).round()) };
                div().id("reader-dl").flex().flex_none().items_center().justify_center().size(px(32.)).tooltip(tip(label)).child(progress_ring_sized(f, 18.)).into_any_element()
            }
            None if c.download_status == "complete" => icon_button("reader-dl", ph::CHECK_CIRCLE, "Saved for offline reading").selected(true).color(Tokens::accent()).into_any_element(),
            None => icon_button("reader-dl", glyphs::DOWNLOAD, "Download this chapter")
                .on_click(cx.listener(|this, _, w, cx| {
                    this.refocus(w, cx);
                    this.download_chapter(cx);
                }))
                .into_any_element(),
        }
    }

    fn top_bar(&self, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let label = self.chapter.as_ref().map(logic::chapter_label).unwrap_or_default();
        let full = window.is_fullscreen();
        div()
            .id("reader-top")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(56.))
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .bg(Tokens::glass_strong())
            .border_b_1()
            .border_color(Tokens::glass_border())
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
                d.child(div().flex_none().mr(px(4.)).text_size(px(size::SM)).text_color(Tokens::text_2()).child(format!("{} / {}", self.current + 1, self.total())))
            })
            .children(self.recap_buttons(cx))
            .when(!self.chapters.is_empty(), |d| d.child(self.chapter_menu(cx)))
            .child(self.download_button(cx))
            .child(icon_button("reader-keys", ph::KEYBOARD, "Keyboard shortcuts").kbd("?").selected(self.show_keys).on_click(cx.listener(|this, _, w, cx| {
                this.refocus(w, cx);
                this.toggle_keys(cx);
            })))
            .child(icon_button("reader-full", if full { ph::CORNERS_IN } else { ph::CORNERS_OUT }, if full { "Exit full screen" } else { "Full screen" }).kbd("f11").on_click(cx.listener(|this, _, w, cx| {
                this.refocus(w, cx);
                w.toggle_fullscreen();
                cx.notify();
            })))
    }

    fn slider(&self, frac: f32, rtl: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let big = self.dragging;
        let (rest, grown) = (14., 18.);
        let place = move |d: Div, size: f32| {
            let d = d.size(px(size)).top(px((28. - size) / 2.));
            if rtl {
                d.right(relative(frac)).mr(px(-size / 2.))
            } else {
                d.left(relative(frac)).ml(px(-size / 2.))
            }
        };
        let thumb = place(div().absolute().rounded_full().bg(Tokens::text_1()).shadow_md(), if big { grown } else { rest })
            .when(big, |d| d.border_2().border_color(Tokens::accent()))
            .when(!big, |d| {
                d.group_hover("reader-slider", move |s| {
                    let s = s.size(px(grown)).top(px((28. - grown) / 2.)).border_2().border_color(Tokens::accent());
                    if rtl {
                        s.mr(px(-grown / 2.))
                    } else {
                        s.ml(px(-grown / 2.))
                    }
                })
            });
        // While dragging, a bubble names the page under the thumb (WCAG 1.4.11 track contrast via Tokens::track).
        let bubble = big.then(|| {
            let d = div()
                .absolute()
                .bottom(px(30.))
                .w(px(72.))
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(radius::SM))
                .bg(Tokens::surface_3())
                .border_1()
                .border_color(Tokens::border_strong())
                .shadow_md()
                .text_size(px(size::SM))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(Tokens::text_1())
                .child(format!("Page {}", self.current + 1));
            if rtl {
                d.right(relative(frac)).mr(px(-36.))
            } else {
                d.left(relative(frac)).ml(px(-36.))
            }
        });
        div()
            .id("slider")
            .relative()
            .flex_1()
            .h(px(28.))
            .flex()
            .items_center()
            .cursor_pointer()
            .group("reader-slider")
            .child(bounds_probe(self.slider_bounds.clone()))
            .child(
                div()
                    .w_full()
                    .h(px(6.))
                    .flex()
                    .when(rtl, |d| d.justify_end())
                    .rounded_full()
                    .bg(Tokens::track())
                    .child(div().h_full().rounded_full().bg(Tokens::accent()).w(relative(frac))),
            )
            .child(thumb)
            .children(bubble)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, w, cx| {
                    cx.stop_propagation();
                    this.refocus(w, cx);
                    this.dragging = true;
                    this.slide_to(ev.position.x, cx);
                }),
            )
    }

    fn bottom_bar(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let total = self.total();
        let has_prev = self.neighbour(-1).is_some();
        let has_next = self.neighbour(1).is_some();
        let frac = if total > 1 { self.current as f32 / (total - 1) as f32 } else { 1. };
        let rtl = self.mode == ReaderMode::Rtl;
        let chapter_btn = |id: &'static str, icon: Ph, tip_text: &'static str, key: &'static str, enabled: bool, why: &'static str, dir: i32, cx: &mut Context<Self>| {
            icon_button(id, icon, tip_text).kbd(key).map(|b| if enabled { b } else { b.disabled(why) }).on_click(cx.listener(move |this, _, w, cx| {
                this.refocus(w, cx);
                this.step_chapter(dir, cx);
            }))
        };
        // In right-to-left the left button goes forward, like the pages.
        let (left_btn, right_btn) = if rtl {
            (chapter_btn("next-ch", ph::SKIP_BACK, "Next chapter", "n", has_next, "This is the last chapter", 1, cx), chapter_btn("prev-ch", ph::SKIP_FORWARD, "Previous chapter", "p", has_prev, "This is the first chapter", -1, cx))
        } else {
            (chapter_btn("prev-ch", ph::SKIP_BACK, "Previous chapter", "p", has_prev, "This is the first chapter", -1, cx), chapter_btn("next-ch", ph::SKIP_FORWARD, "Next chapter", "n", has_next, "This is the last chapter", 1, cx))
        };
        let strip = self.strip();
        let mode_ix = ReaderMode::ALL.iter().position(|m| *m == self.mode).unwrap_or(0);
        let fit_ix = Fit::ALL.iter().position(|f| *f == self.fit).unwrap_or(0);
        let zoom = self.zoom;
        let zoom_group = (!strip && self.transition.is_none()).then(|| {
            div()
                .flex()
                .items_center()
                .gap(px(2.))
                .child(
                    icon_button("zoom-out", ph::MINUS, "Zoom out").small().kbd("-").map(|b| if zoom <= 1. { b.disabled("Already at the page fit") } else { b }).on_click(cx.listener(|this, _, w, cx| {
                        this.refocus(w, cx);
                        this.zoom_by(1. / ZOOM_STEP, None, cx);
                    })),
                )
                .child(
                    btn("zoom-reset", format!("{}%", (zoom * 100.).round()), ButtonKind::Ghost).small().min_w(px(56.)).tooltip("Reset zoom").kbd("0").on_click(cx.listener(|this, _, w, cx| {
                        this.refocus(w, cx);
                        this.set_zoom(1., None, cx);
                    })),
                )
                .child(
                    icon_button("zoom-in", ph::PLUS, "Zoom in").small().kbd("=").map(|b| if zoom >= MAX_ZOOM { b.disabled("Already at the largest zoom") } else { b }).on_click(cx.listener(|this, _, w, cx| {
                        this.refocus(w, cx);
                        this.zoom_by(ZOOM_STEP, None, cx);
                    })),
                )
        });
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
                    .max_w(px(820.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(px(radius::XL))
                    .bg(Tokens::glass_strong())
                    .border_1()
                    .border_color(Tokens::glass_border())
                    .shadow_lg()
                    .occlude()
                    .on_hover(cx.listener(|this, h: &bool, _, _| this.over_bars = *h))
                    // the panel occludes the root, so slider drags are tracked here as well
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| this.on_mouse_move(ev, cx)))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        this.dragging = false;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(left_btn)
                            .child(div().w(px(28.)).flex_none().text_right().text_size(px(size::SM)).text_color(Tokens::text_2()).child(if total > 0 { (self.current + 1).to_string() } else { "-".into() }))
                            .child(self.slider(frac, rtl, cx))
                            .child(div().w(px(28.)).flex_none().text_size(px(size::SM)).text_color(Tokens::text_2()).child(if total > 0 { total.to_string() } else { "-".into() }))
                            .child(right_btn),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                segmented("reader-mode", ReaderMode::ALL.map(|m| (Some(mode_icon(m)), m.short())), mode_ix)
                                    .small()
                                    .tooltip(tip_kbd("Reading mode", "m"))
                                    .on_change(cx.listener(|this, ix: &usize, w, cx| {
                                        this.refocus(w, cx);
                                        this.set_mode(ReaderMode::ALL[*ix], cx);
                                    })),
                            )
                            .when(!strip, |d| {
                                d.child(
                                    segmented("reader-fit", Fit::ALL.map(|f| (Some(fit_icon(f)), f.short())), fit_ix).small().tooltip(tip_kbd("Page fit", "f")).on_change(cx.listener(|this, ix: &usize, w, cx| {
                                        this.refocus(w, cx);
                                        this.set_fit(Fit::ALL[*ix], cx);
                                    })),
                                )
                            })
                            .child(self.auto_control(cx))
                            .child(div().flex_1())
                            .children(zoom_group),
                    ),
            )
    }

    /// Help panel listing every shortcut with key caps (Apple HIG: make shortcuts discoverable
    /// where the command lives): https://developer.apple.com/design/human-interface-guidelines/keyboards
    fn keys_panel(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("reader-keys-backdrop")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(Tokens::overlay())
            .occlude()
            .on_click(cx.listener(|this, _, w, cx| {
                this.refocus(w, cx);
                this.show_keys = false;
                cx.notify();
            }))
            .child(
                div()
                    .id("reader-keys")
                    .w(px(460.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .p(px(20.))
                    .rounded(px(radius::XL))
                    .bg(Tokens::surface_3())
                    .border_1()
                    .border_color(Tokens::border_strong())
                    .shadow_lg()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .pb(px(8.))
                            .child(ph_icon(ph::KEYBOARD, Weight::Fill, Tokens::accent(), icon_px::LG))
                            .child(div().flex_1().ramp(ramp::SUBTITLE_2).text_color(Tokens::text_1()).child("Reader shortcuts"))
                            .child(icon_button("reader-keys-close", ph::X, "Close").small().kbd("escape").on_click(cx.listener(|this, _, w, cx| {
                                this.refocus(w, cx);
                                this.show_keys = false;
                                cx.notify();
                            }))),
                    )
                    .children(SHORTCUTS.iter().map(|(k, what)| {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .h(px(28.))
                            .child(div().w(px(72.)).flex().child(kbd(*k)))
                            .child(div().flex_1().text_size(px(size::SM)).text_color(Tokens::text_2()).child(*what))
                    })),
            )
    }
}

/// Back control: reacts on mouse *down* so it can never lose the click to a re-render or to the
/// page-turn area, and stops the event from reaching anything below it (Enter / Space work too).
fn back_button(id: &'static str, label: bool) -> Stateful<Div> {
    let el = if label { btn(id, "Back", ButtonKind::Ghost).icon(glyphs::BACK).kbd("escape").build() } else { icon_button(id, glyphs::BACK, "Back").kbd("escape").build() };
    el.occlude()
        .on_mouse_down(MouseButton::Left, |_, w, cx| {
            cx.stop_propagation();
            mv_ui::navigate(Route::Back, w, cx);
        })
        .on_click(|ev, w, cx| {
            if ev.is_keyboard() {
                mv_ui::navigate(Route::Back, w, cx);
            }
        })
}

impl Render for ReaderPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Focus on a descendant (not the reader itself) means a control was reached with Tab.
        self.focus_inside = self.focus.contains_focused(window, cx) && !self.focus.is_focused(window);
        let strip = self.strip();
        let body: AnyElement = match &self.pages {
            Load::Loading => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(12.))
                .child(spinner("chapter-spin", Tokens::text_2(), icon_px::HERO))
                .child(div().text_size(px(size::MD)).child(shimmer_text("Loading chapter\u{2026}")))
                .into_any_element(),
            Load::Failed(e) => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    error_view(e.clone())
                        .title("Could not open this chapter")
                        .retry(cx.listener(|this, _, _, cx| {
                            if let Some(c) = this.chapter.as_ref().map(|c| c.id.clone()) {
                                this.open_chapter(c, false, cx);
                            }
                        }))
                        .secondary(btn("fail-back", "Back", ButtonKind::Ghost).icon(glyphs::BACK).on_click(|_, w, cx| mv_ui::navigate(Route::Back, w, cx))),
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
                    .on_scroll_wheel(cx.listener(|this, _, window, cx| {
                        this.aloud_touched();
                        // after the scroll has been applied
                        cx.defer_in(window, |this, _, cx| this.sync_strip_page(cx));
                    }))
                    .on_click(cx.listener(|this, ev: &ClickEvent, w, cx| this.on_page_click(ev, w, cx)))
                    // one child per page, so `top_item` / `scroll_to_top_of_item` map to pages
                    .children((0..n).map(|i| div().w_full().max_w(px(STRIP_WIDTH)).flex_none().child(self.page_view(i, Fit::Width, true, false, cx))))
                    .child(div().flex_none().flex().justify_center().py(px(48.)).pb(px(160.)).child(self.transition_card(1, cx)))
                    .into_any_element()
            }
            Load::Ready(_) => {
                let zoomed = self.zoom > 1. && self.transition.is_none();
                // Ctrl + wheel zooms. The handler sits on a child that covers the whole area so it
                // runs before the scroll container's own wheel handling and can stop it.
                let wheel = cx.listener(|this, ev: &ScrollWheelEvent, _, cx| {
                    if ev.modifiers.control || ev.modifiers.platform {
                        let dy = f32::from(ev.delta.pixel_delta(px(20.)).y);
                        if dy != 0. {
                            this.zoom_by(if dy > 0. { 1.1 } else { 1. / 1.1 }, Some(ev.position), cx);
                        }
                        cx.stop_propagation();
                    } else {
                        this.aloud_touched();
                    }
                });
                let surface: AnyElement = match self.transition {
                    Some(dir) => div().size_full().flex().items_center().justify_center().child(self.transition_card(dir, cx)).into_any_element(),
                    None if zoomed => {
                        let (bw, bh) = self.zoom_base();
                        let page = self.page_view(self.current, self.fit, false, true, cx);
                        div()
                            .id("zoom-surface")
                            .flex_none()
                            .min_w(relative(1.))
                            .min_h(relative(1.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_scroll_wheel(wheel)
                            .child(div().flex_none().w(px(bw * self.zoom)).h(px(bh * self.zoom)).child(page))
                            .into_any_element()
                    }
                    None => {
                        let page = self.page_view(self.current, self.fit, false, false, cx);
                        // `flex_none`: the scroll container sizes its scroll range from this child, so
                        // it must keep the page's full height instead of shrinking to the window.
                        match self.fit {
                            Fit::Screen => div().id("zoom-surface").size_full().on_scroll_wheel(wheel).child(page).into_any_element(),
                            Fit::Width => div()
                                .id("zoom-surface")
                                .flex_none()
                                .w_full()
                                .min_h(relative(1.))
                                .flex()
                                .flex_col()
                                .items_center()
                                .on_scroll_wheel(wheel)
                                .child(div().flex_none().w_full().max_w(px(PAGED_WIDTH)).child(page))
                                .into_any_element(),
                            Fit::Original => div()
                                .id("zoom-surface")
                                .flex_none()
                                .w_full()
                                .min_h(relative(1.))
                                .flex()
                                .flex_col()
                                .items_center()
                                .on_scroll_wheel(wheel)
                                .child(div().flex_none().child(page))
                                .into_any_element(),
                        }
                    }
                };
                let grabbing = self.pan.is_some();
                div()
                    .id("paged")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .flex_col()
                    .map(|d| if zoomed { d.items_start().overflow_scroll().cursor(if grabbing { CursorStyle::ClosedHand } else { CursorStyle::OpenHand }) } else { d.items_center() })
                    .when(!zoomed && (self.fit != Fit::Screen || self.transition.is_some()), |d| d.overflow_y_scroll())
                    .track_scroll(&self.page_scroll)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, _, _| {
                            if this.zoom > 1. {
                                this.pan = Some((ev.position, this.page_scroll.offset()));
                                this.panned = false;
                            }
                        }),
                    )
                    .on_click(cx.listener(|this, ev: &ClickEvent, w, cx| this.on_page_click(ev, w, cx)))
                    .child(surface)
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
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.dragging = false;
                this.pan = None;
                cx.notify();
            }))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.dragging = false;
                this.pan = None;
                cx.notify();
            }))
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(Tokens::canvas())
            .child(bounds_probe(self.root_bounds.clone()))
            .child(body)
            .when(overlay, |d| d.child(self.top_bar(window, cx)).child(self.bottom_bar(cx)))
            // With the overlay hidden a small back button stays in the corner for mouse users.
            .when(!overlay, |d| d.child(div().absolute().top(px(10.)).left(px(10.)).opacity(0.55).hover(|s| s.opacity(1.)).rounded(px(radius::MD)).bg(Tokens::glass()).child(back_button("reader-back-mini", false))))
            // Paged auto-scroll: a thin line along the bottom fills up until the page turns.
            .when(self.auto && !strip && matches!(self.pages, Load::Ready(_)), |d| {
                let f = (self.auto_elapsed / self.auto_secs.max(0.1)).clamp(0., 1.);
                d.child(div().absolute().bottom_0().left_0().right_0().h(px(3.)).bg(Tokens::track()).child(div().h_full().w(relative(f)).bg(Tokens::accent())))
            })
            .children(self.aloud_player(cx))
            .when(self.show_keys, |d| d.child(self.keys_panel(cx)))
    }
}
