//! Step 1, Manga (design 9.4) [story]: the episode's chapters with their Read status, a page
//! browser with the Read's panel boxes, and each panel's people and lines. Reads run through
//! Studio's own queue (`ui_studio::pipeline::enqueue`, design 3.3); this screen never reads a
//! page itself and never edits the Read (panels and speakers are fixed in Studio). Chapters can
//! be added to or removed from the episode here. W3-S1.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_anime::{source, ChapterPick, Episode, Store};
use mv_recap::{ChapterRead, PageRead};
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::{btn, checkbox, confirm_dialog, empty_view, error_view, icon_button, icon_ph, list_row, muted, ph, progress_bar, size, skeleton_block, skeleton_line, skeleton_row, spinner, tag, tip, toast, ButtonKind, Ph, Route, Toast, Tokens, Tone};
use ui_studio::data::ChapterItem;
use ui_studio::pipeline::{self, ItemState};

use super::shared::plain_reason;
use super::{Step, StepCtx};
use crate::page::Load;
use crate::style::{COLUMN_HEAD_H, PANEL_W, POLL_MS};
use crate::widgets::{column_body, column_head, count, error_block, hover_actions, info_line, is_narrow, label_row, list_item, loading_fill, marker, section, selectable, spec, workspace, LineTone};

/// Width of the page in the browser; its height follows the page (a long strip scrolls).
const PAGE_W: f32 = 480.;
/// Page strip thumbnails (portrait).
const PAGE_THUMB: (f32, f32) = (54., 76.);
/// The panel crop in the right column fills its inner width.
const CROP_W: f32 = PANEL_W - 2. * space::MD;
/// The box the panel crop is fitted into (a tall panel stands upright in it).
const CROP_H: f32 = 320.;
/// Height of the page's loading shimmer, as a share of its width (a typical page).
const PAGE_LOADING_RATIO: f32 = 1.4;
/// The page is shown from a preview about twice its on-screen width (sharp on a high-density
/// screen), never from the original file.
const PAGE_PREVIEW_W: u32 = (PAGE_W * 2.) as u32;
/// The panel crop's preview: twice its box.
const CROP_PREVIEW: (u32, u32) = ((CROP_W * 2.) as u32, (CROP_H * 2.) as u32);

// ---------------------------------------------------------------- data

/// One chapter of the episode as read from disk.
#[derive(Clone)]
struct Chapter {
    pick: ChapterPick,
    /// What Studio's queue takes to read it.
    item: ChapterItem,
    /// Page files in reading order (empty: not downloaded).
    pages: Vec<PathBuf>,
    read: Option<ChapterRead>,
}

impl Chapter {
    fn label(&self) -> String {
        if self.pick.label.trim().is_empty() {
            self.item.label()
        } else {
            self.pick.label.clone()
        }
    }

    /// Distinct pages the Read covers.
    fn read_pages(&self) -> usize {
        let mut idx: Vec<usize> = self.read.as_ref().map(|r| r.pages.iter().map(|p| p.index).collect()).unwrap_or_default();
        idx.sort_unstable();
        idx.dedup();
        idx.len()
    }

    fn total(&self) -> usize {
        self.pages.len().max(self.read_pages())
    }

    fn page_read(&self, page: usize) -> Option<&PageRead> {
        self.read.as_ref().and_then(|r| r.pages.iter().find(|p| p.index == page))
    }
}

/// The episode and its chapters.
#[derive(Clone)]
struct Data {
    episode: Episode,
    chapters: Vec<Chapter>,
    /// Chapters used by the project's other episodes (chapter id → episode name).
    elsewhere: HashMap<String, String>,
}

/// This manga's downloaded chapters (for "Add chapters").
type Library = Vec<ChapterItem>;

/// What Studio's queue needs for a chapter that is not in the library list.
fn item_of(pick: &ChapterPick, title: &str, pages: usize) -> ChapterItem {
    ChapterItem {
        id: pick.chapter_id.clone(),
        manga_id: pick.manga_id.clone(),
        manga_title: title.to_string(),
        number: pick.number,
        title: (pick.number.is_none() && !pick.label.trim().is_empty()).then(|| pick.label.clone()),
        pages_dir: source::resolve_dir(&pick.pages_dir),
        output_dir: source::resolve_dir(&pick.output_dir),
        page_count: pages,
        has_text: false,
        video: None,
        audio_files: 0,
    }
}

/// Reads the episode, its chapters' pages and Reads. Blocking.
fn load(store: &Store, ep: &str, library: Option<&Library>) -> anyhow::Result<Data> {
    let series = store.series().context("The project could not be read")?;
    let episode = store.episode(ep).context("The episode could not be read")?;
    let title = series.manga_title.clone();
    let chapters = episode
        .chapters
        .iter()
        .map(|pick| {
            let dir = source::resolve_dir(&pick.pages_dir);
            let pages = if dir.as_os_str().is_empty() { Vec::new() } else { mv_recap::list_pages(&dir) };
            let read = source::read(pick, &title).filter(|r| !r.pages.is_empty());
            let item = library.and_then(|l| l.iter().find(|c| c.id == pick.chapter_id).cloned()).unwrap_or_else(|| item_of(pick, &title, pages.len()));
            Chapter { pick: pick.clone(), item, pages, read }
        })
        .collect();
    let mut elsewhere = HashMap::new();
    for r in series.episodes.iter().filter(|r| r.id != episode.id) {
        if let Ok(other) = store.episode(&r.id) {
            let name = if r.title.trim().is_empty() { r.id.clone() } else { r.title.clone() };
            for c in other.chapters {
                elsewhere.entry(c.chapter_id).or_insert_with(|| name.clone());
            }
        }
    }
    Ok(Data { episode, chapters, elsewhere })
}

/// This manga's downloaded chapters, by number. Blocking.
fn load_library(manga_id: &str) -> anyhow::Result<Library> {
    let groups = ui_studio::data::load()?;
    let mut list: Library = groups.into_iter().filter(|g| g.id == manga_id).flat_map(|g| g.chapters).collect();
    list.sort_by(|a, b| a.number.unwrap_or(f64::MAX).total_cmp(&b.number.unwrap_or(f64::MAX)));
    Ok(list)
}

// ---------------------------------------------------------------- read state (pure)

/// Where a chapter's Read stands.
#[derive(Clone, Debug, PartialEq)]
enum ReadState {
    /// Its pages are not on disk.
    Missing,
    NotRead,
    Partly { read: usize, total: usize },
    /// In Studio's queue, not started.
    Waiting { id: u64 },
    Reading { id: u64 },
    Paused { id: u64 },
    Read,
    Failed { id: u64, error: String },
}

impl ReadState {
    /// A Read can be started for it.
    fn can_read(&self) -> bool {
        matches!(self, ReadState::NotRead | ReadState::Partly { .. } | ReadState::Failed { .. })
    }

    fn active(&self) -> bool {
        matches!(self, ReadState::Waiting { .. } | ReadState::Reading { .. } | ReadState::Paused { .. })
    }
}

/// The state from the files and Studio's queue item for the chapter (id, state, error).
fn read_state(downloaded: bool, read: usize, total: usize, item: Option<(u64, ItemState, Option<String>)>) -> ReadState {
    let done = total > 0 && read >= total;
    match item {
        Some((id, ItemState::Running, _)) => return ReadState::Reading { id },
        Some((id, ItemState::Queued, _)) => return ReadState::Waiting { id },
        Some((id, ItemState::Paused, _)) if !done => return ReadState::Paused { id },
        _ => {}
    }
    if done {
        return ReadState::Read;
    }
    if let Some((id, ItemState::Failed, error)) = item {
        let error = error.map(|e| e.trim().to_string()).filter(|e| !e.is_empty()).unwrap_or_else(|| "Studio could not read this chapter.".into());
        return ReadState::Failed { id, error };
    }
    if read > 0 {
        ReadState::Partly { read, total }
    } else if !downloaded {
        ReadState::Missing
    } else {
        ReadState::NotRead
    }
}

/// The second line of a chapter row.
fn state_line(state: &ReadState, pages: usize, progress: Option<(usize, usize)>) -> String {
    match state {
        ReadState::Missing => "Not downloaded".into(),
        ReadState::NotRead => format!("Not read yet · {}", count(pages, "page")),
        ReadState::Partly { read, total } => format!("Read {read} of {total} pages"),
        ReadState::Waiting { .. } => "Waiting in Studio's queue".into(),
        ReadState::Reading { .. } => match progress {
            Some((done, total)) if total > 0 => format!("Reading · {done} of {total} pages"),
            _ => "Reading…".into(),
        },
        ReadState::Paused { .. } => "Paused in Studio's queue".into(),
        ReadState::Read => format!("Read · {}", count(pages, "page")),
        ReadState::Failed { .. } => "Reading stopped with a problem".into(),
    }
}

/// The screen's one main action.
#[derive(Clone, Debug, PartialEq)]
enum Main {
    /// Read these chapter ids.
    Read(Vec<String>),
    /// Reads are running; nothing to start.
    Busy,
    /// Every chapter is read: on to the Story step.
    Continue,
    /// Nothing can be read (pages missing).
    Blocked,
}

fn main_action(states: &[(String, ReadState)]) -> Main {
    let ids: Vec<String> = states.iter().filter(|(_, s)| s.can_read()).map(|(id, _)| id.clone()).collect();
    if !ids.is_empty() {
        Main::Read(ids)
    } else if states.iter().any(|(_, s)| s.active()) {
        Main::Busy
    } else if !states.is_empty() && states.iter().all(|(_, s)| *s == ReadState::Read) {
        Main::Continue
    } else {
        Main::Blocked
    }
}

/// Speakers and people of a Read in the order they first appear (for marker colours).
fn people_order(read: &ChapterRead) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |n: &str| {
        let n = n.trim();
        if !n.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(n)) {
            out.push(n.to_string());
        }
    };
    let mut pages: Vec<&PageRead> = read.pages.iter().collect();
    pages.sort_by_key(|p| p.index);
    for p in pages {
        for panel in &p.panels {
            panel.characters.iter().for_each(|c| add(&c.name));
            panel.lines.iter().filter_map(|l| l.speaker.as_deref()).for_each(&mut add);
        }
    }
    out
}

/// "thought", "narration", "sound"; plain speech has no tag.
fn kind_word(kind: &str) -> Option<&'static str> {
    match kind.trim().to_ascii_lowercase().as_str() {
        "thought" => Some("thought"),
        "narration" => Some("narration"),
        "sfx" => Some("sound"),
        _ => None,
    }
}

// ---------------------------------------------------------------- page pictures (pure)

/// The tallest preview made (pixels): a graphics card takes a picture this tall, so a very long
/// webtoon page is scaled down to it instead of being drawn from its original file.
const MAX_PREVIEW_H: u32 = 4096;
/// How many wanted previews wait while one is being made.
const PREVIEW_WAITING: usize = 4;
/// Bytes read to find a picture's size (a JPEG's size can follow a large block of camera data).
const HEAD_BYTES: u64 = 256 << 10;

/// Width and height of a PNG, JPEG, GIF, BMP or WebP picture, from its first bytes.
fn picture_size(head: &[u8]) -> Option<(u32, u32)> {
    let be16 = |i: usize| head.get(i..i + 2).map(|b| u32::from(u16::from_be_bytes([b[0], b[1]])));
    let le16 = |i: usize| head.get(i..i + 2).map(|b| u32::from(u16::from_le_bytes([b[0], b[1]])));
    let be32 = |i: usize| head.get(i..i + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let le32 = |i: usize| head.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let le24 = |i: usize| head.get(i..i + 3).map(|b| u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16);
    let size = if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        (be32(16)?, be32(20)?)
    } else if head.starts_with(b"GIF8") {
        (le16(6)?, le16(8)?)
    } else if head.starts_with(b"BM") {
        (le32(18)?, (le32(22)? as i32).unsigned_abs())
    } else if head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP".as_slice()) {
        match head.get(12..16)? {
            b"VP8 " => (le16(26)? & 0x3fff, le16(28)? & 0x3fff),
            b"VP8L" => {
                let bits = le32(21)?;
                ((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)
            }
            b"VP8X" => (le24(24)? + 1, le24(27)? + 1),
            _ => return None,
        }
    } else if head.starts_with(&[0xFF, 0xD8]) {
        // JPEG: walk the segments to the frame header (SOF0–SOF15 but DHT, JPG and DAC).
        let mut i = 2;
        loop {
            while head.get(i) == Some(&0xFF) && head.get(i + 1) == Some(&0xFF) {
                i += 1;
            }
            if head.get(i) != Some(&0xFF) {
                return None;
            }
            let marker = *head.get(i + 1)?;
            i += 2;
            match marker {
                0x01 | 0xD0..=0xD8 => {}
                0xD9 | 0xDA => return None,
                0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => break (be16(i + 5)?, be16(i + 3)?),
                _ => i += be16(i)? as usize,
            }
        }
    } else {
        return None;
    };
    (size.0 > 0 && size.1 > 0).then_some(size)
}

/// The size of the preview of a `w`×`h` picture shown up to `max_w` × `max_h`: its own shape,
/// never larger than the picture, at most `max_w` wide and `max_h` tall (a tall page is scaled
/// down whole, never cut).
fn preview_size(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let (w, h) = (f64::from(w.max(1)), f64::from(h.max(1)));
    let scale = (f64::from(max_w.max(1)) / w).min(f64::from(max_h.max(1)) / h).min(1.);
    (((w * scale).round() as u32).max(1), ((h * scale).round() as u32).max(1))
}

/// A bounded copy of a page or panel picture for the screen, made once in the project cache
/// with the same technique as the strip's previews. Blocking: call it off the UI thread.
fn make_preview(src: &std::path::Path, max_w: u32, max_h: u32) -> anyhow::Result<PathBuf> {
    use std::io::Read as _;
    let mut head = Vec::new();
    std::fs::File::open(src).context("The picture is not there")?.take(HEAD_BYTES).read_to_end(&mut head).context("The picture could not be read")?;
    let (w, h) = picture_size(&head).ok_or_else(|| anyhow::anyhow!("The picture's size could not be read"))?;
    let (pw, ph) = preview_size(w, h, max_w, max_h.min(MAX_PREVIEW_H));
    mv_anime::store::page_thumb(src, pw, ph)
}

// ---------------------------------------------------------------- the view

/// Studio's AI run on one of our chapters.
#[derive(Clone, Debug, Default, PartialEq)]
struct Activity {
    chapter: String,
    done: usize,
    total: usize,
    status: String,
    waiting: bool,
}

fn activity_now() -> Option<Activity> {
    let a = pipeline::ai_activity()?;
    if a.kind != "read" || a.state != "running" {
        return None;
    }
    Some(Activity { chapter: a.chapter_id?, done: a.pages_done, total: a.pages_total, status: a.status, waiting: a.waiting.is_some() })
}

pub struct MangaStep {
    ctx: StepCtx,
    store: Store,
    data: Load<Data>,
    library: Option<Load<Library>>,
    /// Library chapters Studio has read (looked up with the library, off the UI thread).
    lib_read: HashSet<String>,
    studio: pipeline::Queue,
    activity: Option<Activity>,
    /// Selected chapter id, page and panel.
    chapter: Option<String>,
    page: usize,
    panel: Option<usize>,
    /// "Add chapters" is open with these chapter ids ticked.
    picker: Option<BTreeSet<String>>,
    seen: [u64; 2],
    loading: bool,
    again: bool,
    saving: bool,
    strip: ScrollHandle,
    /// Page previews by page file: `Some(file)` made, `None` it could not be made.
    thumbs: HashMap<PathBuf, Option<PathBuf>>,
    /// The chapter whose strip previews are being made in the background.
    thumbing: Option<String>,
    /// Bumped when another chapter's strip is wanted: an older batch stops at its next page.
    thumb_gen: u64,
    /// Bounded pictures of the big page and the panel crop (never the original file), by source
    /// file: `Some(file)` made, `None` it could not be made.
    previews: HashMap<PathBuf, Option<PathBuf>>,
    /// A big preview is being made; the ones wanted meanwhile wait here (newest last).
    previewing: bool,
    preview_next: Vec<(PathBuf, u32, u32)>,
    /// The decoded big pictures of the page on screen; a new page gets a new cache and the
    /// old one's pictures are freed.
    images: Entity<RetainAllImageCache>,
    /// The project's files changed: look again when this view is next drawn.
    refresh: bool,
    /// Studio's queue or reading moved on: read the chapters again when next drawn.
    reread: bool,
}

pub fn open(ctx: StepCtx, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| {
        let mut s = MangaStep {
            store: Store::open(&ctx.project),
            ctx,
            data: Load::Loading,
            library: None,
            lib_read: HashSet::new(),
            studio: pipeline::snapshot(),
            activity: activity_now(),
            chapter: None,
            page: 0,
            panel: None,
            picker: None,
            seen: versions(),
            loading: false,
            again: false,
            saving: false,
            strip: ScrollHandle::new(),
            thumbs: HashMap::new(),
            thumbing: None,
            thumb_gen: 0,
            previews: HashMap::new(),
            previewing: false,
            preview_next: Vec::new(),
            images: RetainAllImageCache::new(cx),
            refresh: false,
            reread: false,
        };
        s.reload(cx);
        s.load_library(cx);
        s.poll(window, cx);
        s
    })
    .into()
}

fn versions() -> [u64; 2] {
    [mv_anime::store::version(), pipeline::version()]
}

impl MangaStep {
    fn reload(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            self.again = true;
            return;
        }
        self.loading = true;
        let (store, ep) = (self.store.clone(), self.ctx.episode.clone());
        let library = match &self.library {
            Some(Load::Ready(l)) => Some(l.clone()),
            _ => None,
        };
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || load(&store, &ep, library.as_ref())).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match res {
                    Ok(d) => {
                        this.keep_selection(&d);
                        this.data = Load::Ready(d);
                        // A picture that was missing (a panel crop not made yet) is looked for again.
                        this.previews.retain(|_, made| made.is_some());
                    }
                    Err(e) => {
                        // Keep what is shown when a later re-read fails.
                        if !matches!(this.data, Load::Ready(_)) {
                            this.data = Load::Failed("The episode could not be read. Check that the project's drive is connected, then try again.".into());
                        }
                        mv_core::logging::global().warn("anime", &format!("manga step: {e:#}"));
                    }
                }
                if std::mem::take(&mut this.again) {
                    this.reload(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_library(&mut self, cx: &mut Context<Self>) {
        if matches!(self.library, Some(Load::Loading)) {
            return;
        }
        self.library = Some(Load::Loading);
        let store = self.store.clone();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || -> anyhow::Result<(Library, HashSet<String>)> {
                let series = store.series().context("The project could not be read")?;
                let lib = load_library(&series.manga_id)?;
                let read = lib.iter().filter(|c| c.output_dir.join("studio").join("read.json").is_file()).map(|c| c.id.clone()).collect();
                Ok((lib, read))
            })
            .await
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.library = Some(match res {
                    Ok((l, read)) => {
                        this.lib_read = read;
                        Load::Ready(l)
                    }
                    Err(e) => {
                        mv_core::logging::global().warn("anime", &format!("manga step: the library could not be listed: {e:#}"));
                        Load::Failed("Your downloaded chapters could not be listed. Try again in a moment.".into())
                    }
                });
                // Studio's queue reads best from the library's own chapter records.
                this.reload(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Keeps the selection on a chapter that is still there, else the first.
    fn keep_selection(&mut self, d: &Data) {
        let still = self.chapter.as_ref().is_some_and(|id| d.chapters.iter().any(|c| &c.pick.chapter_id == id));
        if !still {
            self.chapter = d.chapters.first().map(|c| c.pick.chapter_id.clone());
            self.page = 0;
            self.panel = None;
        }
        if let Some(c) = self.chapter.as_ref().and_then(|id| d.chapters.iter().find(|c| &c.pick.chapter_id == id)) {
            let total = c.total();
            if total > 0 && self.page >= total {
                self.page = total - 1;
                self.panel = None;
            }
        }
        if d.chapters.is_empty() && self.picker.is_none() {
            self.picker = Some(BTreeSet::new());
        }
    }

    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(POLL_MS)).await;
            if this.update(cx, |s, cx| s.tick(cx)).is_err() {
                break;
            }
        })
        .detach();
    }

    /// Reads the episode's chapter list in the background and reloads when it changed.
    fn check_chapters(&mut self, cx: &mut Context<Self>) {
        let ours = match &self.data {
            Load::Ready(d) => d.episode.chapters.iter().map(|c| c.chapter_id.clone()).collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        let (store, ep) = (self.store.clone(), self.ctx.episode.clone());
        cx.spawn(async move |this, cx| {
            let now = mv_ui::bg::blocking(move || store.episode(&ep).map(|e| e.chapters.into_iter().map(|c| c.chapter_id).collect::<Vec<_>>()).unwrap_or_default()).await;
            if now.is_ok_and(|now| now != ours) {
                let _ = this.update(cx, |this, cx| this.reload(cx));
            }
        })
        .detach();
    }

    /// Notes what changed; the files are read again when the view is drawn next (right away
    /// while it is on screen), so a step that is not shown never re-reads.
    fn tick(&mut self, cx: &mut Context<Self>) {
        let v = versions();
        let act = activity_now();
        let mut changed = false;
        if v[1] != self.seen[1] {
            self.studio = pipeline::snapshot();
            self.reread = true;
            changed = true;
        }
        if v[0] != self.seen[0] {
            // Another step saved: re-read only when this episode's chapters changed.
            self.refresh = true;
            changed = true;
        }
        if act != self.activity {
            // A new batch of pages read: the browser shows them as they come.
            self.reread |= act.as_ref().map(|a| a.done) != self.activity.as_ref().map(|a| a.done);
            self.activity = act;
            changed = true;
        }
        self.seen = v;
        if changed {
            cx.notify();
        }
    }

    fn state_of(&self, c: &Chapter) -> ReadState {
        let item = self.studio.for_chapter(&c.pick.chapter_id).map(|i| (i.id, i.state, i.error.clone()));
        read_state(!c.pages.is_empty(), c.read_pages(), c.total(), item)
    }

    fn progress_of(&self, c: &Chapter) -> Option<(usize, usize)> {
        self.activity.as_ref().filter(|a| a.chapter == c.pick.chapter_id).map(|a| (a.done, a.total))
    }

    // ------------------------------------------------------------ actions

    /// Starts a Read of these chapters in Studio's queue (design 3.3).
    fn start_read(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        let Load::Ready(d) = &self.data else { return };
        let items: Vec<ChapterItem> = d.chapters.iter().filter(|c| ids.contains(&c.pick.chapter_id)).map(|c| c.item.clone()).collect();
        if items.is_empty() {
            return;
        }
        if !pipeline::has_context() {
            let env = ui_studio::setup::latest().map(|c| c.env);
            pipeline::set_context(pipeline::context_from_app(env, cx));
        }
        let (new, widened, already) = pipeline::enqueue(&items, mv_recap::Step::Read, None, mv_recap::FORMAT_LANDSCAPE, None, true);
        mv_core::logging::global().info("anime", &format!("manga step: read {} chapter(s) in Studio ({new} new, {widened} widened, {already} already queued)", items.len()));
        let msg = if items.len() == 1 { "Studio is reading the chapter. You can keep working; progress shows here.".to_string() } else { format!("Studio is reading {} chapters. You can keep working; progress shows here.", items.len()) };
        toast::show(cx, Toast::info(msg).icon(ph::BOOK_OPEN_TEXT));
        self.studio = pipeline::snapshot();
        cx.notify();
    }

    fn select_chapter(&mut self, id: String, cx: &mut Context<Self>) {
        if self.chapter.as_deref() != Some(id.as_str()) {
            self.chapter = Some(id);
            self.page = 0;
            self.panel = None;
            self.strip.set_offset(point(px(0.), px(0.)));
            self.free_images(cx);
        }
        cx.notify();
    }

    fn select_page(&mut self, page: usize, cx: &mut Context<Self>) {
        if self.page != page {
            self.page = page;
            self.panel = None;
            // The strip keeps the selected page in view (each tile is one of its children).
            self.strip.scroll_to_item(page);
            self.free_images(cx);
        }
        cx.notify();
    }

    /// The page left the screen: its decoded pictures are let go (a new cache for the next one).
    fn free_images(&mut self, cx: &mut Context<Self>) {
        self.images = RetainAllImageCache::new(cx);
    }

    /// Makes the missing strip previews of the chapter's `pages` in the background, one at a
    /// time, each shown as soon as it is ready (the strip shows a shimmer until then). Another
    /// chapter's batch still running stops at its next page, so a newly opened chapter never
    /// waits for the one before.
    fn ensure_thumbs(&mut self, chapter: &str, pages: &[PathBuf], cx: &mut Context<Self>) {
        if self.thumbing.as_deref() == Some(chapter) {
            return;
        }
        let missing: Vec<PathBuf> = pages.iter().filter(|p| !self.thumbs.contains_key(*p)).cloned().collect();
        if missing.is_empty() {
            return;
        }
        self.thumb_gen = self.thumb_gen.wrapping_add(1);
        let gen = self.thumb_gen;
        self.thumbing = Some(chapter.to_string());
        let (w, h) = (PAGE_THUMB.0 * 2., PAGE_THUMB.1 * 2.);
        cx.spawn(async move |this, cx| {
            for page in missing {
                let src = page.clone();
                let made = mv_ui::bg::blocking(move || mv_anime::store::page_thumb(&src, w as u32, h as u32)).await.and_then(|r| r);
                let made = match made {
                    Ok(p) => Some(p),
                    Err(e) => {
                        mv_core::logging::global().warn("anime", &format!("manga step: no page preview: {e:#}"));
                        None
                    }
                };
                let current = this.update(cx, |this, cx| {
                    this.thumbs.insert(page, made);
                    cx.notify();
                    this.thumb_gen == gen
                });
                // The view is gone, or another chapter's strip is wanted now.
                if !current.unwrap_or(false) {
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                if this.thumb_gen == gen {
                    this.thumbing = None;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Makes the bounded preview of a picture shown up to `max_w` × `max_h` pixels (the page or a
    /// panel crop) in the background; one at a time, the latest wanted first.
    fn want_preview(&mut self, src: PathBuf, max_w: u32, max_h: u32, cx: &mut Context<Self>) {
        if self.previews.contains_key(&src) {
            return;
        }
        if self.previewing {
            self.preview_next.retain(|(p, _, _)| *p != src);
            self.preview_next.push((src, max_w, max_h));
            // Only the last few wanted matter (the page and its crop on screen).
            if self.preview_next.len() > PREVIEW_WAITING {
                self.preview_next.remove(0);
            }
            return;
        }
        self.previewing = true;
        cx.spawn(async move |this, cx| {
            let from = src.clone();
            let made = mv_ui::bg::blocking(move || make_preview(&from, max_w, max_h)).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                let made = match made {
                    Ok(p) => Some(p),
                    Err(e) => {
                        mv_core::logging::global().warn("anime", &format!("manga step: no page picture: {e:#}"));
                        None
                    }
                };
                this.previews.insert(src, made);
                this.previewing = false;
                if let Some((next, w, h)) = this.preview_next.pop() {
                    this.want_preview(next, w, h, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle_pick(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some(p) = self.picker.as_mut() {
            if !p.remove(&id) {
                p.insert(id);
            }
        }
        cx.notify();
    }

    fn open_picker(&mut self, cx: &mut Context<Self>) {
        self.picker = Some(BTreeSet::new());
        if !matches!(self.library, Some(Load::Ready(_))) {
            self.library = None;
            self.load_library(cx);
        }
        cx.notify();
    }

    /// Adds the ticked chapters to the episode, in chapter order.
    fn add_picked(&mut self, cx: &mut Context<Self>) {
        let (Some(picked), Some(Load::Ready(lib))) = (self.picker.clone(), &self.library) else { return };
        let items: Vec<ChapterItem> = lib.iter().filter(|c| picked.contains(&c.id)).cloned().collect();
        if items.is_empty() || self.saving {
            return;
        }
        let first = items[0].id.clone();
        self.saving = true;
        let (store, ep) = (self.store.clone(), self.ctx.episode.clone());
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || -> anyhow::Result<usize> {
                let mut episode = store.episode(&ep)?;
                let before = episode.chapters.len();
                for it in &items {
                    if !episode.chapters.iter().any(|c| c.chapter_id == it.id) {
                        episode.chapters.push(crate::home::pick_of(it));
                    }
                }
                episode.chapters.sort_by(|a, b| a.number.unwrap_or(f64::MAX).total_cmp(&b.number.unwrap_or(f64::MAX)));
                store.save_episode(&episode).context("Could not save the episode")?;
                Ok(episode.chapters.len() - before)
            })
            .await
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
                match res {
                    Ok(n) => {
                        this.picker = None;
                        this.chapter = Some(first);
                        this.page = 0;
                        this.panel = None;
                        toast::show(cx, Toast::success(format!("Added {} to the episode.", count(n, "chapter"))));
                        this.reload(cx);
                    }
                    Err(e) => {
                        mv_core::logging::global().warn("anime", &format!("manga step: chapters not added: {e:#}"));
                        toast::show(cx, Toast::error("The chapters could not be added. Check that the project's drive is connected, then try again."));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn confirm_remove(&mut self, id: String, label: String, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        confirm_dialog(
            format!("Remove {label} from this episode?"),
            "Nothing is deleted: the chapter stays in your library and its Read stays in Studio. Scenes adapted from it leave the story the next time you adapt it, unless you changed them.",
            "Remove chapter",
            false,
        )
        .cancel_label("Keep it")
        .on_confirm(move |_, cx| {
            let id = id.clone();
            let _ = this.update(cx, |s, cx| s.remove(id, cx));
        })
        .open(window, cx);
    }

    fn remove(&mut self, id: String, cx: &mut Context<Self>) {
        let (store, ep) = (self.store.clone(), self.ctx.episode.clone());
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || -> anyhow::Result<()> {
                let mut episode = store.episode(&ep)?;
                episode.chapters.retain(|c| c.chapter_id != id);
                store.save_episode(&episode).context("Could not save the episode")
            })
            .await
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(()) => this.reload(cx),
                    Err(e) => {
                        mv_core::logging::global().warn("anime", &format!("manga step: chapter not removed: {e:#}"));
                        toast::show(cx, Toast::error("The chapter could not be removed. Check that the project's drive is connected, then try again."));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

// ---------------------------------------------------------------- render

impl Render for MangaStep {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // What changed while this view was not drawn is read now (it is on screen again).
        if std::mem::take(&mut self.reread) {
            self.refresh = false;
            self.reload(cx);
        } else if std::mem::take(&mut self.refresh) {
            self.check_chapters(cx);
        }
        // The open chapter's page previews (made once, in the background), and the bounded
        // pictures of the page and the panel on screen.
        let open: Option<(String, Vec<PathBuf>, Option<PathBuf>, Option<PathBuf>)> = match &self.data {
            Load::Ready(d) => self.chapter.as_ref().and_then(|id| d.chapters.iter().find(|c| &c.pick.chapter_id == id)).filter(|c| c.read.is_some()).map(|c| {
                let page = c.pages.get(self.page.min(c.pages.len().saturating_sub(1))).cloned();
                let crop = self.panel.filter(|_| c.page_read(self.page).is_some()).map(|k| source::panel_file(&c.pick, &c.item.manga_title, self.page, k));
                (c.pick.chapter_id.clone(), c.pages.clone(), page, crop)
            }),
            _ => None,
        };
        if let Some((chapter, pages, page, crop)) = open {
            self.ensure_thumbs(&chapter, &pages, cx);
            if let Some(p) = page {
                self.want_preview(p, PAGE_PREVIEW_W, MAX_PREVIEW_H, cx);
            }
            if let Some(p) = crop {
                self.want_preview(p, CROP_PREVIEW.0, CROP_PREVIEW.1, cx);
            }
        }
        let d = match &self.data {
            Load::Loading => return self.skeleton(window).into_any_element(),
            Load::Failed(e) => {
                let e = e.clone();
                return div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(error_view(e).retry(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.data = Load::Loading;
                        this.reload(cx);
                        cx.notify();
                    })))
                    .into_any_element();
            }
            Load::Ready(d) => d,
        };
        let states: Vec<(String, ReadState)> = d.chapters.iter().map(|c| (c.pick.chapter_id.clone(), self.state_of(c))).collect();
        let list = self.list(d, &states, cx);
        let current = self.chapter.as_ref().and_then(|id| d.chapters.iter().position(|c| &c.pick.chapter_id == id));
        let (center, panel) = match current {
            Some(i) => {
                let c = &d.chapters[i];
                let state = states[i].1.clone();
                let narrow = is_narrow(window);
                let panel = (c.page_read(self.page).is_some() && (!narrow || self.panel.is_some())).then(|| self.inspector(c, narrow, cx).into_any_element());
                (self.chapter_view(c, &state, cx).into_any_element(), panel)
            }
            None => (
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(empty_view(ph::BOOK_OPEN_TEXT, "No chapters in this episode yet", "Tick the chapters this episode tells on the left. They come from the manga you downloaded."))
                    .into_any_element(),
                None,
            ),
        };
        workspace(window, Some(list.into_any_element()), center, panel).into_any_element()
    }
}

/// The 40 px header of a column with a real title (not an uppercase label).
fn title_head(title: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(space::SM))
        .h(px(COLUMN_HEAD_H))
        .px(px(space::LG))
        .border_b_1()
        .border_color(Tokens::border())
        .child(div().min_w_0().truncate().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title.into()))
}

fn state_icon(id: impl Into<ElementId>, s: &ReadState) -> AnyElement {
    let (icon, color): (Ph, Rgba) = match s {
        ReadState::Reading { .. } => return spinner(id, Tokens::accent(), icon_px::SM).into_any_element(),
        ReadState::Read => (ph::CHECK_CIRCLE, Tokens::success()),
        ReadState::Partly { .. } => (ph::CIRCLE_NOTCH, Tokens::warning()),
        ReadState::NotRead => (ph::CIRCLE_DASHED, Tokens::text_3()),
        ReadState::Missing => (ph::CLOUD_ARROW_DOWN, Tokens::warning()),
        ReadState::Waiting { .. } => (ph::HOURGLASS, Tokens::text_2()),
        ReadState::Paused { .. } => (ph::PAUSE_CIRCLE, Tokens::text_2()),
        ReadState::Failed { .. } => (ph::WARNING_CIRCLE, Tokens::danger()),
    };
    icon_ph(icon, color, icon_px::SM).into_any_element()
}

impl MangaStep {
    fn skeleton(&self, window: &Window) -> Div {
        let list = div()
            .flex()
            .flex_col()
            .size_full()
            .child(column_head("Chapters"))
            .child(div().flex().flex_col().gap(px(space::XS)).p(px(space::MD)).children((0..4).map(|_| skeleton_row())));
        let center = div()
            .flex()
            .flex_col()
            .size_full()
            .child(div().h(px(COLUMN_HEAD_H)).flex().items_center().px(px(space::LG)).child(skeleton_line(px(180.))))
            .child(div().flex().gap(px(space::XS)).p(px(space::LG)).children((0..8).map(|_| skeleton_block(px(PAGE_THUMB.0), px(PAGE_THUMB.1)))))
            .child(div().px(px(space::LG)).child(skeleton_block(px(PAGE_W), px(PAGE_W * 1.4))));
        workspace(window, Some(list.into_any_element()), center, None)
    }

    // ------------------------------------------------------------ left: chapters

    fn list(&self, d: &Data, states: &[(String, ReadState)], cx: &mut Context<Self>) -> Div {
        let read = states.iter().filter(|(_, s)| *s == ReadState::Read).count();
        let head = column_head("Chapters").child(spec(format!("{read} / {} read", d.chapters.len())));
        let mut body = column_body("mg-chapters");
        for (i, c) in d.chapters.iter().enumerate() {
            body = body.child(self.chapter_row(i, c, &states[i].1, cx));
        }
        let picking = self.picker.is_some();
        if picking {
            body = body.child(self.picker_view(d, cx));
        } else {
            body = body.child(
                div().child(btn("mg-add", "Add chapters", ButtonKind::Ghost).small().icon(ph::PLUS).tooltip("Pick more downloaded chapters for this episode").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.open_picker(cx)))),
            );
        }
        let foot = (!picking && !d.chapters.is_empty()).then(|| self.footer(states, cx));
        div().flex().flex_col().size_full().child(head).child(body).children(foot)
    }

    fn chapter_row(&self, i: usize, c: &Chapter, state: &ReadState, cx: &mut Context<Self>) -> Div {
        let selected = self.chapter.as_deref() == Some(c.pick.chapter_id.as_str());
        let group = SharedString::from(format!("mg-ch-{i}"));
        let id = c.pick.chapter_id.clone();
        let line = state_line(state, c.total(), self.progress_of(c));
        let row = list_item(("mg-ch", i), selected)
            .child(div().flex_none().w(px(icon_px::MD)).flex().justify_center().child(state_icon(("mg-ch-spin", i), state)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(c.label()))
                    .child(div().truncate().text_size(px(size::XS)).text_color(if matches!(state, ReadState::Failed { .. }) { Tokens::danger() } else { Tokens::text_3() }).child(line)),
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select_chapter(id.clone(), cx)));
        let (rid, label) = (c.pick.chapter_id.clone(), c.label());
        div().relative().group(group.clone()).child(row).child(
            hover_actions(group).child(icon_button(("mg-rm", i), ph::X, "Remove from this episode").small().on_click(cx.listener(move |this, _: &ClickEvent, w, cx| this.confirm_remove(rid.clone(), label.clone(), w, cx)))),
        )
    }

    fn footer(&self, states: &[(String, ReadState)], cx: &mut Context<Self>) -> Div {
        let main = main_action(states);
        let go = self.ctx.go.clone();
        let button = match main {
            Main::Read(ids) => {
                let n = ids.len();
                btn("mg-read", format!("Read {}", count(n, "chapter")), ButtonKind::Primary)
                    .icon(ph::BOOK_OPEN_TEXT)
                    .full_width()
                    .tooltip("Studio reads each page: its panels, who is in them and every line")
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.start_read(ids.clone(), cx)))
            }
            // Only a Read really running turns a spinner (nothing animates while it waits).
            Main::Busy if states.iter().any(|(_, s)| matches!(s, ReadState::Reading { .. })) => btn("mg-read", "Reading in Studio…", ButtonKind::Primary).full_width().loading(true),
            Main::Busy if states.iter().any(|(_, s)| matches!(s, ReadState::Waiting { .. })) => {
                btn("mg-read", "Waiting in Studio's queue", ButtonKind::Primary).icon(ph::HOURGLASS).full_width().disabled("It starts when the chapters before it in Studio's queue are done")
            }
            Main::Busy => btn("mg-read", "Paused in Studio's queue", ButtonKind::Primary).icon(ph::PAUSE_CIRCLE).full_width().disabled("Resume it in Studio's queue"),
            Main::Continue => btn("mg-next", "Continue to Story", ButtonKind::Primary).icon(ph::NOTE_PENCIL).full_width().on_click(move |_, w, cx| go(Step::Story, w, cx)),
            Main::Blocked => btn("mg-read", "Read chapters", ButtonKind::Primary).full_width().disabled("Download the missing chapters first, in Manga › Library"),
        };
        let read = states.iter().filter(|(_, s)| *s == ReadState::Read).count();
        let line = if read == states.len() { "Every chapter is read".to_string() } else { format!("{read} of {} read", count(states.len(), "chapter")) };
        div().flex().flex_none().flex_col().gap(px(space::SM)).p(px(space::MD)).border_t_1().border_color(Tokens::border()).child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(line)).child(button)
    }

    fn picker_view(&self, d: &Data, cx: &mut Context<Self>) -> Div {
        let picked = self.picker.clone().unwrap_or_default();
        let mut out = div().flex().flex_col().gap(px(space::SM)).pt(px(space::SM)).child(label_row("Add chapters", if picked.is_empty() { String::new() } else { format!("{} picked", picked.len()) }));
        match &self.library {
            None | Some(Load::Loading) => {
                out = out.child(div().flex().flex_col().gap(px(space::XS)).children((0..3).map(|_| skeleton_row())));
            }
            Some(Load::Failed(e)) => {
                out = out.child(error_block(e.clone()).child(btn("mg-lib-retry", "Try again", ButtonKind::Secondary).small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.library = None;
                    this.load_library(cx);
                }))));
            }
            Some(Load::Ready(lib)) => {
                let free: Vec<&ChapterItem> = lib.iter().filter(|c| !d.chapters.iter().any(|x| x.pick.chapter_id == c.id)).collect();
                if free.is_empty() {
                    out = out.child(muted("Every downloaded chapter of this manga is in this episode. Download more in Manga › Library.")).child(
                        div().child(btn("mg-lib", "Open Library", ButtonKind::Ghost).small().trailing_icon(ph::ARROW_UP_RIGHT).on_click(|_, w, cx| mv_ui::navigate(Route::Library, w, cx))),
                    );
                }
                for (i, c) in free.into_iter().enumerate() {
                    let on = picked.contains(&c.id);
                    let id = c.id.clone();
                    let read = self.lib_read.contains(&c.id);
                    let other = d.elsewhere.get(&c.id).cloned();
                    out = out.child(
                        list_row(("mg-pick", i), on).flex_none()
                            .child(checkbox(("mg-pick-box", i), on))
                            .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).child(c.label()))
                            .when_some(other, |d, ep| d.child(tag(format!("In {ep}"), Tone::Neutral)))
                            .when(read, |d| d.child(tag("Read", Tone::Success)))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_pick(id.clone(), cx))),
                    );
                }
            }
        }
        let n = picked.len();
        let add = btn("mg-pick-add", if n == 0 { "Add chapters".to_string() } else { format!("Add {}", count(n, "chapter")) }, ButtonKind::Primary).loading(self.saving);
        let add = if n == 0 { add.disabled("Tick at least one chapter") } else { add.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.add_picked(cx))) };
        let has_chapters = !d.chapters.is_empty();
        out.child(
            div()
                .flex()
                .gap(px(space::SM))
                .pt(px(space::XS))
                .child(add)
                .when(has_chapters, |row| row.child(btn("mg-pick-cancel", "Cancel", ButtonKind::Ghost).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.picker = None;
                    cx.notify();
                })))),
        )
    }

    // ------------------------------------------------------------ centre: the chapter

    fn chapter_view(&self, c: &Chapter, state: &ReadState, cx: &mut Context<Self>) -> Div {
        let pages = c.total();
        let status = match state {
            ReadState::Read => tag("Read", Tone::Success),
            ReadState::Partly { .. } => tag("Partly read", Tone::Warning),
            ReadState::NotRead => tag("Not read", Tone::Neutral),
            ReadState::Missing => tag("Not downloaded", Tone::Warning),
            ReadState::Waiting { .. } => tag("Waiting", Tone::Neutral),
            ReadState::Reading { .. } => tag("Reading", Tone::Info),
            ReadState::Paused { .. } => tag("Paused", Tone::Neutral),
            ReadState::Failed { .. } => tag("Stopped", Tone::Danger),
        };
        let head = title_head(c.label())
            .child(spec(count(pages, "page")))
            .child(status)
            .child(div().flex_1())
            .child(btn("mg-studio", "Open Studio", ButtonKind::Ghost).small().trailing_icon(ph::ARROW_SQUARE_OUT).tooltip("Opens Studio, where panels and speakers are changed; pick this chapter there").on_click(|_, w, cx| mv_ui::navigate(Route::Studio, w, cx)));
        let mut body = div().id("mg-body").flex().flex_col().flex_1().min_h_0().gap(px(space::LG)).p(px(space::LG)).overflow_y_scroll();
        if let Some(s) = self.status_block(c, state, cx) {
            body = body.child(s);
        }
        let browse = !c.pages.is_empty() && c.read.is_some();
        if browse {
            body = body.child(self.page_view(c, cx));
        }
        // The strip and the page row stay at the top while the page under them scrolls.
        let strip = browse.then(|| {
            div().flex_none().flex().flex_col().gap(px(space::SM)).w_full().px(px(space::LG)).pt(px(space::MD)).pb(px(space::SM)).border_b_1().border_color(Tokens::border()).child(self.page_strip(c, cx)).child(self.page_nav(c, cx))
        });
        div().flex().flex_col().size_full().child(head).children(strip).child(body)
    }

    /// What is going on with the chapter's Read, when it is not simply read.
    fn status_block(&self, c: &Chapter, state: &ReadState, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = c.pick.chapter_id.clone();
        let el = match state {
            ReadState::Read => return None,
            ReadState::Missing => empty_view(ph::CLOUD_ARROW_DOWN, "This chapter is not downloaded", "Its pages are not on this computer. Download it in Manga › Library, then come back.")
                .compact()
                .action(btn("mg-lib2", "Open Library", ButtonKind::Secondary).trailing_icon(ph::ARROW_UP_RIGHT).on_click(|_, w, cx| mv_ui::navigate(Route::Library, w, cx)))
                .into_any_element(),
            ReadState::NotRead => empty_view(ph::BOOK_OPEN_TEXT, "Not read yet", "Studio reads every page: the panels, who is in them and each line. The story is adapted from that.")
                .compact()
                .action(btn("mg-read-one", "Read this chapter", ButtonKind::Secondary).icon(ph::BOOK_OPEN_TEXT).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.start_read(vec![id.clone()], cx))))
                .into_any_element(),
            ReadState::Partly { read, total } => info_line(ph::INFO, LineTone::Warning, format!("{read} of {total} pages are read. Reading it again carries on from where it stopped; the story needs every page."))
                .child(btn("mg-read-rest", "Read the rest", ButtonKind::Secondary).small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.start_read(vec![id.clone()], cx))))
                .into_any_element(),
            ReadState::Waiting { id: item } => {
                let item = *item;
                info_line(ph::HOURGLASS, LineTone::Neutral, "Waiting in Studio's queue. It starts when the chapters before it are done.").child(btn("mg-pause", "Pause", ButtonKind::Ghost).small().on_click(move |_, _, _| pipeline::pause(item))).into_any_element()
            }
            ReadState::Paused { id: item } => {
                let item = *item;
                info_line(ph::PAUSE_CIRCLE, LineTone::Neutral, "Paused in Studio's queue.").child(btn("mg-resume", "Resume", ButtonKind::Secondary).small().on_click(move |_, _, _| pipeline::resume(item))).into_any_element()
            }
            ReadState::Reading { id: item } => {
                let item = *item;
                let act = self.activity.clone().filter(|a| a.chapter == c.pick.chapter_id);
                let (p, line, waiting) = match &act {
                    Some(a) if a.total > 0 => (a.done as f32 / a.total as f32, format!("Reading page {} of {}", (a.done + 1).min(a.total), a.total), a.waiting),
                    Some(a) => (0., a.status.clone(), a.waiting),
                    None => (0., "Starting to read…".into(), false),
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::SM))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::SM))
                            .child(if waiting { icon_ph(ph::CLOCK, Tokens::warning(), icon_px::SM).into_any_element() } else { spinner("mg-reading", Tokens::accent(), icon_px::SM).into_any_element() })
                            .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).text_color(Tokens::text_1()).child(line))
                            .child(btn("mg-pause", "Pause", ButtonKind::Ghost).small().on_click(move |_, _, _| pipeline::pause(item))),
                    )
                    .child(progress_bar("mg-read-progress", p, !waiting))
                    .when_some(act.filter(|a| a.waiting), |d, a| {
                        d.child(info_line(ph::CLOCK, LineTone::Warning, a.status).child(btn("mg-now", "Try now", ButtonKind::Secondary).small().on_click(|_, _, _| mv_recap::retry_now())))
                    })
                    .into_any_element()
            }
            ReadState::Failed { id: item, error } => {
                let item = *item;
                let text = match plain_reason(error) {
                    Some(why) => format!("Reading stopped: {why} Try again."),
                    None => "Studio could not read this chapter. Try again.".to_string(),
                };
                error_block(text).child(btn("mg-retry", "Try again", ButtonKind::Secondary).small().icon(ph::ARROW_CLOCKWISE).on_click(move |_, _, _| pipeline::resume(item))).into_any_element()
            }
        };
        Some(el)
    }

    /// The chapter's pages in a row: each tile its page's preview (a shimmer while it is made),
    /// the page number as a badge. It scrolls sideways, also with the mouse wheel.
    fn page_strip(&self, c: &Chapter, cx: &mut Context<Self>) -> Stateful<Div> {
        let (w, h) = PAGE_THUMB;
        // Every tile is a direct child, so `scroll_to_item(page)` brings the selected one in view.
        let mut strip = div().id("mg-strip").flex().w_full().min_w_0().gap(px(space::XS)).pb(px(space::XS)).overflow_x_scroll().track_scroll(&self.strip);
        for (i, path) in c.pages.iter().enumerate() {
            let read = c.page_read(i).is_some();
            let picture = match self.thumbs.get(path) {
                Some(Some(file)) => img(file.clone()).size_full().object_fit(ObjectFit::Cover).with_loading(loading_fill).with_fallback(|| icon_ph(ph::IMAGE, Tokens::text_3(), icon_px::MD).into_any_element()).into_any_element(),
                Some(None) => div().size_full().flex().items_center().justify_center().child(icon_ph(ph::IMAGE_BROKEN, Tokens::text_3(), icon_px::MD)).into_any_element(),
                None => loading_fill(),
            };
            strip = strip.child(
                selectable(("mg-page", i), i == self.page)
                    .flex_none()
                    .relative()
                    .p(px(space::XXS))
                    .tooltip(tip(if read { format!("Page {}", i + 1) } else { format!("Page {} · not read yet", i + 1) }))
                    .child(div().w(px(w)).h(px(h)).rounded(px(radius::MD)).overflow_hidden().bg(Tokens::surface_2()).when(!read, |d| d.opacity(0.5)).child(picture))
                    .child(div().absolute().bottom(px(space::XS)).left(px(space::XS)).child(spec(format!("{}", i + 1))))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select_page(i, cx))),
            );
        }
        strip
    }

    /// "Page n of N" with the previous and next buttons and what the page holds.
    fn page_nav(&self, c: &Chapter, cx: &mut Context<Self>) -> Div {
        let total = c.pages.len();
        let page = self.page.min(total.saturating_sub(1));
        let read = c.page_read(page);
        div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .child(icon_button("mg-prev", ph::CARET_LEFT, "Previous page").small().map(|b| if page == 0 { b.disabled("This is the first page") } else { b.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select_page(page - 1, cx))) }))
            .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(format!("Page {} of {total}", page + 1)))
            .child(icon_button("mg-next-page", ph::CARET_RIGHT, "Next page").small().map(|b| if page + 1 >= total { b.disabled("This is the last page") } else { b.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select_page(page + 1, cx))) }))
            .child(div().flex_1())
            .child(match &read {
                Some(p) if !p.panels.is_empty() => muted(format!("{} · click one to see its lines", count(p.panels.len(), "panel"))).into_any_element(),
                Some(_) => muted("No panels found on this page").into_any_element(),
                None => muted("This page is not read yet").into_any_element(),
            })
    }

    fn page_view(&self, c: &Chapter, cx: &mut Context<Self>) -> Div {
        let total = c.pages.len();
        let page = self.page.min(total.saturating_sub(1));
        let Some(path) = c.pages.get(page).cloned() else { return div() };
        let read = c.page_read(page).cloned();
        // A shimmer at the page's usual shape until its preview is ready; a clear note (and the
        // way to open the page itself) if it cannot be made. The original file is never drawn:
        // a long page would be too large for the screen's memory.
        let loading = || div().w(px(PAGE_W)).h(px(PAGE_W * PAGE_LOADING_RATIO)).child(loading_fill()).into_any_element();
        let failed = {
            let original = path.clone();
            move || {
                let original = original.clone();
                div()
                    .w(px(PAGE_W))
                    .h(px(PAGE_W))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(space::SM))
                    .child(icon_ph(ph::IMAGE_BROKEN, Tokens::text_3(), icon_px::LG))
                    .child(muted("This page could not be shown here."))
                    .child(btn("mg-page-open", "Open the page", ButtonKind::Ghost).small().icon(ph::ARROWS_OUT_SIMPLE).tooltip("Opens the page in your picture viewer").on_click(move |_, _, cx| cx.open_with_system(&original)))
                    .into_any_element()
            }
        };
        let (page_img, overlay) = match self.previews.get(&path) {
            Some(Some(file)) => (img(file.clone()).w(px(PAGE_W)).with_loading(loading).with_fallback(failed).into_any_element(), true),
            Some(None) => (failed(), false),
            None => (loading(), false),
        };
        let mut frame = div().relative().flex_none().w(px(PAGE_W)).rounded(px(radius::MD)).overflow_hidden().bg(Tokens::well()).border_1().border_color(Tokens::border()).child(image_cache(self.images.clone()).w(px(PAGE_W)).child(page_img));
        if let Some(p) = read.as_ref().filter(|_| overlay) {
            for (k, panel) in p.panels.iter().enumerate() {
                let Some(b) = panel.bbox else { continue };
                let [x0, y0, x1, y1] = b.0.map(|v| v.clamp(0., 1.));
                if x1 <= x0 || y1 <= y0 {
                    continue;
                }
                let on = self.panel == Some(k);
                frame = frame.child(
                    div()
                        .id(("mg-panel", k))
                        .absolute()
                        .left(relative(x0))
                        .top(relative(y0))
                        .w(relative(x1 - x0))
                        .h(relative(y1 - y0))
                        .rounded(px(radius::XS))
                        .cursor_pointer()
                        .map(|d| if on { d.border_2().border_color(Tokens::accent()).bg(Tokens::accent_a(0.12)) } else { d.border_1().border_color(Tokens::accent_border()).hover(|s| s.bg(Tokens::accent_a(0.08))) })
                        .tooltip(tip(format!("Panel {}", k + 1)))
                        .child(div().absolute().top(px(space::XS)).left(px(space::XS)).child(if on { tag(format!("{}", k + 1), Tone::Accent) } else { spec(format!("{}", k + 1)) }))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.panel = if this.panel == Some(k) { None } else { Some(k) };
                            cx.notify();
                        })),
                );
            }
        }
        div().flex().flex_col().gap(px(space::MD)).child(div().flex().justify_center().child(frame))
    }

    // ------------------------------------------------------------ right: page and panel

    fn inspector(&self, c: &Chapter, narrow: bool, cx: &mut Context<Self>) -> Div {
        let Some(p) = c.page_read(self.page) else { return div() };
        let people = c.read.as_ref().map(people_order).unwrap_or_default();
        let hue = |name: &str| people.iter().position(|n| n.eq_ignore_ascii_case(name.trim())).unwrap_or(people.len());
        let title = match self.panel {
            Some(k) => format!("Page {} · panel {}", self.page + 1, k + 1),
            None => format!("Page {}", self.page + 1),
        };
        let head = column_head(&title).when(narrow, |d| {
            d.child(icon_button("mg-close", ph::X, "Close").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.panel = None;
                cx.notify();
            })))
        });
        let mut body = column_body("mg-inspector");
        match self.panel.and_then(|k| p.panels.get(k).map(|pn| (k, pn))) {
            Some((k, panel)) => {
                let full = source::panel_file(&c.pick, &c.item.manga_title, self.page, k);
                // Not looked at on disk here (render runs every frame): a missing crop shows nothing.
                // The whole panel from its bounded preview, fitted inside the box (a tall webtoon
                // panel stands upright in it); "Open full size" shows the file at its own size.
                let missing = || div().size_full().flex().items_center().justify_center().child(muted("No picture of this panel yet")).into_any_element();
                let picture = match self.previews.get(&full) {
                    Some(Some(file)) => img(file.clone()).size_full().object_fit(ObjectFit::Contain).with_loading(loading_fill).with_fallback(missing).into_any_element(),
                    Some(None) => missing(),
                    None => loading_fill(),
                };
                let open = full.clone();
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(space::XS))
                        .child(
                            div()
                                .w(px(CROP_W))
                                .h(px(CROP_H))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(radius::MD))
                                .overflow_hidden()
                                .bg(Tokens::well())
                                .border_1()
                                .border_color(Tokens::border())
                                .child(image_cache(self.images.clone()).size_full().child(picture)),
                        )
                        .child(div().child(btn(("mg-crop-full", k), "Open full size", ButtonKind::Ghost).small().icon(ph::ARROWS_OUT_SIMPLE).tooltip("Opens the panel in your picture viewer").on_click(move |_, _, cx| cx.open_with_system(&open)))),
                );
                if !panel.description.trim().is_empty() {
                    body = body.child(div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_2()).child(panel.description.trim().to_string()));
                }
                let who: Vec<&str> = panel.characters.iter().map(|ch| ch.name.trim()).filter(|n| !n.is_empty()).collect();
                if !who.is_empty() {
                    let mut chips = div().flex().flex_wrap().gap(px(space::SM));
                    for n in who {
                        chips = chips.child(div().flex().items_center().gap(px(space::XS)).child(marker(hue(n), n, 20.)).child(div().text_size(px(size::SM)).text_color(Tokens::text_1()).child(n.to_string())));
                    }
                    body = body.child(section("In this panel").child(chips));
                }
                let mut lines = section("Lines");
                let spoken: Vec<_> = panel.lines.iter().filter(|l| !l.text.trim().is_empty()).collect();
                if spoken.is_empty() {
                    lines = lines.child(muted("Nobody speaks in this panel."));
                }
                for l in spoken {
                    let name = l.speaker.as_deref().map(str::trim).filter(|s| !s.is_empty());
                    let who = match name {
                        Some(n) => div().flex().items_center().gap(px(space::XS)).child(marker(hue(n), n, 18.)).child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(n.to_string())),
                        None if kind_word(&l.kind) == Some("narration") => div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_2()).child("Narration"),
                        None => div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_3()).child("Unknown speaker"),
                    };
                    lines = lines.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(space::XS))
                            .child(div().flex().items_center().gap(px(space::SM)).child(who).when_some(kind_word(&l.kind).filter(|_| name.is_some() || kind_word(&l.kind) != Some("narration")), |d, k| d.child(tag(k, Tone::Neutral))))
                            .child(div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_1()).child(format!("\u{201c}{}\u{201d}", l.text.trim()))),
                    );
                }
                body = body.child(lines);
            }
            None => {
                let mut list = section("Panels");
                if p.panels.is_empty() {
                    list = list.child(muted("Studio found no panels on this page."));
                }
                for (k, panel) in p.panels.iter().enumerate() {
                    let n = panel.lines.iter().filter(|l| !l.text.trim().is_empty()).count();
                    let first = panel.description.trim().to_string();
                    list = list.child(
                        list_row(("mg-panel-row", k), false).flex_none()
                            .child(spec(format!("{}", k + 1)))
                            .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).child(if first.is_empty() { format!("Panel {}", k + 1) } else { first }))
                            .child(div().flex_none().text_size(px(size::XS)).text_color(Tokens::text_3()).child(if n == 0 { "no lines".to_string() } else { count(n, "line") }))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.panel = Some(k);
                                cx.notify();
                            })),
                    );
                }
                body = body.child(list);
            }
        }
        // The page as Studio read it.
        let s = &p.scene;
        let mut scene = div().flex().flex_wrap().gap(px(space::XS));
        if !s.kind.trim().is_empty() {
            scene = scene.child(spec(s.kind.trim().to_string()));
        }
        if !s.mood.trim().is_empty() {
            scene = scene.child(spec(s.mood.trim().to_string()));
        }
        scene = scene.child(spec(format!("intensity {} / 10", s.intensity.min(10))));
        body = body.child(
            section("This page")
                .child(scene)
                .when(!p.summary.trim().is_empty(), |d| d.child(div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_2()).child(p.summary.trim().to_string()))),
        );
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap(px(space::XS))
                .pt(px(space::SM))
                .child(div().child(btn("mg-fix", "Open Studio", ButtonKind::Ghost).small().trailing_icon(ph::ARROW_UP_RIGHT).tooltip("Opens Studio; pick this chapter there to change its panels").on_click(|_, w, cx| mv_ui::navigate(Route::Studio, w, cx))))
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("This step shows the panels and speakers as Studio read them. They are changed in Studio.")),
        );
        div().flex().flex_col().size_full().child(head).child(body)
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: with test-support, `gpui_kit::*` carries a `test` macro that shadows `#[test]`.
    use super::{kind_word, main_action, people_order, picture_size, preview_size, read_state, state_line, Main, ReadState, MAX_PREVIEW_H};
    use mv_recap::{ChapterRead, CharacterSeen, Line, PageRead, Panel};
    use ui_studio::pipeline::ItemState;

    #[test]
    fn picture_sizes_are_read_from_the_first_bytes() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend(800u32.to_be_bytes());
        png.extend(12_000u32.to_be_bytes());
        assert_eq!(picture_size(&png), Some((800, 12_000)));
        // A JPEG with a block of camera data before its frame header.
        let mut jpg = vec![0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x06, 1, 2, 3, 4, 0xFF, 0xC0, 0x00, 0x11, 8];
        jpg.extend(1_400u16.to_be_bytes());
        jpg.extend(960u16.to_be_bytes());
        assert_eq!(picture_size(&jpg), Some((960, 1_400)));
        let mut webp = b"RIFF\0\0\0\0WEBPVP8X\0\0\0\0\0\0\0\0".to_vec();
        webp.extend([0xFF, 0x02, 0x00, 0x0F, 0x27, 0x00]);
        assert_eq!(picture_size(&webp), Some((768, 10_000)));
        assert_eq!(picture_size(b"not a picture"), None);
        assert_eq!(picture_size(&[0xFF, 0xD8, 0xFF]), None, "cut short");
    }

    #[test]
    fn previews_stay_small_enough_for_the_screen() {
        // A long webtoon page: scaled down whole to the tallest picture a graphics card takes.
        let (w, h) = preview_size(800, 12_000, 960, MAX_PREVIEW_H);
        assert_eq!(h, MAX_PREVIEW_H);
        assert!((w as f32 / h as f32 - 800. / 12_000.).abs() < 0.01);
        // A big page: twice the page's width on screen; a small one is never blown up.
        assert_eq!(preview_size(1_500, 2_100, 960, MAX_PREVIEW_H), (960, 1_344));
        assert_eq!(preview_size(700, 1_000, 960, MAX_PREVIEW_H), (700, 1_000));
        assert_eq!(preview_size(0, 0, 960, MAX_PREVIEW_H), (1, 1));
    }

    #[test]
    fn read_state_follows_the_queue_then_the_files() {
        assert_eq!(read_state(true, 0, 18, None), ReadState::NotRead);
        assert_eq!(read_state(false, 0, 0, None), ReadState::Missing);
        assert_eq!(read_state(true, 12, 18, None), ReadState::Partly { read: 12, total: 18 });
        assert_eq!(read_state(true, 18, 18, None), ReadState::Read);
        assert_eq!(read_state(true, 3, 18, Some((7, ItemState::Running, None))), ReadState::Reading { id: 7 });
        assert_eq!(read_state(true, 0, 18, Some((7, ItemState::Queued, None))), ReadState::Waiting { id: 7 });
        assert_eq!(read_state(true, 0, 18, Some((7, ItemState::Paused, None))), ReadState::Paused { id: 7 });
        // A finished Read wins over a later step that failed or was paused in Studio.
        assert_eq!(read_state(true, 18, 18, Some((7, ItemState::Failed, Some("no model".into())))), ReadState::Read);
        assert_eq!(read_state(true, 18, 18, Some((7, ItemState::Paused, None))), ReadState::Read);
        assert_eq!(read_state(true, 4, 18, Some((7, ItemState::Failed, Some(" No AI model is picked ".into())))), ReadState::Failed { id: 7, error: "No AI model is picked".into() });
        assert_eq!(read_state(true, 0, 18, Some((7, ItemState::Failed, None))), ReadState::Failed { id: 7, error: "Studio could not read this chapter.".into() });
        // Pages read before the download went missing still count.
        assert_eq!(read_state(false, 5, 5, None), ReadState::Read);
    }

    #[test]
    fn rows_say_where_a_chapter_stands() {
        assert_eq!(state_line(&ReadState::Read, 18, None), "Read · 18 pages");
        assert_eq!(state_line(&ReadState::NotRead, 1, None), "Not read yet · 1 page");
        assert_eq!(state_line(&ReadState::Partly { read: 12, total: 18 }, 18, None), "Read 12 of 18 pages");
        assert_eq!(state_line(&ReadState::Reading { id: 1 }, 18, Some((5, 18))), "Reading · 5 of 18 pages");
        assert_eq!(state_line(&ReadState::Reading { id: 1 }, 18, None), "Reading…");
        assert_eq!(state_line(&ReadState::Missing, 0, None), "Not downloaded");
    }

    #[test]
    fn one_main_action() {
        let s = |v: &[ReadState]| v.iter().enumerate().map(|(i, s)| (format!("c{i}"), s.clone())).collect::<Vec<_>>();
        assert_eq!(main_action(&s(&[ReadState::Read, ReadState::NotRead, ReadState::Partly { read: 1, total: 2 }])), Main::Read(vec!["c1".into(), "c2".into()]));
        assert_eq!(main_action(&s(&[ReadState::Read, ReadState::Reading { id: 1 }])), Main::Busy);
        assert_eq!(main_action(&s(&[ReadState::Read, ReadState::Read])), Main::Continue);
        assert_eq!(main_action(&s(&[ReadState::Read, ReadState::Missing])), Main::Blocked);
        assert_eq!(main_action(&s(&[ReadState::Failed { id: 2, error: "x".into() }])), Main::Read(vec!["c0".into()]));
        assert_eq!(main_action(&[]), Main::Blocked);
    }

    #[test]
    fn people_keep_their_first_appearance_order() {
        let panel = |chars: &[&str], speakers: &[Option<&str>]| Panel {
            characters: chars.iter().map(|n| CharacterSeen { name: n.to_string(), ..Default::default() }).collect(),
            lines: speakers.iter().map(|s| Line { speaker: s.map(str::to_string), text: "x".into(), kind: "speech".into() }).collect(),
            ..Default::default()
        };
        let read = ChapterRead {
            pages: vec![
                PageRead { index: 1, panels: vec![panel(&["Ahn"], &[Some("jinwoo")])], ..Default::default() },
                PageRead { index: 0, panels: vec![panel(&["Jinwoo", " "], &[None, Some("Song")])], ..Default::default() },
            ],
            ..Default::default()
        };
        assert_eq!(people_order(&read), vec!["Jinwoo", "Song", "Ahn"]);
        assert_eq!(kind_word("SFX"), Some("sound"));
        assert_eq!(kind_word("speech"), None);
    }
}
