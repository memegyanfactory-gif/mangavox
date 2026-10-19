//! App shell: slim title bar + collapsible left sidebar + optional assistant dock + page.
use gpui_kit::component::TitleBar;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::nav::{set_back_target, set_navigator};
use mv_ui::{empty_state, icon, icon_button, size, Route, Tokens};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Manga,
    Anime,
    Movies,
}

impl Section {
    fn label(self) -> &'static str {
        match self {
            Section::Manga => "Manga",
            Section::Anime => "Anime",
            Section::Movies => "Movies",
        }
    }
    fn home(self) -> Route {
        match self {
            Section::Manga => Route::Library,
            Section::Anime => Route::AnimeHome,
            Section::Movies => Route::MoviesHome,
        }
    }
}

struct NavEntry {
    route: Route,
    label: &'static str,
    icon: &'static str,
}

fn nav_for(section: Section) -> Vec<NavEntry> {
    match section {
        Section::Manga => vec![
            NavEntry { route: Route::Browse, label: "Browse", icon: "icons/compass.svg" },
            NavEntry { route: Route::Library, label: "Library", icon: "icons/book-open.svg" },
            NavEntry { route: Route::MangaHistory, label: "History", icon: "icons/clock.svg" },
            NavEntry { route: Route::MangaDownloads, label: "Downloads", icon: "icons/download.svg" },
        ],
        Section::Anime => vec![
            NavEntry { route: Route::AnimeHome, label: "Home", icon: "icons/monitor-play.svg" },
            NavEntry { route: Route::Discover, label: "Discover", icon: "icons/shuffle.svg" },
            NavEntry { route: Route::MediaHistory, label: "History", icon: "icons/clock.svg" },
            NavEntry { route: Route::Favorites, label: "Favorites", icon: "icons/heart.svg" },
        ],
        Section::Movies => vec![
            NavEntry { route: Route::MoviesHome, label: "Home", icon: "icons/film.svg" },
            NavEntry { route: Route::Discover, label: "Discover", icon: "icons/shuffle.svg" },
            NavEntry { route: Route::MediaHistory, label: "History", icon: "icons/clock.svg" },
            NavEntry { route: Route::Favorites, label: "Favorites", icon: "icons/heart.svg" },
        ],
    }
}

fn title_for(route: &Route) -> SharedString {
    match route {
        Route::Browse => "Browse".into(),
        Route::Library => "Library".into(),
        Route::MangaHistory | Route::MediaHistory => "History".into(),
        Route::MangaDownloads => "Downloads".into(),
        Route::MangaDetail { .. } => "Manga".into(),
        Route::Reader { .. } => "Reader".into(),
        Route::AnimeHome | Route::MoviesHome => "Home".into(),
        Route::AnimeDetail { title, .. } | Route::MovieDetail { title, .. } => title.clone().into(),
        Route::Discover => "Discover".into(),
        Route::Favorites => "Favorites".into(),
        Route::Player { request } => request.title.clone().into(),
        Route::Studio => "Studio".into(),
        Route::Settings { .. } => "Settings".into(),
        Route::Back => "".into(),
    }
}

/// Full-screen routes hide the sidebar and dock.
fn is_focus(route: &Route) -> bool {
    matches!(route, Route::Reader { .. } | Route::Player { .. })
}

/// Pages that stay alive once opened, so the sidebar brings back the same scroll position,
/// source, filters and loaded results instead of starting over.
fn keep_alive(route: &Route) -> bool {
    matches!(route, Route::Browse | Route::AnimeHome | Route::MoviesHome | Route::Discover)
}

/// A history entry. The view is kept so Back restores the page exactly as it was;
/// the player is never kept (it would keep playing in the background).
struct Entry {
    route: Route,
    view: Option<AnyView>,
}

/// One section's navigation state, parked while another section is shown.
struct Parked {
    route: Route,
    page: AnyView,
    history: Vec<Entry>,
}

pub struct Shell {
    collapsed: bool,
    section: Section,
    route: Route,
    history: Vec<Entry>,
    page: AnyView,
    kept: std::collections::HashMap<String, AnyView>,
    parked: std::collections::HashMap<Section, Parked>,
    dock: Option<AnyView>,
    dock_open: bool,
    /// Sidebar state to restore after leaving the (wide) Studio page.
    saved_collapsed: Option<bool>,
    _keys: Subscription,
    _mode: Subscription,
    _update: Subscription,
    _downloads: Subscription,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        mv_ui::mode::init(cx, ui_settings::studio_saved());
        let mode = cx.observe_global_in::<mv_ui::mode::StudioMode>(window, |this, window, cx| this.mode_changed(window, cx));
        let shell = cx.entity().downgrade();
        set_navigator(cx, move |route, window, cx| {
            if let Some(s) = shell.upgrade() {
                s.update(cx, |s, cx| s.go(route, window, cx));
            }
        });
        // Alt+Left goes back from anywhere (Escape is handled by the reader / player themselves).
        let keys = cx.observe_keystrokes(|this, ev, window, cx| {
            let k = &ev.keystroke;
            if k.modifiers.alt && !k.modifiers.control && k.key == "left" {
                this.back(window, cx);
            }
        });
        let updater = mv_ui::update::updater(cx);
        let update = cx.observe(&updater, |_, _, cx| cx.notify());
        // The download notification floats over every page.
        let dl = ui_manga::downloads(cx);
        let downloads = cx.observe(&dl, |_, _, cx| cx.notify());
        let route = Route::Library;
        let page = Self::resolve(&route, window, cx);
        Self {
            collapsed: false,
            section: Section::Manga,
            route,
            history: vec![],
            page,
            kept: Default::default(),
            parked: Default::default(),
            dock: None,
            dock_open: false,
            saved_collapsed: None,
            _keys: keys,
            _mode: mode,
            _update: update,
            _downloads: downloads,
        }
    }

    /// Open `route`, reusing the live page for keep-alive routes.
    fn open(&mut self, route: &Route, window: &mut Window, cx: &mut App) -> AnyView {
        if !keep_alive(route) {
            return Self::resolve(route, window, cx);
        }
        let key = format!("{route:?}");
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
            .unwrap_or_else(|| {
                let title = title_for(route);
                cx.new(|_| Placeholder { title }).into()
            })
    }

    fn go(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) {
        if route == Route::Back {
            self.back(window, cx);
            return;
        }
        if route == self.route {
            return;
        }
        let page = self.open(&route, window, cx);
        let prev = std::mem::replace(&mut self.route, route);
        let prev_view = std::mem::replace(&mut self.page, page);
        let view = (!matches!(prev, Route::Player { .. })).then_some(prev_view);
        self.history.push(Entry { route: prev, view });
        if self.history.len() > 50 {
            self.history.remove(0);
        }
        self.sync_back(cx);
        cx.notify();
    }

    /// Back leads to the previous page, or to the section's home page when there is none.
    fn back_route(&self) -> Option<Route> {
        match self.history.last() {
            Some(e) => Some(e.route.clone()),
            None => (self.route != self.section.home() && !is_focus(&self.route)).then(|| self.section.home()),
        }
    }

    /// Called after every route change: keeps the back target current and gives the wide
    /// Studio page the 60px rail, restoring the user's own sidebar state afterwards.
    fn sync_back(&mut self, cx: &mut App) {
        set_back_target(cx, self.back_route());
        if self.route == Route::Studio {
            if self.saved_collapsed.is_none() {
                self.saved_collapsed = Some(self.collapsed);
                self.collapsed = true;
            }
        } else if let Some(prev) = self.saved_collapsed.take() {
            self.collapsed = prev;
        }
    }

    /// Studio mode toggled: hide the Studio entries immediately.
    fn mode_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !mv_ui::mode::studio_enabled(cx) {
            self.dock_open = false;
            if self.route == Route::Studio {
                let home = self.section.home();
                self.go(home, window, cx);
            }
        }
        cx.notify();
    }

    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.route, Route::Player { .. }) && window.is_fullscreen() {
            window.toggle_fullscreen();
        }
        if let Some(prev) = self.history.pop() {
            self.page = match prev.view {
                Some(v) => {
                    ui_assistant::set_page_context(&prev.route);
                    v
                }
                None => self.open(&prev.route, window, cx),
            };
            self.route = prev.route;
        } else if let Some(home) = self.back_route() {
            // Nothing to go back to (e.g. opened right after a section switch): land on the section home.
            self.page = self.open(&home, window, cx);
            self.route = home;
        } else {
            return;
        }
        self.sync_back(cx);
        cx.notify();
    }

    /// Switching sections parks the current one (page + history) and restores the target's.
    fn set_section(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) {
        if section == self.section {
            return;
        }
        let route = std::mem::replace(&mut self.route, section.home());
        let history = std::mem::take(&mut self.history);
        if !matches!(route, Route::Player { .. } | Route::Settings { .. } | Route::Studio) {
            self.parked.insert(self.section, Parked { route, page: self.page.clone(), history });
        }
        self.section = section;
        match self.parked.remove(&section) {
            Some(p) => {
                ui_assistant::set_page_context(&p.route);
                self.route = p.route;
                self.page = p.page;
                self.history = p.history;
            }
            None => {
                let home = section.home();
                self.page = self.open(&home, window, cx);
            }
        }
        self.sync_back(cx);
        cx.notify();
    }

    fn toggle_dock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dock_open = !self.dock_open;
        if self.dock_open && self.dock.is_none() {
            self.dock = ui_assistant::dock(window, cx);
        }
        cx.notify();
    }

    fn title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let title = title_for(&self.route);
        let in_section = !matches!(self.route, Route::Settings { .. } | Route::Player { .. } | Route::Reader { .. } | Route::Studio);
        let show_section = in_section && title.as_ref() != self.section.label() && title.as_ref() != "Home";
        let shown: SharedString = if title.as_ref() == "Home" { self.section.label().into() } else { title };
        TitleBar::new().bg(Tokens::base()).border_b_1().border_color(Tokens::border()).child(
            div()
                .flex()
                .items_center()
                .gap_1()
                // `occlude` keeps the title bar's drag area from swallowing these clicks.
                .child(icon_button("toggle-sidebar", "icons/panel-left.svg", true).occlude().on_click(cx.listener(|this, _, _, cx| {
                    this.collapsed = !this.collapsed;
                    cx.notify();
                })))
                .child(icon_button("back", "icons/arrow-left.svg", self.back_route().is_some()).occlude().on_click(cx.listener(|this, _, w, cx| this.back(w, cx))))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .ml_2()
                        .text_size(px(size::MD))
                        .when(show_section, |d| {
                            d.child(div().text_color(Tokens::text_3()).child(self.section.label()))
                                .child(icon("icons/chevron-right.svg", Tokens::text_3(), 12.))
                        })
                        .child(div().text_color(Tokens::text_1()).font_weight(FontWeight::MEDIUM).child(shown)),
                ),
        )
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let collapsed = self.collapsed;
        let segment = div()
            .flex()
            .when(collapsed, |d| d.flex_col())
            .gap(px(2.))
            .p(px(3.))
            .rounded(px(10.))
            .bg(rgba(0x00000038))
            .border_1()
            .border_color(Tokens::border())
            .children([(Section::Manga, "icons/book-open.svg"), (Section::Anime, "icons/monitor-play.svg"), (Section::Movies, "icons/film.svg")].map(|(sec, ic)| {
                let active = self.section == sec;
                let fg = if active { Tokens::text_1() } else { Tokens::text_3() };
                div()
                    .id(sec.label())
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .h(px(28.))
                    .when(!collapsed, |d| d.px(px(6.)))
                    .rounded(px(7.))
                    .text_size(px(12.))
                    .text_color(fg)
                    .cursor_pointer()
                    .when(active, |d| d.bg(Tokens::active()))
                    .hover(|s| s.text_color(Tokens::text_1()))
                    .child(icon(ic, fg, 13.))
                    .when(!collapsed, |d| d.child(sec.label()))
                    .on_click(cx.listener(move |this, _, w, cx| this.set_section(sec, w, cx)))
            }));

        let items: Vec<_> = nav_for(self.section)
            .into_iter()
            .map(|e| {
                let active = self.route == e.route;
                self.side_item(e.label, e.icon, active, e.route, cx)
            })
            .collect();

        let studio_on = mv_ui::mode::studio_enabled(cx);
        let studio_group = studio_on.then(|| {
            let assistant = self.side_button("Assistant", "icons/sparkles.svg", self.dock_open).on_click(cx.listener(|this, _, w, cx| this.toggle_dock(w, cx)));
            let studio = self.side_item("Studio", "icons/wand-sparkles.svg", self.route == Route::Studio, Route::Studio, cx);
            // Same look as the nav above; a quiet label (a divider when collapsed) sets it apart.
            div()
                .flex()
                .flex_col()
                .gap(px(1.))
                .when(collapsed, |d| d.pt(px(10.)).border_t_1().border_color(Tokens::border()))
                .when(!collapsed, |d| {
                    d.child(
                        div()
                            .px(px(10.))
                            .pb(px(4.))
                            .text_size(px(size::XS))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(Tokens::text_3())
                            .child("STUDIO"),
                    )
                })
                .child(studio)
                .child(assistant)
        });
        let settings = self.side_item("Settings", "icons/settings.svg", matches!(self.route, Route::Settings { .. }), Route::Settings { tab: None }, cx);

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(if collapsed { px(60.) } else { px(244.) })
            .h_full()
            .gap(px(14.))
            .pt(px(12.))
            .pb(px(10.))
            .px(px(10.))
            .bg(Tokens::sidebar())
            .border_r_1()
            .border_color(Tokens::border())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .h(px(30.))
                    .when(collapsed, |d| d.justify_center())
                    .when(!collapsed, |d| d.px(px(6.)))
                    .child(
                        div()
                            .size(px(22.))
                            .flex_none()
                            .rounded(px(6.))
                            .bg(Tokens::accent())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(Tokens::base())
                            .text_size(px(12.))
                            .font_weight(FontWeight::BOLD)
                            .child("M"),
                    )
                    .when(!collapsed, |d| d.child(div().text_size(px(14.)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child("MangaVox"))),
            )
            .child(segment)
            .child(div().flex().flex_col().gap(px(1.)).children(items))
            .children(studio_group)
            .child(div().flex_1())
            // Collapsed, the footer badge turns into a green update icon instead of the card.
            .when(!collapsed, |d| d.children(mv_ui::update::prompt(cx)))
            .child(
                div()
                    .flex()
                    .when(collapsed, |d| d.flex_col().items_center())
                    .when(!collapsed, |d| d.items_center())
                    .gap(px(2.))
                    .pt(px(10.))
                    .border_t_1()
                    .border_color(Tokens::border())
                    .child(div().when(!collapsed, |d| d.flex_1()).child(settings))
                    .child(mv_ui::update::badge(collapsed, cx)),
            )
    }

    fn side_button(&self, label: &'static str, ic: &'static str, active: bool) -> Stateful<Div> {
        let collapsed = self.collapsed;
        div()
            .id(label)
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(32.))
            .when(collapsed, |d| d.justify_center().w(px(40.)))
            .when(!collapsed, |d| d.px(px(10.)))
            .rounded(px(8.))
            .cursor_pointer()
            .text_size(px(size::MD))
            .text_color(if active { Tokens::text_1() } else { Tokens::text_2() })
            .when(active, |d| d.bg(Tokens::active()))
            .hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
            .child(icon(ic, if active { Tokens::accent() } else { Tokens::text_2() }, 16.))
            .when(!collapsed, |d| d.child(label))
    }

    fn side_item(&self, label: &'static str, ic: &'static str, active: bool, route: Route, cx: &mut Context<Self>) -> Stateful<Div> {
        self.side_button(label, ic, active).on_click(cx.listener(move |this, _, w, cx| this.go(route.clone(), w, cx)))
    }
}

struct Placeholder {
    title: SharedString,
}

impl Render for Placeholder {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(empty_state(self.title.clone(), "This page is being built."))
    }
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::perf::first_frame();
        let focus = is_focus(&self.route);
        // Settings brings its own navigation rail; the app sidebar would make four columns.
        let in_settings = matches!(self.route, Route::Settings { .. });
        // Studio embeds its own chat on the right, so the docked assistant stays away from it.
        let studio_on = mv_ui::mode::studio_enabled(cx);
        let dock = self.dock.clone().filter(|_| self.dock_open && !focus && studio_on && self.route != Route::Studio);
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(Tokens::base())
            // The mouse's back side button works everywhere.
            .on_mouse_down(MouseButton::Navigate(NavigationDirection::Back), cx.listener(|this, _, w, cx| this.back(w, cx)))
            .text_color(Tokens::text_1())
            .font_family("Segoe UI")
            .child(self.title_bar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .when(!focus && !in_settings, |d| d.child(self.sidebar(cx)))
                    .when_some(dock, |d, dock| d.child(div().flex_none().w(px(400.)).h_full().border_r_1().border_color(Tokens::border()).child(dock)))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_hidden()
                            .child(self.page.clone())
                            .when(!focus, |d| d.children(ui_manga::download_toast(cx)))
                            // App-wide toasts (mv_ui::toast::show), bottom centre of the page area.
                            .child(mv_ui::toast::host(cx)),
                    ),
            )
    }
}
