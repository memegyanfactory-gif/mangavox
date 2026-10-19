//! The Studio page. Left: downloaded chapters. Middle: the focused chapter moving through
//! Read → Transcript → Story → Audio → Video, with that phase's main content shown big.
//! Right (collapsible): the AI panel — the current run, the manga's character brain, chat.
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState, TextareaState};
use gpui_kit::component::scroll::{Scrollbar, ScrollbarAxis, ScrollbarHandle, ScrollbarMode};
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_recap::{ChapterRead, ChapterRef, ModelChoice, PhaseStatus, Prefs, Story, Transcript};
use mv_ui::{button, empty_state, error_state, icon, icon_button, size, ButtonKind, Route, Tokens};

use crate::data::{self, ChapterItem, MangaGroup};
use crate::run::{self, RunInfo};
use crate::service::{self, JobInfo, JobView};
use crate::setup_view::SetupView;

pub const MONO: &str = if cfg!(windows) {
    "Consolas"
} else if cfg!(target_os = "macos") {
    "Menlo"
} else {
    "DejaVu Sans Mono"
};
const POLL: Duration = Duration::from_millis(300);
const SAVE_DELAY: Duration = Duration::from_millis(700);
pub(crate) const LIBRARY_W: f32 = 272.;
pub(crate) const SIDE_W: f32 = 384.;

pub(crate) enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Phase {
    Read,
    Transcript,
    Story,
    Audio,
    Video,
}

impl Phase {
    pub const ALL: [Phase; 5] = [Phase::Read, Phase::Transcript, Phase::Story, Phase::Audio, Phase::Video];

    pub fn label(self) -> &'static str {
        match self {
            Phase::Read => "Read",
            Phase::Transcript => "Transcript",
            Phase::Story => "Story",
            Phase::Audio => "Audio",
            Phase::Video => "Video",
        }
    }

    /// Whether the focused chapter has finished this phase.
    pub fn done(self, st: &PhaseStatus) -> bool {
        match self {
            Phase::Read => st.total_pages > 0 && st.read_pages >= st.total_pages,
            Phase::Transcript => st.transcript,
            Phase::Story => st.story,
            Phase::Audio => st.audio_files > 0,
            Phase::Video => st.video.is_some(),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Side {
    Activity,
    Brain,
    Chat,
}

/// Everything loaded for the chapter shown in the middle.
pub(crate) struct Focus {
    pub item: ChapterItem,
    pub chref: ChapterRef,
    /// Page image and its width / height ratio.
    pub pages: Vec<(PathBuf, Option<f32>)>,
    pub read: Option<ChapterRead>,
    pub transcript: Option<Transcript>,
    pub story: Option<Story>,
    pub status: PhaseStatus,
}

pub(crate) struct Notice {
    pub error: bool,
    pub text: String,
}

pub struct StudioPage {
    pub(crate) setup: Entity<SetupView>,
    pub(crate) library: Load<Vec<MangaGroup>>,
    pub(crate) statuses: HashMap<String, PhaseStatus>,
    pub(crate) open_groups: HashSet<String>,
    pub(crate) selected: BTreeSet<String>,
    pub(crate) search: Entity<InputState>,
    pub(crate) focus: Option<Focus>,
    pub(crate) focus_id: Option<String>,
    pub(crate) phase: Phase,
    pub(crate) list: ListState,
    pub(crate) lib_scroll: ScrollHandle,
    pub(crate) side_scroll: ScrollHandle,
    list_key: (Option<String>, Phase, usize),
    // Editors for transcript pages / story segments, created as rows render.
    pub(crate) editors: HashMap<(Phase, usize), Entity<TextareaState>>,
    editor_subs: Vec<Subscription>,
    save_gen: u64,
    pub(crate) ask: Option<usize>,
    pub(crate) ask_input: Entity<InputState>,
    pub(crate) asking: Option<usize>,
    pub(crate) prefs: Prefs,
    pub(crate) lang: &'static str,
    pub(crate) previewing: Option<String>,
    pub(crate) jobs: Vec<JobInfo>,
    seen_jobs: u64,
    pub(crate) run: Option<RunInfo>,
    seen_run: u64,
    pub(crate) side_open: bool,
    pub(crate) side: Side,
    pub(crate) dock: Option<AnyView>,
    pub(crate) model_button: AnyView,
    pub(crate) usage_meter: AnyView,
    pub(crate) show_setup: bool,
    pub(crate) notice: Option<Notice>,
    pub(crate) renaming: Option<(String, Entity<InputState>)>,
    pub(crate) rename_sub: Option<Subscription>,
    _subs: Vec<Subscription>,
}

impl StudioPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let setup = cx.new(SetupView::new);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Filter manga"));
        let ask_input = cx.new(|cx| InputState::new(window, cx).placeholder("e.g. \"Ruko says the second line, not Kai\""));
        let mut subs = vec![cx.observe(&setup, |_, _, cx| cx.notify())];
        subs.push(cx.subscribe_in(&search, window, |_, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::Change) {
                cx.notify();
            }
        }));
        subs.push(cx.subscribe_in(&ask_input, window, |this: &mut Self, _, ev: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                if let Some(page) = this.ask {
                    this.rewrite_page(page, cx);
                }
            }
        }));
        let prefs = Prefs::load();
        let voice = prefs.voice.clone().filter(|v| mv_studio::tts::find_voice(v).is_some()).unwrap_or_else(|| "af_heart".into());
        let mut prefs = prefs;
        prefs.voice = Some(voice.clone());
        if !(0.5..=2.0).contains(&prefs.speed) {
            prefs.speed = 1.0;
        }
        let model_button = ui_assistant::model_picker(window, cx);
        let usage_meter = ui_assistant::usage_meter(window, cx);
        let mut page = Self {
            setup,
            library: Load::Loading,
            statuses: HashMap::new(),
            open_groups: HashSet::new(),
            selected: BTreeSet::new(),
            search,
            focus: None,
            focus_id: None,
            phase: Phase::Read,
            list: ListState::new(0, ListAlignment::Top, px(800.)),
            lib_scroll: ScrollHandle::new(),
            side_scroll: ScrollHandle::new(),
            list_key: (None, Phase::Read, 0),
            editors: HashMap::new(),
            editor_subs: vec![],
            save_gen: 0,
            ask: None,
            ask_input,
            asking: None,
            lang: lang_of(&voice),
            prefs,
            previewing: None,
            jobs: service::snapshot(),
            seen_jobs: service::version(),
            run: run::snapshot(),
            seen_run: run::version(),
            side_open: true,
            side: Side::Activity,
            dock: None,
            model_button,
            usage_meter,
            show_setup: false,
            notice: None,
            renaming: None,
            rename_sub: None,
            _subs: subs,
        };
        page.reload_library(cx);
        page.poll(cx);
        page
    }

    // ------------------------------------------------------------ loading

    pub(crate) fn reload_library(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(|| {
                let groups = data::load()?;
                let statuses: HashMap<String, PhaseStatus> =
                    groups.iter().flat_map(|g| g.chapters.iter()).map(|c| (c.id.clone(), mv_recap::chapter_status(&chref(c)))).collect();
                anyhow::Ok((groups, statuses))
            })
            .await
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok((groups, statuses)) => {
                        if this.open_groups.is_empty() {
                            if let Some(g) = groups.first() {
                                this.open_groups.insert(g.id.clone());
                            }
                        }
                        this.statuses = statuses;
                        // Keep the focused chapter's status in step with the library.
                        if let Some(f) = this.focus.as_mut() {
                            if let Some(st) = this.statuses.get(&f.item.id) {
                                f.status = st.clone();
                            }
                        }
                        this.library = Load::Ready(groups);
                    }
                    Err(e) => this.library = Load::Failed(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Shows `item` in the middle and loads its pages and phase data.
    pub(crate) fn focus_chapter(&mut self, item: ChapterItem, cx: &mut Context<Self>) {
        if self.focus_id.as_deref() == Some(item.id.as_str()) && self.focus.is_some() {
            return;
        }
        self.focus_id = Some(item.id.clone());
        self.editors.clear();
        self.editor_subs.clear();
        self.ask = None;
        self.load_focus(item, cx);
        cx.notify();
    }

    fn load_focus(&mut self, item: ChapterItem, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || {
                let r = chref(&item);
                let pages = mv_recap::list_pages(&item.pages_dir)
                    .into_iter()
                    .map(|p| {
                        let ratio = image::image_dimensions(&p).ok().filter(|(_, h)| *h > 0).map(|(w, h)| w as f32 / h as f32);
                        (p, ratio)
                    })
                    .collect();
                Focus {
                    read: mv_recap::load_read(&r),
                    transcript: mv_recap::load_transcript(&r),
                    story: mv_recap::load_story(&r),
                    status: mv_recap::chapter_status(&r),
                    pages,
                    chref: r,
                    item,
                }
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(f) = res {
                    if this.focus_id.as_deref() == Some(f.item.id.as_str()) {
                        this.statuses.insert(f.item.id.clone(), f.status.clone());
                        this.focus = Some(f);
                        this.update_assistant_context();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-reads the focused chapter's phase files (after a run or a pipeline job).
    pub(crate) fn refresh_focus(&mut self, keep_editors: bool, cx: &mut Context<Self>) {
        let Some(f) = &self.focus else { return };
        let r = f.chref.clone();
        let id = f.item.id.clone();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || (mv_recap::load_read(&r), mv_recap::load_transcript(&r), mv_recap::load_story(&r), mv_recap::chapter_status(&r))).await;
            let _ = this.update(cx, |this, cx| {
                let Ok((read, transcript, story, status)) = res else { return };
                if let Some(f) = this.focus.as_mut().filter(|f| f.item.id == id) {
                    f.read = read;
                    f.transcript = transcript;
                    f.story = story;
                    f.status = status.clone();
                    this.statuses.insert(id, status);
                    if !keep_editors {
                        this.editors.clear();
                        this.editor_subs.clear();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Follows the AI run and the narration queue while the page is alive.
    fn poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL).await;
            let (rv, jv) = (run::version(), service::version());
            let alive = this.update(cx, |this, cx| {
                let mut changed = false;
                if rv != this.seen_run {
                    this.seen_run = rv;
                    let prev = this.run.take();
                    let now = run::snapshot();
                    let was_running = prev.as_ref().is_some_and(|r| r.running());
                    let page_moved = prev.as_ref().map(|r| r.done) != now.as_ref().map(|r| r.done);
                    let on_focus = now.as_ref().and_then(|r| r.chapter_id.clone()) == this.focus_id;
                    let finished = was_running && !now.as_ref().is_some_and(|r| r.running());
                    this.run = now;
                    if finished {
                        this.refresh_focus(false, cx);
                        this.reload_library(cx);
                    } else if page_moved && on_focus {
                        // Show each page's reading as soon as it lands.
                        this.refresh_focus(true, cx);
                    }
                    changed = true;
                }
                if jv != this.seen_jobs {
                    this.seen_jobs = jv;
                    let jobs = service::snapshot();
                    let done = |js: &[JobInfo]| js.iter().filter(|j| j.state == JobView::Done).count();
                    if done(&jobs) > done(&this.jobs) {
                        this.refresh_focus(true, cx);
                        this.reload_library(cx);
                    }
                    this.jobs = jobs;
                    changed = true;
                }
                // Keep the "0:42" timer of a running read ticking.
                let ticking = this.run.as_ref().is_some_and(|r| r.running() && !r.reading.is_empty());
                if changed || ticking {
                    cx.notify();
                }
            });
            if alive.is_err() {
                break;
            }
        })
        .detach();
    }

    // ------------------------------------------------------------ model + targets

    /// The model picked in the model button (the same one the Assistant uses).
    pub(crate) fn model(&self, cx: &mut App) -> Option<(ModelChoice, String)> {
        let (provider, model) = ui_assistant::assistant_model(cx);
        let provider: mv_ai::AiProviderId = serde_json::from_value(serde_json::Value::String(provider)).ok()?;
        Some((ModelChoice { provider, model }, ui_assistant::model_label(cx)))
    }

    /// Chapters a batch action works on: the checked ones, else the focused one.
    pub(crate) fn targets(&self) -> Vec<ChapterItem> {
        let Load::Ready(groups) = &self.library else { return vec![] };
        let all = groups.iter().flat_map(|g| g.chapters.iter());
        if self.selected.is_empty() {
            all.filter(|c| Some(&c.id) == self.focus_id.as_ref()).cloned().collect()
        } else {
            all.filter(|c| self.selected.contains(&c.id)).cloned().collect()
        }
    }

    pub(crate) fn target_label(&self, verb: &str) -> String {
        match self.selected.len() {
            0 => verb.to_string(),
            1 => format!("{verb} 1 chapter"),
            n => format!("{verb} {n} chapters"),
        }
    }

    pub(crate) fn say(&mut self, error: bool, text: impl Into<String>) {
        self.notice = Some(Notice { error, text: text.into() });
    }

    // ------------------------------------------------------------ AI actions

    pub(crate) fn start_read(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some((model, label)) = self.model(cx) else {
            self.say(true, "Pick an AI model first (model button at the top).");
            return cx.notify();
        };
        let targets = self.targets();
        let chapters: Vec<ChapterRef> = targets.iter().map(chref).collect();
        if chapters.is_empty() {
            self.say(true, "Pick a chapter on the left first.");
            return cx.notify();
        }
        // Reading chapters that are already fully read means "read them again".
        let all_read = targets.iter().all(|c| self.statuses.get(&c.id).is_some_and(|s| s.total_pages > 0 && s.read_pages >= s.total_pages));
        let force = force || all_read;
        if run::start_read(ui_assistant::registry(cx), model, label, chapters, force) {
            self.side_open = true;
            self.side = Side::Activity;
            self.notice = None;
        } else {
            self.say(true, "Another AI run is still going. Stop it first.");
        }
        cx.notify();
    }

    pub(crate) fn reread_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let (Some((model, label)), Some(f)) = (self.model(cx), &self.focus) else { return };
        if !run::start_read_pages(ui_assistant::registry(cx), model, label, f.chref.clone(), vec![page]) {
            self.say(true, "Another AI run is still going. Stop it first.");
        }
        cx.notify();
    }

    pub(crate) fn start_story(&mut self, cx: &mut Context<Self>) {
        let Some((model, label)) = self.model(cx) else {
            self.say(true, "Pick an AI model first (model button at the top).");
            return cx.notify();
        };
        self.flush_edits(cx);
        let chapters: Vec<ChapterRef> = self.targets().iter().filter(|c| self.statuses.get(&c.id).is_some_and(|s| s.read_pages > 0)).map(chref).collect();
        if chapters.is_empty() {
            self.say(true, "Read the chapter first: the story is written from what the AI read.");
            return cx.notify();
        }
        let styles: Vec<String> = self.prefs.styles.clone();
        if run::start_story(ui_assistant::registry(cx), model, label, chapters, styles) {
            self.side_open = true;
            self.side = Side::Activity;
            self.notice = None;
        } else {
            self.say(true, "Another AI run is still going. Stop it first.");
        }
        cx.notify();
    }

    /// Asks the AI to rewrite one transcript page following the instruction box.
    pub(crate) fn rewrite_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let instruction = self.ask_input.read(cx).value().to_string();
        if instruction.trim().is_empty() || self.asking.is_some() {
            return;
        }
        let (Some((model, _)), Some(f)) = (self.model(cx), &self.focus) else { return };
        let current = self.editor_text(Phase::Transcript, page, cx).unwrap_or_else(|| transcript_text(f, page));
        let ch = f.chref.clone();
        let brain = run::brain_snapshot(&ch.manga_id, &ch.manga_title);
        let reg = ui_assistant::registry(cx);
        self.asking = Some(page);
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(mv_recap::rewrite_page(reg, model, ch, page, current, instruction, brain, mv_ai::CancellationToken::new())).await;
            let _ = this.update(cx, |this, cx| {
                this.asking = None;
                match res.and_then(|r| r) {
                    Ok(text) => {
                        this.set_transcript_page(page, text.trim().to_string(), cx);
                        this.ask = None;
                    }
                    Err(e) => this.say(true, format!("The AI could not rewrite page {}: {e:#}", page + 1)),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    // ------------------------------------------------------------ editing

    pub(crate) fn editor(&mut self, phase: Phase, idx: usize, initial: &str, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextareaState> {
        if let Some(e) = self.editors.get(&(phase, idx)) {
            return e.clone();
        }
        let text = initial.to_string();
        let e = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx).auto_grow(2, 14);
            s.set_value(text, window, cx);
            s
        });
        self.editor_subs.push(cx.subscribe_in(&e, window, |this: &mut Self, _, ev: &InputEvent, _, cx| match ev {
            InputEvent::Change => this.schedule_save(cx),
            InputEvent::Blur => this.flush_edits(cx),
            _ => {}
        }));
        self.editors.insert((phase, idx), e.clone());
        e
    }

    fn editor_text(&self, phase: Phase, idx: usize, cx: &App) -> Option<String> {
        self.editors.get(&(phase, idx)).map(|e| e.read(cx).value().to_string())
    }

    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        self.save_gen += 1;
        let gen = self.save_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                if this.save_gen == gen {
                    this.flush_edits(cx);
                }
            });
        })
        .detach();
    }

    /// Writes changed transcript pages / story segments to disk (story edits are re-exported
    /// for the audio pipeline).
    pub(crate) fn flush_edits(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.focus.as_mut() else { return };
        let mut transcript_changed = false;
        let mut story_changed = false;
        for ((phase, idx), e) in &self.editors {
            let value = e.read(cx).value().to_string();
            match phase {
                Phase::Transcript => {
                    let t = f.transcript.get_or_insert_with(Transcript::default);
                    match t.pages.iter_mut().find(|p| p.index == *idx) {
                        Some(p) if p.text != value => {
                            p.text = value;
                            p.edited = true;
                            transcript_changed = true;
                        }
                        Some(_) => {}
                        None if !value.trim().is_empty() => {
                            t.pages.push(mv_recap::TranscriptPage { index: *idx, text: value, edited: true });
                            t.pages.sort_by_key(|p| p.index);
                            transcript_changed = true;
                        }
                        None => {}
                    }
                }
                Phase::Story => {
                    if let Some(seg) = f.story.as_mut().and_then(|s| s.segments.get_mut(*idx)) {
                        if seg.text != value {
                            seg.text = value;
                            story_changed = true;
                        }
                    }
                }
                _ => {}
            }
        }
        if !transcript_changed && !story_changed {
            return;
        }
        let r = f.chref.clone();
        let transcript = transcript_changed.then(|| f.transcript.clone()).flatten();
        let story = story_changed.then(|| f.story.clone()).flatten();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || -> anyhow::Result<()> {
                if let Some(t) = transcript {
                    mv_recap::save_transcript(&r, &t)?;
                }
                if let Some(s) = story {
                    mv_recap::save_story(&r, &s)?;
                    mv_recap::export_for_pipeline(&r, &s)?;
                }
                Ok(())
            })
            .await
            .and_then(|r| r);
            if let Err(e) = res {
                let _ = this.update(cx, |this, cx| {
                    this.say(true, format!("Could not save your edit: {e:#}"));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn set_transcript_page(&mut self, page: usize, text: String, cx: &mut Context<Self>) {
        if let Some(f) = self.focus.as_mut() {
            let t = f.transcript.get_or_insert_with(Transcript::default);
            match t.pages.iter_mut().find(|p| p.index == page) {
                Some(p) => {
                    p.text = text;
                    p.edited = true;
                }
                None => t.pages.push(mv_recap::TranscriptPage { index: page, text, edited: true }),
            }
            let (r, t) = (f.chref.clone(), t.clone());
            cx.background_executor().spawn(async move { mv_recap::save_transcript(&r, &t) }).detach();
        }
        // Rebuild the editor from the saved text on the next render.
        self.editors.remove(&(Phase::Transcript, page));
    }

    /// Restores a page to what the AI read (drops the user's edit).
    pub(crate) fn reset_transcript_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let Some(f) = self.focus.as_mut() else { return };
        let Some(read) = &f.read else { return };
        let fresh = mv_recap::transcript_from_read(read);
        let Some(orig) = fresh.pages.into_iter().find(|p| p.index == page) else { return };
        let t = f.transcript.get_or_insert_with(Transcript::default);
        t.pages.retain(|p| p.index != page);
        t.pages.push(orig);
        t.pages.sort_by_key(|p| p.index);
        let (r, t) = (f.chref.clone(), t.clone());
        cx.background_executor().spawn(async move { mv_recap::save_transcript(&r, &t) }).detach();
        self.editors.remove(&(Phase::Transcript, page));
        cx.notify();
    }

    pub(crate) fn update_assistant_context(&self) {
        let ctx = self.focus.as_ref().map(|f| {
            let mut s = format!(
                "The user is in MangaVox Studio working on \"{}\" {} ({} pages), phase: {}.",
                f.item.manga_title,
                f.item.label(),
                f.pages.len(),
                self.phase.label()
            );
            if let Some(p) = self.ask {
                s.push_str(&format!(" They are editing page {}: {}", p + 1, transcript_text(f, p)));
            }
            s
        });
        ui_assistant::set_studio_context(ctx);
    }

    pub(crate) fn set_phase(&mut self, phase: Phase, cx: &mut Context<Self>) {
        if self.phase != phase {
            self.flush_edits(cx);
            self.phase = phase;
            self.ask = None;
            self.update_assistant_context();
            cx.notify();
        }
    }

    // ------------------------------------------------------------ layout pieces

    fn toolbar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Room left for the steps once the side columns are drawn; narrow windows show only
        // the active step's name so the usage meter and setup stay visible.
        let center = f32::from(window.viewport_size().width) - 60. - LIBRARY_W - if self.side_open { SIDE_W } else { 0. };
        let compact = center < 820.;
        let status = self.focus.as_ref().map(|f| f.status.clone()).unwrap_or_default();
        let steps = div().flex().items_center().gap(px(2.)).children(Phase::ALL.iter().enumerate().map(|(i, &p)| {
            let active = self.phase == p;
            let done = self.focus.is_some() && p.done(&status);
            let fg = if active { Tokens::text_1() } else { Tokens::text_3() };
            div()
                .flex()
                .items_center()
                .gap(px(2.))
                .when(i > 0, |d| d.child(div().w(px(8.)).h(px(1.)).bg(Tokens::border())))
                .child(
                    div()
                        .id(SharedString::from(format!("phase-{}", p.label())))
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .h(px(30.))
                        .px(px(10.))
                        .rounded(px(8.))
                        .cursor_pointer()
                        .text_size(px(size::SM))
                        .text_color(fg)
                        .when(active, |d| d.bg(Tokens::active()))
                        .hover(|s| s.text_color(Tokens::text_1()))
                        .child(
                            div()
                                .size(px(18.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .text_size(px(10.))
                                .font_weight(FontWeight::BOLD)
                                .map(|d| {
                                    if done {
                                        d.bg(Tokens::accent()).text_color(Tokens::base()).child(icon("icons/check.svg", Tokens::base(), 11.))
                                    } else if active {
                                        d.bg(Tokens::text_1()).text_color(Tokens::base()).child(format!("{}", i + 1))
                                    } else {
                                        d.border_1().border_color(Tokens::border()).child(format!("{}", i + 1))
                                    }
                                }),
                        )
                        .when(!compact || active, |d| d.child(p.label()))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_phase(p, cx))),
                )
        }));
        let issues = self.setup.read(cx).issues();
        let checked = self.setup.read(cx).env.is_some();
        let setup_pill = div()
            .id("studio-setup-pill")
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .px(px(10.))
            .rounded(px(8.))
            .cursor_pointer()
            .text_size(px(size::XS))
            .border_1()
            .border_color(Tokens::border())
            .when(self.show_setup, |d| d.bg(Tokens::active()))
            .hover(|s| s.bg(Tokens::hover()))
            .map(|d| match (checked, issues) {
                (false, _) => d.text_color(Tokens::text_3()).child(icon("icons/loader-circle.svg", Tokens::text_3(), 13.)).child("Checking"),
                (true, 0) => d.text_color(Tokens::text_2()).child(icon("icons/circle-check.svg", Tokens::accent(), 13.)).child("Ready"),
                (true, n) => d.text_color(Tokens::warning()).child(icon("icons/triangle-alert.svg", Tokens::warning(), 13.)).child(format!("Setup · {n}")),
            })
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.show_setup = !this.show_setup;
                cx.notify();
            }));
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .h(px(52.))
            .px(px(14.))
            .border_b_1()
            .border_color(Tokens::border())
            .child(div().flex_1().min_w_0().overflow_hidden().child(steps))
            .child(self.usage_meter.clone())
            .child(setup_pill)
            .child(
                icon_button("studio-side-toggle", if self.side_open { "icons/panel-right-close.svg" } else { "icons/panel-right.svg" }, true).on_click(cx.listener(
                    |this, _: &ClickEvent, _, cx| {
                        this.side_open = !this.side_open;
                        cx.notify();
                    },
                )),
            )
    }

    fn library_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let filter = self.search.read(cx).value().to_lowercase();
        let body: AnyElement = match &self.library {
            Load::Loading => div().p(px(14.)).text_size(px(size::SM)).text_color(Tokens::text_3()).child("Loading your chapters…").into_any_element(),
            Load::Failed(e) => div()
                .flex()
                .flex_col()
                .p(px(14.))
                .gap(px(8.))
                .child(error_state(e.clone()))
                .child(button("studio-lib-retry", "Retry", ButtonKind::Secondary).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.library = Load::Loading;
                    this.reload_library(cx);
                    cx.notify();
                })))
                .into_any_element(),
            Load::Ready(groups) if groups.is_empty() => div()
                .flex()
                .flex_col()
                .items_center()
                .p(px(14.))
                .gap(px(10.))
                .child(empty_state("Nothing downloaded", "Studio works on chapters you've downloaded. Open a manga and download a chapter."))
                .child(button("studio-open-library", "Open Library", ButtonKind::Primary).on_click(|_, w, cx| mv_ui::navigate(Route::Library, w, cx)))
                .into_any_element(),
            Load::Ready(groups) => div()
                .relative()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .id("studio-lib-list")
                        .flex()
                        .flex_col()
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.lib_scroll)
                        .pl(px(8.))
                        .pr(px(14.))
                        .pb(px(8.))
                        .children(groups.iter().filter(|g| filter.is_empty() || g.title.to_lowercase().contains(&filter)).map(|g| self.group_view(g, cx))),
                )
                .child(vbar("studio-lib-bar", &self.lib_scroll))
                .into_any_element(),
        };
        let total: usize = match &self.library {
            Load::Ready(g) => g.iter().map(|g| g.chapters.len()).sum(),
            _ => 0,
        };
        let n = self.selected.len();
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(LIBRARY_W))
            .h_full()
            .map(panel)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .p(px(12.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(div().flex_1().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).child("Downloaded"))
                            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{total} chapters"))),
                    )
                    .child(Input::new(&self.search).small().prefix(icon("icons/search.svg", Tokens::text_3(), 13.))),
            )
            .child(body)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .h(px(40.))
                    .px(px(12.))
                    .border_t_1()
                    .border_color(Tokens::border())
                    .text_size(px(size::XS))
                    .text_color(Tokens::text_3())
                    .child(div().flex_1().child(if n == 0 { "Tick chapters to batch".to_string() } else { format!("{n} selected") }))
                    .when(n > 0, |d| {
                        d.child(button("studio-clear-sel", "Clear", ButtonKind::Ghost).h(px(24.)).px(px(8.)).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.selected.clear();
                            cx.notify();
                        })))
                    }),
            )
    }

    fn group_view(&self, g: &MangaGroup, cx: &mut Context<Self>) -> Div {
        let open = self.open_groups.contains(&g.id);
        let gid = g.id.clone();
        let all_ids: Vec<String> = g.chapters.iter().map(|c| c.id.clone()).collect();
        let all_on = all_ids.iter().all(|id| self.selected.contains(id));
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(SharedString::from(format!("grp-{}", g.id)))
                    .group("grp")
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(56.))
                    .px(px(6.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .when(open, |d| d.bg(Tokens::hover()))
                    .hover(|s| s.bg(Tokens::hover()))
                    .child(
                        div()
                            .w(px(34.))
                            .h(px(46.))
                            .flex_none()
                            .rounded(px(5.))
                            .overflow_hidden()
                            .bg(Tokens::surface_2())
                            .flex()
                            .items_center()
                            .justify_center()
                            .map(|d| match &g.cover {
                                Some(p) => d.child(img(p.clone()).size_full().rounded(px(5.)).object_fit(ObjectFit::Cover)),
                                None => d.child(icon("icons/book-open.svg", Tokens::text_3(), 14.)),
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(g.title.clone()))
                            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{} chapter{}", g.chapters.len(), plural(g.chapters.len())))),
                    )
                    .child(icon(if open { "icons/chevron-down.svg" } else { "icons/chevron-right.svg" }, Tokens::text_3(), 13.))
                    .child(
                        div()
                            .id(SharedString::from(format!("grp-all-{}", g.id)))
                            .invisible()
                            .group_hover("grp", |s| s.visible())
                            .px(px(5.))
                            .rounded(px(5.))
                            .text_size(px(10.))
                            .text_color(Tokens::text_3())
                            .hover(|s| s.bg(Tokens::active()).text_color(Tokens::text_1()))
                            .child(if all_on { "None" } else { "All" })
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                if all_on {
                                    for id in &all_ids {
                                        this.selected.remove(id);
                                    }
                                } else {
                                    this.selected.extend(all_ids.iter().cloned());
                                }
                                cx.notify();
                            })),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if !this.open_groups.remove(&gid) {
                            this.open_groups.insert(gid.clone());
                        }
                        cx.notify();
                    })),
            )
            .when(open, |d| d.children(g.chapters.iter().map(|c| self.chapter_row(c, cx))))
    }

    fn chapter_row(&self, c: &ChapterItem, cx: &mut Context<Self>) -> Stateful<Div> {
        let checked = self.selected.contains(&c.id);
        let focused = self.focus_id.as_deref() == Some(c.id.as_str());
        let st = self.statuses.get(&c.id).cloned().unwrap_or_default();
        let running = self.run.as_ref().is_some_and(|r| r.running() && r.chapter_id.as_deref() == Some(c.id.as_str()))
            || self.jobs.iter().any(|j| j.chapter_id == c.id && !j.state.finished());
        let id = c.id.clone();
        let item = c.clone();
        div()
            .id(SharedString::from(format!("ch-{}", c.id)))
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(32.))
            .pl(px(48.))
            .pr(px(6.))
            .rounded(px(7.))
            .cursor_pointer()
            .when(focused, |d| d.bg(Tokens::active()))
            .when(!focused, |d| d.hover(|s| s.bg(Tokens::hover())))
            .child(
                div()
                    .id(SharedString::from(format!("chk-{}", c.id)))
                    .flex_none()
                    .child(icon(if checked { "icons/square-check.svg" } else { "icons/square.svg" }, if checked { Tokens::accent() } else { Tokens::text_3() }, 14.))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        if !this.selected.remove(&id) {
                            this.selected.insert(id.clone());
                        }
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(size::SM))
                    .text_color(if focused { Tokens::text_1() } else { Tokens::text_2() })
                    .child(c.label()),
            )
            .when(running, |d| d.child(icon("icons/loader-circle.svg", Tokens::accent(), 12.)))
            .child(div().flex().gap(px(3.)).children(Phase::ALL.map(|p| {
                let on = p.done(&st);
                div().size(px(5.)).rounded_full().bg(if on { Tokens::accent() } else { Tokens::active() })
            })))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.focus_chapter(item.clone(), cx)))
    }

    fn notice_bar(&self, cx: &mut Context<Self>) -> Option<Div> {
        let n = self.notice.as_ref()?;
        let color = if n.error { Tokens::danger() } else { Tokens::accent() };
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .mx(px(20.))
                .mt(px(10.))
                .px(px(12.))
                .py(px(8.))
                .rounded(px(8.))
                .bg(Tokens::surface())
                .border_1()
                .border_color(color)
                .child(icon(if n.error { "icons/circle-x.svg" } else { "icons/circle-check.svg" }, color, 14.))
                .child(div().flex_1().text_size(px(size::SM)).text_color(Tokens::text_1()).child(n.text.clone()))
                .child(icon_button("studio-notice-x", "icons/x.svg", true).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.notice = None;
                    cx.notify();
                }))),
        )
    }

    /// Keeps the virtual list's item count in step with what the phase shows.
    pub(crate) fn sync_list(&mut self, count: usize) {
        let key = (self.focus_id.clone(), self.phase, count);
        if key != self.list_key {
            let same_view = key.0 == self.list_key.0 && key.1 == self.list_key.1;
            if same_view {
                let old = self.list_key.2;
                self.list.splice(0..old, count);
            } else {
                self.list.reset(count);
            }
            self.list_key = key;
        }
    }
}

impl Render for StudioPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let center = self.center(window, cx);
        let side = self.side_open.then(|| self.side_panel(window, cx));
        div()
            .flex()
            .size_full()
            .gap(px(6.))
            .p(px(6.))
            .bg(rgb(0x07080a))
            .child(self.library_panel(cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .map(panel)
                    .child(self.toolbar(window, cx))
                    .children(self.notice_bar(cx))
                    .when(self.show_setup, |d| {
                        d.child(
                            div()
                                .id("studio-setup")
                                .flex_none()
                                .max_h(px(420.))
                                .overflow_y_scroll()
                                .mx(px(20.))
                                .mt(px(12.))
                                .p(px(16.))
                                .rounded(px(12.))
                                .bg(Tokens::surface())
                                .border_1()
                                .border_color(Tokens::border())
                                .child(self.setup.clone()),
                        )
                    })
                    .child(center),
            )
            .children(side)
    }
}

// ---------------------------------------------------------------- helpers

/// A Studio column: rounded card with a soft light sheen along its top edge.
pub(crate) fn panel(d: Div) -> Div {
    d.relative()
        .rounded(px(12.))
        .overflow_hidden()
        .bg(Tokens::sidebar())
        .border_1()
        .border_color(Tokens::border())
        .child(div().absolute().top_0().left(px(14.)).right(px(14.)).h(px(1.)).bg(rgba(0xffffff1f)))
}

/// An always-visible vertical scrollbar laid over a `relative` scroll area, so long lists
/// can be dragged from top to bottom.
pub(crate) fn vbar<H: ScrollbarHandle + Clone>(id: &'static str, h: &H) -> Div {
    div().absolute().inset_0().child(Scrollbar::new(h).id(id).axis(ScrollbarAxis::Vertical).mode(ScrollbarMode::Always).viewport_from_layout())
}

pub(crate) fn chref(c: &ChapterItem) -> ChapterRef {
    ChapterRef {
        chapter_id: c.id.clone(),
        manga_id: c.manga_id.clone(),
        manga_title: c.manga_title.clone(),
        label: c.label(),
        pages_dir: c.pages_dir.clone(),
        output_dir: c.output_dir.clone(),
    }
}

/// The transcript text of `page` (empty when not read yet).
pub(crate) fn transcript_text(f: &Focus, page: usize) -> String {
    f.transcript.as_ref().and_then(|t| t.pages.iter().find(|p| p.index == page)).map(|p| p.text.clone()).unwrap_or_default()
}

pub(crate) fn lang_of(voice: &str) -> &'static str {
    match mv_studio::tts::find_voice(voice).map(|v| v.lang) {
        Some("hi") => "hi",
        _ => "en",
    }
}

pub(crate) fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}
