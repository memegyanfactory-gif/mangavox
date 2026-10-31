//! Studio's Video mode: a Premiere Pro-style editor for the current manga's finished recap.
//! Laid out like Premiere's Editing workspace: Effect Controls top left, the Program monitor in the
//! middle, the sequence's Captions / Music / Look on the right; the Project bin, the tools and the
//! timeline (V3 captions, V2 layers, V1 panels, A1 narration, A2 music) below.
//!
//! The monitor is a live preview of the edit (panels, layers and captions drawn here, the narration
//! and music played by `vedit_play`); Render makes the real video through the Python renderer (see
//! `vedit::render_extra`), and Export is the dashboard's Full recap export over those videos.
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, empty_view, icon_button, icon_ph, icon_toggle, interactive, muted, ph, segmented, selected_badge_sized, size, switch, tip, ButtonKind, Tokens};

use crate::data::{ChapterItem, MangaGroup};
use crate::look::{Effect, Preset};
use crate::page::{plural, Load, StudioPage, View};
use crate::restyle::{place_index, set_place, set_size, PLACES, SIZES};
use crate::vedit::{self, Beat, ChapterEdit, Clip, Layer, LayerKind, LookEdit, Motion, MusicSource, Placed, Project, Track};
use crate::vedit_history::History;
use crate::vedit_keys::{self, Command};
use crate::vedit_tl::{self as tl, NavGrip};
use crate::vedit_play;
use crate::video::{self, SubtitleSetting};

mod manga;

/// The timeline's measures (bhippi's): track head column, ruler, a clip's label bar.
const HEAD_W: f32 = 150.;
const RULER_H: f32 = 34.;
const LABEL_H: f32 = 15.;
const NAV_H: f32 = 16.;
const LEFT_W: f32 = 320.;
const RIGHT_W: f32 = 300.;
const TOOLS_W: f32 = 40.;
/// The bottom row (bin, tools, timeline).
const BOTTOM_H: f32 = 376.;
const TICK: Duration = Duration::from_millis(33);
/// How long the clips an assistant turn changed stay lit.
const AI_FLASH: Duration = Duration::from_secs(5);
const SWATCHES: [&str; 6] = ["#ffffff", "#ffe600", "#58d8ff", "#ff2fd2", "#ff9d2e", "#111111"];

fn lane_h(t: Track) -> f32 {
    match t {
        Track::Captions | Track::Layers => 40.,
        Track::Clips | Track::Narration => 52.,
        Track::Music => 44.,
    }
}

/// A track's clip colour (bhippi's palette in the theme's tokens): picture blue, sound green,
/// captions the accent, layers the second accent.
fn track_color(t: Track) -> Rgba {
    match t {
        Track::Clips => Tokens::info(),
        Track::Narration | Track::Music => Tokens::success(),
        Track::Captions => Tokens::accent(),
        Track::Layers => Tokens::accent_2(),
    }
}

/// A clip on a lane `h` tall: 3 px corners, a 1 px border lighter than its fill, and a 15 px label
/// bar. `ai`: the assistant just changed it.
fn clip_box(id: SharedString, x: f32, w: f32, h: f32, c: Rgba, selected: bool, ai: bool, icon: Option<mv_ui::Ph>, label: String) -> Stateful<Div> {
    let border = if selected {
        Tokens::text_1()
    } else if ai {
        Tokens::warning()
    } else {
        soft(c, 0.95)
    };
    div()
        .id(id)
        .absolute()
        .left(px(x))
        .top(px(1.))
        .w(px(w.max(2.)))
        .h(px(h - 2.))
        .rounded(px(3.))
        .overflow_hidden()
        .bg(soft(c, if ai { 0.55 } else { 0.38 }))
        .border_1()
        .border_color(border)
        .when(selected || ai, |d| d.border_2())
        .when(w > 18., |d| {
            d.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(LABEL_H))
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .px(px(4.))
                    .bg(soft(c, 0.72))
                    .children(icon.map(|i| icon_ph(i, Tokens::text_1(), 10.)))
                    .child(div().min_w_0().truncate().text_size(px(10.)).text_color(Tokens::text_1()).child(label)),
            )
        })
}

/// A waveform filling its box (one column per entry of `slice`).
fn wave(slice: Vec<(f32, f32)>) -> Canvas<()> {
    canvas(|_, _, _| (), move |b, _, window, _| crate::vedit_media::paint_waveform(b, &slice, window))
}

/// The visible part of a clip at `x`, `w` wide on lanes `view_w` wide: (from, to) in clip pixels.
fn visible_part(x: f32, w: f32, view_w: f32) -> Option<(f32, f32)> {
    let (a, b) = ((-x).max(0.), (view_w - x).min(w));
    (b - a >= 1.).then_some((a, b))
}

/// A clip's sound as a waveform under its label bar: `peaks` from `src_s` seconds at the clip's
/// left edge, at `pps`; only the visible part is drawn.
fn clip_wave(peaks: &crate::vedit_media::Peaks, src_s: f64, x: f32, w: f32, h: f32, pps: f32, view_w: f32) -> Option<Div> {
    let (a, b) = visible_part(x, w, view_w)?;
    let cols = (((b - a) / 2.).ceil() as usize).clamp(1, 1600);
    let slice = peaks.slice(src_s + a as f64 / pps as f64, src_s + b as f64 / pps as f64, cols);
    Some(div().absolute().left(px(a)).w(px(b - a)).top(px(LABEL_H)).h(px((h - 4. - LABEL_H).max(4.))).child(wave(slice).size_full()))
}

/// A panel clip's picture: its thumbnail tiled along the clip (not stretched), visible tiles only.
fn clip_tiles(image: &Path, x: f32, w: f32, h: f32, view_w: f32) -> Vec<AnyElement> {
    let body = (h - 4. - LABEL_H).max(8.);
    let Some((thumb, aspect)) = crate::vedit_media::thumb_ready(image, 90) else { return vec![] };
    let Some((a, b)) = visible_part(x, w, view_w) else { return vec![] };
    let tw = (body * aspect).max(8.);
    let (k0, k1) = ((a / tw).floor() as usize, ((b / tw).ceil() as usize).min((a / tw).floor() as usize + 60));
    (k0..k1)
        .map(|k| img(thumb.clone()).absolute().left(px(k as f32 * tw)).top(px(LABEL_H)).w(px(tw)).h(px(body)).object_fit(ObjectFit::Cover).opacity(0.85).into_any_element())
        .collect()
}

fn hex(s: &str) -> Rgba {
    video::parse_color(s).unwrap_or(Rgba { r: 0., g: 0., b: 0., a: 1. })
}

fn soft(c: Rgba, a: f32) -> Rgba {
    Rgba { a, ..c }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tool {
    Select,
    Razor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sel {
    Clip(usize),
    Layer(u64),
    Track(Track),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    Playhead,
    /// Reordering a clip: where it was grabbed (ms into it) and the pointer time.
    Clip { index: usize, grab: i64, at: i64 },
    /// Trimming a clip edge: the pointer time it started at, and what is applied so far.
    Trim { index: usize, head: bool, from: i64, applied: i64 },
    Layer { id: u64, grab: i64 },
    LayerEdge { id: u64, head: bool },
    /// A layer moved on the monitor.
    Place { id: u64 },
    /// The captions moved on the monitor (every caption of the manga); where on the caption it
    /// was grabbed, in % of the frame from its centre, and where they were before.
    Caption { grab: (f32, f32), from: Option<(f32, f32)> },
    /// A value bar in the controls.
    Bar(&'static str),
    /// The navigator under the timeline: panning its thumb, or zooming with a handle; where the
    /// pointer, zoom and scroll were when it began.
    Nav { grip: NavGrip, x0: f32, pps0: f32, scroll0: f32 },
}

/// The assistant turn being folded into one undo step.
#[derive(Clone, Copy, Debug)]
struct AiTurn {
    turn: u64,
    /// The history step before the turn.
    mark: usize,
}

/// The open chapter.
pub(crate) struct Seq {
    pub item: ChapterItem,
    pub beats: Vec<Beat>,
    pub edit: ChapterEdit,
    pub subs: SubtitleSetting,
    pub score: Option<PathBuf>,
    pub reels: bool,
}

impl Seq {
    fn placed(&self) -> Vec<Placed> {
        vedit::place(&self.beats, &self.edit.clips(&self.beats))
    }
}

/// The Video mode's state (one per Studio page).
pub(crate) struct Editor {
    manga: Option<String>,
    project: Project,
    pub(crate) seq: Option<Seq>,
    playhead: u64,
    playing: Option<(Instant, u64)>,
    /// Zoom (pixels per second) and the timeline's scroll (pixels).
    pps: f32,
    scroll_x: f32,
    snap: bool,
    snapped: Option<u64>,
    /// Zoom to fit once the timeline has been laid out (its width is known).
    fit_pending: bool,
    tool: Tool,
    sel: Option<Sel>,
    drag: Option<Drag>,
    bin_tab: usize,
    side_tab: usize,
    pub(crate) export_open: bool,
    lanes: Rc<Cell<Bounds<Pixels>>>,
    monitor: Rc<Cell<Bounds<Pixels>>>,
    bars: Rc<RefCell<HashMap<&'static str, Bounds<Pixels>>>>,
    text: Option<Entity<InputState>>,
    text_for: Option<Sel>,
    /// The open chapter's labelled undo history (`vedit_history`).
    hist: History<ChapterEdit>,
    /// The assistant turn whose edits fold into one step, and the clips / layers it changed
    /// (lit for a few seconds).
    ai: Option<AiTurn>,
    ai_flash: Option<(Instant, Vec<usize>, Vec<u64>)>,
    /// Shift held during a drag (inverts snapping).
    shift: bool,
    /// In / out points and markers (this session's, on the open timeline).
    mark_in: Option<u64>,
    mark_out: Option<u64>,
    markers: Vec<u64>,
    /// J / L shuttle: when it began, from where, and the speed (negative: backwards); and the
    /// last direction with how many presses in a row.
    shuttle: Option<(Instant, u64, f32)>,
    shuttle_n: (i8, u32),
    /// The keyboard shortcuts list is open.
    keys_open: bool,
    /// Inspector sections folded shut.
    collapsed: HashSet<&'static str>,
    /// The navigator bar's bounds.
    nav: Rc<Cell<Bounds<Pixels>>>,
    /// `vedit_media::loaded_version` last painted.
    seen_media: u64,
    /// The Video mode export's choices.
    pub(crate) vexport: crate::export::PresetExport,
    /// The dashboard's export options are shown in the Export dialog.
    export_more: bool,
    /// Render keys of the chapters not open (worked out once; see `unrendered`) and the bin's
    /// first-page thumbnails.
    keys: RefCell<HashMap<String, u64>>,
    thumbs: RefCell<HashMap<String, Option<PathBuf>>>,
    /// The assistant tools' edit version last loaded (`vedit_tools::version`).
    seen_tools: u64,
    /// The grid anchor a dragged caption snapped to.
    cap_lit: Option<usize>,
    /// Hearted caption templates (shared with the caption studio, see `video::favorites`) and
    /// when their file was read.
    favs: Vec<String>,
    favs_at: Option<SystemTime>,
    /// The whole-manga timeline ("All chapters") and Re-edit full timeline (see `manga`).
    whole: manga::Whole,
    _subs: Vec<Subscription>,
}

impl Default for Editor {
    fn default() -> Self {
        Editor {
            manga: None,
            project: Project::default(),
            seq: None,
            playhead: 0,
            playing: None,
            pps: 60.,
            scroll_x: 0.,
            snap: true,
            snapped: None,
            fit_pending: false,
            tool: Tool::Select,
            sel: None,
            drag: None,
            bin_tab: 0,
            side_tab: 0,
            export_open: false,
            lanes: Rc::new(Cell::new(Bounds::default())),
            monitor: Rc::new(Cell::new(Bounds::default())),
            bars: Rc::new(RefCell::new(HashMap::new())),
            text: None,
            text_for: None,
            hist: History::new(ChapterEdit::default()),
            ai: None,
            ai_flash: None,
            shift: false,
            mark_in: None,
            mark_out: None,
            markers: vec![],
            shuttle: None,
            shuttle_n: (0, 0),
            keys_open: false,
            collapsed: HashSet::new(),
            nav: Rc::new(Cell::new(Bounds::default())),
            seen_media: 0,
            vexport: Default::default(),
            export_more: false,
            keys: RefCell::new(HashMap::new()),
            thumbs: RefCell::new(HashMap::new()),
            seen_tools: crate::vedit_tools::version(),
            cap_lit: None,
            favs: vec![],
            favs_at: None,
            whole: Default::default(),
            _subs: vec![],
        }
    }
}

/// A chapter with a finished video.
fn made(statuses: &HashMap<String, mv_recap::PhaseStatus>, c: &ChapterItem) -> bool {
    statuses.get(&c.id).and_then(|s| s.video.as_ref()).is_some_and(|p| p.is_file())
}

impl StudioPage {
    // ------------------------------------------------------------------------------ loading

    /// The manga the Video mode edits: the dashboard's, else the Editor's chapter's.
    fn video_manga(&self) -> Option<MangaGroup> {
        let Load::Ready(groups) = &self.library else { return None };
        match self.dash_manga.clone().or_else(|| self.focus.as_ref().map(|f| f.item.manga_id.clone())) {
            Some(id) => groups.iter().find(|g| g.id == id).cloned(),
            // Nothing picked yet: the first manga with a finished chapter.
            None => groups.iter().find(|g| g.chapters.iter().any(|c| made(&self.statuses, c))).cloned(),
        }
    }

    fn made_chapters(&self, g: &MangaGroup) -> Vec<ChapterItem> {
        g.chapters.iter().filter(|c| made(&self.statuses, c)).cloned().collect()
    }

    fn manga_dir(item: &ChapterItem) -> PathBuf {
        item.output_dir.parent().map(Path::to_path_buf).unwrap_or_else(|| item.output_dir.clone())
    }

    /// Opens the manga (its project file and first finished chapter) when it changed.
    fn video_sync(&mut self) {
        let g = self.video_manga();
        let id = g.as_ref().map(|g| g.id.clone());
        if self.vedit.manga == id {
            return;
        }
        self.video_stop();
        self.vedit.manga = id;
        self.vedit.seq = None;
        self.vedit.sel = None;
        self.manga_reset();
        let Some(g) = g else { return };
        let chapters = self.made_chapters(&g);
        if let Some(first) = chapters.first() {
            self.vedit.project = Project::load(&Self::manga_dir(first));
            self.video_open(first.clone());
        }
    }

    /// Opens `item` as the timeline's sequence.
    fn video_open(&mut self, item: ChapterItem) {
        self.video_stop();
        self.vedit.whole.all = None;
        if let Some(prev) = &self.vedit.seq {
            self.vedit.keys.borrow_mut().remove(&prev.item.id);
        }
        let pages = mv_recap::list_pages(&item.pages_dir);
        let beats = vedit::load_beats(&item.output_dir, &pages);
        let edit = self.vedit.project.chapters.get(&item.id).cloned().unwrap_or_default();
        let subs = self.vedit.project.captions(&video::subtitle_setting_for(&item));
        let score = Some(mv_score::score_wav(&crate::page::chref(&item))).filter(|p| p.is_file());
        let reels = self.statuses.get(&item.id).is_some_and(|s| !s.reels.is_empty());
        self.vedit.hist = History::new(edit.clone());
        self.vedit.ai = None;
        self.vedit.ai_flash = None;
        self.vedit.seq = Some(Seq { item, beats, edit, subs, score, reels });
        self.vedit.playhead = 0;
        self.vedit.scroll_x = 0.;
        self.vedit.sel = None;
        self.vedit.mark_in = None;
        self.vedit.mark_out = None;
        self.vedit.markers.clear();
        self.vedit.fit_pending = true;
        self.video_tell_assistant();
    }

    /// Tells the assistant what the Video mode has open ("this chapter" in the chat).
    fn video_tell_assistant(&self) {
        let Some(s) = &self.vedit.seq else { return };
        crate::vedit_tools::set_current(&s.item.manga_id, &s.item.id);
        if self.view == View::Video {
            let placed = s.placed();
            ui_assistant::set_studio_context(Some(format!(
                "The user is in Studio's Video editor on \"{}\" {} ({} clips, {}). Use the video_* tools to edit it (video_timeline to inspect, video_restyle / video_edit_clips for passes over many clips, video_render at the end); chat edits show on their timeline at once.",
                s.item.manga_title,
                s.item.label(),
                placed.len(),
                vedit::timecode(vedit::total_ms(&placed))
            )));
        }
    }

    /// Called by the page's poll: chat edits (the assistant's `video_*` tools) reach the open
    /// timeline (the edit before them stays an undo step), and its Open / Export requests run.
    pub(crate) fn video_poll(&mut self, cx: &mut Context<Self>) {
        match crate::vedit_tools::take_request() {
            Some(crate::vedit_tools::Request::Open { manga, chapter }) => {
                self.view = View::Video;
                self.dash_manga = Some(manga);
                self.video_sync();
                if let Some(c) = self.video_manga().and_then(|g| g.chapters.into_iter().find(|c| c.id == chapter)) {
                    self.video_open(c);
                }
                cx.notify();
            }
            Some(crate::vedit_tools::Request::Export { manga, prefs }) => {
                self.view = View::Video;
                self.dash_manga = Some(manga);
                self.export_prefs = prefs;
                self.video_stop();
                self.vedit.export_open = true;
                // The chat chose the dashboard's options (parts, hook, quality): show them.
                self.vedit.export_more = true;
                crate::encode::warm();
                cx.notify();
            }
            None => {}
        }
        self.manga_poll(cx);
        // Waveforms and thumbnails made in the background since the last paint.
        let media = crate::vedit_media::loaded_version();
        if media != self.vedit.seen_media && self.view == View::Video {
            self.vedit.seen_media = media;
            cx.notify();
        }
        let v = crate::vedit_tools::version();
        if v == self.vedit.seen_tools {
            return;
        }
        self.vedit.seen_tools = v;
        self.vedit.keys.borrow_mut().clear();
        let Some(seq) = self.vedit.seq.as_mut() else { return };
        let dir = seq.item.output_dir.parent().map(Path::to_path_buf).unwrap_or_else(|| seq.item.output_dir.clone());
        self.vedit.project = Project::load(&dir);
        let fresh = self.vedit.project.chapters.get(&seq.item.id).cloned().unwrap_or_default();
        seq.subs = self.vedit.project.captions(&video::subtitle_setting_for(&seq.item));
        if fresh != seq.edit {
            // One undo step per chat turn, labelled with what was asked: every tool edit of the
            // turn is folded into the step the turn's first edit made.
            let (turn, prompt) = crate::vedit_tools::last_turn().unwrap_or((0, "chat edit".into()));
            let (ai, first) = match self.vedit.ai {
                Some(a) if a.turn == turn && self.vedit.hist.mark() == a.mark + 1 => (a, false),
                _ => (AiTurn { turn, mark: self.vedit.hist.mark() }, true),
            };
            self.vedit.ai = Some(ai);
            self.vedit.hist.commit("AI: edit", fresh.clone());
            self.vedit.hist.squash_turn(ai.mark, &tl::ai_label(&prompt));
            seq.edit = fresh;
            self.vedit.sel = None;
            self.video_flash_turn(first, cx);
        }
        cx.notify();
    }

    /// Lights the clips and layers the current assistant turn changed, for a few seconds, and
    /// offers to revert the turn.
    fn video_flash_turn(&mut self, toast_it: bool, cx: &mut Context<Self>) {
        let Some(ai) = self.vedit.ai else { return };
        let Some(seq) = &self.vedit.seq else { return };
        // The edit before the turn (its history step), to compare with.
        let h = &mut self.vedit.hist;
        let label = h.undo_label().unwrap_or("AI").to_string();
        let now = h.mark();
        let before = if h.jump(ai.mark) { h.current().clone() } else { ChapterEdit::default() };
        h.jump(now);
        let clips = tl::changed_clips(&before.clips(&seq.beats), &seq.edit.clips(&seq.beats));
        let layers = tl::changed_layers(&before.layers, &seq.edit.layers);
        self.vedit.ai_flash = Some((Instant::now(), clips, layers));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(AI_FLASH).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
        if !toast_it {
            return;
        }
        let page = cx.entity().downgrade();
        toast::show(
            cx,
            Toast::info(format!("{label} · one undo step")).icon(ph::SPARKLE).action("Revert", move |_, cx| {
                let _ = page.update(cx, |p, cx| p.video_revert_turn(cx));
            }),
        );
    }

    /// Undoes the last assistant turn (when nothing was edited after it).
    fn video_revert_turn(&mut self, cx: &mut Context<Self>) {
        let Some(ai) = self.vedit.ai else { return };
        if self.vedit.hist.mark() != ai.mark + 1 {
            toast::show(cx, Toast::info("Undo the edits made after the chat's first"));
            return;
        }
        self.video_undo(false, cx);
    }

    /// Zoom so the whole sequence fits the timeline.
    fn video_fit(&mut self) {
        if self.vedit.whole.all.is_some() {
            return self.manga_fit();
        }
        let total = self.vedit.seq.as_ref().map(|s| vedit::total_ms(&s.placed())).unwrap_or(0);
        let w = (f32::from(self.vedit.lanes.get().size.width)).max(400.);
        if total > 0 {
            self.vedit.pps = (w / (total as f32 / 1000.)).clamp(0.2, 400.);
            self.vedit.scroll_x = 0.;
        }
    }

    // ------------------------------------------------------------------------------ editing

    /// Applies `f` to the open chapter's edit, keeps it as an undo step called `label` and saves
    /// the project.
    fn video_edit(&mut self, label: &str, f: impl FnOnce(&mut Seq), cx: &mut Context<Self>) {
        let Some(seq) = self.vedit.seq.as_mut() else { return };
        let before = seq.edit.clone();
        f(seq);
        if seq.edit != before {
            self.vedit.hist.commit(label, seq.edit.clone());
            self.video_save(cx);
        }
        cx.notify();
    }

    /// The edit changed in place (a drag): one undo step for the whole gesture.
    fn video_commit(&mut self, label: &str, cx: &mut Context<Self>) {
        let Some(seq) = &self.vedit.seq else { return };
        if seq.edit != *self.vedit.hist.current() {
            self.vedit.hist.commit(label, seq.edit.clone());
            self.video_save(cx);
        }
    }

    /// Edits the clip list (with the beats it was made on).
    fn video_clips(&mut self, label: &str, f: impl FnOnce(&[Beat], &mut Vec<Clip>), cx: &mut Context<Self>) {
        self.video_edit(
            label,
            |s| {
                let mut clips = s.edit.clips(&s.beats);
                f(&s.beats, &mut clips);
                s.edit.set_clips(&s.beats, clips);
            },
            cx,
        );
    }

    /// Saves the open chapter's edit (other chapters keep what is on disk: the assistant may have
    /// changed them).
    fn video_save(&mut self, cx: &mut Context<Self>) {
        let Some(seq) = &self.vedit.seq else { return };
        let (id, edit) = (seq.item.id.clone(), seq.edit.clone());
        match vedit::update_project(&Self::manga_dir(&seq.item), |p| {
            p.chapters.insert(id, edit);
        }) {
            Ok(p) => self.vedit.project = p,
            Err(e) => toast::show(cx, Toast::error(format!("Could not save the edit: {e}"))),
        }
        self.manga_sync(cx);
    }

    /// Ctrl+Z / Ctrl+Shift+Z: steps the labelled history and says which step it was.
    fn video_undo(&mut self, redo: bool, cx: &mut Context<Self>) {
        if self.vedit.seq.is_none() {
            return;
        }
        let label = if redo { self.vedit.hist.redo() } else { self.vedit.hist.undo() };
        if let (Some(label), Some(seq)) = (label, self.vedit.seq.as_mut()) {
            seq.edit = self.vedit.hist.current().clone();
            self.vedit.sel = None;
            self.vedit.ai_flash = None;
            self.video_save(cx);
            toast::show(cx, Toast::info(format!("{} {label}", if redo { "Redid" } else { "Undid" })).icon(if redo { ph::ARROW_CLOCKWISE } else { ph::ARROW_COUNTER_CLOCKWISE }));
        }
        cx.notify();
    }

    /// Saves the caption setting to the chapter (the file the renderer and caption studio share).
    fn video_subs(&mut self, f: impl FnOnce(&mut SubtitleSetting), cx: &mut Context<Self>) {
        let Some(seq) = self.vedit.seq.as_mut() else { return };
        f(&mut seq.subs);
        video::save_chapter_setting(&seq.item.output_dir, Some(&seq.subs));
        self.manga_sync(cx);
        cx.notify();
    }

    /// Saves where the captions sit (the open chapter's caption setting) as the place of every
    /// caption in the manga: the project file, which every chapter's render reads.
    fn video_caption_pos_save(&mut self, cx: &mut Context<Self>) {
        let Some(seq) = &self.vedit.seq else { return };
        video::save_chapter_setting(&seq.item.output_dir, Some(&seq.subs));
        let pos = seq.subs.pos;
        match vedit::update_project(&Self::manga_dir(&seq.item), |p| p.caption_pos = pos) {
            Ok(p) => self.vedit.project = p,
            Err(e) => toast::show(cx, Toast::error(format!("Could not save the caption place: {e}"))),
        }
        // Every chapter's render changed.
        self.vedit.keys.borrow_mut().clear();
        cx.notify();
    }

    /// Reads the hearted caption templates again when their file changed (the caption studio
    /// shares it).
    fn video_favs_refresh(&mut self) {
        let at = std::fs::metadata(video::defaults_file()).and_then(|m| m.modified()).ok();
        if at != self.vedit.favs_at {
            self.vedit.favs_at = at;
            self.vedit.favs = video::favorites();
        }
    }

    fn video_toggle_fav(&mut self, id: &str, cx: &mut Context<Self>) {
        let mut favs = video::favorites();
        vedit::toggle_favorite(&mut favs, id);
        video::save_favorites(&favs);
        self.vedit.favs = favs;
        self.vedit.favs_at = std::fs::metadata(video::defaults_file()).and_then(|m| m.modified()).ok();
        cx.notify();
    }

    fn locked(&self, t: Track) -> bool {
        self.vedit.seq.as_ref().is_some_and(|s| s.edit.flags(t).locked)
    }

    fn video_delete_selection(&mut self, cx: &mut Context<Self>) {
        match self.vedit.sel {
            Some(Sel::Clip(i)) if !self.locked(Track::Clips) => {
                self.video_clips("Delete clip", |_, c| vedit::delete(c, i), cx);
                self.vedit.sel = None;
            }
            Some(Sel::Layer(id)) if !self.locked(Track::Layers) => {
                self.video_edit("Delete layer", |s| s.edit.layers.retain(|l| l.id != id), cx);
                self.vedit.sel = None;
            }
            Some(Sel::Track(t)) => self.video_delete_track(t, cx),
            _ => {}
        }
    }

    /// "Delete" on a track: captions off, every layer gone, no music, no narration.
    fn video_delete_track(&mut self, t: Track, cx: &mut Context<Self>) {
        if self.locked(t) {
            return;
        }
        match t {
            Track::Clips => toast::show(cx, Toast::info("The panels are the picture: delete or trim clips instead")),
            Track::Layers => self.video_edit("Delete layers", |s| s.edit.layers.clear(), cx),
            Track::Music => self.video_edit("Remove music", |s| s.edit.music.source = MusicSource::None, cx),
            Track::Captions | Track::Narration => self.video_edit("Turn track off", |s| s.edit.flags_mut(t).off = true, cx),
        }
    }

    /// Shift+Delete: the selected clip goes and everything after it moves up, the layers too
    /// (plain Delete leaves the layers where they are in time). A layer's ripple closes its gap.
    fn video_ripple_delete(&mut self, cx: &mut Context<Self>) {
        let Some(seq) = &self.vedit.seq else { return };
        match self.vedit.sel {
            Some(Sel::Clip(i)) if !self.locked(Track::Clips) => {
                let Some(p) = seq.placed().into_iter().find(|p| p.index == i) else { return };
                let shift_layers = !self.locked(Track::Layers);
                self.video_edit(
                    "Ripple delete",
                    |s| {
                        let mut clips = s.edit.clips(&s.beats);
                        vedit::delete(&mut clips, i);
                        s.edit.set_clips(&s.beats, clips);
                        if shift_layers {
                            tl::ripple_layers(&mut s.edit.layers, p.end_ms(), p.dur_ms);
                        }
                    },
                    cx,
                );
                self.vedit.sel = None;
            }
            Some(Sel::Layer(id)) if !self.locked(Track::Layers) => {
                let Some(l) = seq.edit.layers.iter().find(|l| l.id == id).cloned() else { return };
                self.video_edit(
                    "Ripple delete",
                    |s| {
                        s.edit.layers.retain(|x| x.id != id);
                        tl::ripple_layers(&mut s.edit.layers, l.end_ms, l.end_ms - l.start_ms);
                    },
                    cx,
                );
                self.vedit.sel = None;
            }
            _ => self.video_delete_selection(cx),
        }
    }

    /// Q / W: trims the clip under the playhead from its start (or to its end) to the playhead;
    /// the clips after it move up.
    fn video_ripple_trim(&mut self, head: bool, cx: &mut Context<Self>) {
        let Some(seq) = &self.vedit.seq else { return };
        if self.locked(Track::Clips) {
            return;
        }
        let now = self.vedit.playhead;
        let Some(p) = vedit::at(&seq.placed(), now).cloned() else { return };
        let delta = if head { now - p.start_ms } else { p.end_ms() - now } as i64;
        if delta <= 0 {
            return;
        }
        self.video_clips(if head { "Ripple trim start" } else { "Ripple trim end" }, |b, c| vedit::trim(b, c, p.index, head, delta), cx);
        if head {
            self.seek(p.start_ms, cx);
        }
    }

    /// Alt+Left / Right: a layer moves a frame; a clip swaps with its neighbour.
    fn video_nudge(&mut self, dir: i64, cx: &mut Context<Self>) {
        match self.vedit.sel {
            Some(Sel::Layer(id)) if !self.locked(Track::Layers) => {
                let f = (1000 / vedit::FPS) as i64 * dir;
                self.video_edit(
                    "Nudge layer",
                    |s| {
                        if let Some(l) = s.edit.layers.iter_mut().find(|l| l.id == id) {
                            let d = l.end_ms - l.start_ms;
                            l.start_ms = (l.start_ms as i64 + f).max(0) as u64;
                            l.end_ms = l.start_ms + d;
                        }
                    },
                    cx,
                );
            }
            Some(Sel::Clip(i)) if !self.locked(Track::Clips) => {
                let n = self.vedit.seq.as_ref().map(|s| s.edit.clips(&s.beats).len()).unwrap_or(0);
                let to = i as i64 + dir;
                if to < 0 || to as usize >= n {
                    return;
                }
                self.video_clips("Move clip", |_, c| vedit::reorder(c, i, to as usize), cx);
                self.vedit.sel = Some(Sel::Clip(to as usize));
            }
            _ => {}
        }
    }

    /// `[` / `]`: the music a decibel down / up.
    fn video_volume(&mut self, db: f32, cx: &mut Context<Self>) {
        if self.locked(Track::Music) {
            return;
        }
        self.video_edit(if db > 0. { "Volume up" } else { "Volume down" }, |s| s.edit.music.volume = tl::nudge_db(s.edit.music.volume, db), cx);
    }

    /// The snap targets of the open timeline: the clip edges, the layers' (but `skip`'s), the
    /// playhead, the in / out points and the markers.
    fn snap_targets(&self, skip: Option<u64>) -> Vec<u64> {
        let Some(seq) = &self.vedit.seq else { return vec![] };
        let placed = seq.placed();
        let mut pts = vedit::snap_points(&placed, &seq.edit.layers, self.vedit.playhead, skip);
        pts.extend(placed.iter().map(|p| p.start_ms));
        pts.extend(self.vedit.mark_in);
        pts.extend(self.vedit.mark_out);
        pts.extend(self.vedit.markers.iter().copied());
        pts
    }

    fn video_split_at_playhead(&mut self, cx: &mut Context<Self>) {
        let Some(seq) = &self.vedit.seq else { return };
        if self.locked(Track::Clips) {
            return;
        }
        let placed = seq.placed();
        let ph_ms = self.vedit.playhead;
        let Some(p) = vedit::at(&placed, ph_ms).cloned() else { return };
        let mut ok = false;
        self.video_clips("Split clip", |b, c| ok = vedit::split(b, c, p.index, ph_ms - p.start_ms), cx);
        if !ok {
            toast::show(cx, Toast::info("Too close to the clip's edge to cut there"));
        }
    }

    fn add_layer(&mut self, kind: LayerKind, cx: &mut Context<Self>) {
        let start = self.vedit.playhead;
        let mut id = 0;
        self.video_edit("Add layer", 
            |s| {
                let total = vedit::total_ms(&s.placed());
                let (dur, x, y, scale, opacity) = match &kind {
                    LayerKind::Image { .. } => (5_000, 50., 50., 0.3, 1.),
                    LayerKind::Text { .. } => (4_000, 50., 22., 0.06, 1.),
                };
                id = s.edit.next_layer_id();
                let start = start.min(total.saturating_sub(vedit::MIN_MS));
                s.edit.layers.push(Layer { id, kind, start_ms: start, end_ms: (start + dur).min(total.max(start + vedit::MIN_MS)), x, y, scale, opacity });
            },
            cx,
        );
        self.vedit.sel = Some(Sel::Layer(id));
    }

    /// The Look & branding logo as a layer over the whole chapter, in the top-right corner.
    fn add_logo_layer(&mut self, cx: &mut Context<Self>) {
        let Some(path) = crate::look::Look::load().logo.path.filter(|p| p.is_file()) else {
            toast::show(cx, Toast::info("Add a logo in Look & branding first, or import an image"));
            return;
        };
        let mut id = 0;
        self.video_edit("Add logo", 
            |s| {
                id = s.edit.next_layer_id();
                let total = vedit::total_ms(&s.placed());
                s.edit.layers.push(Layer { id, kind: LayerKind::Image { path }, start_ms: 0, end_ms: total, x: 90., y: 12., scale: 0.12, opacity: 0.85 });
            },
            cx,
        );
        self.vedit.sel = Some(Sel::Layer(id));
    }

    fn import_image(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.vedit.seq.as_ref().map(|s| Self::manga_dir(&s.item)) else { return };
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Add this image".into()) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(src) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| match vedit::import_media(&src, &dir) {
                Ok(path) => this.add_layer(LayerKind::Image { path }, cx),
                Err(e) => toast::show(cx, Toast::warning(format!("Could not add that image: {e}"))),
            });
        })
        .detach();
    }

    fn import_music(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.vedit.seq.as_ref().map(|s| Self::manga_dir(&s.item)) else { return };
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Use this music".into()) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(src) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| match vedit::import_media(&src, &dir) {
                Ok(path) => {
                    this.video_edit("Use music file", 
                        |s| {
                            s.edit.music.source = MusicSource::File { path };
                            s.edit.flags_mut(Track::Music).off = false;
                        },
                        cx,
                    );
                    this.vedit.sel = Some(Sel::Track(Track::Music));
                }
                Err(e) => toast::show(cx, Toast::warning(format!("Could not use that music: {e}"))),
            });
        })
        .detach();
    }

    // ------------------------------------------------------------------------------ playback

    /// The playhead of whichever timeline is open (manga time in "All chapters").
    fn now_any(&self) -> u64 {
        if self.vedit.whole.all.is_some() {
            self.manga_now()
        } else {
            self.playhead_now()
        }
    }

    fn play_any(&mut self, cx: &mut Context<Self>) {
        self.vedit.shuttle = None;
        if self.vedit.whole.all.is_some() {
            self.manga_toggle_play(cx)
        } else {
            self.video_toggle_play(cx)
        }
    }

    /// J / K / L: J and L shuttle backwards / forwards, faster with each press (1x, 2x, 4x, 8x;
    /// L once plays with sound), K stops.
    fn video_shuttle(&mut self, dir: i8, cx: &mut Context<Self>) {
        if dir == 0 {
            self.vedit.shuttle_n = (0, 0);
            self.video_stop();
            cx.notify();
            return;
        }
        let (d, n) = self.vedit.shuttle_n;
        let n = if d == dir { n + 1 } else { 1 };
        self.vedit.shuttle_n = (dir, n);
        let playing = self.vedit.playing.is_some() || self.vedit.whole.all.as_ref().is_some_and(|m| m.playing.is_some());
        if dir > 0 && n == 1 {
            if !playing {
                self.play_any(cx);
            }
            return;
        }
        let from = self.now_any();
        self.video_stop();
        let speed = vedit_keys::shuttle_speed(n) * dir as f32;
        let running = self.vedit.shuttle.is_some();
        self.vedit.shuttle = Some((Instant::now(), from, speed));
        if running {
            return;
        }
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(TICK).await;
            let alive = this.update(cx, |this, cx| {
                let Some((t0, from, speed)) = this.vedit.shuttle else { return false };
                let total = this.timeline_ms();
                let t = (from as f64 + t0.elapsed().as_millis() as f64 * speed as f64).clamp(0., total as f64) as u64;
                if this.vedit.whole.all.is_some() {
                    this.manga_set_playhead(t);
                } else {
                    this.vedit.playhead = t;
                    this.follow_playhead();
                }
                if t == 0 || t >= total || this.view != View::Video {
                    this.vedit.shuttle = None;
                    this.vedit.shuttle_n = (0, 0);
                }
                cx.notify();
                this.vedit.shuttle.is_some()
            });
            if !matches!(alive, Ok(true)) {
                break;
            }
        })
        .detach();
        cx.notify();
    }

    fn video_stop(&mut self) {
        self.vedit.shuttle = None;
        self.manga_stop();
        if let Some((t0, from)) = self.vedit.playing.take() {
            self.vedit.playhead = from + t0.elapsed().as_millis() as u64;
            vedit_play::stop();
        }
    }

    fn video_toggle_play(&mut self, cx: &mut Context<Self>) {
        if self.vedit.playing.is_some() {
            self.video_stop();
            cx.notify();
            return;
        }
        let Some(seq) = &self.vedit.seq else { return };
        let placed = seq.placed();
        let total = vedit::total_ms(&placed);
        if total == 0 {
            return;
        }
        if self.vedit.playhead + 50 >= total {
            self.vedit.playhead = 0;
        }
        let from = self.vedit.playhead;
        let narration = if seq.edit.flags(Track::Narration).off {
            vec![]
        } else {
            placed
                .iter()
                .filter(|p| p.end_ms() > from)
                .filter_map(|p| {
                    let b = seq.beats.get(p.clip.src)?;
                    Some(vedit_play::Slice { file: b.audio.clone(), offset_ms: b.offset_ms + p.clip.trim_in_ms, dur_ms: p.dur_ms, speed: p.clip.speed() })
                })
                .collect()
        };
        let skip_ms = vedit::at(&placed, from).map(|p| from - p.start_ms).unwrap_or(0);
        let vol = seq.edit.music.volume;
        let music = match (&seq.edit.music.source, seq.edit.flags(Track::Music).off || vol <= 0.) {
            (_, true) | (MusicSource::None, _) => None,
            (MusicSource::Score, false) => seq.score.clone().map(|p| (p, from, vol * 0.8)),
            (MusicSource::File { path }, false) => Some((path.clone(), from, vol * 0.25)),
        };
        vedit_play::play(vedit_play::Play { narration, skip_ms, music, music_parts: vec![] });
        self.vedit.playing = Some((Instant::now(), from));
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(TICK).await;
            let alive = this.update(cx, |this, cx| {
                let Some((t0, from)) = this.vedit.playing else { return false };
                let now = from + t0.elapsed().as_millis() as u64;
                let total = this.vedit.seq.as_ref().map(|s| vedit::total_ms(&s.placed())).unwrap_or(0);
                if now >= total || this.view != View::Video {
                    this.video_stop();
                    this.vedit.playhead = now.min(total);
                } else {
                    this.vedit.playhead = now;
                    this.follow_playhead();
                }
                cx.notify();
                this.vedit.playing.is_some()
            });
            if !matches!(alive, Ok(true)) {
                break;
            }
        })
        .detach();
        cx.notify();
    }

    fn playhead_now(&self) -> u64 {
        match self.vedit.playing {
            Some((t0, from)) => from + t0.elapsed().as_millis() as u64,
            None => self.vedit.playhead,
        }
    }

    /// Keeps the playing playhead in view (pages the timeline like Premiere).
    fn follow_playhead(&mut self) {
        let w = f32::from(self.vedit.lanes.get().size.width);
        let x = self.vedit.playhead as f32 / 1000. * self.vedit.pps;
        if w > 0. && (x < self.vedit.scroll_x || x > self.vedit.scroll_x + w - 20.) {
            self.vedit.scroll_x = (x - 40.).max(0.);
        }
    }

    fn seek(&mut self, ms: u64, cx: &mut Context<Self>) {
        if self.vedit.whole.all.is_some() {
            return self.manga_seek(ms, cx);
        }
        let playing = self.vedit.playing.is_some();
        self.video_stop();
        let total = self.vedit.seq.as_ref().map(|s| vedit::total_ms(&s.placed())).unwrap_or(0);
        self.vedit.playhead = ms.min(total);
        if playing {
            self.video_toggle_play(cx);
        }
        cx.notify();
    }

    /// Previous / next clip edge from the playhead.
    fn step_edit(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(seq) = &self.vedit.seq else { return };
        let ph_ms = self.playhead_now();
        let mut edges: Vec<u64> = vec![0];
        edges.extend(seq.placed().iter().map(Placed::end_ms));
        let to = if forward { edges.iter().copied().find(|e| *e > ph_ms + 5) } else { edges.iter().rev().copied().find(|e| *e + 5 < ph_ms) };
        if let Some(t) = to {
            self.seek(t, cx);
        }
    }

    // ------------------------------------------------------------------------------ keys

    /// Video mode shortcuts: every key goes through the `vedit_keys` command table (the user's
    /// overrides included); `?` lists them. Esc closes the open dialog.
    pub(crate) fn video_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.vedit.text.as_ref().is_some_and(|t| t.read(cx).focus_handle(cx).is_focused(window)) {
            return false;
        }
        let k = &ev.keystroke;
        let m = &k.modifiers;
        if k.key == "escape" && !m.modified() {
            let e = &mut self.vedit;
            let open = e.export_open || e.keys_open || e.whole.confirm;
            e.export_open = false;
            e.keys_open = false;
            e.whole.confirm = false;
            cx.notify();
            return open;
        }
        let stroke = vedit_keys::stroke_of(&k.key, m.control, m.alt, m.shift, m.platform);
        let Some(command) = vedit_keys::lookup(&stroke) else { return false };
        self.video_command(command, cx);
        cx.notify();
        true
    }

    /// Runs a keyboard command on the open timeline.
    fn video_command(&mut self, c: Command, cx: &mut Context<Self>) {
        let manga = self.vedit.whole.all.is_some();
        let frame = 1000 / vedit::FPS;
        if c != Command::ShuttleBack && c != Command::ShuttleForward {
            self.vedit.shuttle_n = (0, 0);
        }
        match c {
            Command::PlayPause => self.play_any(cx),
            Command::ShuttleBack => self.video_shuttle(-1, cx),
            Command::ShuttlePause => self.video_shuttle(0, cx),
            Command::ShuttleForward => self.video_shuttle(1, cx),
            Command::FrameBack => self.seek(self.now_any().saturating_sub(frame), cx),
            Command::FrameForward => self.seek(self.now_any() + frame, cx),
            Command::Back5Frames => self.seek(self.now_any().saturating_sub(5 * frame), cx),
            Command::Forward5Frames => self.seek(self.now_any() + 5 * frame, cx),
            Command::PrevEdit if manga => self.manga_step(false, cx),
            Command::NextEdit if manga => self.manga_step(true, cx),
            Command::PrevEdit => self.step_edit(false, cx),
            Command::NextEdit => self.step_edit(true, cx),
            Command::GoStart => self.seek(0, cx),
            Command::GoEnd => self.seek(u64::MAX, cx),
            Command::MarkIn => {
                let t = self.now_any();
                self.vedit.mark_in = Some(t);
                if self.vedit.mark_out.is_some_and(|o| o <= t) {
                    self.vedit.mark_out = None;
                }
            }
            Command::MarkOut => {
                let t = self.now_any();
                self.vedit.mark_out = Some(t);
                if self.vedit.mark_in.is_some_and(|i| i >= t) {
                    self.vedit.mark_in = None;
                }
            }
            Command::AddMarker => {
                let t = self.now_any();
                match self.vedit.markers.iter().position(|m| m.abs_diff(t) < frame) {
                    Some(i) => {
                        self.vedit.markers.remove(i);
                    }
                    None => {
                        self.vedit.markers.push(t);
                        self.vedit.markers.sort_unstable();
                    }
                }
            }
            Command::Split => self.video_split_at_playhead(cx),
            Command::RippleTrimStart => self.video_ripple_trim(true, cx),
            Command::RippleTrimEnd => self.video_ripple_trim(false, cx),
            Command::Delete => self.video_delete_selection(cx),
            Command::RippleDelete => self.video_ripple_delete(cx),
            Command::Undo => self.video_undo(false, cx),
            Command::Redo => self.video_undo(true, cx),
            Command::NudgeLeft => self.video_nudge(-1, cx),
            Command::NudgeRight => self.video_nudge(1, cx),
            // One picture track: nothing to move a clip to.
            Command::NudgeUp | Command::NudgeDown => {}
            Command::VolumeDown => self.video_volume(-1., cx),
            Command::VolumeUp => self.video_volume(1., cx),
            Command::DefaultTransition => {
                if let Some(Sel::Clip(i)) = self.vedit.sel {
                    self.video_clips("Default transition", |_, c| c[i].cut = false, cx);
                }
            }
            Command::Export => {
                self.video_stop();
                self.vedit.export_open = true;
                crate::encode::warm();
            }
            Command::SelectTool => self.vedit.tool = Tool::Select,
            Command::RazorTool => self.vedit.tool = Tool::Razor,
            Command::ToggleSnap => self.vedit.snap = !self.vedit.snap,
            Command::ZoomIn => self.zoom(1.5, None),
            Command::ZoomOut => self.zoom(1. / 1.5, None),
            Command::ZoomFit => self.video_fit(),
            Command::PrevChapter if manga => self.manga_chapter_step(false, cx),
            Command::NextChapter if manga => self.manga_chapter_step(true, cx),
            Command::PrevChapter | Command::NextChapter => {}
            Command::Shortcuts => self.vedit.keys_open = !self.vedit.keys_open,
        }
    }

    /// Zooms the open timeline by `k`, keeping `anchor_x` (lane pixels) or the playhead still.
    fn zoom(&mut self, k: f32, anchor_x: Option<f32>) {
        let now = self.now_any();
        let min = if self.vedit.whole.all.is_some() { tl::MIN_PPS } else { 0.2 };
        let e = &mut self.vedit;
        let ax = anchor_x.unwrap_or(now as f32 / 1000. * e.pps - e.scroll_x);
        (e.pps, e.scroll_x) = tl::zoom_at(e.pps, e.scroll_x, ax, k, min, tl::MAX_PPS);
    }

    // ------------------------------------------------------------------------------ mouse

    /// Timeline time (ms) under window x.
    fn time_at(&self, x: Pixels) -> i64 {
        let b = self.vedit.lanes.get();
        (((f32::from(x) - f32::from(b.origin.x) + self.vedit.scroll_x) / self.vedit.pps) * 1000.) as i64
    }

    /// Snapping is on: the S toggle, inverted while Shift is held.
    fn snapping(&self) -> bool {
        tl::snap_active(self.vedit.snap, self.vedit.shift)
    }

    /// `t` snapped (when snapping) to the playhead, clip and layer edges, in / out and markers;
    /// the snap line shows where.
    fn snap_ms(&mut self, t: i64, skip_layer: Option<u64>) -> i64 {
        self.vedit.snapped = None;
        if !self.snapping() {
            return t;
        }
        if self.vedit.whole.all.is_some() {
            return self.manga_snap(t);
        }
        let pts = self.snap_targets(skip_layer);
        match tl::snap_point(t, &pts, tl::snap_reach(self.vedit.pps)) {
            Some(p) => {
                self.vedit.snapped = Some(p);
                p as i64
            }
            None => t,
        }
    }

    /// A span `dur` long starting near `start`, snapped by whichever edge is nearer a target.
    fn snap_span(&mut self, start: i64, dur: u64, skip_layer: Option<u64>) -> i64 {
        self.vedit.snapped = None;
        if !self.snapping() {
            return start;
        }
        let pts = self.snap_targets(skip_layer);
        match tl::snap_span(start, dur, &pts, tl::snap_reach(self.vedit.pps)) {
            Some((s, line)) => {
                self.vedit.snapped = Some(line);
                s
            }
            None => start,
        }
    }

    /// A value bar's 0..1 position under window x.
    fn bar_fraction(&self, name: &'static str, x: Pixels) -> Option<f32> {
        let b = *self.vedit.bars.borrow().get(name)?;
        let w = f32::from(b.size.width);
        (w > 0.).then(|| ((f32::from(x) - f32::from(b.origin.x)) / w).clamp(0., 1.))
    }

    fn drag_move(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = self.vedit.drag else { return };
        match drag {
            Drag::Playhead => {
                let t = self.time_at(pos.x).max(0);
                let t = self.snap_ms(t, None);
                self.seek(t as u64, cx);
            }
            Drag::Clip { index, grab, .. } => {
                let at = self.time_at(pos.x);
                self.vedit.drag = Some(Drag::Clip { index, grab, at });
            }
            Drag::Trim { index, head, from, applied } => {
                let t = self.time_at(pos.x);
                let t = if head { t } else { self.snap_ms(t, None) };
                let want = if head { t - from } else { from - t };
                let delta = want - applied;
                if delta != 0 {
                    if let Some(seq) = self.vedit.seq.as_mut() {
                        let mut clips = seq.edit.clips(&seq.beats);
                        let before = clips[index.min(clips.len().saturating_sub(1))].clone();
                        vedit::trim(&seq.beats, &mut clips, index, head, delta);
                        let after = clips.get(index).cloned();
                        let moved = after.map(|a| if head { a.trim_in_ms as i64 - before.trim_in_ms as i64 } else { a.trim_out_ms as i64 - before.trim_out_ms as i64 }).unwrap_or(0);
                        seq.edit.set_clips(&seq.beats, clips);
                        self.vedit.drag = Some(Drag::Trim { index, head, from, applied: applied + moved });
                    }
                }
            }
            Drag::Layer { id, grab } => {
                let t = self.time_at(pos.x) - grab;
                let dur = self.vedit.seq.as_ref().and_then(|s| s.edit.layers.iter().find(|l| l.id == id)).map(|l| l.end_ms - l.start_ms).unwrap_or(0);
                let t = self.snap_span(t, dur, Some(id)).max(0) as u64;
                if let Some(seq) = self.vedit.seq.as_mut() {
                    let total = vedit::total_ms(&seq.placed());
                    if let Some(l) = seq.edit.layers.iter_mut().find(|l| l.id == id) {
                        let d = l.end_ms - l.start_ms;
                        l.start_ms = t.min(total.saturating_sub(d));
                        l.end_ms = l.start_ms + d;
                    }
                }
            }
            Drag::LayerEdge { id, head } => {
                let t = self.snap_ms(self.time_at(pos.x), Some(id)).max(0) as u64;
                if let Some(seq) = self.vedit.seq.as_mut() {
                    let total = vedit::total_ms(&seq.placed());
                    if let Some(l) = seq.edit.layers.iter_mut().find(|l| l.id == id) {
                        if head {
                            l.start_ms = t.min(l.end_ms.saturating_sub(vedit::MIN_MS));
                        } else {
                            l.end_ms = t.clamp(l.start_ms + vedit::MIN_MS, total.max(l.start_ms + vedit::MIN_MS));
                        }
                    }
                }
            }
            Drag::Place { id } => {
                let b = self.vedit.monitor.get();
                let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
                if w > 0. && h > 0. {
                    let x = ((f32::from(pos.x) - f32::from(b.origin.x)) / w * 100.).clamp(0., 100.);
                    let y = ((f32::from(pos.y) - f32::from(b.origin.y)) / h * 100.).clamp(0., 100.);
                    let ((x, y), _) = video::snap_point(x, y);
                    if let Some(l) = self.vedit.seq.as_mut().and_then(|s| s.edit.layers.iter_mut().find(|l| l.id == id)) {
                        l.x = x;
                        l.y = y;
                    }
                }
            }
            Drag::Caption { grab, .. } => {
                let b = self.vedit.monitor.get();
                let (fx, fy) = (f32::from(pos.x - b.origin.x), f32::from(pos.y - b.origin.y));
                if let Some((p, lit)) = vedit::caption_drop(fx, fy, f32::from(b.size.width), f32::from(b.size.height), grab) {
                    self.vedit.cap_lit = lit;
                    if let Some(seq) = self.vedit.seq.as_mut() {
                        seq.subs.pos = Some(p);
                    }
                }
            }
            Drag::Bar(name) => {
                if let Some(f) = self.bar_fraction(name, pos.x) {
                    self.bar_set(name, f, cx);
                }
            }
            Drag::Nav { grip, x0, pps0, scroll0 } => {
                let bar_w = f32::from(self.vedit.nav.get().size.width);
                let view_w = f32::from(self.vedit.lanes.get().size.width);
                let total_s = self.timeline_ms() as f32 / 1000.;
                let min = if self.vedit.whole.all.is_some() { tl::MIN_PPS } else { 0.2 };
                if bar_w > 0. && view_w > 0. {
                    (self.vedit.pps, self.vedit.scroll_x) = tl::nav_drag(grip, f32::from(pos.x) - x0, bar_w, total_s, view_w, pps0, scroll0, min);
                }
            }
        }
        cx.notify();
    }

    /// The drag ends: drops a reordered clip and keeps one undo step for the whole drag.
    fn drag_end(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.vedit.drag.take() else { return };
        self.vedit.snapped = None;
        self.vedit.shift = false;
        match drag {
            Drag::Clip { index, grab, at } => {
                let Some(seq) = &self.vedit.seq else { return };
                let placed = seq.placed();
                let Some(me) = placed.iter().find(|p| p.index == index) else { return };
                if (at - grab - me.start_ms as i64).abs() < 150 {
                    return;
                }
                // Where it lands: before the first other clip whose middle is after its middle.
                let mid = at - grab + me.dur_ms as i64 / 2;
                let others: Vec<&Placed> = placed.iter().filter(|p| p.index != index).collect();
                let slot = others.iter().position(|p| (p.start_ms + p.dur_ms / 2) as i64 > mid).unwrap_or(others.len());
                self.video_clips("Move clip", |_, c| vedit::reorder(c, index, slot), cx);
                self.vedit.sel = Some(Sel::Clip(slot));
            }
            // The drag edited in place: one undo step for the whole gesture.
            Drag::Trim { .. } => self.video_commit("Trim clip", cx),
            Drag::Layer { .. } => self.video_commit("Move layer", cx),
            Drag::LayerEdge { .. } => self.video_commit("Trim layer", cx),
            Drag::Place { .. } => self.video_commit("Place layer", cx),
            Drag::Caption { from, .. } => {
                self.vedit.cap_lit = None;
                // A click (no move) only selects the clip.
                if self.vedit.seq.as_ref().is_some_and(|s| s.subs.pos != from) {
                    self.video_caption_pos_save(cx);
                }
            }
            Drag::Bar(name) => {
                let Some(seq) = &self.vedit.seq else { return };
                video::save_chapter_setting(&seq.item.output_dir, Some(&seq.subs));
                self.video_commit(
                    match name {
                        "music" => "Music volume",
                        "opacity" => "Opacity",
                        "scale" => "Scale",
                        "x" | "y" => "Position",
                        _ => "Change",
                    },
                    cx,
                );
                if name == "caption-y" {
                    self.video_caption_pos_save(cx);
                }
                self.manga_sync(cx);
            }
            Drag::Playhead | Drag::Nav { .. } => {}
        }
        cx.notify();
    }

    /// Sets the value a bar controls (`f` is 0..1); saved when the drag ends.
    fn bar_set(&mut self, name: &'static str, f: f32, _cx: &mut Context<Self>) {
        let sel = self.vedit.sel;
        let Some(seq) = self.vedit.seq.as_mut() else { return };
        let layer = match sel {
            Some(Sel::Layer(id)) => seq.edit.layers.iter_mut().find(|l| l.id == id),
            _ => None,
        };
        match (name, layer) {
            ("opacity", Some(l)) => l.opacity = (f * 100.).round() / 100.,
            ("scale", Some(l)) => {
                l.scale = match l.kind {
                    LayerKind::Image { .. } => (0.02 + f * 0.98 * 100.).round() / 100.,
                    LayerKind::Text { .. } => ((0.01 + f * 0.19) * 1000.).round() / 1000.,
                }
            }
            ("x", Some(l)) => l.x = (f * 1000.).round() / 10.,
            ("y", Some(l)) => l.y = (f * 1000.).round() / 10.,
            ("music", _) => seq.edit.music.volume = (f * 2. * 20.).round() / 20.,
            ("caption-y", _) => seq.subs.pos = Some((seq.subs.pos.map(|p| p.0).unwrap_or(50.), (f * 1000.).round() / 10.)),
            ("caption-size", _) => seq.subs.size = (0.4 + f * 2.1).clamp(0.4, 2.5),
            _ => {}
        }
    }

    // ------------------------------------------------------------------------------ render

    /// The whole Video mode (below Studio's toolbar).
    pub(crate) fn video_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        video::ensure_fonts(cx);
        self.video_sync();
        self.ensure_text_input(window, cx);
        let Some(g) = self.video_manga() else {
            return self.video_empty("Pick a manga first", "Open a manga on the Dashboard, then come back to edit its recap video here.", cx);
        };
        if self.vedit.seq.is_none() {
            return self.video_empty(
                "No finished chapters yet",
                &format!("{} has no chapter video yet. Make one with Generate on the Dashboard; finished chapters open here as sequences.", g.title),
                cx,
            );
        }
        self.sync_text_input(window, cx);
        self.video_favs_refresh();
        if self.vedit.fit_pending {
            if f32::from(self.vedit.lanes.get().size.width) > 0. {
                self.vedit.fit_pending = false;
                self.video_fit();
            } else {
                cx.notify();
            }
        }
        let pending = self.unrendered(&g);
        let all = self.vedit.whole.all.is_some();
        let top = div()
            .flex()
            .flex_1()
            .min_h(px(260.))
            .gap(px(4.))
            .child(self.panel("Effect Controls", None, self.effect_controls(window, cx)).w(px(LEFT_W)).flex_none())
            .child(if all { self.manga_monitor_panel(&g, window, cx) } else { self.monitor_panel(window, cx) })
            .child(self.panel(if all { "Sequence · every chapter" } else { "Sequence" }, None, self.sequence_panel(window, cx)).w(px(RIGHT_W)).flex_none());
        let bottom = div()
            .flex()
            .flex_none()
            .h(px(BOTTOM_H))
            .gap(px(4.))
            .child(self.panel(&format!("Project: {}", g.title), None, self.bin(&g, &pending, cx)).w(px(LEFT_W)).flex_none())
            .child(self.tools(cx))
            .child(if all { self.manga_timeline(&g, cx) } else { self.video_timeline(cx) });
        div()
            .id("vedit")
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(px(4.))
            .p(px(4.))
            .bg(Tokens::canvas())
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if this.vedit.drag.is_some() {
                    if ev.pressed_button == Some(MouseButton::Left) {
                        this.vedit.shift = ev.modifiers.shift;
                        this.drag_move(ev.position, cx);
                    } else {
                        this.drag_end(cx);
                    }
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.drag_end(cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.drag_end(cx)))
            .child(self.workspace_bar(&g, &pending, cx))
            .child(top)
            .child(bottom)
            .when(self.vedit.export_open, |d| d.child(self.export_dialog(&g, pending, cx)))
            .when(self.vedit.whole.confirm, |d| d.child(self.reedit_dialog(&g, cx)))
            .when(self.vedit.keys_open, |d| d.child(self.shortcuts_dialog(cx)))
            .into_any_element()
    }

    fn video_empty(&self, title: &str, body: &str, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(space::MD))
                    .max_w(px(480.))
                    .child(empty_view(ph::FILM_SLATE, title.to_string(), body.to_string()))
                    .child(btn("vedit-to-dash", "Go to the Dashboard", ButtonKind::Secondary).icon(ph::SQUARES_FOUR).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.view = View::Dashboard;
                        cx.notify();
                    }))),
            )
            .into_any_element()
    }

    /// A Premiere panel: a tab-strip header and the body.
    fn panel(&self, title: &str, extra: Option<AnyElement>, body: impl IntoElement) -> Div {
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .rounded(px(radius::MD))
            .bg(Tokens::surface())
            .border_1()
            .border_color(Tokens::border())
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(space::SM))
                    .h(px(30.))
                    .px(px(space::MD))
                    .border_b_1()
                    .border_color(Tokens::border())
                    .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).truncate().child(title.to_string()))
                    .child(div().flex_1())
                    .children(extra),
            )
            .child(body)
    }

    /// Premiere's Import / Edit / Export header, with the edit's state and Render.
    fn workspace_bar(&self, g: &MangaGroup, pending: &[ChapterItem], cx: &mut Context<Self>) -> Div {
        let seq = self.vedit.seq.as_ref();
        let item = seq.map(|s| s.item.clone());
        let queued = item.as_ref().and_then(|c| self.queue.for_chapter(&c.id)).filter(|q| q.state.active()).map(|q| q.progress());
        let this_pending = item.as_ref().is_some_and(|c| pending.iter().any(|p| p.id == c.id));
        let status = match (queued, this_pending) {
            (Some(p), _) => format!("Rendering · {}%", (p * 100.).round()),
            (None, true) => "Edited · not rendered yet".to_string(),
            (None, false) => "Rendered".to_string(),
        };
        let title = item.as_ref().map(|c| format!("{} · {}", g.title, c.label())).unwrap_or_default();
        let ws = |id: &'static str, on: bool| {
            div()
                .id(id)
                .px(px(space::MD))
                .h(px(28.))
                .flex()
                .items_center()
                .text_size(px(size::MD))
                .cursor_pointer()
                .text_color(if on { Tokens::text_1() } else { Tokens::text_2() })
                .when(on, |d| d.font_weight(FontWeight::SEMIBOLD).border_b_2().border_color(Tokens::accent()))
                .hover(|s| s.text_color(Tokens::text_1()))
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(space::XS))
            .h(px(38.))
            .px(px(space::SM))
            .rounded(px(radius::MD))
            .bg(Tokens::surface())
            .border_1()
            .border_color(Tokens::border())
            .child(ws("vedit-ws-import", false).child("Import").tooltip(tip("Add an image layer from a file")).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.import_image(cx))))
            .child(ws("vedit-ws-edit", !self.vedit.export_open).child("Edit").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.vedit.export_open = false;
                cx.notify();
            })))
            .child(ws("vedit-ws-export", self.vedit.export_open).child("Export").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.video_command(Command::Export, cx);
                cx.notify();
            })))
            .child(div().flex_1().flex().justify_center().min_w_0().child(div().truncate().text_size(px(size::MD)).text_color(Tokens::text_1()).child(title)))
            .child(div().flex_none().text_size(px(size::SM)).text_color(if this_pending { Tokens::warning() } else { Tokens::text_3() }).child(status))
            .child({
                let undo = self.vedit.hist.undo_label().map(|l| format!("Undo {l} (Ctrl+Z)"));
                icon_button("vedit-undo", ph::ARROW_COUNTER_CLOCKWISE, undo.clone().unwrap_or_else(|| "Undo (Ctrl+Z)".into())).small().when(undo.is_none(), |b| b.disabled("Nothing to undo")).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.video_undo(false, cx)))
            })
            .child({
                let redo = self.vedit.hist.redo_label().map(|l| format!("Redo {l} (Ctrl+Shift+Z)"));
                icon_button("vedit-redo", ph::ARROW_CLOCKWISE, redo.clone().unwrap_or_else(|| "Redo (Ctrl+Shift+Z)".into())).small().when(redo.is_none(), |b| b.disabled("Nothing to redo")).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.video_undo(true, cx)))
            })
            .child(icon_button("vedit-keys", ph::KEYBOARD, "Keyboard shortcuts (?)").small().selected(self.vedit.keys_open).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.vedit.keys_open = !this.vedit.keys_open;
                cx.notify();
            })))
            .child(
                btn("vedit-reset", "Reset edit", ButtonKind::Ghost)
                    .small()
                    .tooltip("Back to the chapter as Generate made it (the caption style stays; Undo brings the edit back)")
                    .when(seq.is_none_or(|s| ChapterEdit { rendered: None, ..s.edit.clone() } == ChapterEdit::default()), |b| b.disabled("Nothing edited"))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.vedit.sel = None;
                        this.video_edit("Reset edit", |s| s.edit = ChapterEdit { rendered: s.edit.rendered, ..ChapterEdit::default() }, cx)
                    })),
            )
            .when_some(item, |d, item| {
                let busy = queued.is_some();
                d.child(
                    btn("vedit-render", "Render chapter", ButtonKind::Primary)
                        .small()
                        .icon(ph::FILM_STRIP)
                        .tooltip("Make this chapter's video with the edit (the full recap export uses it)")
                        .when(busy, |b| b.disabled("Rendering"))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.render_chapters(vec![item.clone()], cx))),
                )
            })
    }

    /// What a render of chapter `c` would make now (the open chapter live, others once).
    fn render_key_of(&self, c: &ChapterItem) -> u64 {
        if let Some(s) = self.vedit.seq.as_ref().filter(|s| s.item.id == c.id) {
            return vedit::render_key(&vedit::render_extra(&s.beats, &s.edit, &s.subs));
        }
        *self.vedit.keys.borrow_mut().entry(c.id.clone()).or_insert_with(|| vedit::extra_for(&c.output_dir, &c.id, &c.pages_dir, &video::subtitle_setting_for(c)).map(|x| vedit::render_key(&x)).unwrap_or(0))
    }

    /// Made chapters whose edit is newer than their video.
    fn unrendered(&self, g: &MangaGroup) -> Vec<ChapterItem> {
        let open = self.vedit.seq.as_ref();
        self.made_chapters(g)
            .into_iter()
            .filter(|c| {
                let edit = match open.filter(|s| s.item.id == c.id) {
                    Some(s) => Some(s.edit.clone()),
                    None => self.vedit.project.chapters.get(&c.id).cloned(),
                };
                let Some(e) = edit else { return false };
                // Never edited (a reset after a render still needs one).
                if e.rendered.is_none() && (ChapterEdit { rendered: None, ..e.clone() }) == ChapterEdit::default() {
                    return false;
                }
                e.rendered != Some(self.render_key_of(c))
            })
            .collect()
    }

    /// Re-makes the chapters' videos with their edits (Video step only, in the pipeline queue).
    fn render_chapters(&mut self, chapters: Vec<ChapterItem>, cx: &mut Context<Self>) {
        if chapters.is_empty() {
            return;
        }
        self.video_stop();
        self.sync_pipeline(cx);
        let n = match crate::vedit_tools::render(&chapters) {
            Ok((n, project)) => {
                if let Some(s) = self.vedit.seq.as_mut() {
                    s.edit.rendered = project.chapters.get(&s.item.id).and_then(|e| e.rendered);
                }
                self.vedit.project = project;
                n
            }
            Err(e) => {
                toast::show(cx, Toast::error(e));
                return;
            }
        };
        toast::show(cx, Toast::success(format!("Rendering {n} chapter video{} with the edit", plural(n))).icon(ph::FILM_STRIP));
        cx.notify();
    }

    // ------------------------------------------------------------------------------ monitor

    fn monitor_panel(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some(seq) = &self.vedit.seq else { return div() };
        let placed = seq.placed();
        let total = vedit::total_ms(&placed);
        let now = self.playhead_now().min(total);
        let playing = self.vedit.playing.is_some();
        let video = seq.item.video.clone().or_else(|| self.statuses.get(&seq.item.id).and_then(|s| s.video.clone()));
        let title = format!("{} · Rendered", seq.item.label());
        let play_rendered = btn("vedit-play-rendered", "Play rendered", ButtonKind::Ghost)
            .small()
            .icon(ph::MONITOR_PLAY)
            .tooltip("Play the chapter's rendered video in the player")
            .when(video.is_none(), |b| b.disabled("No rendered video"))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.video_stop();
                if let Some(v) = &video {
                    crate::phases::play_video(&title, v, window, cx);
                }
            }));
        let viewport = window.viewport_size();
        let (vw, vh) = (f32::from(viewport.width), f32::from(viewport.height));
        let room_w = (vw - LEFT_W - RIGHT_W - 60.).max(240.);
        let room_h = (vh - 52. - 38. - BOTTOM_H - 30. - 96. - 40.).max(140.);
        let aspect = if seq.reels { 9. / 16. } else { 16. / 9. };
        let (fw, fh) = if room_w / room_h > aspect { (room_h * aspect, room_h) } else { (room_w, room_w / aspect) };
        let frame = self.frame(seq, &placed, now, fw, fh, window, cx);
        let transport = |id: &'static str, icon, label: &'static str| icon_button(id, icon, label).small();
        let scrub_frac = if total > 0 { now as f32 / total as f32 } else { 0. };
        let bars = self.vedit.bars.clone();
        let scrub = div()
            .id("vedit-scrub")
            .relative()
            .h(px(14.))
            .mx(px(space::MD))
            .cursor_pointer()
            .child(div().absolute().left_0().right_0().top(px(6.)).h(px(2.)).bg(Tokens::border_strong()))
            .child(div().absolute().left(relative(scrub_frac)).top_0().w(px(2.)).h_full().bg(Tokens::accent()))
            .child(canvas(move |b, _, _| { bars.borrow_mut().insert("scrub", b); }, |_, _, _, _| {}).absolute().inset_0())
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                if let Some(f) = this.bar_fraction("scrub", ev.position.x) {
                    this.seek((f * total as f32) as u64, cx);
                }
            }));
        let body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(div().flex().flex_1().min_h_0().items_center().justify_center().p(px(space::SM)).bg(Tokens::well()).child(frame))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(px(30.))
                    .px(px(space::MD))
                    .child(div().text_size(px(size::MD)).font_family(crate::page::MONO).text_color(Tokens::accent()).child(vedit::timecode(now)))
                    .child(div().flex_1())
                    .child(muted(if seq.reels { "9:16 · Fit" } else { "16:9 · Fit" }))
                    .child(div().flex_1())
                    .child(div().text_size(px(size::MD)).font_family(crate::page::MONO).text_color(Tokens::text_2()).child(vedit::timecode(total))),
            )
            .child(scrub)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .gap(px(space::XS))
                    .h(px(40.))
                    .child(transport("vedit-start", ph::SKIP_BACK, "Go to start (Home)").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.seek(0, cx))))
                    .child(transport("vedit-prev", ph::CARET_DOUBLE_LEFT, "Previous edit (Up)").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.step_edit(false, cx))))
                    .child(transport("vedit-back", ph::CARET_LEFT, "Back one frame (Left)").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        let t = this.playhead_now().saturating_sub(1000 / vedit::FPS);
                        this.seek(t, cx)
                    })))
                    .child(icon_button("vedit-play", if playing { ph::PAUSE } else { ph::PLAY }, if playing { "Pause (Space)" } else { "Play (Space)" }).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.video_toggle_play(cx))))
                    .child(transport("vedit-fwd", ph::CARET_RIGHT, "Forward one frame (Right)").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        let t = this.playhead_now() + 1000 / vedit::FPS;
                        this.seek(t, cx)
                    })))
                    .child(transport("vedit-next", ph::CARET_DOUBLE_RIGHT, "Next edit (Down)").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.step_edit(true, cx))))
                    .child(transport("vedit-end", ph::SKIP_FORWARD, "Go to end (End)").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.seek(u64::MAX, cx))))
                    .child(div().w(px(space::LG)))
                    .child(transport("vedit-cut", ph::SCISSORS, "Cut the clip at the playhead (Ctrl+K)").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.video_split_at_playhead(cx))))
                    .child(transport("vedit-add-text", ph::TEXT_T, "Add a text layer at the playhead").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.add_layer(LayerKind::Text { text: "Your text".into(), color: "#ffffff".into(), boxed: false }, cx)
                    }))),
            );
        self.panel(&format!("Program: {}", seq.item.label()), Some(play_rendered.into_any_element()), body).flex_1()
    }

    /// The program frame at `now`: the clip's panel, the layers and the caption cue.
    fn frame(&self, seq: &Seq, placed: &[Placed], now: u64, fw: f32, fh: f32, window: &Window, cx: &Context<Self>) -> Stateful<Div> {
        let cur = vedit::at(placed, now).or(placed.last());
        // The panel as the chapter's look draws it (blurred fill, duo panels, finish).
        let mut d = div().id("vedit-frame").relative().flex_none().w(px(fw)).h(px(fh)).overflow_hidden().bg(video::paint(hex("#000000"), 1.)).children(self.look_backdrop(seq, placed, now, fw, fh));
        if !seq.edit.flags(Track::Layers).off {
            for l in seq.edit.layers.iter().filter(|l| l.start_ms <= now && now < l.end_ms) {
                let selected = self.vedit.sel == Some(Sel::Layer(l.id));
                let id = l.id;
                let el = match &l.kind {
                    LayerKind::Image { path } => {
                        let w = fw * l.scale;
                        div().w(px(w)).h(px(w)).child(img(path.clone()).size_full().object_fit(ObjectFit::Contain))
                    }
                    LayerKind::Text { text, color, boxed } => {
                        let fs = (fh * l.scale).max(6.);
                        let c = video::parse_color(color).unwrap_or(hex("#ffffff"));
                        div()
                            .px(px(fs * 0.25))
                            .when(*boxed, |d| d.bg(video::paint(hex("#000000"), 0.55 * l.opacity)))
                            .text_size(px(fs))
                            .font_family("Inter")
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .whitespace_nowrap()
                            .text_color(video::paint(c, 1.))
                            .child(text.clone())
                    }
                };
                // Centred on (x, y): a full-frame flex box puts it there.
                let (x, y) = (fw * l.x / 100., fh * l.y / 100.);
                d = d.child(
                    div()
                        .absolute()
                        .left(px(x - fw))
                        .top(px(y - fh))
                        .w(px(fw * 2.))
                        .h(px(fh * 2.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .id(("vedit-layer-on-frame", id as usize))
                                .opacity(l.opacity)
                                .when(selected, |d| d.border_1().border_color(Tokens::accent()))
                                .cursor(CursorStyle::OpenHand)
                                .child(el)
                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.vedit.sel = Some(Sel::Layer(id));
                                    if !this.locked(Track::Layers) {
                                        this.vedit.drag = Some(Drag::Place { id });
                                    }
                                    cx.notify();
                                })),
                        ),
                );
            }
        }
        if !seq.edit.flags(Track::Captions).off && seq.subs.enabled {
            if let Some(p) = cur {
                let text = p.clip.text.clone().or_else(|| seq.beats.get(p.clip.src).map(|b| b.text.clone())).unwrap_or_default();
                let st = seq.subs.effective_style();
                let per = video::words_per_cue(&st, seq.subs.user_words());
                let frac = (now.saturating_sub(p.start_ms)) as f32 / p.dur_ms.max(1) as f32;
                let (words, active) = vedit::cue_at(&text, per, frac);
                if !words.is_empty() {
                    d = d.child(video::caption_layer(&words, &st, seq.subs.user_words(), fw, fh, active, true, self.vedit.playing.is_some(), window));
                    // Grab the caption to move every caption of the manga (snaps to the 3x3 grid);
                    // a click also selects its clip, to edit the text.
                    if let Some((x, y, w, h)) = video::caption_box(&words, &st, seq.subs.user_words(), fw, fh, window) {
                        let dragging = matches!(self.vedit.drag, Some(Drag::Caption { .. }));
                        let on = dragging || self.vedit.sel == Some(Sel::Clip(p.index));
                        let (index, centre) = (p.index, st.pos);
                        d = d.child(
                            div()
                                .id("vedit-caption-on-frame")
                                .absolute()
                                .left(px(x))
                                .top(px(y))
                                .w(px(w))
                                .h(px(h))
                                .rounded(px(radius::SM))
                                .border_1()
                                .border_color(if on { Tokens::accent() } else { soft(Tokens::accent(), 0.) })
                                .hover(|d| d.border_color(Tokens::accent()))
                                .cursor(if dragging { CursorStyle::ClosedHand } else { CursorStyle::OpenHand })
                                .tooltip(tip("Drag to move every caption (snaps to the grid). Click to edit the text."))
                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.vedit.sel = Some(Sel::Clip(index));
                                    if !this.locked(Track::Captions) {
                                        let b = this.vedit.monitor.get();
                                        let at = vedit::frame_percent(f32::from(ev.position.x - b.origin.x), f32::from(ev.position.y - b.origin.y), f32::from(b.size.width), f32::from(b.size.height));
                                        let grab = at.map(|(x, y)| (x - centre.0, y - centre.1)).unwrap_or((0., 0.));
                                        let from = this.vedit.seq.as_ref().and_then(|s| s.subs.pos);
                                        this.vedit.drag = Some(Drag::Caption { grab, from });
                                    }
                                    cx.notify();
                                })),
                        );
                    }
                }
            }
        }
        if matches!(self.vedit.drag, Some(Drag::Caption { .. })) {
            d = d.child(video::grid_overlay(fw, fh, self.vedit.cap_lit));
        }
        let mon = self.vedit.monitor.clone();
        d.child(canvas(move |b, _, _| mon.set(b), |_, _, _, _| {}).absolute().inset_0())
    }

    // ------------------------------------------------------------------------------ panels

    /// An Effect Controls section (bhippi's Properties panel): a header that folds it shut and,
    /// when the section has one, a reset button; then its controls.
    fn section(&self, key: &'static str, icon: mv_ui::Ph, title: &str, reset: Option<Box<dyn Fn(&mut Self, &mut Context<Self>)>>, body: impl IntoElement, cx: &Context<Self>) -> Div {
        let open = !self.vedit.collapsed.contains(key);
        let head = div()
            .id(SharedString::from(format!("vedit-sec-{key}")))
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(26.))
            .px(px(4.))
            .rounded(px(radius::SM))
            .cursor_pointer()
            .hover(|d| d.bg(Tokens::hover()))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if !this.vedit.collapsed.remove(key) {
                    this.vedit.collapsed.insert(key);
                }
                cx.notify();
            }))
            .child(icon_ph(if open { ph::CARET_DOWN } else { ph::CARET_RIGHT }, Tokens::text_3(), 10.))
            .child(icon_ph(icon, Tokens::text_2(), icon_px::XS))
            .child(div().flex_1().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title.to_string()))
            .when_some(reset, |d, f| {
                d.child(icon_button(SharedString::from(format!("vedit-reset-{key}")), ph::ARROW_COUNTER_CLOCKWISE, format!("Reset {}", title.to_lowercase())).small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    f(this, cx);
                })))
            });
        div()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(Tokens::border())
            .pb(px(if open { 8. } else { 2. }))
            .child(head)
            .when(open, |d| d.child(div().flex().flex_col().gap(px(8.)).pt(px(4.)).px(px(4.)).child(body)))
    }

    /// Top left: what the selection can change, in folding sections with resets.
    fn effect_controls(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(seq) = &self.vedit.seq else { return div().into_any_element() };
        let body = div().id("vedit-fx").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().gap(px(4.)).p(px(space::SM));
        let body = match self.vedit.sel {
            None => body.child(muted("Select a clip or layer in the timeline to change it.")).child(self.add_buttons(cx)),
            Some(Sel::Clip(i)) => {
                let clips = seq.edit.clips(&seq.beats);
                let Some(c) = clips.get(i).cloned() else { return div().into_any_element() };
                let b = seq.beats.get(c.src);
                let placed = seq.placed();
                let p = placed.iter().find(|p| p.index == i);
                let locked = seq.edit.flags(Track::Clips).locked;
                let basic = segmented("vedit-motion", Motion::BASIC.iter().map(|m| (None, m.label())), Motion::BASIC.iter().position(|m| *m == c.motion).unwrap_or(0)).small().on_change(cx.listener(move |this, ix: &usize, _, cx| {
                    let m = Motion::BASIC[(*ix).min(Motion::BASIC.len() - 1)];
                    this.video_clips("Clip motion", |_, cl| cl[i].motion = m, cx);
                }));
                // Moves the assistant set (pans, Ken Burns, speed, transitions) are listed under it.
                let extra = c.amount.is_some() || c.focus_from.is_some() || c.focus_to.is_some() || c.speed() != 1. || c.trans.is_some() || !Motion::BASIC.contains(&c.motion);
                let motion = div().flex().flex_col().gap(px(4.)).child(basic).when(extra, |d| d.child(muted(c.summary())));
                let trims = |head: bool| {
                    let label = if head { "In" } else { "Out" };
                    let v = if head { c.trim_in_ms } else { c.trim_out_ms };
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::XS))
                        .child(div().w(px(36.)).text_size(px(size::SM)).text_color(Tokens::text_2()).child(label))
                        .child(icon_button(if head { "vedit-in-less" } else { "vedit-out-less" }, ph::MINUS, "0.1 s less trimmed").small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.video_clips("Trim clip", |b, cl| vedit::trim(b, cl, i, head, -100), cx))))
                        .child(div().w(px(64.)).text_size(px(size::SM)).font_family(crate::page::MONO).text_color(Tokens::text_1()).child(format!("{:.1} s", v as f32 / 1000.)))
                        .child(icon_button(if head { "vedit-in-more" } else { "vedit-out-more" }, ph::PLUS, "Trim 0.1 s more").small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.video_clips("Trim clip", |b, cl| vedit::trim(b, cl, i, head, 100), cx))))
                };
                let look = match &seq.edit.look {
                    Some(l) => Preset::parse(&l.preset).unwrap_or_default().label().to_string(),
                    None => format!("{} (Studio look)", crate::look::Look::load().preset.label()),
                };
                let narr_off = seq.edit.flags(Track::Narration).off;
                let vol = seq.edit.music.volume;
                body.child(self.prop_head(ph::IMAGE, &format!("Clip {} · page {}", i + 1, b.map(|b| b.page + 1).unwrap_or(0)), p.map(|p| format!("{} – {}", vedit::timecode(p.start_ms), vedit::timecode(p.end_ms()))).unwrap_or_default()))
                    .when(locked, |d| d.child(muted("The Panels track is locked.")))
                    .child(self.section(
                        "motion",
                        ph::ARROWS_OUT,
                        "Motion",
                        (c.motion != Motion::Auto || extra).then(|| -> Box<dyn Fn(&mut Self, &mut Context<Self>)> {
                            Box::new(move |this: &mut Self, cx: &mut Context<Self>| {
                                this.video_clips(
                                    "Reset motion",
                                    |_, cl| {
                                        cl[i].reset_motion();
                                        cl[i].speed = 1.;
                                        cl[i].trans = None;
                                    },
                                    cx,
                                )
                            })
                        }),
                        motion,
                        cx,
                    ))
                    .child(self.section(
                        "timing",
                        ph::TIMER,
                        "Timing",
                        (c.trim_in_ms + c.trim_out_ms > 0).then(|| -> Box<dyn Fn(&mut Self, &mut Context<Self>)> {
                            Box::new(move |this: &mut Self, cx: &mut Context<Self>| {
                                this.video_clips(
                                    "Reset trim",
                                    |_, cl| {
                                        cl[i].trim_in_ms = 0;
                                        cl[i].trim_out_ms = 0;
                                    },
                                    cx,
                                )
                            })
                        }),
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(trims(true))
                            .child(trims(false))
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap(px(space::SM))
                                    .child(btn("vedit-split", "Cut at playhead", ButtonKind::Secondary).small().icon(ph::SCISSORS).tooltip("Ctrl+K").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.video_split_at_playhead(cx))))
                                    .child(btn("vedit-del-clip", "Delete", ButtonKind::DangerGhost).small().icon(ph::TRASH).tooltip("Delete · Shift+Delete moves the layers after it up too").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.video_delete_selection(cx)))),
                            ),
                        cx,
                    ))
                    .child(self.section(
                        "effects",
                        ph::MAGIC_WAND,
                        "Effects",
                        (c.cut || c.trans.is_some()).then(|| -> Box<dyn Fn(&mut Self, &mut Context<Self>)> {
                            Box::new(move |this: &mut Self, cx: &mut Context<Self>| {
                                this.video_clips(
                                    "Reset transition",
                                    |_, cl| {
                                        cl[i].cut = false;
                                        cl[i].trans = None;
                                    },
                                    cx,
                                )
                            })
                        }),
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(switch("vedit-cut", c.cut).label("Hard cut in (no transition)").on_click(cx.listener(move |this, on: &bool, _, cx| {
                                let on = *on;
                                this.video_clips("Transition", |_, cl| cl[i].cut = on, cx)
                            })))
                            .child(muted(format!("Look: {look} · Sequence → Look changes it"))),
                        cx,
                    ))
                    .child(self.section(
                        "captions",
                        ph::CLOSED_CAPTIONING,
                        "Captions",
                        c.text.is_some().then(|| -> Box<dyn Fn(&mut Self, &mut Context<Self>)> {
                            Box::new(move |this: &mut Self, cx: &mut Context<Self>| {
                                this.video_clips("Reset caption", |_, cl| cl[i].text = None, cx);
                                this.vedit.text_for = None;
                            })
                        }),
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(self.text_field())
                            .child(muted("The voice keeps the narration; only the caption changes."))
                            .child(self.caption_preview(seq, &c, window))
                            .child(div().id("vedit-clip-styles").flex().gap(px(6.)).pb(px(4.)).overflow_x_scroll().children(self.style_cards("vedit-clip-style", 96., 54., window, cx))),
                        cx,
                    ))
                    .child(self.section(
                        "audio",
                        ph::SPEAKER_HIGH,
                        "Audio",
                        (narr_off || (vol - 1.).abs() > 0.001).then(|| -> Box<dyn Fn(&mut Self, &mut Context<Self>)> {
                            Box::new(|this: &mut Self, cx: &mut Context<Self>| {
                                this.video_edit(
                                    "Reset audio",
                                    |s| {
                                        s.edit.flags_mut(Track::Narration).off = false;
                                        s.edit.music.volume = 1.;
                                    },
                                    cx,
                                )
                            })
                        }),
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(switch("vedit-clip-narr", !narr_off).label("Narration").on_click(cx.listener(|this, on: &bool, _, cx| {
                                let on = *on;
                                this.video_edit("Mute narration", |s| s.edit.flags_mut(Track::Narration).off = !on, cx)
                            })))
                            .child(self.bar("music", "Music volume ([ / ])", vol / 2., format!("{:.0}%", vol * 100.), cx)),
                        cx,
                    ))
            }
            Some(Sel::Layer(id)) => {
                let Some(l) = seq.edit.layers.iter().find(|l| l.id == id).cloned() else { return div().into_any_element() };
                let is_text = matches!(l.kind, LayerKind::Text { .. });
                let (scale_f, scale_label) = match l.kind {
                    LayerKind::Image { .. } => ((l.scale - 0.02) / 0.98, format!("{}% of the width", (l.scale * 100.).round())),
                    LayerKind::Text { .. } => ((l.scale - 0.01) / 0.19, format!("{}% of the height", (l.scale * 1000.).round() / 10.)),
                };
                let (dx, dy, ds) = if is_text { (50., 22., 0.06) } else { (50., 50., 0.3) };
                let set_time = |head: bool| {
                    let label = if head { "Start at playhead" } else { "End at playhead" };
                    btn(if head { "vedit-l-start" } else { "vedit-l-end" }, label, ButtonKind::Secondary).small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let t = this.playhead_now();
                        this.video_edit(
                            "Layer timing",
                            |s| {
                                if let Some(l) = s.edit.layers.iter_mut().find(|l| l.id == id) {
                                    if head {
                                        l.start_ms = t.min(l.end_ms.saturating_sub(vedit::MIN_MS));
                                    } else {
                                        l.end_ms = t.max(l.start_ms + vedit::MIN_MS);
                                    }
                                }
                            },
                            cx,
                        )
                    }))
                };
                let moved = (l.x - dx).abs() > 0.01 || (l.y - dy).abs() > 0.01 || (l.scale - ds).abs() > 0.0001;
                let mut b = body
                    .child(self.prop_head(if is_text { ph::TEXT_T } else { ph::IMAGE }, &l.name(), format!("{} – {}", vedit::timecode(l.start_ms), vedit::timecode(l.end_ms))))
                    .child(self.section(
                        "motion",
                        ph::ARROWS_OUT,
                        "Motion",
                        moved.then(|| -> Box<dyn Fn(&mut Self, &mut Context<Self>)> {
                            Box::new(move |this: &mut Self, cx: &mut Context<Self>| {
                                this.video_edit(
                                    "Reset motion",
                                    |s| {
                                        if let Some(l) = s.edit.layers.iter_mut().find(|l| l.id == id) {
                                            (l.x, l.y, l.scale) = (dx, dy, ds);
                                        }
                                    },
                                    cx,
                                )
                            })
                        }),
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child(self.bar("x", "Position X", l.x / 100., format!("{:.0}%", l.x), cx))
                            .child(self.bar("y", "Position Y", l.y / 100., format!("{:.0}%", l.y), cx))
                            .child(self.bar("scale", "Scale", scale_f, scale_label, cx))
                            .child(muted("Or drag the layer on the Program monitor.")),
                        cx,
                    ))
                    .child(self.section(
                        "opacity",
                        ph::SQUARE_HALF,
                        "Opacity",
                        (l.opacity < 0.999).then(|| -> Box<dyn Fn(&mut Self, &mut Context<Self>)> {
                            Box::new(move |this: &mut Self, cx: &mut Context<Self>| {
                                this.video_edit(
                                    "Reset opacity",
                                    |s| {
                                        if let Some(l) = s.edit.layers.iter_mut().find(|l| l.id == id) {
                                            l.opacity = 1.;
                                        }
                                    },
                                    cx,
                                )
                            })
                        }),
                        self.bar("opacity", "Opacity", l.opacity, format!("{:.0}%", l.opacity * 100.), cx),
                        cx,
                    ));
                if let LayerKind::Text { color, boxed, .. } = &l.kind {
                    let sw = SWATCHES.iter().map(|s| {
                        let s = *s;
                        let on = color.eq_ignore_ascii_case(s);
                        div()
                            .id(SharedString::from(format!("vedit-sw-{s}")))
                            .size(px(20.))
                            .rounded_full()
                            .bg(video::paint(hex(s), 1.))
                            .border_2()
                            .border_color(if on { Tokens::accent() } else { Tokens::border() })
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.video_edit(
                                    "Text colour",
                                    |sq| {
                                        if let Some(LayerKind::Text { color, .. }) = sq.edit.layers.iter_mut().find(|l| l.id == id).map(|l| &mut l.kind) {
                                            *color = s.to_string();
                                        }
                                    },
                                    cx,
                                )
                            }))
                    });
                    let styled = !color.eq_ignore_ascii_case("#ffffff") || *boxed;
                    b = b.child(self.section(
                        "text",
                        ph::TEXT_T,
                        "Text",
                        styled.then(|| -> Box<dyn Fn(&mut Self, &mut Context<Self>)> {
                            Box::new(move |this: &mut Self, cx: &mut Context<Self>| {
                                this.video_edit(
                                    "Reset text style",
                                    |sq| {
                                        if let Some(LayerKind::Text { color, boxed, .. }) = sq.edit.layers.iter_mut().find(|l| l.id == id).map(|l| &mut l.kind) {
                                            *color = "#ffffff".into();
                                            *boxed = false;
                                        }
                                    },
                                    cx,
                                )
                            })
                        }),
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child(self.text_field())
                            .child(div().flex().gap(px(6.)).children(sw))
                            .child(switch("vedit-box", *boxed).label("Dark box behind the text").on_click(cx.listener(move |this, on: &bool, _, cx| {
                                let on = *on;
                                this.video_edit(
                                    "Text box",
                                    |sq| {
                                        if let Some(LayerKind::Text { boxed, .. }) = sq.edit.layers.iter_mut().find(|l| l.id == id).map(|l| &mut l.kind) {
                                            *boxed = on;
                                        }
                                    },
                                    cx,
                                )
                            }))),
                        cx,
                    ));
                }
                b.child(self.section(
                    "timing",
                    ph::TIMER,
                    "Timing",
                    None,
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(space::SM))
                        .child(set_time(true))
                        .child(set_time(false))
                        .child(btn("vedit-del-layer", "Delete", ButtonKind::DangerGhost).small().icon(ph::TRASH).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.video_delete_selection(cx)))),
                    cx,
                ))
            }
            Some(Sel::Track(t)) => body
                .child(self.prop_head(
                    match t {
                        Track::Captions => ph::CLOSED_CAPTIONING,
                        Track::Layers => ph::STACK,
                        Track::Clips => ph::IMAGE,
                        Track::Narration => ph::MICROPHONE,
                        Track::Music => ph::MUSIC_NOTES,
                    },
                    &format!("{} · {}", t.code(), t.label()),
                    String::new(),
                ))
                .child(muted(match t {
                    Track::Captions => "The captions follow the narration. Their style and place are in Sequence → Captions.",
                    Track::Layers => "Image and text layers over the picture. Add one below or with Import.",
                    Track::Clips => "One clip per narrated panel. Drag to reorder, drag an edge to trim, cut with the razor (C) or at the playhead (Ctrl+K).",
                    Track::Narration => "The narration of every clip. Mute it to keep only the music.",
                    Track::Music => "The chapter's score, your own music, or none. Change it in Sequence → Music; [ and ] nudge the volume.",
                }))
                .when(t == Track::Layers, |d| d.child(self.add_buttons(cx)))
                .when(t != Track::Clips, |d| {
                    d.child(btn("vedit-del-track", format!("Delete {}", t.label().to_lowercase()), ButtonKind::DangerGhost).small().icon(ph::TRASH).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.video_delete_track(t, cx))))
                }),
        };
        body.into_any_element()
    }

    fn add_buttons(&self, cx: &Context<Self>) -> Div {
        div()
            .flex()
            .flex_wrap()
            .gap(px(space::SM))
            .child(btn("vedit-add-text-2", "Text layer", ButtonKind::Secondary).small().icon(ph::TEXT_T).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.add_layer(LayerKind::Text { text: "Your text".into(), color: "#ffffff".into(), boxed: false }, cx)
            })))
            .child(btn("vedit-add-image", "Image layer…", ButtonKind::Secondary).small().icon(ph::IMAGE).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.import_image(cx))))
            .child(btn("vedit-add-logo", "Logo", ButtonKind::Secondary).small().icon(ph::SEAL_CHECK).tooltip("The Look & branding logo over the whole chapter").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.add_logo_layer(cx))))
    }

    fn prop_head(&self, icon: mv_ui::Ph, title: &str, sub: String) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .child(icon_ph(icon, Tokens::text_2(), icon_px::SM))
            .child(div().flex().flex_col().min_w_0().child(div().truncate().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title.to_string())).when(!sub.is_empty(), |d| d.child(div().text_size(px(size::XS)).font_family(crate::page::MONO).text_color(Tokens::text_3()).child(sub))))
    }

    fn prop(&self, label: &str, control: AnyElement) -> Div {
        div().flex().flex_col().gap(px(6.)).child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(label.to_string())).child(control)
    }

    /// The caption templates as live thumbnails (`w` x `h`), favourites first: each draws its
    /// look, hearts with a click and picks the template for the chapter's captions.
    fn style_cards(&self, key: &'static str, w: f32, h: f32, window: &Window, cx: &Context<Self>) -> Vec<AnyElement> {
        let Some(seq) = &self.vedit.seq else { return vec![] };
        let favs = &self.vedit.favs;
        vedit::favorites_first(video::styles(), |st| st.id.as_str(), favs)
            .into_iter()
            .map(|st| {
                let selected = st.id == seq.subs.style;
                let fav = favs.contains(&st.id);
                let (id, fid) = (st.id.clone(), st.id.clone());
                div()
                    .id(SharedString::from(format!("{key}-{}", st.id)))
                    .flex()
                    .flex_none()
                    .flex_col()
                    .gap(px(3.))
                    .w(px(w + 8.))
                    .p(px(2.))
                    .rounded(px(radius::MD))
                    .border_2()
                    .border_color(if selected { Tokens::accent() } else { Tokens::border() })
                    .bg(if selected { Tokens::accent_soft() } else { Tokens::surface_2() })
                    .cursor_pointer()
                    .when(!selected, |d| d.hover(|d| d.bg(Tokens::hover())))
                    .tooltip(tip(st.summary()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let id = id.clone();
                        this.video_subs(|s| s.style = id, cx)
                    }))
                    .child(
                        div()
                            .relative()
                            .w(px(w))
                            .h(px(h))
                            .rounded(px(radius::SM))
                            .overflow_hidden()
                            .child(video::style_thumb(st, w, h, window))
                            .child(
                                div().absolute().top(px(1.)).right(px(1.)).rounded(px(radius::SM)).bg(Tokens::scrim()).child(
                                    icon_toggle(SharedString::from(format!("{key}-fav-{}", st.id)), ph::HEART, fav, Tokens::danger(), "Remove from favourites", "Add to favourites")
                                        .size(12.)
                                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                            cx.stop_propagation();
                                            this.video_toggle_fav(&fid, cx)
                                        })),
                                ),
                            )
                            .when(selected, |d| d.child(div().absolute().top(px(4.)).left(px(4.)).child(selected_badge_sized(14.)))),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(3.))
                            .px(px(2.))
                            .when(fav, |d| d.child(icon_ph(ph::HEART, Tokens::danger(), 10.)))
                            .child(div().flex_1().min_w_0().truncate().text_size(px(size::XS)).text_color(if selected { Tokens::text_1() } else { Tokens::text_2() }).child(st.label.clone()))
                            .when(st.dynamic.is_some(), |d| d.child(icon_ph(ph::LIGHTNING, Tokens::text_3(), 10.))),
                    )
                    .into_any_element()
            })
            .collect()
    }

    /// The selected clip's caption (its first cue, as typed) in the chapter's caption style and
    /// place, over the clip's panel: the text and the style seen together while editing.
    fn caption_preview(&self, seq: &Seq, clip: &Clip, window: &Window) -> Div {
        let w = LEFT_W - 2. * space::MD - 8.;
        let (fw, fh) = if seq.reels { (112., 200.) } else { (w, w * 9. / 16.) };
        let beat = seq.beats.get(clip.src);
        let text = clip.text.clone().or_else(|| beat.map(|b| b.text.clone())).unwrap_or_default();
        let st = seq.subs.effective_style();
        let (words, _) = vedit::cue_at(&text, video::words_per_cue(&st, seq.subs.user_words()), 0.);
        let shown = seq.subs.enabled && !seq.edit.flags(Track::Captions).off;
        let frame = div()
            .relative()
            .flex_none()
            .w(px(fw))
            .h(px(fh))
            .rounded(px(radius::MD))
            .overflow_hidden()
            .bg(video::paint(hex("#000000"), 1.))
            .when_some(beat.and_then(|b| b.image.clone()), |d, p| d.child(img(p).absolute().inset_0().size_full().object_fit(ObjectFit::Contain).opacity(0.6)))
            .when(shown && !words.is_empty(), |d| d.child(video::caption_layer(&words, &st, seq.subs.user_words(), fw, fh, 1.min(words.len() - 1), false, true, window)));
        let note = if shown { format!("{} · drag the caption on the Program monitor to move every caption", video::find_style(&seq.subs.style).label) } else { "Captions are off (Sequence → Captions).".to_string() };
        div().flex().flex_col().gap(px(6.)).child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child("Preview")).child(div().flex().justify_center().child(frame)).child(muted(note))
    }

    /// A draggable value bar (`f` 0..1).
    fn bar(&self, name: &'static str, label: &str, f: f32, value: String, cx: &Context<Self>) -> Div {
        let bars = self.vedit.bars.clone();
        let f = f.clamp(0., 1.);
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(div().flex().child(div().flex_1().text_size(px(size::SM)).text_color(Tokens::text_2()).child(label.to_string())).child(div().text_size(px(size::SM)).font_family(crate::page::MONO).text_color(Tokens::accent()).child(value)))
            .child(
                div()
                    .id(name)
                    .relative()
                    .h(px(16.))
                    .cursor_pointer()
                    .child(div().absolute().left_0().right_0().top(px(6.)).h(px(4.)).rounded_full().bg(Tokens::track()))
                    .child(div().absolute().left_0().top(px(6.)).h(px(4.)).w(relative(f)).rounded_full().bg(Tokens::accent()))
                    .child(div().absolute().left(relative(f)).top(px(1.)).ml(px(-7.)).size(px(14.)).rounded_full().bg(Tokens::text_1()).border_2().border_color(Tokens::accent()))
                    .child(canvas(move |b, _, _| { bars.borrow_mut().insert(name, b); }, |_, _, _, _| {}).absolute().inset_0())
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        if let Some(f) = this.bar_fraction(name, ev.position.x) {
                            this.vedit.drag = Some(Drag::Bar(name));
                            this.bar_set(name, f, cx);
                            cx.notify();
                        }
                    })),
            )
    }

    /// The text box for the selected clip's caption or text layer.
    fn text_field(&self) -> AnyElement {
        match &self.vedit.text {
            Some(t) => Input::new(t).into_any_element(),
            None => div().into_any_element(),
        }
    }

    fn ensure_text_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.vedit.text.is_some() {
            return;
        }
        let t = cx.new(|cx| InputState::new(window, cx).placeholder("Text"));
        let sub = cx.subscribe_in(&t, window, |this: &mut Self, input, ev: &InputEvent, _, cx| {
            if !matches!(ev, InputEvent::Change) {
                return;
            }
            let value = input.read(cx).value().to_string();
            match this.vedit.text_for {
                Some(Sel::Clip(i)) => {
                    let current = this.vedit.seq.as_ref().and_then(|s| {
                        let c = s.edit.clips(&s.beats).get(i).cloned()?;
                        Some(c.text.clone().or_else(|| s.beats.get(c.src).map(|b| b.text.clone())).unwrap_or_default())
                    });
                    if current.as_deref() != Some(value.as_str()) {
                        this.video_clips("Caption text", 
                            |b, cl| {
                                if let Some(c) = cl.get_mut(i) {
                                    let own = b.get(c.src).map(|b| b.text.as_str()) == Some(value.as_str());
                                    c.text = (!own).then(|| value.clone());
                                }
                            },
                            cx,
                        );
                        // The Program monitor shows the edited caption in its style.
                        let span = this.vedit.seq.as_ref().and_then(|s| s.placed().into_iter().find(|p| p.index == i)).map(|p| (p.start_ms, p.end_ms()));
                        if let Some((a, b)) = span {
                            let now = this.playhead_now();
                            if this.vedit.playing.is_none() && (now < a || now >= b) {
                                this.seek(a, cx);
                            }
                        }
                    }
                }
                Some(Sel::Layer(id)) => {
                    let same = this.vedit.seq.as_ref().and_then(|s| s.edit.layers.iter().find(|l| l.id == id)).is_some_and(|l| matches!(&l.kind, LayerKind::Text { text, .. } if *text == value));
                    if !same {
                        this.video_edit("Layer text", 
                            |s| {
                                if let Some(LayerKind::Text { text, .. }) = s.edit.layers.iter_mut().find(|l| l.id == id).map(|l| &mut l.kind) {
                                    *text = value.clone();
                                }
                            },
                            cx,
                        );
                    }
                }
                _ => {}
            }
        });
        self.vedit.text = Some(t);
        self.vedit._subs.push(sub);
    }

    /// Fills the text box when the selection changes.
    fn sync_text_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sel = self.vedit.sel.filter(|s| matches!(s, Sel::Clip(_) | Sel::Layer(_)));
        if sel == self.vedit.text_for {
            return;
        }
        self.vedit.text_for = None;
        let value = match (sel, &self.vedit.seq) {
            (Some(Sel::Clip(i)), Some(s)) => s.edit.clips(&s.beats).get(i).and_then(|c| c.text.clone().or_else(|| s.beats.get(c.src).map(|b| b.text.clone()))),
            (Some(Sel::Layer(id)), Some(s)) => s.edit.layers.iter().find(|l| l.id == id).and_then(|l| match &l.kind {
                LayerKind::Text { text, .. } => Some(text.clone()),
                _ => None,
            }),
            _ => None,
        };
        if let Some(t) = &self.vedit.text {
            let v = value.unwrap_or_default();
            t.update(cx, |s, cx| s.set_value(v, window, cx));
        }
        self.vedit.text_for = sel;
    }

    /// Top right: the sequence's captions, music and look.
    fn sequence_panel(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(seq) = &self.vedit.seq else { return div().into_any_element() };
        let tabs = segmented("vedit-side", [(Some(ph::CLOSED_CAPTIONING), "Captions"), (Some(ph::MUSIC_NOTES), "Music"), (Some(ph::MAGIC_WAND), "Look")], self.vedit.side_tab).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
            this.vedit.side_tab = *ix;
            cx.notify();
        }));
        let body = div().id("vedit-seq").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().gap(px(space::MD)).p(px(space::MD)).child(tabs);
        let body = match self.vedit.side_tab {
            0 => {
                let s = &seq.subs;
                let off = seq.edit.flags(Track::Captions).off;
                // Two thumbnails a row in the panel.
                let tw = ((RIGHT_W - 2. * space::MD - 12. - 6.) / 2. - 8.).floor();
                let styles = self.style_cards("vedit-style", tw, (tw * 9. / 16.).round(), window, cx);
                let nfav = video::styles().iter().filter(|st| self.vedit.favs.contains(&st.id)).count();
                let place = segmented("vedit-place", PLACES.iter().map(|p| (None, p.0)), place_index(s.pos)).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
                    let ix = *ix;
                    this.video_subs(|s| set_place(s, ix), cx);
                    this.video_caption_pos_save(cx);
                }));
                let sizes = segmented("vedit-size", SIZES.iter().map(|p| (None, p.0)), crate::restyle::size_index(s.size)).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
                    let v = SIZES[(*ix).min(SIZES.len() - 1)].1;
                    this.video_subs(|s| set_size(s, v), cx)
                }));
                let y = s.pos.map(|p| p.1).unwrap_or_else(|| s.effective_style().pos.1);
                body.child(
                    switch("vedit-caps-on", s.enabled && !off).label("Captions on").on_click(cx.listener(|this, on: &bool, _, cx| {
                        let on = *on;
                        this.video_edit("Captions on/off", |sq| sq.edit.flags_mut(Track::Captions).off = !on, cx);
                        if on {
                            this.video_subs(|s| s.enabled = true, cx);
                        }
                    })),
                )
                .child(self.prop(
                    "Style",
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(muted(if nfav == 0 { "Heart a style to keep it first.".to_string() } else { format!("{nfav} favourite{} first", if nfav == 1 { "" } else { "s" }) }))
                        .child(div().flex().flex_wrap().gap(px(6.)).children(styles))
                        .into_any_element(),
                ))
                .child(self.prop("Place", place.into_any_element()))
                .child(muted("Or drag a caption on the Program monitor: it snaps to the grid and moves every caption in this manga."))
                .child(self.bar("caption-y", "Height in the frame", y / 100., format!("{y:.0}%"), cx))
                .child(self.prop("Size", sizes.into_any_element()))
                .child(
                    switch("vedit-caps-emph", s.emphasis).label("Highlight the spoken word").on_click(cx.listener(|this, on: &bool, _, cx| {
                        let on = *on;
                        this.video_subs(|s| s.emphasis = on, cx)
                    })),
                )
                .child(muted("The style is saved as this chapter's caption style (the caption studio in the Editor shows the same); the place applies to every chapter of the manga."))
            }
            1 => {
                let m = &seq.edit.music;
                let off = seq.edit.flags(Track::Music).off;
                let which = match m.source {
                    MusicSource::Score => 0,
                    MusicSource::File { .. } => 1,
                    MusicSource::None => 2,
                };
                let src = segmented("vedit-music", [(Some(ph::MUSIC_NOTES), "Score"), (Some(ph::FILE_AUDIO), "My file"), (Some(ph::SPEAKER_X), "None")], which).small().on_change(cx.listener(|this, ix: &usize, _, cx| match *ix {
                    0 => this.video_edit("Use the score", 
                        |s| {
                            s.edit.music.source = MusicSource::Score;
                            s.edit.flags_mut(Track::Music).off = false;
                        },
                        cx,
                    ),
                    1 => this.import_music(cx),
                    _ => this.video_edit("No music", |s| s.edit.music.source = MusicSource::None, cx),
                }));
                let what = match &m.source {
                    MusicSource::Score if seq.score.is_none() => "This chapter has no score yet (make it in the Music step).".to_string(),
                    MusicSource::Score => "The chapter's own score, timed to the narration.".to_string(),
                    MusicSource::File { path } => format!("{} · looped under the narration", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                    MusicSource::None => "No music: narration only.".to_string(),
                };
                body.child(self.prop("Music", src.into_any_element()))
                    .child(muted(what))
                    .when(matches!(m.source, MusicSource::File { .. }), |d| d.child(btn("vedit-music-replace", "Replace file…", ButtonKind::Secondary).small().icon(ph::FOLDER_OPEN).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.import_music(cx)))))
                    .child(self.bar("music", "Volume", m.volume / 2., format!("{:.0}%", m.volume * 100.), cx))
                    .child(
                        switch("vedit-music-mute", off).label("Mute the music").on_click(cx.listener(|this, on: &bool, _, cx| {
                            let on = *on;
                            this.video_edit("Mute music", |s| s.edit.flags_mut(Track::Music).off = on, cx)
                        })),
                    )
                    .child(
                        switch("vedit-narr-mute", seq.edit.flags(Track::Narration).off).label("Mute the narration").on_click(cx.listener(|this, on: &bool, _, cx| {
                            let on = *on;
                            this.video_edit("Mute narration", |s| s.edit.flags_mut(Track::Narration).off = on, cx)
                        })),
                    )
            }
            _ => {
                let global = crate::look::Look::load();
                let (preset, overrides) = match &seq.edit.look {
                    Some(l) => (Preset::parse(&l.preset).unwrap_or_default(), Some(l.effects.clone())),
                    None => (global.preset, None),
                };
                let fx = match &overrides {
                    Some(o) => {
                        let base = preset.effects();
                        Effect::ALL.map(|e| o.get(e.key()).copied().unwrap_or(base.get(e)))
                    }
                    None => {
                        let f = global.effects();
                        Effect::ALL.map(|e| f.get(e))
                    }
                };
                let own = seq.edit.look.is_some();
                let scope = segmented("vedit-look-scope", [(None, "Studio look"), (None, "This chapter")], own as usize).small().on_change(cx.listener(move |this, ix: &usize, _, cx| {
                    let own = *ix == 1;
                    this.video_edit("Look scope", |s| s.edit.look = own.then(|| LookEdit { preset: crate::look::Look::load().preset.key().into(), effects: Default::default() }), cx)
                }));
                let presets = segmented("vedit-preset", Preset::ALL.iter().map(|p| (None, p.label())), Preset::ALL.iter().position(|p| *p == preset).unwrap_or(3)).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
                    let p = Preset::ALL[(*ix).min(3)];
                    this.video_edit("Look preset", |s| s.edit.look = Some(LookEdit { preset: p.key().into(), effects: Default::default() }), cx)
                }));
                let switches = Effect::ALL.iter().enumerate().map(|(k, e)| {
                    let e = *e;
                    switch(SharedString::from(format!("vedit-fx-{}", e.key())), fx[k]).label(e.label()).on_click(cx.listener(move |this, on: &bool, _, cx| {
                        let on = *on;
                        this.video_edit("Look effect", 
                            |s| {
                                let mut l = s.edit.look.clone().unwrap_or_else(|| LookEdit { preset: crate::look::Look::load().preset.key().into(), effects: Default::default() });
                                let base = Preset::parse(&l.preset).unwrap_or_default().effects().get(e);
                                if on == base {
                                    l.effects.remove(e.key());
                                } else {
                                    l.effects.insert(e.key().into(), on);
                                }
                                s.edit.look = Some(l);
                            },
                            cx,
                        )
                    }))
                });
                let cuts = segmented("vedit-trans", [(None, "Look's transitions"), (None, "Hard cuts")], seq.edit.cuts as usize).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
                    let on = *ix == 1;
                    this.video_edit("Transitions", |s| s.edit.cuts = on, cx)
                }));
                body.child(self.prop("Look for", scope.into_any_element()))
                    .child(self.prop("Preset", presets.into_any_element()))
                    .child(muted(preset.blurb()))
                    .child(self.prop("Effects", div().flex().flex_col().gap(px(6.)).children(switches).into_any_element()))
                    .child(self.prop("Transitions", cuts.into_any_element()))
                    .child(muted("Looks, effects and transitions show in the rendered video; Render the chapter to see them."))
            }
        };
        body.into_any_element()
    }

    /// Bottom left: the manga's chapters as sequences (and Premiere's Effects tab).
    fn bin(&self, g: &MangaGroup, unrendered: &[ChapterItem], cx: &mut Context<Self>) -> AnyElement {
        let tabs = segmented("vedit-bin-tab", [(Some(ph::FOLDER), "Project"), (Some(ph::MAGIC_WAND), "Effects")], self.vedit.bin_tab).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
            this.vedit.bin_tab = *ix;
            cx.notify();
        }));
        let open = self.vedit.seq.as_ref().map(|s| s.item.id.clone());
        let pending: Vec<String> = unrendered.iter().map(|c| c.id.clone()).collect();
        let body = div().id("vedit-bin").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().gap(px(6.)).p(px(space::SM)).child(tabs);
        if self.vedit.bin_tab == 1 {
            let sel_clip = matches!(self.vedit.sel, Some(Sel::Clip(_)));
            let item = |id: &'static str, icon, label: &'static str, hint: &'static str| {
                interactive(id)
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .px(px(space::SM))
                    .h(px(30.))
                    .rounded(px(radius::SM))
                    .text_size(px(size::SM))
                    .text_color(Tokens::text_1())
                    .cursor_pointer()
                    .tooltip(tip(hint))
                    .child(icon_ph(icon, Tokens::text_2(), icon_px::SM))
                    .child(label)
            };
            let motion = |id: &'static str, m: Motion, hint: &'static str| {
                item(id, ph::ARROWS_OUT, m.label(), hint).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| match this.vedit.sel {
                    Some(Sel::Clip(i)) => this.video_clips("Clip motion", |_, c| c[i].motion = m, cx),
                    _ => toast::show(cx, Toast::info("Select a clip first")),
                }))
            };
            let look = |id: &'static str, p: Preset| {
                item(id, ph::PALETTE, p.label(), p.blurb()).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.video_edit("Look preset", |s| s.edit.look = Some(LookEdit { preset: p.key().into(), effects: Default::default() }), cx);
                    this.vedit.side_tab = 2;
                }))
            };
            return body
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("LOOKS (this chapter)"))
                .child(look("vedit-fx-clean", Preset::Clean))
                .child(look("vedit-fx-cine", Preset::Cinematic))
                .child(look("vedit-fx-recap", Preset::Recap))
                .child(look("vedit-fx-dyn", Preset::Dynamic))
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(if sel_clip { "MOTION (selected clip)" } else { "MOTION (select a clip)" }))
                .child(motion("vedit-fx-still", Motion::Still, "No movement"))
                .child(motion("vedit-fx-zoom", Motion::Zoom, "A slow push in"))
                .child(motion("vedit-fx-shake", Motion::Shake, "Zoom punch, camera shake and a flash"))
                .child(motion("vedit-fx-auto", Motion::Auto, "The look decides"))
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("TRANSITIONS"))
                .child(item("vedit-fx-cuts", ph::SCISSORS, "Hard cuts", "Cut between every clip").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.video_edit("Hard cuts", |s| s.edit.cuts = true, cx))))
                .child(item("vedit-fx-trans", ph::SPARKLE, "Look's transitions", "Wipes, slides and fades from the look").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.video_edit("Look transitions", |s| s.edit.cuts = false, cx))))
                .into_any_element();
        }
        let rows = self.made_chapters(g).into_iter().map(|c| {
            let on = open.as_deref() == Some(c.id.as_str());
            let edited = pending.contains(&c.id);
            let thumb = self.vedit.thumbs.borrow_mut().entry(c.id.clone()).or_insert_with(|| mv_recap::list_pages(&c.pages_dir).into_iter().next()).clone();
            let secs = c.video.as_ref().or(self.statuses.get(&c.id).and_then(|s| s.video.as_ref())).and_then(|p| crate::export::video_seconds(p));
            let id = c.id.clone();
            interactive(SharedString::from(format!("vedit-seq-{}", c.id)))
                .flex()
                .items_center()
                .gap(px(space::SM))
                .p(px(4.))
                .rounded(px(radius::SM))
                .cursor_pointer()
                .border_1()
                .border_color(if on { Tokens::accent() } else { Tokens::border() })
                .bg(if on { Tokens::accent_soft() } else { Tokens::surface_2() })
                .child(div().flex_none().w(px(64.)).h(px(36.)).rounded(px(2.)).overflow_hidden().bg(Tokens::well()).when_some(thumb, |d, p| d.child(img(p).size_full().object_fit(ObjectFit::Cover))))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .flex_1()
                        .child(div().truncate().text_size(px(size::SM)).text_color(Tokens::text_1()).child(c.label()))
                        .child(div().text_size(px(size::XS)).font_family(crate::page::MONO).text_color(Tokens::text_3()).child(secs.map(|s| vedit::timecode((s * 1000.) as u64)).unwrap_or_default())),
                )
                .when(edited, |d| d.child(icon_ph(ph::PENCIL_SIMPLE, Tokens::warning(), icon_px::XS)))
                .tooltip(tip(if edited { "Edited, not rendered yet" } else { "Open this chapter's sequence" }))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(c) = this.video_manga().and_then(|g| g.chapters.into_iter().find(|c| c.id == id)) {
                        this.video_open(c);
                    }
                    cx.notify();
                }))
        });
        let n = pending.len();
        body.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("SEQUENCES · {} finished chapter{}", self.made_chapters(g).len(), plural(self.made_chapters(g).len()))))
            .children(rows)
            .when(n > 0, |d| {
                let list: Vec<ChapterItem> = unrendered.to_vec();
                d.child(btn("vedit-render-all", format!("Render {n} edited chapter{}", plural(n)), ButtonKind::Secondary).small().icon(ph::FILM_STRIP).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.render_chapters(list.clone(), cx))))
            })
            .into_any_element()
    }

    /// Premiere's tools column.
    fn tools(&self, cx: &mut Context<Self>) -> Div {
        let tool = self.vedit.tool;
        div()
            .flex()
            .flex_col()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .w(px(TOOLS_W))
            .py(px(space::SM))
            .rounded(px(radius::MD))
            .bg(Tokens::surface())
            .border_1()
            .border_color(Tokens::border())
            .child(icon_button("vedit-tool-select", ph::ARROW_UP_RIGHT, "Selection tool (V)").small().selected(tool == Tool::Select).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.vedit.tool = Tool::Select;
                cx.notify();
            })))
            .child(icon_button("vedit-tool-razor", ph::SCISSORS, "Razor tool (C): click a clip to cut it").small().selected(tool == Tool::Razor).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.vedit.tool = Tool::Razor;
                cx.notify();
            })))
            .child(icon_button("vedit-tool-text", ph::TEXT_T, "Type tool: add a text layer").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.add_layer(LayerKind::Text { text: "Your text".into(), color: "#ffffff".into(), boxed: false }, cx)
            })))
            .child(icon_button("vedit-tool-image", ph::IMAGE, "Add an image layer").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.import_image(cx))))
            .child(icon_button("vedit-tool-music", ph::MUSIC_NOTES, "Replace the music with a file").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.import_music(cx))))
    }

    // ------------------------------------------------------------------------------ timeline

    /// The ruler of the open timeline (`total` ms long, playhead at `now`): labelled ticks from
    /// the tick-step table (at least 80 px apart) with minor ticks between, the in / out range,
    /// markers and the playhead's head. Click or drag it to move the playhead.
    pub(super) fn ruler(&self, id: &'static str, total: u64, now: u64, cx: &mut Context<Self>) -> Stateful<Div> {
        let (pps, sx) = (self.vedit.pps, self.vedit.scroll_x);
        let view_w = f32::from(self.vedit.lanes.get().size.width).max(1.);
        let xof = |ms: u64| ms as f32 / 1000. * pps - sx;
        let mut r = div()
            .id(id)
            .relative()
            .flex_none()
            .h(px(RULER_H))
            .overflow_hidden()
            .bg(Tokens::surface_2())
            .border_b_1()
            .border_color(Tokens::border())
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.vedit.drag = Some(Drag::Playhead);
                this.vedit.shift = ev.modifiers.shift;
                this.drag_move(ev.position, cx);
            }));
        // In / out: the range, and a bracket at each end.
        if self.vedit.mark_in.is_some() || self.vedit.mark_out.is_some() {
            let (a, b) = (xof(self.vedit.mark_in.unwrap_or(0)), xof(self.vedit.mark_out.unwrap_or(total)));
            r = r.child(div().absolute().left(px(a)).w(px((b - a).max(1.))).top_0().bottom_0().bg(soft(Tokens::accent(), 0.14)));
            for (t, left) in [(self.vedit.mark_in, true), (self.vedit.mark_out, false)] {
                if let Some(t) = t {
                    let x = xof(t);
                    r = r.child(div().absolute().left(px(if left { x } else { x - 5. })).top(px(4.)).w(px(6.)).h(px(RULER_H - 8.)).border_color(Tokens::accent()).border_t_2().border_b_2().when(left, |d| d.border_l_2()).when(!left, |d| d.border_r_2()));
                }
            }
        }
        let step = tl::tick_step(pps);
        let (start_s, end_s) = ((sx / pps) as f64, (((sx + view_w) / pps) as f64).min(total as f64 / 1000. + step));
        let first = (start_s / step).floor() * step;
        let mut k = 0;
        loop {
            let at = first + k as f64 * step;
            if at > end_s || k > 400 {
                break;
            }
            let x = (at * pps as f64) as f32 - sx;
            r = r.child(div().absolute().left(px(x)).bottom_0().w(px(1.)).h(px(10.)).bg(Tokens::text_3()));
            if x + 56. <= view_w {
                r = r.child(div().absolute().left(px(x + 3.)).top(px(5.)).text_size(px(10.)).font_family(crate::page::MONO).text_color(Tokens::text_3()).child(tl::tick_label((at * 1000.).round() as u64, step)));
            }
            for m in 1..tl::MINOR {
                let mx = ((at + step * m as f64 / tl::MINOR as f64) * pps as f64) as f32 - sx;
                r = r.child(div().absolute().left(px(mx)).bottom_0().w(px(1.)).h(px(5.)).bg(Tokens::border_strong()));
            }
            k += 1;
        }
        for (i, m) in self.vedit.markers.iter().enumerate() {
            let x = xof(*m);
            let t = *m;
            r = r.child(
                div()
                    .id(("vedit-marker", i))
                    .absolute()
                    .left(px(x - 4.))
                    .bottom(px(1.))
                    .w(px(9.))
                    .h(px(10.))
                    .rounded_t(px(2.))
                    .bg(Tokens::favorite())
                    .tooltip(tip(format!("Marker {} · {} (M at the playhead removes it)", i + 1, vedit::timecode(t))))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.seek(t, cx);
                    })),
            );
        }
        r.child(div().absolute().left(px(xof(now) - 5.)).top(px(RULER_H - 13.)).w(px(11.)).h(px(12.)).rounded_b(px(3.)).bg(Tokens::accent()))
    }

    /// The navigator under the timeline: the thumb shows (and pans) the visible part; its handles
    /// zoom (bhippi's zoom bar).
    pub(super) fn navigator(&self, id: &'static str, total: u64, cx: &mut Context<Self>) -> Stateful<Div> {
        let bar = self.vedit.nav.get();
        let bar_w = f32::from(bar.size.width);
        let view_w = f32::from(self.vedit.lanes.get().size.width).max(1.);
        let content = total as f32 / 1000. * self.vedit.pps;
        let (l, w) = tl::nav_thumb(bar_w, content, view_w, self.vedit.scroll_x);
        let start = move |grip: NavGrip| {
            move |this: &mut Self, ev: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>| {
                cx.stop_propagation();
                this.vedit.drag = Some(Drag::Nav { grip, x0: f32::from(ev.position.x), pps0: this.vedit.pps, scroll0: this.vedit.scroll_x });
                cx.notify();
            }
        };
        let handle = |hid: &'static str, left: bool| {
            div()
                .id(hid)
                .absolute()
                .top_0()
                .bottom_0()
                .w(px(7.))
                .when(left, |d| d.left_0())
                .when(!left, |d| d.right_0())
                .rounded(px(3.))
                .cursor(CursorStyle::ResizeLeftRight)
                .bg(soft(Tokens::text_2(), 0.35))
                .hover(|d| d.bg(Tokens::text_2()))
        };
        let nav = self.vedit.nav.clone();
        div()
            .id(id)
            .relative()
            .flex_none()
            .h(px(NAV_H))
            .ml(px(HEAD_W))
            .my(px(2.))
            .rounded(px(3.))
            .bg(Tokens::well())
            .cursor_pointer()
            .child(canvas(move |b, _, _| nav.set(b), |_, _, _, _| {}).absolute().inset_0())
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                // A click on the bar centres the thumb there, then drags it.
                let b = this.vedit.nav.get();
                let bw = f32::from(b.size.width);
                let vw = f32::from(this.vedit.lanes.get().size.width).max(1.);
                let content = this.timeline_ms() as f32 / 1000. * this.vedit.pps;
                if bw > 0. && content > vw {
                    let ratio = bw / content;
                    let (_, tw) = tl::nav_thumb(bw, content, vw, 0.);
                    this.vedit.scroll_x = ((f32::from(ev.position.x - b.origin.x) - tw / 2.) / ratio).clamp(0., content - vw);
                }
                this.vedit.drag = Some(Drag::Nav { grip: NavGrip::Move, x0: f32::from(ev.position.x), pps0: this.vedit.pps, scroll0: this.vedit.scroll_x });
                cx.notify();
            }))
            .child(
                div()
                    .id("vedit-nav-thumb")
                    .absolute()
                    .top(px(1.))
                    .bottom(px(1.))
                    .left(px(l))
                    .w(px(w))
                    .rounded(px(3.))
                    .bg(Tokens::border_strong())
                    .cursor(CursorStyle::OpenHand)
                    .on_mouse_down(MouseButton::Left, cx.listener(start(NavGrip::Move)))
                    .child(handle("vedit-nav-l", true).on_mouse_down(MouseButton::Left, cx.listener(start(NavGrip::Left))))
                    .child(handle("vedit-nav-r", false).on_mouse_down(MouseButton::Left, cx.listener(start(NavGrip::Right)))),
            )
    }

    /// Ctrl / Alt + wheel zooms at the pointer; the wheel alone scrolls the timeline.
    pub(super) fn timeline_wheel(&mut self, ev: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let d = ev.delta.pixel_delta(px(20.));
        let (dx, dy) = (f32::from(d.x), f32::from(d.y));
        if ev.modifiers.control || ev.modifiers.alt {
            let ax = f32::from(ev.position.x) - f32::from(self.vedit.lanes.get().origin.x);
            self.zoom(tl::wheel_factor(dy), Some(ax));
        } else {
            let total = self.timeline_ms() as f32 / 1000. * self.vedit.pps;
            let w = f32::from(self.vedit.lanes.get().size.width);
            self.vedit.scroll_x = (self.vedit.scroll_x - if dx.abs() > dy.abs() { dx } else { dy }).clamp(0., (total - w + 40.).max(0.));
        }
        cx.notify();
    }

    /// The timeline toolbar's shared buttons: snap, zoom, fit and the shortcuts list.
    pub(super) fn timeline_buttons(&self, cx: &mut Context<Self>) -> Div {
        let snap = self.vedit.snap;
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .child(icon_button("vedit-snap", ph::LINK_SIMPLE, if snap { "Snap: on (S; hold Shift to invert)" } else { "Snap: off (S; hold Shift to invert)" }).small().selected(snap).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.vedit.snap = !this.vedit.snap;
                cx.notify();
            })))
            .child(icon_button("vedit-zoom-out", ph::MAGNIFYING_GLASS_MINUS, "Zoom out (-)").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.zoom(1. / 1.5, None);
                cx.notify();
            })))
            .child(icon_button("vedit-zoom-in", ph::MAGNIFYING_GLASS, "Zoom in (=) · Ctrl+wheel zooms at the pointer").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.zoom(1.5, None);
                cx.notify();
            })))
            .child(icon_button("vedit-zoom-fit", ph::ARROWS_IN_SIMPLE, "Zoom to the whole timeline (\\)").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.video_fit();
                cx.notify();
            })))
            .child(icon_button("vedit-help", ph::QUESTION, "Keyboard shortcuts (?)").small().selected(self.vedit.keys_open).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.vedit.keys_open = !this.vedit.keys_open;
                cx.notify();
            })))
    }

    /// In / out and markers, as a short note for the toolbar.
    fn marks_note(&self) -> Option<String> {
        let mut parts = vec![];
        if self.vedit.mark_in.is_some() || self.vedit.mark_out.is_some() {
            let f = |t: Option<u64>| t.map(vedit::timecode).unwrap_or_else(|| "--".into());
            parts.push(format!("In {} · Out {}", f(self.vedit.mark_in), f(self.vedit.mark_out)));
        }
        if !self.vedit.markers.is_empty() {
            parts.push(format!("{} marker{}", self.vedit.markers.len(), plural(self.vedit.markers.len())));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    /// The clips and layers the last assistant turn changed, while they are lit.
    fn flashed(&self) -> (Vec<usize>, Vec<u64>) {
        match &self.vedit.ai_flash {
            Some((t, c, l)) if t.elapsed() < AI_FLASH => (c.clone(), l.clone()),
            _ => (vec![], vec![]),
        }
    }

    fn video_timeline(&self, cx: &mut Context<Self>) -> Div {
        let Some(seq) = &self.vedit.seq else { return div() };
        let placed = seq.placed();
        let total = vedit::total_ms(&placed);
        let pps = self.vedit.pps;
        let sx = self.vedit.scroll_x;
        let view_w = f32::from(self.vedit.lanes.get().size.width).max(1.);
        let (t0, t1) = ((sx / pps * 1000.) as u64, ((sx + view_w) / pps * 1000.) as u64 + 1);
        let xof = |ms: u64| ms as f32 / 1000. * pps - sx;
        let now = self.playhead_now();
        let (ai_clips, ai_layers) = self.flashed();
        let toolbar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(space::XS))
            .h(px(34.))
            .px(px(space::SM))
            .border_b_1()
            .border_color(Tokens::border())
            .child(div().text_size(px(size::LG)).font_family(crate::page::MONO).text_color(Tokens::accent()).child(vedit::timecode(now)))
            .child(div().w(px(space::SM)))
            .child(self.timeline_buttons(cx))
            .when_some(self.vedit.shuttle, |d, (_, _, s)| d.child(div().text_size(px(size::SM)).font_family(crate::page::MONO).text_color(Tokens::accent()).child(format!("{}{}x", if s < 0. { "◀ " } else { "▶ " }, s.abs()))))
            .child(div().flex_1())
            .children(self.marks_note().map(muted))
            .child(muted(format!("{} clips · {}", placed.len(), vedit::timecode(total))))
            .when(seq.edit.stale(&seq.beats), |d| d.child(div().text_size(px(size::SM)).text_color(Tokens::warning()).child("Re-narrated: clip edits reset")));
        let ruler = self.ruler("vedit-ruler", total, now, cx);

        // Lanes.
        let sel = self.vedit.sel;
        let drag = self.vedit.drag;
        let razor = self.vedit.tool == Tool::Razor;
        let visible = |p: &&Placed| p.end_ms() >= t0 && p.start_ms <= t1;
        let mut lanes: Vec<AnyElement> = vec![];
        for t in Track::ALL {
            let h = lane_h(t);
            let flags = seq.edit.flags(t);
            let color = track_color(t);
            let mut lane = div()
                .id(SharedString::from(format!("vedit-lane-{}", t.code())))
                .relative()
                .h(px(h))
                .overflow_hidden()
                .border_b_1()
                .border_color(Tokens::border())
                .when(flags.locked, |d| d.bg(soft(Tokens::text_3(), 0.07)))
                .when(flags.off, |d| d.opacity(0.4));
            match t {
                Track::Clips | Track::Narration | Track::Captions => {
                    for p in placed.iter().filter(visible) {
                        let (x, w) = (xof(p.start_ms), p.dur_ms as f32 / 1000. * pps);
                        let index = p.index;
                        let beat = seq.beats.get(p.clip.src);
                        let selected = sel == Some(Sel::Clip(index));
                        let dragging = matches!(drag, Some(Drag::Clip { index: i, .. }) if i == index);
                        let label = match t {
                            Track::Clips => format!("p{}{}", beat.map(|b| b.page + 1).unwrap_or(0), if p.clip.motion == Motion::Auto { String::new() } else { format!(" · {}", p.clip.motion.label()) }),
                            Track::Narration => format!("{:.1}s · {}", p.dur_ms as f32 / 1000., beat.and_then(|b| b.audio.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                            _ => p.clip.text.clone().or_else(|| beat.map(|b| b.text.clone())).unwrap_or_default(),
                        };
                        let mut el = clip_box(SharedString::from(format!("vedit-{}-{index}", t.code())), x, w, h, color, selected, ai_clips.contains(&index), None, label).when(dragging, |d| d.opacity(0.35));
                        match (t, beat) {
                            (Track::Clips, Some(b)) if w > 12. => {
                                if let Some(image) = &b.image {
                                    el = el.children(clip_tiles(image, x, w, h, view_w));
                                }
                            }
                            (Track::Narration, Some(b)) if w > 6. => {
                                if let Some(pk) = crate::vedit_media::peaks_ready(&b.audio) {
                                    el = el.children(clip_wave(&pk, (b.offset_ms + p.clip.trim_in_ms) as f64 / 1000., x, w, h, pps, view_w));
                                }
                            }
                            _ => {}
                        }
                        if p.clip.cut && t == Track::Clips {
                            el = el.child(div().absolute().left_0().top(px(LABEL_H)).bottom_0().w(px(3.)).bg(Tokens::warning()));
                        }
                        // Trim handles on the picture clips.
                        if t == Track::Clips && !razor && !flags.locked && w > 14. {
                            for head in [true, false] {
                                el = el.child(
                                    div()
                                        .id(SharedString::from(format!("vedit-trim-{index}-{head}")))
                                        .absolute()
                                        .top_0()
                                        .bottom_0()
                                        .w(px(7.))
                                        .when(head, |d| d.left_0())
                                        .when(!head, |d| d.right_0())
                                        .cursor(CursorStyle::ResizeLeftRight)
                                        .hover(|d| d.bg(soft(Tokens::text_1(), 0.3)))
                                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                            cx.stop_propagation();
                                            let from = this.time_at(ev.position.x);
                                            this.vedit.sel = Some(Sel::Clip(index));
                                            this.vedit.drag = Some(Drag::Trim { index, head, from, applied: 0 });
                                            cx.notify();
                                        })),
                                );
                            }
                        }
                        let start = p.start_ms;
                        let dur = p.dur_ms;
                        lane = lane.child(el.cursor(if razor { CursorStyle::Crosshair } else { CursorStyle::PointingHand }).on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.vedit.shift = ev.modifiers.shift;
                            let at = this.time_at(ev.position.x);
                            if this.vedit.tool == Tool::Razor {
                                if this.locked(Track::Clips) {
                                    return;
                                }
                                let cut = this.snap_ms(at, None);
                                this.vedit.snapped = None;
                                let into = (cut - start as i64).clamp(0, dur as i64) as u64;
                                let mut ok = false;
                                this.video_clips("Razor cut", |b, c| ok = vedit::split(b, c, index, into), cx);
                                if !ok {
                                    toast::show(cx, Toast::info("Too close to the clip's edge to cut there"));
                                }
                                return;
                            }
                            this.vedit.sel = Some(Sel::Clip(index));
                            if t == Track::Clips && !this.locked(Track::Clips) {
                                this.vedit.drag = Some(Drag::Clip { index, grab: at - start as i64, at });
                            }
                            cx.notify();
                        })));
                    }
                }
                Track::Layers => {
                    for l in seq.edit.layers.iter().filter(|l| l.end_ms >= t0 && l.start_ms <= t1) {
                        let (x, w) = (xof(l.start_ms), (l.end_ms - l.start_ms) as f32 / 1000. * pps);
                        let id = l.id;
                        let selected = sel == Some(Sel::Layer(id));
                        let icon = if matches!(l.kind, LayerKind::Text { .. }) { ph::TEXT_T } else { ph::IMAGE };
                        let mut el = clip_box(SharedString::from(format!("vedit-layer-{id}")), x, w, h, color, selected, ai_layers.contains(&id), Some(icon), l.name());
                        if let LayerKind::Image { path } = &l.kind {
                            if w > 30. {
                                el = el.children(clip_tiles(path, x, w.min(h * 2.), h, view_w));
                            }
                        }
                        if !flags.locked && w > 14. {
                            for head in [true, false] {
                                el = el.child(
                                    div()
                                        .id(SharedString::from(format!("vedit-ledge-{id}-{head}")))
                                        .absolute()
                                        .top_0()
                                        .bottom_0()
                                        .w(px(7.))
                                        .when(head, |d| d.left_0())
                                        .when(!head, |d| d.right_0())
                                        .cursor(CursorStyle::ResizeLeftRight)
                                        .hover(|d| d.bg(soft(Tokens::text_1(), 0.3)))
                                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                            cx.stop_propagation();
                                            this.vedit.shift = ev.modifiers.shift;
                                            this.vedit.sel = Some(Sel::Layer(id));
                                            this.vedit.drag = Some(Drag::LayerEdge { id, head });
                                            cx.notify();
                                        })),
                                );
                            }
                        }
                        let start = l.start_ms;
                        lane = lane.child(el.cursor(CursorStyle::PointingHand).on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.vedit.shift = ev.modifiers.shift;
                            this.vedit.sel = Some(Sel::Layer(id));
                            if !this.locked(Track::Layers) {
                                this.vedit.drag = Some(Drag::Layer { id, grab: this.time_at(ev.position.x) - start as i64 });
                            }
                            cx.notify();
                        })));
                    }
                }
                Track::Music => {
                    let src = match &seq.edit.music.source {
                        MusicSource::Score => seq.score.clone().map(|p| (p, "score".to_string())),
                        MusicSource::File { path } => Some((path.clone(), path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())),
                        MusicSource::None => None,
                    };
                    if let Some((path, name)) = src {
                        let selected = sel == Some(Sel::Track(Track::Music));
                        let (x, w) = (xof(0), total as f32 / 1000. * pps);
                        let mut el = clip_box("vedit-music-clip".into(), x, w, h, color, selected, false, Some(ph::MUSIC_NOTES), format!("{name} · {:.0}%", seq.edit.music.volume * 100.));
                        if let Some(pk) = crate::vedit_media::peaks_ready(&path) {
                            el = el.children(clip_wave(&pk, 0., x, w, h, pps, view_w));
                        }
                        lane = lane.child(el.on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.vedit.sel = Some(Sel::Track(Track::Music));
                            this.vedit.side_tab = 1;
                            cx.notify();
                        })));
                    }
                }
            }
            lanes.push(lane.into_any_element());
        }
        // Where a dragged clip would land, the in / out range, the snap line and the playhead.
        let lanes_h: f32 = Track::ALL.iter().map(|t| lane_h(*t)).sum();
        let mut over = div().absolute().top_0().left_0().right_0().h(px(lanes_h));
        if let Some(Drag::Clip { index, grab, at }) = drag {
            if let Some(me) = placed.iter().find(|p| p.index == index) {
                let x = (at - grab) as f32 / 1000. * pps - sx;
                let top: f32 = lane_h(Track::Captions) + lane_h(Track::Layers);
                over = over.child(div().absolute().left(px(x)).top(px(top + 1.)).w(px(me.dur_ms as f32 / 1000. * pps)).h(px(lane_h(Track::Clips) - 2.)).rounded(px(3.)).border_2().border_color(Tokens::text_1()).bg(soft(Tokens::info(), 0.5)));
            }
        }
        over = over.children(self.lane_overlays(total, now, None));

        let lanes_box = self.vedit.lanes.clone();
        let content = div()
            .id("vedit-lanes")
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .child(ruler)
            .child(div().relative().flex().flex_col().children(lanes).child(over))
            .child(canvas(move |b, _, _| lanes_box.set(b), |_, _, _, _| {}).absolute().inset_0())
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                // Empty timeline: move the playhead and clear the selection.
                this.vedit.sel = None;
                this.vedit.drag = Some(Drag::Playhead);
                this.vedit.shift = ev.modifiers.shift;
                this.drag_move(ev.position, cx);
            }))
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| this.timeline_wheel(ev, cx)));

        // Track heads: V3 / V2 / V1 / A1 / A2, lock and eye / mute.
        let mut heads = div().flex().flex_col().flex_none().w(px(HEAD_W)).border_r_1().border_color(Tokens::border()).child(div().h(px(RULER_H)).bg(Tokens::surface_2()).border_b_1().border_color(Tokens::border()));
        for t in Track::ALL {
            let flags = seq.edit.flags(t);
            let selected = sel == Some(Sel::Track(t));
            let code = t.code();
            heads = heads.child(
                self.track_head(SharedString::from(format!("vedit-head-{code}")), t, lane_h(t), selected)
                    .child(icon_button(SharedString::from(format!("vedit-lock-{code}")), if flags.locked { ph::LOCK } else { ph::LOCK_OPEN }, if flags.locked { "Unlock the track" } else { "Lock the track" }).small().selected(flags.locked).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.video_edit(
                            "Lock track",
                            |s| {
                                let f = s.edit.flags_mut(t);
                                f.locked = !f.locked;
                            },
                            cx,
                        );
                    })))
                    .when(t != Track::Clips, |d| {
                        let (icon, label) = match (t.audio(), flags.off) {
                            (true, true) => (ph::SPEAKER_X, "Unmute the track"),
                            (true, false) => (ph::SPEAKER_HIGH, "Mute the track"),
                            (false, true) => (ph::EYE_SLASH, "Show the track"),
                            (false, false) => (ph::EYE, "Hide the track"),
                        };
                        d.child(icon_button(SharedString::from(format!("vedit-off-{code}")), icon, label).small().selected(flags.off).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            if this.locked(t) {
                                return;
                            }
                            this.video_edit(
                                "Track on/off",
                                |s| {
                                    let f = s.edit.flags_mut(t);
                                    f.off = !f.off;
                                },
                                cx,
                            );
                        })))
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.vedit.sel = Some(Sel::Track(t));
                        if t == Track::Captions {
                            this.vedit.side_tab = 0;
                        } else if t.audio() {
                            this.vedit.side_tab = 1;
                        }
                        cx.notify();
                    })),
            );
        }

        let nav = self.navigator("vedit-nav", total, cx);
        let body = div().flex().flex_col().flex_1().min_h_0().child(toolbar).child(div().flex().flex_1().min_h_0().child(heads).child(content)).child(nav);
        self.panel(&format!("{} ×", seq.item.label()), Some(self.chapter_switcher(cx)), body).flex_1()
    }

    /// A track head: the track's code in its colour, then its name (the buttons follow).
    pub(super) fn track_head(&self, id: SharedString, t: Track, h: f32, selected: bool) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(4.))
            .h(px(h))
            .pl(px(6.))
            .pr(px(2.))
            .border_b_1()
            .border_color(Tokens::border())
            .bg(if selected { Tokens::active() } else { Tokens::surface() })
            .cursor_pointer()
            .child(div().flex_none().w(px(24.)).h(px(18.)).rounded(px(3.)).flex().items_center().justify_center().text_size(px(10.)).font_weight(FontWeight::SEMIBOLD).bg(soft(track_color(t), 0.55)).text_color(Tokens::text_1()).child(t.code()))
            .child(div().flex_1().min_w_0().truncate().text_size(px(size::XS)).text_color(Tokens::text_2()).child(t.label()))
    }

    /// Over the lanes: the in / out range, chapter `edges`, the snap line and the playhead.
    pub(super) fn lane_overlays(&self, total: u64, now: u64, edges: Option<&[u64]>) -> Vec<AnyElement> {
        let (pps, sx) = (self.vedit.pps, self.vedit.scroll_x);
        let xof = |ms: u64| ms as f32 / 1000. * pps - sx;
        let mut v: Vec<AnyElement> = vec![];
        if self.vedit.mark_in.is_some() || self.vedit.mark_out.is_some() {
            let (a, b) = (xof(self.vedit.mark_in.unwrap_or(0)), xof(self.vedit.mark_out.unwrap_or(total)));
            v.push(div().absolute().left(px(a)).w(px((b - a).max(1.))).top_0().bottom_0().bg(soft(Tokens::accent(), 0.06)).into_any_element());
        }
        for e in edges.unwrap_or_default() {
            v.push(div().absolute().left(px(xof(*e))).top_0().bottom_0().w(px(1.)).bg(Tokens::border_strong()).into_any_element());
        }
        for m in &self.vedit.markers {
            v.push(div().absolute().left(px(xof(*m))).top_0().bottom_0().w(px(1.)).bg(soft(Tokens::favorite(), 0.5)).into_any_element());
        }
        if let Some(s) = self.vedit.snapped {
            v.push(div().absolute().left(px(xof(s))).top_0().bottom_0().w(px(1.)).bg(Tokens::favorite()).into_any_element());
        }
        v.push(div().absolute().left(px(xof(now))).top_0().bottom_0().w(px(1.)).bg(Tokens::accent()).into_any_element());
        v
    }

    /// `?`: every Video mode shortcut, by group, as the user's keymap has them.
    fn shortcuts_dialog(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let rows = vedit_keys::help_rows();
        let mut groups: Vec<(&str, Vec<(&str, String)>)> = vec![];
        for (g, label, keys) in rows {
            match groups.iter_mut().find(|(n, _)| *n == g) {
                Some((_, v)) => v.push((label, keys)),
                None => groups.push((g, vec![(label, keys)])),
            }
        }
        let col = |gs: &[(&str, Vec<(&str, String)>)]| {
            div().flex().flex_col().flex_1().gap(px(space::MD)).children(gs.iter().map(|(g, items)| {
                div().flex().flex_col().gap(px(3.)).child(div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_3()).child(g.to_uppercase())).children(items.iter().map(|(label, keys)| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).text_color(Tokens::text_1()).child(label.to_string()))
                        .child(div().flex_none().px(px(6.)).py(px(1.)).rounded(px(radius::SM)).bg(Tokens::surface_2()).border_1().border_color(Tokens::border()).text_size(px(size::XS)).font_family(crate::page::MONO).text_color(Tokens::text_2()).child(if keys.is_empty() { "—".to_string() } else { keys.clone() }))
                }))
            }))
        };
        let half = groups.len().div_ceil(2);
        div()
            .id("vedit-keys-dialog")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(Tokens::scrim())
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.vedit.keys_open = false;
                cx.notify();
            }))
            .child(
                div()
                    .id("vedit-keys-card")
                    .flex()
                    .flex_col()
                    .gap(px(space::MD))
                    .w(px(760.))
                    .max_h(relative(0.88))
                    .overflow_y_scroll()
                    .p(px(space::LG))
                    .rounded(px(radius::LG))
                    .bg(Tokens::surface())
                    .border_1()
                    .border_color(Tokens::border_strong())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(div().flex_1().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Keyboard shortcuts"))
                            .child(icon_button("vedit-keys-close", ph::X, "Close (Esc)").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.vedit.keys_open = false;
                                cx.notify();
                            }))),
                    )
                    .child(muted(format!("Change them in {} (command id → keys).", vedit_keys::overrides_path().display())))
                    .child(div().flex().gap(px(space::LG)).child(col(&groups[..half])).child(col(&groups[half..]))),
            )
    }

    // ------------------------------------------------------------------------------ export

    /// Premiere's Export: the Video mode's presets (YouTube 1080 / 4K, Shorts, Draft) and, folded
    /// away, the dashboard's Full recap export (parts, hook, source quality), both over the
    /// rendered chapter videos.
    fn export_dialog(&mut self, g: &MangaGroup, pending: Vec<ChapterItem>, cx: &mut Context<Self>) -> Stateful<Div> {
        let n = pending.len();
        let rendering = self.made_chapters(g).iter().any(|c| self.queue.for_chapter(&c.id).is_some_and(|q| q.state.active()));
        let more = self.vedit.export_more;
        let presets = self.export_presets(g, cx);
        let section = more.then(|| self.video_section(g, cx));
        div()
            .id("vedit-export")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(Tokens::scrim())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id("vedit-export-card")
                    .flex()
                    .flex_col()
                    .gap(px(space::MD))
                    .w(px(720.))
                    .max_h(relative(0.9))
                    .overflow_y_scroll()
                    .p(px(space::LG))
                    .rounded(px(radius::LG))
                    .bg(Tokens::surface())
                    .border_1()
                    .border_color(Tokens::border_strong())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(div().flex_1().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("Export · {}", g.title)))
                            .child(icon_button("vedit-export-close", ph::X, "Close (Esc)").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.vedit.export_open = false;
                                cx.notify();
                            }))),
                    )
                    .when(n > 0 || rendering, |d| {
                        let list = pending.clone();
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::MD))
                                .p(px(space::MD))
                                .rounded(px(radius::MD))
                                .bg(Tokens::warning_soft())
                                .border_1()
                                .border_color(Tokens::warning_border())
                                .child(icon_ph(ph::WARNING, Tokens::warning(), icon_px::SM))
                                .child(div().flex_1().text_size(px(size::SM)).text_color(Tokens::text_1()).child(if rendering {
                                    "Chapters are still rendering; export once they finish so the edits are in.".to_string()
                                } else {
                                    format!("{n} chapter{} edited but not rendered: the export would use the old video{}.", plural(n), if n == 1 { "" } else { "s" })
                                }))
                                .when(n > 0 && !rendering, |d| d.child(btn("vedit-export-render", "Render first", ButtonKind::Primary).small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.render_chapters(list.clone(), cx))))),
                        )
                    })
                    .child(presets)
                    .child(
                        div()
                            .id("vedit-export-more")
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.vedit.export_more = !this.vedit.export_more;
                                cx.notify();
                            }))
                            .child(icon_ph(if more { ph::CARET_DOWN } else { ph::CARET_RIGHT }, Tokens::text_3(), 10.))
                            .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child("Parts, hook and source-quality export")),
                    )
                    .when_some(section, |d, s| d.child(s)),
            )
    }

    /// The Video mode's own export: `encode`'s presets, the GPU encoder when one works, loudness
    /// to -14 LUFS; progress with the time left and Cancel.
    fn export_presets(&self, g: &MangaGroup, cx: &mut Context<Self>) -> Div {
        use crate::encode::{Encoder, Preset, Quality as Q};
        let p = self.vedit.vexport;
        let busy = self.export_job.is_some();
        let n = self.made_chapters(g).len();
        let progress = self.export_progress("vedit-exp-job", g, cx);
        let card = |preset: Preset, icon: mv_ui::Ph, sub: &'static str| {
            let on = p.preset == preset;
            div()
                .id(SharedString::from(format!("vedit-preset-{preset:?}")))
                .flex()
                .flex_col()
                .flex_1()
                .gap(px(4.))
                .p(px(space::SM))
                .rounded(px(radius::MD))
                .border_2()
                .border_color(if on { Tokens::accent() } else { Tokens::border() })
                .bg(if on { Tokens::accent_soft() } else { Tokens::surface_2() })
                .cursor_pointer()
                .hover(|d| d.border_color(Tokens::accent_border()))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.vedit.vexport.preset = preset;
                    cx.notify();
                }))
                .child(icon_ph(icon, if on { Tokens::accent() } else { Tokens::text_2() }, icon_px::SM))
                .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(preset.label()))
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(sub))
        };
        let quality = segmented("vedit-exp-q", [(None, "Draft"), (None, "Standard"), (None, "High")], p.quality as usize).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
            this.vedit.vexport.quality = [Q::Draft, Q::Standard, Q::High][(*ix).min(2)];
            cx.notify();
        }));
        let gpu_note = match crate::encode::known_encoders() {
            None => "Checking this PC's encoders…".to_string(),
            Some(e) => {
                let gpu: Vec<&str> = e.available.iter().filter(|x| x.is_gpu()).map(|x| match x {
                    Encoder::H264Nvenc | Encoder::HevcNvenc => "NVIDIA NVENC",
                    Encoder::H264Qsv => "Intel Quick Sync",
                    _ => "AMD AMF",
                }).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
                if gpu.is_empty() {
                    "No GPU encoder works here: x264 / x265 on the CPU".to_string()
                } else {
                    let pick = crate::encode::usable(p.gpu).pick(p.preset == Preset::Youtube4k).name();
                    format!("{} found · this export uses {pick}", gpu.join(", "))
                }
            }
        };
        let group = g.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(space::MD))
            .child(
                div()
                    .flex()
                    .gap(px(space::SM))
                    .child(card(Preset::Youtube1080, ph::YOUTUBE_LOGO, "1920x1080 · 16 Mb/s"))
                    .child(card(Preset::Youtube4k, ph::MONITOR, "3840x2160 · HEVC 10-bit when it can"))
                    .child(card(Preset::Shorts, ph::DEVICE_MOBILE, "1080x1920 reels of up to a minute"))
                    .child(card(Preset::Draft720, ph::LIGHTNING, "1280x720 · quick to check")),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(space::MD))
                    .child(div().flex().items_center().gap(px(6.)).child(muted("Quality")).child(quality))
                    .child(switch("vedit-exp-gpu", p.gpu).label("GPU encoding").on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.vedit.vexport.gpu = *on;
                        cx.notify();
                    })))
                    .child(switch("vedit-exp-loud", p.loudness).label("Loudness -14 LUFS").on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.vedit.vexport.loudness = *on;
                        cx.notify();
                    })))
                    .child(switch("vedit-exp-music", p.music).label("Music").on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.vedit.vexport.music = *on;
                        cx.notify();
                    }))),
            )
            .child(muted(gpu_note))
            .child(muted("Exports the rendered chapter videos (what Render made from this edit), joined in order, with the Look's watermark."))
            .child(
                div().flex().items_center().gap(px(space::SM)).child(div().flex_1().children(progress)).child(
                    btn("vedit-exp-go", "Export…", ButtonKind::Primary)
                        .icon(ph::EXPORT)
                        .when(n == 0 || busy, |b| b.disabled(if n == 0 { "Make a chapter video first" } else { "Busy with an export" }))
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            let p = this.vedit.vexport;
                            this.start_preset_export(&group, p, window, cx)
                        })),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{lane_h, soft, Rgba, Track};
    use crate::vedit::import_media;

    #[test]
    fn imported_media_is_copied_into_the_manga_folder() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("pick.png");
        std::fs::write(&src, b"png").unwrap();
        let manga = dir.path().join("manga");
        let out = import_media(&src, &manga).unwrap();
        assert_eq!(out, manga.join("media").join("pick.png"));
        assert_eq!(std::fs::read(&out).unwrap(), b"png");
        // Importing the kept copy again leaves it in place.
        assert_eq!(import_media(&out, &manga).unwrap(), out);
    }

    #[test]
    fn lanes_and_colours() {
        assert!(Track::ALL.iter().all(|t| lane_h(*t) >= 30.));
        let c = soft(Rgba { r: 1., g: 0., b: 0., a: 1. }, 0.3);
        assert!((c.a - 0.3).abs() < 1e-6 && c.r == 1.);
    }
}
