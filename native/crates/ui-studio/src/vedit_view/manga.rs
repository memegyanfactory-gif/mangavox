//! Video mode's whole-manga timeline ("All chapters"): every made chapter of the manga end to
//! end on one timeline, scrubbed and played across chapters, with a chapter track that shows
//! which videos are on an old look or being re-made, and "Re-edit full timeline".
//!
//! The chapter under the playhead is the open sequence (`Editor::seq`), so the monitor, Effect
//! Controls and the Sequence panel keep working on it; settings that belong to the whole manga
//! (captions, look, transitions, music) are carried to every chapter (`manga_sync`). The pure
//! parts are in `vedit_manga`. A child module of `vedit_view`, so it shares the editor's state.
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, icon_button, icon_ph, muted, ph, size, tip, ButtonKind, Tokens};

use super::{lane_h, made, soft, Drag, Sel, Seq, HEAD_W, LEFT_W, RIGHT_W, RULER_H, TICK};
use crate::data::{ChapterItem, MangaGroup};
use crate::look::{Look, Preset};
use crate::page::{plural, StudioPage, View};
use crate::pipeline::{short_label, ItemState};
use crate::vedit::{self, Beat, ChapterEdit, LayerKind, Motion, Placed, Project, Track};
use crate::vedit_manga::{self as vm, LookState, Span};
use crate::vedit_play;
use crate::video::{self, SubtitleSetting};

/// The chapter track's height.
const CH_H: f32 = 36.;

/// One chapter on the manga timeline.
pub(crate) struct MChapter {
    pub item: ChapterItem,
    pub beats: Vec<Beat>,
    pub edit: ChapterEdit,
    pub subs: SubtitleSetting,
    pub score: Option<PathBuf>,
    pub reels: bool,
    pub placed: Vec<Placed>,
}

impl MChapter {
    fn total(&self) -> u64 {
        vedit::total_ms(&self.placed)
    }
}

/// The open whole-manga timeline.
pub(crate) struct MangaSeq {
    pub chapters: Vec<MChapter>,
    pub spans: Vec<Span>,
    /// The chapter open as `Editor::seq` (under the playhead).
    pub focus: usize,
    /// Manga time.
    pub playhead: u64,
    pub playing: Option<(Instant, u64)>,
}

impl MangaSeq {
    fn relayout(&mut self) {
        self.spans = vm::layout(&self.chapters.iter().map(MChapter::total).collect::<Vec<_>>());
    }

    fn total(&self) -> u64 {
        vm::total(&self.spans)
    }
}

/// The whole-manga state the Video mode keeps (one per Studio page).
#[derive(Default)]
pub(crate) struct Whole {
    /// Some while "All chapters" is open.
    pub all: Option<MangaSeq>,
    /// The "Re-edit full timeline" confirmation is open.
    pub confirm: bool,
    /// Each made chapter's look state (worked out once; dropped when something changed).
    looks: RefCell<HashMap<String, LookState>>,
    /// Chapters of the manga still being re-made at the last poll.
    remaking: Vec<String>,
    seen_tools: u64,
    look_saved: Option<SystemTime>,
    /// The Studio look, re-read at most once a second (the monitor draws it every frame).
    look: RefCell<Option<(Instant, Look)>>,
}

/// Where a chapter's re-make stands in the queue.
pub(crate) struct Remake {
    pub label: String,
    pub progress: Option<f32>,
    pub failed: bool,
}

impl StudioPage {
    // ------------------------------------------------------------------------------ state

    /// The Studio look (cached for a second).
    fn studio_look(&self) -> Look {
        let mut c = self.vedit.whole.look.borrow_mut();
        match c.as_ref() {
            Some((t, l)) if t.elapsed().as_secs() < 1 => l.clone(),
            _ => {
                let l = Look::load();
                *c = Some((Instant::now(), l.clone()));
                l
            }
        }
    }

    fn mchapter(&self, c: &ChapterItem) -> MChapter {
        if let Some(s) = self.vedit.seq.as_ref().filter(|s| s.item.id == c.id) {
            let placed = s.placed();
            return MChapter { item: s.item.clone(), beats: s.beats.clone(), edit: s.edit.clone(), subs: s.subs.clone(), score: s.score.clone(), reels: s.reels, placed };
        }
        let pages = mv_recap::list_pages(&c.pages_dir);
        let beats = vedit::load_beats(&c.output_dir, &pages);
        let edit = self.vedit.project.chapters.get(&c.id).cloned().unwrap_or_default();
        let placed = vedit::place(&beats, &edit.clips(&beats));
        MChapter {
            item: c.clone(),
            subs: video::subtitle_setting_for(c),
            score: Some(mv_score::score_wav(&crate::page::chref(c))).filter(|p| p.is_file()),
            reels: self.statuses.get(&c.id).is_some_and(|s| !s.reels.is_empty()),
            beats,
            edit,
            placed,
        }
    }

    /// Opens "All chapters": the made chapters end to end, the playhead where it was.
    pub(super) fn manga_enter(&mut self, cx: &mut Context<Self>) {
        let Some(g) = self.video_manga() else { return };
        self.video_stop();
        let chapters: Vec<MChapter> = self.made_chapters(&g).iter().map(|c| self.mchapter(c)).collect();
        if chapters.is_empty() {
            return;
        }
        let focus = self.vedit.seq.as_ref().and_then(|s| chapters.iter().position(|c| c.item.id == s.item.id)).unwrap_or(0);
        let mut m = MangaSeq { chapters, spans: vec![], focus, playhead: 0, playing: None };
        m.relayout();
        m.playhead = m.spans[focus].start_ms + self.vedit.playhead.min(m.spans[focus].dur_ms);
        if self.vedit.seq.as_ref().is_none_or(|s| s.item.id != m.chapters[focus].item.id) {
            self.vedit.seq = Some(Self::seq_of(&m.chapters[focus]));
        }
        self.vedit.whole.all = Some(m);
        self.vedit.whole.seen_tools = crate::vedit_tools::version();
        self.vedit.whole.looks.borrow_mut().clear();
        self.vedit.sel = None;
        self.vedit.fit_pending = true;
        cx.notify();
    }

    /// Back to one chapter's own timeline (`to`, else the one under the playhead).
    pub(super) fn manga_leave(&mut self, to: Option<usize>, cx: &mut Context<Self>) {
        self.manga_stop();
        let Some(m) = self.vedit.whole.all.take() else { return };
        let k = to.unwrap_or(m.focus).min(m.chapters.len().saturating_sub(1));
        let Some(c) = m.chapters.get(k) else { return };
        let local = if k == m.focus { self.vedit.playhead } else { 0 };
        self.video_open(c.item.clone());
        self.vedit.playhead = local;
        cx.notify();
    }

    /// Leaves "All chapters" without opening anything (another manga was picked).
    pub(super) fn manga_reset(&mut self) {
        self.manga_stop();
        let w = &mut self.vedit.whole;
        w.all = None;
        w.confirm = false;
        w.remaking.clear();
        w.looks.borrow_mut().clear();
    }

    fn seq_of(c: &MChapter) -> Seq {
        Seq { item: c.item.clone(), beats: c.beats.clone(), edit: c.edit.clone(), subs: c.subs.clone(), score: c.score.clone(), reels: c.reels }
    }

    /// Makes chapter `k` the open sequence (as the playhead crosses into it).
    fn manga_focus(&mut self, k: usize) {
        let Some(m) = self.vedit.whole.all.as_mut() else { return };
        if k == m.focus || k >= m.chapters.len() {
            return;
        }
        m.focus = k;
        let c = &m.chapters[k];
        crate::vedit_tools::set_current(&c.item.manga_id, &c.item.id);
        self.vedit.seq = Some(Self::seq_of(c));
        self.vedit.sel = None;
        self.vedit.undo.clear();
        self.vedit.redo.clear();
    }

    /// Moves the manga playhead (and the open chapter with it).
    fn manga_set_playhead(&mut self, t: u64) {
        let Some(m) = self.vedit.whole.all.as_mut() else { return };
        let t = t.min(m.total());
        m.playhead = t;
        if let Some((k, local)) = vm::locate(&m.spans, t) {
            self.manga_focus(k);
            self.vedit.playhead = local;
        }
    }

    /// The manga playhead now (moving while it plays).
    pub(super) fn manga_now(&self) -> u64 {
        let Some(m) = self.vedit.whole.all.as_ref() else { return 0 };
        match m.playing {
            Some((t0, from)) => (from + t0.elapsed().as_millis() as u64).min(m.total()),
            None => m.playhead,
        }
    }

    /// The timeline's length (whichever timeline is open).
    pub(super) fn timeline_ms(&self) -> u64 {
        match &self.vedit.whole.all {
            Some(m) => m.total(),
            None => self.vedit.seq.as_ref().map(|s| vedit::total_ms(&s.placed())).unwrap_or(0),
        }
    }

    pub(super) fn manga_stop(&mut self) {
        let Some(m) = self.vedit.whole.all.as_mut() else { return };
        if let Some((t0, from)) = m.playing.take() {
            vedit_play::stop();
            let t = from + t0.elapsed().as_millis() as u64;
            self.manga_set_playhead(t);
        }
    }

    pub(super) fn manga_seek(&mut self, ms: u64, cx: &mut Context<Self>) {
        let playing = self.vedit.whole.all.as_ref().is_some_and(|m| m.playing.is_some());
        self.manga_stop();
        self.manga_set_playhead(ms);
        if playing {
            self.manga_toggle_play(cx);
        }
        cx.notify();
    }

    /// Snaps manga time to the chapter starts and the playhead.
    pub(super) fn manga_snap(&mut self, t: i64) -> i64 {
        let Some(m) = self.vedit.whole.all.as_ref() else { return t };
        let mut pts = vm::edges(&m.spans);
        pts.push(m.playhead);
        let reach = (vedit::SNAP_PX / self.vedit.pps * 1000.) as u64;
        let (s, hit) = vedit::snap(t, &pts, reach);
        if hit {
            self.vedit.snapped = Some(s.max(0) as u64);
        }
        s
    }

    pub(super) fn manga_fit(&mut self) {
        let total = self.timeline_ms();
        let w = f32::from(self.vedit.lanes.get().size.width).max(400.);
        if total > 0 {
            self.vedit.pps = (w / (total as f32 / 1000.)).clamp(0.02, 400.);
            self.vedit.scroll_x = 0.;
        }
    }

    fn manga_zoom(&mut self, k: f32, anchor_x: Option<f32>) {
        let now = self.manga_now();
        let e = &mut self.vedit;
        let old = e.pps;
        e.pps = (e.pps * k).clamp(0.02, 400.);
        let ax = anchor_x.unwrap_or(now as f32 / 1000. * old - e.scroll_x);
        let t = (e.scroll_x + ax) / old;
        e.scroll_x = (t * e.pps - ax).max(0.);
    }

    fn manga_follow(&mut self) {
        let w = f32::from(self.vedit.lanes.get().size.width);
        let x = self.manga_now() as f32 / 1000. * self.vedit.pps;
        if w > 0. && (x < self.vedit.scroll_x || x > self.vedit.scroll_x + w - 20.) {
            self.vedit.scroll_x = (x - 40.).max(0.);
        }
    }

    /// Plays from the manga playhead across chapters (each chapter's narration and music).
    pub(super) fn manga_toggle_play(&mut self, cx: &mut Context<Self>) {
        if self.vedit.whole.all.as_ref().is_some_and(|m| m.playing.is_some()) {
            self.manga_stop();
            cx.notify();
            return;
        }
        // The open chapter's latest edit plays.
        self.manga_absorb();
        let Some(m) = self.vedit.whole.all.as_mut() else { return };
        let total = m.total();
        if total == 0 {
            return;
        }
        if m.playhead + 50 >= total {
            m.playhead = 0;
        }
        let from = m.playhead;
        let sound: Vec<vm::ChapterSound> = m.chapters.iter().map(|c| vm::ChapterSound { beats: &c.beats, placed: &c.placed, edit: &c.edit, score: c.score.as_deref() }).collect();
        let (narration, skip_ms, music_parts) = vm::sound_from(&sound, &m.spans, from);
        vedit_play::play(vedit_play::Play { narration, skip_ms, music: None, music_parts });
        m.playing = Some((Instant::now(), from));
        self.manga_set_playhead(from);
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(TICK).await;
            let alive = this.update(cx, |this, cx| {
                let Some((t0, from, total)) = this.vedit.whole.all.as_ref().and_then(|m| m.playing.map(|(t0, f)| (t0, f, m.total()))) else { return false };
                let now = from + t0.elapsed().as_millis() as u64;
                if now >= total || this.view != View::Video {
                    this.manga_stop();
                    this.manga_set_playhead(now.min(total));
                } else {
                    this.manga_set_playhead(now);
                    this.manga_follow();
                }
                cx.notify();
                this.vedit.whole.all.as_ref().is_some_and(|m| m.playing.is_some())
            });
            if !matches!(alive, Ok(true)) {
                break;
            }
        })
        .detach();
        cx.notify();
    }

    /// Previous / next edit on the manga timeline (clip edges and chapter starts).
    fn manga_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(m) = self.vedit.whole.all.as_ref() else { return };
        let now = self.manga_now();
        let mut edges: Vec<u64> = vm::edges(&m.spans);
        for (c, s) in m.chapters.iter().zip(&m.spans) {
            edges.extend(c.placed.iter().map(|p| s.start_ms + p.end_ms()));
        }
        edges.sort_unstable();
        edges.dedup();
        let to = if forward { edges.iter().copied().find(|e| *e > now + 5) } else { edges.iter().rev().copied().find(|e| *e + 5 < now) };
        if let Some(t) = to {
            self.manga_seek(t, cx);
        }
    }

    fn manga_chapter_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(m) = self.vedit.whole.all.as_ref() else { return };
        let now = self.manga_now();
        let starts: Vec<u64> = m.spans.iter().filter(|s| s.dur_ms > 0).map(|s| s.start_ms).collect();
        let to = if forward { starts.iter().copied().find(|s| *s > now + 5) } else { starts.iter().rev().copied().find(|s| *s + 1500 < now) };
        self.manga_seek(to.unwrap_or(if forward { u64::MAX } else { 0 }), cx);
    }

    /// The keys that move around the manga timeline; the rest act on the open chapter.
    pub(super) fn manga_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let k = &ev.keystroke;
        let cmd = k.modifiers.control || k.modifiers.platform;
        match k.key.as_str() {
            "escape" if self.vedit.whole.confirm => self.vedit.whole.confirm = false,
            "space" if !cmd => self.manga_toggle_play(cx),
            "home" => self.manga_seek(0, cx),
            "end" => self.manga_seek(u64::MAX, cx),
            "up" => self.manga_step(false, cx),
            "down" => self.manga_step(true, cx),
            "pageup" => self.manga_chapter_step(false, cx),
            "pagedown" => self.manga_chapter_step(true, cx),
            "left" => self.manga_seek(self.manga_now().saturating_sub(1000 / vedit::FPS), cx),
            "right" => self.manga_seek(self.manga_now() + 1000 / vedit::FPS, cx),
            "=" | "+" => self.manga_zoom(1.5, None),
            "-" => self.manga_zoom(1. / 1.5, None),
            "\\" => self.manga_fit(),
            _ => return false,
        }
        true
    }

    // ------------------------------------------------------------------------------ sync

    /// Takes the open chapter's edit into the manga timeline (its clips may have new lengths).
    fn manga_absorb(&mut self) {
        let Some(seq) = self.vedit.seq.as_ref() else { return };
        let Some(m) = self.vedit.whole.all.as_mut() else { return };
        let Some(c) = m.chapters.get_mut(m.focus).filter(|c| c.item.id == seq.item.id) else { return };
        if c.edit != seq.edit {
            c.edit = seq.edit.clone();
            c.placed = seq.placed();
            m.relayout();
        }
    }

    /// After the open chapter's edit or caption setting was saved. In "All chapters" the
    /// settings that belong to the whole manga (captions, look, transitions, music, the caption /
    /// music / narration switches) that changed are carried to every other chapter of it, and
    /// saved. Either way the look states are worked out again.
    pub(super) fn manga_sync(&mut self, cx: &mut Context<Self>) {
        let open = self.vedit.seq.as_ref().map(|s| s.item.id.clone());
        if self.vedit.whole.all.is_none() {
            if let Some(id) = open {
                self.vedit.whole.looks.borrow_mut().remove(&id);
            }
            return;
        }
        self.vedit.whole.looks.borrow_mut().clear();
        let Some(seq) = self.vedit.seq.as_ref() else { return };
        let Some(m) = self.vedit.whole.all.as_mut() else { return };
        let k = m.focus;
        let Some(me) = m.chapters.get_mut(k).filter(|c| c.item.id == seq.item.id) else { return };
        let (before, before_subs) = (std::mem::replace(&mut me.edit, seq.edit.clone()), std::mem::replace(&mut me.subs, seq.subs.clone()));
        me.placed = seq.placed();
        let mut changed: Vec<(String, ChapterEdit)> = vec![];
        for (i, c) in m.chapters.iter_mut().enumerate() {
            if i == k {
                continue;
            }
            if vm::apply_global(&before, &seq.edit, &mut c.edit) {
                changed.push((c.item.id.clone(), c.edit.clone()));
            }
            if before_subs != seq.subs && c.subs != seq.subs {
                c.subs = seq.subs.clone();
                video::save_chapter_setting(&c.item.output_dir, Some(&c.subs));
            }
        }
        m.relayout();
        let dir = Self::manga_dir(&seq.item);
        if !changed.is_empty() {
            match vedit::update_project(&dir, |p| {
                for (id, e) in changed {
                    let rendered = p.chapters.get(&id).and_then(|e| e.rendered);
                    p.chapters.insert(id, ChapterEdit { rendered, ..e });
                }
            }) {
                Ok(p) => self.vedit.project = p,
                Err(e) => toast::show(cx, Toast::error(format!("Could not save the edit to every chapter: {e}"))),
            }
        }
    }

    /// Called by the page's poll: chapters that finished re-making are read again (their videos,
    /// timing and look stamp are new), and chat edits reach the other chapters.
    pub(super) fn manga_poll(&mut self, cx: &mut Context<Self>) {
        let Some(g) = self.video_manga() else { return };
        // The Studio look changed (Look & branding): every look state may have.
        let saved = crate::look::look_file().metadata().and_then(|m| m.modified()).ok();
        if saved != self.vedit.whole.look_saved {
            self.vedit.whole.look_saved = saved;
            self.vedit.whole.looks.borrow_mut().clear();
        }
        let active: Vec<String> = g.chapters.iter().filter(|c| self.queue.for_chapter(&c.id).is_some_and(|q| q.state.active())).map(|c| c.id.clone()).collect();
        let finished: Vec<String> = self.vedit.whole.remaking.iter().filter(|id| !active.contains(id)).cloned().collect();
        self.vedit.whole.remaking = active;
        let tools = crate::vedit_tools::version();
        let tools_moved = tools != self.vedit.whole.seen_tools;
        self.vedit.whole.seen_tools = tools;
        if finished.is_empty() && !tools_moved {
            return;
        }
        self.vedit.whole.looks.borrow_mut().clear();
        let Some(m) = self.vedit.whole.all.as_mut() else { return };
        let project = Project::load(&Self::manga_dir(&m.chapters[0].item));
        let focus_id = m.chapters.get(m.focus).map(|c| c.item.id.clone());
        for c in m.chapters.iter_mut() {
            if finished.contains(&c.item.id) {
                // Re-made: the sync map (beats), score and caption setting are read again.
                let pages = mv_recap::list_pages(&c.item.pages_dir);
                c.beats = vedit::load_beats(&c.item.output_dir, &pages);
                c.subs = video::subtitle_setting_for(&c.item);
                c.score = Some(mv_score::score_wav(&crate::page::chref(&c.item))).filter(|p| p.is_file());
            }
            if tools_moved || finished.contains(&c.item.id) {
                c.edit = project.chapters.get(&c.item.id).cloned().unwrap_or_default();
            }
            c.placed = vedit::place(&c.beats, &c.edit.clips(&c.beats));
        }
        m.relayout();
        // The open chapter follows (its edit is reloaded by `video_poll` after a chat edit).
        if let (Some(id), Some(seq)) = (focus_id, self.vedit.seq.as_mut()) {
            if finished.contains(&id) && seq.item.id == id {
                if let Some(c) = m.chapters.get(m.focus) {
                    seq.beats = c.beats.clone();
                    seq.subs = c.subs.clone();
                    seq.score = c.score.clone();
                }
            }
        }
        let t = m.playhead;
        self.manga_set_playhead(t);
        cx.notify();
    }

    // ------------------------------------------------------------------------------ status

    /// Each made chapter's look state: its video was made with what the editor shows now, or not.
    pub(super) fn looks_for(&self, g: &MangaGroup) -> HashMap<String, LookState> {
        let made = self.made_chapters(g);
        let missing: Vec<&ChapterItem> = {
            let cache = self.vedit.whole.looks.borrow();
            made.iter().filter(|c| !cache.contains_key(&c.id)).collect()
        };
        if !missing.is_empty() {
            let look = crate::look::video_config();
            let mut cache = self.vedit.whole.looks.borrow_mut();
            for c in missing {
                let live = self.vedit.seq.as_ref().filter(|s| s.item.id == c.id).map(|s| (s.beats.as_slice(), &s.edit, &s.subs));
                let mem = self.vedit.whole.all.as_ref().and_then(|m| m.chapters.iter().find(|x| x.item.id == c.id)).map(|x| (x.beats.as_slice(), &x.edit, &x.subs));
                let key = match live.or(mem) {
                    Some((beats, edit, subs)) => vm::look_key(&look, vm::music_on(c), &vedit::render_extra(beats, edit, subs)),
                    None => vm::current_key(c),
                };
                cache.insert(c.id.clone(), vm::look_state(vm::stamp(&c.output_dir), key));
            }
        }
        self.vedit.whole.looks.borrow().clone()
    }

    /// Where a chapter's re-make stands (None when it is not in the queue or finished).
    pub(super) fn remake_of(&self, id: &str) -> Option<Remake> {
        let q = self.queue.for_chapter(id)?;
        let (label, progress, failed) = match q.state {
            ItemState::Done => return None,
            ItemState::Queued if self.queue.paused => ("queue paused".to_string(), None, false),
            ItemState::Queued => ("waiting to re-make".to_string(), None, false),
            ItemState::Paused => ("paused".to_string(), None, false),
            ItemState::Failed => ("re-make failed".to_string(), None, true),
            ItemState::Running => match q.current_step() {
                Some(mv_recap::Step::Video) => {
                    let p = self.jobs.iter().find(|j| j.chapter_id == id && !j.state.finished()).map(|j| j.progress() as f32);
                    (format!("re-making{}", p.map(|p| format!(" · {}%", (p * 100.).round())).unwrap_or_default()), Some(p.unwrap_or(0.)), false)
                }
                Some(s) => (format!("re-making · {}", s.label().to_lowercase()), Some(0.), false),
                None => ("re-making".to_string(), Some(0.), false),
            },
        };
        Some(Remake { label, progress, failed })
    }

    // ------------------------------------------------------------------------------ re-edit

    /// Re-makes `chapters` (this manga's made chapters only) from the Video step with their
    /// edits, the current look, captions and music, in the dashboard's queue.
    fn reedit_start(&mut self, g: &MangaGroup, chapters: Vec<ChapterItem>, cx: &mut Context<Self>) {
        self.vedit.whole.confirm = false;
        let statuses = &self.statuses;
        let targets = vm::reedit_targets(&g.id, &chapters, |c| made(statuses, c));
        if targets.is_empty() {
            toast::show(cx, Toast::info("No finished chapter of this manga to re-make"));
            return;
        }
        self.video_stop();
        self.manga_absorb();
        self.sync_pipeline(cx);
        match crate::vedit_tools::render(&targets) {
            Ok((n, project)) => {
                if let Some(s) = self.vedit.seq.as_mut() {
                    s.edit.rendered = project.chapters.get(&s.item.id).and_then(|e| e.rendered);
                }
                if let Some(m) = self.vedit.whole.all.as_mut() {
                    for c in m.chapters.iter_mut() {
                        c.edit.rendered = project.chapters.get(&c.item.id).and_then(|e| e.rendered);
                    }
                }
                self.vedit.project = project;
                self.vedit.whole.remaking = targets.iter().map(|c| c.id.clone()).collect();
                self.vedit.whole.seen_tools = crate::vedit_tools::version();
                toast::show(cx, Toast::success(format!("Re-making {n} chapter{} of {} with the current look", plural(n), g.title)).icon(ph::ARROWS_CLOCKWISE));
            }
            Err(e) => toast::show(cx, Toast::error(e)),
        }
        cx.notify();
    }

    /// The confirmation: how many chapters, roughly how long, and that nothing else is touched.
    pub(super) fn reedit_dialog(&self, g: &MangaGroup, cx: &mut Context<Self>) -> Stateful<Div> {
        let chapters = self.made_chapters(g);
        let n = chapters.len();
        let looks = self.looks_for(g);
        let old: Vec<ChapterItem> = chapters.iter().filter(|c| looks.get(&c.id) != Some(&LookState::Current)).cloned().collect();
        let busy = chapters.iter().filter(|c| self.queue.for_chapter(&c.id).is_some_and(|q| q.state.active())).count();
        // The queue voices a chapter again first when its narrator was changed since (as Redo does).
        let revoice = chapters
            .iter()
            .filter(|c| {
                let files = self.statuses.get(&c.id).map(|s| s.audio_files).unwrap_or(0);
                crate::pipeline::voice_changed(files, crate::pipeline::narrated_voice(&c.output_dir).as_deref(), &mv_studio::voices::voice_for_chapter(&c.id))
            })
            .count();
        let look = self.studio_look();
        let lengths = |list: &[ChapterItem]| -> Vec<f64> {
            list.iter()
                .map(|c| {
                    let v = c.video.clone().or_else(|| self.statuses.get(&c.id).and_then(|s| s.video.clone()));
                    v.and_then(|p| crate::export::video_seconds(&p)).unwrap_or(60.)
                })
                .collect()
        };
        let est = vm::estimate_secs(&lengths(&chapters), look.effects().grain);
        let est_old = vm::estimate_secs(&lengths(&old), look.effects().grain);
        let fx: Vec<&str> = crate::look::Effect::ALL.iter().filter(|e| look.effects().get(**e)).map(|e| e.label()).collect();
        let line = |icon, text: String| div().flex().items_center().gap(px(space::SM)).child(icon_ph(icon, Tokens::text_2(), icon_px::SM)).child(div().flex_1().text_size(px(size::SM)).text_color(Tokens::text_1()).child(text));
        let all_list = chapters.clone();
        let old_list = old.clone();
        let (g1, g2) = (g.clone(), g.clone());
        div()
            .id("vedit-reedit")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(Tokens::scrim())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id("vedit-reedit-card")
                    .flex()
                    .flex_col()
                    .gap(px(space::MD))
                    .w(px(560.))
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
                            .child(div().flex_1().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("Re-edit full timeline · {}", g.title)))
                            .child(icon_button("vedit-reedit-close", ph::X, "Close (Esc)").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.vedit.whole.confirm = false;
                                cx.notify();
                            }))),
                    )
                    .child(muted(format!(
                        "Every finished chapter of {} is made again from the Video step, with each chapter's edit, its captions and music, and the current look. A chapter whose score is out of date gets its music made first. No other manga is touched.",
                        g.title
                    )))
                    .child(line(ph::FILM_STRIP, format!("{n} chapter{} to re-make · {} on an old look", plural(n), old.len())))
                    .child(line(ph::MAGIC_WAND, format!("Look: {}{}", look.preset.label(), if fx.is_empty() { String::new() } else { format!(" · {}", fx.join(", ")) })))
                    .child(line(ph::CLOCK, format!("Estimated time: {} (rough; it depends on your PC)", vm::duration_words(est))))
                    .when(revoice > 0, |d| d.child(div().text_size(px(size::SM)).text_color(Tokens::warning()).child(format!("{revoice} chapter{} {} a new narrator: {} voiced again first, which takes longer.", plural(revoice), if revoice == 1 { "has" } else { "have" }, if revoice == 1 { "it is" } else { "they are" }))))
                    .when(busy > 0, |d| d.child(div().text_size(px(size::SM)).text_color(Tokens::warning()).child(format!("{busy} of them {} already in the queue: {} the re-make once {} done.", if busy == 1 { "is" } else { "are" }, if busy == 1 { "it joins" } else { "they join" }, if busy == 1 { "it is" } else { "they are" }))))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap(px(space::SM))
                            .child(btn("vedit-reedit-cancel", "Cancel", ButtonKind::Ghost).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.vedit.whole.confirm = false;
                                cx.notify();
                            })))
                            .when(!old.is_empty() && old.len() < n, |d| {
                                d.child(
                                    btn("vedit-reedit-old", format!("Only the {} on an old look", old.len()), ButtonKind::Secondary)
                                        .tooltip(format!("Re-make only the chapters whose video is out of date ({})", vm::duration_words(est_old)))
                                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.reedit_start(&g1, old_list.clone(), cx))),
                                )
                            })
                            .child(
                                btn("vedit-reedit-go", format!("Re-make {n} chapter{}", plural(n)), ButtonKind::Primary)
                                    .icon(ph::ARROWS_CLOCKWISE)
                                    .when(n == 0, |b| b.disabled("No finished chapters"))
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.reedit_start(&g2, all_list.clone(), cx))),
                            ),
                    ),
            )
    }

    // ------------------------------------------------------------------------------ switcher

    /// "All chapters" and each chapter's own timeline, with its look state; and Re-edit.
    pub(super) fn chapter_switcher(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(g) = self.video_manga() else { return div().into_any_element() };
        let chapters = self.made_chapters(&g);
        let looks = self.looks_for(&g);
        let all_on = self.vedit.whole.all.is_some();
        let open = self.vedit.seq.as_ref().map(|s| s.item.id.clone());
        let chip = |id: SharedString, on: bool| {
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .gap(px(4.))
                .h(px(22.))
                .px(px(space::SM))
                .rounded(px(radius::SM))
                .text_size(px(size::XS))
                .cursor_pointer()
                .border_1()
                .border_color(if on { Tokens::accent() } else { Tokens::border() })
                .bg(if on { Tokens::accent_soft() } else { Tokens::surface_2() })
                .text_color(Tokens::text_1())
                .hover(|d| d.bg(Tokens::hover()))
        };
        let mut row = div().id("vedit-chapters").flex().items_center().gap(px(4.)).min_w_0().overflow_x_scroll().child(
            chip("vedit-ch-all".into(), all_on).child(icon_ph(ph::FILM_STRIP, Tokens::text_1(), icon_px::XS)).child("All chapters").tooltip(tip("Every chapter of the manga on one timeline")).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                if this.vedit.whole.all.is_none() {
                    this.manga_enter(cx);
                }
            })),
        );
        for (k, c) in chapters.iter().enumerate() {
            let remake = self.remake_of(&c.id);
            let state = looks.get(&c.id).copied();
            let (dot, why) = match (&remake, state) {
                (Some(r), _) if r.failed => (Tokens::danger(), r.label.clone()),
                (Some(r), _) => (Tokens::accent(), r.label.clone()),
                (None, Some(LookState::Current)) => (Tokens::success(), "current look".to_string()),
                (None, _) => (Tokens::warning(), "old look — re-edit to update".to_string()),
            };
            let on = !all_on && open.as_deref() == Some(c.id.as_str());
            let focused = all_on && open.as_deref() == Some(c.id.as_str());
            let id = c.id.clone();
            row = row.child(
                chip(SharedString::from(format!("vedit-ch-{}", c.id)), on)
                    .when(focused, |d| d.border_color(Tokens::accent_border()))
                    .child(div().size(px(6.)).rounded_full().bg(dot))
                    .child(short_label(c.number, c.title.as_deref()))
                    .tooltip(tip(format!("{}: {why}. Open its own timeline.", c.label())))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if this.vedit.whole.all.is_some() {
                            this.manga_leave(Some(k), cx);
                        } else if let Some(c) = this.video_manga().and_then(|g| g.chapters.into_iter().find(|c| c.id == id)) {
                            this.video_open(c);
                            cx.notify();
                        }
                    })),
            );
        }
        let old = chapters.iter().filter(|c| looks.get(&c.id) != Some(&LookState::Current)).count();
        div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .min_w_0()
            .max_w(px(760.))
            .child(row)
            .child(
                btn("vedit-reedit-open", "Re-edit full timeline", if old > 0 { ButtonKind::Primary } else { ButtonKind::Secondary })
                    .small()
                    .icon(ph::ARROWS_CLOCKWISE)
                    .tooltip(if old > 0 {
                        format!("{old} chapter{} on an old look: re-make every chapter with the current look, captions, music and edits", plural(old))
                    } else {
                        "Re-make every chapter with the current look, captions, music and edits".to_string()
                    })
                    .when(chapters.is_empty(), |b| b.disabled("No finished chapters"))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.video_stop();
                        this.vedit.whole.confirm = true;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    // ------------------------------------------------------------------------------ monitor

    /// The Program monitor over the manga: the chapter under the playhead (drawn with its look),
    /// the manga's transport, and the rendered videos to play (what the export joins).
    pub(super) fn manga_monitor_panel(&self, g: &MangaGroup, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let (Some(m), Some(seq)) = (self.vedit.whole.all.as_ref(), self.vedit.seq.as_ref()) else { return div() };
        let placed = seq.placed();
        let local = self.vedit.playhead.min(vedit::total_ms(&placed));
        let total = m.total();
        let now = self.manga_now();
        let playing = m.playing.is_some();
        let looks = self.looks_for(g);
        let remake = self.remake_of(&seq.item.id);
        let (state, colour) = match (&remake, looks.get(&seq.item.id)) {
            (Some(r), _) => (r.label.clone(), if r.failed { Tokens::danger() } else { Tokens::accent() }),
            (None, Some(LookState::Current)) => ("current look".to_string(), Tokens::text_3()),
            (None, _) => ("old look — re-edit to update".to_string(), Tokens::warning()),
        };
        let video = seq.item.video.clone().or_else(|| self.statuses.get(&seq.item.id).and_then(|s| s.video.clone()));
        let title = format!("{} · {}", g.title, seq.item.label());
        let play_chapter = btn("vedit-m-play-ch", "Chapter video", ButtonKind::Ghost)
            .small()
            .icon(ph::MONITOR_PLAY)
            .tooltip("Play this chapter's made video")
            .when(video.is_none(), |b| b.disabled("No video yet"))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.video_stop();
                if let Some(v) = &video {
                    crate::phases::play_video(&title, v, window, cx);
                }
            }));
        let old = looks.values().filter(|s| **s == LookState::Old).count();
        let gp = g.clone();
        let play_full = btn("vedit-m-play-full", "Full recap", ButtonKind::Ghost)
            .small()
            .icon(ph::PLAY_CIRCLE)
            .tooltip(if old > 0 { format!("Play the made videos joined, as the export makes them ({old} still on an old look)") } else { "Play the made videos joined, exactly as the export makes them".to_string() })
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.video_stop();
                this.play_full(&gp, window, cx);
            }));
        let viewport = window.viewport_size();
        let (vw, vh) = (f32::from(viewport.width), f32::from(viewport.height));
        let room_w = (vw - LEFT_W - RIGHT_W - 60.).max(240.);
        let room_h = (vh - 52. - 38. - 330. - 30. - 96. - 40.).max(140.);
        let aspect = if seq.reels { 9. / 16. } else { 16. / 9. };
        let (fw, fh) = if room_w / room_h > aspect { (room_h * aspect, room_h) } else { (room_w, room_w / aspect) };
        let frame = self.frame(seq, &placed, local, fw, fh, window, cx);
        let scrub_frac = if total > 0 { now as f32 / total as f32 } else { 0. };
        let bars = self.vedit.bars.clone();
        let scrub = div()
            .id("vedit-m-scrub")
            .relative()
            .h(px(14.))
            .mx(px(space::MD))
            .cursor_pointer()
            .child(div().absolute().left_0().right_0().top(px(6.)).h(px(2.)).bg(Tokens::border_strong()))
            .children(m.spans.iter().skip(1).filter(|_| total > 0).map(|s| div().absolute().left(relative(s.start_ms as f32 / total as f32)).top(px(3.)).w(px(1.)).h(px(8.)).bg(Tokens::text_3())))
            .child(div().absolute().left(relative(scrub_frac)).top_0().w(px(2.)).h_full().bg(Tokens::accent()))
            .child(canvas(move |b, _, _| { bars.borrow_mut().insert("mscrub", b); }, |_, _, _, _| {}).absolute().inset_0())
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                if let Some(f) = this.bar_fraction("mscrub", ev.position.x) {
                    this.manga_seek((f * total as f32) as u64, cx);
                }
            }));
        let transport = |id: &'static str, icon, label: &'static str| icon_button(id, icon, label).small();
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
                    .gap(px(space::SM))
                    .h(px(30.))
                    .px(px(space::MD))
                    .child(div().text_size(px(size::MD)).font_family(crate::page::MONO).text_color(Tokens::accent()).child(vedit::timecode(now)))
                    .child(div().flex_1())
                    .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(seq.item.label()))
                    .child(div().text_size(px(size::SM)).text_color(colour).child(state))
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
                    .child(transport("vedit-m-start", ph::SKIP_BACK, "Go to start (Home)").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.manga_seek(0, cx))))
                    .child(transport("vedit-m-prev-ch", ph::REWIND, "Previous chapter (Page Up)").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.manga_chapter_step(false, cx))))
                    .child(transport("vedit-m-prev", ph::CARET_DOUBLE_LEFT, "Previous edit (Up)").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.manga_step(false, cx))))
                    .child(icon_button("vedit-m-play", if playing { ph::PAUSE } else { ph::PLAY }, if playing { "Pause (Space)" } else { "Play every chapter (Space)" }).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.manga_toggle_play(cx))))
                    .child(transport("vedit-m-next", ph::CARET_DOUBLE_RIGHT, "Next edit (Down)").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.manga_step(true, cx))))
                    .child(transport("vedit-m-next-ch", ph::FAST_FORWARD, "Next chapter (Page Down)").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.manga_chapter_step(true, cx))))
                    .child(transport("vedit-m-end", ph::SKIP_FORWARD, "Go to end (End)").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.manga_seek(u64::MAX, cx)))),
            );
        let extra = div().flex().items_center().gap(px(4.)).child(play_chapter).child(play_full);
        self.panel(&format!("Program: {} · all chapters", g.title), Some(extra.into_any_element()), body).flex_1()
    }

    // ------------------------------------------------------------------------------ timeline

    /// The manga timeline: the chapter track over the chapters' captions, layers, panels,
    /// narration and music, end to end.
    pub(super) fn manga_timeline(&self, g: &MangaGroup, cx: &mut Context<Self>) -> Div {
        let Some(m) = self.vedit.whole.all.as_ref() else { return div() };
        let total = m.total();
        let pps = self.vedit.pps;
        let sx = self.vedit.scroll_x;
        let view_w = f32::from(self.vedit.lanes.get().size.width).max(1.);
        let (t0, t1) = ((sx / pps * 1000.) as u64, ((sx + view_w) / pps * 1000.) as u64 + 1);
        let xof = move |ms: u64| ms as f32 / 1000. * pps - sx;
        let now = self.manga_now();
        let looks = self.looks_for(g);
        let old = m.chapters.iter().filter(|c| looks.get(&c.item.id) != Some(&LookState::Current)).count();
        let remaking = m.chapters.iter().filter(|c| self.remake_of(&c.item.id).is_some_and(|r| !r.failed)).count();
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
            .child(icon_button("vedit-m-zoom-out", ph::MAGNIFYING_GLASS_MINUS, "Zoom out (-)").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.manga_zoom(1. / 1.5, None);
                cx.notify();
            })))
            .child(icon_button("vedit-m-zoom-in", ph::MAGNIFYING_GLASS, "Zoom in (=)").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.manga_zoom(1.5, None);
                cx.notify();
            })))
            .child(icon_button("vedit-m-zoom-fit", ph::ARROWS_IN_SIMPLE, "Zoom to the whole manga (\\)").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.manga_fit();
                cx.notify();
            })))
            .child(div().flex_1())
            .when(remaking > 0, |d| d.child(div().text_size(px(size::SM)).text_color(Tokens::accent()).child(format!("Re-making {remaking}"))))
            .when(old > 0, |d| d.child(div().text_size(px(size::SM)).text_color(Tokens::warning()).child(format!("{old} on an old look"))))
            .child(muted(format!("{} chapters · {}", m.chapters.len(), vedit::timecode(total))));

        // Ruler.
        let (major, minor) = vedit::ruler_step(pps);
        let mut ruler = div()
            .id("vedit-m-ruler")
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
        let step_ms = ((major * 1000.) as u64).max(1);
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

        let sel = self.vedit.sel;
        let focus = m.focus;
        let visible: Vec<(usize, &MChapter, Span)> = m.chapters.iter().zip(&m.spans).enumerate().filter(|(_, (_, s))| s.dur_ms > 0 && s.end_ms() >= t0 && s.start_ms <= t1).map(|(k, (c, s))| (k, c, *s)).collect();
        let block = |id: SharedString, x: f32, w: f32, h: f32, fill: Rgba, edge: Rgba, selected: bool| {
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

        // The chapter track.
        let mut ch_lane = div().id("vedit-m-lane-ch").relative().h(px(CH_H)).overflow_hidden().border_b_1().border_color(Tokens::border());
        for (k, c, s) in &visible {
            let (k, s) = (*k, *s);
            let (x, w) = (xof(s.start_ms), s.dur_ms as f32 / 1000. * pps);
            let remake = self.remake_of(&c.item.id);
            let state = looks.get(&c.item.id).copied();
            let (fill, edge, note) = match (&remake, state) {
                (Some(r), _) if r.failed => (soft(Tokens::danger(), 0.25), Tokens::danger(), r.label.clone()),
                (Some(r), _) => (soft(Tokens::accent(), 0.30), Tokens::accent(), r.label.clone()),
                (None, Some(LookState::Current)) => (soft(Tokens::info(), 0.22), Tokens::info(), "current look".to_string()),
                (None, _) => (soft(Tokens::warning(), 0.22), Tokens::warning(), "old look — re-edit to update".to_string()),
            };
            let mut el = block(SharedString::from(format!("vedit-m-ch-{k}")), x, w, CH_H, fill, edge, k == focus)
                .cursor_pointer()
                .tooltip(tip(format!("{} · {}: {note}. Double-click to open its own timeline.", c.item.label(), vedit::timecode(s.dur_ms))));
            if w > 30. {
                el = el.child(
                    div()
                        .absolute()
                        .left(px(4.).max(px(-x + 4.)))
                        .top(px(2.))
                        .right(px(4.))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(div().flex_none().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(short_label(c.item.number, c.item.title.as_deref())))
                        .when(w > 110., |d| d.child(div().min_w_0().truncate().text_size(px(size::XS)).text_color(edge).child(note))),
                );
            }
            if let Some(p) = remake.as_ref().and_then(|r| r.progress) {
                el = el.child(div().absolute().left_0().right_0().bottom_0().h(px(4.)).bg(Tokens::track())).child(div().absolute().left_0().bottom_0().h(px(4.)).w(relative(p.clamp(0.02, 1.))).bg(Tokens::accent()));
            }
            ch_lane = ch_lane.child(el.on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                if ev.click_count() >= 2 {
                    this.manga_leave(Some(k), cx);
                } else {
                    this.manga_seek(s.start_ms, cx);
                }
            })));
        }

        // The tracks, chapter by chapter.
        let mut lanes: Vec<AnyElement> = vec![ch_lane.into_any_element()];
        for t in Track::ALL {
            let h = lane_h(t);
            let mut lane = div().id(SharedString::from(format!("vedit-m-lane-{}", t.code()))).relative().h(px(h)).overflow_hidden().border_b_1().border_color(Tokens::border());
            let (fill, edge) = match t {
                Track::Clips => (soft(Tokens::info(), 0.32), Tokens::info()),
                Track::Narration | Track::Music => (soft(Tokens::success(), 0.26), Tokens::success()),
                Track::Layers => (soft(Tokens::accent_2(), 0.35), Tokens::accent_2()),
                Track::Captions => (soft(Tokens::accent(), 0.28), Tokens::accent()),
            };
            for (k, c, s) in &visible {
                let (k, s) = (*k, *s);
                let off = c.edit.flags(t).off;
                let fill = if off { soft(fill, 0.08) } else { fill };
                match t {
                    Track::Clips | Track::Narration | Track::Captions => {
                        // Zoomed far out: one block for the chapter instead of every clip.
                        let thin = c.placed.is_empty() || (s.dur_ms as f32 / 1000. * pps) / (c.placed.len() as f32) < 4.;
                        if thin {
                            let x = xof(s.start_ms);
                            lane = lane.child(block(SharedString::from(format!("vedit-m-{}-{k}", t.code())), x, s.dur_ms as f32 / 1000. * pps, h, fill, edge, false));
                            continue;
                        }
                        for p in c.placed.iter().filter(|p| s.start_ms + p.end_ms() >= t0 && s.start_ms + p.start_ms <= t1) {
                            let start = s.start_ms + p.start_ms;
                            let (x, w) = (xof(start), p.dur_ms as f32 / 1000. * pps);
                            let index = p.index;
                            let beat = c.beats.get(p.clip.src);
                            let selected = k == focus && sel == Some(Sel::Clip(index));
                            let label = match t {
                                Track::Clips => format!("p{}{}", beat.map(|b| b.page + 1).unwrap_or(0), if p.clip.motion == Motion::Auto { String::new() } else { format!(" · {}", p.clip.motion.label()) }),
                                Track::Narration => beat.and_then(|b| b.audio.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                                _ => p.clip.text.clone().or_else(|| beat.map(|b| b.text.clone())).unwrap_or_default(),
                            };
                            let mut el = block(SharedString::from(format!("vedit-m-{}-{k}-{index}", t.code())), x, w, h, fill, edge, selected);
                            if t == Track::Clips && w > 44. {
                                if let Some(img_p) = beat.and_then(|b| b.image.clone()) {
                                    el = el.child(img(img_p).absolute().left_0().top_0().h_full().w(px((h - 4.) * 0.75)).object_fit(ObjectFit::Cover));
                                }
                            }
                            if w > 26. {
                                let pad = if t == Track::Clips && w > 44. { (h - 4.) * 0.75 + 4. } else { 4. };
                                el = el.child(div().absolute().left(px(pad)).top(px(2.)).right(px(2.)).truncate().text_size(px(size::XS)).text_color(Tokens::text_1()).child(label));
                            }
                            lane = lane.child(el.cursor_pointer().on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.manga_seek(start, cx);
                                this.vedit.sel = Some(Sel::Clip(index));
                                cx.notify();
                            })));
                        }
                    }
                    Track::Layers => {
                        for l in c.edit.layers.iter().filter(|l| s.start_ms + l.end_ms >= t0 && s.start_ms + l.start_ms <= t1) {
                            let start = s.start_ms + l.start_ms;
                            let (x, w) = (xof(start), (l.end_ms - l.start_ms) as f32 / 1000. * pps);
                            let id = l.id;
                            let icon = if matches!(l.kind, LayerKind::Text { .. }) { ph::TEXT_T } else { ph::IMAGE };
                            let el = block(SharedString::from(format!("vedit-m-layer-{k}-{id}")), x, w, h, fill, edge, k == focus && sel == Some(Sel::Layer(id)))
                                .when(w > 26., |d| d.child(div().absolute().left(px(4.)).top(px(2.)).right(px(4.)).flex().items_center().gap(px(4.)).child(icon_ph(icon, Tokens::text_1(), icon_px::XS)).child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_1()).child(l.name()))));
                            lane = lane.child(el.cursor_pointer().on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.manga_seek(start, cx);
                                this.vedit.sel = Some(Sel::Layer(id));
                                cx.notify();
                            })));
                        }
                    }
                    Track::Music => {
                        let name = match &c.edit.music.source {
                            vedit::MusicSource::Score => c.score.as_ref().map(|_| "score".to_string()),
                            vedit::MusicSource::File { path } => path.file_name().map(|n| n.to_string_lossy().into_owned()),
                            vedit::MusicSource::None => None,
                        };
                        if let Some(name) = name {
                            let x = xof(s.start_ms);
                            let w = s.dur_ms as f32 / 1000. * pps;
                            lane = lane.child(
                                block(SharedString::from(format!("vedit-m-music-{k}")), x, w, h, soft(fill, 0.8), edge, false)
                                    .when(w > 40., |d| d.child(div().absolute().left(px(4.).max(px(-x + 4.))).top(px(2.)).flex().items_center().gap(px(4.)).child(icon_ph(ph::MUSIC_NOTES, Tokens::text_1(), icon_px::XS)).child(div().text_size(px(size::XS)).text_color(Tokens::text_1()).child(format!("{name} · {:.0}%", c.edit.music.volume * 100.))))),
                            );
                        }
                    }
                }
            }
            lanes.push(lane.into_any_element());
        }

        // Chapter boundaries, the snap line and the playhead, over every lane.
        let lanes_h: f32 = CH_H + Track::ALL.iter().map(|t| lane_h(*t)).sum::<f32>();
        let mut over = div().absolute().top_0().left_0().right_0().h(px(lanes_h));
        for (_, _, s) in visible.iter().skip_while(|(k, _, _)| *k == 0) {
            over = over.child(div().absolute().left(px(xof(s.start_ms))).top_0().bottom_0().w(px(1.)).bg(Tokens::border_strong()));
        }
        if let Some(s) = self.vedit.snapped {
            over = over.child(div().absolute().left(px(xof(s))).top_0().bottom_0().w(px(1.)).bg(Tokens::warning()));
        }
        over = over.child(div().absolute().left(px(xof(now))).top_0().bottom_0().w(px(1.)).bg(Tokens::accent()));

        let lanes_box = self.vedit.lanes.clone();
        let content = div()
            .id("vedit-m-lanes")
            .relative()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .child(ruler)
            .child(div().relative().flex().flex_col().children(lanes).child(over))
            .child(canvas(move |b, _, _| lanes_box.set(b), |_, _, _, _| {}).absolute().inset_0())
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                this.vedit.sel = None;
                this.vedit.drag = Some(Drag::Playhead);
                this.drag_move(ev.position, cx);
            }))
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| {
                let d = ev.delta.pixel_delta(px(20.));
                let (dx, dy) = (f32::from(d.x), f32::from(d.y));
                if ev.modifiers.control || ev.modifiers.alt {
                    let ax = f32::from(ev.position.x) - f32::from(this.vedit.lanes.get().origin.x);
                    this.manga_zoom(if dy > 0. { 1.2 } else { 1. / 1.2 }, Some(ax));
                } else {
                    let total = this.timeline_ms() as f32 / 1000. * this.vedit.pps;
                    let w = f32::from(this.vedit.lanes.get().size.width);
                    this.vedit.scroll_x = (this.vedit.scroll_x - if dx.abs() > dy.abs() { dx } else { dy }).clamp(0., (total - w + 40.).max(0.));
                }
                cx.notify();
            }));

        // Track headers (the switches act on the open chapter and, being global, on every chapter).
        let head = |id: SharedString, code: &'static str, label: &'static str, h: f32, audio: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(4.))
                .h(px(h))
                .px(px(4.))
                .border_b_1()
                .border_color(Tokens::border())
                .child(div().flex_none().w(px(26.)).h(px(20.)).rounded(px(3.)).flex().items_center().justify_center().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).bg(if audio { soft(Tokens::success(), 0.5) } else { soft(Tokens::info(), 0.5) }).text_color(Tokens::text_1()).child(code))
                .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).text_color(Tokens::text_2()).child(label))
        };
        let mut heads = div().flex().flex_col().flex_none().w(px(HEAD_W)).border_r_1().border_color(Tokens::border()).child(div().h(px(RULER_H)).border_b_1().border_color(Tokens::border()));
        heads = heads.child(head("vedit-m-head-ch".into(), "CH", "Chapters", CH_H, false));
        for t in Track::ALL {
            let selected = sel == Some(Sel::Track(t));
            heads = heads.child(head(SharedString::from(format!("vedit-m-head-{}", t.code())), t.code(), t.label(), lane_h(t), t.audio()).cursor_pointer().when(selected, |d| d.bg(Tokens::active())).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.vedit.sel = Some(Sel::Track(t));
                if t == Track::Captions {
                    this.vedit.side_tab = 0;
                } else if t.audio() {
                    this.vedit.side_tab = 1;
                }
                cx.notify();
            })));
        }

        let total_px = (total as f32 / 1000. * pps).max(1.);
        let (bar_l, bar_w) = ((sx / total_px).clamp(0., 1.), (view_w / total_px).clamp(0.02, 1.));
        let scroll = div()
            .id("vedit-m-hscroll")
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
        self.panel(&format!("{} · All chapters", g.title), Some(self.chapter_switcher(cx)), body).flex_1()
    }

    // ------------------------------------------------------------------------------ look preview

    /// The picture under the layers and captions, drawn the way the chapter's look renders it:
    /// the blurred fill, the panel (or two tall panels side by side for the duo looks), the
    /// action flash, and the grade and vignette. An approximation of the renderer's finish.
    pub(super) fn look_backdrop(&self, seq: &Seq, placed: &[Placed], now: u64, fw: f32, fh: f32) -> Vec<AnyElement> {
        let mut out = vec![];
        let cur_i = placed.partition_point(|p| p.end_ms() <= now).min(placed.len().saturating_sub(1));
        let Some(cur) = placed.get(cur_i) else { return out };
        let Some(image) = seq.beats.get(cur.clip.src).and_then(|b| b.image.clone()) else { return out };
        let (preset, fx) = match &seq.edit.look {
            Some(l) => {
                let p = Preset::parse(&l.preset).unwrap_or_default();
                let base = p.effects();
                let pick = |e: crate::look::Effect| l.effects.get(e.key()).copied().unwrap_or(base.get(e));
                (p, [pick(crate::look::Effect::Grade), pick(crate::look::Effect::Vignette), pick(crate::look::Effect::Glow), pick(crate::look::Effect::Impact)])
            }
            None => {
                let l = self.studio_look();
                let f = l.effects();
                (l.preset, [f.grade, f.vignette, f.glow, f.impact])
            }
        };
        let [grade, vignette, glow, impact] = fx;
        out.push(img(image.clone()).absolute().inset_0().size_full().object_fit(ObjectFit::Cover).opacity(0.28).into_any_element());
        out.push(div().absolute().inset_0().bg(video::paint(super::hex("#000000"), 0.35)).into_any_element());
        // The duo looks pair tall panels: the new one in one half, the one before it in the other.
        let duo_look = matches!(preset, Preset::Recap | Preset::Dynamic) && !seq.reels;
        let duo_ok = |p: &Placed| !matches!(p.clip.motion, Motion::Still | Motion::Zoom) && seq.beats.get(p.clip.src).and_then(|b| b.image.as_deref()).and_then(aspect).is_some_and(vm::duo_aspect);
        if duo_look && duo_ok(cur) {
            let run = placed[..=cur_i].iter().rev().take_while(|p| duo_ok(p)).count();
            let slot = (run + 1) % 2;
            let partner = (run > 1).then(|| placed.get(cur_i - 1)).flatten().and_then(|p| seq.beats.get(p.clip.src)).and_then(|b| b.image.clone());
            for (path, s) in [(Some(image.clone()), slot), (partner, 1 - slot)] {
                let Some(path) = path else { continue };
                let a = aspect(&path).unwrap_or(0.6);
                let (x, y, w, h, vis) = vm::duo_box(a, fw, fh, s);
                out.push(div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(vis)).overflow_hidden().child(img(path).absolute().left_0().top_0().w(px(w)).h(px(h)).object_fit(ObjectFit::Fill)).into_any_element());
            }
        } else {
            out.push(img(image).absolute().inset_0().size_full().object_fit(ObjectFit::Contain).into_any_element());
        }
        // The action punch's white flash at the start of an action beat.
        let into = now.saturating_sub(cur.start_ms);
        if into < 400 {
            let text = cur.clip.text.clone().or_else(|| seq.beats.get(cur.clip.src).map(|b| b.text.clone())).unwrap_or_default();
            let punch = cur.clip.motion == Motion::Shake || (impact && cur.clip.motion == Motion::Auto && vm::is_impact(&text));
            if punch {
                out.push(div().absolute().inset_0().bg(video::paint(super::hex("#ffffff"), 0.55 * (1. - into as f32 / 400.))).into_any_element());
            }
        }
        if grade {
            out.push(div().absolute().inset_0().bg(video::paint(super::hex("#ff9a3c"), 0.06)).into_any_element());
        }
        if glow {
            out.push(div().absolute().inset_0().bg(video::paint(super::hex("#ffffff"), 0.04)).into_any_element());
        }
        if vignette {
            let shade = |angle: f32| linear_gradient(angle, linear_color_stop(video::paint(super::hex("#000000"), 0.55), 0.), linear_color_stop(video::paint(super::hex("#000000"), 0.), 1.));
            out.push(div().absolute().left_0().right_0().top_0().h(relative(0.22)).bg(shade(180.)).into_any_element());
            out.push(div().absolute().left_0().right_0().bottom_0().h(relative(0.22)).bg(shade(0.)).into_any_element());
            out.push(div().absolute().top_0().bottom_0().left_0().w(relative(0.16)).bg(shade(90.)).into_any_element());
            out.push(div().absolute().top_0().bottom_0().right_0().w(relative(0.16)).bg(shade(270.)).into_any_element());
        }
        out
    }
}

/// An image's width / height (cached; read from its header once).
fn aspect(path: &std::path::Path) -> Option<f32> {
    static CACHE: std::sync::Mutex<Option<HashMap<PathBuf, Option<f32>>>> = std::sync::Mutex::new(None);
    let mut g = CACHE.lock().ok()?;
    let map = g.get_or_insert_with(HashMap::new);
    if let Some(a) = map.get(path) {
        return *a;
    }
    let a = image::image_dimensions(path).ok().filter(|(_, h)| *h > 0).map(|(w, h)| w as f32 / h as f32);
    map.insert(path.to_path_buf(), a);
    a
}
