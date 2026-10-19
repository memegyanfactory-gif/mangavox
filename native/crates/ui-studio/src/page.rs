//! The Studio page. Left: downloaded chapters. Middle: the focused chapter moving through
//! Read → Transcript → Story → Audio → Music → Video, with that phase's main content shown big.
//! Right (collapsible): the AI panel — the current run, the manga's character brain, chat.
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use gpui_kit::component::input::{Input, InputEvent, InputState, TextareaState};
use gpui_kit::component::scroll::{Scrollbar, ScrollbarAxis, ScrollbarHandle, ScrollbarMode};
use gpui_kit::component::slider::{SliderEvent, SliderState};
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_recap::{ChapterRead, ChapterRef, ModelChoice, PhaseStatus, Prefs, Step, Story, Transcript};
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{
    btn, checkbox, dot_trail, empty_view, error_view, fmt_bytes, glyphs, icon_button, icon_ph, inline_alert, interactive, list_row, ph, ph_icon, size, skeleton, skeleton_line, spinner,
    stepper, tip, AlertKind, ButtonKind, CheckState, Route, Tokens, Weight,
};

use crate::data::{self, ChapterItem, MangaGroup};
use crate::run::{self, RunInfo, RunKind, RunState};
use crate::service::{self, JobInfo, JobView};
use crate::setup_view::SetupView;
use crate::widgets::{column, open_reset_dialog, phase_icon, wav_secs, ResetDialog};

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
    Music,
    Video,
}

impl Phase {
    pub const ALL: [Phase; 6] = [Phase::Read, Phase::Transcript, Phase::Story, Phase::Audio, Phase::Music, Phase::Video];

    pub fn label(self) -> &'static str {
        self.step().label()
    }

    pub fn index(self) -> usize {
        self.step().index()
    }

    /// The mv-recap step this phase shows (what a reset clears).
    pub fn step(self) -> Step {
        match self {
            Phase::Read => Step::Read,
            Phase::Transcript => Step::Transcript,
            Phase::Story => Step::Story,
            Phase::Audio => Step::Audio,
            Phase::Music => Step::Music,
            Phase::Video => Step::Video,
        }
    }

    pub fn of(step: Step) -> Phase {
        Phase::ALL[step.index()]
    }

    /// Whether the focused chapter has finished this phase.
    pub fn done(self, st: &PhaseStatus) -> bool {
        match self {
            Phase::Read => st.total_pages > 0 && st.read_pages >= st.total_pages,
            Phase::Transcript => st.transcript,
            Phase::Story => st.story,
            Phase::Audio => st.audio_files > 0,
            Phase::Music => st.music,
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

/// Who narrated the chapter's clips (`audio/narration_params.json`, written by tts_bridge).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NarratedBy {
    pub engine: String,
    pub voice: String,
    pub synth_voice: String,
    /// Chatterbox would not start, so the stand-in narrated.
    pub chatterbox_error: Option<String>,
}

impl NarratedBy {
    fn load(output_dir: &std::path::Path) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(output_dir.join("audio").join("narration_params.json")).ok()?).ok()?;
        let s = |k: &str| v.get(k).and_then(serde_json::Value::as_str).map(str::to_string);
        Some(Self { engine: s("engine")?, voice: s("voice").unwrap_or_default(), synth_voice: s("synthVoice").unwrap_or_default(), chatterbox_error: s("chatterboxError") })
    }
}

/// The rendered video's file facts, read once per load.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct VideoMeta {
    pub bytes: u64,
    pub modified: Option<SystemTime>,
    /// Narration length (the video follows it exactly).
    pub duration: Option<f32>,
    /// Each reel's length, by file name (`reels/reels.json`).
    pub reels: HashMap<String, f32>,
    /// Rendered upright (9:16): an upright render leaves its `reels/` folder, a landscape one
    /// removes it (video_generator.py), whatever the Format setting says now.
    pub upright: bool,
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
    /// Length of each page clip, in seconds.
    pub clips: HashMap<usize, f32>,
    pub narrated: Option<NarratedBy>,
    pub video: VideoMeta,
    /// The take each page is narrated with (`audio/takes.json`; 0 = the original).
    pub takes: HashMap<usize, u32>,
}

/// What the files on disk say about a chapter's audio and video.
pub(crate) struct Media {
    clips: HashMap<usize, f32>,
    narrated: Option<NarratedBy>,
    video: VideoMeta,
    takes: HashMap<usize, u32>,
}

/// Reads [`Media`] for a chapter (blocking).
fn load_media(r: &ChapterRef, status: &PhaseStatus) -> Media {
    let audio = r.output_dir.join("audio");
    let clips: HashMap<usize, f32> = std::fs::read_dir(&audio)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let page = name.strip_prefix("page_")?.strip_suffix(".wav")?.parse::<usize>().ok()?;
            Some((page, wav_secs(&e.path())?))
        })
        .collect();
    let mut video = VideoMeta::default();
    if let Some(v) = &status.video {
        let meta = std::fs::metadata(v).ok();
        video.bytes = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        video.modified = meta.and_then(|m| m.modified().ok());
        let total: f32 = clips.values().sum();
        video.duration = (total > 0.).then_some(total);
        video.upright = r.output_dir.join("reels").is_dir();
        if let Some(json) = std::fs::read(r.output_dir.join("reels").join("reels.json")).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) {
            for reel in json.get("reels").and_then(|v| v.as_array()).into_iter().flatten() {
                let (Some(file), Some(a), Some(b)) = (reel.get("file").and_then(|v| v.as_str()), reel.get("start").and_then(|v| v.as_f64()), reel.get("end").and_then(|v| v.as_f64())) else { continue };
                video.reels.insert(file.to_string(), (b - a) as f32);
            }
        }
    }
    let takes = std::fs::read(mv_studio::voices::takes_path(&r.output_dir))
        .ok()
        .and_then(|b| serde_json::from_slice::<HashMap<String, u32>>(&b).ok())
        .map(|m| m.into_iter().filter_map(|(k, v)| Some((k.parse().ok()?, v))).collect())
        .unwrap_or_default();
    Media { clips, narrated: NarratedBy::load(&r.output_dir), video, takes }
}

/// A problem shown above the work until it is fixed or dismissed, with the one action that
/// fixes it (NN/g error messages: say what is wrong and offer the fix).
pub(crate) struct Notice {
    pub text: String,
    pub action: Option<NoticeAction>,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum NoticeAction {
    OpenSetup,
    OpenAudioSettings,
    StopRun,
    GoTo(Phase),
}

impl NoticeAction {
    fn label(self) -> &'static str {
        match self {
            NoticeAction::OpenSetup => "Open Setup",
            NoticeAction::OpenAudioSettings => "Open Audio settings",
            NoticeAction::StopRun => "Stop the run",
            NoticeAction::GoTo(Phase::Read) => "Go to Read",
            NoticeAction::GoTo(Phase::Story) => "Go to Story",
            NoticeAction::GoTo(Phase::Audio) => "Go to Audio",
            NoticeAction::GoTo(_) => "Go there",
        }
    }
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
    /// Editors the user typed in since the last save. Only these are written back: an editor
    /// made before the AI read its page still holds the old text and must never overwrite it.
    dirty: HashSet<(Phase, usize)>,
    save_gen: u64,
    pub(crate) ask: Option<usize>,
    pub(crate) ask_input: Entity<InputState>,
    /// (chapter, page) the AI is rewriting.
    pub(crate) asking: Option<(String, usize)>,
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
    /// Saved place restored (the library loaded); until then nothing overwrites it.
    restored: bool,
    pub(crate) renaming: Option<(String, Entity<InputState>)>,
    pub(crate) rename_sub: Option<Subscription>,
    pub(crate) music: crate::music::MusicState,
    /// The focused chapter's emotion strength slider (0-2) and the chapter it shows.
    pub(crate) strength: Entity<SliderState>,
    pub(crate) strength_value: f32,
    pub(crate) strength_for: Option<String>,
    /// (chapter, page) a per-page Regenerate / New take is voicing.
    pub(crate) regenerating: Option<(String, usize)>,
    /// Width of the middle column at the last render (Read rows stack below 720px).
    pub(crate) center_w: f32,
    /// Studio's own focus, so its shortcuts work after a click anywhere on the page (kit
    /// controls do not take focus on click).
    focus_handle: FocusHandle,
    _subs: Vec<Subscription>,
}

impl StudioPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let setup = cx.new(SetupView::new);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Filter manga"));
        let ask_input = cx.new(|cx| InputState::new(window, cx).placeholder("e.g. \"Ruko says the second line, not Kai\""));
        let strength = cx.new(|_| SliderState::new().min(0.).max(2.).step(0.1).default_value(mv_studio::voices::EMOTION_STRENGTH_DEFAULT));
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
            if matches!(ev, InputEvent::Change) {
                cx.notify();
            }
        }));
        subs.push(cx.subscribe_in(&strength, window, |this: &mut Self, _, ev: &SliderEvent, _, cx| match ev {
            SliderEvent::Change(v) => {
                this.strength_value = v.start();
                cx.notify();
            }
            SliderEvent::Release(v) => this.save_strength(v.start(), cx),
        }));
        let prefs = Prefs::load();
        // Default narrator chosen in Settings → Audio (same file, plus config.json fallback).
        let voice = mv_studio::voices::default_voice();
        let mut prefs = prefs;
        prefs.voice = Some(voice.clone());
        if !(0.5..=2.0).contains(&prefs.speed) {
            prefs.speed = 1.0;
        }
        let model_button = ui_assistant::model_picker(window, cx);
        let usage_meter = ui_assistant::usage_meter(window, cx);
        // Studio's shortcuts (Alt+1 to Alt+6, Esc) work from the first frame: kit controls never
        // take focus on a click, so nothing else would hand it to the page.
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        let mut page = Self {
            setup,
            library: Load::Loading,
            statuses: HashMap::new(),
            open_groups: prefs.open_groups.iter().cloned().collect(),
            selected: prefs.selected.iter().cloned().collect(),
            search,
            focus: None,
            focus_id: None,
            phase: Phase::ALL.into_iter().find(|p| prefs.last_phase.as_deref() == Some(p.label())).unwrap_or(Phase::Read),
            list: ListState::new(0, ListAlignment::Top, px(800.)),
            lib_scroll: ScrollHandle::new(),
            side_scroll: ScrollHandle::new(),
            list_key: (None, Phase::Read, 0),
            editors: HashMap::new(),
            editor_subs: vec![],
            dirty: HashSet::new(),
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
            restored: false,
            renaming: None,
            rename_sub: None,
            music: Default::default(),
            strength,
            strength_value: mv_studio::voices::EMOTION_STRENGTH_DEFAULT,
            strength_for: None,
            regenerating: None,
            center_w: 1000.,
            focus_handle,
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
                        // Reopen the chapter the user had open last time.
                        if !this.restored {
                            this.restored = true;
                            let last = this.prefs.last_chapter.clone();
                            if let Some(item) = last.and_then(|id| groups.iter().flat_map(|g| g.chapters.iter()).find(|c| c.id == id).cloned()) {
                                if this.focus_id.is_none() {
                                    this.open_groups.insert(item.manga_id.clone());
                                    this.focus_chapter(item, cx);
                                }
                            }
                        }
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
        self.flush_edits(cx);
        self.focus_id = Some(item.id.clone());
        self.focus = None;
        self.editors.clear();
        self.editor_subs.clear();
        self.dirty.clear();
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
                let status = mv_recap::chapter_status(&r);
                let Media { clips, narrated, video, takes } = load_media(&r, &status);
                Focus {
                    read: mv_recap::load_read(&r),
                    transcript: mv_recap::load_transcript(&r),
                    story: mv_recap::load_story(&r),
                    status,
                    pages,
                    clips,
                    narrated,
                    video,
                    takes,
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
            let res = mv_ui::bg::blocking(move || {
                let status = mv_recap::chapter_status(&r);
                let media = load_media(&r, &status);
                (mv_recap::load_read(&r), mv_recap::load_transcript(&r), mv_recap::load_story(&r), status, media)
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                let Ok((read, transcript, story, status, Media { clips, narrated, video, takes })) = res else { return };
                // Passages are keyed by position: when the AI wrote a new story, an edit in
                // progress no longer belongs to the passage now at that position.
                let mut story_replaced = false;
                if let Some(f) = this.focus.as_mut().filter(|f| f.item.id == id) {
                    story_replaced = f.story != story;
                    f.read = read;
                    f.transcript = transcript;
                    f.story = story;
                    f.status = status.clone();
                    f.clips = clips;
                    f.narrated = narrated;
                    f.video = video;
                    f.takes = takes;
                    this.statuses.insert(id.clone(), status);
                }
                if let Some(f) = this.focus.as_ref().filter(|f| f.item.id == id) {
                    // An editor the user is typing in keeps its text (saved over the fresh data
                    // next). The others are rebuilt from the fresh data unless they already show
                    // it: one made before its page was read would keep the old text on screen.
                    let dirty = &this.dirty;
                    let typing = |k: &(Phase, usize)| dirty.contains(k) && !(story_replaced && k.0 == Phase::Story);
                    this.editors.retain(|k, e| typing(k) || (keep_editors && *e.read(cx).value() == *editor_source(f, *k)));
                    this.dirty.retain(|k| this.editors.contains_key(k));
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
                    if finished {
                        if let Some(r) = now.as_ref() {
                            run_finished_toast(r, cx);
                        }
                    }
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
                    let finished = |js: &[JobInfo]| js.iter().filter(|j| j.state.finished()).count();
                    if finished(&jobs) > finished(&this.jobs) {
                        for j in jobs.iter().filter(|j| j.state.finished() && this.jobs.iter().any(|o| o.id == j.id && !o.state.finished())) {
                            job_finished_toast(j, cx);
                        }
                        this.refresh_focus(true, cx);
                        this.reload_library(cx);
                    }
                    this.jobs = jobs;
                    if this.regenerating.as_ref().is_some_and(|(ch, _)| !this.jobs.iter().any(|j| &j.chapter_id == ch && !j.state.finished())) {
                        this.regenerating = None;
                    }
                    changed = true;
                }
                // Keep the "0:42" timer of a running read and a playing clip's line moving.
                let ticking = this.run.as_ref().is_some_and(|r| r.running() && !r.reading.is_empty()) || crate::audio::now_playing().is_some();
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

    /// A target has some pages read but not all: a stopped read to continue or restart.
    pub(crate) fn partly_read(&self) -> bool {
        self.targets().iter().any(|c| self.statuses.get(&c.id).is_some_and(|s| s.read_pages > 0 && s.read_pages < s.total_pages))
    }

    pub(crate) fn target_label(&self, verb: &str) -> String {
        match self.selected.len() {
            0 => verb.to_string(),
            1 => format!("{verb} 1 chapter"),
            n => format!("{verb} {n} chapters"),
        }
    }

    /// Why an AI step cannot start now (another run is going), if it cannot.
    pub(crate) fn run_blocker(&self) -> Option<&'static str> {
        self.run.as_ref().is_some_and(|r| r.running()).then_some("Another AI run is going. Stop it in Activity first.")
    }

    /// Why this chapter's files cannot be reset now, if they cannot.
    pub(crate) fn reset_blocker(&self, chapter_id: &str) -> Option<&'static str> {
        if self.run.as_ref().is_some_and(|r| r.running() && r.chapter_id.as_deref() == Some(chapter_id)) {
            return Some("Stop the AI run on this chapter first");
        }
        if self.jobs.iter().any(|j| j.chapter_id == chapter_id && !j.state.finished()) {
            return Some("Wait for this chapter's audio or video job to finish, or cancel it");
        }
        if self.music.busy.is_some() && self.music.composing.iter().any(|c| c == chapter_id) {
            return Some("Wait for the score to finish");
        }
        // A rewrite that lands after the reset would write the transcript back.
        if self.asking.as_ref().is_some_and(|(c, _)| c == chapter_id) {
            return Some("Wait for the AI to finish rewriting the page");
        }
        None
    }

    /// Saves where the user is (chapter, step, ticks, expanded manga) whenever it changed, so
    /// Studio reopens right there. Cheap to call every render.
    fn remember(&mut self) {
        if !self.restored {
            return;
        }
        let phase = Some(self.phase.label().to_string());
        let selected: Vec<String> = self.selected.iter().cloned().collect();
        let mut open: Vec<String> = self.open_groups.iter().cloned().collect();
        open.sort();
        let p = &self.prefs;
        if p.last_chapter == self.focus_id && p.last_phase == phase && p.selected == selected && p.open_groups == open {
            return;
        }
        self.prefs.last_chapter = self.focus_id.clone();
        self.prefs.last_phase = phase;
        self.prefs.selected = selected;
        self.prefs.open_groups = open;
        // Only these keys: Settings may have changed others (e.g. the default voice) meanwhile.
        let mut on_disk = Prefs::load();
        on_disk.last_chapter = self.prefs.last_chapter.clone();
        on_disk.last_phase = self.prefs.last_phase.clone();
        on_disk.selected = self.prefs.selected.clone();
        on_disk.open_groups = self.prefs.open_groups.clone();
        on_disk.save();
    }

    /// A problem that stays until fixed or dismissed.
    pub(crate) fn fail(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice { text: text.into(), action: None });
    }

    /// A problem plus the one action that fixes it.
    pub(crate) fn fail_with(&mut self, text: impl Into<String>, action: NoticeAction) {
        self.notice = Some(Notice { text: text.into(), action: Some(action) });
    }

    fn run_notice_action(&mut self, a: NoticeAction, window: &mut Window, cx: &mut Context<Self>) {
        self.notice = None;
        match a {
            NoticeAction::OpenSetup => self.show_setup = true,
            NoticeAction::OpenAudioSettings => mv_ui::navigate(Route::Settings { tab: Some("audio".into()) }, window, cx),
            NoticeAction::StopRun => run::cancel(),
            NoticeAction::GoTo(p) => self.set_phase(p, cx),
        }
        cx.notify();
    }

    // ------------------------------------------------------------ AI actions

    pub(crate) fn start_read(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some((model, label)) = self.model(cx) else {
            self.fail("Pick an AI model first (model button at the top).");
            return cx.notify();
        };
        let targets = self.targets();
        let chapters: Vec<ChapterRef> = targets.iter().map(chref).collect();
        if chapters.is_empty() {
            self.fail("Pick a chapter on the left first.");
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
            self.fail_with("Another AI run is still going.", NoticeAction::StopRun);
        }
        cx.notify();
    }

    pub(crate) fn reread_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let (Some((model, label)), Some(f)) = (self.model(cx), &self.focus) else { return };
        if !run::start_read_pages(ui_assistant::registry(cx), model, label, f.chref.clone(), vec![page]) {
            self.fail_with("Another AI run is still going.", NoticeAction::StopRun);
        }
        cx.notify();
    }

    pub(crate) fn start_story(&mut self, cx: &mut Context<Self>) {
        let Some((model, label)) = self.model(cx) else {
            self.fail("Pick an AI model first (model button at the top).");
            return cx.notify();
        };
        self.flush_edits(cx);
        let chapters: Vec<ChapterRef> = self.targets().iter().filter(|c| self.statuses.get(&c.id).is_some_and(|s| s.read_pages > 0)).map(chref).collect();
        if chapters.is_empty() {
            self.fail_with("Read the chapter first: the story is written from what the AI read.", NoticeAction::GoTo(Phase::Read));
            return cx.notify();
        }
        let styles: Vec<String> = self.prefs.styles.clone();
        if run::start_story(ui_assistant::registry(cx), model, label, chapters, styles) {
            self.side_open = true;
            self.side = Side::Activity;
            self.notice = None;
        } else {
            self.fail_with("Another AI run is still going.", NoticeAction::StopRun);
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
        let brain = (*run::brain_snapshot(&ch.manga_id, &ch.manga_title)).clone();
        let reg = ui_assistant::registry(cx);
        self.asking = Some((ch.chapter_id.clone(), page));
        let target = ch.clone();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(mv_recap::rewrite_page(reg, model, ch, page, current, instruction, brain, mv_ai::CancellationToken::new())).await;
            let _ = this.update(cx, |this, cx| {
                this.asking = None;
                match res.and_then(|r| r) {
                    // The chapter is still open: through the editor state, as before.
                    Ok(text) if this.focus_id.as_deref() == Some(target.chapter_id.as_str()) => {
                        this.set_transcript_page(page, text.trim().to_string(), cx);
                        this.ask = None;
                        toast::show(cx, Toast::success(format!("Page {} rewritten", page + 1)).icon(ph::MAGIC_WAND));
                    }
                    // Another chapter is open now: the text goes to the chapter it was asked for.
                    Ok(text) => {
                        let label = target.label.clone();
                        let text = text.trim().to_string();
                        cx.background_executor()
                            .spawn(async move {
                                let mut t = mv_recap::load_transcript(&target).unwrap_or_default();
                                match t.pages.iter_mut().find(|p| p.index == page) {
                                    Some(p) => {
                                        p.text = text;
                                        p.edited = true;
                                    }
                                    None => {
                                        t.pages.push(mv_recap::TranscriptPage { index: page, text, edited: true });
                                        t.pages.sort_by_key(|p| p.index);
                                    }
                                }
                                mv_recap::save_transcript(&target, &t)
                            })
                            .detach();
                        toast::show(cx, Toast::success(format!("Page {} of {label} rewritten", page + 1)).icon(ph::MAGIC_WAND));
                    }
                    Err(e) => this.fail(format!("The AI could not rewrite page {}: {e:#}", page + 1)),
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
        self.editor_subs.push(cx.subscribe_in(&e, window, move |this: &mut Self, _, ev: &InputEvent, _, cx| match ev {
            InputEvent::Change => {
                this.dirty.insert((phase, idx));
                this.schedule_save(cx)
            }
            InputEvent::Blur => {
                this.flush_edits(cx);
                cx.notify();
            }
            InputEvent::Focus => cx.notify(),
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
        // Only what the user typed: an untouched editor may hold text from before the AI read
        // its page, and writing that back would blank the page and mark it edited.
        let dirty = std::mem::take(&mut self.dirty);
        for ((phase, idx), e) in self.editors.iter().filter(|(k, _)| dirty.contains(*k)) {
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
        self.save_files(r, transcript, story, cx);
    }

    /// Saves a transcript and / or story off the UI thread (a story is re-exported for the
    /// audio pipeline).
    pub(crate) fn save_files(&mut self, r: ChapterRef, transcript: Option<Transcript>, story: Option<Story>, cx: &mut Context<Self>) {
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
                    this.fail(format!("Could not save your edit: {e:#}"));
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
        self.dirty.remove(&(Phase::Transcript, page));
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
        self.dirty.remove(&(Phase::Transcript, page));
        toast::show(cx, Toast::info(format!("Page {} is back to the AI's text", page + 1)).icon(ph::ARROW_U_UP_LEFT));
        cx.notify();
    }

    /// Sets (or clears) how the narrator delivers one story passage, and re-exports the story.
    pub(crate) fn set_delivery(&mut self, ix: usize, delivery: Option<mv_recap::Delivery>, cx: &mut Context<Self>) {
        self.flush_edits(cx);
        let Some(f) = self.focus.as_mut() else { return };
        let Some(seg) = f.story.as_mut().and_then(|s| s.segments.get_mut(ix)) else { return };
        if seg.delivery == delivery {
            return;
        }
        seg.delivery = delivery;
        let (r, story) = (f.chref.clone(), f.story.clone());
        self.save_files(r, None, story, cx);
        cx.notify();
    }

    /// Saves the focused chapter's emotion strength (the slider was released).
    fn save_strength(&mut self, v: f32, cx: &mut Context<Self>) {
        let Some(f) = &self.focus else { return };
        let v = (v * 10.).round() / 10.;
        self.strength_value = v;
        let (dir, label) = (f.item.output_dir.clone(), f.item.label());
        match mv_studio::voices::set_emotion_strength(&dir, v) {
            Ok(()) => {
                let weak = cx.entity().downgrade();
                let has_audio = f.status.audio_files > 0;
                let mut t = Toast::success(format!("Emotion strength {v:.1}× for {label}")).icon(ph::MASK_HAPPY);
                if has_audio {
                    t = t.action("Regenerate audio", move |_, cx| {
                        let _ = weak.update(cx, |this, cx| this.run_pipeline(&["tts"], true, cx));
                    });
                }
                toast::show(cx, t);
            }
            Err(e) => self.fail(format!("Could not save the emotion strength: {e}")),
        }
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

    // ------------------------------------------------------------ resets

    /// Asks before clearing `phase` and every later step of the focused chapter, listing what goes.
    pub(crate) fn confirm_reset_phase(&mut self, phase: Phase, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = &self.focus else { return };
        if let Some(why) = self.reset_blocker(&f.item.id) {
            toast::show(cx, Toast::warning(why));
            return;
        }
        let plan = mv_recap::reset_plan(&f.chref, phase.step());
        let notes = if phase == Phase::Read { chapter_notes(f) } else { 0 };
        if plan.is_empty() && notes == 0 {
            toast::show(cx, Toast::info(format!("{} has nothing to reset yet", phase.label())));
            return;
        }
        let label = f.item.label();
        let mut rows: Vec<(mv_ui::Ph, String, String)> =
            plan.steps.iter().map(|(s, fp)| (phase_icon(Phase::of(*s)), format!("{} · {}", s.label(), s.output_name()), fmt_bytes(fp.bytes))).collect();
        if notes > 0 {
            rows.push((ph::BRAIN, "Story notes from this chapter".into(), format!("{notes} line{}", plural(notes))));
        }
        let n = plan.steps.len();
        let later = plan.steps.iter().filter(|(s, _)| *s != phase.step()).count();
        let body = match (phase, later > 0) {
            (Phase::Transcript, true) => format!("Every page of {label} goes back to what the AI read. The steps built from the transcript go too; the pages and the characters stay."),
            (Phase::Transcript, false) => format!("Every page of {label} goes back to what the AI read."),
            (_, true) => format!("The later steps of {label} are built from {}, so they go too. The downloaded pages and the characters stay.", phase.label()),
            (_, false) => format!("This clears {} for {label}. The downloaded pages and the characters stay.", phase.label()),
        };
        let confirm = match n {
            0 | 1 => format!("Reset {}", phase.label()),
            n => format!("Reset {n} steps"),
        };
        let warning = (plan.edited_pages > 0).then(|| format!("Your edits to {} transcript page{} will be lost.", plan.edited_pages, plural(plan.edited_pages)));
        let weak = cx.entity().downgrade();
        let chapter = f.item.id.clone();
        let dialog = ResetDialog { title: format!("Reset {}?", phase.label()), body, rows, warning, confirm, cancel: "Keep my work".into() };
        open_reset_dialog(
            dialog,
            move |_, cx| {
                let _ = weak.update(cx, |this, cx| this.do_reset_phase(&chapter, phase, cx));
            },
            window,
            cx,
        );
    }

    /// Drops the in-memory work a reset is about to delete, so nothing writes it back. True when
    /// it stopped the player (the reset then waits for it to let go of the file).
    fn forget_open_work(&mut self, chapter_id: &str, output_dir: &std::path::Path) -> bool {
        // A clip or score of this chapter may be open in the player (the file goes); a clip of
        // another chapter keeps playing.
        let playing = crate::audio::now_playing();
        let stop = playing.as_ref().is_some_and(|p| self.focus_id.as_deref() == Some(chapter_id) || p.starts_with(output_dir));
        if stop {
            crate::audio::stop();
        }
        if self.focus_id.as_deref() == Some(chapter_id) {
            // Cancel pending autosaves and throw the editors away unsaved.
            self.save_gen += 1;
            self.editors.clear();
            self.editor_subs.clear();
            self.dirty.clear();
            self.ask = None;
        }
        if self.music.chapter.as_deref() == Some(chapter_id) {
            self.music.chapter = None;
        }
        // The emotion strength slider reads the chapter again (a chapter reset drops it).
        if self.strength_for.as_deref() == Some(chapter_id) {
            self.strength_for = None;
        }
        stop
    }

    fn do_reset_phase(&mut self, chapter_id: &str, phase: Phase, cx: &mut Context<Self>) {
        // The chapter the dialog was about (never another one that got the focus meanwhile).
        let Some(f) = self.focus.as_ref().filter(|f| f.item.id == chapter_id) else { return };
        let (ch, id, label) = (f.chref.clone(), f.item.id.clone(), f.item.label());
        // Something may have started on the chapter while the dialog was open.
        if let Some(why) = self.reset_blocker(&id) {
            toast::show(cx, Toast::warning(why));
            return;
        }
        let stopped = self.forget_open_work(&id, &ch.output_dir);
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || -> anyhow::Result<(mv_recap::ResetReport, usize)> {
                if stopped {
                    crate::audio::wait_stopped(Duration::from_millis(600));
                }
                let report = mv_recap::reset_phase(&ch, phase.step())?;
                let mut notes = 0;
                if phase == Phase::Read {
                    // The re-read writes them again; keeping both would repeat the story memory.
                    run::edit_brain(&ch.manga_id, &ch.manga_title, |b| notes = b.forget_chapter(&ch.label));
                }
                // Every step from this one is undone now, whatever had files (a mark can outlive
                // files removed by hand).
                let _ = data::mark_stages_reset(&id, phase.step().and_later());
                Ok((report, notes))
            })
            .await
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok((report, _)) => reset_toast(&report, &label, None, cx),
                    Err(e) => this.fail(format!("Could not reset {}: {e:#}", phase.label())),
                }
                this.refresh_focus(false, cx);
                this.reload_library(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Asks before making `item` brand new; measures what goes first (off the UI thread).
    pub(crate) fn confirm_reset_chapter(&mut self, item: ChapterItem, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(why) = self.reset_blocker(&item.id) {
            toast::show(cx, Toast::warning(why));
            return;
        }
        let r = chref(&item);
        let notes = {
            let brain = run::brain_snapshot(&item.manga_id, &item.manga_title);
            let prefix = format!("- [{} p", r.label);
            brain.notes.lines().filter(|l| l.starts_with(&prefix)).count()
        };
        let own_voice = mv_studio::voices::chapter_voice(&item.id);
        cx.spawn_in(window, async move |this, cx| {
            let r2 = r.clone();
            let Ok(plan) = mv_ui::bg::blocking(move || mv_recap::chapter_plan(&r2)).await else { return };
            let _ = this.update_in(cx, |_, window, cx| {
                let label = item.label();
                if plan.is_empty() && notes == 0 && own_voice.is_none() {
                    toast::show(cx, Toast::info(format!("{label} is already brand new")));
                    return;
                }
                let mut rows: Vec<(mv_ui::Ph, String, String)> =
                    plan.steps.iter().map(|(s, fp)| (phase_icon(Phase::of(*s)), format!("{} · {}", s.label(), s.output_name()), fmt_bytes(fp.bytes))).collect();
                if !plan.settings.is_empty() || own_voice.is_some() {
                    let what = if own_voice.is_some() { "Chapter settings · own voice, emotion strength, music" } else { "Chapter settings · emotion strength, music" };
                    rows.push((ph::SLIDERS_HORIZONTAL, what.into(), fmt_bytes(plan.settings.bytes.max(1))));
                }
                if !plan.other.is_empty() {
                    rows.push((ph::FILE, "Other files Studio made".into(), fmt_bytes(plan.other.bytes)));
                }
                if notes > 0 {
                    rows.push((ph::BRAIN, "Story notes from this chapter".into(), format!("{notes} line{}", plural(notes))));
                }
                let freed = plan.bytes();
                let body = if freed > 0 {
                    format!("Studio starts {label} over and frees {}. The downloaded pages and the manga's characters stay.", fmt_bytes(freed))
                } else {
                    format!("Studio starts {label} over. The downloaded pages and the manga's characters stay.")
                };
                let warning = (plan.edited_pages > 0).then(|| format!("Your edits to {} transcript page{} will be lost.", plan.edited_pages, plural(plan.edited_pages)));
                let weak = this.clone();
                let it = item.clone();
                let dialog = ResetDialog { title: format!("Reset {label}?"), body, rows, warning, confirm: "Reset chapter".into(), cancel: "Keep chapter".into() };
                open_reset_dialog(
                    dialog,
                    move |_, cx| {
                        let _ = weak.update(cx, |this, cx| this.do_reset_chapter(it.clone(), cx));
                    },
                    window,
                    cx,
                );
            });
        })
        .detach();
    }

    fn do_reset_chapter(&mut self, item: ChapterItem, cx: &mut Context<Self>) {
        let (ch, id, label) = (chref(&item), item.id.clone(), item.label());
        if let Some(why) = self.reset_blocker(&id) {
            toast::show(cx, Toast::warning(why));
            return;
        }
        let stopped = self.forget_open_work(&id, &ch.output_dir);
        self.selected.remove(&id);
        let job_id = id.clone();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || -> anyhow::Result<mv_recap::ResetReport> {
                if stopped {
                    crate::audio::wait_stopped(Duration::from_millis(600));
                }
                let mut out = None;
                run::edit_brain(&ch.manga_id, &ch.manga_title, |b| out = Some(mv_recap::reset_chapter(&ch, b)));
                let mut report = out.unwrap_or_else(|| Ok(Default::default()))?;
                if let Err(e) = mv_studio::voices::set_chapter_voice(&job_id, None) {
                    report.errors.push(format!("chapter voice: {e}"));
                }
                let _ = data::mark_stages_reset(&job_id, &Step::ALL);
                Ok(report)
            })
            .await
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(report) => reset_toast(&report, &label, Some("is brand new"), cx),
                    Err(e) => this.fail(format!("Could not reset {label}: {e:#}")),
                }
                // Only the reset chapter's view is stale: another open chapter keeps its editors
                // (and whatever is being typed in them).
                if this.focus_id.as_deref() == Some(id.as_str()) {
                    this.refresh_focus(false, cx);
                }
                this.reload_library(cx);
                cx.notify();
            });
        })
        .detach();
    }

    // ------------------------------------------------------------ layout pieces

    /// (running, failed) phase of the focused chapter, for the stepper.
    fn phase_activity(&self) -> (Option<usize>, Option<usize>) {
        let Some(f) = &self.focus else { return (None, None) };
        let id = f.item.id.as_str();
        let phase_of_run = |r: &RunInfo| match r.kind {
            RunKind::Read => Phase::Read.index(),
            RunKind::Story => Phase::Story.index(),
        };
        let phase_of_job = |j: &JobInfo| if j.stages.iter().any(|s| s == "video") { Phase::Video.index() } else { Phase::Audio.index() };
        let run = self.run.as_ref().filter(|r| r.chapter_id.as_deref() == Some(id));
        let job = self.jobs.iter().find(|j| j.chapter_id == id);
        let running = run
            .filter(|r| r.running())
            .map(phase_of_run)
            .or_else(|| job.filter(|j| !j.state.finished()).map(phase_of_job))
            .or_else(|| (self.music.busy.is_some() && self.music.composing.iter().any(|c| c == id)).then_some(Phase::Music.index()));
        let failed = run.filter(|r| matches!(r.state, RunState::Failed(_))).map(phase_of_run).or_else(|| job.filter(|j| matches!(j.state, JobView::Failed(_))).map(phase_of_job));
        (running, failed)
    }

    fn step_tip(&self, p: Phase, running: bool, failed: bool) -> String {
        let Some(f) = &self.focus else { return format!("{}: pick a chapter first", p.label()) };
        let st = &f.status;
        let what = match p {
            Phase::Read => format!("{}/{} pages read", st.read_pages, st.total_pages),
            Phase::Transcript => match f.transcript.as_ref().map(|t| t.pages.iter().filter(|p| p.edited).count()) {
                Some(0) => "ready".into(),
                Some(n) => format!("{n} page{} edited", plural(n)),
                None => "not made yet".into(),
            },
            Phase::Story => f.story.as_ref().map(|s| format!("{} passages", s.segments.len())).unwrap_or_else(|| "not written yet".into()),
            Phase::Audio if st.audio_files > 0 => format!("{} page clips", st.audio_files),
            Phase::Audio => "not narrated yet".into(),
            Phase::Music => if st.music { "score composed" } else { "no score yet" }.into(),
            Phase::Video => if st.video.is_some() { "rendered" } else { "not rendered yet" }.into(),
        };
        let state = if failed {
            " · failed"
        } else if running {
            " · working"
        } else {
            ""
        };
        format!("{} · {what}{state} · {}", p.label(), mv_ui::kbd_label(&format!("alt-{}", p.index() + 1)))
    }

    /// Studio's shortcuts: Alt+1 to Alt+6 open a step (they never type text, so they work while
    /// editing too); Esc closes the open cue, then the Setup sheet. Pro media tools are keyboard
    /// driven (NN/g flexibility and efficiency: <https://www.nngroup.com/articles/flexibility-efficiency-heuristic/>).
    fn on_key(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let m = &k.modifiers;
        if m.alt && !m.control && !m.platform && !m.shift {
            if let Some(n) = k.key.parse::<usize>().ok().filter(|n| (1..=Phase::ALL.len()).contains(n)) {
                if self.focus.is_some() {
                    self.set_phase(Phase::ALL[n - 1], cx);
                }
                cx.stop_propagation();
            }
            return;
        }
        // The cue panel only counts while the Music step shows it; a cue left selected there
        // must not swallow Escape on the other steps.
        let cue_open = self.phase == Phase::Music && self.music.cue.is_some();
        if k.key == "escape" && !m.modified() && ((cue_open && self.music.cue.take().is_some()) || std::mem::take(&mut self.show_setup)) {
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn toolbar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Room left for the steps once the side columns are drawn; narrow windows show icons
        // only, with the names in the tooltips.
        let center = f32::from(window.viewport_size().width) - 60. - LIBRARY_W - if self.side_open { SIDE_W } else { 0. };
        let compact = center < 980.;
        let status = self.focus.as_ref().map(|f| f.status.clone()).unwrap_or_default();
        let done: Vec<bool> = Phase::ALL.iter().map(|p| self.focus.is_some() && p.done(&status)).collect();
        let (running, failed) = self.phase_activity();
        let tips: Vec<String> = Phase::ALL.iter().enumerate().map(|(i, p)| self.step_tip(*p, running == Some(i), failed == Some(i))).collect();
        let steps = stepper("studio-phases", Phase::ALL.iter().map(|p| (phase_icon(*p), p.label())), self.phase.index(), &done, running, failed)
            .tooltips(tips)
            .when(compact, |s| s.compact())
            .on_select(cx.listener(|this, ix: &usize, _, cx| this.set_phase(Phase::ALL[(*ix).min(5)], cx)));
        let setup = self.setup.read(cx);
        let (issues, checked, checking) = (setup.issues(), setup.env.is_some(), setup.checking);
        let (pill_icon, pill_color, pill_text, pill_tip): (Option<mv_ui::Ph>, Rgba, String, String) = match (checked, issues) {
            (false, _) => (None, Tokens::text_3(), "Checking".into(), "Checking Python, voices and FFmpeg…".into()),
            (true, 0) => (Some(ph::CHECK_CIRCLE), Tokens::success(), "Ready".into(), "Everything Studio needs is installed".into()),
            (true, n) => (Some(ph::WARNING), Tokens::warning(), format!("Setup · {n}"), format!("{n} thing{} need{} attention", plural(n), if n == 1 { "s" } else { "" })),
        };
        let setup_pill = interactive("studio-setup-pill")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .px(px(10.))
            .rounded(px(radius::MD))
            .border_1()
            .border_color(if self.show_setup { Tokens::border_strong() } else { Tokens::border() })
            .text_size(px(size::SM))
            .text_color(Tokens::text_1())
            .role(Role::Button)
            .aria_expanded(self.show_setup)
            .when(self.show_setup, |d| d.bg(Tokens::active()))
            .tooltip(tip(pill_tip))
            .map(|d| match pill_icon {
                None => d.child(spinner("setup-spin", Tokens::text_3(), icon_px::SM)),
                Some(i) => d.child(ph_icon(i, Weight::Fill, pill_color, icon_px::SM)),
            })
            .child(pill_text)
            .when(checking && checked, |d| d.child(spinner("setup-spin-2", Tokens::text_3(), icon_px::XS)))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.show_setup = !this.show_setup;
                cx.notify();
            }));
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(space::MD))
            .h(px(52.))
            .px(px(14.))
            .border_b_1()
            .border_color(Tokens::border())
            .child(div().flex_1().min_w_0().overflow_hidden().child(steps))
            .child(self.usage_meter.clone())
            .child(setup_pill)
            .child(icon_button("studio-side-toggle", ph::SIDEBAR_SIMPLE, if self.side_open { "Hide the AI panel" } else { "Show the AI panel" }).selected(self.side_open).on_click(cx.listener(
                |this, _: &ClickEvent, _, cx| {
                    this.side_open = !this.side_open;
                    cx.notify();
                },
            )))
    }

    fn library_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let filter = self.search.read(cx).value().to_lowercase();
        let body: AnyElement = match &self.library {
            // Content-shaped placeholders, like the rows that will come (NN/g skeleton screens).
            Load::Loading => div()
                .flex()
                .flex_col()
                .gap(px(space::XS))
                .px(px(space::SM))
                .children((0..6).map(|_| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .h(px(56.))
                        .px(px(6.))
                        .child(skeleton(px(34.), px(46.), radius::SM))
                        .child(div().flex().flex_col().gap(px(space::SM)).flex_1().child(skeleton_line(relative(0.6))).child(skeleton_line(relative(0.35)).h(px(10.))))
                }))
                .into_any_element(),
            Load::Failed(e) => div()
                .p(px(space::MD))
                .child(error_view(format!("Could not load your chapters. {e}")).title("Library unavailable").compact().retry(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.library = Load::Loading;
                    this.reload_library(cx);
                    cx.notify();
                })))
                .into_any_element(),
            Load::Ready(groups) if groups.is_empty() => div()
                .p(px(space::MD))
                .child(
                    empty_view(glyphs::DOWNLOADS, "Nothing downloaded yet", "Studio works on chapters you've downloaded. Open a manga and download a chapter.")
                        .compact()
                        .action(btn("studio-open-library", "Open Library", ButtonKind::Primary).icon(glyphs::LIBRARY).on_click(|_, w, cx| mv_ui::navigate(Route::Library, w, cx))),
                )
                .into_any_element(),
            Load::Ready(groups) => {
                let shown: Vec<&MangaGroup> = groups.iter().filter(|g| filter.is_empty() || g.title.to_lowercase().contains(&filter)).collect();
                if shown.is_empty() {
                    div().p(px(space::MD)).child(empty_view(glyphs::SEARCH, "No manga match", format!("Nothing downloaded matches \"{filter}\".")).compact()).into_any_element()
                } else {
                    div()
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
                                .pl(px(space::SM))
                                .pr(px(14.))
                                .pb(px(space::SM))
                                .children(shown.into_iter().map(|g| self.group_view(g, cx))),
                        )
                        .child(vbar("studio-lib-bar", &self.lib_scroll))
                        .into_any_element()
                }
            }
        };
        let total: usize = match &self.library {
            Load::Ready(g) => g.iter().map(|g| g.chapters.len()).sum(),
            _ => 0,
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(LIBRARY_W))
            .h_full()
            .map(column)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .p(px(space::MD))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::SM))
                            .child(icon_ph(glyphs::DOWNLOADS, Tokens::text_2(), icon_px::MD))
                            .child(div().flex_1().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Downloaded"))
                            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{total} chapter{}", plural(total)))),
                    )
                    .child(Input::new(&self.search).small().prefix(icon_ph(glyphs::SEARCH, Tokens::text_3(), icon_px::SM))),
            )
            .child(body)
            .child(self.batch_bar(cx))
    }

    /// The footer: what a selection lets you do (Carbon data table batch actions).
    fn batch_bar(&self, cx: &mut Context<Self>) -> Div {
        let n = self.selected.len();
        let bar = div().flex().flex_none().items_center().gap(px(space::XS)).h(px(44.)).px(px(10.)).border_t_1().border_color(Tokens::border()).text_size(px(size::SM));
        if n == 0 {
            return bar
                .text_color(Tokens::text_3())
                .child(icon_ph(ph::LIST_CHECKS, Tokens::text_3(), icon_px::SM))
                .child(div().flex_1().min_w_0().truncate().child("Tick chapters to process them together"));
        }
        let blocker = self.run_blocker();
        let what = format!("{n} chapter{}", plural(n));
        bar.bg(Tokens::accent_soft())
            .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::MEDIUM).text_color(Tokens::accent()).child(format!("{n} selected")))
            .child(
                icon_button("batch-read", glyphs::READ, format!("Read {what}"))
                    .small()
                    .when_some(blocker, |b, r| b.disabled(r))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.start_read(false, cx))),
            )
            .child(
                icon_button("batch-story", glyphs::STORY, format!("Write the story for {what}"))
                    .small()
                    .when_some(blocker, |b, r| b.disabled(r))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.start_story(cx))),
            )
            .child(
                icon_button("batch-narrate", glyphs::NARRATE, format!("Narrate the pages of {what} that have no audio yet"))
                    .small()
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.run_pipeline(&["tts"], false, cx))),
            )
            .child(icon_button("batch-render", glyphs::VIDEO, format!("Render {what}")).small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.render_video(cx))))
            .child(div().w(px(1.)).h(px(18.)).mx(px(2.)).bg(Tokens::border_strong()))
            .child(icon_button("batch-clear", ph::X, "Clear the selection").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.selected.clear();
                cx.notify();
            })))
    }

    fn group_view(&self, g: &MangaGroup, cx: &mut Context<Self>) -> Div {
        let open = self.open_groups.contains(&g.id);
        let gid = g.id.clone();
        let all_ids: Vec<String> = g.chapters.iter().map(|c| c.id.clone()).collect();
        let ticked = all_ids.iter().filter(|id| self.selected.contains(*id)).count();
        let state = match ticked {
            0 => CheckState::Unchecked,
            n if n == all_ids.len() => CheckState::Checked,
            _ => CheckState::Mixed,
        };
        let all_on = state == CheckState::Checked;
        div()
            .flex()
            .flex_col()
            .child(
                list_row(SharedString::from(format!("grp-{}", g.id)), false)
                    .h(px(56.))
                    .px(px(6.))
                    .gap(px(space::SM))
                    .when(open, |d| d.bg(Tokens::hover()))
                    .aria_expanded(open)
                    .child(
                        div()
                            .w(px(34.))
                            .h(px(46.))
                            .flex_none()
                            .rounded(px(radius::SM))
                            .overflow_hidden()
                            .bg(Tokens::surface_2())
                            .flex()
                            .items_center()
                            .justify_center()
                            .map(|d| match &g.cover {
                                Some(p) => d.child(img(p.clone()).size_full().object_fit(ObjectFit::Cover)),
                                None => d.child(ph_icon(glyphs::MANGA, Weight::Duotone, Tokens::text_3(), icon_px::MD)),
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
                            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(match ticked {
                                0 => format!("{} chapter{}", g.chapters.len(), plural(g.chapters.len())),
                                t => format!("{t} of {} ticked", g.chapters.len()),
                            })),
                    )
                    .child(icon_ph(if open { ph::CARET_DOWN } else { ph::CARET_RIGHT }, Tokens::text_3(), icon_px::SM))
                    .child(
                        checkbox(SharedString::from(format!("grp-all-{}", g.id)), state)
                            .tooltip(tip(if all_on { "Untick every chapter" } else { "Tick every chapter" }))
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
            || self.jobs.iter().any(|j| j.chapter_id == c.id && !j.state.finished())
            || (self.music.busy.is_some() && self.music.composing.contains(&c.id));
        let trail_tip = Phase::ALL.iter().map(|p| format!("{} {}", p.label(), if p.done(&st) { "✓" } else { "–" })).collect::<Vec<_>>().join(" · ");
        let blocker = self.reset_blocker(&c.id);
        let (id, item, reset_item) = (c.id.clone(), c.clone(), c.clone());
        list_row(SharedString::from(format!("ch-{}", c.id)), focused)
            .group("ch-row")
            .h(px(34.))
            .min_h(px(34.))
            .pl(px(36.))
            .pr(px(4.))
            .py(px(0.))
            .gap(px(6.))
            .when(checked && !focused, |d| d.bg(Tokens::accent_soft()))
            .child(
                checkbox(SharedString::from(format!("chk-{}", c.id)), checked)
                    .tooltip(tip(if checked { "Untick" } else { "Tick to process with other chapters" }))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        if !this.selected.remove(&id) {
                            this.selected.insert(id.clone());
                        }
                        cx.notify();
                    })),
            )
            .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).child(c.label()))
            .when(running, |d| d.child(spinner(SharedString::from(format!("row-spin-{}", c.id)), Tokens::accent(), icon_px::SM)))
            .child(div().id(SharedString::from(format!("row-trail-{}", c.id))).tooltip(tip(trail_tip)).child(dot_trail(SharedString::from(format!("row-dots-{}", c.id)), Phase::ALL.iter().map(|p| p.done(&st)).collect(), running, 5.)))
            .child(
                icon_button(SharedString::from(format!("ch-reset-{}", c.id)), glyphs::DELETE, "Reset chapter")
                    .small()
                    .danger()
                    .when_some(blocker, |b, r| b.disabled(r))
                    .when(!focused, |b| b.invisible().group_hover("ch-row", |s| s.visible()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.confirm_reset_chapter(reset_item.clone(), window, cx);
                    })),
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.focus_chapter(item.clone(), cx)))
    }

    fn notice_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let n = self.notice.as_ref()?;
        let mut alert = inline_alert(AlertKind::Error, n.text.clone(), "").dismissible(cx.listener(|this, _: &ClickEvent, _, cx| {
            this.notice = None;
            cx.notify();
        }));
        if let Some(a) = n.action {
            alert = alert.action(a.label(), cx.listener(move |this, _: &ClickEvent, window, cx| this.run_notice_action(a, window, cx)));
        }
        Some(div().flex_none().mx(px(space::XL)).mt(px(space::MD)).child(alert).into_any_element())
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

impl StudioPage {
    /// Keyboard focus sits on something Studio no longer shows (an editor of another step, the
    /// closed Ask box, a rename field or editor that went away): Studio takes it back, or its
    /// shortcuts would stop working until the next click (keys reach no view at all).
    fn reclaim_focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        let orphaned = window.focused(cx).is_none()
            || (self.ask.is_none() && self.ask_input.read(cx).focus_handle(cx).is_focused(window))
            || self.editors.iter().any(|((p, _), e)| *p != self.phase && e.read(cx).focus_handle(cx).is_focused(window));
        if orphaned {
            window.focus(&self.focus_handle, cx);
        }
    }
}

impl Render for StudioPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.reclaim_focus(window, cx);
        self.remember();
        self.center_w = f32::from(window.viewport_size().width) - 24. - LIBRARY_W - if self.side_open { SIDE_W } else { 0. };
        let center = self.center(window, cx);
        let side = self.side_open.then(|| self.side_panel(window, cx));
        let sheet = self.show_setup.then(|| {
            div()
                .id("studio-setup")
                .flex_none()
                .max_h(px(460.))
                .overflow_y_scroll()
                .mx(px(space::XL))
                .mt(px(space::MD))
                .p(px(space::LG))
                .rounded(px(radius::LG))
                .bg(Tokens::surface())
                .border_1()
                .border_color(Tokens::border())
                .child(self.setup.clone())
        });
        div()
            .flex()
            .size_full()
            .gap(px(6.))
            .p(px(6.))
            .bg(Tokens::canvas())
            .track_focus(&self.focus_handle)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    // A click on a kit control keeps focus where it was; when that was outside
                    // Studio (or nowhere), Studio takes it so its shortcuts work.
                    if !this.focus_handle.contains_focused(window, cx) {
                        window.focus(&this.focus_handle, cx);
                    }
                }),
            )
            .on_key_down(cx.listener(Self::on_key))
            .child(self.library_panel(cx))
            .child(div().flex().flex_col().flex_1().min_w_0().h_full().map(column).child(self.toolbar(window, cx)).children(self.notice_bar(cx)).children(sheet).child(center))
            .children(side)
    }
}

// ---------------------------------------------------------------- helpers

/// Says that a reset happened, how much it freed, and anything it could not remove.
fn reset_toast(report: &mv_recap::ResetReport, label: &str, chapter: Option<&str>, cx: &mut App) {
    let steps: Vec<&str> = report.steps.iter().map(|s| s.label()).collect();
    let freed = if report.bytes > 0 { format!(" · {} freed", fmt_bytes(report.bytes)) } else { String::new() };
    let msg = match (chapter, steps.as_slice()) {
        (Some(what), _) => format!("{label} {what}{freed}"),
        (None, []) => format!("Nothing to reset in {label}"),
        (None, [one]) => format!("{one} reset for {label}{freed}"),
        (None, many) => format!("{} reset for {label}{freed}", join_and(many)),
    };
    if report.errors.is_empty() {
        toast::show(cx, Toast::success(msg).icon(ph::ARROW_COUNTER_CLOCKWISE));
    } else {
        // Usually a clip still open in a player.
        toast::show(cx, Toast::warning(format!("{msg}. {} file{} could not be removed (in use?).", report.errors.len(), plural(report.errors.len()))));
    }
}

fn run_finished_toast(r: &RunInfo, cx: &mut App) {
    let what = match r.kind {
        RunKind::Read => "Reading",
        RunKind::Story => "The story",
    };
    let where_ = r.chapter.clone().unwrap_or_default();
    match &r.state {
        RunState::Done => toast::show(cx, Toast::success(format!("{what} finished · {where_}")).icon(if r.kind == RunKind::Read { glyphs::READ } else { glyphs::STORY })),
        RunState::Failed(e) => toast::show(cx, Toast::error(format!("{what} failed: {e}"))),
        RunState::Cancelled | RunState::Running => {}
    }
}

fn job_finished_toast(j: &JobInfo, cx: &mut App) {
    let kind = if j.stages.iter().any(|s| s == "video") { "Video" } else { "Audio" };
    match &j.state {
        JobView::Done => toast::show(cx, Toast::success(format!("{kind} ready · {}", j.chapter_label)).icon(if kind == "Video" { glyphs::VIDEO } else { glyphs::NARRATE })),
        JobView::Failed(e) => toast::show(cx, Toast::error(format!("{kind} failed for {}: {e}", j.chapter_label))),
        _ => {}
    }
}

/// "Story, Audio and Video".
pub(crate) fn join_and(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Lines of the manga's story notes the focused chapter's read wrote.
fn chapter_notes(f: &Focus) -> usize {
    let brain = run::brain_snapshot(&f.item.manga_id, &f.item.manga_title);
    let prefix = format!("- [{} p", f.chref.label);
    brain.notes.lines().filter(|l| l.starts_with(&prefix)).count()
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

/// The text an editor opens with: its transcript page or story passage.
fn editor_source(f: &Focus, (phase, idx): (Phase, usize)) -> String {
    match phase {
        Phase::Transcript => transcript_text(f, idx),
        Phase::Story => f.story.as_ref().and_then(|s| s.segments.get(idx)).map(|s| s.text.clone()).unwrap_or_default(),
        _ => String::new(),
    }
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

#[cfg(test)]
mod tests {
    // Explicit imports: with test-support, `gpui_kit::*` carries a `test` macro that shadows `#[test]`.
    use super::{join_and, NarratedBy, Phase};

    #[test]
    fn phases_follow_the_recap_steps() {
        for (i, p) in Phase::ALL.iter().enumerate() {
            assert_eq!(p.index(), i);
            assert_eq!(Phase::of(p.step()), *p);
            assert_eq!(p.label(), mv_recap::Step::ALL[i].label());
        }
    }

    #[test]
    fn lists_read_naturally() {
        assert_eq!(join_and(&["Story"]), "Story");
        assert_eq!(join_and(&["Story", "Audio"]), "Story and Audio");
        assert_eq!(join_and(&["Story", "Audio", "Video"]), "Story, Audio and Video");
    }

    #[test]
    fn narration_params_name_the_engine() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(NarratedBy::load(d.path()), None);
        std::fs::create_dir_all(d.path().join("audio")).unwrap();
        std::fs::write(d.path().join("audio/narration_params.json"), r#"{"engine":"kokoro","voice":"cb_narrator_f","synthVoice":"af_heart","chatterboxError":"no CUDA","pages":[]}"#).unwrap();
        let n = NarratedBy::load(d.path()).unwrap();
        assert_eq!((n.engine.as_str(), n.synth_voice.as_str(), n.chatterbox_error.as_deref()), ("kokoro", "af_heart", Some("no CUDA")));
    }
}
