//! Settings page (native counterpart of src/renderer/pages/Settings.tsx).
//! Left: grouped tab list (220 px). Right: the selected tab, centred, max 860 px.
//! Config is read and written off the UI thread through `mv_ui::bg::blocking`.
mod data;
mod tabs;

use std::collections::HashMap;

use anyhow::Result;
use gpui_kit::component::input::{InputState};
use gpui_kit::component::select::{SelectEvent, SelectState};
use gpui_kit::component::IndexPath;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::{size, Route, Tokens};

use data::{Playback, Snapshot};

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
    Playback,
    Appearance,
    Mode,
    Sources,
    Addons,
    Storage,
    Providers,
    Models,
    About,
}

/// Sidebar groups, in order.
const GROUPS: &[(&str, &[Tab])] = &[
    ("General", &[Tab::Profile, Tab::Playback, Tab::Appearance, Tab::Mode]),
    ("Content", &[Tab::Sources, Tab::Addons, Tab::Storage]),
    ("AI", &[Tab::Providers, Tab::Models]),
    ("App", &[Tab::About]),
];

impl Tab {
    /// Route ids (`Route::Settings { tab }`). `themes` and `mode` are the Electron ids.
    pub fn from_id(id: &str) -> Option<Tab> {
        Some(match id {
            "profile" => Tab::Profile,
            "playback" => Tab::Playback,
            "themes" => Tab::Appearance,
            "mode" => Tab::Mode,
            "sources" => Tab::Sources,
            "addons" => Tab::Addons,
            "storage" => Tab::Storage,
            "providers" => Tab::Providers,
            "models" => Tab::Models,
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
            Tab::About => "about",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Profile => "Profile",
            Tab::Playback => "Playback",
            Tab::Appearance => "Appearance",
            Tab::Mode => "App mode",
            Tab::Sources => "Sources",
            Tab::Addons => "Addons",
            Tab::Storage => "Storage",
            Tab::Providers => "AI Providers",
            Tab::Models => "Model Hub",
            Tab::About => "About",
        }
    }

    /// Tabs whose body is an embedded panel from another crate.
    fn is_panel(self) -> bool {
        matches!(self, Tab::Sources | Tab::Addons | Tab::Providers | Tab::Models)
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
    /// Custom folder per `data::STORAGE_KINDS` entry ("" = default).
    storage: Vec<String>,
    /// `None` while the cache size is still being measured.
    cache_bytes: Option<u64>,
    panels: HashMap<Tab, AnyView>,
    notice: Option<Notice>,
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
            mode: "entertainment".into(),
            storage: vec![String::new(); data::STORAGE_KINDS.len()],
            cache_bytes: None,
            panels: HashMap::new(),
            notice: None,
        };
        let initial = this.tab;
        this.ensure_panel(initial, window, cx);

        cx.spawn_in(window, async move |this, cx| {
            if let Ok(snap) = mv_ui::bg::blocking(data::load_snapshot).await {
                this.update_in(cx, |v, window, cx| v.apply(snap, window, cx)).ok();
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

    fn ensure_panel(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        if !tab.is_panel() || self.panels.contains_key(&tab) {
            return;
        }
        let view = match tab {
            Tab::Sources => ui_manga::sources_panel(window, cx),
            Tab::Addons => ui_media::addons_panel(window, cx),
            Tab::Providers => ui_assistant::providers_panel(window, cx),
            _ => ui_assistant::model_hub_panel(window, cx),
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
        cx.notify();
        self.background(cx, move || data::save_mode(mode), |v, r, cx| {
            let text = if v.mode == "studio" { "Manga Studio is unlocked" } else { "Entertainment mode is active" };
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

    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut nav = div()
            .id("settings-nav")
            .w(px(220.))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(16.))
            .px(px(12.))
            .py(px(20.))
            .bg(Tokens::sidebar())
            .border_r_1()
            .border_color(Tokens::border())
            .overflow_y_scroll();
        for (title, tabs) in GROUPS {
            let mut group = div().flex().flex_col().gap(px(2.)).child(
                div()
                    .px(px(10.))
                    .pb(px(4.))
                    .text_size(px(size::XS))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(Tokens::text_3())
                    .child(title.to_uppercase()),
            );
            for tab in tabs.iter().copied() {
                let active = self.tab == tab;
                group = group.child(
                    div()
                        .id(tab.id())
                        .h(px(30.))
                        .px(px(10.))
                        .rounded(px(8.))
                        .flex()
                        .items_center()
                        .text_size(px(size::SM))
                        .cursor_pointer()
                        .map(|d| {
                            if active {
                                d.bg(Tokens::active()).text_color(Tokens::text_1()).font_weight(FontWeight::SEMIBOLD)
                            } else {
                                d.text_color(Tokens::text_2()).hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
                            }
                        })
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.select_tab(tab, window, cx)))
                        .child(tab.label()),
                );
            }
            nav = nav.child(group);
        }
        nav
    }

    fn render_body(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.tab {
            Tab::Profile => self.render_profile(cx).into_any_element(),
            Tab::Playback => self.render_playback(cx).into_any_element(),
            Tab::Appearance => self.render_appearance(cx).into_any_element(),
            Tab::Mode => self.render_mode(cx).into_any_element(),
            Tab::Storage => self.render_storage(cx).into_any_element(),
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
        let body = self.render_body(cx);
        let mut column = div()
            .w_full()
            .max_w(px(860.))
            .mx_auto()
            .px(px(28.))
            .py(px(28.))
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(mv_ui::page_header(self.tab.label(), None));
        if let Some(n) = &self.notice {
            column = column.child(tabs::notice_bar(n));
        }
        column = column.child(body);

        div()
            .size_full()
            .flex()
            .bg(Tokens::base())
            .text_color(Tokens::text_1())
            .text_size(px(size::MD))
            .child(self.render_nav(cx))
            .child(
                div()
                    .id("settings-content")
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .overflow_y_scroll()
                    .child(column),
            )
    }
}
