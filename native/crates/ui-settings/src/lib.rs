//! Settings page (native counterpart of src/renderer/pages/Settings.tsx).
//! Left: a searchable, grouped category rail with colour-coded icon tiles. Right: the selected
//! page (a heading with the same tile, then flat `setting_section`s) or, for Model Library, its
//! own sub-navigation and pages. Settings apply and save as they change; feedback is a toast.
//! Config is read and written off the UI thread through `mv_ui::bg::blocking`.
mod audio;
mod cli_tools;
pub mod cli_updates;
mod data;
pub mod models;
mod storage;
mod system;
mod tabs;
mod ui;

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{SelectEvent, SelectState};
use gpui_kit::component::slider::{SliderEvent, SliderState};
use gpui_kit::component::IndexPath;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::icon as icon_px;
use mv_ui::toast::{self, Toast};
use mv_ui::{glyphs, ph, ph_icon, size, Hue, Ph, Route, Tokens, Weight};

use data::{Playback, Profile, Snapshot};
use models::{LibraryEvent, ModelLibrary, Page};

/// Saved Studio-mode flag, for the shell's startup.
pub fn studio_saved() -> bool {
    data::studio_saved()
}

pub use cli_updates::{cli_updates_available, start_cli_update_checks};

pub fn open(route: &Route, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    match route {
        Route::Settings { tab } => {
            let tab = tab.clone();
            Some(cx.new(|cx| SettingsView::new(tab.as_deref(), window, cx)).into())
        }
        _ => None,
    }
}

/// The page an open Settings view shows, for the shell's breadcrumb ("Settings › Audio").
/// `None` for any other view. The view notifies when the page changes, so the shell can
/// `cx.observe` the downcast entity. NN/g breadcrumbs: <https://www.nngroup.com/articles/breadcrumbs/>.
pub fn current_tab(view: &AnyView, cx: &App) -> Option<Tab> {
    view.clone().downcast::<SettingsView>().ok().map(|v| v.read(cx).tab)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    Profile,
    Mode,
    Appearance,
    Playback,
    Audio,
    Sources,
    Addons,
    Providers,
    Models,
    CliTools,
    Storage,
    System,
    Studio,
    About,
}

/// Rail groups, in order. An empty title draws no label.
const GROUPS: &[(&str, &[Tab])] = &[
    ("", &[Tab::Profile, Tab::Mode]),
    ("Look and feel", &[Tab::Appearance, Tab::Playback, Tab::Audio]),
    ("Content", &[Tab::Sources, Tab::Addons]),
    ("AI", &[Tab::Providers, Tab::Models, Tab::CliTools]),
    ("Device", &[Tab::Storage, Tab::System]),
];

/// Shown only while Studio mode is on.
const STUDIO_GROUP: (&str, &[Tab]) = ("Studio", &[Tab::Studio]);

const HELP_URL: &str = "https://github.com/memegyanfactory-gif/mangavox/issues";

impl Tab {
    /// Route ids (`Route::Settings { tab }`). `themes` and `mode` are the Electron ids;
    /// `models:<page>` opens a Model Library page (e.g. `models:engines`).
    pub fn from_id(id: &str) -> Option<Tab> {
        let id = id.split(':').next().unwrap_or(id);
        Some(match id {
            "profile" | "general" => Tab::Profile,
            "playback" => Tab::Playback,
            "audio" | "voices" => Tab::Audio,
            "system" | "performance" => Tab::System,
            "themes" | "appearance" => Tab::Appearance,
            "mode" => Tab::Mode,
            "sources" | "extensions" => Tab::Sources,
            "addons" | "streaming" => Tab::Addons,
            "storage" => Tab::Storage,
            "providers" => Tab::Providers,
            "models" | "model-library" => Tab::Models,
            "cli-tools" | "cli" => Tab::CliTools,
            "studio" => Tab::Studio,
            "about" => Tab::About,
            _ => return None,
        })
    }

    pub fn id(self) -> &'static str {
        match self {
            Tab::Profile => "profile",
            Tab::Playback => "playback",
            Tab::Audio => "audio",
            Tab::System => "system",
            Tab::Appearance => "themes",
            Tab::Mode => "mode",
            Tab::Sources => "sources",
            Tab::Addons => "addons",
            Tab::Storage => "storage",
            Tab::Providers => "providers",
            Tab::Models => "models",
            Tab::CliTools => "cli-tools",
            Tab::Studio => "studio",
            Tab::About => "about",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Profile => "General",
            Tab::Playback => "Playback",
            Tab::Audio => "Audio",
            Tab::System => "System",
            Tab::Appearance => "Appearance",
            Tab::Mode => "App mode",
            Tab::Sources => "Manga sources",
            Tab::Addons => "Streaming addons",
            Tab::Storage => "Storage",
            Tab::Providers => "AI providers",
            Tab::Models => "Model Library",
            Tab::CliTools => "CLI tools",
            Tab::Studio => "Studio",
            Tab::About => "About",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Tab::Profile => "Your name and email. Stored only in your local config file.",
            Tab::Playback => "How episodes and movies play, and which streams and subtitles are picked first.",
            Tab::Audio => "Narration voices that run on this PC: set up an engine, hear every voice and pick the default narrator.",
            Tab::System => "Keep the PC responsive while MangaVox narrates, renders or runs local models.",
            Tab::Appearance => "Colour theme and motion. Changes apply immediately.",
            Tab::Mode => "Turn the Studio workspace and Assistant on or off.",
            Tab::Sources => "Where manga comes from: installed sources, plugins and repositories.",
            Tab::Addons => "Stremio-compatible addons that find streams for movies, series and anime.",
            Tab::Storage => "Where MangaVox keeps your data, what takes up space, and how to free it.",
            Tab::Providers => "Local and cloud AI services for the assistant, with keys and default models.",
            Tab::Models => "Models on this PC and what MangaVox uses them for.",
            Tab::CliTools => "Installed coding-agent CLIs, their versions and updates.",
            Tab::Studio => "Defaults for narration and video output in Studio projects.",
            Tab::About => "Version, data folder and links.",
        }
    }

    /// Phosphor glyph, one per destination. App mode is an app window (a toggle glyph read as
    /// a switch that is off), AI providers a circuit head (sparkle collided with Studio's wand).
    pub fn icon(self) -> Ph {
        match self {
            Tab::Profile => ph::USER_CIRCLE,
            Tab::Playback => ph::PLAY_CIRCLE,
            Tab::Audio => ph::WAVEFORM,
            Tab::System => ph::CPU,
            Tab::Appearance => ph::PALETTE,
            Tab::Mode => ph::APP_WINDOW,
            Tab::Sources => glyphs::SOURCES,
            Tab::Addons => ph::PLUGS_CONNECTED,
            Tab::Storage => glyphs::STORAGE,
            Tab::Providers => ph::HEAD_CIRCUIT,
            Tab::Models => glyphs::MODEL,
            Tab::CliTools => ph::TERMINAL_WINDOW,
            Tab::Studio => glyphs::STUDIO,
            Tab::About => ph::INFO,
        }
    }

    /// Category colour of the rail tile and page heading.
    pub fn hue(self) -> Hue {
        match self {
            Tab::Profile => Hue::Slate,
            Tab::Mode => Hue::Blue,
            Tab::Appearance => Hue::Rose,
            Tab::Playback => Hue::Orange,
            Tab::Audio => Hue::Cyan,
            Tab::Sources => Hue::Blue,
            Tab::Addons => Hue::Violet,
            Tab::Providers => Hue::Amber,
            Tab::Models => Hue::Green,
            Tab::CliTools => Hue::Slate,
            Tab::Storage => Hue::Blue,
            Tab::System => Hue::Green,
            Tab::Studio => Hue::Violet,
            Tab::About => Hue::Slate,
        }
    }

    /// Words people search for that the label does not say (also useful to the command palette).
    pub fn keywords(self) -> &'static str {
        match self {
            Tab::Profile => "name email account user",
            Tab::Playback => "autoplay auto-play next episode countdown skip intro subtitles subtitle language quality resolution stream pre-download",
            Tab::Audio => "voice voices narrator narration tts kokoro piper chatterbox emotion speech sample lab",
            Tab::System => "cpu memory ram resource limit performance threads",
            Tab::Appearance => "theme colour color dark preset accent motion animation reduce",
            Tab::Mode => "studio entertainment assistant workspace",
            Tab::Sources => "manga plugins repositories extensions",
            Tab::Addons => "stremio streams torrent movies series anime",
            Tab::Storage => "folder disk space delete reset cache downloads data move factory",
            Tab::Providers => "api key openai claude gemini ollama lm studio openrouter cloud",
            Tab::Models => "gguf ollama whisper comfyui local models engines",
            Tab::CliTools => "claude code codex gemini cli update terminal",
            Tab::Studio => "resolution speed default voice video export",
            Tab::About => "version update licence github bug report",
        }
    }

    /// Case-insensitive match on the label, description or keywords (`q` is lowercase).
    pub fn matches(self, q: &str) -> bool {
        let q = q.trim();
        q.is_empty() || [self.label(), self.description(), self.keywords()].iter().any(|t| t.to_lowercase().contains(q))
    }

    /// Tabs whose body is an embedded panel from another crate.
    fn is_panel(self) -> bool {
        matches!(self, Tab::Sources | Tab::Addons | Tab::Providers)
    }
}

pub struct SettingsView {
    tab: Tab,
    search: Entity<InputState>,
    query: String,
    name: Entity<InputState>,
    email: Entity<InputState>,
    /// What was last saved, so a blur without changes does not write again. `None` until the
    /// config is read: the empty fields must never overwrite the saved name.
    saved_profile: Option<Profile>,
    email_error: Option<SharedString>,
    timer: Entity<SelectState<Vec<SharedString>>>,
    quality: Entity<SelectState<Vec<SharedString>>>,
    subtitle: Entity<SelectState<Vec<SharedString>>>,
    playback: Playback,
    preset: String,
    reduce_motion: bool,
    mode: String,
    studio: data::StudioPrefs,
    studio_speed: Entity<SliderState>,
    audio: audio::AudioState,
    lab_voice: Entity<SelectState<Vec<SharedString>>>,
    /// Resource limit percentage shown next to the slider.
    limit: u8,
    limit_slider: Entity<SliderState>,
    meter: Option<system::Meter>,
    sampling: bool,
    panels: HashMap<Tab, AnyView>,
    library: Option<Entity<ModelLibrary>>,
    cli: Entity<cli_updates::CliUpdates>,
    /// Model Library page requested by the route (`models:<page>`).
    library_page: Option<Page>,
    storage_mgr: Entity<storage::StorageManager>,
    /// Field that just autosaved, with a counter so an older timer cannot hide a newer mark.
    saved: Option<(&'static str, u64)>,
    /// Bumped on every profile keystroke, so only the last one's delayed save runs.
    profile_edits: u64,
    _subs: Vec<Subscription>,
}

fn select_state(items: Vec<SharedString>, selected: usize, window: &mut Window, cx: &mut Context<SettingsView>) -> Entity<SelectState<Vec<SharedString>>> {
    cx.new(|cx| SelectState::new(items, Some(IndexPath::new(selected)), window, cx))
}

fn timer_label(seconds: u64) -> SharedString {
    format!("{seconds}s").into()
}

fn subtitle_label(code: &str) -> &'static str {
    data::SUBTITLE_LANGS.iter().find(|(c, _)| *c == code).map(|(_, l)| *l).unwrap_or("English")
}

fn subtitle_code(label: &str) -> &'static str {
    data::SUBTITLE_LANGS.iter().find(|(_, l)| *l == label).map(|(c, _)| *c).unwrap_or("en")
}

impl SettingsView {
    fn new(tab: Option<&str>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search settings"));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Display name"));
        let email = cx.new(|cx| InputState::new(window, cx).placeholder("you@example.com"));

        let limit_slider = system::limit_slider(window, cx);
        let studio_speed = tabs::speed_slider(window, cx);
        let defaults = Playback::default();
        let timers: Vec<SharedString> = data::TIMERS.iter().map(|t| timer_label(*t)).collect();
        let timer_idx = data::TIMERS.iter().position(|t| *t == defaults.auto_play_timer).unwrap_or(2);
        let timer = select_state(timers, timer_idx, window, cx);

        let qualities: Vec<SharedString> = data::QUALITIES.iter().map(|q| SharedString::from(*q)).collect();
        let quality_idx = data::QUALITIES.iter().position(|q| *q == defaults.preferred_quality).unwrap_or(2);
        let quality = select_state(qualities, quality_idx, window, cx);

        let langs: Vec<SharedString> = data::SUBTITLE_LANGS.iter().map(|(_, l)| SharedString::from(*l)).collect();
        let lang_idx = data::SUBTITLE_LANGS.iter().position(|(c, _)| *c == defaults.subtitle_language).unwrap_or(0);
        let subtitle = select_state(langs, lang_idx, window, cx);

        let lab_voice = audio::lab_voice_select(window, cx);

        let mut subs = vec![];
        // Playback selects apply (and save) as soon as a value is picked.
        subs.push(cx.subscribe_in(&timer, window, |this, _, ev: &SelectEvent<Vec<SharedString>>, _, cx| {
            if let SelectEvent::Confirm(Some(v)) = ev {
                if let Ok(secs) = v.trim_end_matches('s').parse::<u64>() {
                    this.playback.auto_play_timer = secs;
                    this.save_playback(cx);
                }
            }
        }));
        subs.push(cx.subscribe_in(&quality, window, |this, _, ev: &SelectEvent<Vec<SharedString>>, _, cx| {
            if let SelectEvent::Confirm(Some(v)) = ev {
                this.playback.preferred_quality = v.to_string();
                this.save_playback(cx);
            }
        }));
        subs.push(cx.subscribe_in(&subtitle, window, |this, _, ev: &SelectEvent<Vec<SharedString>>, _, cx| {
            if let SelectEvent::Confirm(Some(v)) = ev {
                this.playback.subtitle_language = subtitle_code(v).to_string();
                this.save_playback(cx);
            }
        }));
        // Profile fields save when you leave them or press Enter, and a moment after typing
        // stops: kit controls (the shell's sidebar included) do not take focus on click, so
        // leaving Settings that way never blurs the field and the edit was lost.
        for field in [&name, &email] {
            subs.push(cx.subscribe_in(field, window, |this, _, ev: &InputEvent, _, cx| match ev {
                InputEvent::Blur | InputEvent::PressEnter { .. } => this.save_profile(cx),
                InputEvent::Change => {
                    if this.email_error.take().is_some() {
                        cx.notify();
                    }
                    this.save_profile_soon(cx);
                }
                _ => {}
            }));
        }
        subs.push(cx.subscribe_in(&search, window, |this, state, ev: &InputEvent, window, cx| match ev {
            InputEvent::Change => {
                this.query = state.read(cx).value().to_string();
                cx.notify();
            }
            // Enter opens the first match; with nothing typed it stays on the current page.
            InputEvent::PressEnter { .. } if !this.query.trim().is_empty() => {
                if let Some(first) = this.visible_tabs(cx).into_iter().next() {
                    this.select_tab(first, window, cx);
                }
            }
            _ => {}
        }));
        subs.push(cx.subscribe_in(&studio_speed, window, |this, _, ev: &SliderEvent, _, cx| match ev {
            SliderEvent::Change(v) => {
                this.studio.speed = tabs::round_speed(v.start());
                cx.notify();
            }
            SliderEvent::Release(v) => {
                this.studio.speed = tabs::round_speed(v.start());
                this.save_studio(cx);
            }
        }));

        let library_page = tab.and_then(|t| t.split_once(':')).and_then(|(_, p)| Page::from_id(p));
        let mut this = Self {
            tab: tab.and_then(Tab::from_id).unwrap_or(Tab::Profile),
            search,
            query: String::new(),
            name,
            email,
            saved_profile: None,
            email_error: None,
            timer,
            quality,
            subtitle,
            playback: defaults,
            // The live palette, so the active preset is right before the config is read.
            preset: mv_ui::theme::palette_id().into(),
            reduce_motion: false,
            mode: if mv_ui::mode::studio_enabled(cx) { "studio" } else { "entertainment" }.into(),
            studio: data::StudioPrefs::default(),
            studio_speed,
            audio: audio::AudioState::default(),
            lab_voice,
            limit: mv_core::limits::percent(),
            limit_slider,
            meter: None,
            sampling: false,
            panels: HashMap::new(),
            library: None,
            cli: cli_updates::cli_updates(cx),
            library_page,
            storage_mgr: storage::create(cx),
            saved: None,
            profile_edits: 0,
            _subs: subs,
        };
        // The rail's Studio group follows the live mode flag.
        let obs = cx.observe_global::<mv_ui::mode::StudioMode>(|this, cx| {
            this.mode = if mv_ui::mode::studio_enabled(cx) { "studio" } else { "entertainment" }.into();
            if this.mode != "studio" && this.tab == Tab::Studio {
                this.tab = Tab::Mode;
            }
            cx.notify();
        });
        this._subs.push(obs);
        let updater = mv_ui::update::updater(cx);
        this._subs.push(cx.observe(&updater, |_, _, cx| cx.notify()));
        let cli = this.cli.clone();
        this._subs.push(cx.observe(&cli, |_, _, cx| cx.notify()));
        // The shell drops this view when the palette opens another Settings page; an edit typed
        // in the last moment before that would otherwise never reach the 1.2 s autosave.
        this._subs.push(cx.on_release(|this, cx| {
            if let Some(p) = this.pending_profile(cx) {
                std::thread::spawn(move || data::save_profile(&p.name, &p.email));
            }
        }));
        cli_updates::start_cli_update_checks(cx);
        let initial = this.tab;
        this.ensure_panel(initial, window, cx);
        this.on_tab_shown(cx);
        // Live parts: the System meter, setup / download progress and samples on Audio.
        cx.spawn(async move |this, cx| {
            let mut n: u32 = 0;
            // One more tick after the page goes quiet, so a sample that just ended (or a stop
            // the player finished after the click) repaints Play instead of a stale Stop.
            let mut was_busy = false;
            loop {
                cx.background_executor().timer(Duration::from_millis(500)).await;
                n = n.wrapping_add(1);
                let alive = this.update(cx, |v, cx| match v.tab {
                    Tab::System if n.is_multiple_of(2) => v.sample_meter(cx),
                    Tab::Audio | Tab::Studio => {
                        let busy = v.audio.busy();
                        if busy || was_busy {
                            v.audio_tick(cx);
                        }
                        was_busy = busy;
                    }
                    _ => {}
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();

        cx.spawn_in(window, async move |this, cx| {
            if let Ok((snap, studio)) = mv_ui::bg::blocking(|| (data::load_snapshot(), data::load_studio_prefs())).await {
                this.update_in(cx, |v, window, cx| {
                    v.apply(snap, window, cx);
                    v.apply_studio(studio, window, cx);
                })
                .ok();
            }
        })
        .detach();
        this
    }

    /// Copies a loaded snapshot into the form state and the selects / inputs.
    fn apply(&mut self, snap: Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        let Snapshot { profile, playback, preset, reduce_motion, mode } = snap;
        let name = profile.name.clone();
        self.name.update(cx, |s, cx| s.set_value(name, window, cx));
        let email = profile.email.clone();
        self.email.update(cx, |s, cx| s.set_value(email, window, cx));
        self.saved_profile = Some(profile);

        let t = timer_label(playback.auto_play_timer);
        self.timer.update(cx, |s, cx| s.set_selected_value(&t, window, cx));
        let q = SharedString::from(playback.preferred_quality.clone());
        self.quality.update(cx, |s, cx| s.set_selected_value(&q, window, cx));
        let l = SharedString::from(subtitle_label(&playback.subtitle_language));
        self.subtitle.update(cx, |s, cx| s.set_selected_value(&l, window, cx));

        self.playback = playback;
        self.preset = preset;
        self.reduce_motion = reduce_motion;
        self.mode = mode;
        cx.notify();
    }

    fn apply_studio(&mut self, prefs: data::StudioPrefs, window: &mut Window, cx: &mut Context<Self>) {
        let speed = prefs.speed as f32;
        self.studio_speed.update(cx, |s, cx| s.set_value(speed, window, cx));
        self.studio = prefs;
        cx.notify();
    }

    fn set_resolution(&mut self, res: &'static str, cx: &mut Context<Self>) {
        if self.studio.resolution == res {
            return;
        }
        self.studio.resolution = res.to_string();
        self.save_studio(cx);
    }

    fn save_studio(&mut self, cx: &mut Context<Self>) {
        self.studio.speed = self.studio.speed.clamp(0.5, 2.0);
        let p = self.studio.clone();
        cx.notify();
        self.background(cx, move || data::save_studio_prefs(&p), |v, r, cx| v.report_result(r, cx));
    }

    fn ensure_panel(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        if tab == Tab::Models && self.library.is_none() {
            let page = self.library_page.take();
            let lib = cx.new(|cx| ModelLibrary::new(page, window, cx));
            let sub = cx.subscribe_in(&lib, window, |this, _, ev: &LibraryEvent, window, cx| match ev {
                LibraryEvent::OpenTab(id) => {
                    if let Some(t) = Tab::from_id(id) {
                        this.select_tab(t, window, cx);
                    }
                }
            });
            self._subs.push(sub);
            self.library = Some(lib);
            return;
        }
        if !tab.is_panel() || self.panels.contains_key(&tab) {
            return;
        }
        let view = match tab {
            Tab::Sources => ui_manga::sources_panel(window, cx),
            Tab::Addons => ui_media::addons_panel(window, cx),
            _ => ui_assistant::providers_panel(window, cx),
        };
        self.panels.insert(tab, view);
    }

    fn select_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        // Leaving General with an edited field: keep it (inputs save on blur, which a click
        // on the rail does not always cause).
        if self.tab == Tab::Profile && tab != Tab::Profile {
            self.save_profile(cx);
            // The error sits under a field that is about to go off screen: say it here.
            if self.email_error.is_some() {
                toast::show(cx, Toast::warning("Your email was not saved. Enter a full address, like you@example.com."));
            }
        }
        self.tab = tab;
        // Back on Model Library: show the narrator and chat model picked meanwhile elsewhere.
        if let (Tab::Models, Some(lib)) = (tab, &self.library) {
            lib.update(cx, |l, cx| l.refresh_selections(window, cx));
        }
        self.ensure_panel(tab, window, cx);
        self.on_tab_shown(cx);
        cx.notify();
    }

    /// Loads what a page needs the first time it is shown.
    fn on_tab_shown(&mut self, cx: &mut Context<Self>) {
        match self.tab {
            // Studio's speed preview needs the same engine check.
            Tab::Audio | Tab::Studio => self.audio_open(cx),
            Tab::System => self.sample_meter(cx),
            Tab::Storage => self.storage_mgr.update(cx, |m, cx| m.refresh(cx)),
            _ => {}
        }
    }

    /// Runs blocking work off the UI thread, then applies the result on the view.
    fn background<T, F, A>(&self, cx: &mut Context<Self>, work: F, done: A)
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T> + Send + 'static,
        A: FnOnce(&mut Self, Result<T>, &mut Context<Self>) + 'static,
    {
        cx.spawn(async move |this, cx| {
            let result = mv_ui::bg::blocking(work).await.and_then(|r| r);
            this.update(cx, |v, cx| done(v, result, cx)).ok();
        })
        .detach();
    }

    /// Success toast (when there is something to say) or an error toast that stays longer.
    /// Material snackbars: <https://m3.material.io/components/snackbar/guidelines>.
    fn report(&mut self, ok_text: &str, result: Result<()>, cx: &mut Context<Self>) {
        match result {
            Ok(()) if !ok_text.is_empty() => toast::show(cx, Toast::success(ok_text.to_string())),
            Ok(()) => {}
            Err(e) => toast::show(cx, Toast::error(format!("{e:#}"))),
        }
        cx.notify();
    }

    /// Only reports failures; used for changes that already show their effect.
    fn report_result(&mut self, result: Result<()>, cx: &mut Context<Self>) {
        self.report("", result, cx);
    }

    /// Shows the passive "Saved" mark next to `field` for a moment.
    fn flash_saved(&mut self, field: &'static str, cx: &mut Context<Self>) {
        let n = self.saved.map_or(1, |s| s.1 + 1);
        self.saved = Some((field, n));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1800)).await;
            this.update(cx, |v, cx| {
                if v.saved.is_some_and(|s| s.1 == n) {
                    v.saved = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn saved_now(&self, field: &str) -> bool {
        self.saved.is_some_and(|s| s.0 == field)
    }

    /// Validates and saves the profile when it changed. Errors show under the field.
    fn save_profile(&mut self, cx: &mut Context<Self>) {
        self.save_profile_with(false, cx);
    }

    /// Autosave 1.2 s after the last keystroke (Google Docs / Notion style). A half-typed email
    /// is skipped quietly; its error shows only when the field is left or Enter is pressed.
    /// NN/g, inline validation timing: <https://www.nngroup.com/articles/errors-forms-design-guidelines/>.
    fn save_profile_soon(&mut self, cx: &mut Context<Self>) {
        self.profile_edits += 1;
        let n = self.profile_edits;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1200)).await;
            this.update(cx, |v, cx| {
                if v.profile_edits == n {
                    v.save_profile_with(true, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// The fields as a profile (an empty name is "User") and whether the email is usable.
    fn profile_fields(&self, cx: &App) -> (Profile, bool) {
        let name = self.name.read(cx).value().trim().to_string();
        let name = if name.is_empty() { "User".to_string() } else { name };
        let email = self.email.read(cx).value().trim().to_string();
        let valid = email.is_empty() || email.contains('@');
        (Profile { name, email }, valid)
    }

    /// The profile in the fields when it is valid and differs from what was saved.
    fn pending_profile(&self, cx: &App) -> Option<Profile> {
        let (next, valid) = self.profile_fields(cx);
        let saved = self.saved_profile.as_ref()?;
        (valid && next != *saved).then_some(next)
    }

    fn save_profile_with(&mut self, quiet: bool, cx: &mut Context<Self>) {
        let Some(saved) = self.saved_profile.clone() else { return };
        let (next, valid) = self.profile_fields(cx);
        if !valid {
            if quiet {
                return;
            }
            self.email_error = Some("Enter a full email address, like you@example.com".into());
            return cx.notify();
        }
        self.email_error = None;
        if next == saved {
            return cx.notify();
        }
        let changed = if next.name != saved.name { "name" } else { "email" };
        let p = next.clone();
        self.background(cx, move || data::save_profile(&p.name, &p.email), move |v, r, cx| match r {
            Ok(()) => {
                v.saved_profile = Some(next);
                v.flash_saved(changed, cx);
            }
            Err(e) => v.report("", Err(e), cx),
        });
    }

    fn save_playback(&mut self, cx: &mut Context<Self>) {
        let p = self.playback.clone();
        cx.notify();
        self.background(cx, move || data::save_playback(&p), |v, r, cx| v.report_result(r, cx));
    }

    fn set_preset(&mut self, preset: &'static str, cx: &mut Context<Self>) {
        if self.preset == preset {
            return;
        }
        self.preset = preset.to_string();
        mv_ui::theme::set_palette(preset, cx);
        cx.notify();
        self.background(cx, move || data::save_preset(preset), |v, r, cx| v.report_result(r, cx));
    }

    fn set_reduce_motion(&mut self, on: bool, cx: &mut Context<Self>) {
        self.reduce_motion = on;
        mv_ui::motion::set_reduced(on, cx);
        cx.notify();
        self.background(cx, move || data::save_reduce_motion(on), |v, r, cx| v.report_result(r, cx));
    }

    fn set_mode(&mut self, mode: &'static str, cx: &mut Context<Self>) {
        if self.mode == mode {
            return;
        }
        self.mode = mode.to_string();
        mv_ui::mode::set_studio(cx, mode == "studio");
        cx.notify();
        self.background(cx, move || data::save_mode(mode), |v, r, cx| {
            let text = if v.mode == "studio" { "Studio mode is on" } else { "Studio mode is off" };
            v.report(text, r, cx);
        });
    }

    /// Creates the folder if needed, then shows it in the system file manager.
    fn open_folder(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        self.background(
            cx,
            move || {
                std::fs::create_dir_all(&path)?;
                Ok(path)
            },
            |_, r, cx| {
                if let Ok(p) = r {
                    cx.reveal_path(&p);
                }
            },
        );
    }

    /// Rail destinations that match the search, in rail order (About last).
    fn visible_tabs(&self, cx: &App) -> Vec<Tab> {
        let q = self.query.trim().to_lowercase();
        let studio_on = mv_ui::mode::studio_enabled(cx);
        GROUPS
            .iter()
            .chain(studio_on.then_some(&STUDIO_GROUP))
            .flat_map(|(_, tabs)| tabs.iter().copied())
            .chain([Tab::About])
            .filter(|t| t.matches(&q))
            .collect()
    }

    fn rail_entry(&self, tab: Tab, cx: &mut Context<Self>) -> Stateful<Div> {
        let dot = tab == Tab::CliTools && self.cli.read(cx).banners().iter().any(|(k, _)| k.contains('@'));
        // The dot is colour and shape only, so it also says what it means (WCAG 1.4.1).
        ui::rail_item(tab.id(), tab.icon(), tab.hue(), tab.label(), self.tab == tab)
            .when(dot, |d| d.child(div().flex_none().size(px(7.)).rounded_full().bg(Tokens::accent())).tooltip(mv_ui::tip("A CLI update is available")))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.select_tab(tab, window, cx)))
    }

    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let q = self.query.trim().to_lowercase();
        let searching = !q.is_empty();
        let mut groups = div().flex().flex_col().gap(px(16.));
        let studio_on = mv_ui::mode::studio_enabled(cx);
        let mut shown = 0;
        for (title, tabs) in GROUPS.iter().chain(studio_on.then_some(&STUDIO_GROUP)) {
            let tabs: Vec<Tab> = tabs.iter().copied().filter(|t| t.matches(&q)).collect();
            if tabs.is_empty() {
                continue;
            }
            shown += tabs.len();
            let mut group = div().flex().flex_col().gap(px(2.));
            if !title.is_empty() && !searching {
                group = group.child(mv_ui::section_label(*title).px(px(8.)));
            }
            for tab in tabs {
                group = group.child(self.rail_entry(tab, cx));
            }
            groups = groups.child(group);
        }
        let about_match = Tab::About.matches(&q);
        if searching && shown == 0 && !about_match {
            groups = groups.child(mv_ui::empty_view(glyphs::SEARCH, "No matching settings", "Try a word like subtitles, voice or cache.").compact());
        }
        let help = ui::rail_item("help", ph::LIFEBUOY, Hue::Slate, "Help & feedback", false)
            .child(ph_icon(ph::ARROW_UP_RIGHT, Weight::Bold, Tokens::text_3(), icon_px::XS))
            .tooltip(mv_ui::tip("Opens the issue tracker in your browser"))
            .on_click(|_, _, cx| cx.open_url(HELP_URL));
        let bottom = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .pt(px(10.))
            .border_t_1()
            .border_color(Tokens::border())
            .child(help)
            .when(!searching || about_match, |d| d.child(self.rail_entry(Tab::About, cx)));

        div()
            .w(px(240.))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(12.))
            .px(px(12.))
            .pt(px(20.))
            .pb(px(12.))
            .bg(Tokens::sidebar())
            .border_r_1()
            .border_color(Tokens::border())
            .child(div().px(px(8.)).text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Settings"))
            .child(Input::new(&self.search).cleanable(true).prefix(ph_icon(glyphs::SEARCH, Weight::Bold, Tokens::text_3(), icon_px::SM)))
            .child(div().id("settings-nav").flex_1().min_h_0().overflow_y_scroll().child(groups))
            .child(bottom)
    }

    fn render_body(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.tab {
            Tab::Profile => self.render_profile(cx).into_any_element(),
            Tab::Playback => self.render_playback(cx).into_any_element(),
            Tab::Audio => self.render_audio(cx).into_any_element(),
            Tab::System => self.render_system(cx).into_any_element(),
            Tab::Appearance => self.render_appearance(cx).into_any_element(),
            Tab::Mode => self.render_mode(cx).into_any_element(),
            Tab::Storage => self.storage_mgr.clone().into_any_element(),
            Tab::Studio => self.render_studio(cx).into_any_element(),
            Tab::About => self.render_about(cx).into_any_element(),
            Tab::CliTools => self.render_cli_tools(cx).into_any_element(),
            tab => match self.panels.get(&tab) {
                Some(view) => div().w_full().child(view.clone()).into_any_element(),
                // Shaped like the panel's rows, not a second title.
                None => div().flex().flex_col().gap(px(4.)).children((0..4).map(|_| mv_ui::skeleton_row())).into_any_element(),
            },
        }
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content: AnyElement = match (&self.library, self.tab) {
            // The library draws its own header; CLI update banners (an AI page) sit above it,
            // which they never did before because this branch skipped them.
            (Some(lib), Tab::Models) => {
                let banners = self.render_cli_banners(cx);
                if banners.is_empty() {
                    div().flex_1().h_full().min_w_0().child(lib.clone()).into_any_element()
                } else {
                    div()
                        .flex_1()
                        .h_full()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().flex_none().flex().flex_col().gap(px(8.)).px(px(32.)).pt(px(16.)).children(banners))
                        .child(div().flex_1().min_h_0().child(lib.clone()))
                        .into_any_element()
                }
            }
            _ => {
                let tab = self.tab;
                let body = div().flex().flex_col().gap(px(24.)).children(self.render_cli_banners(cx)).child(self.render_body(cx));
                let column = div()
                    .w_full()
                    .max_w(px(780.))
                    .px(px(40.))
                    .pt(px(32.))
                    .pb(px(48.))
                    .flex()
                    .flex_col()
                    .gap(px(24.))
                    .child(ui::page_head(tab.icon(), tab.hue(), tab.label(), tab.description()))
                    // A short fade on page change (instant under reduced motion).
                    .child(mv_ui::appear(("settings-page", tab as usize), body));
                div().id("settings-content").flex_1().h_full().min_w_0().overflow_y_scroll().child(column).into_any_element()
            }
        };

        div()
            .size_full()
            .flex()
            .bg(Tokens::base())
            .text_color(Tokens::text_1())
            .text_size(px(size::MD))
            .child(self.render_nav(cx))
            .child(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn tab_ids_round_trip_and_aliases() {
        for (_, tabs) in GROUPS {
            for t in tabs.iter() {
                assert_eq!(Tab::from_id(t.id()), Some(*t));
            }
        }
        assert_eq!(Tab::from_id("about"), Some(Tab::About));
        assert_eq!(Tab::from_id("addons"), Some(Tab::Addons));
        assert_eq!(Tab::from_id("streaming"), Some(Tab::Addons));
        assert_eq!(Tab::from_id("extensions"), Some(Tab::Sources));
        assert_eq!(Tab::from_id("models:engines"), Some(Tab::Models));
        assert_eq!(Page::from_id("engines"), Some(Page::Engines));
        assert_eq!(Tab::from_id("nope"), None);
        // Every tab id the assistant's open_settings tool can send resolves.
        for id in ui_assistant::tools::SETTINGS_TABS {
            assert!(Tab::from_id(id).is_some(), "{id}");
        }
    }

    #[::core::prelude::v1::test]
    fn search_finds_settings_by_what_they_do() {
        assert!(Tab::Playback.matches("subtitle"));
        assert!(Tab::System.matches("resource limit"));
        assert!(Tab::Audio.matches("chatterbox"));
        assert!(Tab::Storage.matches("cache"));
        assert!(!Tab::About.matches("subtitle"));
        assert!(Tab::About.matches(""));
    }
}
