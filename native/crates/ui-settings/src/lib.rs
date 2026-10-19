//! Settings page (native counterpart of src/renderer/pages/Settings.tsx).
//! Left: grouped category rail. Right: the selected page — a title, a one-line description
//! and flat sections — or, for Model Library, its own sub-navigation and pages.
//! Config is read and written off the UI thread through `mv_ui::bg::blocking`.
mod data;
pub mod models;
mod tabs;
mod ui;

use std::collections::HashMap;

use anyhow::Result;
use gpui_kit::component::input::InputState;
use gpui_kit::component::select::{SelectEvent, SelectState};
use gpui_kit::component::IndexPath;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::{icon, size, Route, Tokens};

use data::{Playback, Snapshot};
use models::{LibraryEvent, ModelLibrary, Page};

/// Saved Studio-mode flag, for the shell's startup.
pub fn studio_saved() -> bool {
    data::studio_saved()
}

pub fn open(route: &Route, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    match route {
        Route::Settings { tab } => {
            let tab = tab.clone();
            Some(cx.new(|cx| SettingsView::new(tab.as_deref(), window, cx)).into())
        }
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    Profile,
    Mode,
    Appearance,
    Playback,
    Sources,
    Addons,
    Providers,
    Models,
    Storage,
    Studio,
    About,
}

/// Rail groups, in order. An empty title draws no label.
const GROUPS: &[(&str, &[Tab])] = &[
    ("", &[Tab::Profile, Tab::Mode]),
    ("Look and feel", &[Tab::Appearance, Tab::Playback]),
    ("Content", &[Tab::Sources, Tab::Addons]),
    ("AI", &[Tab::Providers, Tab::Models]),
    ("Projects and media", &[Tab::Storage]),
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
            "themes" | "appearance" => Tab::Appearance,
            "mode" => Tab::Mode,
            "sources" | "extensions" => Tab::Sources,
            "addons" | "streaming" => Tab::Addons,
            "storage" => Tab::Storage,
            "providers" => Tab::Providers,
            "models" | "model-library" => Tab::Models,
            "studio" => Tab::Studio,
            "about" => Tab::About,
            _ => return None,
        })
    }

    pub fn id(self) -> &'static str {
        match self {
            Tab::Profile => "profile",
            Tab::Playback => "playback",
            Tab::Appearance => "themes",
            Tab::Mode => "mode",
            Tab::Sources => "sources",
            Tab::Addons => "addons",
            Tab::Storage => "storage",
            Tab::Providers => "providers",
            Tab::Models => "models",
            Tab::Studio => "studio",
            Tab::About => "about",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Profile => "General",
            Tab::Playback => "Playback",
            Tab::Appearance => "Appearance",
            Tab::Mode => "App mode",
            Tab::Sources => "Manga sources",
            Tab::Addons => "Streaming addons",
            Tab::Storage => "Storage",
            Tab::Providers => "AI providers",
            Tab::Models => "Model Library",
            Tab::Studio => "Studio",
            Tab::About => "About",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Tab::Profile => "Your name and email. Stored only in your local config file.",
            Tab::Playback => "How episodes and movies play, and which streams and subtitles are picked first.",
            Tab::Appearance => "Colour theme and motion. Changes apply immediately.",
            Tab::Mode => "Turn the Studio workspace and Assistant on or off.",
            Tab::Sources => "Where manga comes from: installed sources, plugins and repositories.",
            Tab::Addons => "Stremio-compatible addons that find streams for movies, series and anime.",
            Tab::Storage => "Download folders and cached data.",
            Tab::Providers => "Local and cloud AI services for the assistant, with keys and default models.",
            Tab::Models => "Models on this PC and what MangaVox uses them for.",
            Tab::Studio => "Defaults for narration and video output in Studio projects.",
            Tab::About => "Version, data folder and links.",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Tab::Profile => "icons/user.svg",
            Tab::Playback => "icons/play.svg",
            Tab::Appearance => "icons/palette.svg",
            Tab::Mode => "icons/toggle-left.svg",
            Tab::Sources => "icons/book-open.svg",
            Tab::Addons => "icons/puzzle.svg",
            Tab::Storage => "icons/hard-drive.svg",
            Tab::Providers => "icons/sparkles.svg",
            Tab::Models => "icons/boxes.svg",
            Tab::Studio => "icons/wand-sparkles.svg",
            Tab::About => "icons/info.svg",
        }
    }

    /// Tabs whose body is an embedded panel from another crate.
    fn is_panel(self) -> bool {
        matches!(self, Tab::Sources | Tab::Addons | Tab::Providers)
    }
}

pub struct Notice {
    pub text: SharedString,
    pub error: bool,
}

pub struct SettingsView {
    tab: Tab,
    name: Entity<InputState>,
    email: Entity<InputState>,
    timer: Entity<SelectState<Vec<SharedString>>>,
    quality: Entity<SelectState<Vec<SharedString>>>,
    subtitle: Entity<SelectState<Vec<SharedString>>>,
    playback: Playback,
    preset: String,
    reduce_motion: bool,
    mode: String,
    studio: data::StudioPrefs,
    studio_voice: Entity<InputState>,
    studio_speed: Entity<InputState>,
    /// Custom folder per `data::STORAGE_KINDS` entry ("" = default).
    storage: Vec<String>,
    /// `None` while the cache size is still being measured.
    cache_bytes: Option<u64>,
    panels: HashMap<Tab, AnyView>,
    library: Option<Entity<ModelLibrary>>,
    /// Model Library page requested by the route (`models:<page>`).
    library_page: Option<Page>,
    notice: Option<Notice>,
    _subs: Vec<Subscription>,
}

fn select_state(
    items: Vec<SharedString>,
    selected: usize,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> Entity<SelectState<Vec<SharedString>>> {
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
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Display name"));
        let email = cx.new(|cx| InputState::new(window, cx).placeholder("you@example.com"));

        let studio_voice = cx.new(|cx| InputState::new(window, cx).placeholder("Default"));
        let studio_speed = cx.new(|cx| InputState::new(window, cx).placeholder("1.0"));
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

        cx.subscribe_in(&timer, window, |this, _, ev: &SelectEvent<Vec<SharedString>>, _, cx| {
            if let SelectEvent::Confirm(Some(v)) = ev {
                if let Ok(secs) = v.trim_end_matches('s').parse::<u64>() {
                    this.playback.auto_play_timer = secs;
                    cx.notify();
                }
            }
        })
        .detach();
        cx.subscribe_in(&quality, window, |this, _, ev: &SelectEvent<Vec<SharedString>>, _, cx| {
            if let SelectEvent::Confirm(Some(v)) = ev {
                this.playback.preferred_quality = v.to_string();
                cx.notify();
            }
        })
        .detach();
        cx.subscribe_in(&subtitle, window, |this, _, ev: &SelectEvent<Vec<SharedString>>, _, cx| {
            if let SelectEvent::Confirm(Some(v)) = ev {
                this.playback.subtitle_language = subtitle_code(v).to_string();
                cx.notify();
            }
        })
        .detach();

        let library_page = tab.and_then(|t| t.split_once(':')).and_then(|(_, p)| Page::from_id(p));
        let mut this = Self {
            tab: tab.and_then(Tab::from_id).unwrap_or(Tab::Profile),
            name,
            email,
            timer,
            quality,
            subtitle,
            playback: defaults,
            preset: data::DEFAULT_PRESET.into(),
            reduce_motion: false,
            mode: if mv_ui::mode::studio_enabled(cx) { "studio" } else { "entertainment" }.into(),
            studio: data::StudioPrefs::default(),
            studio_voice,
            studio_speed,
            storage: vec![String::new(); data::STORAGE_KINDS.len()],
            cache_bytes: None,
            panels: HashMap::new(),
            library: None,
            library_page,
            notice: None,
            _subs: vec![],
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
        let initial = this.tab;
        this.ensure_panel(initial, window, cx);

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
        let Snapshot { profile, playback, preset, reduce_motion, mode, storage, cache_bytes } = snap;
        let name = profile.name.clone();
        self.name.update(cx, |s, cx| s.set_value(name, window, cx));
        let email = profile.email;
        self.email.update(cx, |s, cx| s.set_value(email, window, cx));

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
        self.storage = storage;
        self.cache_bytes = Some(cache_bytes);
        cx.notify();
    }

    fn apply_studio(&mut self, prefs: data::StudioPrefs, window: &mut Window, cx: &mut Context<Self>) {
        let (voice, speed) = (prefs.voice.clone(), format!("{}", prefs.speed));
        self.studio_voice.update(cx, |s, cx| s.set_value(voice, window, cx));
        self.studio_speed.update(cx, |s, cx| s.set_value(speed, window, cx));
        self.studio = prefs;
        cx.notify();
    }

    fn set_resolution(&mut self, res: &'static str, cx: &mut Context<Self>) {
        self.studio.resolution = res.to_string();
        cx.notify();
    }

    fn save_studio(&mut self, cx: &mut Context<Self>) {
        let speed = self.studio_speed.read(cx).value().trim().parse::<f64>().ok().filter(|s| (0.5..=2.0).contains(s));
        let Some(speed) = speed else {
            self.report("", Err(anyhow::anyhow!("Speed must be a number between 0.5 and 2.0")), cx);
            return;
        };
        self.studio.voice = self.studio_voice.read(cx).value().trim().to_string();
        self.studio.speed = speed;
        let p = self.studio.clone();
        self.background(cx, move || data::save_studio_prefs(&p), |v, r, cx| v.report("Studio defaults saved", r, cx));
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
        self.tab = tab;
        self.notice = None;
        self.ensure_panel(tab, window, cx);
        cx.notify();
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

    fn report(&mut self, ok_text: &str, result: Result<()>, cx: &mut Context<Self>) {
        self.notice = Some(match result {
            Ok(()) => Notice { text: ok_text.to_string().into(), error: false },
            Err(e) => Notice { text: format!("{e:#}").into(), error: true },
        });
        cx.notify();
    }

    fn save_profile(&mut self, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_string();
        let name = if name.is_empty() { "User".to_string() } else { name };
        let email = self.email.read(cx).value().trim().to_string();
        if !email.is_empty() && !email.contains('@') {
            self.report("", Err(anyhow::anyhow!("Enter a valid email address")), cx);
            return;
        }
        let (n, e) = (name, email.clone());
        self.background(cx, move || data::save_profile(&n, &e), |v, r, cx| v.report("Profile saved", r, cx));
    }

    fn save_playback(&mut self, cx: &mut Context<Self>) {
        let p = self.playback.clone();
        self.background(cx, move || data::save_playback(&p), |v, r, cx| v.report("Playback settings saved", r, cx));
    }

    fn set_preset(&mut self, preset: &'static str, cx: &mut Context<Self>) {
        if self.preset == preset {
            return;
        }
        self.preset = preset.to_string();
        cx.notify();
        self.background(cx, move || data::save_preset(preset), |v, r, cx| v.report_result(r, cx));
    }

    fn set_reduce_motion(&mut self, on: bool, cx: &mut Context<Self>) {
        self.reduce_motion = on;
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

    /// Only reports failures; used for changes that already show their effect.
    fn report_result(&mut self, result: Result<()>, cx: &mut Context<Self>) {
        if let Err(e) = result {
            self.report("", Err(e), cx);
        } else {
            cx.notify();
        }
    }

    fn pick_storage(&mut self, idx: usize, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            let picked = match rx.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let Some(path) = picked else { return };
            let kind = data::STORAGE_KINDS[idx].0;
            let saved = mv_ui::bg::blocking(move || data::save_storage(kind, &path.to_string_lossy())).await.and_then(|r| r);
            this.update(cx, |v, cx| match saved {
                Ok(value) => {
                    v.storage[idx] = value;
                    v.report(&format!("{} folder updated", data::STORAGE_KINDS[idx].1), Ok(()), cx);
                }
                Err(e) => v.report("", Err(e), cx),
            })
            .ok();
        })
        .detach();
    }

    fn reset_storage(&mut self, idx: usize, cx: &mut Context<Self>) {
        let kind = data::STORAGE_KINDS[idx].0;
        self.background(cx, move || data::save_storage(kind, ""), move |v, r, cx| match r {
            Ok(value) => {
                v.storage[idx] = value;
                v.report("Folder reset to default", Ok(()), cx);
            }
            Err(e) => v.report("", Err(e), cx),
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

    fn clear_cache(&mut self, cx: &mut Context<Self>) {
        self.background(cx, data::clear_cache, |v, r, cx| {
            if r.is_ok() {
                v.cache_bytes = Some(0);
            }
            v.report("Cache cleared", r, cx);
        });
    }

    fn rail_item(&self, id: &'static str, icon_path: &'static str, label: &'static str, active: bool) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(32.))
            .px(px(10.))
            .rounded(px(8.))
            .border_1()
            .text_size(px(size::SM))
            .cursor_pointer()
            .map(|d| {
                if active {
                    d.bg(Tokens::active()).border_color(rgba(0x2bd49a59)).text_color(Tokens::text_1()).font_weight(FontWeight::MEDIUM)
                } else {
                    d.border_color(transparent_black()).text_color(Tokens::text_2()).hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
                }
            })
            .child(icon(icon_path, if active { Tokens::accent() } else { Tokens::text_3() }, 15.))
            .child(label)
    }

    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut groups = div().flex().flex_col().gap(px(18.));
        let studio_on = mv_ui::mode::studio_enabled(cx);
        for (title, tabs) in GROUPS.iter().chain(studio_on.then_some(&STUDIO_GROUP)) {
            let mut group = div().flex().flex_col().gap(px(2.));
            if !title.is_empty() {
                group = group.child(div().px(px(10.)).pb(px(6.)).child(ui::section_label(title)));
            }
            for tab in tabs.iter().copied() {
                group = group.child(
                    self.rail_item(tab.id(), tab.icon(), tab.label(), self.tab == tab)
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.select_tab(tab, window, cx))),
                );
            }
            groups = groups.child(group);
        }
        let bottom = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .pt(px(12.))
            .border_t_1()
            .border_color(Tokens::border())
            .child(self.rail_item("help", "icons/life-buoy.svg", "Help & feedback", false).on_click(|_, _, cx| cx.open_url(HELP_URL)))
            .child(
                self.rail_item(Tab::About.id(), Tab::About.icon(), Tab::About.label(), self.tab == Tab::About)
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.select_tab(Tab::About, window, cx))),
            );

        div()
            .w(px(232.))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(12.))
            .px(px(12.))
            .pt(px(22.))
            .pb(px(14.))
            .bg(Tokens::sidebar())
            .border_r_1()
            .border_color(Tokens::border())
            .child(div().px(px(10.)).pb(px(6.)).text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Settings"))
            .child(div().id("settings-nav").flex_1().min_h_0().overflow_y_scroll().child(groups))
            .child(bottom)
    }

    fn render_body(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.tab {
            Tab::Profile => self.render_profile(cx).into_any_element(),
            Tab::Playback => self.render_playback(cx).into_any_element(),
            Tab::Appearance => self.render_appearance(cx).into_any_element(),
            Tab::Mode => self.render_mode(cx).into_any_element(),
            Tab::Storage => self.render_storage(cx).into_any_element(),
            Tab::Studio => self.render_studio(cx).into_any_element(),
            Tab::About => self.render_about(cx).into_any_element(),
            tab => match self.panels.get(&tab) {
                Some(view) => div().w_full().child(view.clone()).into_any_element(),
                None => mv_ui::empty_state(tab.label(), "Loading…").into_any_element(),
            },
        }
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content: AnyElement = match (&self.library, self.tab) {
            (Some(lib), Tab::Models) => div().flex_1().h_full().min_w_0().child(lib.clone()).into_any_element(),
            _ => {
                let body = self.render_body(cx);
                let column = div()
                    .w_full()
                    .max_w(px(760.))
                    .px(px(40.))
                    .py(px(32.))
                    .flex()
                    .flex_col()
                    .gap(px(22.))
                    .child(ui::page_title(self.tab.label(), self.tab.description()))
                    .when_some(self.notice.as_ref(), |d, n| d.child(tabs::notice_bar(n)))
                    .child(body);
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
}
