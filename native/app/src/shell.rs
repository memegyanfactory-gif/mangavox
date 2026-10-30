//! App shell: a 40px title bar (sidebar toggle, back and forward, breadcrumbs, search, the
//! assistant toggle), the sidebar (a 60px rail when collapsed or when the window is narrow), the
//! page, the assistant dock on the trailing edge and the command palette (Ctrl+K).
//!
//! Layout follows Fluent NavigationView (expanded pane at 1008px and wider, compact rail below,
//! the pane opening as an overlay in compact mode) and HIG split views (supplementary panes on
//! the trailing side): <https://learn.microsoft.com/en-us/windows/apps/design/controls/navigationview>,
//! <https://developer.apple.com/design/human-interface-guidelines/split-views>
use std::collections::{HashMap, HashSet};

use gpui_kit::component::command::CommandState;
use gpui_kit::component::{FocusTrapElement as _, TitleBar, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::nav::{set_back_target, set_current_route, set_navigator};
use mv_ui::theme::{icon as icon_px, radius};
use mv_ui::toast::{self, Toast};
use mv_ui::update::{version, Status, Updater};
use mv_ui::{appear, btn, empty_view, glyphs, icon_button, icon_ph, kbd, nav_item, ph, section_label, segmented, size, skeleton_line, tip, ButtonKind, Interactive as _, Ph, Route, Tokens};

use crate::keys::{self, Shortcut};
use crate::palette::{self, Cmd, Palette, Title};
use crate::prefs::{self, ShellPrefs, DOCK_MAX, DOCK_MIN};
use crate::routes::{is_detail, is_focus, is_workspace, keep_alive, nav_for, nav_label, Section, SECTIONS};
use crate::updates;

/// Fluent NavigationView: the expanded pane needs 1008px; below it the sidebar is a rail.
const RAIL_BELOW: f32 = 1008.;
/// Below this the assistant floats over the page instead of squeezing it.
const DOCK_OVERLAY_BELOW: f32 = 900.;
/// The search field turns into an icon button below this width.
const SEARCH_PILL_FROM: f32 = 840.;
const SIDEBAR_W: f32 = 256.;
const RAIL_W: f32 = 60.;
/// The page never gets narrower than this next to the docked assistant.
const PAGE_MIN: f32 = 480.;
/// Windows title bars grow from 32px when they hold controls; 40px fits a 32px button with air
/// (the gpui-component caption buttons are a fixed 34px wide, so 48px would make them look thin).
const TITLE_BAR_H: f32 = 40.;
const HISTORY_MAX: usize = 50;

/// A history entry. The view is kept so Back restores the page exactly as it was;
/// the player is never kept (it would keep playing in the background).
struct Entry {
    route: Route,
    view: Option<AnyView>,
}

impl Entry {
    fn new(route: Route, view: AnyView) -> Self {
        let view = (!matches!(route, Route::Player { .. })).then_some(view);
        Entry { route, view }
    }
}

/// One section's navigation state, parked while another section is shown.
struct Parked {
    route: Route,
    page: AnyView,
    history: Vec<Entry>,
    forward: Vec<Entry>,
}

/// Where a breadcrumb leads.
#[derive(Clone, Debug, PartialEq)]
enum CrumbTo {
    Home(Section),
    Up(Route),
    Manga(String),
}

struct Crumb {
    label: SharedString,
    to: Option<CrumbTo>,
    /// The name is still being looked up: drawn as a skeleton, not as a generic word.
    loading: bool,
}

fn here(label: impl Into<SharedString>) -> Crumb {
    Crumb { label: label.into(), to: None, loading: false }
}

impl Crumb {
    fn loading(mut self, on: bool) -> Self {
        self.loading = on;
        self
    }
}

/// Drag payload for the assistant's resize edge.
#[derive(Clone)]
struct DockResize;

struct DragGhost;
impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

pub struct Shell {
    /// The user's sidebar choice (Studio forces the rail and restores this afterwards).
    collapsed: bool,
    /// Narrow window: the full sidebar shown over the page from the rail.
    nav_overlay: bool,
    section: Section,
    route: Route,
    history: Vec<Entry>,
    forward: Vec<Entry>,
    page: AnyView,
    kept: HashMap<String, AnyView>,
    parked: HashMap<Section, Parked>,
    dock: Option<AnyView>,
    dock_open: bool,
    dock_w: f32,
    /// The assistant edge is being dragged.
    resizing: bool,
    /// Sidebar state to restore after leaving the (wide) Studio page.
    saved_collapsed: Option<bool>,
    /// The layout last written to config.json.
    saved: ShellPrefs,
    palette: Option<Palette>,
    /// Manga and chapter names for breadcrumbs, looked up once per id.
    manga_names: HashMap<String, SharedString>,
    chapter_names: HashMap<String, SharedString>,
    /// Manga and chapter ids whose names are being looked up right now.
    looking_up: HashSet<String>,
    /// The page the open Settings view shows ("Settings › Audio"); it changes inside the view.
    settings_tab: Option<ui_settings::Tab>,
    _settings_watch: Option<Subscription>,
    font: SharedString,
    window: AnyWindowHandle,
    /// Bumped on every navigation; a keystroke that navigated is not handled again here.
    nav_serial: u64,
    serial_at_key: u64,
    /// "Check for updates" from the palette: report the result in a toast.
    manual_update_check: bool,
    _subs: Vec<Subscription>,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        mv_ui::mode::init(cx, ui_settings::studio_saved());
        let mut subs = vec![cx.observe_global_in::<mv_ui::mode::StudioMode>(window, |this, window, cx| this.mode_changed(window, cx))];
        let shell = cx.entity().downgrade();
        set_navigator(cx, move |route, window, cx| {
            if let Some(s) = shell.upgrade() {
                s.update(cx, |s, cx| s.go(route, window, cx));
            }
        });
        // Shortcuts work wherever focus is (see keys.rs); the interceptor notes the navigation
        // count before the page sees the key, so a key a page used to navigate is left alone.
        let weak = cx.entity().downgrade();
        subs.push(cx.intercept_keystrokes(move |_, _, cx| {
            if let Some(s) = weak.upgrade() {
                s.update(cx, |s, _| s.serial_at_key = s.nav_serial);
            }
        }));
        subs.push(cx.observe_keystrokes(|this, ev, window, cx| this.on_keystroke(ev, window, cx)));
        let updater = mv_ui::update::updater(cx);
        subs.push(cx.observe(&updater, |this, u, cx| this.update_changed(u, cx)));
        // The download notification and the Downloads badge follow the queue.
        let dl = ui_manga::downloads(cx);
        subs.push(cx.observe(&dl, |_, _, cx| cx.notify()));
        // Title bar dims while the window is inactive.
        subs.push(cx.observe_window_activation(window, |_, _, cx| cx.notify()));
        let saved = prefs::load();
        // A fresh install (or a setup left part-way) opens on the welcome setup.
        let route = if ui_welcome::shows_at_start() { Route::Welcome } else { Route::Library };
        let page = Self::resolve(&route, window, cx);
        let mut shell = Self {
            collapsed: saved.collapsed,
            nav_overlay: false,
            section: Section::Manga,
            route,
            history: vec![],
            forward: vec![],
            page,
            kept: Default::default(),
            parked: Default::default(),
            dock: None,
            dock_open: false,
            dock_w: saved.dock_w,
            resizing: false,
            saved_collapsed: None,
            saved,
            palette: None,
            manga_names: Default::default(),
            chapter_names: Default::default(),
            looking_up: Default::default(),
            settings_tab: None,
            _settings_watch: None,
            font: ui_font(cx),
            window: window.window_handle(),
            nav_serial: 0,
            serial_at_key: 0,
            manual_update_check: false,
            _subs: subs,
        };
        shell.sync_back(cx);
        shell.sync_window_title(window);
        shell
    }

    // ── Pages ───────────────────────────────────────────────────────────────────────────────

    /// Open `route`, reusing the live page for keep-alive routes. Anime and Movies each keep
    /// their own Discover, so a filter set in one does not change the other.
    fn open(&mut self, route: &Route, window: &mut Window, cx: &mut App) -> AnyView {
        if !keep_alive(route) {
            return Self::resolve(route, window, cx);
        }
        let key = format!("{:?}/{route:?}", self.section);
        if let Some(v) = self.kept.get(&key) {
            ui_assistant::set_page_context(route);
            return v.clone();
        }
        let v = Self::resolve(route, window, cx);
        self.kept.insert(key, v.clone());
        v
    }

    fn resolve(route: &Route, window: &mut Window, cx: &mut App) -> AnyView {
        // Keep the assistant aware of where the user is.
        ui_assistant::set_page_context(route);
        ui_manga::open(route, window, cx)
            .or_else(|| ui_media::open(route, window, cx))
            .or_else(|| ui_player::open(route, window, cx))
            .or_else(|| ui_assistant::open(route, window, cx))
            .or_else(|| ui_settings::open(route, window, cx))
            .or_else(|| ui_studio::open(route, window, cx))
            .or_else(|| ui_phone::open(route, window, cx))
            .or_else(|| ui_youtube::open(route, window, cx))
            .or_else(|| ui_welcome::open(route, window, cx))
            .unwrap_or_else(|| {
                let title = route_title(route).into();
                cx.new(|_| Placeholder { title }).into()
            })
    }

    // ── Navigation ──────────────────────────────────────────────────────────────────────────

    fn go(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) {
        if route == Route::Back {
            self.back(window, cx);
            return;
        }
        self.nav_overlay = false;
        if self.already_there(&route) {
            cx.notify();
            return;
        }
        if self.route == Route::Welcome {
            self.leave_welcome(route, window, cx);
            return;
        }
        // A destination of another section (the assistant's "open my library", a link from a
        // page) opens in its own section, so the sidebar, breadcrumbs and Back belong to it.
        if nav_label(self.section, &route).is_none() {
            if let Some(s) = SECTIONS.into_iter().find(|s| nav_label(*s, &route).is_some()) {
                // Studio and Settings belong to no section, so Back from the destination still
                // leads to them ("Open Library" in Studio, then Back).
                let carry = (is_workspace(&self.route) || matches!(self.route, Route::Mobile | Route::Settings { .. })).then(|| self.current_entry()).filter(|e| e.view.is_some());
                self.set_section(s, window, cx);
                if self.already_there(&route) {
                    // The section opened on the destination itself (its home, or the page it was
                    // left on): Back still leads to Studio / Settings.
                    if let Some(e) = carry {
                        push_capped(&mut self.history, e);
                        self.forward.clear();
                        self.after_nav(window, cx);
                    }
                    return;
                }
                if let Some(Entry { route: from, view: Some(view) }) = carry {
                    let parked = Entry::new(std::mem::replace(&mut self.route, from), std::mem::replace(&mut self.page, view));
                    push_capped(&mut self.history, parked);
                }
            }
        }
        self.leave_fullscreen(window);
        // Moving between Settings pages replaces the page instead of stacking Back steps.
        let between_settings = matches!(self.route, Route::Settings { .. }) && matches!(route, Route::Settings { .. });
        let page = self.open(&route, window, cx);
        let prev = std::mem::replace(&mut self.route, route);
        let prev_view = std::mem::replace(&mut self.page, page);
        if !between_settings {
            push_capped(&mut self.history, Entry::new(prev, prev_view));
        }
        self.forward.clear();
        self.after_nav(window, cx);
    }

    /// The welcome setup is not a page to go back to: leaving it opens `route` in its own
    /// section with a fresh history (and never parks the setup in a section).
    fn leave_welcome(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = SECTIONS.into_iter().find(|s| nav_label(*s, &route).is_some()) {
            self.parked.remove(&s);
            self.section = s;
        }
        self.history.clear();
        self.forward.clear();
        self.page = self.open(&route, window, cx);
        self.route = route;
        self.after_nav(window, cx);
    }

    /// `route` is the page on screen. The Settings view moves between its pages itself, so
    /// Settings with no page named (Ctrl+,, the palette's "Settings") keeps the page you are on,
    /// and a named page counts as there only when the view shows it.
    fn already_there(&self, route: &Route) -> bool {
        match (route, &self.route) {
            (Route::Settings { tab: None }, Route::Settings { .. }) => true,
            // `models:<page>` names a Model Library sub-page the view does not report.
            (Route::Settings { tab: Some(id) }, Route::Settings { .. }) if !id.contains(':') => ui_settings::Tab::from_id(id).is_some_and(|t| Some(t) == self.settings_tab),
            _ => *route == self.route,
        }
    }

    /// Back leads to the previous page, or to the section's home page when there is none.
    fn back_route(&self) -> Option<Route> {
        match self.history.last() {
            Some(e) => Some(e.route.clone()),
            None => self.home_fallback(),
        }
    }

    fn home_fallback(&self) -> Option<Route> {
        (self.route != self.section.home() && !is_focus(&self.route)).then(|| self.section.home())
    }

    fn current_entry(&self) -> Entry {
        Entry::new(self.route.clone(), self.page.clone())
    }

    fn leave_fullscreen(&self, window: &mut Window) {
        if matches!(self.route, Route::Player { .. }) && window.is_fullscreen() {
            window.toggle_fullscreen();
        }
    }

    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The page undoes its own last step first (closes its dialog, leaves a genre or a
        // "See all" grid), so Back returns to exactly where the user was.
        if mv_ui::local_back(self.page.entity_id(), window, cx) {
            cx.notify();
            return;
        }
        let target = match self.history.pop() {
            Some(e) => e,
            // Nothing to go back to (e.g. right after a section switch): land on the section home.
            None => match self.home_fallback() {
                Some(home) => Entry { route: home, view: None },
                None => return,
            },
        };
        self.leave_fullscreen(window);
        let cur = self.current_entry();
        push_capped(&mut self.forward, cur);
        self.show(target, window, cx);
    }

    /// Alt+Right / the mouse forward button: the page Back left.
    fn forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(next) = self.forward.pop() else { return };
        self.leave_fullscreen(window);
        let cur = self.current_entry();
        push_capped(&mut self.history, cur);
        self.show(next, window, cx);
    }

    /// Back several steps at once (a breadcrumb): the pages in between move to Forward.
    fn jump_back(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.history.len() {
            return;
        }
        self.leave_fullscreen(window);
        let mut later = self.history.split_off(ix + 1);
        let Some(target) = self.history.pop() else { return };
        let cur = self.current_entry();
        push_capped(&mut self.forward, cur);
        while let Some(e) = later.pop() {
            push_capped(&mut self.forward, e);
        }
        self.show(target, window, cx);
    }

    /// Up to `route`: back to it when it is in history (as it was left), otherwise open it.
    /// Already there (the logo clicked on the home page) does nothing, instead of swapping the
    /// page for an older copy of itself from history.
    fn go_up(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) {
        if self.already_there(&route) {
            // Still closes the floating sidebar.
            self.go(route, window, cx);
            return;
        }
        match self.history.iter().rposition(|e| e.route == route) {
            Some(ix) => self.jump_back(ix, window, cx),
            None => self.go(route, window, cx),
        }
    }

    fn go_up_to_manga(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        match self.history.iter().rposition(|e| matches!(&e.route, Route::MangaDetail { manga_id, .. } if *manga_id == id)) {
            Some(ix) => self.jump_back(ix, window, cx),
            None => self.go(Route::MangaDetail { manga_id: id, source_id: None }, window, cx),
        }
    }

    fn show(&mut self, e: Entry, window: &mut Window, cx: &mut Context<Self>) {
        self.page = match e.view {
            Some(v) => {
                ui_assistant::set_page_context(&e.route);
                v
            }
            None => self.open(&e.route, window, cx),
        };
        self.route = e.route;
        self.nav_overlay = false;
        self.after_nav(window, cx);
    }

    fn after_nav(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.nav_serial += 1;
        self.sync_back(cx);
        self.watch_settings(window, cx);
        self.resolve_names(window, cx);
        self.sync_window_title(window);
        cx.notify();
    }

    /// Follows the page the Settings view shows, for "Settings › Audio" and the palette.
    fn watch_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = matches!(self.route, Route::Settings { .. }).then(|| self.page.clone().downcast::<ui_settings::SettingsView>().ok()).flatten();
        self.settings_tab = view.as_ref().and_then(|v| ui_settings::current_tab(&v.clone().into(), cx));
        self._settings_watch = view.map(|v| {
            cx.observe_in(&v, window, |this, v, window, cx| {
                let tab = ui_settings::current_tab(&v.into(), cx);
                if tab != this.settings_tab {
                    this.settings_tab = tab;
                    this.sync_window_title(window);
                    cx.notify();
                }
            })
        });
    }

    /// Keeps the back target current (pages label their own back control with it) and gives
    /// the wide Studio page the 60px rail, restoring the user's own sidebar state afterwards.
    fn sync_back(&mut self, cx: &mut App) {
        set_back_target(cx, self.back_route());
        set_current_route(cx, self.route.clone());
        if is_workspace(&self.route) {
            if self.saved_collapsed.is_none() {
                self.saved_collapsed = Some(self.collapsed);
                self.collapsed = true;
            }
        } else if let Some(prev) = self.saved_collapsed.take() {
            self.collapsed = prev;
        }
    }

    /// The window title names the page (taskbar, Alt+Tab and screen readers).
    fn sync_window_title(&self, window: &mut Window) {
        let page = match (&self.route, self.settings_tab) {
            (Route::Settings { .. }, Some(t)) => format!("{} – Settings", t.label()),
            (r, _) => self.label(r).to_string(),
        };
        window.set_window_title(&format!("{page} – MangaVox"));
    }

    /// Switching sections parks the current one (page, history and forward) and restores the
    /// target's. Settings, Studio and the player are not parked: the section keeps the page
    /// under them.
    fn set_section(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) {
        if section == self.section {
            return;
        }
        self.leave_fullscreen(window);
        let mut route = std::mem::replace(&mut self.route, section.home());
        let mut page = self.page.clone();
        let mut history = std::mem::take(&mut self.history);
        let mut forward = std::mem::take(&mut self.forward);
        let parked = loop {
            if !matches!(route, Route::Player { .. } | Route::Settings { .. } | Route::Mobile) && !is_workspace(&route) {
                break true;
            }
            forward.clear();
            match history.pop() {
                Some(Entry { route: r, view: Some(v) }) => {
                    route = r;
                    page = v;
                }
                Some(_) => {}
                None => break false,
            }
        };
        if parked {
            self.parked.insert(self.section, Parked { route, page, history, forward });
        }
        self.section = section;
        match self.parked.remove(&section) {
            Some(p) => {
                ui_assistant::set_page_context(&p.route);
                self.route = p.route;
                self.page = p.page;
                self.history = p.history;
                self.forward = p.forward;
            }
            None => {
                let home = section.home();
                self.page = self.open(&home, window, cx);
            }
        }
        self.nav_overlay = false;
        self.after_nav(window, cx);
    }

    /// Ctrl+1/2/3 and the section switcher. Asking for the section you are in from Settings
    /// goes back to it.
    fn show_section(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) {
        if section != self.section {
            self.set_section(section, window, cx);
        } else if matches!(self.route, Route::Settings { .. }) {
            self.back(window, cx);
        }
    }

    /// The sidebar destination to highlight: the page itself, or for a title, chapter or
    /// player the destination it was opened from (NN/g "you are here" cues:
    /// <https://www.nngroup.com/articles/navigation-you-are-here/>).
    fn nav_owner(&self) -> Option<Route> {
        let dest = nav_for(self.section);
        let is_dest = |r: &Route| is_workspace(r) || matches!(r, Route::Mobile) || dest.iter().any(|e| &e.route == r);
        if is_dest(&self.route) {
            return Some(self.route.clone());
        }
        if !is_detail(&self.route) {
            return None;
        }
        self.history.iter().rev().map(|e| &e.route).find(|r| is_dest(r)).cloned().or_else(|| Some(self.section.home()))
    }

    /// A route's name as people say it ("Library", "Anime home", "One Piece").
    fn label(&self, r: &Route) -> SharedString {
        if let Some(l) = nav_label(self.section, r) {
            return if l == "Home" { format!("{} home", self.section.label()).into() } else { l.into() };
        }
        match r {
            Route::MangaDetail { manga_id, .. } => self.manga_names.get(manga_id).cloned().unwrap_or_else(|| "Manga".into()),
            Route::Reader { manga_id, chapter_id } => match (self.manga_names.get(manga_id), self.chapter_names.get(chapter_id)) {
                (Some(m), Some(c)) => format!("{m} · {c}").into(),
                (Some(m), None) => m.clone(),
                (None, Some(c)) => c.clone(),
                (None, None) => "Reader".into(),
            },
            Route::AnimeDetail { title, .. } | Route::MovieDetail { title, .. } => title.clone().into(),
            Route::Player { request } => request.title.clone().into(),
            other => route_title(other).into(),
        }
    }

    /// Section › destination › title. Every level but the last links back up
    /// (NN/g breadcrumbs: <https://www.nngroup.com/articles/breadcrumbs/>).
    fn crumbs(&self) -> Vec<Crumb> {
        let sec = self.section;
        let link = |label: SharedString, to: CrumbTo| Crumb { label, to: Some(to), loading: false };
        let section_crumb = Crumb { label: sec.label().into(), to: (self.route != sec.home()).then_some(CrumbTo::Home(sec)), loading: false };
        match &self.route {
            // "Settings" is not a page of its own (the view always shows one of its pages), so it
            // is a plain label in front of the page name.
            Route::Settings { .. } => match self.settings_tab {
                Some(t) => vec![Crumb { label: "Settings".into(), to: None, loading: false }, here(t.label())],
                None => vec![here("Settings")],
            },
            Route::Studio => vec![here("Studio")],
            Route::Mobile => vec![here("Mobile")],
            Route::YouTube => vec![here("YouTube")],
            Route::Suggestions => vec![here("Suggestions")],
            Route::Reader { manga_id, chapter_id } => vec![
                section_crumb,
                link(self.manga_names.get(manga_id).cloned().unwrap_or_else(|| "Manga".into()), CrumbTo::Manga(manga_id.clone())).loading(self.looking_up.contains(manga_id)),
                here(self.chapter_names.get(chapter_id).cloned().unwrap_or_else(|| "Reader".into())).loading(self.looking_up.contains(chapter_id)),
            ],
            r => match nav_label(sec, r) {
                Some(l) => vec![section_crumb, here(l)],
                None => {
                    let owner = self.nav_owner().filter(|o| o != r);
                    let mut v = match &owner {
                        Some(Route::Studio) => vec![link("Studio".into(), CrumbTo::Up(Route::Studio))],
                        Some(Route::YouTube) => vec![link("YouTube".into(), CrumbTo::Up(Route::YouTube))],
                        Some(Route::Suggestions) => vec![link("Suggestions".into(), CrumbTo::Up(Route::Suggestions))],
                        Some(o) => vec![section_crumb, link(nav_label(sec, o).unwrap_or("Back").into(), CrumbTo::Up(o.clone()))],
                        None => vec![section_crumb],
                    };
                    let pending = matches!(r, Route::MangaDetail { manga_id, .. } if self.looking_up.contains(manga_id));
                    v.push(here(self.label(r)).loading(pending));
                    v
                }
            },
        }
    }

    fn follow_crumb(&mut self, to: CrumbTo, window: &mut Window, cx: &mut Context<Self>) {
        match to {
            CrumbTo::Home(s) if s != self.section => self.set_section(s, window, cx),
            CrumbTo::Home(s) => self.go_up(s.home(), window, cx),
            CrumbTo::Up(r) => self.go_up(r, window, cx),
            CrumbTo::Manga(id) => self.go_up_to_manga(id, window, cx),
        }
    }

    /// Looks up the manga title and chapter name for the breadcrumb, once per id.
    fn resolve_names(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (manga_id, chapter_id) = match &self.route {
            Route::MangaDetail { manga_id, .. } => (manga_id.clone(), None),
            Route::Reader { manga_id, chapter_id } => (manga_id.clone(), Some(chapter_id.clone())),
            _ => return,
        };
        let need_manga = !self.manga_names.contains_key(&manga_id) && !self.looking_up.contains(&manga_id);
        let chapter_id = chapter_id.filter(|c| !self.chapter_names.contains_key(c) && !self.looking_up.contains(c));
        if !need_manga && chapter_id.is_none() {
            return;
        }
        // The crumbs show skeletons until the names arrive (or the lookup gives up).
        let ids: Vec<String> = need_manga.then(|| manga_id.clone()).into_iter().chain(chapter_id.clone()).collect();
        self.looking_up.extend(ids.iter().cloned());
        cx.spawn_in(window, async move |this, cx| {
            let found = mv_ui::bg::blocking(move || {
                ui_manga::backend::with_db(|c| {
                    let title = if need_manga { mv_core::repo::manga::get(c, &manga_id)?.map(|m| m.title) } else { None };
                    let chapter = match chapter_id {
                        Some(id) => mv_core::repo::chapter::get(c, &id)?.map(|ch| (id, chapter_label(ch.number, ch.title.as_deref()))),
                        None => None,
                    };
                    Ok((manga_id, title, chapter))
                })
            })
            .await;
            let _ = this.update_in(cx, |s, window, cx| {
                for id in &ids {
                    s.looking_up.remove(id);
                }
                // Not found or the database failed: the crumb falls back to "Manga" / "Reader".
                if let Ok(Ok((manga_id, title, chapter))) = found {
                    if let Some(t) = title {
                        s.manga_names.insert(manga_id, t.into());
                    }
                    if let Some((id, label)) = chapter {
                        s.chapter_names.insert(id, label.into());
                    }
                }
                s.sync_window_title(window);
                cx.notify();
            });
        })
        .detach();
    }

    /// Recently opened titles for the palette (newest first, not the page you are on).
    fn recent(&self) -> Vec<(SharedString, Ph, Route)> {
        let mut out: Vec<(SharedString, Ph, Route)> = vec![];
        for e in self.history.iter().rev() {
            let icon = match &e.route {
                Route::MangaDetail { .. } => glyphs::MANGA,
                Route::Reader { .. } => ph::BOOK_OPEN_TEXT,
                Route::AnimeDetail { .. } => glyphs::ANIME,
                Route::MovieDetail { .. } => glyphs::MOVIES,
                _ => continue,
            };
            if e.route == self.route || out.iter().any(|(_, _, r)| *r == e.route) {
                continue;
            }
            out.push((self.label(&e.route), icon, e.route.clone()));
            if out.len() == 5 {
                break;
            }
        }
        out
    }

    // ── Shell state ─────────────────────────────────────────────────────────────────────────

    /// Studio mode toggled: hide the Studio entries immediately, and keep Back and Forward
    /// (here and in parked sections) from leading to the Studio page that is now hidden.
    fn mode_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !mv_ui::mode::studio_enabled(cx) {
            self.dock_open = false;
            if is_workspace(&self.route) {
                let home = self.section.home();
                self.go(home, window, cx);
            }
            let not_studio = |e: &Entry| !is_workspace(&e.route);
            self.history.retain(not_studio);
            self.forward.retain(not_studio);
            for p in self.parked.values_mut() {
                p.history.retain(not_studio);
                p.forward.retain(not_studio);
            }
            self.sync_back(cx);
        }
        cx.notify();
    }

    fn sidebar_shown(&self) -> bool {
        !is_focus(&self.route) && !matches!(self.route, Route::Settings { .. })
    }

    /// Wide window: collapse or expand (remembered). Narrow window: open the sidebar over the page.
    fn toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.sidebar_shown() {
            return;
        }
        if viewport_w(window) < RAIL_BELOW {
            self.nav_overlay = !self.nav_overlay;
        } else {
            self.collapsed = !self.collapsed;
            if self.saved_collapsed.is_none() {
                self.save_prefs();
            }
        }
        cx.notify();
    }

    fn dock_allowed(&self, cx: &App) -> bool {
        mv_ui::mode::studio_enabled(cx) && !is_focus(&self.route) && !is_workspace(&self.route)
    }

    fn toggle_dock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.dock_allowed(cx) {
            return;
        }
        self.dock_open = !self.dock_open;
        if self.dock_open && self.dock.is_none() {
            self.dock = ui_assistant::dock(window, cx);
        }
        cx.notify();
    }

    fn save_prefs(&mut self) {
        let p = ShellPrefs { collapsed: self.saved_collapsed.unwrap_or(self.collapsed), dock_w: self.dock_w.clamp(DOCK_MIN, DOCK_MAX).round() };
        if p != self.saved {
            self.saved = p;
            prefs::save(p);
        }
    }

    fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            self.close_palette(window, cx);
            return;
        }
        // The video is drawn over the app window, so the palette would open out of sight
        // and take the player's keys (ui-player binds Ctrl+K for the same reason).
        if matches!(self.route, Route::Player { .. }) {
            return;
        }
        self.nav_overlay = false;
        let prev_focus = window.focused(cx);
        let state = cx.new(|cx| CommandState::new(window, cx));
        state.update(cx, |s, cx| s.focus(window, cx));
        self.palette = Some(Palette { state, prev_focus, trap: cx.focus_handle(), query: SharedString::default(), titles: vec![] });
        // Library titles load off the UI thread; they join the list when ready.
        cx.spawn(async move |this, cx| {
            let found = mv_ui::bg::blocking(|| {
                let hidden = ui_manga::backend::browse_only();
                ui_manga::backend::with_db(|c| mv_core::repo::manga::list(c)).map(|list| {
                    list.into_iter()
                        .filter(|m| !hidden.contains(&m.id))
                        .take(500)
                        .map(|m| Title { id: m.id, source: m.source_plugin, title: m.title.into(), author: m.author.map(Into::into) })
                        .collect::<Vec<_>>()
                })
            })
            .await;
            let Ok(Ok(titles)) = found else { return };
            let _ = this.update(cx, |s, cx| {
                if let Some(p) = s.palette.as_mut() {
                    p.titles = titles;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Closes the palette and gives focus back to what had it (the reader, a field).
    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.palette.take() else { return };
        match p.prev_focus {
            Some(h) => window.focus(&h, cx),
            None => window.blur(cx),
        }
        cx.notify();
    }

    fn run(&mut self, cmd: Cmd, window: &mut Window, cx: &mut Context<Self>) {
        match cmd {
            Cmd::Open(section, route) => {
                if section != self.section {
                    self.set_section(section, window, cx);
                }
                self.go(route, window, cx);
            }
            Cmd::Go(route) => self.go(route, window, cx),
            Cmd::Run(s) => self.run_shortcut(s, window, cx),
            Cmd::SearchManga(q) => {
                ui_manga::request_search(&q);
                if self.section != Section::Manga {
                    self.set_section(Section::Manga, window, cx);
                }
                self.go(Route::Browse, window, cx);
            }
            Cmd::CheckUpdates => {
                let u = mv_ui::update::updater(cx);
                let status = u.read(cx).status.clone();
                match status {
                    // A check now would do nothing; say what is already happening instead.
                    Status::Downloading(r) => toast::show(cx, Toast::info(format!("MangaVox v{} is already downloading", r.version)).icon(glyphs::DOWNLOAD)),
                    Status::Installing(r) => toast::show(cx, Toast::info(format!("Installing MangaVox v{}. It restarts by itself.", r.version))),
                    _ => {
                        self.manual_update_check = true;
                        u.update(cx, |u, cx| u.check(true, true, cx));
                    }
                }
            }
        }
    }

    fn run_shortcut(&mut self, s: Shortcut, window: &mut Window, cx: &mut Context<Self>) {
        match s {
            Shortcut::Palette => self.toggle_palette(window, cx),
            Shortcut::Sidebar => self.toggle_sidebar(window, cx),
            Shortcut::Assistant => self.toggle_dock(window, cx),
            // Already in Settings, this keeps the page you are on (see `already_there`).
            Shortcut::Settings => self.go(Route::Settings { tab: None }, window, cx),
            Shortcut::Manga => self.show_section(Section::Manga, window, cx),
            Shortcut::Anime => self.show_section(Section::Anime, window, cx),
            Shortcut::Movies => self.show_section(Section::Movies, window, cx),
            Shortcut::Back => self.back(window, cx),
            Shortcut::Forward => self.forward(window, cx),
            Shortcut::Fullscreen => window.toggle_fullscreen(),
        }
    }

    fn on_keystroke(&mut self, ev: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        if window.window_handle() != self.window {
            return;
        }
        // Something on the page used the key: a menu, a text field, the palette list, or a
        // page that navigated (the reader's Escape).
        if ev.action.is_some() || self.nav_serial != self.serial_at_key || window.has_active_dialog(cx) {
            return;
        }
        // The welcome setup has its own keys (Enter, Esc); app shortcuts would leave it half done.
        if self.route == Route::Welcome {
            return;
        }
        let k = &ev.keystroke;
        if keys::is_escape(k) {
            // A text field on screen has the keyboard: Escape there must not throw away typing.
            let typing = ev.context_stack.iter().any(|c| c.contains("Input"));
            self.escape(typing, window, cx);
        } else if let Some(s) = keys::shortcut_for(k) {
            // The reader binds F11 itself (as a plain key, so it reaches here too and would be
            // toggled twice) and the player keeps its own full-screen state on F.
            if s == Shortcut::Fullscreen && is_focus(&self.route) {
                return;
            }
            // While the palette is open only its own toggle and Escape apply.
            if self.palette.is_none() || s == Shortcut::Palette {
                self.run_shortcut(s, window, cx);
            }
        }
    }

    /// Escape closes the topmost floating layer, then leaves Settings.
    fn escape(&mut self, typing: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            self.close_palette(window, cx);
        } else if self.nav_overlay {
            self.nav_overlay = false;
            cx.notify();
        } else if self.dock_open && self.dock_allowed(cx) && dock_floats(viewport_w(window), self.side_w(window)) {
            self.dock_open = false;
            cx.notify();
        } else if matches!(self.route, Route::Settings { .. }) {
            if typing {
                // In a field, Escape lets go of it first (what was typed stays, and fields save
                // on blur); the next Escape leaves Settings.
                window.blur(cx);
                cx.notify();
            } else {
                // Focus left on a page no longer shown, or on a switch reached with Tab, does
                // not block the way out.
                self.back(window, cx);
            }
        }
    }

    fn update_changed(&mut self, u: Entity<Updater>, cx: &mut Context<Self>) {
        if self.manual_update_check {
            let status = u.read(cx).status.clone();
            self.manual_update_check = matches!(status, Status::Checking);
            match status {
                Status::Checking => {}
                Status::UpToDate => toast::show(cx, Toast::success(format!("MangaVox v{} is up to date", version()))),
                Status::Failed(e) => toast::show(cx, Toast::error(format!("Could not check for updates. {e}"))),
                Status::Available(r) => {
                    let v = r.version.clone();
                    toast::show(cx, Toast::info(format!("MangaVox v{v} is available")).icon(ph::ARROW_CIRCLE_UP).action("Update", move |w, cx| updates::confirm_install(&v, w, cx)));
                }
                _ => {}
            }
        }
        cx.notify();
    }

    fn rail(&self, window: &Window) -> bool {
        self.collapsed || viewport_w(window) < RAIL_BELOW
    }

    /// What Ctrl+B does right now, for the title-bar tooltip and the palette row (a narrow
    /// window opens the sidebar over the page instead of expanding it).
    fn sidebar_toggle_label(&self, window: &Window) -> &'static str {
        match (viewport_w(window) < RAIL_BELOW, self.nav_overlay, self.collapsed) {
            (true, true, _) => "Hide navigation",
            (true, false, _) => "Show navigation",
            (false, _, true) => "Expand sidebar",
            (false, _, false) => "Collapse sidebar",
        }
    }

    fn side_w(&self, window: &Window) -> f32 {
        match (self.sidebar_shown(), self.rail(window)) {
            (false, _) => 0.,
            (true, true) => RAIL_W,
            (true, false) => SIDEBAR_W,
        }
    }

    // ── Title bar ───────────────────────────────────────────────────────────────────────────

    fn title_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vw = viewport_w(window);
        let narrow = vw < RAIL_BELOW;
        let in_settings = matches!(self.route, Route::Settings { .. });
        let welcome = self.route == Route::Welcome;
        let back = self.back_route();
        let back_label = back.as_ref().map(|r| self.label(r));

        let toggle = self.sidebar_shown().then(|| {
            let open = if narrow { self.nav_overlay } else { !self.collapsed };
            // Filled panel while the sidebar is open, an outline while it is a rail.
            icon_button("toggle-sidebar", ph::SIDEBAR_SIMPLE.on(open), self.sidebar_toggle_label(window))
                .kbd(Shortcut::Sidebar.keys())
                .occlude()
                .on_click(cx.listener(|this, _, w, cx| this.toggle_sidebar(w, cx)))
        });

        // Settings has no sidebar, so its way out is spelled out.
        let back_el = if in_settings {
            btn("nav-back", format!("Back to {}", back_label.clone().unwrap_or_else(|| self.section.label().into())), ButtonKind::Ghost)
                .small()
                .icon(glyphs::BACK)
                .kbd("escape")
                .occlude()
                .on_click(cx.listener(|this, _, w, cx| this.back(w, cx)))
                .into_any_element()
        } else {
            icon_button("nav-back", glyphs::BACK, back_label.as_ref().map(|l| format!("Back to {l}")).unwrap_or_else(|| "Back".into()))
                .kbd(Shortcut::Back.keys())
                .when(back.is_none(), |b| b.disabled("Nothing to go back to"))
                .occlude()
                .on_click(cx.listener(|this, _, w, cx| this.back(w, cx)))
                .into_any_element()
        };
        let forward_el = (!in_settings).then(|| {
            let next = self.forward.last().map(|e| self.label(&e.route));
            icon_button("nav-forward", ph::ARROW_RIGHT, next.as_ref().map(|l| format!("Forward to {l}")).unwrap_or_else(|| "Forward".into()))
                .kbd(Shortcut::Forward.keys())
                .when(next.is_none(), |b| b.disabled("Nothing to go forward to"))
                .occlude()
                .on_click(cx.listener(|this, _, w, cx| this.forward(w, cx)))
        });

        // No palette over the player (see `toggle_palette`).
        let search = if matches!(self.route, Route::Player { .. } | Route::Welcome) {
            div().flex_none().into_any_element()
        } else if vw >= SEARCH_PILL_FROM {
            div()
                .id("search")
                .flex()
                .flex_none()
                .items_center()
                .gap(px(8.))
                .w(relative(0.32))
                .min_w(px(220.))
                .max_w(px(360.))
                .h(px(28.))
                .pl(px(10.))
                .pr(px(4.))
                .rounded(px(radius::MD))
                .bg(Tokens::well())
                .border_1()
                .border_color(Tokens::border())
                .text_size(px(size::SM))
                .text_color(Tokens::text_3())
                .role(Role::Button)
                .aria_label("Search or jump to")
                .group("search-field")
                .hover(|s| s.border_color(Tokens::border_strong()).text_color(Tokens::text_2()))
                .pressable()
                .occlude()
                // The icon brightens with the text (Material 3: a state covers icon and label).
                .child(icon_ph(glyphs::SEARCH, Tokens::text_3(), icon_px::SM).group_hover("search-field", |s| s.text_color(Tokens::text_2())))
                .child(div().flex_1().min_w_0().truncate().child("Search or jump to…"))
                .child(kbd(Shortcut::Palette.keys()))
                .on_click(cx.listener(|this, _, w, cx| this.toggle_palette(w, cx)))
                .into_any_element()
        } else {
            icon_button("search", glyphs::SEARCH, "Search or jump to").kbd(Shortcut::Palette.keys()).occlude().on_click(cx.listener(|this, _, w, cx| this.toggle_palette(w, cx))).into_any_element()
        };

        let assistant = (mv_ui::mode::studio_enabled(cx) && !is_focus(&self.route)).then(|| {
            let allowed = self.dock_allowed(cx);
            icon_button("assistant", glyphs::ASSISTANT, if self.dock_open { "Close assistant" } else { "Open assistant" })
                .selected(self.dock_open && allowed)
                .kbd(Shortcut::Assistant.keys())
                .when(!allowed, |b| b.disabled("This page has its own assistant panel"))
                .occlude()
                .on_click(cx.listener(|this, _, w, cx| this.toggle_dock(w, cx)))
        });

        TitleBar::new().h(px(TITLE_BAR_H)).bg(Tokens::base()).border_b_1().border_color(Tokens::border()).child(
            div()
                .flex()
                .items_center()
                .w_full()
                .h_full()
                .gap(px(8.))
                .pr(px(8.))
                // Windows: title bar content is dimmed while the window is inactive.
                .when(!window.is_window_active(), |d| d.opacity(0.6))
                // The welcome setup shows only its name: it has its own Back and Skip.
                .child(div().flex().flex_1().min_w_0().items_center().gap(px(2.)).children(toggle).when(!welcome, |d| d.child(back_el).children(forward_el)).child(self.crumbs_el(cx)))
                .child(search)
                .child(div().flex().flex_1().min_w_0().justify_end().items_center().gap(px(4.)).children(assistant)),
        )
    }

    fn crumbs_el(&self, cx: &mut Context<Self>) -> Div {
        let crumbs = self.crumbs();
        let n = crumbs.len();
        let mut row = div().flex().items_center().gap(px(2.)).ml(px(6.)).min_w_0().overflow_hidden().text_size(px(size::MD));
        for (i, c) in crumbs.into_iter().enumerate() {
            if i > 0 {
                row = row.child(icon_ph(ph::CARET_RIGHT, Tokens::text_3(), icon_px::XS));
            }
            // A name still loading is a text-shaped skeleton, never a wrong generic word
            // (NN/g skeleton screens: <https://www.nngroup.com/articles/skeleton-screens/>).
            if c.loading {
                row = row.child(div().flex_none().px(px(6.)).child(skeleton_line(px(if i + 1 == n { 140. } else { 96. }))));
                continue;
            }
            let label = c.label;
            let chars = label.chars().count();
            let el = if i + 1 == n {
                // The title bar is a window drag area; without `occlude` the pointer never
                // reaches the crumb (Windows treats it as caption), so the tooltip could not show.
                // Short names stay part of the drag area. At 1280px about 28 characters fit
                // beside the search field, so longer names get the tooltip.
                div()
                    .id(("crumb", i))
                    .min_w_0()
                    .px(px(4.))
                    .truncate()
                    .text_color(Tokens::text_1())
                    .font_weight(FontWeight::MEDIUM)
                    .when(chars > 28, |d| d.tooltip(tip(label.clone())).occlude())
                    .child(label)
                    .into_any_element()
            } else if let Some(to) = c.to {
                div()
                    .id(("crumb", i))
                    .flex_none()
                    .max_w(px(180.))
                    .h(px(26.))
                    .line_height(px(26.))
                    .px(px(6.))
                    .truncate()
                    .rounded(px(radius::SM))
                    .text_color(Tokens::text_3())
                    .role(Role::Link)
                    .hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
                    .pressable()
                    .occlude()
                    // 180px holds about 24 characters; a cut-off title is named in full.
                    .when(chars > 24, |d| d.tooltip(tip(label.clone())))
                    .on_click(cx.listener(move |this, _, w, cx| this.follow_crumb(to.clone(), w, cx)))
                    .child(label)
                    .into_any_element()
            } else {
                div().flex_none().px(px(6.)).text_color(Tokens::text_3()).child(label).into_any_element()
            };
            row = row.child(el);
        }
        row
    }

    // ── Sidebar ─────────────────────────────────────────────────────────────────────────────

    fn sidebar(&self, rail: bool, floating: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let owner = self.nav_owner();
        let downloads = ui_manga::downloads(cx).read(cx).pending().len();

        let brand = div()
            .id("brand")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .h(px(36.))
            .rounded(px(radius::MD))
            .map(|d| if rail { d.w(px(40.)).justify_center() } else { d.w_full().px(px(10.)) })
            .role(Role::Link)
            .aria_label(format!("MangaVox, go to {}", self.label(&self.section.home())))
            .interactive()
            .tooltip(tip(format!("Go to {}", self.label(&self.section.home()))))
            .on_click(cx.listener(|this, _, w, cx| {
                let home = this.section.home();
                this.go_up(home, w, cx);
            }))
            .child(svg().path("brand/mangavox.svg").size(px(icon_px::XL)).flex_none().text_color(Tokens::accent()))
            .when(!rail, |d| d.child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("MangaVox")));

        let switcher = if rail {
            div()
                .flex()
                .flex_col()
                .flex_none()
                .items_center()
                .gap(px(2.))
                .p(px(2.))
                .rounded(px(radius::LG))
                .bg(Tokens::well())
                .border_1()
                .border_color(Tokens::border())
                .children(SECTIONS.map(|s| {
                    icon_button(("section", s.index()), s.icon(), s.label())
                        .selected(s == self.section)
                        .kbd(s.shortcut().keys())
                        .on_click(cx.listener(move |this, _, w, cx| this.show_section(s, w, cx)))
                }))
                .into_any_element()
        } else {
            segmented("section", SECTIONS.map(|s| (Some(s.icon()), s.label())), self.section.index())
                .small()
                .full_width()
                .on_change(cx.listener(|this, ix: &usize, w, cx| this.show_section(SECTIONS[*ix], w, cx)))
                .into_any_element()
        };

        let items = nav_for(self.section).into_iter().enumerate().map(|(i, e)| {
            let active = owner.as_ref() == Some(&e.route);
            let badge = if e.route == Route::MangaDownloads { downloads } else { 0 };
            let route = e.route.clone();
            nav_item(("nav", i), e.icon, e.label, active, rail).badge(badge).on_click(cx.listener(move |this, _, w, cx| this.go(route.clone(), w, cx)))
        });

        let studio = mv_ui::mode::studio_enabled(cx).then(|| {
            let active = owner.as_ref() == Some(&Route::Studio);
            let yt_active = owner.as_ref() == Some(&Route::YouTube);
            let sg_active = owner.as_ref() == Some(&Route::Suggestions);
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .map(|d| if rail { d.pt(px(10.)).border_t_1().border_color(Tokens::border()) } else { d.child(section_label("Studio").px(px(10.))) })
                .child(nav_item("nav-studio", glyphs::STUDIO, "Studio", active, rail).on_click(cx.listener(|this, _, w, cx| this.go(Route::Studio, w, cx))))
                .child(nav_item("nav-youtube", glyphs::YOUTUBE, "YouTube", yt_active, rail).on_click(cx.listener(|this, _, w, cx| this.go(Route::YouTube, w, cx))))
                .child(nav_item("nav-suggestions", ph::LIGHTBULB, "Suggestions", sg_active, rail).on_click(cx.listener(|this, _, w, cx| this.go(Route::Suggestions, w, cx))))
        });

        let settings = nav_item("nav-settings", glyphs::SETTINGS, "Settings", matches!(self.route, Route::Settings { .. }), rail)
            .kbd(Shortcut::Settings.keys())
            .on_click(cx.listener(|this, _, w, cx| this.go(Route::Settings { tab: None }, w, cx)));
        // In every section: the phone companion is app-wide, like Settings.
        let mobile = nav_item("nav-mobile", ph::DEVICE_MOBILE, "Mobile", owner.as_ref() == Some(&Route::Mobile), rail)
            .on_click(cx.listener(|this, _, w, cx| this.go(Route::Mobile, w, cx)));

        // Distinct ids: the rail and the floating copy can be on screen together.
        div()
            .id(if floating { "sidebar-floating" } else { "sidebar" })
            .flex()
            .flex_col()
            .flex_none()
            .w(px(if rail { RAIL_W } else { SIDEBAR_W }))
            .h_full()
            .gap(px(12.))
            .pt(px(10.))
            .pb(px(10.))
            .px(px(8.))
            .bg(Tokens::sidebar())
            .border_r_1()
            .border_color(Tokens::border())
            .when(rail, |d| d.items_center())
            .when(floating, |d| d.shadow_2xl().occlude())
            .child(brand)
            .child(div().w_full().px(px(2.)).flex().justify_center().child(switcher))
            .child(
                // Scrolls when the window is short; the 2px inset keeps focus rings visible.
                div()
                    .id("nav-list")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .gap(px(14.))
                    .px(px(2.))
                    .py(px(2.))
                    .overflow_y_scroll()
                    .when(rail, |d| d.items_center())
                    .child(div().flex().flex_col().gap(px(2.)).w_full().when(rail, |d| d.items_center()).children(items))
                    .children(studio),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .gap(px(8.))
                    .px(px(2.))
                    .when(rail, |d| d.items_center())
                    .children(updates::footer(rail, cx))
                    .child(div().w_full().pt(px(8.)).border_t_1().border_color(Tokens::border()).flex().flex_col().gap(px(2.)).when(rail, |d| d.items_center()).child(mobile).child(settings)),
            )
    }

    // ── Overlays ────────────────────────────────────────────────────────────────────────────

    fn palette_overlay(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.palette.as_ref()?;
        let ctx = palette::Ctx {
            section: self.section,
            route: &self.route,
            studio: mv_ui::mode::studio_enabled(cx),
            can_dock: self.dock_allowed(cx),
            settings_tab: self.settings_tab.filter(|_| matches!(self.route, Route::Settings { .. })).map(|t| t.id()),
            can_back: self.back_route().is_some(),
            can_forward: !self.forward.is_empty(),
            sidebar: self.sidebar_shown().then(|| self.sidebar_toggle_label(window)),
            dock_open: self.dock_open,
            fullscreen: window.is_fullscreen(),
            recent: self.recent(),
        };
        let shell = cx.entity().downgrade();
        let (on_cmd, on_query, on_cancel) = (shell.clone(), shell.clone(), shell);
        let command = palette::command(
            &ctx,
            p,
            move |cmd, w, cx| {
                let _ = on_cmd.update(cx, |s, cx| {
                    s.close_palette(w, cx);
                    s.run(cmd, w, cx);
                });
            },
            move |q, _, cx| {
                let q = SharedString::from(q.to_string());
                let _ = on_query.update(cx, |s, cx| {
                    if let Some(p) = s.palette.as_mut() {
                        p.query = q;
                        cx.notify();
                    }
                });
            },
            move |w, cx| {
                // Escape on an empty query: close after the palette's own handling finishes.
                let shell = on_cancel.clone();
                w.defer(cx, move |w, cx| {
                    let _ = shell.update(cx, |s, cx| s.close_palette(w, cx));
                });
            },
        );
        let panel = div()
            .id("palette")
            .mt(px(72.))
            .w(px(620.))
            .max_w(relative(0.92))
            .flex()
            .flex_col()
            .rounded(px(radius::LG))
            .bg(Tokens::popover())
            .border_1()
            .border_color(Tokens::border_strong())
            .shadow_2xl()
            .overflow_hidden()
            .occlude()
            // Also keeps a click on the panel's empty parts from moving focus to the trap
            // container (its focus listener runs after this one), so typing goes on.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(command)
            .focus_trap("palette-trap", &p.trap);
        Some(
            div()
                .id("palette-scrim")
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .bg(Tokens::overlay())
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, w, cx| this.close_palette(w, cx)))
                .child(appear("palette-in", panel))
                .into_any_element(),
        )
    }

    fn scrim(&self, id: &'static str, close: impl Fn(&mut Self, &mut Context<Self>) + 'static, cx: &mut Context<Self>) -> Stateful<Div> {
        div().id(id).absolute().inset_0().bg(Tokens::overlay()).occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                close(this, cx);
                cx.notify();
            }),
        )
    }

    /// The docked assistant: trailing edge, drag its left edge to resize (320 to 560 px).
    fn dock_column(&self, dock: AnyView, w: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("dock")
            .relative()
            .flex_none()
            .w(px(w))
            .h_full()
            .border_l_1()
            .border_color(Tokens::border())
            .child(dock)
            .child(
                div()
                    .id("dock-resize")
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(-3.))
                    .w(px(6.))
                    .cursor_col_resize()
                    .group("dock-edge")
                    // The accent line shows on hover and stays lit while dragging (the pointer
                    // soon leaves the 6px edge).
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(px(2.))
                            .w(px(2.))
                            .when(self.resizing, |d| d.bg(Tokens::accent_a(0.6)))
                            .group_hover("dock-edge", |s| s.bg(Tokens::accent_a(0.6))),
                    )
                    .on_drag(DockResize, |_, _, _, cx| cx.new(|_| DragGhost))
                    .on_drag_move(cx.listener(|this, ev: &DragMoveEvent<DockResize>, w, cx| {
                        let x = f32::from(ev.event.position.x);
                        this.dock_w = (viewport_w(w) - x).clamp(DOCK_MIN, DOCK_MAX);
                        this.resizing = true;
                        cx.notify();
                    })),
            )
    }
}

fn push_capped(stack: &mut Vec<Entry>, e: Entry) {
    stack.push(e);
    if stack.len() > HISTORY_MAX {
        stack.remove(0);
    }
}

fn viewport_w(window: &Window) -> f32 {
    f32::from(window.viewport_size().width)
}

/// The assistant floats over the page when docking it would squeeze the page below
/// [`PAGE_MIN`] or the window is narrow.
fn dock_floats(vw: f32, side_w: f32) -> bool {
    vw < DOCK_OVERLAY_BELOW || vw - side_w - DOCK_MIN < PAGE_MIN
}

/// "Chapter 12", "Chapter 10.5", or the chapter's own title when it has no number.
fn chapter_label(number: Option<f64>, title: Option<&str>) -> String {
    match number {
        Some(n) if n.fract() == 0. => format!("Chapter {}", n as i64),
        Some(n) => format!("Chapter {n}"),
        None => title.map(str::trim).filter(|t| !t.is_empty()).unwrap_or("Chapter").to_string(),
    }
}

/// A route's name without context (placeholder pages, other sections).
fn route_title(route: &Route) -> &'static str {
    if let Some(l) = SECTIONS.iter().find_map(|s| nav_label(*s, route)) {
        return l;
    }
    match route {
        Route::MangaDetail { .. } => "Manga",
        Route::Reader { .. } => "Reader",
        Route::AnimeDetail { .. } => "Anime",
        Route::MovieDetail { .. } => "Movie",
        Route::Player { .. } => "Player",
        Route::Studio => "Studio",
        Route::Mobile => "Mobile",
        Route::YouTube => "YouTube",
        Route::Suggestions => "Suggestions",
        Route::Settings { .. } => "Settings",
        Route::Welcome => "Welcome",
        _ => "MangaVox",
    }
}

/// Segoe UI Variable (the Windows 11 UI font, optical sizes for small text) when installed,
/// Segoe UI otherwise. Fluent 2 typography: <https://fluent2.microsoft.design/typography>
fn ui_font(cx: &App) -> SharedString {
    const PREFERRED: &str = "Segoe UI Variable Text";
    if cx.text_system().all_font_names().iter().any(|n| n == PREFERRED) {
        PREFERRED.into()
    } else {
        "Segoe UI".into()
    }
}

struct Placeholder {
    title: SharedString,
}

impl Render for Placeholder {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().flex().items_center().justify_center().child(
            empty_view(ph::TRAFFIC_CONE, self.title.clone(), "This page is not ready yet. Everything else in MangaVox works as usual.")
                .action(btn("placeholder-back", "Go back", ButtonKind::Secondary).icon(glyphs::BACK).on_click(|_, w, cx| mv_ui::navigate(Route::Back, w, cx))),
        )
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::perf::first_frame();
        let vw = viewport_w(window);
        if vw >= RAIL_BELOW {
            self.nav_overlay = false;
        }
        let rail = self.rail(window);
        let focus = is_focus(&self.route);
        let show_sidebar = self.sidebar_shown();
        let side_w = self.side_w(window);
        let dock = self.dock.clone().filter(|_| self.dock_open && self.dock_allowed(cx));
        let floating = dock_floats(vw, side_w);
        let dock_w = if floating { self.dock_w.min(vw * 0.92) } else { self.dock_w.min(vw - side_w - PAGE_MIN).max(DOCK_MIN) };
        // A finished resize is saved once the drag ends.
        if !cx.has_active_drag() {
            self.resizing = false;
            self.save_prefs();
        }

        let page = div()
            .id("page")
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .child(self.page.clone())
            .when(!focus, |d| d.children(ui_manga::download_toast(cx)))
            // App-wide toasts (mv_ui::toast::show), bottom centre of the page area. Mounted once.
            .child(mv_ui::toast::host(cx));

        let mut body = div().relative().flex().flex_1().min_h_0().when(show_sidebar, |d| d.child(self.sidebar(rail, false, cx))).child(page);
        match dock {
            Some(dock) if floating => {
                body = body.child(self.scrim("dock-scrim", |this, _| this.dock_open = false, cx)).child(
                    appear(
                        "dock-in",
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .h_full()
                            .w(px(dock_w))
                            .bg(Tokens::base())
                            .border_l_1()
                            .border_color(Tokens::border())
                            .shadow_2xl()
                            .occlude()
                            .child(dock),
                    ),
                );
            }
            Some(dock) => body = body.child(self.dock_column(dock, dock_w, cx)),
            None => {}
        }
        if self.nav_overlay && show_sidebar {
            body = body
                .child(self.scrim("nav-scrim", |this, _| this.nav_overlay = false, cx))
                .child(appear("nav-in", div().absolute().top_0().left_0().h_full().child(self.sidebar(false, true, cx))));
        }

        let palette = self.palette_overlay(window, cx);
        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(Tokens::base())
            .text_color(Tokens::text_1())
            .font_family(self.font.clone())
            // The mouse's back and forward side buttons work everywhere.
            .on_mouse_down(MouseButton::Navigate(NavigationDirection::Back), cx.listener(|this, _, w, cx| this.back(w, cx)))
            .on_mouse_down(MouseButton::Navigate(NavigationDirection::Forward), cx.listener(|this, _, w, cx| this.forward(w, cx)))
            .child(self.title_bar(window, cx))
            .child(body)
            .children(palette)
            // While the assistant edge is dragged the resize cursor stays on wherever the mouse goes.
            .when(self.resizing, |d| d.child(div().id("resize-cursor").absolute().inset_0().cursor_col_resize().occlude()))
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in gpui's `test` macro, which shadows the built-in one.
    use super::{chapter_label, dock_floats, route_title, RAIL_W, SIDEBAR_W};
    use crate::routes::{nav_for, SECTIONS};
    use mv_ui::Route;

    #[test]
    fn chapter_labels_read_naturally() {
        assert_eq!(chapter_label(Some(12.), None), "Chapter 12");
        assert_eq!(chapter_label(Some(10.5), Some("Side story")), "Chapter 10.5");
        assert_eq!(chapter_label(None, Some("  Oneshot ")), "Oneshot");
        assert_eq!(chapter_label(None, Some(" ")), "Chapter");
    }

    #[test]
    fn dock_floats_when_the_page_would_get_too_narrow() {
        assert!(dock_floats(880., RAIL_W));
        assert!(!dock_floats(1280., SIDEBAR_W));
        // 1024 wide with the full sidebar leaves 448px next to a 320px dock: float it.
        assert!(dock_floats(1024., SIDEBAR_W));
        assert!(!dock_floats(1024., RAIL_W));
    }

    #[test]
    fn route_titles_cover_every_destination() {
        for s in SECTIONS {
            for e in nav_for(s) {
                assert_eq!(route_title(&e.route), e.label);
            }
        }
        assert_eq!(route_title(&Route::Studio), "Studio");
        assert_eq!(route_title(&Route::Mobile), "Mobile");
        assert_eq!(route_title(&Route::YouTube), "YouTube");
    }
}
