//! Studio's Video mode: a Premiere Pro-style editor for the current manga's finished recap.
//! Laid out like Premiere's Editing workspace: Effect Controls top left, the Program monitor in the
//! middle, the sequence's Captions / Music / Look on the right; the Project bin, the tools and the
//! timeline (V3 captions, V2 layers, V1 panels, A1 narration, A2 music) below.
//!
//! The monitor is a live preview of the edit (panels, layers and captions drawn here, the narration
//! and music played by `vedit_play`); Render makes the real video through the Python renderer (see
//! `vedit::render_extra`), and Export is the dashboard's Full recap export over those videos.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_recap::Step;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, empty_view, icon_button, icon_ph, interactive, muted, ph, segmented, size, switch, tip, ButtonKind, Tokens};

use crate::data::{ChapterItem, MangaGroup};
use crate::look::{Effect, Preset};
use crate::page::{plural, Load, StudioPage, View};
use crate::restyle::{place_index, set_place, set_size, PLACES, SIZES};
use crate::vedit::{self, Beat, ChapterEdit, Clip, Layer, LayerKind, LookEdit, Motion, MusicSource, Placed, Project, Track};
use crate::vedit_play;
use crate::video::{self, SubtitleSetting};

/// Track header width, and each lane's height.
const HEAD_W: f32 = 176.;
const RULER_H: f32 = 26.;
const LEFT_W: f32 = 320.;
const RIGHT_W: f32 = 300.;
const TOOLS_W: f32 = 40.;
const TICK: Duration = Duration::from_millis(33);
const UNDO_MAX: usize = 60;
const SWATCHES: [&str; 6] = ["#ffffff", "#ffe600", "#58d8ff", "#ff2fd2", "#ff9d2e", "#111111"];

fn lane_h(t: Track) -> f32 {
    match t {
        Track::Captions => 30.,
        Track::Layers => 34.,
        Track::Clips => 56.,
        Track::Narration | Track::Music => 40.,
    }
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
    /// A value bar in the controls.
    Bar(&'static str),
    /// The timeline's scroll bar.
    Scroll,
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
    undo: Vec<ChapterEdit>,
    redo: Vec<ChapterEdit>,
    /// Render keys of the chapters not open (worked out once; see `unrendered`) and the bin's
    /// first-page thumbnails.
    keys: RefCell<HashMap<String, u64>>,
    thumbs: RefCell<HashMap<String, Option<PathBuf>>>,
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
            pps: 24.,
            scroll_x: 0.,
            snap: true,
            snapped: None,
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
            undo: vec![],
            redo: vec![],
            keys: RefCell::new(HashMap::new()),
            thumbs: RefCell::new(HashMap::new()),
            _subs: vec![],
        }
    }
}

/// A chapter with a finished video.
fn made(statuses: &HashMap<String, mv_recap::PhaseStatus>, c: &ChapterItem) -> bool {
    statuses.get(&c.id).and_then(|s| s.video.as_ref()).is_some_and(|p| p.is_file())
}

/// A picked file copied into `<manga>/media/` (kept with the edit); the copy's path.
fn import_media(src: &Path, manga_dir: &Path) -> std::io::Result<PathBuf> {
    let dir = manga_dir.join("media");
    std::fs::create_dir_all(&dir)?;
    let name = src.file_name().ok_or_else(|| std::io::Error::other("no file name"))?;
    let dest = dir.join(name);
    if dest != src {
        std::fs::copy(src, &dest)?;
    }
    Ok(dest)
}

impl StudioPage {
    // ------------------------------------------------------------------------------ loading

    /// The manga the Video mode edits: the dashboard's, else the Editor's chapter's.
    fn video_manga(&self) -> Option<MangaGroup> {
        let Load::Ready(groups) = &self.library else { return None };
        let id = self.dash_manga.clone().or_else(|| self.focus.as_ref().map(|f| f.item.manga_id.clone()))?;
        groups.iter().find(|g| g.id == id).cloned()
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
        if let Some(prev) = &self.vedit.seq {
            self.vedit.keys.borrow_mut().remove(&prev.item.id);
        }
        let pages = mv_recap::list_pages(&item.pages_dir);
        let beats = vedit::load_beats(&item.output_dir, &pages);
        let edit = self.vedit.project.chapters.get(&item.id).cloned().unwrap_or_default();
        let subs = video::subtitle_setting_for(&item);
        let score = Some(mv_score::score_wav(&crate::page::chref(&item))).filter(|p| p.is_file());
        let reels = self.statuses.get(&item.id).is_some_and(|s| !s.reels.is_empty());
        self.vedit.seq = Some(Seq { item, beats, edit, subs, score, reels });
        self.vedit.playhead = 0;
        self.vedit.scroll_x = 0.;
        self.vedit.sel = None;
        self.vedit.undo.clear();
        self.vedit.redo.clear();
        self.video_fit();
    }

    /// Zoom so the whole sequence fits the timeline.
    fn video_fit(&mut self) {
        let total = self.vedit.seq.as_ref().map(|s| vedit::total_ms(&s.placed())).unwrap_or(0);
        let w = (f32::from(self.vedit.lanes.get().size.width)).max(400.);
        if total > 0 {
            self.vedit.pps = (w / (total as f32 / 1000.)).clamp(0.2, 400.);
            self.vedit.scroll_x = 0.;
        }
    }

    // ------------------------------------------------------------------------------ editing

    /// Applies `f` to the open chapter's edit, keeps an undo step and saves the project.
    fn video_edit(&mut self, f: impl FnOnce(&mut Seq), cx: &mut Context<Self>) {
        let Some(seq) = self.vedit.seq.as_mut() else { return };
        let before = seq.edit.clone();
        f(seq);
        if seq.edit != before {
            self.vedit.undo.push(before);
            if self.vedit.undo.len() > UNDO_MAX {
                self.vedit.undo.remove(0);
            }
            self.vedit.redo.clear();
            self.video_save(cx);
        }
        cx.notify();
    }

    /// Edits the clip list (with the beats it was made on).
    fn video_clips(&mut self, f: impl FnOnce(&[Beat], &mut Vec<Clip>), cx: &mut Context<Self>) {
        self.video_edit(
            |s| {
                let mut clips = s.edit.clips(&s.beats);
                f(&s.beats, &mut clips);
                s.edit.set_clips(&s.beats, clips);
            },
            cx,
        );
    }

    fn video_save(&mut self, cx: &mut Context<Self>) {
        let Some(seq) = &self.vedit.seq else { return };
        self.vedit.project.chapters.insert(seq.item.id.clone(), seq.edit.clone());
        if let Err(e) = self.vedit.project.save(&Self::manga_dir(&seq.item)) {
            toast::show(cx, Toast::error(format!("Could not save the edit: {e}")));
        }
    }

    fn video_undo(&mut self, redo: bool, cx: &mut Context<Self>) {
        let Some(seq) = self.vedit.seq.as_mut() else { return };
        let (from, to) = if redo { (&mut self.vedit.redo, &mut self.vedit.undo) } else { (&mut self.vedit.undo, &mut self.vedit.redo) };
        if let Some(e) = from.pop() {
            to.push(std::mem::replace(&mut seq.edit, e));
            self.vedit.sel = None;
            self.video_save(cx);
        }
        cx.notify();
    }

    /// Saves the caption setting to the chapter (the file the renderer and caption studio share).
    fn video_subs(&mut self, f: impl FnOnce(&mut SubtitleSetting), cx: &mut Context<Self>) {
        let Some(seq) = self.vedit.seq.as_mut() else { return };
        f(&mut seq.subs);
        video::save_chapter_setting(&seq.item.output_dir, Some(&seq.subs));
        cx.notify();
    }

    fn locked(&self, t: Track) -> bool {
        self.vedit.seq.as_ref().is_some_and(|s| s.edit.flags(t).locked)
    }

    fn video_delete_selection(&mut self, cx: &mut Context<Self>) {
        match self.vedit.sel {
            Some(Sel::Clip(i)) if !self.locked(Track::Clips) => {
                self.video_clips(|_, c| vedit::delete(c, i), cx);
                self.vedit.sel = None;
            }
            Some(Sel::Layer(id)) if !self.locked(Track::Layers) => {
                self.video_edit(|s| s.edit.layers.retain(|l| l.id != id), cx);
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
            Track::Layers => self.video_edit(|s| s.edit.layers.clear(), cx),
            Track::Music => self.video_edit(|s| s.edit.music.source = MusicSource::None, cx),
            Track::Captions | Track::Narration => self.video_edit(|s| s.edit.flags_mut(t).off = true, cx),
        }
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
        self.video_clips(|b, c| ok = vedit::split(b, c, p.index, ph_ms - p.start_ms), cx);
        if !ok {
            toast::show(cx, Toast::info("Too close to the clip's edge to cut there"));
        }
    }

    fn add_layer(&mut self, kind: LayerKind, cx: &mut Context<Self>) {
        let start = self.vedit.playhead;
        let mut id = 0;
        self.video_edit(
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
        self.video_edit(
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
            let _ = this.update(cx, |this, cx| match import_media(&src, &dir) {
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
            let _ = this.update(cx, |this, cx| match import_media(&src, &dir) {
                Ok(path) => {
                    this.video_edit(
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

    fn video_stop(&mut self) {
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
                    Some(vedit_play::Slice { file: b.audio.clone(), offset_ms: b.offset_ms + p.clip.trim_in_ms, dur_ms: p.dur_ms })
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
        vedit_play::play(vedit_play::Play { narration, skip_ms, music });
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

    /// Video mode shortcuts (Premiere's): Space play, V / C tools, Delete, Ctrl+K cut at the
    /// playhead, Ctrl+Z / Ctrl+Shift+Z, Home / End, Up / Down between edits, +/- zoom, \ fit.
    pub(crate) fn video_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.vedit.text.as_ref().is_some_and(|t| t.read(cx).focus_handle(cx).is_focused(window)) {
            return false;
        }
        let k = &ev.keystroke;
        let m = &k.modifiers;
        let cmd = m.control || m.platform;
        match k.key.as_str() {
            "escape" if self.vedit.export_open => self.vedit.export_open = false,
            "space" if !cmd => self.video_toggle_play(cx),
            "v" if !cmd => self.vedit.tool = Tool::Select,
            "c" if !cmd => self.vedit.tool = Tool::Razor,
            "k" if cmd => self.video_split_at_playhead(cx),
            "z" if cmd => self.video_undo(m.shift, cx),
            "y" if cmd => self.video_undo(true, cx),
            "delete" | "backspace" => self.video_delete_selection(cx),
            "home" => self.seek(0, cx),
            "end" => self.seek(u64::MAX, cx),
            "up" => self.step_edit(false, cx),
            "down" => self.step_edit(true, cx),
            "left" => self.seek(self.playhead_now().saturating_sub(1000 / vedit::FPS), cx),
            "right" => self.seek(self.playhead_now() + 1000 / vedit::FPS, cx),
            "=" | "+" => self.zoom(1.5, None),
            "-" => self.zoom(1. / 1.5, None),
            "\\" => self.video_fit(),
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Zooms the timeline by `k`, keeping `anchor_x` (lane pixels) or the playhead still.
    fn zoom(&mut self, k: f32, anchor_x: Option<f32>) {
        let e = &mut self.vedit;
        let old = e.pps;
        e.pps = (e.pps * k).clamp(0.2, 400.);
        let ax = anchor_x.unwrap_or(e.playhead as f32 / 1000. * old - e.scroll_x);
        let t = (e.scroll_x + ax) / old;
        e.scroll_x = (t * e.pps - ax).max(0.);
    }

    // ------------------------------------------------------------------------------ mouse

    /// Timeline time (ms) under window x.
    fn time_at(&self, x: Pixels) -> i64 {
        let b = self.vedit.lanes.get();
        (((f32::from(x) - f32::from(b.origin.x) + self.vedit.scroll_x) / self.vedit.pps) * 1000.) as i64
    }

    fn snap_ms(&mut self, t: i64, skip_layer: Option<u64>) -> i64 {
        self.vedit.snapped = None;
        let Some(seq) = &self.vedit.seq else { return t };
        if !self.vedit.snap {
            return t;
        }
        let pts = vedit::snap_points(&seq.placed(), &seq.edit.layers, self.vedit.playhead, skip_layer);
        let reach = (vedit::SNAP_PX / self.vedit.pps * 1000.) as u64;
        let (s, hit) = vedit::snap(t, &pts, reach);
        if hit {
            self.vedit.snapped = Some(s.max(0) as u64);
        }
        s
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
                let t = self.snap_ms(t, Some(id)).max(0) as u64;
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
            Drag::Bar(name) => {
                if let Some(f) = self.bar_fraction(name, pos.x) {
                    self.bar_set(name, f, cx);
                }
            }
            Drag::Scroll => {
                let b = self.vedit.lanes.get();
                let w = f32::from(b.size.width);
                let total = self.vedit.seq.as_ref().map(|s| vedit::total_ms(&s.placed())).unwrap_or(0) as f32 / 1000. * self.vedit.pps;
                if w > 0. && total > w {
                    let f = ((f32::from(pos.x) - f32::from(b.origin.x)) / w).clamp(0., 1.);
                    self.vedit.scroll_x = (f * total - w / 2.).clamp(0., total - w);
                }
            }
        }
        cx.notify();
    }

    /// The drag ends: drops a reordered clip and keeps one undo step for the whole drag.
    fn drag_end(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.vedit.drag.take() else { return };
        self.vedit.snapped = None;
        let saved = self.vedit.project.chapters.get(self.vedit.seq.as_ref().map(|s| s.item.id.as_str()).unwrap_or("")).cloned().unwrap_or_default();
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
                self.video_clips(|_, c| vedit::reorder(c, index, slot), cx);
                self.vedit.sel = Some(Sel::Clip(slot));
            }
            Drag::Trim { .. } | Drag::Layer { .. } | Drag::LayerEdge { .. } | Drag::Place { .. } => {
                // The drag edited in place: record it as one step against the saved edit.
                if let Some(seq) = &self.vedit.seq {
                    if seq.edit != saved {
                        self.vedit.undo.push(saved);
                        self.vedit.redo.clear();
                        self.video_save(cx);
                    }
                }
            }
            Drag::Bar(_) => {
                let Some(seq) = &self.vedit.seq else { return };
                video::save_chapter_setting(&seq.item.output_dir, Some(&seq.subs));
                if seq.edit != saved {
                    self.vedit.undo.push(saved);
                    self.video_save(cx);
                }
            }
            Drag::Playhead | Drag::Scroll => {}
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
        let pending = self.unrendered(&g);
        let top = div()
            .flex()
            .flex_1()
            .min_h(px(260.))
            .gap(px(4.))
            .child(self.panel("Effect Controls", None, self.effect_controls(cx)).w(px(LEFT_W)).flex_none())
            .child(self.monitor_panel(window, cx))
            .child(self.panel("Sequence", None, self.sequence_panel(cx)).w(px(RIGHT_W)).flex_none());
        let bottom = div()
            .flex()
            .flex_none()
            .h(px(330.))
            .gap(px(4.))
            .child(self.panel(&format!("Project: {}", g.title), None, self.bin(&g, &pending, cx)).w(px(LEFT_W)).flex_none())
            .child(self.tools(cx))
            .child(self.video_timeline(cx));
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
                this.video_stop();
                this.vedit.export_open = true;
                cx.notify();
            })))
            .child(div().flex_1().flex().justify_center().min_w_0().child(div().truncate().text_size(px(size::MD)).text_color(Tokens::text_1()).child(title)))
            .child(div().flex_none().text_size(px(size::SM)).text_color(if this_pending { Tokens::warning() } else { Tokens::text_3() }).child(status))
            .child(icon_button("vedit-undo", ph::ARROW_COUNTER_CLOCKWISE, "Undo (Ctrl+Z)").small().when(self.vedit.undo.is_empty(), |b| b.disabled("Nothing to undo")).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.video_undo(false, cx))))
            .child(icon_button("vedit-redo", ph::ARROW_CLOCKWISE, "Redo (Ctrl+Shift+Z)").small().when(self.vedit.redo.is_empty(), |b| b.disabled("Nothing to redo")).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.video_undo(true, cx))))
            .child(
                btn("vedit-reset", "Reset edit", ButtonKind::Ghost)
                    .small()
                    .tooltip("Back to the chapter as Generate made it (the caption style stays; Undo brings the edit back)")
                    .when(seq.is_none_or(|s| ChapterEdit { rendered: None, ..s.edit.clone() } == ChapterEdit::default()), |b| b.disabled("Nothing edited"))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.vedit.sel = None;
                        this.video_edit(|s| s.edit = ChapterEdit { rendered: s.edit.rendered, ..ChapterEdit::default() }, cx)
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
        for c in &chapters {
            let open = self.vedit.seq.as_ref().filter(|s| s.item.id == c.id);
            let key = self.render_key_of(c);
            let mut e = open.map(|s| s.edit.clone()).or_else(|| self.vedit.project.chapters.get(&c.id).cloned()).unwrap_or_default();
            e.rendered = Some(key);
            if let Some(s) = self.vedit.seq.as_mut().filter(|s| s.item.id == c.id) {
                s.edit.rendered = Some(key);
            }
            self.vedit.project.chapters.insert(c.id.clone(), e);
        }
        if let Some(c) = chapters.first() {
            if let Err(e) = self.vedit.project.save(&Self::manga_dir(c)) {
                toast::show(cx, Toast::error(format!("Could not save the edit: {e}")));
                return;
            }
        }
        self.sync_pipeline(cx);
        let (reels, land): (Vec<ChapterItem>, Vec<ChapterItem>) = chapters.into_iter().partition(|c| self.statuses.get(&c.id).is_some_and(|s| !s.reels.is_empty()));
        let mut n = 0;
        for (list, format) in [(land, mv_recap::FORMAT_LANDSCAPE), (reels, mv_recap::FORMAT_REELS)] {
            if !list.is_empty() {
                let (a, w, k) = crate::pipeline::enqueue(&list, Step::Video, Some(Step::Video), format, None, true);
                n += a + w + k;
            }
        }
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
        let room_h = (vh - 52. - 38. - 330. - 30. - 96. - 40.).max(140.);
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
        let image = cur.and_then(|p| seq.beats.get(p.clip.src)).and_then(|b| b.image.clone());
        let mut d = div().id("vedit-frame").relative().flex_none().w(px(fw)).h(px(fh)).overflow_hidden().bg(video::paint(hex("#000000"), 1.));
        if let Some(p) = image {
            d = d.child(img(p.clone()).absolute().inset_0().size_full().object_fit(ObjectFit::Cover).opacity(0.25)).child(img(p).absolute().inset_0().size_full().object_fit(ObjectFit::Contain));
        }
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
                }
            }
        }
        let mon = self.vedit.monitor.clone();
        d.child(canvas(move |b, _, _| mon.set(b), |_, _, _, _| {}).absolute().inset_0())
    }

    // ------------------------------------------------------------------------------ panels

    /// Top left: what the selection can change.
    fn effect_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(seq) = &self.vedit.seq else { return div().into_any_element() };
        let body = div().id("vedit-fx").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().gap(px(space::MD)).p(px(space::MD));
        let body = match self.vedit.sel {
            None => body.child(muted("Select a clip or layer in the timeline to change it.")).child(self.add_buttons(cx)),
            Some(Sel::Clip(i)) => {
                let clips = seq.edit.clips(&seq.beats);
                let Some(c) = clips.get(i).cloned() else { return div().into_any_element() };
                let b = seq.beats.get(c.src);
                let placed = seq.placed();
                let p = placed.iter().find(|p| p.index == i);
                let locked = seq.edit.flags(Track::Clips).locked;
                let motion = segmented("vedit-motion", Motion::ALL.iter().map(|m| (None, m.label())), Motion::ALL.iter().position(|m| *m == c.motion).unwrap_or(0)).small().on_change(cx.listener(move |this, ix: &usize, _, cx| {
                    let m = Motion::ALL[(*ix).min(3)];
                    this.video_clips(|_, cl| cl[i].motion = m, cx);
                }));
                let trims = |head: bool| {
                    let label = if head { "In" } else { "Out" };
                    let v = if head { c.trim_in_ms } else { c.trim_out_ms };
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::XS))
                        .child(div().w(px(36.)).text_size(px(size::SM)).text_color(Tokens::text_2()).child(label))
                        .child(icon_button(if head { "vedit-in-less" } else { "vedit-out-less" }, ph::MINUS, "0.1 s less trimmed").small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.video_clips(|b, cl| vedit::trim(b, cl, i, head, -100), cx))))
                        .child(div().w(px(70.)).text_size(px(size::SM)).font_family(crate::page::MONO).text_color(Tokens::text_1()).child(format!("{:.1} s", v as f32 / 1000.)))
                        .child(icon_button(if head { "vedit-in-more" } else { "vedit-out-more" }, ph::PLUS, "Trim 0.1 s more").small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.video_clips(|b, cl| vedit::trim(b, cl, i, head, 100), cx))))
                };
                body.child(self.prop_head(ph::IMAGE, &format!("Clip {} · page {}", i + 1, b.map(|b| b.page + 1).unwrap_or(0)), p.map(|p| format!("{} – {}", vedit::timecode(p.start_ms), vedit::timecode(p.end_ms()))).unwrap_or_default()))
                    .when(locked, |d| d.child(muted("The Panels track is locked.")))
                    .child(self.prop("Motion", motion.into_any_element()))
                    .child(self.prop("Trim", div().flex().flex_col().gap(px(4.)).child(trims(true)).child(trims(false)).into_any_element()))
                    .child(self.prop(
                        "Transition in",
                        switch("vedit-cut", c.cut)
                            .label("Hard cut instead of the look's transition")
                            .on_click(cx.listener(move |this, on: &bool, _, cx| {
                                let on = *on;
                                this.video_clips(|_, cl| cl[i].cut = on, cx)
                            }))
                            .into_any_element(),
                    ))
                    .child(self.prop("Caption text", self.text_field()))
                    .child(muted("The voice keeps the narration; only the caption changes."))
                    .child(
                        div()
                            .flex()
                            .gap(px(space::SM))
                            .child(btn("vedit-split", "Cut at playhead", ButtonKind::Secondary).small().icon(ph::SCISSORS).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.video_split_at_playhead(cx))))
                            .child(btn("vedit-del-clip", "Delete clip", ButtonKind::DangerGhost).small().icon(ph::TRASH).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.video_delete_selection(cx)))),
                    )
            }
            Some(Sel::Layer(id)) => {
                let Some(l) = seq.edit.layers.iter().find(|l| l.id == id).cloned() else { return div().into_any_element() };
                let is_text = matches!(l.kind, LayerKind::Text { .. });
                let (scale_f, scale_label) = match l.kind {
                    LayerKind::Image { .. } => ((l.scale - 0.02) / 0.98, format!("{}% of the width", (l.scale * 100.).round())),
                    LayerKind::Text { .. } => ((l.scale - 0.01) / 0.19, format!("{}% of the height", (l.scale * 1000.).round() / 10.)),
                };
                let set_time = |head: bool| {
                    let label = if head { "Start at playhead" } else { "End at playhead" };
                    btn(if head { "vedit-l-start" } else { "vedit-l-end" }, label, ButtonKind::Secondary).small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let t = this.playhead_now();
                        this.video_edit(
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
                let mut b = body
                    .child(self.prop_head(if is_text { ph::TEXT_T } else { ph::IMAGE }, &l.name(), format!("{} – {}", vedit::timecode(l.start_ms), vedit::timecode(l.end_ms))))
                    .child(self.bar("x", "Position X", l.x / 100., format!("{:.0}%", l.x), cx))
                    .child(self.bar("y", "Position Y", l.y / 100., format!("{:.0}%", l.y), cx))
                    .child(self.bar("scale", "Scale", scale_f, scale_label, cx))
                    .child(self.bar("opacity", "Opacity", l.opacity, format!("{:.0}%", l.opacity * 100.), cx))
                    .child(div().flex().flex_wrap().gap(px(space::SM)).child(set_time(true)).child(set_time(false)));
                if let LayerKind::Text { color, boxed, .. } = &l.kind {
                    let sw = SWATCHES.iter().map(|s| {
                        let s = *s;
                        let on = color.eq_ignore_ascii_case(s);
                        div()
                            .id(SharedString::from(format!("vedit-sw-{s}")))
                            .size(px(22.))
                            .rounded_full()
                            .bg(video::paint(hex(s), 1.))
                            .border_2()
                            .border_color(if on { Tokens::accent() } else { Tokens::border() })
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.video_edit(
                                    |sq| {
                                        if let Some(LayerKind::Text { color, .. }) = sq.edit.layers.iter_mut().find(|l| l.id == id).map(|l| &mut l.kind) {
                                            *color = s.to_string();
                                        }
                                    },
                                    cx,
                                )
                            }))
                    });
                    b = b
                        .child(self.prop("Text", self.text_field()))
                        .child(self.prop("Colour", div().flex().gap(px(6.)).children(sw).into_any_element()))
                        .child(
                            switch("vedit-box", *boxed)
                                .label("Dark box behind the text")
                                .on_click(cx.listener(move |this, on: &bool, _, cx| {
                                    let on = *on;
                                    this.video_edit(
                                        |sq| {
                                            if let Some(LayerKind::Text { boxed, .. }) = sq.edit.layers.iter_mut().find(|l| l.id == id).map(|l| &mut l.kind) {
                                                *boxed = on;
                                            }
                                        },
                                        cx,
                                    )
                                })),
                        );
                }
                b.child(muted("Drag the layer on the Program monitor to move it.")).child(btn("vedit-del-layer", "Delete layer", ButtonKind::DangerGhost).small().icon(ph::TRASH).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.video_delete_selection(cx))))
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
                    Track::Clips => "One clip per narrated panel. Drag to reorder, drag an edge to trim, or cut with the razor (C).",
                    Track::Narration => "The narration of every clip. Mute it to keep only the music.",
                    Track::Music => "The chapter's score, your own music, or none. Change it in Sequence → Music.",
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
                        this.video_clips(
                            |b, cl| {
                                if let Some(c) = cl.get_mut(i) {
                                    let own = b.get(c.src).map(|b| b.text.as_str()) == Some(value.as_str());
                                    c.text = (!own).then(|| value.clone());
                                }
                            },
                            cx,
                        );
                    }
                }
                Some(Sel::Layer(id)) => {
                    let same = this.vedit.seq.as_ref().and_then(|s| s.edit.layers.iter().find(|l| l.id == id)).is_some_and(|l| matches!(&l.kind, LayerKind::Text { text, .. } if *text == value));
                    if !same {
                        this.video_edit(
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
    fn sequence_panel(&self, cx: &mut Context<Self>) -> AnyElement {
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
                let styles = video::styles().iter().map(|st| {
                    let on = st.id == s.style;
                    let id = st.id.clone();
                    div()
                        .id(SharedString::from(format!("vedit-style-{}", st.id)))
                        .px(px(space::SM))
                        .py(px(6.))
                        .rounded(px(radius::SM))
                        .text_size(px(size::SM))
                        .cursor_pointer()
                        .border_1()
                        .border_color(if on { Tokens::accent() } else { Tokens::border() })
                        .bg(if on { Tokens::accent_soft() } else { Tokens::surface_2() })
                        .text_color(Tokens::text_1())
                        .hover(|d| d.bg(Tokens::hover()))
                        .child(st.label.clone())
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            let id = id.clone();
                            this.video_subs(|s| s.style = id, cx)
                        }))
                });
                let place = segmented("vedit-place", PLACES.iter().map(|p| (None, p.0)), place_index(s.pos)).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
                    let ix = *ix;
                    this.video_subs(|s| set_place(s, ix), cx)
                }));
                let sizes = segmented("vedit-size", SIZES.iter().map(|p| (None, p.0)), crate::restyle::size_index(s.size)).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
                    let v = SIZES[(*ix).min(SIZES.len() - 1)].1;
                    this.video_subs(|s| set_size(s, v), cx)
                }));
                let y = s.pos.map(|p| p.1).unwrap_or_else(|| s.effective_style().pos.1);
                body.child(
                    switch("vedit-caps-on", s.enabled && !off).label("Captions on").on_click(cx.listener(|this, on: &bool, _, cx| {
                        let on = *on;
                        this.video_edit(|sq| sq.edit.flags_mut(Track::Captions).off = !on, cx);
                        if on {
                            this.video_subs(|s| s.enabled = true, cx);
                        }
                    })),
                )
                .child(self.prop("Style", div().flex().flex_wrap().gap(px(6.)).children(styles).into_any_element()))
                .child(self.prop("Place", place.into_any_element()))
                .child(self.bar("caption-y", "Height in the frame", y / 100., format!("{y:.0}%"), cx))
                .child(self.prop("Size", sizes.into_any_element()))
                .child(
                    switch("vedit-caps-emph", s.emphasis).label("Highlight the spoken word").on_click(cx.listener(|this, on: &bool, _, cx| {
                        let on = *on;
                        this.video_subs(|s| s.emphasis = on, cx)
                    })),
                )
                .child(muted("Saved as this chapter's caption style (the caption studio in the Editor shows the same)."))
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
                    0 => this.video_edit(
                        |s| {
                            s.edit.music.source = MusicSource::Score;
                            s.edit.flags_mut(Track::Music).off = false;
                        },
                        cx,
                    ),
                    1 => this.import_music(cx),
                    _ => this.video_edit(|s| s.edit.music.source = MusicSource::None, cx),
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
                            this.video_edit(|s| s.edit.flags_mut(Track::Music).off = on, cx)
                        })),
                    )
                    .child(
                        switch("vedit-narr-mute", seq.edit.flags(Track::Narration).off).label("Mute the narration").on_click(cx.listener(|this, on: &bool, _, cx| {
                            let on = *on;
                            this.video_edit(|s| s.edit.flags_mut(Track::Narration).off = on, cx)
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
                    this.video_edit(|s| s.edit.look = own.then(|| LookEdit { preset: crate::look::Look::load().preset.key().into(), effects: Default::default() }), cx)
                }));
                let presets = segmented("vedit-preset", Preset::ALL.iter().map(|p| (None, p.label())), Preset::ALL.iter().position(|p| *p == preset).unwrap_or(3)).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
                    let p = Preset::ALL[(*ix).min(3)];
                    this.video_edit(|s| s.edit.look = Some(LookEdit { preset: p.key().into(), effects: Default::default() }), cx)
                }));
                let switches = Effect::ALL.iter().enumerate().map(|(k, e)| {
                    let e = *e;
                    switch(SharedString::from(format!("vedit-fx-{}", e.key())), fx[k]).label(e.label()).on_click(cx.listener(move |this, on: &bool, _, cx| {
                        let on = *on;
                        this.video_edit(
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
                    this.video_edit(|s| s.edit.cuts = on, cx)
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
                    .hover(|d| d.bg(Tokens::hover()))
                    .cursor_pointer()
                    .tooltip(tip(hint))
                    .child(icon_ph(icon, Tokens::text_2(), icon_px::SM))
                    .child(label)
            };
            let motion = |id: &'static str, m: Motion, hint: &'static str| {
                item(id, ph::ARROWS_OUT, m.label(), hint).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| match this.vedit.sel {
                    Some(Sel::Clip(i)) => this.video_clips(|_, c| c[i].motion = m, cx),
                    _ => toast::show(cx, Toast::info("Select a clip first")),
                }))
            };
            let look = |id: &'static str, p: Preset| {
                item(id, ph::PALETTE, p.label(), p.blurb()).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.video_edit(|s| s.edit.look = Some(LookEdit { preset: p.key().into(), effects: Default::default() }), cx);
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
                .child(item("vedit-fx-cuts", ph::SCISSORS, "Hard cuts", "Cut between every clip").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.video_edit(|s| s.edit.cuts = true, cx))))
                .child(item("vedit-fx-trans", ph::SPARKLE, "Look's transitions", "Wipes, slides and fades from the look").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.video_edit(|s| s.edit.cuts = false, cx))))
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
                .hover(|d| d.bg(Tokens::hover()))
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
            .child(div().w(px(space::MD)))
            .child(icon_button("vedit-snap", ph::LINK_SIMPLE, if self.vedit.snap { "Snap in timeline: on (S)" } else { "Snap in timeline: off (S)" }).small().selected(self.vedit.snap).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.vedit.snap = !this.vedit.snap;
                cx.notify();
            })))
            .child(icon_button("vedit-zoom-out", ph::MAGNIFYING_GLASS_MINUS, "Zoom out (-)").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.zoom(1. / 1.5, None);
                cx.notify();
            })))
            .child(icon_button("vedit-zoom-in", ph::MAGNIFYING_GLASS, "Zoom in (=)").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.zoom(1.5, None);
                cx.notify();
            })))
            .child(icon_button("vedit-zoom-fit", ph::ARROWS_IN_SIMPLE, "Zoom to the whole sequence (\\)").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.video_fit();
                cx.notify();
            })))
            .child(div().flex_1())
            .child(muted(format!("{} clips · {}", placed.len(), vedit::timecode(total))))
            .when(seq.edit.stale(&seq.beats), |d| d.child(div().text_size(px(size::SM)).text_color(Tokens::warning()).child("Re-narrated: clip edits reset")));

        // Ruler.
        let (major, minor) = vedit::ruler_step(pps);
        let mut ruler = div()
            .id("vedit-ruler")
            .relative()
            .h(px(RULER_H))
            .overflow_hidden()
            .border_b_1()
            .border_color(Tokens::border())
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                this.vedit.drag = Some(Drag::Playhead);
                this.drag_move(ev.position, cx);
            }));
        let step_ms = (major * 1000.) as u64;
        let mut t = t0 / step_ms * step_ms;
        while t <= t1.min(total + step_ms) {
            let x = xof(t);
            ruler = ruler.child(div().absolute().left(px(x)).bottom_0().w(px(1.)).h(px(10.)).bg(Tokens::text_3())).child(div().absolute().left(px(x + 3.)).top(px(2.)).text_size(px(size::XS)).font_family(crate::page::MONO).text_color(Tokens::text_3()).child(vedit::timecode(t)));
            for k in 1..minor {
                let mx = xof(t + step_ms * k as u64 / minor as u64);
                ruler = ruler.child(div().absolute().left(px(mx)).bottom_0().w(px(1.)).h(px(4.)).bg(Tokens::border_strong()));
            }
            t += step_ms;
        }
        ruler = ruler.child(div().absolute().left(px(xof(now) - 5.)).top(px(RULER_H - 12.)).w(px(11.)).h(px(12.)).rounded(px(2.)).bg(Tokens::accent()));

        // Lanes.
        let sel = self.vedit.sel;
        let drag = self.vedit.drag;
        let razor = self.vedit.tool == Tool::Razor;
        let visible = |p: &&Placed| p.end_ms() >= t0 && p.start_ms <= t1;
        let mut lanes: Vec<AnyElement> = vec![];
        for t in Track::ALL {
            let h = lane_h(t);
            let flags = seq.edit.flags(t);
            let mut lane = div().id(SharedString::from(format!("vedit-lane-{}", t.code()))).relative().h(px(h)).overflow_hidden().border_b_1().border_color(Tokens::border()).when(flags.off, |d| d.opacity(0.4));
            let block = |id: SharedString, x: f32, w: f32, fill: Rgba, edge: Rgba, selected: bool| {
                div()
                    .id(id)
                    .absolute()
                    .left(px(x))
                    .top(px(2.))
                    .w(px(w.max(2.)))
                    .h(px(h - 4.))
                    .rounded(px(3.))
                    .overflow_hidden()
                    .bg(fill)
                    .border_1()
                    .border_color(if selected { Tokens::text_1() } else { edge })
                    .when(selected, |d| d.border_2())
            };
            match t {
                Track::Clips | Track::Narration | Track::Captions => {
                    for p in placed.iter().filter(visible) {
                        let (x, w) = (xof(p.start_ms), p.dur_ms as f32 / 1000. * pps);
                        let index = p.index;
                        let beat = seq.beats.get(p.clip.src);
                        let selected = sel == Some(Sel::Clip(index));
                        let dragging = matches!(drag, Some(Drag::Clip { index: i, .. }) if i == index);
                        let (fill, edge) = match t {
                            Track::Clips => (soft(Tokens::info(), 0.32), Tokens::info()),
                            Track::Narration => (soft(Tokens::success(), 0.30), Tokens::success()),
                            _ => (soft(Tokens::accent(), 0.28), Tokens::accent()),
                        };
                        let label = match t {
                            Track::Clips => format!("p{} · {}", beat.map(|b| b.page + 1).unwrap_or(0), if p.clip.motion == Motion::Auto { String::new() } else { p.clip.motion.label().to_string() }),
                            Track::Narration => beat.and_then(|b| b.audio.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                            _ => p.clip.text.clone().or_else(|| beat.map(|b| b.text.clone())).unwrap_or_default(),
                        };
                        let mut el = block(SharedString::from(format!("vedit-{}-{index}", t.code())), x, w, fill, edge, selected).when(dragging, |d| d.opacity(0.35));
                        if t == Track::Clips && w > 44. {
                            if let Some(img_p) = beat.and_then(|b| b.image.clone()) {
                                el = el.child(img(img_p).absolute().left_0().top_0().h_full().w(px((h - 4.) * 0.75)).object_fit(ObjectFit::Cover));
                            }
                        }
                        if w > 26. {
                            let pad = if t == Track::Clips && w > 44. { (h - 4.) * 0.75 + 4. } else { 4. };
                            el = el.child(div().absolute().left(px(pad)).top(px(2.)).right(px(2.)).truncate().text_size(px(size::XS)).text_color(Tokens::text_1()).child(label));
                        }
                        if p.clip.cut && t == Track::Clips {
                            el = el.child(div().absolute().left_0().top_0().bottom_0().w(px(3.)).bg(Tokens::warning()));
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
                                        .w(px(6.))
                                        .when(head, |d| d.left_0())
                                        .when(!head, |d| d.right_0())
                                        .cursor(CursorStyle::ResizeLeftRight)
                                        .hover(|d| d.bg(soft(Tokens::text_1(), 0.35)))
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
                            let at = this.time_at(ev.position.x);
                            if this.vedit.tool == Tool::Razor {
                                if this.locked(Track::Clips) {
                                    return;
                                }
                                let cut = this.snap_ms(at, None);
                                let into = (cut - start as i64).clamp(0, dur as i64) as u64;
                                let mut ok = false;
                                this.video_clips(|b, c| ok = vedit::split(b, c, index, into), cx);
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
                        let mut el = block(SharedString::from(format!("vedit-layer-{id}")), x, w, soft(Tokens::accent_2(), 0.35), Tokens::accent_2(), selected)
                            .child(div().absolute().left(px(4.)).top(px(2.)).right(px(4.)).flex().items_center().gap(px(4.)).child(icon_ph(icon, Tokens::text_1(), icon_px::XS)).child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_1()).child(l.name())));
                        if !flags.locked && w > 14. {
                            for head in [true, false] {
                                el = el.child(
                                    div()
                                        .id(SharedString::from(format!("vedit-ledge-{id}-{head}")))
                                        .absolute()
                                        .top_0()
                                        .bottom_0()
                                        .w(px(6.))
                                        .when(head, |d| d.left_0())
                                        .when(!head, |d| d.right_0())
                                        .cursor(CursorStyle::ResizeLeftRight)
                                        .hover(|d| d.bg(soft(Tokens::text_1(), 0.35)))
                                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                            cx.stop_propagation();
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
                            this.vedit.sel = Some(Sel::Layer(id));
                            if !this.locked(Track::Layers) {
                                this.vedit.drag = Some(Drag::Layer { id, grab: this.time_at(ev.position.x) - start as i64 });
                            }
                            cx.notify();
                        })));
                    }
                }
                Track::Music => {
                    let name = match &seq.edit.music.source {
                        MusicSource::Score => seq.score.as_ref().map(|_| "score.wav".to_string()),
                        MusicSource::File { path } => path.file_name().map(|n| n.to_string_lossy().into_owned()),
                        MusicSource::None => None,
                    };
                    if let Some(name) = name {
                        let selected = sel == Some(Sel::Track(Track::Music));
                        lane = lane.child(
                            block("vedit-music-clip".into(), xof(0), total as f32 / 1000. * pps, soft(Tokens::success(), 0.22), Tokens::success(), selected)
                                .child(div().absolute().left(px(4.).max(px(-xof(0) + 4.))).top(px(2.)).flex().items_center().gap(px(4.)).child(icon_ph(ph::MUSIC_NOTES, Tokens::text_1(), icon_px::XS)).child(div().text_size(px(size::XS)).text_color(Tokens::text_1()).child(format!("{name} · {:.0}%", seq.edit.music.volume * 100.))))
                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.vedit.sel = Some(Sel::Track(Track::Music));
                                    this.vedit.side_tab = 1;
                                    cx.notify();
                                })),
                        );
                    }
                }
            }
            lanes.push(lane.into_any_element());
        }
        // Where a dragged clip would land, the snap line and the playhead, over every lane.
        let lanes_h: f32 = Track::ALL.iter().map(|t| lane_h(*t)).sum();
        let mut over = div().absolute().top_0().left_0().right_0().h(px(lanes_h));
        if let Some(Drag::Clip { index, grab, at }) = drag {
            if let Some(me) = placed.iter().find(|p| p.index == index) {
                let x = (at - grab) as f32 / 1000. * pps - sx;
                let top: f32 = lane_h(Track::Captions) + lane_h(Track::Layers);
                over = over.child(div().absolute().left(px(x)).top(px(top + 2.)).w(px(me.dur_ms as f32 / 1000. * pps)).h(px(lane_h(Track::Clips) - 4.)).rounded(px(3.)).border_2().border_color(Tokens::text_1()).bg(soft(Tokens::info(), 0.5)));
            }
        }
        if let Some(s) = self.vedit.snapped {
            over = over.child(div().absolute().left(px(xof(s))).top_0().bottom_0().w(px(1.)).bg(Tokens::warning()));
        }
        over = over.child(div().absolute().left(px(xof(now))).top_0().bottom_0().w(px(1.)).bg(Tokens::accent()));

        let lanes_box = self.vedit.lanes.clone();
        let content = div()
            .id("vedit-lanes")
            .relative()
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
                this.drag_move(ev.position, cx);
            }))
            .on_scroll_wheel(cx.listener(move |this, ev: &ScrollWheelEvent, _, cx| {
                let d = ev.delta.pixel_delta(px(20.));
                let (dx, dy) = (f32::from(d.x), f32::from(d.y));
                if ev.modifiers.control || ev.modifiers.alt {
                    let ax = f32::from(ev.position.x) - f32::from(this.vedit.lanes.get().origin.x);
                    this.zoom(if dy > 0. { 1.2 } else { 1. / 1.2 }, Some(ax));
                } else {
                    let total = this.vedit.seq.as_ref().map(|s| vedit::total_ms(&s.placed())).unwrap_or(0) as f32 / 1000. * this.vedit.pps;
                    let w = f32::from(this.vedit.lanes.get().size.width);
                    this.vedit.scroll_x = (this.vedit.scroll_x - if dx.abs() > dy.abs() { dx } else { dy }).clamp(0., (total - w + 40.).max(0.));
                }
                cx.notify();
            }));

        // Track headers.
        let mut heads = div().flex().flex_col().flex_none().w(px(HEAD_W)).border_r_1().border_color(Tokens::border()).child(div().h(px(RULER_H)).border_b_1().border_color(Tokens::border()));
        for t in Track::ALL {
            let flags = seq.edit.flags(t);
            let selected = sel == Some(Sel::Track(t));
            let code = t.code();
            heads = heads.child(
                div()
                    .id(SharedString::from(format!("vedit-head-{code}")))
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .h(px(lane_h(t)))
                    .px(px(4.))
                    .border_b_1()
                    .border_color(Tokens::border())
                    .when(selected, |d| d.bg(Tokens::active()))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.vedit.sel = Some(Sel::Track(t));
                        if t == Track::Captions {
                            this.vedit.side_tab = 0;
                        } else if t.audio() {
                            this.vedit.side_tab = 1;
                        }
                        cx.notify();
                    }))
                    .child(div().flex_none().w(px(26.)).h(px(20.)).rounded(px(3.)).flex().items_center().justify_center().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).bg(if t.audio() { soft(Tokens::success(), 0.5) } else { soft(Tokens::info(), 0.5) }).text_color(Tokens::text_1()).child(code))
                    .child(icon_button(SharedString::from(format!("vedit-lock-{code}")), if flags.locked { ph::LOCK } else { ph::LOCK_OPEN }, if flags.locked { "Unlock the track" } else { "Lock the track" }).small().selected(flags.locked).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Some(s) = this.vedit.seq.as_mut() {
                            let f = s.edit.flags_mut(t);
                            f.locked = !f.locked;
                        }
                        this.video_save(cx);
                        cx.notify();
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
                                |s| {
                                    let f = s.edit.flags_mut(t);
                                    f.off = !f.off;
                                },
                                cx,
                            );
                        })))
                    })
                    .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).text_color(Tokens::text_2()).child(t.label())),
            );
        }

        // Horizontal scroll bar (drag it like Premiere's zoom bar).
        let total_px = (total as f32 / 1000. * pps).max(1.);
        let (bar_l, bar_w) = ((sx / total_px).clamp(0., 1.), (view_w / total_px).clamp(0.02, 1.));
        let scroll = div()
            .id("vedit-hscroll")
            .relative()
            .flex_none()
            .h(px(12.))
            .ml(px(HEAD_W))
            .cursor_pointer()
            .child(div().absolute().top(px(3.)).h(px(6.)).left(relative(bar_l)).w(relative(bar_w)).rounded_full().bg(Tokens::border_strong()))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                this.vedit.drag = Some(Drag::Scroll);
                this.drag_move(ev.position, cx);
            }));

        let body = div().flex().flex_col().flex_1().min_h_0().child(toolbar).child(div().flex().flex_1().min_h_0().child(heads).child(content)).child(scroll);
        self.panel(&format!("{} ×", seq.item.label()), None, body).flex_1()
    }

    // ------------------------------------------------------------------------------ export

    /// Premiere's Export: the dashboard's Full recap export (whole, parts, reels, hook,
    /// watermark) over the rendered chapter videos.
    fn export_dialog(&mut self, g: &MangaGroup, pending: Vec<ChapterItem>, cx: &mut Context<Self>) -> Stateful<Div> {
        let n = pending.len();
        let rendering = self.made_chapters(g).iter().any(|c| self.queue.for_chapter(&c.id).is_some_and(|q| q.state.active()));
        let section = self.video_section(g, cx);
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
                    .child(section),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{import_media, lane_h, soft, Rgba, Track};

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
