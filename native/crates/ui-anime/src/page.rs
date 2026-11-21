//! The Anime Engine page (design 9.1, 9.2): the tab home (projects and downloaded manga), or an
//! open project: the 56 px header (back, project and episode menus, chapters, BETA, the queue
//! chip, more actions), the 8-step bar, the step's own view, and the queue strip at the bottom.
//!
//! The page is kept for the whole session (`lib.rs`), so leaving the tab and coming back finds
//! it as it was; each step's view is kept per episode, so a step keeps its selection too. The
//! open project's episode and step are also saved in its `work/ui.json`, so it reopens where
//! the user left it. One poll task follows the engine queue, the project files, Studio's Reads
//! and Magnific's state.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use anyhow::Context as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_anime::queue::{self, JobState, Lane, Queue};
use mv_anime::{ChapterPick, Episode, EpisodeRef, Series, Store, SCHEMA};
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::{badge, banner, icon_button, icon_ph, interactive, menu_button, overflow_menu, ph, size, spinner, tip, toast, AlertKind, MenuItem, Route, Toast, Tokens, Tone};
use serde::{Deserialize, Serialize};

use crate::backends;
use crate::home::{self, MangaRow};
use crate::queue_strip::{summarize, summary_text, web_held, Eta, Strip};
use crate::stepbar;
use crate::steps::{self, Step, StepCtx};
use crate::style::{GUTTER, HEADER_H, POLL_MS};
use crate::widgets::{count, is_narrow};

/// Something read in the background.
pub(crate) enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

/// Where the user was in a project (`work/ui.json`; scratch, safe to lose).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub(crate) struct UiState {
    pub episode: String,
    pub step: String,
}

/// A project in the home list.
pub(crate) struct ProjectRow {
    pub series: Series,
    pub ui: UiState,
    /// Which steps of its open episode are done (the card's progress line).
    pub done: [bool; 8],
}

/// The project in view.
pub(crate) struct OpenProject {
    pub store: Store,
    pub series: Series,
    pub episode: Episode,
    pub step: Step,
    /// Each step's completion rule (design 9.2).
    pub done: [bool; 8],
    /// Step views, kept per (step, episode).
    views: HashMap<(Step, String), AnyView>,
}

/// A project read from disk.
struct Loaded {
    series: Series,
    episode: Episode,
    ui: UiState,
    done: [bool; 8],
}

pub struct EnginePage {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) library: Load<Vec<MangaRow>>,
    pub(crate) projects: Load<Vec<ProjectRow>>,
    pub(crate) search: Option<Entity<InputState>>,
    pub(crate) guide: bool,
    /// The manga whose project is being created.
    pub(crate) creating: Option<String>,
    /// The project being opened.
    pub(crate) opening: Option<String>,
    pub(crate) open: Option<OpenProject>,
    pub(crate) strip: Strip,
    pub(crate) home_scroll: ScrollHandle,
    queue: Queue,
    studio: ui_studio::pipeline::Queue,
    /// Versions seen: engine queue, project files, Studio queue, Magnific state.
    seen: [u64; 4],
    loading: bool,
    again: Option<bool>,
    rename: Option<Entity<InputState>>,
    /// The rename box's subscription (one at a time, dropped with the box).
    rename_sub: Option<Subscription>,
    /// Project files found damaged and put back, shown until dismissed.
    recoveries: Vec<mv_anime::store::Recovery>,
    /// Polls since the last change (the running job's clock redraws every [`TIME_TICKS`]).
    idle_ticks: u64,
    _subs: Vec<Subscription>,
}

/// Polls between two redraws of a running job's time (about a second).
const TIME_TICKS: u64 = 1000 / POLL_MS + 1;

// ---------------------------------------------------------------- disk (blocking)

fn ui_path(store: &Store) -> std::path::PathBuf {
    store.work_dir().join("ui.json")
}

fn load_ui(store: &Store) -> UiState {
    std::fs::read(ui_path(store)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_ui(store: Store, ui: UiState) {
    mv_ui::bg::runtime().spawn_blocking(move || {
        let path = ui_path(&store);
        let res = std::fs::create_dir_all(store.work_dir()).and_then(|_| std::fs::write(&path, serde_json::to_vec_pretty(&ui).unwrap_or_default()));
        if let Err(e) = res {
            mv_core::logging::global().warn("anime", &format!("could not remember the open step in {}: {e}", path.display()));
        }
    });
}

/// The project list. Each card's progress line costs a full completion check (the cut plan
/// included), so it is worked out only for projects not in `known` (their earlier marks, kept
/// while jobs write); the open project's own marks come from [`load_open`].
fn load_projects(known: &HashMap<String, [bool; 8]>) -> Vec<ProjectRow> {
    mv_anime::store::list_projects()
        .into_iter()
        .map(|series| {
            let store = Store::open(&series.id);
            let ui = load_ui(&store);
            let done = match known.get(&series.id) {
                Some(d) => *d,
                None => {
                    let ep = Some(ui.episode.clone()).filter(|e| series.episodes.iter().any(|r| &r.id == e)).or_else(|| series.episodes.first().map(|r| r.id.clone()));
                    ep.and_then(|e| store.episode(&e).ok()).map(|ep| stepbar::completion(&store, &series, &ep)).unwrap_or([false; 8])
                }
            };
            ProjectRow { series, ui, done }
        })
        .collect()
}

/// Reads a project and one of its episodes (the asked one, else the remembered one, else the
/// first), with each step's completion.
fn load_open(store: &Store, episode: Option<&str>) -> anyhow::Result<Loaded> {
    let series = store.series().context("The project could not be read")?;
    let ui = load_ui(store);
    let known = |e: &str| series.episodes.iter().any(|r| r.id == e);
    let ep_id = episode
        .filter(|e| known(e))
        .map(str::to_string)
        .or_else(|| Some(ui.episode.clone()).filter(|e| known(e)))
        .or_else(|| series.episodes.first().map(|r| r.id.clone()))
        .context("This project has no episode yet")?;
    let mut ep = store.episode(&ep_id).context("The episode could not be read")?;
    if ep.id.is_empty() {
        ep.id = ep_id;
    }
    let done = stepbar::completion(store, &series, &ep);
    Ok(Loaded { series, episode: ep, ui, done })
}

/// "Ch. 1–2", "Ch. 3", "3 chapters", "No chapters yet".
pub fn chapter_range(chs: &[ChapterPick]) -> String {
    if chs.is_empty() {
        return "No chapters yet".into();
    }
    let nums: Vec<f64> = chs.iter().filter_map(|c| c.number).collect();
    if nums.len() != chs.len() {
        return count(chs.len(), "chapter");
    }
    let lo = nums.iter().copied().fold(f64::MAX, f64::min);
    let hi = nums.iter().copied().fold(f64::MIN, f64::max);
    let f = |n: f64| if n.fract() == 0.0 { format!("{}", n as i64) } else { format!("{n}") };
    if lo == hi {
        format!("Ch. {}", f(lo))
    } else {
        format!("Ch. {}–{}", f(lo), f(hi))
    }
}

/// The next free episode id ("ep03").
pub fn next_episode_id(eps: &[EpisodeRef]) -> String {
    let mut n = eps.len() + 1;
    loop {
        let id = format!("ep{n:02}");
        if !eps.iter().any(|e| e.id == id) {
            return id;
        }
        n += 1;
    }
}

// ---------------------------------------------------------------- state

impl EnginePage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        backends::ensure_runner(cx);
        let mut page = EnginePage {
            focus_handle,
            library: Load::Loading,
            projects: Load::Loading,
            search: None,
            guide: false,
            creating: None,
            opening: None,
            open: None,
            strip: Strip::default(),
            home_scroll: ScrollHandle::new(),
            queue: queue::snapshot(),
            studio: ui_studio::pipeline::snapshot(),
            seen: versions(),
            loading: false,
            again: None,
            rename: None,
            rename_sub: None,
            recoveries: Vec::new(),
            idle_ticks: 0,
            _subs: Vec::new(),
        };
        page.load(true, cx);
        page.poll(window, cx);
        page
    }

    /// Re-reads the project list, the downloaded manga and the open project's files (on every
    /// re-open of the tab).
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        backends::refresh_model(cx);
        self.queue = queue::snapshot();
        self.studio = ui_studio::pipeline::snapshot();
        self.load(true, cx);
    }

    /// Reads in the background; `library` also re-scans the downloaded manga (slower).
    fn load(&mut self, library: bool, cx: &mut Context<Self>) {
        if self.loading {
            self.again = Some(self.again.unwrap_or(false) || library);
            return;
        }
        self.loading = true;
        let open = self.open.as_ref().map(|o| (o.store.clone(), o.episode.id.clone()));
        // A full reload (the tab opened) works out every card again; a reload after a job wrote
        // keeps the cards' marks and updates the open project's only.
        let mut known: HashMap<String, [bool; 8]> = HashMap::new();
        if !library {
            if let Load::Ready(rows) = &self.projects {
                known = rows.iter().map(|r| (r.series.id.clone(), r.done)).collect();
            }
        }
        // Damaged-file notices of projects that may have been removed meanwhile (their folder
        // was renamed away): those go with their project.
        let watched: Vec<String> = self.recoveries.iter().map(|r| r.project.clone()).collect();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || {
                let projects = load_projects(&known);
                let lib = library.then(|| home::load_library().map_err(|e| format!("{e:#}")));
                let open = open.map(|(store, ep)| (store.id.clone(), load_open(&store, Some(&ep)).map_err(|e| format!("{e:#}"))));
                let gone: Vec<String> = watched.into_iter().filter(|id| !Store::open(id).dir.is_dir()).collect();
                (projects, lib, open, gone)
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match res {
                    Ok((mut projects, lib, open, gone)) => {
                        this.recoveries.retain(|r| !gone.contains(&r.project));
                        for p in &projects {
                            crate::queue_strip::remember_title(&p.series.id, &p.series.manga_title);
                        }
                        // The open project's card shows its fresh marks.
                        if let (Some(o), Some((_, Ok(l)))) = (this.open.as_ref(), open.as_ref()) {
                            let remembered = projects.iter().find(|p| p.series.id == o.store.id).map(|p| p.ui.episode.clone());
                            if let Some(row) = projects.iter_mut().find(|p| p.series.id == o.store.id && remembered.as_deref().is_none_or(|e| e.is_empty() || e == l.episode.id)) {
                                row.done = l.done;
                            }
                        }
                        this.projects = Load::Ready(projects);
                        if let Some(lib) = lib {
                            this.library = match lib {
                                Ok(rows) => Load::Ready(rows),
                                Err(e) => Load::Failed(e),
                            };
                        }
                        if let (Some((id, res)), Some(o)) = (open, this.open.as_mut()) {
                            if o.store.id == id {
                                match res {
                                    Ok(l) => {
                                        o.series = l.series;
                                        // A read started before an episode switch: its marks
                                        // belong to the other episode.
                                        if o.episode.id == l.episode.id {
                                            o.episode = l.episode;
                                            o.done = l.done;
                                        }
                                    }
                                    Err(e) => mv_core::logging::global().warn("anime", &format!("could not re-read project {id}: {e}")),
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let msg = format!("{e:#}");
                        this.projects = Load::Failed(msg.clone());
                        if library {
                            this.library = Load::Failed(msg);
                        }
                    }
                }
                if let Some(lib) = this.again.take() {
                    this.load(lib, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(POLL_MS)).await;
            let v = versions();
            if this.update(cx, |p, cx| p.tick(v, cx)).is_err() {
                break;
            }
        })
        .detach();
    }

    fn tick(&mut self, v: [u64; 4], cx: &mut Context<Self>) {
        // Nothing is read or redrawn while the tab is not on screen; opening it reloads.
        if !crate::tab_shown() {
            return;
        }
        // Project files found damaged and put back: the user is told (until they dismiss it).
        let back = mv_anime::store::take_recoveries();
        if !back.is_empty() {
            for r in &back {
                mv_core::logging::global().warn("anime", &format!("told the user: {} of project {} was damaged and put back ({:?})", r.what, r.project, r.restored));
            }
            self.recoveries.extend(back);
            let extra = self.recoveries.len().saturating_sub(10);
            self.recoveries.drain(..extra);
            cx.notify();
        }
        if v == self.seen {
            // A running job's time counts up even when nothing reports: redraw about once a
            // second so "38 s · about 1 min left" stays true.
            self.idle_ticks = self.idle_ticks.wrapping_add(1);
            let running = self.queue.jobs.iter().any(|j| j.state == mv_anime::queue::JobState::Running);
            if running && self.idle_ticks % TIME_TICKS == 0 {
                cx.notify();
            }
            return;
        }
        // A version moving is not enough to draw again: only what this page shows having changed
        // is (a held queue, a Studio chapter of another manga, a site check nobody sees).
        let mut changed = false;
        if v[0] != self.seen[0] {
            let q = queue::snapshot();
            if q != self.queue {
                self.queue = q;
                changed = true;
            }
        }
        if v[2] != self.seen[2] {
            let s = ui_studio::pipeline::snapshot();
            changed |= self.studio_shown(&s) != self.studio_shown(&self.studio);
            self.studio = s;
        }
        // Magnific's state shows here only in the held-pictures banner.
        if v[3] != self.seen[3] && web_held(&self.queue) {
            changed = true;
        }
        let files = v[1] != self.seen[1];
        self.seen = v;
        if files {
            // Draws again once the files are read.
            self.load(false, cx);
        }
        if changed {
            cx.notify();
        }
    }

    /// What the page shows of Studio's queue: the states of the open episode's chapters (the
    /// Manga step's mark). Nothing on the home.
    fn studio_shown(&self, studio: &ui_studio::pipeline::Queue) -> Vec<(Step, JobState, bool)> {
        let Some(o) = &self.open else { return Vec::new() };
        let chapters: Vec<String> = o.episode.chapters.iter().map(|c| c.chapter_id.clone()).collect();
        stepbar::job_states(&Queue::default(), "", "", studio, &chapters)
    }

    pub(crate) fn ensure_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.is_some() {
            return;
        }
        let s = cx.new(|cx| InputState::new(window, cx).placeholder("Search your manga"));
        self._subs.push(cx.subscribe(&s, |_, _, _: &InputEvent, cx| cx.notify()));
        self.search = Some(s);
    }

    pub(crate) fn open_project(&mut self, id: String, _window: &mut Window, cx: &mut Context<Self>) {
        if self.open.as_ref().is_some_and(|o| o.store.id == id) {
            return;
        }
        self.open = None;
        self.rename = None;
        self.rename_sub = None;
        self.opening = Some(id.clone());
        cx.notify();
        let store = Store::open(&id);
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || load_open(&store, None).map(|l| (store, l))).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if this.opening.as_deref() != Some(id.as_str()) {
                    return;
                }
                this.opening = None;
                match res {
                    Ok((store, l)) => {
                        crate::queue_strip::remember_title(&store.id, &l.series.manga_title);
                        let step = Step::from_key(&l.ui.step).unwrap_or_default();
                        this.open = Some(OpenProject { store, series: l.series, episode: l.episode, step, done: l.done, views: HashMap::new() });
                        this.remember();
                    }
                    Err(e) => {
                        mv_core::logging::global().warn("anime", &format!("could not open project {id}: {e:#}"));
                        toast::show(cx, Toast::error(format!("Could not open the project: {e:#}")));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn close_project(&mut self, cx: &mut Context<Self>) {
        self.open = None;
        self.opening = None;
        self.rename = None;
        self.rename_sub = None;
        self.load(false, cx);
        cx.notify();
    }

    /// Saves the open project's episode and step to `work/ui.json`.
    fn remember(&self) {
        if let Some(o) = &self.open {
            save_ui(o.store.clone(), UiState { episode: o.episode.id.clone(), step: o.step.key().to_string() });
        }
    }

    pub(crate) fn set_step(&mut self, step: Step, cx: &mut Context<Self>) {
        if let Some(o) = self.open.as_mut() {
            if o.step != step {
                o.step = step;
                self.remember();
                cx.notify();
            }
        }
    }

    fn set_episode(&mut self, ep: String, cx: &mut Context<Self>) {
        let Some(o) = &self.open else { return };
        if o.episode.id == ep {
            return;
        }
        let store = o.store.clone();
        let project = store.id.clone();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || load_open(&store, Some(&ep))).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                // Another project was opened (or the tab went home) meanwhile: this answer is stale.
                if !this.open.as_ref().is_some_and(|o| o.store.id == project) {
                    return;
                }
                match res {
                    Ok(l) => {
                        if let Some(o) = this.open.as_mut() {
                            o.series = l.series;
                            o.episode = l.episode;
                            o.done = l.done;
                        }
                        this.remember();
                    }
                    Err(e) => toast::show(cx, Toast::error(format!("Could not open the episode: {e:#}"))),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Adds the next episode, with the downloaded chapter after the current episode's last one
    /// (none when there is no later chapter; the Manga step adds them).
    fn new_episode(&mut self, cx: &mut Context<Self>) {
        let Some(o) = &self.open else { return };
        let store = o.store.clone();
        let last = o.episode.chapters.iter().filter_map(|c| c.number).fold(None, |m: Option<f64>, n| Some(m.map_or(n, |m| m.max(n))));
        let chapters = match &self.library {
            Load::Ready(rows) => rows.iter().find(|r| r.group.id == o.series.manga_id).map(|r| r.group.chapters.clone()).unwrap_or_default(),
            _ => Vec::new(),
        };
        let first = home::next_chapter(&chapters, last).map(home::pick_of);
        let project = store.id.clone();
        cx.spawn(async move |this, cx| {
            // The id and the list come from the project file as it is now, changed under its
            // lock, so nothing a job wrote meanwhile is lost.
            let res = mv_ui::bg::blocking(move || -> anyhow::Result<String> {
                // The episode's own file first (never saved inside another file's update); the
                // project then lists it, changed under its lock.
                let fresh = store.series().context("The project could not be read")?;
                let id = next_episode_id(&fresh.episodes);
                let ep = Episode { schema: SCHEMA, id: id.clone(), title: format!("Episode {}", fresh.episodes.len() + 1), chapters: first.into_iter().collect(), ..Default::default() };
                store.save_episode(&ep)?;
                store.update_series(|series| {
                    if !series.episodes.iter().any(|r| r.id == ep.id) {
                        series.episodes.push(EpisodeRef { id: ep.id.clone(), title: ep.title.clone() });
                    }
                })?;
                Ok(id)
            })
            .await
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| match res {
                Ok(id) => {
                    let ours = this.open.as_mut().filter(|o| o.store.id == project).map(|o| o.step = Step::Manga).is_some();
                    if ours {
                        this.set_episode(id, cx);
                    }
                }
                Err(e) => toast::show(cx, Toast::error(format!("Could not add the episode: {e:#}"))),
            });
        })
        .detach();
    }

    fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(o) = &self.open else { return };
        let title = o.episode.title.clone();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Episode name"));
        input.update(cx, |s, cx| {
            s.set_value(title, window, cx);
            s.focus(window, cx);
        });
        // One box, one subscription: the box's own (replaced with the next rename).
        self.rename_sub = Some(cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                this.commit_rename(cx);
            }
        }));
        self.rename = Some(input);
        cx.notify();
    }

    fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.rename.take() else { return };
        self.rename_sub = None;
        let title = input.read(cx).value().trim().to_string();
        cx.notify();
        let Some(o) = self.open.as_mut() else { return };
        if title.is_empty() || title == o.episode.title {
            return;
        }
        o.episode.title = title.clone();
        if let Some(r) = o.series.episodes.iter_mut().find(|r| r.id == o.episode.id) {
            r.title = title.clone();
        }
        let (store, ep_id) = (o.store.clone(), o.episode.id.clone());
        cx.spawn(async move |_, cx| {
            // Both files are read fresh and changed under their locks: only the title changes.
            let res = mv_ui::bg::blocking(move || -> anyhow::Result<()> {
                let t = title.clone();
                store.update_json::<Episode>(&format!("episodes/{ep_id}/episode.json"), move |e| e.title = t)?;
                store.update_series(|series| {
                    if let Some(r) = series.episodes.iter_mut().find(|r| r.id == ep_id) {
                        r.title = title;
                    }
                })?;
                Ok(())
            })
            .await
            .and_then(|r| r);
            if let Err(e) = res {
                let _ = cx.update(|cx| toast::show(cx, Toast::error(format!("Could not rename the episode: {e:#}"))));
            }
        })
        .detach();
    }

    fn set_variants(&mut self, n: u8, cx: &mut Context<Self>) {
        let Some(o) = self.open.as_mut() else { return };
        o.series.settings.variants = n;
        let store = o.store.clone();
        cx.notify();
        cx.spawn(async move |_, cx| {
            // Changed in the project file as it is now (a step may have just saved it).
            let res = mv_ui::bg::blocking(move || store.update_series(|s| s.settings.variants = n).map(|_| ())).await.and_then(|r| r);
            if let Err(e) = res {
                let _ = cx.update(|cx| toast::show(cx, Toast::error(format!("Could not save the setting: {e:#}"))));
            }
        })
        .detach();
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let m = &k.modifiers;
        if m.alt && !m.control && !m.platform && !m.shift && self.open.is_some() {
            if let Some(step) = k.key.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).and_then(Step::from_index) {
                self.set_step(step, cx);
                cx.stop_propagation();
            }
        }
    }
}

impl EnginePage {
    /// Project titles by id, as listed.
    fn titles(&self) -> HashMap<String, String> {
        match &self.projects {
            Load::Ready(rows) => rows.iter().map(|r| (r.series.id.clone(), r.series.manga_title.clone())).collect(),
            _ => HashMap::new(),
        }
    }
}

/// What the user reads about project files found damaged and put back: what it was and in which
/// project, how old the copy put back is (or that it had to start empty), that later changes are
/// lost, and that the damaged copy is kept in the project's folder.
pub(crate) fn recovery_text(list: &[mv_anime::store::Recovery], titles: &HashMap<String, String>) -> String {
    let one = |r: &mv_anime::store::Recovery| {
        let project = titles.get(&r.project).filter(|t| !t.trim().is_empty()).map(|t| format!(" of {t}")).unwrap_or_default();
        match &r.restored {
            Some(when) => format!("A damaged file was found: {}{project}. It was put back as it was {when}; anything changed after that is lost.", r.what),
            None => format!("A damaged file was found: {}{project}. It had to start empty; what it held is lost.", r.what),
        }
    };
    let mut text: Vec<String> = list.iter().take(3).map(one).collect();
    if list.len() > 3 {
        text.push(format!("{} more files were put back the same way.", list.len() - 3));
    }
    text.push(if list.len() == 1 { "The damaged copy is kept in the project's folder.".into() } else { "The damaged copies are kept in the project's folder.".into() });
    text.join(" ")
}

fn versions() -> [u64; 4] {
    [queue::version(), mv_anime::store::version(), ui_studio::pipeline::version(), backends::site_version()]
}

// ---------------------------------------------------------------- view

impl Render for EnginePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = if self.open.is_some() {
            self.project_view(window, cx).into_any_element()
        } else if self.opening.is_some() {
            self.opening_view(cx).into_any_element()
        } else {
            self.home_view(window, cx).into_any_element()
        };
        // On the home, the strip shows only while there is something in the queue.
        let all = summarize(&self.queue, None);
        let strip = (self.open.is_none() && (all.active() > 0 || all.failed > 0 || self.queue.interrupted)).then(|| self.strip.view(&self.queue, &Eta::current(), None, cx));
        // Project files found damaged and put back: what, from when, what was lost, where the
        // damaged copy is. Stays until dismissed.
        // Each names its project; one whose project left the list goes with it.
        let recovered = (!self.recoveries.is_empty()).then(|| {
            let text = recovery_text(&self.recoveries, &self.titles());
            let page = cx.entity().downgrade();
            div().flex_none().px(px(GUTTER)).pt(px(GUTTER)).child(banner(AlertKind::Warning, text).action("Dismiss", move |_, _, cx| {
                let _ = page.update(cx, |this, cx| {
                    this.recoveries.clear();
                    cx.notify();
                });
            }))
        });
        div()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    // A click on the page keeps its shortcuts working (Alt+1..8).
                    if !this.focus_handle.contains_focused(window, cx) {
                        window.focus(&this.focus_handle, cx);
                    }
                }),
            )
            .size_full()
            .flex()
            .flex_col()
            .bg(Tokens::canvas())
            .children(recovered)
            .child(div().flex_1().min_h_0().child(body))
            .children(strip)
    }
}

impl EnginePage {
    fn opening_view(&self, cx: &mut Context<Self>) -> Div {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .h(px(HEADER_H))
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .px(px(space::LG))
                    .border_b_1()
                    .border_color(Tokens::border())
                    .child(icon_button("ae-back", ph::ARROW_LEFT, "All projects").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.close_project(cx))))
                    .child(mv_ui::skeleton_block(px(220.), px(18.))),
            )
            .child(div().flex_1().flex().items_center().justify_center().gap(px(space::SM)).child(spinner("ae-opening", Tokens::accent(), icon_px::MD)).child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child("Opening the project…")))
    }

    fn project_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let view = self.step_view(window, cx);
        let header = self.header(window, cx);
        let Some(o) = &self.open else { return div() };
        let chapters: Vec<String> = o.episode.chapters.iter().map(|c| c.chapter_id.clone()).collect();
        let states = stepbar::job_states(&self.queue, &o.store.id, &o.episode.id, &self.studio, &chapters);
        let marks = stepbar::marks(&states, &o.done);
        let page = cx.entity().downgrade();
        let bar = stepbar::bar(o.step, &marks, is_narrow(window), move |s, _, cx| {
            let _ = page.update(cx, |p, cx| p.set_step(s, cx));
        });
        // Only when picture jobs really wait on the user (the lane is held with jobs in it):
        // being signed out is the picture panel's to say. One sentence for what stopped them,
        // one action.
        let waiting_pictures = self.queue.jobs.iter().any(|j| j.lane == Lane::Web && matches!(j.state, JobState::Queued | JobState::Paused | JobState::Running));
        let held = (web_held(&self.queue) && waiting_pictures).then(|| {
            let site = backends::site();
            let (text, action) = backends::held_for(&site);
            div().flex_none().px(px(GUTTER)).pt(px(GUTTER)).child(banner(AlertKind::Warning, text).action(action, move |_, _, cx| backends::act_on_held(&site, cx)))
        });
        let project = o.store.id.clone();
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(header)
            .child(bar)
            .children(held)
            .child(div().flex_1().min_h_0().p(px(GUTTER)).child(view))
            .child(self.strip.view(&self.queue, &Eta::current(), Some(&project), cx))
    }

    /// The current step's view, made once per (step, episode).
    fn step_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let page = cx.entity().downgrade();
        let Some(o) = self.open.as_mut() else { return div().into_any_element() };
        let key = (o.step, o.episode.id.clone());
        if let Some(v) = o.views.get(&key) {
            return v.clone().into_any_element();
        }
        let ctx = StepCtx {
            project: o.store.id.clone(),
            episode: o.episode.id.clone(),
            go: Rc::new(move |s, _, cx| {
                let _ = page.update(cx, |p, cx| p.set_step(s, cx));
            }),
        };
        let v = steps::open(o.step, ctx, window, cx);
        o.views.insert(key, v.clone());
        v.into_any_element()
    }

    fn header(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let _ = window;
        let Some(o) = &self.open else { return div() };
        let page = cx.entity().downgrade();
        let current = o.store.id.clone();
        let title = o.series.manga_title.clone();

        // Project menu: switch project, or go back to all projects.
        let mut items: Vec<MenuItem> = Vec::new();
        if let Load::Ready(rows) = &self.projects {
            for r in rows {
                let (id, p) = (r.series.id.clone(), page.clone());
                items.push(MenuItem::new(r.series.manga_title.clone(), move |w, cx| {
                    let _ = p.update(cx, |p, cx| p.open_project(id.clone(), w, cx));
                })
                .checked(r.series.id == current));
            }
            items.push(MenuItem::separator());
        }
        let p = page.clone();
        items.push(MenuItem::new("All projects", move |_, cx| {
            let _ = p.update(cx, |p, cx| p.close_project(cx));
        })
        .icon(ph::SQUARES_FOUR));
        let project_menu = menu_button("ae-project-menu", ph::FILM_STRIP, "Switch project", items).label(title.clone()).anchor(Anchor::TopLeft).min_width(240.);

        // Episode menu: switch, add, rename.
        let mut eps: Vec<MenuItem> = Vec::new();
        for r in &o.series.episodes {
            let (id, p) = (r.id.clone(), page.clone());
            let label = if r.title.trim().is_empty() { r.id.clone() } else { r.title.clone() };
            eps.push(MenuItem::new(label, move |_, cx| {
                let _ = p.update(cx, |p, cx| p.set_episode(id.clone(), cx));
            })
            .checked(r.id == o.episode.id));
        }
        eps.push(MenuItem::separator());
        let p = page.clone();
        eps.push(MenuItem::new("New episode", move |_, cx| {
            let _ = p.update(cx, |p, cx| p.new_episode(cx));
        })
        .icon(ph::PLUS));
        let p = page.clone();
        eps.push(MenuItem::new("Rename episode", move |w, cx| {
            let _ = p.update(cx, |p, cx| p.start_rename(w, cx));
        })
        .icon(ph::PENCIL_SIMPLE));
        let ep_title = if o.episode.title.trim().is_empty() { o.episode.id.clone() } else { o.episode.title.clone() };
        let episode: AnyElement = match &self.rename {
            Some(input) => div().w(px(220.)).child(Input::new(input)).into_any_element(),
            None => menu_button("ae-episode-menu", ph::STACK, "Episodes", eps).label(ep_title).anchor(Anchor::TopLeft).min_width(220.).into_any_element(),
        };

        // Queue chip: what runs, opens the strip.
        let all = summarize(&self.queue, None);
        let held = self.queue.paused || self.queue.interrupted;
        let expanded = self.strip.expanded;
        let chip = interactive("ae-queue-chip")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .px(px(10.))
            .rounded(px(radius::MD))
            .border_1()
            .border_color(if expanded { Tokens::border_strong() } else { Tokens::border() })
            .when(expanded, |d| d.bg(Tokens::active()))
            .text_size(px(size::SM))
            .text_color(Tokens::text_1())
            .tooltip(tip(if expanded { "Fold the queue" } else { "Show everything in the queue" }))
            .map(|d| if all.running > 0 && !held { d.child(spinner("ae-queue-spin", Tokens::accent(), icon_px::SM)) } else { d.child(icon_ph(ph::QUEUE, Tokens::text_2(), icon_px::SM)) })
            .child(summary_text(&all, held))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.strip.expanded = !this.strip.expanded;
                cx.notify();
            }));

        // More: folder, Magnific check, pictures per Generate, the AI, remove.
        let folder = o.store.dir.clone();
        let variants = o.series.settings.variants.clamp(1, 4);
        let mut more = vec![
            MenuItem::new("Open project folder", move |_, cx| {
                let _ = std::fs::create_dir_all(&folder);
                cx.open_with_system(&folder);
            })
            .icon(ph::FOLDER_OPEN),
            MenuItem::new("Check Magnific", |_, cx| {
                backends::check_site();
                toast::show(cx, Toast::info("Checking Magnific in its window. Nothing is uploaded or generated.").icon(ph::PLUGS_CONNECTED));
            })
            .icon(ph::PLUGS_CONNECTED),
            MenuItem::separator(),
            MenuItem::heading("Pictures per Generate"),
        ];
        for n in 1..=4u8 {
            let p = page.clone();
            more.push(MenuItem::new(format!("{n} picture{}", crate::widgets::plural(n as usize)), move |_, cx| {
                let _ = p.update(cx, |p, cx| p.set_variants(n, cx));
            })
            .checked(variants == n));
        }
        more.push(MenuItem::separator());
        more.push(MenuItem::heading(format!("Writes with {}", ui_assistant::model_label(cx))));
        more.push(MenuItem::new("Change the AI model in Settings…", |w, cx| mv_ui::navigate(Route::Settings { tab: Some("providers".into()) }, w, cx)).icon(ph::GEAR_SIX));
        more.push(MenuItem::separator());
        let (p, id) = (page.clone(), current.clone());
        more.push(MenuItem::new("Remove from list", move |w, cx| home::confirm_remove(p.clone(), id.clone(), title.clone(), w, cx)).icon(ph::TRASH).danger());

        div()
            .flex_none()
            .h(px(HEADER_H))
            .flex()
            .items_center()
            .gap(px(space::SM))
            .px(px(space::LG))
            .border_b_1()
            .border_color(Tokens::border())
            .child(icon_button("ae-back", ph::ARROW_LEFT, "All projects").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.close_project(cx))))
            .child(div().flex_none().w(px(1.)).h(px(20.)).bg(Tokens::border()))
            .child(div().min_w_0().child(project_menu))
            .child(icon_ph(ph::CARET_RIGHT, Tokens::text_3(), icon_px::XS))
            .child(div().min_w_0().child(episode))
            .child(div().flex_none().text_size(px(size::SM)).text_color(Tokens::text_3()).child(chapter_range(&o.episode.chapters)))
            .child(badge("BETA", Tone::Warning))
            .child(div().flex_1())
            .child(chip)
            .child(overflow_menu("ae-more", more).min_width(240.))
    }
}

#[cfg(test)]
mod tests {
    use super::{chapter_range, next_episode_id, recovery_text};
    use mv_anime::store::Recovery;
    use mv_anime::{ChapterPick, EpisodeRef};
    use std::collections::HashMap;

    #[test]
    fn a_file_put_back_says_what_was_lost_and_where_the_copy_is() {
        let titles: HashMap<String, String> = [("p1".to_string(), "Solo Leveling".to_string())].into();
        let one = recovery_text(&[Recovery { project: "p1".into(), what: "the story of episode 1".into(), restored: Some("8 minutes ago".into()) }], &titles);
        assert_eq!(one, "A damaged file was found: the story of episode 1 of Solo Leveling. It was put back as it was 8 minutes ago; anything changed after that is lost. The damaged copy is kept in the project's folder.");
        let empty = recovery_text(&[Recovery { project: "gone".into(), what: "the voices".into(), restored: None }], &titles);
        assert!(empty.starts_with("A damaged file was found: the voices. It had to start empty") && !empty.contains("gone"), "never the project's id");
    }

    fn ch(n: Option<f64>) -> ChapterPick {
        ChapterPick { number: n, ..Default::default() }
    }

    #[test]
    fn chapter_ranges_read_naturally() {
        assert_eq!(chapter_range(&[]), "No chapters yet");
        assert_eq!(chapter_range(&[ch(Some(3.))]), "Ch. 3");
        assert_eq!(chapter_range(&[ch(Some(2.)), ch(Some(1.))]), "Ch. 1–2");
        assert_eq!(chapter_range(&[ch(Some(4.5)), ch(Some(6.))]), "Ch. 4.5–6");
        assert_eq!(chapter_range(&[ch(Some(1.)), ch(None)]), "2 chapters");
    }

    #[test]
    fn episode_ids_never_collide() {
        let r = |id: &str| EpisodeRef { id: id.into(), title: String::new() };
        assert_eq!(next_episode_id(&[]), "ep01");
        assert_eq!(next_episode_id(&[r("ep01")]), "ep02");
        assert_eq!(next_episode_id(&[r("ep01"), r("ep03")]), "ep04");
        assert_eq!(next_episode_id(&[r("ep02")]), "ep03");
    }
}
