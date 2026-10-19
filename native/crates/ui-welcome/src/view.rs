//! The setup screens. Every screen has one decision, Back, "Skip this step" and Continue, and
//! the header always offers a way out ("Skip setup"). Choices are saved as they are confirmed,
//! so a restart resumes at the same screen.
//!
//! References: Apple HIG onboarding (short, skippable, ask in context, let people keep going
//! while content downloads) <https://developer.apple.com/design/human-interface-guidelines/onboarding>;
//! Mihon getting started (ships with no content; add a repository, then install extensions)
//! <https://mihon.app/docs/guides/getting-started>.
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::onboarding::{self, StepResult};
use mv_sources::SourceInfo;
use mv_studio::voice_setup::{self, SetupState, Target};
use mv_studio::voices::{self, VoicePack};
use mv_studio::StudioError;
use mv_ui::theme::{icon as icon_px, radius, ramp};
use mv_ui::toast::{self, Toast};
use mv_ui::{
    appear, btn, checkbox, dot_trail, fmt_bytes, glyphs, icon, icon_button, icon_ph, inline_alert, meter, muted, path_chip, ph, progress_bar, size, status_tag, switch, tag, AlertKind,
    ButtonKind, Hue, RampExt as _, Route, StatusKind, Tokens, Tone,
};
use ui_manga::backend::{self, PluginLinkBox, RepoBrowser, SourcesEvent};

use crate::cards::{card_text, fact, heading, icon_tile, option_card, summary_row};
use crate::plan::{self, Step, Use};

/// Width of the setup column (the window can be as narrow as 720px).
const COLUMN_W: f32 = 680.;

/// The Movies and Anime catalogs are warmed up once per run; `main` skips it while the
/// setup is on screen, so finishing with "watch" chosen starts it here.
static MEDIA_WARMED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq)]
enum MangaPanel {
    Builtin,
    Repository,
    Plugin,
}

pub struct WelcomeView {
    step: Step,
    uses: Vec<Use>,
    /// The setup finished before (Settings › About "Run welcome setup again", or forced on a
    /// folder with data): nothing is reset, and leaving early changes nothing.
    rerun: bool,
    root: PathBuf,
    focus: FocusHandle,
    results: BTreeMap<String, StepResult>,
    // Manga
    builtin: Vec<SourceInfo>,
    enabled: HashSet<String>,
    adult_ok: bool,
    panel: MangaPanel,
    repos: Entity<RepoBrowser>,
    plugin_box: Entity<PluginLinkBox>,
    // Streaming addons
    addon_input: Entity<InputState>,
    addon_busy: bool,
    addon_result: Option<Result<String, String>>,
    addons: Vec<(String, String)>,
    // Voices
    voice: &'static str,
    recommended: &'static str,
    voice_started: Option<&'static str>,
    polling: bool,
    free_bytes: Option<u64>,
    ram_bytes: Option<u64>,
    _subs: Vec<Subscription>,
}

/// Free space on the drive holding `path` (the longest matching mount point).
fn free_space(path: &Path) -> Option<u64> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks.list().iter().filter(|d| path.starts_with(d.mount_point())).max_by_key(|d| d.mount_point().as_os_str().len()).map(|d| d.available_space())
}

fn total_memory() -> Option<u64> {
    let mut s = sysinfo::System::new();
    s.refresh_memory();
    Some(s.total_memory()).filter(|m| *m > 0)
}

fn installed_addons() -> Vec<(String, String)> {
    ui_media::addon_registry().list().into_iter().map(|a| (a.manifest.name, a.manifest.version)).collect()
}

fn names(list: &[&SourceInfo]) -> String {
    let n: Vec<&str> = list.iter().map(|s| s.name.as_str()).collect();
    match n.len() {
        0 => String::new(),
        1 => n[0].to_string(),
        _ => format!("{} and {}", n[..n.len() - 1].join(", "), n[n.len() - 1]),
    }
}

impl WelcomeView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let root = mv_core::paths::root();
        let state = onboarding::load(&root);
        let rerun = state.as_ref().is_some_and(|s| s.finished());
        // Resumed or repeated: what was picked last time stays picked.
        let uses: Vec<Use> = state.as_ref().map(|s| s.uses.iter().filter_map(|u| Use::from_id(u)).collect()).unwrap_or_default();
        let step = match &state {
            Some(s) if !rerun => plan::resume_at(&s.step, &uses),
            _ => Step::Welcome,
        };
        let seeded = state.as_ref().is_some_and(|s| s.seeded);
        let builtin = backend::builtin_sources();
        // A fresh setup starts with nothing on; a resumed or repeated one shows what is on now.
        let enabled: HashSet<String> = if rerun || seeded {
            let off = backend::disabled_sources();
            builtin.iter().filter(|s| !off.contains(&s.id)).map(|s| s.id.clone()).collect()
        } else {
            HashSet::new()
        };
        let adult_ok = builtin.iter().any(|s| s.nsfw && enabled.contains(&s.id));
        let repos = cx.new(|cx| RepoBrowser::new(window, cx));
        let plugin_box = cx.new(|cx| PluginLinkBox::new(window, cx));
        let addon_input = cx.new(|cx| InputState::new(window, cx).placeholder("Addon link (manifest.json or stremio://)"));
        let subs = vec![
            cx.subscribe(&repos, |_, _, _: &SourcesEvent, cx| cx.notify()),
            cx.subscribe(&plugin_box, |_, _, _: &SourcesEvent, cx| cx.notify()),
            cx.subscribe_in(&addon_input, window, |this, _, ev: &InputEvent, _, cx| match ev {
                InputEvent::PressEnter { .. } => this.install_addon(cx),
                InputEvent::Change => {
                    this.addon_result = None;
                    cx.notify();
                }
                _ => {}
            }),
        ];
        let free_bytes = free_space(&root);
        let ram_bytes = total_memory();
        let recommended = plan::recommended_voice(ram_bytes, free_bytes);
        // A voice setup already running (started before a restart, or from Settings) shows here.
        let running = plan::VOICE_CHOICES.into_iter().find(|id| matches!(voice_setup::state(id), Some(SetupState::Running(_))));
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut v = Self {
            step,
            uses,
            rerun,
            root,
            focus,
            results: state.map(|s| s.steps).unwrap_or_default(),
            builtin,
            enabled,
            adult_ok,
            panel: MangaPanel::Builtin,
            repos,
            plugin_box,
            addon_input,
            addon_busy: false,
            addon_result: None,
            addons: installed_addons(),
            voice: running.unwrap_or(recommended),
            recommended,
            voice_started: running,
            polling: false,
            free_bytes,
            ram_bytes,
            _subs: subs,
        };
        if running.is_some() {
            v.poll(cx);
        }
        v
    }

    fn has(&self, u: Use) -> bool {
        self.uses.contains(&u)
    }

    fn uses_ids(&self) -> Vec<String> {
        self.uses.iter().map(|u| u.id().to_string()).collect()
    }

    fn save(&self, f: impl FnOnce(&mut onboarding::State)) {
        if self.rerun {
            return;
        }
        if let Err(e) = onboarding::update(&self.root, f) {
            eprintln!("[welcome] could not save the setup progress: {e:#}");
        }
    }

    // ── Moving between screens ─────────────────────────────────────────────────────────────

    fn go_to(&mut self, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        self.step = step;
        let (id, uses) = (step.id().to_string(), self.uses_ids());
        self.save(|s| {
            s.step = id;
            s.uses = uses;
        });
        // Enter and Esc work again on the new screen.
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn record(&mut self, step: Step, r: StepResult) {
        self.results.insert(step.id().to_string(), r);
        let (id, r2) = (step.id().to_string(), r);
        self.save(|s| {
            s.steps.insert(id, r2);
        });
    }

    /// "Set up MangaVox": the setup starts and, on a fresh install, every built-in source is
    /// switched off, so quitting from here on still leaves a clean app.
    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.rerun {
            match onboarding::begin(&self.root) {
                Ok(s) if !s.seeded => {
                    if let Err(e) = backend::seed_clean_start() {
                        toast::show(cx, Toast::error(format!("Could not save the source settings: {e:#}")));
                    }
                    self.save(|s| s.seeded = true);
                }
                Ok(_) => {}
                Err(e) => eprintln!("[welcome] could not start the setup state: {e:#}"),
            }
        }
        self.go_to(Step::Uses, window, cx);
    }

    /// "Start empty" (or "Skip setup" / "Close setup" later): nothing more is set up.
    fn leave_empty(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.rerun {
            mv_ui::navigate(Route::Library, window, cx);
            return;
        }
        if self.step == Step::Welcome {
            // Identical to finishing with nothing chosen: no source, no addon, Studio off.
            let fresh = onboarding::load(&self.root).is_none_or(|s| !s.seeded);
            if fresh {
                if let Err(e) = backend::seed_clean_start() {
                    eprintln!("[welcome] could not switch the built-in sources off: {e:#}");
                }
            }
            let _ = mv_core::config::Config::open_default().set("app.mode", serde_json::json!("entertainment"));
            mv_ui::mode::set_studio(cx, false);
            if let Err(e) = onboarding::skip(&self.root) {
                eprintln!("[welcome] could not save the skipped setup: {e:#}");
            }
            mv_ui::navigate(Route::Library, window, cx);
        } else {
            // Part-way: keep what was already set up and show the summary.
            for s in plan::flow(&self.uses) {
                if !matches!(s, Step::Welcome | Step::Uses | Step::Done) && !self.results.contains_key(s.id()) {
                    self.record(s, StepResult::Skipped);
                }
            }
            self.go_to(Step::Done, window, cx);
        }
    }

    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let p = plan::prev(self.step, &self.uses);
        self.go_to(p, window, cx);
    }

    /// Continue: applies this screen's choice, then moves on.
    fn continue_(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.step {
            Step::Welcome => return self.start(window, cx),
            Step::Done => return self.finish(plan::landing(&self.uses), window, cx),
            Step::Uses => {
                let studio = self.has(Use::Studio);
                if studio || !self.rerun {
                    let _ = mv_core::config::Config::open_default().set("app.mode", serde_json::json!(if studio { "studio" } else { "entertainment" }));
                    mv_ui::mode::set_studio(cx, studio);
                }
                self.record(Step::Uses, if self.uses.is_empty() { StepResult::Skipped } else { StepResult::Configured });
            }
            Step::Manga => {
                if let Err(e) = backend::set_builtins_enabled(&self.enabled) {
                    toast::show(cx, Toast::error(format!("Could not save the source settings: {e:#}")));
                }
                backend::bump_data();
                let r = if self.manga_configured(cx) { StepResult::Configured } else { StepResult::Skipped };
                self.record(Step::Manga, r);
            }
            Step::Watch => {
                let r = if self.addons.is_empty() { StepResult::Skipped } else { StepResult::Configured };
                self.record(Step::Watch, r);
            }
            Step::Voices => {
                let r = match self.voice_started {
                    Some(id) if matches!(voice_setup::state(id), Some(SetupState::Running(_))) => StepResult::Later,
                    Some(id) if voice_setup::state(id).is_none() && plan::voice_pack(id).is_some_and(voices::installed) => StepResult::Configured,
                    Some(_) => StepResult::Later,
                    None => StepResult::Skipped,
                };
                self.record(Step::Voices, r);
            }
        }
        let n = plan::next(self.step, &self.uses);
        self.go_to(n, window, cx);
    }

    /// "Skip this step": nothing on this screen is applied.
    fn skip_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.step {
            Step::Welcome => return self.leave_empty(window, cx),
            Step::Done => return self.finish(plan::landing(&self.uses), window, cx),
            Step::Uses => {
                self.uses.clear();
                self.record(Step::Uses, StepResult::Skipped);
            }
            s => self.record(s, StepResult::Skipped),
        }
        let n = plan::next(self.step, &self.uses);
        self.go_to(n, window, cx);
    }

    /// Records the setup as done and opens `to`.
    fn finish(&mut self, to: Route, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(e) = onboarding::complete(&self.root, &self.uses_ids(), self.results.clone()) {
            eprintln!("[welcome] could not save the finished setup: {e:#}");
        }
        if self.has(Use::Watch) && crate::shows_at_start() && !MEDIA_WARMED.swap(true, Ordering::Relaxed) {
            ui_media::warm_up();
        }
        mv_ui::navigate(to, window, cx);
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Only when no field or control has the keyboard: Enter in a link box installs.
        if !self.focus.is_focused(window) {
            return;
        }
        match ev.keystroke.key.as_str() {
            "enter" => self.continue_(window, cx),
            "escape" if !matches!(self.step, Step::Welcome | Step::Done) => self.skip_step(window, cx),
            _ => {}
        }
    }

    // ── Manga ──────────────────────────────────────────────────────────────────────────────

    fn manga_configured(&self, cx: &App) -> bool {
        !self.enabled.is_empty() || self.repos.read(cx).installed_count() > 0 || self.plugin_box.read(cx).installed_count() > 0
    }

    fn toggle_source(&mut self, id: String, on: bool, cx: &mut Context<Self>) {
        if on {
            self.enabled.insert(id);
        } else {
            self.enabled.remove(&id);
        }
        cx.notify();
    }

    // ── Addons ─────────────────────────────────────────────────────────────────────────────

    fn paste_addon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
            self.addon_input.update(cx, |s, cx| s.set_value(t, window, cx));
        }
    }

    fn install_addon(&mut self, cx: &mut Context<Self>) {
        let raw = self.addon_input.read(cx).value().trim().to_string();
        if raw.is_empty() || self.addon_busy {
            return;
        }
        // stremio:// links are plain https manifests.
        let url = raw.replacen("stremio://", "https://", 1);
        self.addon_busy = true;
        self.addon_result = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move { ui_media::addon_registry().install(&url).await }).await.and_then(|r| Ok(r?));
            let _ = this.update(cx, |this, cx| {
                this.addon_busy = false;
                match res {
                    Ok(a) => {
                        this.addon_result = Some(Ok(format!("Added {} {}", a.manifest.name, a.manifest.version)));
                        this.addons = installed_addons();
                    }
                    Err(e) => this.addon_result = Some(Err(format!("Could not add it: {e:#}"))),
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ── Voices ─────────────────────────────────────────────────────────────────────────────

    /// Starts the one-click voice setup for `pack` (runtime, engine, model, check). It runs on
    /// the background runtime, so it keeps going after the setup closes; Settings › Audio
    /// shows the same job. A toast reports the end wherever the user is.
    fn start_voice(&mut self, pack: &'static VoicePack, cx: &mut Context<Self>) {
        self.voice_started = Some(pack.id);
        voice_setup::clear_error(pack.id);
        let name = pack.name;
        let default = pack.voices.first().copied();
        cx.spawn(async move |_, cx| {
            let res = mv_ui::bg::run(voice_setup::run(Target::Pack(pack))).await;
            cx.update(|cx| match res {
                Ok(Ok(())) => {
                    // Piper and Chatterbox packs bring their own voices; use the first one.
                    if let Some(v) = default {
                        let _ = voices::set_default_voice(v);
                    }
                    toast::show(cx, Toast::success(format!("{name} is ready to narrate")));
                }
                Ok(Err(StudioError::Cancelled)) => {}
                Ok(Err(e)) => toast::show(cx, Toast::error(format!("{name} setup did not finish: {e}. Try again in Settings › Audio."))),
                Err(e) => toast::show(cx, Toast::error(format!("{name} setup did not finish: {e:#}"))),
            });
        })
        .detach();
        self.poll(cx);
        cx.notify();
    }

    /// Redraws while a voice job runs, for its progress bar.
    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.polling {
            return;
        }
        self.polling = true;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(400)).await;
            let go = this
                .update(cx, |this, cx| {
                    cx.notify();
                    let running = this.voice_started.is_some_and(|id| matches!(voice_setup::state(id), Some(SetupState::Running(_))));
                    if !running {
                        this.polling = false;
                    }
                    running
                })
                .unwrap_or(false);
            if !go {
                break;
            }
        })
        .detach();
    }

    // ── Screens ────────────────────────────────────────────────────────────────────────────

    fn welcome_screen(&self, cx: &mut Context<Self>) -> AnyElement {
        let free = self.free_bytes.map(|f| format!("{} free", fmt_bytes(f)));
        div()
            .flex()
            .flex_col()
            .items_center()
            .text_center()
            .gap(px(20.))
            .pt(px(24.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(84.))
                    .rounded(px(radius::XL + 6.))
                    .bg(Tokens::accent_soft())
                    .border_1()
                    .border_color(Tokens::accent_border())
                    .shadow(vec![BoxShadow { color: Tokens::accent_a(0.28).into(), offset: point(px(0.), px(8.)), blur_radius: px(32.), spread_radius: px(0.), inset: false }])
                    .child(icon("brand/mangavox.svg", Tokens::accent(), 48.)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.))
                    .child(div().ramp(ramp::TITLE).text_color(Tokens::text_1()).child(if self.rerun { "Welcome setup" } else { "Welcome to MangaVox" }))
                    .child(div().max_w(px(520.)).ramp(ramp::BODY_LARGE).text_color(Tokens::text_2()).child(
                        "Read manga, watch anime and films, and turn chapters into narrated recap videos. Nothing is switched on yet: choose what you want in a few short steps.",
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .gap(px(10.))
                    .pt(px(8.))
                    .child(
                        btn("welcome-start", if self.rerun { "Start" } else { "Set up MangaVox" }, ButtonKind::Primary)
                            .large()
                            .trailing_icon(ph::ARROW_RIGHT)
                            .kbd("enter")
                            .on_click(cx.listener(|this, _, w, cx| this.start(w, cx))),
                    )
                    .child(
                        btn("welcome-empty", if self.rerun { "Close setup" } else { "Start empty" }, ButtonKind::Ghost)
                            .large()
                            .tooltip(if self.rerun { "Leave without changing anything" } else { "No sources, addons or voices. Add them later in Settings." })
                            .on_click(cx.listener(|this, _, w, cx| this.leave_empty(w, cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .items_center()
                    .gap(px(8.))
                    .pt(px(16.))
                    .text_size(px(size::SM))
                    .text_color(Tokens::text_3())
                    .child(icon_ph(ph::HARD_DRIVE, Tokens::text_3(), icon_px::SM))
                    .child("Your data is kept in")
                    .child(path_chip(self.root.display().to_string()))
                    .when_some(free, |d, f| d.child(f)),
            )
            .child(muted("Takes about a minute. Nothing downloads until you ask, and you can change everything later in Settings."))
            .into_any_element()
    }

    fn uses_screen(&self, cx: &mut Context<Self>) -> AnyElement {
        let card = |u: Use, icon: Ph3, title: &'static str, body: &'static str, cx: &mut Context<Self>| {
            let on = self.uses.contains(&u);
            option_card(SharedString::from(format!("use-{}", u.id())), on)
                .items_center()
                .child(icon_tile(icon.0, icon.1, on))
                .child(card_text(title, body))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(i) = this.uses.iter().position(|x| *x == u) {
                        this.uses.remove(i);
                    } else {
                        this.uses.push(u);
                        this.uses.sort();
                    }
                    cx.notify();
                }))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(heading("What will you use MangaVox for?", "Pick any that apply. The next screens only cover what you pick."))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(card(Use::Manga, Ph3(glyphs::MANGA, Hue::Violet), "Read manga", "Browse sources, keep a library and read chapters online or offline.", cx))
                    .child(card(Use::Watch, Ph3(ph::MONITOR_PLAY, Hue::Blue), "Watch anime and films", "Catalogs are built in; episodes play from streaming addons you add.", cx))
                    .child(card(
                        Use::Studio,
                        Ph3(ph::MAGIC_WAND, Hue::Amber),
                        "Turn manga into recap videos",
                        "Studio mode: adds Studio and the Assistant. Needs a narration voice, FFmpeg and an AI model.",
                        cx,
                    )),
            )
            .into_any_element()
    }

    fn manga_screen(&self, cx: &mut Context<Self>) -> AnyElement {
        let general: Vec<&SourceInfo> = self.builtin.iter().filter(|s| !s.nsfw).collect();
        let adult: Vec<&SourceInfo> = self.builtin.iter().filter(|s| s.nsfw).collect();
        let on_count = self.enabled.len();
        let repos = self.repos.read(cx);
        let ext_count = repos.installed_count();
        let repo_count = repos.repo_count();
        let plugins = self.plugin_box.read(cx).installed_count();
        let tab = |p: MangaPanel, id: &'static str, icon: Ph3, title: &'static str, body: String, cx: &mut Context<Self>| {
            let on = self.panel == p;
            option_card(id, on)
                .flex_col()
                .flex_1()
                .min_w(px(180.))
                .gap(px(10.))
                .child(icon_tile(icon.0, icon.1, on))
                .child(card_text(title, body))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.panel = p;
                    cx.notify();
                }))
        };
        let builtin_body = match on_count {
            0 => format!("{} sites built into MangaVox", general.len()),
            n => format!("{n} switched on"),
        };
        let repo_body = match (repo_count, ext_count) {
            (0, _) => "Paste a Mihon or Tachiyomi repository link".to_string(),
            (_, 0) => format!("{repo_count} added · nothing installed yet"),
            (_, n) => format!("{n} extension{} installed", if n == 1 { "" } else { "s" }),
        };
        let plugin_body = match plugins {
            0 => "Install one plugin from a GitHub link".to_string(),
            n => format!("{n} installed"),
        };
        let panel: AnyElement = match self.panel {
            MangaPanel::Builtin => self.builtin_panel(&general, &adult, cx),
            MangaPanel::Repository => div().flex().flex_col().gap(px(12.)).child(self.repos.clone()).child(ui_manga::backend::trust_note()).into_any_element(),
            MangaPanel::Plugin => div().flex().flex_col().gap(px(12.)).child(self.plugin_box.clone()).child(ui_manga::backend::trust_note()).into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(heading(
                "Add your manga sources",
                "MangaVox comes with no source switched on. Add your repository or plugins here, or turn on built-in sites. Change it any time in Settings › Sources.",
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(10.))
                    .child(tab(MangaPanel::Builtin, "manga-builtin", Ph3(glyphs::SOURCES, Hue::Violet), "Built-in sites", builtin_body, cx))
                    .child(tab(MangaPanel::Repository, "manga-repo", Ph3(ph::GIT_BRANCH, Hue::Cyan), "Add a repository", repo_body, cx))
                    .child(tab(MangaPanel::Plugin, "manga-plugin", Ph3(ph::PUZZLE_PIECE, Hue::Green), "Plugin link", plugin_body, cx)),
            )
            .child(div().p(px(16.)).rounded(px(radius::LG)).bg(Tokens::surface()).border_1().border_color(Tokens::border()).child(panel))
            .into_any_element()
    }

    fn source_switch(&self, i: usize, s: &SourceInfo, cx: &mut Context<Self>) -> Stateful<Div> {
        let on = self.enabled.contains(&s.id);
        let id = s.id.clone();
        div()
            .id(("builtin", i))
            .flex()
            .items_center()
            .gap(px(12.))
            .py(px(8.))
            .child(ui_manga::backend::source_avatar(s, 28., false))
            .child(div().flex_1().min_w_0().flex().items_center().gap(px(8.)).child(div().truncate().text_size(px(size::MD)).text_color(Tokens::text_1()).child(s.name.clone())).when(s.nsfw, |d| d.child(tag("18+", Tone::Danger))))
            .child(switch(("builtin-sw", i), on).tooltip(if on { "Shown in Browse" } else { "Hidden from Browse" }).on_click(cx.listener(move |this, v: &bool, _, cx| this.toggle_source(id.clone(), *v, cx))))
    }

    fn builtin_panel(&self, general: &[&SourceInfo], adult: &[&SourceInfo], cx: &mut Context<Self>) -> AnyElement {
        let all_on = general.iter().all(|s| self.enabled.contains(&s.id));
        let ids: Vec<String> = general.iter().map(|s| s.id.clone()).collect();
        let adult_ids: Vec<String> = adult.iter().map(|s| s.id.clone()).collect();
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(muted(format!("Off until you switch them on. {}", names(general))))
                    .child(btn("builtin-all", if all_on { "Switch all off" } else { "Switch all on" }, ButtonKind::Ghost).small().on_click(cx.listener(move |this, _, _, cx| {
                        for id in &ids {
                            if all_on {
                                this.enabled.remove(id);
                            } else {
                                this.enabled.insert(id.clone());
                            }
                        }
                        cx.notify();
                    }))),
            )
            .children(general.iter().enumerate().map(|(i, s)| self.source_switch(i, s, cx)))
            .when(!adult.is_empty(), |d| {
                d.child(div().mt(px(8.)).pt(px(12.)).border_t_1().border_color(Tokens::border()).child(
                    checkbox("adult-ok", self.adult_ok).child(format!("I am 18 or older. Show the {} adult sources", adult.len())).on_click(cx.listener(move |this, _, _, cx| {
                        this.adult_ok = !this.adult_ok;
                        if !this.adult_ok {
                            for id in &adult_ids {
                                this.enabled.remove(id);
                            }
                        }
                        cx.notify();
                    })),
                ))
                .when(self.adult_ok, |d| d.children(adult.iter().enumerate().map(|(i, s)| self.source_switch(100 + i, s, cx))))
            })
            .into_any_element()
    }

    fn watch_screen(&self, cx: &mut Context<Self>) -> AnyElement {
        let url = self.addon_input.read(cx).value().to_string();
        let empty = url.trim().is_empty();
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(heading(
                "Add a streaming addon",
                "Catalogs, artwork and trailers are built in. Episodes and films play from Stremio-compatible addons that you add. None is preinstalled.",
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .p(px(16.))
                    .rounded(px(radius::LG))
                    .bg(Tokens::surface())
                    .border_1()
                    .border_color(Tokens::border())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().flex_1().min_w_0().child(Input::new(&self.addon_input).cleanable(true).prefix(icon_ph(ph::PLUGS, Tokens::text_3(), icon_px::SM))))
                            .child(icon_button("addon-paste", ph::CLIPBOARD_TEXT, "Paste link").on_click(cx.listener(|this, _, w, cx| this.paste_addon(w, cx))))
                            .child(
                                btn("addon-install", if self.addon_busy { "Adding" } else { "Add" }, ButtonKind::Secondary)
                                    .icon(ph::PLUS)
                                    .loading(self.addon_busy)
                                    .map(|b| if empty { b.disabled("Paste an addon link first") } else { b })
                                    .on_click(cx.listener(|this, _, _, cx| this.install_addon(cx))),
                            ),
                    )
                    .when_some(self.addon_result.as_ref(), |d, r| {
                        let (c, g, t) = match r {
                            Ok(t) => (Tokens::success(), ph::CHECK_CIRCLE, t.clone()),
                            Err(e) => (Tokens::danger(), ph::WARNING_CIRCLE, e.clone()),
                        };
                        d.child(div().flex().items_start().gap(px(6.)).text_size(px(size::SM)).text_color(c).child(div().pt(px(2.)).child(icon_ph(g, c, icon_px::SM))).child(div().min_w_0().child(t)))
                    })
                    .when(!self.addons.is_empty(), |d| {
                        d.child(div().flex().flex_col().pt(px(4.)).children(self.addons.iter().enumerate().map(|(i, (n, v))| {
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .py(px(8.))
                                .when(i > 0, |d| d.border_t_1().border_color(Tokens::border()))
                                .child(icon_ph(ph::PLUGS_CONNECTED, Tokens::accent(), icon_px::MD))
                                .child(div().flex_1().min_w_0().truncate().text_size(px(size::MD)).text_color(Tokens::text_1()).child(n.clone()))
                                .child(muted(format!("v{v}")))
                        })))
                    }),
            )
            .child(fact(ph::SHIELD_CHECK, "Addons are third-party services. Add only ones you trust; manage them later in Settings › Streaming addons."))
            .into_any_element()
    }

    fn voice_card(&self, id: &'static str, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let pack = plan::voice_pack(id)?;
        let selected = self.voice == id;
        let downloaded = voices::installed(pack);
        let gpu = voices::needs_gpu(pack);
        let size_line = format!("{} voices + {} engine · {} in all", fmt_bytes(pack.size()), voice_setup::engine_size_hint(voices::engine_of(pack)), fmt_bytes(plan::voice_download(pack)));
        Some(
            option_card(SharedString::from(format!("voice-{id}")), selected)
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.))
                        .pr(px(28.))
                        .child(div().ramp(ramp::BODY_STRONG).text_color(Tokens::text_1()).child(pack.name))
                        .when(id == self.recommended, |d| d.child(tag("Recommended", Tone::Accent)))
                        .when(gpu, |d| d.child(tag("Needs an NVIDIA GPU", Tone::Warning)))
                        .when(downloaded, |d| d.child(status_tag(StatusKind::Done).label("Downloaded"))),
                )
                .child(div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_2()).child(pack.summary))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(16.))
                        .child(div().flex().items_center().gap(px(6.)).child(muted("Quality")).child(meter(pack.quality as f32, 5)))
                        .child(div().flex().items_center().gap(px(6.)).child(muted("Speed")).child(meter(pack.speed as f32, 5)))
                        .child(fact(ph::TRANSLATE, pack.languages))
                        .child(fact(ph::DOWNLOAD_SIMPLE, size_line)),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.voice = id;
                    cx.notify();
                })),
        )
    }

    fn voice_action(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(pack) = plan::voice_pack(self.voice) else { return div().into_any_element() };
        let state = voice_setup::state(pack.id);
        match state {
            Some(SetupState::Running(p)) => {
                let frac = p.download.map(|d| d.fraction()).unwrap_or(0.);
                let key = pack.id;
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .p(px(14.))
                    .rounded(px(radius::LG))
                    .bg(Tokens::surface())
                    .border_1()
                    .border_color(Tokens::border())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).text_color(Tokens::text_1()).child(format!("Setting up {} · {}", pack.name, p.summary())))
                            .child(btn("voice-cancel", "Cancel", ButtonKind::Ghost).small().tooltip("Stop; what is already downloaded is kept for next time").on_click(cx.listener(move |this, _, _, cx| {
                                voice_setup::cancel(key);
                                this.voice_started = None;
                                cx.notify();
                            }))),
                    )
                    .child(progress_bar("voice-progress", if p.download.is_some() { frac } else { 0. }, true))
                    .child(muted("You can continue: the download keeps going in the background and resumes after a restart. Settings › Audio shows it too."))
                    .into_any_element()
            }
            Some(SetupState::Failed(msg)) => {
                let weak = cx.entity().downgrade();
                inline_alert(AlertKind::Error, format!("{} setup did not finish", pack.name), msg)
                    .action("Try again", move |_, _, cx| {
                        let _ = weak.update(cx, |this, cx| this.start_voice(pack, cx));
                    })
                    .into_any_element()
            }
            None if self.voice_started == Some(pack.id) && voices::installed(pack) => inline_alert(AlertKind::Success, format!("{} is ready to narrate", pack.name), "").into_any_element(),
            None => {
                let total = plan::voice_download(pack);
                let fits = plan::fits(total, self.free_bytes);
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        btn("voice-start", if voices::installed(pack) { format!("Finish setting up · up to {}", voice_setup::engine_size_hint(voices::engine_of(pack))) } else { format!("Download and set up · {}", fmt_bytes(total)) }, ButtonKind::Secondary)
                            .large()
                            .icon(ph::DOWNLOAD_SIMPLE)
                            .when(!fits, |b| b.disabled(format!("Not enough free space: needs {}, {} free", fmt_bytes(total + 500_000_000), fmt_bytes(self.free_bytes.unwrap_or(0)))))
                            .on_click(cx.listener(move |this, _, _, cx| this.start_voice(pack, cx))),
                    )
                    .child(muted("Installs Python if this PC has none, then the engine and the voices. No admin rights needed."))
                    .into_any_element()
            }
        }
    }

    fn voices_screen(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut facts = div().flex().flex_wrap().gap(px(16.));
        if let Some(f) = self.free_bytes {
            facts = facts.child(fact(ph::HARD_DRIVE, format!("{} free on the data drive", fmt_bytes(f))));
        }
        if let Some(r) = self.ram_bytes {
            facts = facts.child(fact(ph::MEMORY, format!("{} of memory", fmt_bytes(r))));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(heading(
                "Pick a narration voice",
                "Studio reads your recaps aloud with a voice that runs on this PC. Nothing downloads until you press the button below.",
            ))
            .child(facts)
            .child(div().flex().flex_col().gap(px(10.)).children(plan::VOICE_CHOICES.into_iter().filter_map(|id| self.voice_card(id, cx))))
            .child(self.voice_action(cx))
            .into_any_element()
    }

    fn done_screen(&self, cx: &mut Context<Self>) -> AnyElement {
        let link = |id: &'static str, tab: &'static str, cx: &mut Context<Self>| -> AnyElement {
            btn(id, "Settings", ButtonKind::Ghost)
                .small()
                .trailing_icon(ph::ARROW_RIGHT)
                .tooltip("Finish the setup and open this page in Settings")
                .on_click(cx.listener(move |this, _, w, cx| this.finish(Route::Settings { tab: Some(tab.into()) }, w, cx)))
                .into_any_element()
        };
        let mut rows: Vec<Div> = vec![];
        if self.has(Use::Manga) || self.has(Use::Studio) {
            let on: Vec<&SourceInfo> = self.builtin.iter().filter(|s| self.enabled.contains(&s.id)).collect();
            let exts = self.repos.read(cx).installed_count() + self.plugin_box.read(cx).installed_count();
            let mut parts = vec![];
            if !on.is_empty() {
                parts.push(format!("{} on", names(&on)));
            }
            if exts > 0 {
                parts.push(format!("{exts} extension{} installed", if exts == 1 { "" } else { "s" }));
            }
            let done = !parts.is_empty();
            let detail = if done { parts.join(" · ") } else { "Nothing switched on. Browse stays empty until you add a source.".into() };
            rows.push(summary_row(done, "Manga sources", detail, Some(link("done-sources", "sources", cx))));
        }
        if self.has(Use::Watch) {
            let n = self.addons.len();
            let detail = if n == 0 { "No addon yet. Catalogs work; add an addon to play.".to_string() } else { format!("{n} addon{} added", if n == 1 { "" } else { "s" }) };
            rows.push(summary_row(n > 0, "Streaming addons", detail, Some(link("done-addons", "addons", cx))));
        }
        if self.has(Use::Studio) {
            // The pack that was started, even if another card was picked afterwards.
            let pack = self.voice_started.and_then(plan::voice_pack);
            let (done, detail) = match (self.voice_started, pack) {
                (Some(id), Some(p)) => match voice_setup::state(id) {
                    Some(SetupState::Running(s)) => (false, format!("{}: {} (keeps going in the background)", p.name, s.summary())),
                    Some(SetupState::Failed(_)) => (false, format!("{} did not finish. Try again in Settings › Audio.", p.name)),
                    None if voices::installed(p) => (true, format!("{} is ready", p.name)),
                    None => (false, format!("{} was stopped. Resume it in Settings › Audio.", p.name)),
                },
                _ => (false, "No voice yet. Studio asks for one before it narrates.".to_string()),
            };
            rows.push(summary_row(done, "Narration voice", detail, Some(link("done-audio", "audio", cx))));
            rows.push(summary_row(false, "AI model for Studio", "Studio reads pages with an AI model. Pick a local or online one.", Some(link("done-ai", "providers", cx))));
        }
        let nothing = self.uses.is_empty();
        let landing = plan::landing(&self.uses);
        let open_label = match landing {
            Route::AnimeHome => "Open Anime",
            Route::Studio => "Open Studio",
            _ => "Open Library",
        };
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(56.))
                    .rounded_full()
                    .bg(Tokens::accent_soft())
                    .child(mv_ui::ph_icon(ph::CHECK_CIRCLE, mv_ui::Weight::Fill, Tokens::accent(), icon_px::HERO)),
            )
            .child(heading(
                if nothing { "MangaVox starts empty" } else { "You're all set" },
                if nothing { "Nothing is switched on. Add sources, addons and voices whenever you like in Settings." } else { "Here is what is set up. You can change all of it later in Settings." },
            ))
            .when(!rows.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .px(px(16.))
                        .rounded(px(radius::LG))
                        .bg(Tokens::surface())
                        .border_1()
                        .border_color(Tokens::border())
                        .children(rows.into_iter().enumerate().map(|(i, r)| r.when(i > 0, |r| r.border_t_1().border_color(Tokens::border())))),
                )
            })
            .child(
                div().flex().child(
                    btn("done-open", open_label, ButtonKind::Primary)
                        .large()
                        .trailing_icon(ph::ARROW_RIGHT)
                        .kbd("enter")
                        .on_click(cx.listener(move |this, _, w, cx| this.finish(landing.clone(), w, cx))),
                ),
            )
            .into_any_element()
    }

    // ── Chrome ─────────────────────────────────────────────────────────────────────────────

    fn header(&self, cx: &mut Context<Self>) -> Div {
        let dots = plan::progress(self.step, &self.uses).map(|(i, n)| {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(dot_trail("welcome-dots", (0..n).map(|k| k <= i).collect(), false, 6.))
                .child(muted(format!("Step {} of {n}", i + 1)))
        });
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .w_full()
            .max_w(px(COLUMN_W))
            .mx_auto()
            .px(px(24.))
            .pt(px(16.))
            .child(icon("brand/mangavox.svg", Tokens::accent(), icon_px::LG))
            .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Setup"))
            .child(div().flex_1())
            .children(dots)
            .when(self.step != Step::Done, |d| {
                d.child(
                    btn("welcome-skip-all", if self.rerun { "Close setup" } else { "Skip setup" }, ButtonKind::Ghost)
                        .small()
                        .tooltip(if self.rerun { "Leave without changing anything more" } else { "Keep what is set up so far and skip the rest" })
                        .on_click(cx.listener(|this, _, w, cx| this.leave_empty(w, cx))),
                )
            })
    }

    fn footer(&self, cx: &mut Context<Self>) -> Div {
        let last = plan::next(self.step, &self.uses) == Step::Done;
        let busy = match self.step {
            Step::Manga => self.repos.read(cx).is_busy() || self.plugin_box.read(cx).is_busy(),
            Step::Watch => self.addon_busy,
            _ => false,
        };
        div()
            .flex_none()
            .border_t_1()
            .border_color(Tokens::border())
            .bg(Tokens::base())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .w_full()
                    .max_w(px(COLUMN_W))
                    .mx_auto()
                    .px(px(24.))
                    .py(px(12.))
                    .child(btn("welcome-back", "Back", ButtonKind::Ghost).icon(glyphs::BACK).on_click(cx.listener(|this, _, w, cx| this.back(w, cx))))
                    .child(div().flex_1())
                    .child(btn("welcome-skip", "Skip this step", ButtonKind::Ghost).kbd("escape").on_click(cx.listener(|this, _, w, cx| this.skip_step(w, cx))))
                    .child(
                        btn("welcome-next", if last { "Finish" } else { "Continue" }, ButtonKind::Primary)
                            .trailing_icon(ph::ARROW_RIGHT)
                            .kbd("enter")
                            .when(busy, |b| b.disabled("Wait for the install to finish"))
                            .on_click(cx.listener(|this, _, w, cx| this.continue_(w, cx))),
                    ),
            )
    }
}

/// An icon and its category tint.
#[derive(Clone, Copy)]
struct Ph3(mv_ui::Ph, Hue);

impl Render for WelcomeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.step {
            Step::Welcome => self.welcome_screen(cx),
            Step::Uses => self.uses_screen(cx),
            Step::Manga => self.manga_screen(cx),
            Step::Watch => self.watch_screen(cx),
            Step::Voices => self.voices_screen(cx),
            Step::Done => self.done_screen(cx),
        };
        let middle = !matches!(self.step, Step::Welcome | Step::Done);
        div()
            .id("welcome")
            .key_context("Welcome")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| this.on_key(ev, w, cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(Tokens::base())
            .text_color(Tokens::text_1())
            .when(self.step != Step::Welcome, |d| d.child(self.header(cx)))
            .child(
                div().id("welcome-scroll").flex_1().min_h_0().overflow_y_scroll().child(
                    div()
                        .w_full()
                        .max_w(px(COLUMN_W))
                        .mx_auto()
                        .px(px(24.))
                        .py(px(28.))
                        // A short fade per screen (instant under Reduce motion).
                        .child(appear(SharedString::from(format!("welcome-{}", self.step.id())), div().w_full().child(body))),
                ),
            )
            .when(middle, |d| d.child(self.footer(cx)))
    }
}
