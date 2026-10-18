//! App shell: slim title bar + collapsible left sidebar + optional assistant dock + page.
use gpui_kit::component::TitleBar;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::nav::set_navigator;
use mv_ui::{empty_state, icon, icon_button, size, Route, Tokens};

#[derive(Clone, Copy, PartialEq, Eq)]
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

pub struct Shell {
    collapsed: bool,
    section: Section,
    route: Route,
    history: Vec<Route>,
    page: AnyView,
    dock: Option<AnyView>,
    dock_open: bool,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shell = cx.entity().downgrade();
        set_navigator(cx, move |route, window, cx| {
            if let Some(s) = shell.upgrade() {
                s.update(cx, |s, cx| s.go(route, window, cx));
            }
        });
        let route = Route::Library;
        let page = Self::resolve(&route, window, cx);
        Self { collapsed: false, section: Section::Manga, route, history: vec![], page, dock: None, dock_open: false }
    }

    fn resolve(route: &Route, window: &mut Window, cx: &mut App) -> AnyView {
        // Keep the assistant aware of where the user is.
        ui_assistant::set_page_context(route);
        ui_manga::open(route, window, cx)
            .or_else(|| ui_media::open(route, window, cx))
            .or_else(|| ui_player::open(route, window, cx))
            .or_else(|| ui_assistant::open(route, window, cx))
            .or_else(|| ui_settings::open(route, window, cx))
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
        let prev = std::mem::replace(&mut self.route, route.clone());
        self.history.push(prev);
        if self.history.len() > 50 {
            self.history.remove(0);
        }
        self.page = Self::resolve(&route, window, cx);
        cx.notify();
    }

    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(prev) = self.history.pop() {
            self.page = Self::resolve(&prev, window, cx);
            self.route = prev;
            cx.notify();
        }
    }

    fn set_section(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) {
        self.section = section;
        self.history.clear();
        self.route = section.home();
        let route = self.route.clone();
        self.page = Self::resolve(&route, window, cx);
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
                .child(icon_button("toggle-sidebar", "icons/panel-left.svg", true).on_click(cx.listener(|this, _, _, cx| {
                    this.collapsed = !this.collapsed;
                    cx.notify();
                })))
                .child(icon_button("back", "icons/arrow-left.svg", !self.history.is_empty()).on_click(cx.listener(|this, _, w, cx| this.back(w, cx))))
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
                    .px(px(6.))
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

        let assistant = self.side_button("Assistant", "icons/sparkles.svg", self.dock_open).on_click(cx.listener(|this, _, w, cx| this.toggle_dock(w, cx)));
        let studio = self.side_item("Studio", "icons/wand-sparkles.svg", self.route == Route::Studio, Route::Studio, cx);
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
                    .px(px(6.))
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
            .child(div().flex().flex_col().gap(px(1.)).child(assistant).child(studio))
            .child(div().flex_1())
            .child(div().flex().flex_col().gap(px(2.)).pt(px(10.)).border_t_1().border_color(Tokens::border()).child(settings))
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
        let dock = self.dock.clone().filter(|_| self.dock_open && !focus);
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(Tokens::base())
            .text_color(Tokens::text_1())
            .font_family("Segoe UI")
            .child(self.title_bar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .when(!focus, |d| d.child(self.sidebar(cx)))
                    .when_some(dock, |d, dock| d.child(div().flex_none().w(px(400.)).h_full().border_r_1().border_color(Tokens::border()).child(dock)))
                    .child(div().flex_1().min_w_0().h_full().overflow_hidden().child(self.page.clone())),
            )
    }
}
