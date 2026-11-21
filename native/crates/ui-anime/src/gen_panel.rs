//! The one Generation panel (design 9.3), laid out like Magnific's left panel and used by the
//! Bible sheets and plates, Frames and the style frame: the model with its Free mark, the
//! references (n / 14, faces n / 4) each captioned with its job, the prompt (the AI's, editable,
//! with "Reset to the AI's suggestion" and a pinned state once edited), the size pills, the
//! primary Generate with the credit-free line under it, and the states around a job: Magnific
//! signed out or waiting for a person, waiting in the queue, drawing, done and failed.
//!
//! It is an entity. The step owns the card; the panel shows its `GenSpec`, edits a copy and
//! hands every change back through `on_change`, which the step saves:
//!
//! ```ignore
//! let panel = cx.new(|_| GenPanel::new(Target::ShotFrame { episode, shot }, card.gen.clone())
//!     .project(&ctx.project)
//!     .title("Frame 3.4")
//!     .on_change(move |spec, window, cx| { /* card.gen = spec; save */ }));
//! // when the selection or the compiled prompt changes:
//! panel.update(cx, |p, cx| { p.set(card.gen.clone(), Some(compiled)); cx.notify() });
//! // render: .child(panel.clone())
//! ```
//!
//! Generate enqueues the job for the target itself (`JobKind::Frame` or `JobKind::Sheet`, with
//! the spec's variants). Callbacks run after the panel's own update, so they may update the panel
//! again.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_anime::queue::{self, Job, JobKind, JobState, Lane};
use mv_anime::{Compiled, GenSpec, RefRole, RefSlot, SiteSlot, Store, Take, TakeKind, TakeMeta, TakeSource, Target};
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::{appear, btn, disclosure, icon_ph, ph, popup_surface, progress_bar, size, spinner, tag, tip, toast, ButtonKind, Ph, Toast, Tokens, Tone};

use crate::backends::{self, Models, Site};
use crate::queue_strip::Eta;
use crate::style::{POLL_MS, REF_TILE};
use crate::widgets::{ago, error_block, field, info_line, label_caps, now_secs, pill, LineTone};

/// Called with the edited spec (prompt override, references, model, size, variants).
pub type OnChange = Rc<dyn Fn(GenSpec, &mut Window, &mut App)>;

/// Most references Magnific takes, and most identity references per picture (design 6.6).
pub const MAX_REFS: usize = mv_magnific::MAX_REFS;
pub const MAX_FACES: usize = 4;
const MAX_VARIANTS: u8 = 4;
const ASPECTS: [&str; 5] = ["16:9", "9:16", "1:1", "4:3", "21:9"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Menu {
    Model,
    Aspect,
    Variants,
}

/// Where this panel's latest job stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Busy {
    Idle,
    /// In the queue; `ahead` jobs wait before it on the Web lane.
    Waiting { id: u64, ahead: usize },
    Working { id: u64, progress: f32, line: String, started: Option<i64> },
    Failed { id: u64, error: String },
    Done { at: Option<i64> },
}

pub struct GenPanel {
    target: Target,
    spec: GenSpec,
    compiled: Option<Compiled>,
    prompt: Option<Entity<TextareaState>>,
    on_change: Option<OnChange>,
    project: Option<String>,
    title: SharedString,
    label: Option<String>,
    lock_aspect: bool,
    menu: Option<Menu>,
    /// The prompt box shows something else than the spec's text (set it on the next render).
    resync: bool,
    /// The prompt changed since the last `on_change`.
    typed: bool,
    importing: bool,
    jobs: Vec<Job>,
    web_held: bool,
    seen: (u64, u64),
    /// What the poll last saw of the outside ([`Shown`]).
    last: Option<Shown>,
    polling: bool,
    /// This target's pictures from Magnific with their Magnific id, newest first, and the store
    /// version they were read at (read in the background).
    made: Vec<mv_anime::sheets::MadeOnMagnific>,
    made_seen: Option<u64>,
    made_reading: bool,
    _subs: Vec<Subscription>,
}

// ---------------------------------------------------------------- pure parts

/// A picture's Magnific id as the take list shows it.
pub fn made_label(site_id: &str) -> String {
    format!("Magnific #{}", site_id.trim())
}

/// Who pressed Generate for a picture, as its tooltip says.
pub fn made_tip(pressed_by_app: bool) -> &'static str {
    if pressed_by_app {
        "Made by MangaVox on Magnific"
    } else {
        "Generate pressed by you at the window"
    }
}

/// The AI's prompt: the compiled text when there is one, else the spec's `prompt_ai`.
pub fn ai_text<'a>(spec: &'a GenSpec, compiled: Option<&'a Compiled>) -> &'a str {
    match compiled {
        Some(c) if !c.text.trim().is_empty() => &c.text,
        _ => &spec.prompt_ai,
    }
}

/// What the prompt box shows: the user's override, or the AI's prompt.
pub fn shown_text(spec: &GenSpec, compiled: Option<&Compiled>) -> String {
    spec.prompt_user.clone().unwrap_or_else(|| ai_text(spec, compiled).to_string())
}

/// Applies the prompt box's text: the AI's own text clears the override; anything else pins it
/// (with the hash of `prompt_ai` it overrode, design 5.3). Returns whether the spec changed.
pub fn apply_prompt(spec: &mut GenSpec, text: &str, compiled: Option<&Compiled>) -> bool {
    let before = (spec.prompt_user.clone(), spec.prompt_user_base.clone());
    if text == ai_text(spec, compiled) {
        spec.prompt_user = None;
        spec.prompt_user_base = None;
    } else {
        spec.prompt_user = Some(text.to_string());
        if before.0.is_none() || spec.prompt_user_base.is_none() {
            spec.prompt_user_base = Some(mv_anime::hash::sha256_hex(spec.prompt_ai.as_bytes()));
        }
    }
    before != (spec.prompt_user.clone(), spec.prompt_user_base.clone())
}

/// "Reset to the AI's suggestion".
pub fn reset_prompt(spec: &mut GenSpec) {
    spec.reset_prompt()
}

/// The AI rewrote its prompt after the user pinned theirs.
pub fn suggestion_moved(spec: &GenSpec) -> bool {
    spec.prompt_moved()
}

/// The queue job that draws `target` (`None` for targets that are not pictures).
pub fn job_for(target: &Target, variants: u8) -> Option<JobKind> {
    let variants = variants.clamp(1, MAX_VARIANTS);
    match target {
        Target::ShotFrame { episode, shot } => Some(JobKind::Frame { episode: episode.clone(), shot: shot.clone(), variants }),
        Target::Sheet { .. } | Target::PlaceSheet { .. } | Target::Plate { .. } | Target::StyleFrame => Some(JobKind::Sheet { target: target.clone(), variants }),
        _ => None,
    }
}

/// `kind` draws `target`.
pub fn job_matches(kind: &JobKind, target: &Target) -> bool {
    match (kind, target) {
        (JobKind::Frame { episode, shot, .. }, Target::ShotFrame { episode: e, shot: s }) => episode == e && shot == s,
        (JobKind::Sheet { target: t, .. }, other) => t == other,
        _ => false,
    }
}

/// The queue's name for a picture job when the step gives none.
pub fn default_label(target: &Target) -> String {
    match target {
        Target::ShotFrame { shot, .. } => format!("Frame {shot}"),
        Target::Sheet { owner, sheet } => format!("{sheet} sheet · {owner}"),
        Target::PlaceSheet { place } => format!("Place {place}"),
        Target::Plate { place, station, state } => format!("Plate {place} · {station} · {state}"),
        Target::StyleFrame => "Style frame".into(),
        _ => "Picture".into(),
    }
}

/// A reference tile's caption: its job in a word or two (the name is in the tooltip).
pub fn ref_caption(role: &RefRole) -> &'static str {
    match role {
        RefRole::Plate => "room + camera",
        RefRole::Layout => "layout",
        RefRole::Identity(_) => "identity",
        RefRole::Style => "style",
        RefRole::Expression(_) => "expression",
        RefRole::Prop(_) => "prop",
        RefRole::Grip(_) => "grip",
        RefRole::ColourModel(_) => "colours",
        RefRole::PlaceView => "place",
        RefRole::Panel => "manga panel",
        RefRole::User | RefRole::Unknown => "your picture",
    }
}

/// (references, identity references).
pub fn ref_counts(refs: &[RefSlot]) -> (usize, usize) {
    (refs.len(), refs.iter().filter(|r| matches!(r.role, RefRole::Identity(_))).count())
}

/// Where the latest job of `target` in `project` stands, from the queue's jobs.
pub fn busy_of(jobs: &[Job], project: &str, target: &Target) -> Busy {
    let Some(j) = jobs.iter().filter(|j| j.project == project && job_matches(&j.kind, target)).max_by_key(|j| j.id) else { return Busy::Idle };
    match j.state {
        JobState::Running => Busy::Working { id: j.id, progress: j.progress, line: j.line.clone(), started: j.started_at },
        JobState::Queued | JobState::Paused => Busy::Waiting { id: j.id, ahead: jobs.iter().filter(|o| o.lane == Lane::Web && o.id < j.id && matches!(o.state, JobState::Queued | JobState::Running)).count() },
        JobState::Failed if !j.hidden => Busy::Failed { id: j.id, error: j.error.clone().unwrap_or_else(|| "The picture could not be made.".into()) },
        JobState::Failed => Busy::Idle,
        JobState::Done => Busy::Done { at: j.finished_at },
    }
}

/// The footer's one main action (the big button), in every state of the site and the job.
#[derive(Clone, Debug, PartialEq)]
pub enum Main {
    /// Generate; disabled with the reason when `Some`.
    Generate(Option<&'static str>),
    /// Signed out: bring the Magnific window forward to sign in.
    SignIn,
    /// The window was closed: open it again (as Sign in does).
    OpenAgain,
    /// After signing in, a check for a person or a held lane: check the page, and let pictures
    /// run again when it is ready.
    Continue,
    /// Look at the page again (a price, a warning, a changed page, a check that failed); the
    /// label says which.
    CheckAgain(&'static str),
    /// The picture failed: queue it again.
    Retry(u64),
    /// Nothing to press: what is happening now (drawing, waiting, checking).
    Status(String),
}

/// Exactly one main action for the panel's state. `blocked` is why Generate cannot run when
/// nothing else stops it (no project, too many references, no prompt).
pub fn main_action(site: &Site, held: bool, busy: &Busy, blocked: Option<&'static str>, now: i64) -> Main {
    match (site, busy) {
        (_, Busy::Working { started, .. }) => Main::Status(format!("Drawing… {}", started.map(|t| crate::widgets::fmt_duration((now - crate::widgets::unix_secs(t)).max(0) as u64)).unwrap_or_default()).trim_end().to_string()),
        (Site::Checking, _) => Main::Status("Checking Magnific…".into()),
        (Site::NoWindow, _) => Main::OpenAgain,
        (Site::SignedOut, _) => Main::SignIn,
        (Site::SigningIn | Site::NeedsYou, _) => Main::Continue,
        (Site::NotFree(_) | Site::Warning, _) => Main::CheckAgain("Check again"),
        (Site::Changed(_) | Site::Failed(_), _) => Main::CheckAgain("Try again"),
        _ if held => Main::Continue,
        (_, Busy::Waiting { ahead, .. }) if *ahead > 0 => Main::Status(format!("Waiting · {ahead} ahead")),
        (_, Busy::Waiting { .. }) => Main::Status("Waiting to start".into()),
        (_, Busy::Failed { id, .. }) => Main::Retry(*id),
        _ => Main::Generate(blocked),
    }
}

/// The one sentence above the button that says what stops pictures (none when nothing does).
pub fn site_line(site: &Site, held: bool) -> Option<(LineTone, &'static str)> {
    match site {
        Site::NoWindow => Some((LineTone::Warning, "The Magnific window was closed.")),
        Site::SignedOut => Some((LineTone::Warning, "Sign in to Magnific to draw pictures here.")),
        Site::SigningIn => Some((LineTone::Info, "Sign in to Magnific in its window, then press Continue.")),
        Site::NeedsYou => Some((LineTone::Warning, "Magnific wants to check that you are a person. Do that in its window, then press Continue.")),
        Site::NotFree(_) => Some((LineTone::Warning, mv_magnific::PRICE_SHOWN)),
        Site::Warning => Some((LineTone::Warning, mv_magnific::WARNING_SHOWN)),
        Site::Changed(_) => Some((LineTone::Warning, "Magnific's page looks different from what MangaVox knows, so pictures are paused.")),
        Site::Failed(_) => Some((LineTone::Warning, "MangaVox could not reach Magnific's page just now.")),
        _ if held => Some((LineTone::Warning, "Pictures are paused. Press Continue to go on.")),
        _ => None,
    }
}

/// What the panel shows that comes from outside it (the job, the held lane, the site, its
/// models, the model that drew last): the poll draws again only when this changes.
type Shown = (Busy, bool, Site, Models, Option<String>);

/// Polls between two redraws of a drawing job's clock (about a second).
const CLOCK_TICKS: u64 = 1000 / POLL_MS + 1;

// ---------------------------------------------------------------- building

impl GenPanel {
    pub fn new(target: Target, spec: GenSpec) -> Self {
        GenPanel {
            target,
            spec,
            compiled: None,
            prompt: None,
            on_change: None,
            project: None,
            title: SharedString::default(),
            label: None,
            lock_aspect: false,
            menu: None,
            resync: true,
            typed: false,
            importing: false,
            jobs: Vec::new(),
            web_held: false,
            seen: (u64::MAX, u64::MAX),
            last: None,
            polling: false,
            made: Vec::new(),
            made_seen: None,
            made_reading: false,
            _subs: Vec::new(),
        }
    }

    /// The project (series id): needed for references, uploads and the panel's own enqueue.
    pub fn project(mut self, id: &str) -> Self {
        self.project = Some(id.to_string());
        self
    }

    /// What is drawn ("Frame 3.4", "Jinwoo · Face"), shown at the top.
    pub fn title(mut self, t: impl Into<SharedString>) -> Self {
        self.title = t.into();
        self
    }

    /// The queue's name for the job (default from the target).
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = Some(l.into());
        self
    }

    /// The target has a fixed aspect (the aspect pill shows it but has no menu).
    pub fn lock_aspect(mut self) -> Self {
        self.lock_aspect = true;
        self
    }

    pub fn on_change(mut self, f: impl Fn(GenSpec, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }

    /// New spec (and compiled prompt) for the selected item. Call `cx.notify()` after.
    /// While the user is typing in the prompt box, their text is kept.
    pub fn set(&mut self, spec: GenSpec, compiled: Option<Compiled>) {
        if spec != self.spec || compiled != self.compiled {
            self.spec = spec;
            self.compiled = compiled;
            self.resync = true;
        }
    }

    fn store(&self) -> Option<Store> {
        self.project.as_deref().map(Store::open)
    }

    fn emit_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.typed = false;
        if let Some(f) = self.on_change.clone() {
            let spec = self.spec.clone();
            window.defer(cx, move |w, cx| f(spec, w, cx));
        }
        cx.notify();
    }

    /// Hands a typed prompt to the step (on blur and before Generate).
    fn flush(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.typed {
            self.emit_change(window, cx);
        }
    }

    fn ensure_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prompt.is_some() {
            return;
        }
        let state = cx.new(|cx| TextareaState::new(window, cx).auto_grow(4, 9).placeholder("Describe the picture: who, where, the camera and the mood"));
        let sub = cx.subscribe_in(&state, window, |this, st, ev: &InputEvent, window, cx| match ev {
            InputEvent::Change => {
                let text = st.read(cx).value().to_string();
                let compiled = this.compiled.clone();
                if apply_prompt(&mut this.spec, &text, compiled.as_ref()) {
                    this.typed = true;
                    cx.notify();
                }
            }
            InputEvent::Blur => this.flush(window, cx),
            _ => {}
        });
        self.prompt = Some(state);
        self._subs.push(sub);
        self.resync = true;
    }

    /// Puts the spec's text in the prompt box unless the user is typing there.
    fn sync_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.resync {
            return;
        }
        let Some(state) = self.prompt.clone() else { return };
        let focused = state.read(cx).focus_handle(cx).contains_focused(window, cx);
        if focused && self.typed {
            return;
        }
        self.resync = false;
        let want = shown_text(&self.spec, self.compiled.as_ref());
        if *state.read(cx).value() != *want {
            state.update(cx, |s, cx| s.set_value(want, window, cx));
        }
    }

    fn start_poll(&mut self, cx: &mut Context<Self>) {
        if self.polling {
            return;
        }
        self.polling = true;
        backends::check_site_once();
        self.refresh();
        self.last = Some(self.shown());
        cx.spawn(async move |this, cx| {
            let mut ticks: u64 = 0;
            loop {
                cx.background_executor().timer(Duration::from_millis(POLL_MS)).await;
                // Nothing is read while the tab is not on screen (it is read again when it is).
                if !crate::tab_shown() {
                    if this.upgrade().is_none() {
                        break;
                    }
                    continue;
                }
                ticks = ticks.wrapping_add(1);
                let v = (queue::version(), backends::site_version());
                let alive = this.update(cx, |p, cx| {
                    // Draws again only when what it shows changed, or once a second for the
                    // clock of a picture being drawn right now.
                    if p.seen != v {
                        p.read_made(cx);
                        p.refresh();
                        let now = p.shown();
                        if p.last.as_ref() != Some(&now) {
                            p.last = Some(now);
                            cx.notify();
                            return;
                        }
                    }
                    if ticks % CLOCK_TICKS == 0 && matches!(p.busy(), Busy::Working { .. }) {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Reads this target's Magnific pictures again in the background when the project's files
    /// changed since the last read (a new picture was stored).
    fn read_made(&mut self, cx: &mut Context<Self>) {
        let ver = mv_anime::store::version();
        if self.made_reading || self.made_seen == Some(ver) {
            return;
        }
        let Some(store) = self.store() else { return };
        self.made_reading = true;
        let target = self.target.clone();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || mv_anime::sheets::made_on_magnific(&store, &target)).await;
            let _ = this.update(cx, |p, cx| {
                p.made_reading = false;
                p.made_seen = Some(ver);
                if let Ok(made) = res {
                    if made != p.made {
                        p.made = made;
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// What the panel shows from outside itself, as of now.
    fn shown(&self) -> Shown {
        (self.busy(), self.web_held, backends::site(), backends::models(), backends::fallback_model(&self.spec.model))
    }

    /// Re-reads this target's jobs, the Web lane and the time estimates.
    fn refresh(&mut self) {
        let v = (queue::version(), backends::site_version());
        if v.0 != self.seen.0 {
            let q = queue::snapshot();
            self.web_held = crate::queue_strip::web_held(&q);
            self.jobs = q.jobs.into_iter().filter(|j| j.lane == Lane::Web).collect();
        }
        self.seen = v;
    }

    fn busy(&self) -> Busy {
        match &self.project {
            Some(p) => busy_of(&self.jobs, p, &self.target),
            None => Busy::Idle,
        }
    }

    fn set_menu(&mut self, m: Menu, cx: &mut Context<Self>) {
        self.menu = if self.menu == Some(m) { None } else { Some(m) };
        if m == Menu::Model && self.menu.is_some() && matches!(backends::models(), Models::Failed(_)) {
            backends::load_models();
        }
        cx.notify();
    }

    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        reset_prompt(&mut self.spec);
        self.resync = true;
        if let Some(state) = self.prompt.clone() {
            let want = shown_text(&self.spec, self.compiled.as_ref());
            state.update(cx, |s, cx| s.set_value(want, window, cx));
            self.resync = false;
        }
        self.emit_change(window, cx);
    }

    fn generate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A typed prompt reaches the step first (deferred callbacks run in order).
        self.flush(window, cx);
        let (Some(project), Some(kind)) = (self.project.clone(), job_for(&self.target, self.spec.variants.max(1))) else { return };
        let label = self.label.clone().unwrap_or_else(|| default_label(&self.target));
        window.defer(cx, move |_, cx| {
            backends::ensure_runner(cx);
            queue::enqueue(kind, &project, label);
        });
    }

    // ------------------------------------------------------------ references

    /// Where the reference `key` (its take hash and file) is in the list now: actions find their
    /// reference by what it is, never by where it was when the tile was drawn.
    fn ref_index(&self, key: &RefKey) -> Option<usize> {
        self.spec.refs.iter().position(|r| r.hash == key.0 && r.file == key.1)
    }

    fn pick_files(&mut self, replace: Option<RefKey>, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: replace.is_none(), prompt: Some(if replace.is_some() { "Use this picture".into() } else { "Add as references".into() }) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let _ = this.update_in(cx, |this, window, cx| this.import(paths, replace, window, cx));
        })
        .detach();
    }

    /// Copies pictures into the project as Upload takes, then adds them (or replaces one).
    fn import(&mut self, paths: Vec<PathBuf>, replace: Option<RefKey>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.store() else { return };
        let room = MAX_REFS.saturating_sub(self.spec.refs.len());
        let paths: Vec<PathBuf> = if replace.is_some() { paths.into_iter().take(1).collect() } else { paths.into_iter().take(room).collect() };
        if paths.is_empty() {
            if replace.is_none() {
                toast::show(cx, Toast::info(format!("Magnific takes {MAX_REFS} references at most. Remove one first.")));
            }
            return;
        }
        self.importing = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let res = mv_ui::bg::blocking(move || paths.iter().map(|p| import_one(&store, p)).collect::<Vec<_>>()).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.importing = false;
                let results = match res {
                    Ok(r) => r,
                    Err(e) => vec![Err(format!("{e:#}"))],
                };
                let mut added = false;
                // The reference to replace, found again now (the list may have changed while the
                // file dialog was open); gone meanwhile: the picture is added instead.
                let at = replace.as_ref().and_then(|k| this.ref_index(k));
                for r in results {
                    match r {
                        Ok(take) => {
                            added = true;
                            match at.and_then(|i| this.spec.refs.get_mut(i)) {
                                Some(slot) => {
                                    slot.file = take.file.clone();
                                    slot.hash = take.id.clone();
                                    slot.pinned = true;
                                }
                                None => this.spec.refs.push(RefSlot { file: take.file.clone(), hash: take.id.clone(), role: RefRole::User, job: "added by you".into(), slot: SiteSlot::Add, pinned: true }),
                            }
                        }
                        Err(e) => toast::show(cx, Toast::warning(e)),
                    }
                }
                if added {
                    this.spec.refs_user = true;
                    this.emit_change(window, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Changes the reference `key` where it is now; nothing when it is no longer in the list.
    fn edit_ref(&mut self, key: &RefKey, f: impl FnOnce(&mut Vec<RefSlot>, usize), window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.ref_index(key) {
            f(&mut self.spec.refs, i);
            self.spec.refs_user = true;
            self.emit_change(window, cx);
        }
    }
}

/// A reference by what it is: (take hash, file).
type RefKey = (String, String);

/// One picture into the project's take store (PNG, JPG or WebP).
fn import_one(store: &Store, path: &std::path::Path) -> Result<Take, String> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let ext = match ext.as_str() {
        "png" => "png",
        "jpg" | "jpeg" => "jpg",
        "webp" => "webp",
        _ => return Err(format!("{name}: only PNG, JPG or WebP pictures can be references.")),
    };
    let bytes = std::fs::read(path).map_err(|e| format!("Could not read {name}: {e}"))?;
    let meta = TakeMeta { kind: TakeKind::Image, target: Target::Upload, source: TakeSource::Upload { name: name.clone() }, ..Default::default() };
    store.add_take(&bytes, ext, meta).map_err(|e| format!("Could not add {name} to the project: {e:#}"))
}

// ---------------------------------------------------------------- view

impl Render for GenPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_input(window, cx);
        self.start_poll(cx);
        self.read_made(cx);
        self.sync_text(window, cx);
        let busy = self.busy();
        // Only the settings above scroll; the size chips and the main button are a fixed footer,
        // so Generate (or Sign in, Continue, Drawing…) is always in the same place.
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .child(
                div().id("ag-scroll").flex_1().min_h_0().overflow_y_scroll().child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(space::LG))
                        .p(px(space::MD))
                        .child(self.head())
                        .child(self.model_section(cx))
                        .child(self.references(cx))
                        .child(self.prompt_section(window, cx))
                        .child(self.made_section(cx)),
                ),
            )
            .child(self.footer(&busy, cx))
    }
}

impl GenPanel {
    fn head(&self) -> Div {
        let res = if self.spec.res.is_empty() { "1K" } else { self.spec.res.as_str() };
        div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .p(px(space::MD))
            .rounded(px(radius::LG))
            .bg(Tokens::surface_2())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("Magnific website · {res}")))
                    .child(div().truncate().text_size(px(size::MD)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child(if self.title.is_empty() { SharedString::from("Generate") } else { self.title.clone() })),
            )
            .child(icon_ph(ph::SPARKLE, Tokens::accent(), icon_px::MD))
    }

    fn model_section(&self, cx: &mut Context<Self>) -> Div {
        let open = self.menu == Some(Menu::Model);
        let id = if self.spec.model.is_empty() { "nano-banana-2-1".to_string() } else { self.spec.model.clone() };
        let free = match backends::models() {
            Models::Ready(v) => v.iter().find(|m| backends::same_model(&m.cy, &id)).map(|m| m.free),
            _ => None,
        };
        let button = field("ag-model", Some(ph::CUBE), backends::model_name(&id), open)
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.set_menu(Menu::Model, cx)))
            .when_some(free, |d, f| d.child(if f { tag("Free", Tone::Success) } else { tag("Credits", Tone::Warning) }));
        let row = div().relative().flex().items_center().child(button).when(open, |d| d.child(self.model_menu(&id, cx)));
        div().flex().flex_col().gap(px(space::SM)).child(label_caps("Model")).child(row).child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("Drawn on the Magnific website, credit-free models only"))
    }

    fn model_menu(&self, current: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let mut pop = popup_surface().w(px(280.)).p(px(space::SM)).gap(px(2.)).on_mouse_down_out(cx.listener(|this, _, _, cx| {
            this.menu = None;
            cx.notify();
        }));
        let row = |k: usize, id: String, name: String, free: Option<bool>, cx: &mut Context<Self>| {
            let on = backends::same_model(&id, current);
            let usable = free != Some(false);
            div()
                .id(("ag-model-row", k))
                .flex()
                .items_center()
                .gap(px(space::SM))
                .h(px(32.))
                .px(px(space::SM))
                .rounded(px(radius::MD))
                .when(on, |d| d.bg(Tokens::active()))
                .map(|d| if usable { d.cursor_pointer().hover(|s| s.bg(Tokens::hover())) } else { d.tooltip(tip("Magnific charges credits for this model, so MangaVox does not use it")) })
                .when(usable, |d| {
                    d.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.spec.model = id.clone();
                        this.menu = None;
                        this.emit_change(window, cx);
                    }))
                })
                .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).text_color(if usable { Tokens::text_1() } else { Tokens::text_3() }).child(name))
                .when_some(free, |d, f| d.child(if f { tag("Free", Tone::Success) } else { tag("Credits", Tone::Neutral) }))
                .when(on, |d| d.child(icon_ph(ph::CHECK, Tokens::accent(), icon_px::SM)))
        };
        match backends::models() {
            Models::Ready(list) if !list.is_empty() => {
                // The current model when it is not among the free ones (a priced one shows so).
                if !list.iter().any(|m| m.free && backends::same_model(&m.cy, current)) {
                    let free = list.iter().find(|m| backends::same_model(&m.cy, current)).map(|m| m.free);
                    pop = pop.child(row(0, current.to_string(), backends::model_name(current), free, cx));
                }
                // Only the models whose row showed the free marking, grouped by provider in the
                // site's order, each with its short note ("Free at 2K"). A pick keeps the site's full
                // name for the model (its own `data-cy` suffix).
                let mut groups: Vec<&str> = Vec::new();
                for m in list.iter().filter(|m| m.free) {
                    if !groups.contains(&m.group.as_str()) {
                        groups.push(m.group.as_str());
                    }
                }
                let mut k = 0;
                for g in groups {
                    if !g.trim().is_empty() {
                        pop = pop.child(div().px(px(space::SM)).pt(px(space::XS)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(g.to_string()));
                    }
                    for m in list.iter().filter(|m| m.free && m.group == g) {
                        k += 1;
                        let name = if m.name.trim().is_empty() { backends::model_name(&m.cy) } else { m.name.clone() };
                        pop = pop.child(row(k, m.cy.clone(), name, None, cx).child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(m.note.clone())));
                    }
                }
            }
            Models::Unavailable => {
                pop = pop
                    .child(row(0, current.to_string(), backends::model_name(current), None, cx))
                    .child(div().px(px(space::SM)).py(px(space::XS)).text_size(px(size::XS)).text_color(Tokens::text_3()).whitespace_normal().child("Magnific's other models show here once you are signed in."));
            }
            Models::Failed(e) => {
                pop = pop
                    .child(row(0, current.to_string(), backends::model_name(current), None, cx))
                    .child(div().px(px(space::SM)).py(px(space::XS)).text_size(px(size::XS)).text_color(Tokens::text_3()).whitespace_normal().child(e))
                    .child(div().px(px(space::SM)).pb(px(space::XS)).child(btn("ag-models-retry", "Try again", ButtonKind::Ghost).small().icon(ph::ARROW_CLOCKWISE).on_click(|_, _, _| backends::load_models())));
            }
            _ => {
                pop = pop.child(row(0, current.to_string(), backends::model_name(current), None, cx)).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .h(px(32.))
                        .px(px(space::SM))
                        .child(spinner("ag-models-spin", Tokens::text_3(), icon_px::SM))
                        .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("Reading Magnific's models…")),
                );
            }
        }
        div().absolute().top_full().left_0().child(deferred(anchored().anchor(Anchor::TopLeft).offset(point(px(0.), px(6.))).snap_to_window_with_margin(px(8.)).child(appear("ag-model-pop", pop))).with_priority(3))
    }

    fn references(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let (n, faces) = ref_counts(&self.spec.refs);
        let counter_color = if n > MAX_REFS || faces > MAX_FACES {
            Tokens::danger()
        } else if n >= MAX_REFS - 2 {
            Tokens::warning()
        } else {
            Tokens::text_3()
        };
        let counter = if faces > 0 { format!("{n} / {MAX_REFS} · faces {faces} / {MAX_FACES}") } else { format!("{n} / {MAX_REFS}") };
        let store = self.store();
        let mut grid = div().flex().flex_wrap().gap(px(space::SM));
        for (i, r) in self.spec.refs.iter().enumerate() {
            grid = grid.child(self.ref_tile(i, r, store.as_ref(), cx));
        }
        let full = n >= MAX_REFS;
        grid = grid.child(
            div().flex().flex_col().w(px(REF_TILE)).child(
                div()
                    .id("ag-ref-add")
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(space::XS))
                    .size(px(REF_TILE))
                    .rounded(px(radius::LG))
                    .border_1()
                    .border_dashed()
                    .border_color(Tokens::border_strong())
                    .bg(Tokens::surface_2())
                    .map(|d| if full || store.is_none() { d.opacity(0.5).tooltip(tip(if full { "Magnific takes 14 references at most" } else { "Open a project first" })) } else { d.cursor_pointer().hover(|s| s.bg(Tokens::hover())).tooltip(tip("Add your own picture (or drop files here)")).on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.pick_files(None, window, cx))) })
                    .child(if self.importing { spinner("ag-ref-busy", Tokens::text_2(), icon_px::SM).into_any_element() } else { icon_ph(ph::PLUS, Tokens::text_1(), icon_px::SM).into_any_element() })
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child("Add")),
            ),
        );
        div()
            .id("ag-refs")
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .rounded(px(radius::LG))
            .drag_over::<ExternalPaths>(|s, _, _, _| s.bg(Tokens::accent_soft()))
            .on_drop(cx.listener(|this, p: &ExternalPaths, window, cx| this.import(p.paths().to_vec(), None, window, cx)))
            .child(div().flex().items_center().gap(px(space::SM)).child(label_caps("References")).child(div().flex_1()).child(div().text_size(px(size::XS)).text_color(counter_color).child(counter)))
            .child(grid)
            .when(self.spec.refs.is_empty(), |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).whitespace_normal().child("The AI picks the references when it writes the prompt: the room, the faces, the style. Add your own with +.")))
            .when(faces > MAX_FACES, |d| d.child(info_line(ph::WARNING, LineTone::Warning, format!("More than {MAX_FACES} faces confuse the model: remove {}.", faces - MAX_FACES))))
    }

    fn ref_tile(&self, i: usize, r: &RefSlot, store: Option<&Store>, cx: &mut Context<Self>) -> Div {
        // Not looked at on disk here (render runs every frame): a missing file shows the broken icon.
        let path = store.filter(|_| !r.file.trim().is_empty()).map(|s| s.path(&r.file));
        let group = SharedString::from(format!("ag-ref-{i}"));
        let role = backends::role_label(&r.role);
        let job = if r.job.trim().is_empty() { role.to_string() } else { r.job.clone() };
        let pinned = r.pinned;
        let mini = |id: (&'static str, usize), icon: Ph, hint: SharedString| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(18.))
                .rounded_full()
                .bg(Tokens::surface_3())
                .border_1()
                .border_color(Tokens::border_strong())
                .cursor_pointer()
                .hover(|s| s.bg(Tokens::hover()))
                .tooltip(tip(hint))
                .child(icon_ph(icon, Tokens::text_1(), icon_px::XS - 2.))
        };
        let why = format!("Image {} · {role}: {job}", i + 1);
        // Each action finds this reference again by what it is when clicked.
        let key: RefKey = (r.hash.clone(), r.file.clone());
        let (swap_key, pin_key, x_key) = (key.clone(), key.clone(), key);
        let actions = div()
            .absolute()
            .bottom(px(3.))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .gap(px(2.))
            .invisible()
            .group_hover(group.clone(), |s| s.visible())
            .child(mini(("ag-ref-info", i), ph::INFO, why.clone().into()))
            .child(mini(("ag-ref-swap", i), ph::SWAP, "Swap for your own picture".into()).on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.pick_files(Some(swap_key.clone()), window, cx))))
            .child(mini(("ag-ref-pin", i), if pinned { ph::LOCK_SIMPLE_OPEN } else { ph::LOCK_SIMPLE }, (if pinned { "Let the AI replace it again" } else { "Keep it when the AI picks again" }).into()).on_click(cx.listener(move |this, _: &ClickEvent, w, cx| {
                this.edit_ref(
                    &pin_key,
                    |refs, at| {
                        if let Some(r) = refs.get_mut(at) {
                            r.pinned = !r.pinned;
                        }
                    },
                    w,
                    cx,
                )
            })))
            .child(mini(("ag-ref-x", i), ph::X, "Remove".into()).on_click(cx.listener(move |this, _: &ClickEvent, w, cx| {
                this.edit_ref(
                    &x_key,
                    |refs, at| {
                        if at < refs.len() {
                            refs.remove(at);
                        }
                    },
                    w,
                    cx,
                )
            })));
        let tile = div()
            .id(("ag-ref", i))
            .relative()
            .group(group)
            .size(px(REF_TILE))
            .rounded(px(radius::LG))
            .overflow_hidden()
            .border_1()
            .border_color(if pinned { Tokens::accent_border() } else { Tokens::border() })
            .bg(Tokens::surface_2())
            .tooltip(tip(why))
            .map(|d| match path {
                Some(p) => d.flex().items_center().justify_center().child(img(mv_ui::thumbs::cover_source(p)).size_full().object_fit(ObjectFit::Cover).with_fallback(|| icon_ph(ph::IMAGE_BROKEN, Tokens::text_3(), icon_px::MD).into_any_element())),
                None => d.flex().items_center().justify_center().child(icon_ph(ph::IMAGE_BROKEN, Tokens::text_3(), icon_px::MD)),
            })
            .when(pinned, |d| d.child(div().absolute().top(px(3.)).left(px(3.)).flex().items_center().justify_center().size(px(16.)).rounded_full().bg(Tokens::accent()).child(icon_ph(ph::LOCK_SIMPLE, Tokens::on_accent(), icon_px::XS - 2.))))
            .child(actions);
        // The caption is the reference's job ("identity", "room + camera", "style"); who or what
        // it shows is in the tooltip and behind the info mark.
        div().flex().flex_col().gap(px(3.)).w(px(REF_TILE)).child(tile).child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_2()).child(ref_caption(&r.role)))
    }

    fn prompt_section(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let pinned = self.spec.prompt_user.is_some();
        let Some(state) = self.prompt.clone() else { return div() };
        let focused = state.read(cx).focus_handle(cx).contains_focused(window, cx);
        let chars = state.read(cx).value().chars().count();
        let head = div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .child(label_caps("Prompt"))
            .when(pinned, |d| d.child(tag("Edited by you", Tone::Accent)))
            .child(div().flex_1())
            .when(pinned, |d| d.child(btn("ag-reset", "Reset", ButtonKind::Ghost).small().icon(ph::ARROW_COUNTER_CLOCKWISE).tooltip("Reset to the AI's suggestion").on_click(cx.listener(|this, _: &ClickEvent, w, cx| this.reset(w, cx)))));
        let frame = div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .p(px(space::MD))
            .rounded(px(radius::LG))
            .border_1()
            .border_color(if focused { Tokens::text_1() } else if pinned { Tokens::accent_border() } else { Tokens::border() })
            .bg(Tokens::surface())
            // The app's body size; long prompts scroll inside the box.
            .text_size(px(size::SM))
            .child(Textarea::new(&state).appearance(false).bordered(false))
            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{chars} characters")));
        let sections = self.compiled.as_ref().filter(|c| !c.sections.is_empty() && !pinned).map(|c| {
            disclosure("ag-sections", "What the model receives").content(div().flex().flex_col().gap(px(space::SM)).children(c.sections.iter().map(|(name, text)| {
                div().flex().flex_col().gap(px(2.)).child(label_caps(name)).child(div().text_size(px(size::XS)).line_height(px(16.)).text_color(Tokens::text_2()).whitespace_normal().child(text.clone()))
            })))
        });
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .child(head)
            .child(frame)
            .when(pinned, |d| d.child(info_line(ph::LOCK_SIMPLE, LineTone::Neutral, "Pinned: changes in this step no longer rewrite this prompt. Reset to follow the AI again.")))
            .when(suggestion_moved(&self.spec), |d| d.child(info_line(ph::ARROWS_CLOCKWISE, LineTone::Warning, "The AI's suggestion changed after you edited this prompt.")))
            .children(sections)
    }

    /// This picture's takes from Magnific, each with its Magnific id (to match with the
    /// Creations list on magnific.com), and the link to the list of every picture MangaVox made.
    fn made_section(&self, cx: &mut Context<Self>) -> Div {
        let link = div()
            .id("ag-made-list")
            .text_size(px(size::XS))
            .text_color(Tokens::accent())
            .cursor_pointer()
            .hover(|s| s.underline())
            .tooltip(tip("Opens the folder of the list of every Magnific picture MangaVox made"))
            .child("Show the list of Magnific pictures")
            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                let file = mv_anime::store::magnific_list_path();
                if file.is_file() {
                    cx.reveal_path(&file);
                } else if let Some(dir) = file.parent() {
                    let _ = std::fs::create_dir_all(dir);
                    cx.reveal_path(dir);
                }
            }));
        let rows = self.made.iter().take(8).enumerate().map(|(i, m)| {
            div()
                .id(("ag-made", i))
                .text_size(px(size::XS))
                .text_color(Tokens::text_3())
                .tooltip(tip(made_tip(m.pressed_by_app)))
                .child(made_label(&m.site_id))
        });
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .when(!self.made.is_empty(), |d| d.child(label_caps("On Magnific")).children(rows))
            .child(link)
    }

    fn options(&self, cx: &mut Context<Self>) -> Div {
        let aspect = if self.spec.aspect.is_empty() { "16:9".to_string() } else { self.spec.aspect.clone() };
        let variants = self.spec.variants.clamp(1, MAX_VARIANTS);
        let aspect_pill = div()
            .relative()
            .child(
                pill("ag-aspect")
                    .map(|d| if self.lock_aspect { d.cursor_default().tooltip(tip("This picture always has this shape")) } else { d.tooltip(tip("Shape of the picture")).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.set_menu(Menu::Aspect, cx))) })
                    .child(icon_ph(ph::CROP, Tokens::text_2(), icon_px::XS))
                    .child(aspect.clone()),
            )
            .when(self.menu == Some(Menu::Aspect), |d| {
                let items: Vec<(String, String)> = ASPECTS.iter().map(|a| (a.to_string(), a.to_string())).collect();
                d.child(self.small_menu("ag-aspect-pop", items, &aspect, cx, |this, v| this.spec.aspect = v))
            });
        let variants_pill = div()
            .relative()
            .child(
                pill("ag-variants")
                    .tooltip(tip("Pictures per Generate; each is drawn on its own"))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.set_menu(Menu::Variants, cx)))
                    .child(icon_ph(ph::IMAGES, Tokens::text_2(), icon_px::XS))
                    .child(format!("{variants} picture{}", crate::widgets::plural(variants as usize))),
            )
            .when(self.menu == Some(Menu::Variants), |d| {
                let items: Vec<(String, String)> = (1..=MAX_VARIANTS).map(|n| (n.to_string(), format!("{n} picture{}", crate::widgets::plural(n as usize)))).collect();
                d.child(self.small_menu("ag-variants-pop", items, &variants.to_string(), cx, |this, v| this.spec.variants = v.parse().unwrap_or(1)))
            });
        let res = pill("ag-res").cursor_default().tooltip(tip("Magnific's credit-free size")).child(icon_ph(ph::FRAME_CORNERS, Tokens::text_2(), icon_px::XS)).child(if self.spec.res.is_empty() { "1K".to_string() } else { self.spec.res.clone() });
        let free = pill("ag-free").cursor_default().bg(Tokens::info_soft()).text_color(Tokens::info()).tooltip(tip("MangaVox only presses Generate when Magnific shows the job is credit-free")).child(icon_ph(ph::INFINITY, Tokens::info(), icon_px::XS)).child("Free");
        div().flex().flex_wrap().items_center().gap(px(space::XS)).child(aspect_pill).child(res).child(variants_pill).child(free)
    }

    /// A small option list under a pill; `items` are (value, label).
    fn small_menu(&self, id: &'static str, items: Vec<(String, String)>, current: &str, cx: &mut Context<Self>, set: fn(&mut GenPanel, String)) -> impl IntoElement {
        let mut pop = popup_surface().w(px(160.)).gap(px(2.)).on_mouse_down_out(cx.listener(|this, _, _, cx| {
            this.menu = None;
            cx.notify();
        }));
        for (k, (v, label)) in items.into_iter().enumerate() {
            let on = v == current;
            pop = pop.child(
                div()
                    .id((id, k))
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .h(px(30.))
                    .px(px(space::SM))
                    .rounded(px(radius::MD))
                    .cursor_pointer()
                    .when(on, |d| d.bg(Tokens::active()))
                    .hover(|s| s.bg(Tokens::hover()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        set(this, v.clone());
                        this.menu = None;
                        this.emit_change(window, cx);
                    }))
                    .child(div().flex_1().text_size(px(size::SM)).text_color(Tokens::text_1()).child(label))
                    .when(on, |d| d.child(icon_ph(ph::CHECK, Tokens::accent(), icon_px::SM))),
            );
        }
        div().absolute().top_full().left_0().child(deferred(anchored().anchor(Anchor::TopLeft).offset(point(px(0.), px(6.))).snap_to_window_with_margin(px(8.)).child(appear(id, pop))).with_priority(3))
    }

    fn generate_block(&self, busy: &Busy, cx: &mut Context<Self>) -> Div {
        let site = backends::site();
        let variants = self.spec.variants.clamp(1, MAX_VARIANTS);
        let (n, faces) = ref_counts(&self.spec.refs);
        // Why Generate cannot run when nothing on the site or in the queue stops it.
        let blocked: Option<&'static str> = if self.project.is_none() {
            Some("Open a project first")
        } else if n > MAX_REFS {
            Some("Magnific takes 14 references at most")
        } else if faces > MAX_FACES {
            Some("Use at most 4 faces")
        } else if shown_text(&self.spec, self.compiled.as_ref()).trim().is_empty() {
            Some("Write a prompt first")
        } else {
            None
        };
        let now = now_secs();
        // One main action in every state: it is always this button, in the same place.
        let main = main_action(&site, self.web_held, busy, blocked, now);
        let (label, icon): (String, Ph) = match &main {
            Main::Generate(_) => (format!("Generate · {variants} picture{}", crate::widgets::plural(variants as usize)), ph::SPARKLE),
            Main::SignIn => ("Sign in to Magnific".into(), ph::SIGN_IN),
            Main::OpenAgain => ("Open it again".into(), ph::APP_WINDOW),
            Main::Continue => ("Continue".into(), ph::ARROW_RIGHT),
            Main::CheckAgain(l) => (l.to_string(), ph::ARROW_CLOCKWISE),
            Main::Retry(_) => ("Try again".into(), ph::ARROW_CLOCKWISE),
            Main::Status(s) => (
                s.clone(),
                match busy {
                    Busy::Waiting { .. } => ph::HOURGLASS,
                    Busy::Working { .. } => ph::SPARKLE,
                    _ => ph::CIRCLE_NOTCH,
                },
            ),
        };
        let disabled: Option<&'static str> = match &main {
            Main::Generate(why) => *why,
            Main::Status(_) if matches!(busy, Busy::Working { .. }) => Some("This picture is being drawn"),
            Main::Status(_) if matches!(busy, Busy::Waiting { .. }) => Some("This picture is already waiting in the queue"),
            Main::Status(_) => Some("Checking Magnific…"),
            _ => None,
        };
        let action = main.clone();
        let button = div()
            .id("ag-generate")
            .flex()
            .items_center()
            .gap(px(space::SM))
            .h(px(40.))
            .px(px(space::MD))
            .rounded(px(radius::LG))
            .bg(Tokens::accent())
            .map(|d| match disabled {
                None => d.cursor_pointer().hover(|s| s.bg(Tokens::accent_hover())).active(|s| s.bg(Tokens::accent_pressed())).on_click(cx.listener(move |this, _: &ClickEvent, window, cx| match &action {
                    Main::SignIn | Main::OpenAgain => backends::connect(),
                    Main::Continue | Main::CheckAgain(_) => backends::resume_pictures(cx),
                    Main::Retry(id) => {
                        backends::ensure_runner(cx);
                        queue::retry(*id);
                    }
                    Main::Generate(_) => this.generate(window, cx),
                    Main::Status(_) => {}
                })),
                Some(why) => d.opacity(0.55).tooltip(tip(why)),
            })
            .when(matches!(busy, Busy::Working { .. }), |d| d.child(spinner("ag-gen-spin", Tokens::on_accent(), icon_px::SM)))
            .child(div().flex_1().truncate().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::on_accent()).child(label))
            .child(icon_ph(icon, Tokens::on_accent(), icon_px::SM));
        let kind = if matches!(self.target, Target::ShotFrame { .. }) { "frame" } else { "sheet" };
        let free_line = match Eta::current().mean(kind) {
            Some(m) => format!("Credit-free on Magnific · about {} each", crate::widgets::fmt_duration(m.round().max(1.) as u64)),
            None => "Credit-free on Magnific · MangaVox never presses Generate on a paid picture".to_string(),
        };
        // Messages sit above the chips and the button, so the button never moves.
        let mut block = div().flex().flex_col().gap(px(space::XS));
        match busy {
            Busy::Working { id, progress, line, .. } => {
                let id = *id;
                block = block.child(div().pt(px(2.)).child(progress_bar("ag-progress", *progress, !self.web_held))).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(size::XS)).text_color(Tokens::text_2()).child(if line.trim().is_empty() { "Drawing on Magnific".to_string() } else { line.clone() }))
                        .child(btn("ag-cancel", "Cancel", ButtonKind::Ghost).small().on_click(move |_, _, _| queue::cancel(id))),
                );
            }
            Busy::Waiting { id, .. } => {
                let id = *id;
                block = block.child(div().flex().justify_end().child(btn("ag-cancel", "Cancel", ButtonKind::Ghost).small().on_click(move |_, _, _| queue::cancel(id))));
            }
            _ => {}
        }
        // What stops pictures right now, in one plain sentence (the page's own control names stay
        // in the log); its action is the main button below.
        if let Some((tone, text)) = site_line(&site, self.web_held) {
            let icon = match &site {
                Site::NoWindow => ph::APP_WINDOW,
                Site::SignedOut | Site::SigningIn => ph::SIGN_IN,
                _ => ph::WARNING,
            };
            block = block.child(info_line(icon, tone, text));
        }
        match busy {
            // The failure in its own words; "Try again" is the main button.
            Busy::Failed { error, .. } => {
                block = block.child(error_block(error.clone()));
            }
            Busy::Done { at } => {
                block = block.child(info_line(ph::CHECK_CIRCLE, LineTone::Success, match at {
                    Some(t) => format!("Made {}. Earlier versions are kept.", ago(*t, now)),
                    None => "Made. Earlier versions are kept.".into(),
                }));
                // The model asked for was not in Magnific's list: say which one drew it.
                if let Some(drew) = backends::fallback_model(&self.spec.model) {
                    block = block.child(info_line(ph::INFO, LineTone::Warning, format!("Drawn with {drew}: Magnific's list did not show {}.", backends::model_name(&self.spec.model))));
                }
            }
            _ => {}
        }
        block.child(info_line(ph::INFINITY, LineTone::Info, free_line)).child(self.options(cx)).child(button)
    }

    /// The fixed footer under the scrolling settings: messages, the size chips and the button,
    /// with a hairline above.
    fn footer(&self, busy: &Busy, cx: &mut Context<Self>) -> Div {
        div().flex_none().flex().flex_col().p(px(space::MD)).border_t_1().border_color(Tokens::border()).bg(Tokens::sidebar()).child(self.generate_block(busy, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::{ai_text, apply_prompt, busy_of, default_label, job_for, job_matches, made_label, made_tip, ref_counts, reset_prompt, shown_text, suggestion_moved, Busy};
    use mv_anime::queue::{Job, JobKind, JobState, Lane};
    use mv_anime::{Compiled, GenSpec, RefRole, RefSlot, Target};

    fn spec(ai: &str) -> GenSpec {
        GenSpec { prompt_ai: ai.into(), ..Default::default() }
    }

    #[test]
    fn a_take_shows_its_magnific_id_and_who_pressed_generate() {
        assert_eq!(made_label(" 123456 "), "Magnific #123456");
        assert_eq!(made_tip(true), "Made by MangaVox on Magnific");
        assert_eq!(made_tip(false), "Generate pressed by you at the window");
    }

    #[test]
    fn editing_pins_and_reset_unpins() {
        let mut s = spec("a girl on a roof");
        assert_eq!(shown_text(&s, None), "a girl on a roof");
        assert!(!apply_prompt(&mut s, "a girl on a roof", None));
        assert!(apply_prompt(&mut s, "a girl on a roof at night", None));
        assert_eq!(s.prompt_user.as_deref(), Some("a girl on a roof at night"));
        let base = s.prompt_user_base.clone();
        assert!(base.is_some());
        // Typing more keeps the base of the first override.
        apply_prompt(&mut s, "a girl on a roof at night, rain", None);
        assert_eq!(s.prompt_user_base, base);
        assert!(!suggestion_moved(&s));
        s.prompt_ai = "a girl on a rooftop".into();
        assert!(suggestion_moved(&s));
        reset_prompt(&mut s);
        assert_eq!((s.prompt_user.clone(), s.prompt_user_base.clone()), (None, None));
        assert_eq!(shown_text(&s, None), "a girl on a rooftop");
    }

    #[test]
    fn typing_the_ai_text_back_clears_the_pin() {
        let mut s = spec("x");
        apply_prompt(&mut s, "y", None);
        assert!(apply_prompt(&mut s, "x", None));
        assert!(s.prompt_user.is_none());
    }

    #[test]
    fn compiled_text_wins_over_prompt_ai() {
        let s = spec("old");
        let c = Compiled { text: "compiled".into(), sections: vec![] };
        assert_eq!(ai_text(&s, Some(&c)), "compiled");
        let empty = Compiled::default();
        assert_eq!(ai_text(&s, Some(&empty)), "old");
    }

    #[test]
    fn jobs_for_targets() {
        let frame = Target::ShotFrame { episode: "ep01".into(), shot: "sh0004".into() };
        let k = job_for(&frame, 9).unwrap_or(JobKind::Unknown);
        assert_eq!(k, JobKind::Frame { episode: "ep01".into(), shot: "sh0004".into(), variants: 4 });
        assert!(job_matches(&k, &frame));
        assert!(!job_matches(&k, &Target::ShotFrame { episode: "ep01".into(), shot: "sh0005".into() }));
        let sheet = Target::StyleFrame;
        let k = job_for(&sheet, 0).unwrap_or(JobKind::Unknown);
        assert_eq!(k, JobKind::Sheet { target: Target::StyleFrame, variants: 1 });
        assert!(job_matches(&k, &sheet));
        assert!(job_for(&Target::Upload, 1).is_none());
        assert_eq!(default_label(&frame), "Frame sh0004");
    }

    #[test]
    fn counts_faces() {
        let r = |role: RefRole| RefSlot { role, ..Default::default() };
        assert_eq!(ref_counts(&[r(RefRole::Plate), r(RefRole::Identity("a".into())), r(RefRole::Identity("b".into()))]), (3, 2));
    }

    #[test]
    fn exactly_one_main_action_in_every_state() {
        use super::{main_action, site_line, Main};
        use crate::backends::Site;
        let idle = Busy::Idle;
        let m = |site: Site, held: bool, busy: &Busy| main_action(&site, held, busy, None, 0);
        let line = |site: Site, held: bool| site_line(&site, held).map(|(_, t)| t);
        // Signed out: sign in.
        assert_eq!(m(Site::SignedOut, false, &idle), Main::SignIn);
        // The window was closed (also while the user was signing in): one sentence, open it again.
        assert_eq!(m(Site::NoWindow, false, &idle), Main::OpenAgain);
        assert_eq!(m(Site::NoWindow, true, &idle), Main::OpenAgain);
        assert_eq!(line(Site::NoWindow, false), Some("The Magnific window was closed."));
        // Signing in or a check for a person: Continue.
        assert_eq!(m(Site::SigningIn, false, &idle), Main::Continue);
        assert_eq!(m(Site::NeedsYou, true, &idle), Main::Continue);
        // Checking: nothing to press.
        assert_eq!(m(Site::Checking, false, &idle), Main::Status("Checking Magnific…".into()));
        // Would cost credits, a warning: check again. A changed page or a failed check: try again.
        assert_eq!(m(Site::NotFree("x".into()), false, &idle), Main::CheckAgain("Check again"));
        assert_eq!(m(Site::Warning, true, &idle), Main::CheckAgain("Check again"));
        assert_eq!(m(Site::Changed(vec![]), false, &idle), Main::CheckAgain("Try again"));
        assert_eq!(m(Site::Failed("x".into()), false, &idle), Main::CheckAgain("Try again"));
        // Held for another reason: Continue, with its sentence.
        assert_eq!(m(Site::Ready, true, &idle), Main::Continue);
        assert!(line(Site::Ready, true).is_some_and(|t| t.contains("Continue")));
        // Working and waiting: the button says what happens (Cancel sits above it).
        assert_eq!(m(Site::Ready, false, &Busy::Working { id: 1, progress: 0.5, line: String::new(), started: None }), Main::Status("Drawing…".into()));
        assert_eq!(m(Site::Ready, false, &Busy::Waiting { id: 1, ahead: 2 }), Main::Status("Waiting · 2 ahead".into()));
        // Failed: try again. Ready: Generate, disabled with the reason when something is missing.
        assert_eq!(m(Site::Ready, false, &Busy::Failed { id: 7, error: "x".into() }), Main::Retry(7));
        assert_eq!(m(Site::Ready, false, &idle), Main::Generate(None));
        assert_eq!(main_action(&Site::Ready, false, &idle, Some("Write a prompt first"), 0), Main::Generate(Some("Write a prompt first")));
        assert_eq!(line(Site::Ready, false), None);
        // Signed out wins over an old failure: what stops pictures comes first.
        assert_eq!(m(Site::SignedOut, false, &Busy::Failed { id: 7, error: "x".into() }), Main::SignIn);
    }

    #[test]
    fn busy_follows_the_latest_job() {
        let t = Target::ShotFrame { episode: "ep01".into(), shot: "s1".into() };
        let mk = |id: u64, state: JobState| Job { id, project: "p".into(), lane: Lane::Web, state, kind: JobKind::Frame { episode: "ep01".into(), shot: "s1".into(), variants: 1 }, ..Default::default() };
        let other = Job { id: 1, project: "p".into(), lane: Lane::Web, state: JobState::Running, kind: JobKind::Frame { episode: "ep01".into(), shot: "s2".into(), variants: 1 }, ..Default::default() };
        assert_eq!(busy_of(&[], "p", &t), Busy::Idle);
        assert_eq!(busy_of(&[other.clone(), mk(2, JobState::Queued)], "p", &t), Busy::Waiting { id: 2, ahead: 1 });
        assert!(matches!(busy_of(&[mk(2, JobState::Done), mk(3, JobState::Failed)], "p", &t), Busy::Failed { id: 3, .. }));
        assert!(matches!(busy_of(&[mk(3, JobState::Done)], "p", &t), Busy::Done { .. }));
        assert_eq!(busy_of(&[mk(3, JobState::Done)], "other", &t), Busy::Idle);
    }
}
