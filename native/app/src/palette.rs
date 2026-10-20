//! Command palette (Ctrl+K): one search over every destination, every Settings page, the
//! shell's actions and the manga library, on the gpui-component `Command` list (keyboard
//! navigation, filtering, Escape clears the query and then closes).
//! Linear / Raycast command menus; NN/g on command palettes and keyboard accelerators for
//! expert users: <https://www.nngroup.com/articles/ui-accelerators/>
use std::rc::Rc;

use gpui_kit::component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_kit::component::Icon;
use gpui_kit::*;
use mv_ui::{empty_view, glyphs, kbd, ph, size, Ph, Route, Tokens};

use crate::keys::Shortcut;
use crate::routes::{nav_for, Section, SECTIONS, SETTINGS_TABS};

/// A manga in the library, for "jump to title".
#[derive(Clone, Debug)]
pub struct Title {
    pub id: String,
    pub source: String,
    pub title: SharedString,
    pub author: Option<SharedString>,
}

/// What a palette row does when chosen.
#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    /// A page that belongs to a section (switches section first).
    Open(Section, Route),
    /// A page in the current context (Settings, Studio, a recent title).
    Go(Route),
    /// Same as pressing the shortcut.
    Run(Shortcut),
    CheckUpdates,
}

/// The open palette.
pub struct Palette {
    pub state: Entity<CommandState>,
    /// Focus to give back on close (the reader or player keeps its keyboard shortcuts).
    pub prev_focus: Option<FocusHandle>,
    /// The panel's focus trap: Tab stays in the palette instead of moving to the controls
    /// behind the backdrop (WAI-ARIA dialog pattern:
    /// <https://www.w3.org/WAI/ARIA/apg/patterns/dialog-modal/>).
    pub trap: FocusHandle,
    pub query: SharedString,
    pub titles: Vec<Title>,
}

/// What the palette needs to know about the shell.
pub struct Ctx<'a> {
    pub section: Section,
    pub route: &'a Route,
    pub studio: bool,
    /// The assistant can open here (Studio mode on, not in Studio or a full-screen page).
    pub can_dock: bool,
    /// The Settings page on screen (`Tab::id`), marked in the Settings group.
    pub settings_tab: Option<&'static str>,
    pub can_back: bool,
    pub can_forward: bool,
    /// What the sidebar toggle does right now ("Collapse sidebar", "Show navigation"...), or
    /// `None` where there is no sidebar (Settings, the reader, the player).
    pub sidebar: Option<&'static str>,
    pub dock_open: bool,
    pub fullscreen: bool,
    pub recent: Vec<(SharedString, Ph, Route)>,
}

struct Row {
    label: SharedString,
    icon: Ph,
    keywords: Vec<SharedString>,
    shortcut: Option<Shortcut>,
    current: bool,
    cmd: Cmd,
}

struct Group {
    heading: SharedString,
    rows: Vec<Row>,
}

fn row(label: impl Into<SharedString>, icon: Ph, cmd: Cmd) -> Row {
    Row { label: label.into(), icon, keywords: vec![], shortcut: None, current: false, cmd }
}

impl Row {
    fn keys(mut self, words: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.keywords.extend(words.into_iter().map(Into::into));
        self
    }

    fn shortcut(mut self, s: Shortcut) -> Self {
        self.shortcut = Some(s);
        self
    }

    fn current(mut self, on: bool) -> Self {
        self.current = on;
        self
    }
}

/// Groups keep their positions whatever the query (`Command` keeps the highlight by group and
/// row index, so a group appearing in front would move it): only the last group, the library,
/// fills and empties as you type, and an empty group draws nothing.
fn groups(c: &Ctx, query: &str, titles: &[Title]) -> Vec<Group> {
    let mut out = vec![];
    let searching = !query.trim().is_empty();

    if !c.recent.is_empty() {
        out.push(Group { heading: "Recent".into(), rows: c.recent.iter().map(|(label, icon, route)| row(label.clone(), *icon, Cmd::Go(route.clone()))).collect() });
    }

    // Destinations, the current section first.
    let mut order = vec![c.section];
    order.extend(SECTIONS.into_iter().filter(|s| *s != c.section));
    for s in order {
        let rows = nav_for(s)
            .into_iter()
            .map(|e| {
                let label = if e.label == "Home" { format!("{} home", s.label()) } else { e.label.to_string() };
                let here = s == c.section && &e.route == c.route;
                row(label, e.icon, Cmd::Open(s, e.route.clone()))
                    .keys([s.label().to_string(), format!("{} {}", s.label(), e.label)])
                    .keys(e.keywords.iter().copied())
                    .current(here)
            })
            .collect();
        out.push(Group { heading: s.label().into(), rows });
    }

    if c.studio {
        out.push(Group {
            heading: "Studio".into(),
            rows: vec![row("Studio", glyphs::STUDIO, Cmd::Go(Route::Studio)).keys(["recap", "video", "narrate", "project"]).current(*c.route == Route::Studio)],
        });
    }

    let mut settings = vec![row("Settings", glyphs::SETTINGS, Cmd::Go(Route::Settings { tab: None })).keys(["preferences", "options"]).shortcut(Shortcut::Settings)];
    // Icons, names and search words come from ui-settings, so a row looks like its rail entry.
    settings.extend(SETTINGS_TABS.iter().filter(|id| c.studio || **id != "studio").filter_map(|id| {
        let tab = ui_settings::Tab::from_id(id)?;
        let words = ["settings", *id, tab.description()].into_iter().chain(tab.keywords().split_whitespace()).map(str::to_string);
        Some(row(tab.label(), tab.icon(), Cmd::Go(Route::Settings { tab: Some(id.to_string()) })).keys(words).current(c.settings_tab == Some(*id)))
    }));
    out.push(Group { heading: "Settings".into(), rows: settings });

    let mut actions = vec![];
    if let Some(label) = c.sidebar {
        actions.push(row(label, ph::SIDEBAR_SIMPLE, Cmd::Run(Shortcut::Sidebar)).keys(["toggle sidebar", "navigation", "menu"]).shortcut(Shortcut::Sidebar));
    }
    // Only where it can open: Studio has its own assistant panel, full-screen pages none.
    if c.can_dock {
        actions.push(row(if c.dock_open { "Close assistant" } else { "Open assistant" }, glyphs::ASSISTANT, Cmd::Run(Shortcut::Assistant)).keys(["toggle assistant", "chat", "ai"]).shortcut(Shortcut::Assistant));
    }
    if c.can_back {
        actions.push(row("Go back", glyphs::BACK, Cmd::Run(Shortcut::Back)).keys(["previous"]).shortcut(Shortcut::Back));
    }
    if c.can_forward {
        actions.push(row("Go forward", ph::ARROW_RIGHT, Cmd::Run(Shortcut::Forward)).keys(["next"]).shortcut(Shortcut::Forward));
    }
    for s in SECTIONS.into_iter().filter(|s| *s != c.section) {
        actions.push(row(format!("Switch to {}", s.label()), s.icon(), Cmd::Run(s.shortcut())).keys(["section"]).shortcut(s.shortcut()));
    }
    actions.push(row(if c.fullscreen { "Exit full screen" } else { "Enter full screen" }, if c.fullscreen { ph::CORNERS_IN } else { ph::CORNERS_OUT }, Cmd::Run(Shortcut::Fullscreen)).keys(["fullscreen", "maximize"]).shortcut(Shortcut::Fullscreen));
    actions.push(row("Check for updates", glyphs::REFRESH, Cmd::CheckUpdates).keys(["update", "version", "upgrade"]));
    out.push(Group { heading: "Actions".into(), rows: actions });

    // Titles only while searching, so the empty palette stays a short menu; a title already
    // under Recent is not listed twice.
    let recent_manga = |id: &str| c.recent.iter().any(|(_, _, r)| matches!(r, Route::MangaDetail { manga_id, .. } if manga_id == id));
    let rows = titles
        .iter()
        .filter(|t| searching && !recent_manga(&t.id))
        .map(|t| {
            let route = Route::MangaDetail { manga_id: t.id.clone(), source_id: Some(t.source.clone()) };
            row(t.title.clone(), glyphs::MANGA, Cmd::Open(Section::Manga, route)).keys(t.author.clone()).keys(["manga"])
        })
        .collect();
    out.push(Group { heading: "Your library".into(), rows });
    out
}

/// The palette list for this frame. `on_cmd` runs the chosen row; `on_cancel` closes.
pub fn command(
    c: &Ctx,
    p: &Palette,
    on_cmd: impl Fn(Cmd, &mut Window, &mut App) + 'static,
    on_query: impl Fn(&str, &mut Window, &mut App) + 'static,
    on_cancel: impl Fn(&mut Window, &mut App) + 'static,
) -> Command {
    let groups = groups(c, &p.query, &p.titles);
    let table: Rc<Vec<Vec<Cmd>>> = Rc::new(groups.iter().map(|g| g.rows.iter().map(|r| r.cmd.clone()).collect()).collect());
    let mut cmd = Command::new(&p.state)
        .placeholder("Search pages, settings, actions and your library")
        .bordered(false)
        .max_h(px(400.))
        .on_query(on_query)
        .on_confirm(move |ix, w, cx| {
            if let Some(c) = table.get(ix.section).and_then(|g| g.get(ix.row)) {
                on_cmd(c.clone(), w, cx);
            }
        })
        .on_cancel(on_cancel)
        .empty(|_, _, _| empty_view(ph::MAGNIFYING_GLASS, "No matches", "Try a page such as Library, a setting such as Audio, or a manga title.").compact())
        .footer(|_, _, _| footer());
    for g in groups {
        let items = g.rows.into_iter().map(|r| {
            // The page you are on: checked, with its icon in the fill weight like the sidebar's
            // current destination (Fluent 2: filled icons mark the selected state).
            let item = CommandItem::new().label(r.label).icon(Icon::empty().path(r.icon.on(r.current))).keywords(r.keywords).checked(r.current);
            // The bound action only labels the row with its shortcut; `on_confirm` does the work.
            match r.shortcut {
                Some(s) => item.action(s.action()),
                None => item,
            }
        });
        cmd = cmd.group(CommandGroup::new().label(g.heading).items(items));
    }
    cmd
}

fn footer() -> Div {
    let hint = |keys: &'static [&'static str], what: &'static str| {
        div().flex().items_center().gap(px(4.)).children(keys.iter().map(|k| kbd(*k))).child(what)
    };
    div()
        .flex()
        .items_center()
        .gap(px(14.))
        .px(px(12.))
        .py(px(8.))
        .border_t_1()
        .border_color(Tokens::border())
        .text_size(px(size::XS))
        .text_color(Tokens::text_3())
        .child(hint(&["up", "down"], "Move"))
        .child(hint(&["enter"], "Open"))
        .child(hint(&["escape"], "Close"))
        .child(div().flex_1())
        .child(hint(&["ctrl-k"], "Toggle"))
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in gpui's `test` macro, which shadows the built-in one.
    use super::{groups, Cmd, Ctx, Group, Title};
    use crate::keys::Shortcut;
    use crate::routes::{nav_for, Section, SECTIONS};
    use gpui_kit::SharedString;
    use mv_ui::{glyphs, Route};

    fn ctx(route: &Route) -> Ctx<'_> {
        Ctx { section: Section::Manga, route, studio: false, can_dock: false, settings_tab: None, can_back: false, can_forward: false, sidebar: Some("Collapse sidebar"), dock_open: false, fullscreen: false, recent: vec![] }
    }

    #[test]
    fn the_sidebar_row_says_what_the_toggle_does_and_only_where_there_is_a_sidebar() {
        let route = Route::Library;
        let label = |c: &Ctx| groups(c, "", &[]).iter().flat_map(|g| g.rows.iter()).find(|r| r.cmd == Cmd::Run(Shortcut::Sidebar)).map(|r| r.label.to_string());
        assert_eq!(label(&Ctx { sidebar: Some("Show navigation"), ..ctx(&route) }).as_deref(), Some("Show navigation"));
        assert_eq!(label(&Ctx { sidebar: None, ..ctx(&route) }), None);
    }

    fn cmds(gs: &[Group]) -> Vec<Cmd> {
        gs.iter().flat_map(|g| g.rows.iter().map(|r| r.cmd.clone())).collect()
    }

    fn title(id: &str, name: &str) -> Title {
        Title { id: id.into(), source: "src".into(), title: name.to_string().into(), author: None }
    }

    #[test]
    fn every_destination_is_reachable_and_the_current_section_comes_first() {
        let route = Route::Library;
        let gs = groups(&ctx(&route), "", &[]);
        let all = cmds(&gs);
        for s in SECTIONS {
            for e in nav_for(s) {
                assert!(all.contains(&Cmd::Open(s, e.route.clone())), "{} / {}", s.label(), e.label);
            }
        }
        assert_eq!(gs[0].heading.as_ref(), "Manga");
        // The page you are on is marked.
        assert!(gs[0].rows.iter().any(|r| r.current && r.cmd == Cmd::Open(Section::Manga, Route::Library)));
    }

    #[test]
    fn studio_rows_follow_studio_mode() {
        let route = Route::Library;
        let off = cmds(&groups(&ctx(&route), "", &[]));
        assert!(!off.contains(&Cmd::Go(Route::Studio)));
        assert!(!off.contains(&Cmd::Go(Route::Settings { tab: Some("studio".into()) })));
        assert!(!off.contains(&Cmd::Run(Shortcut::Assistant)));
        let on = Ctx { studio: true, can_dock: true, ..ctx(&route) };
        let on = cmds(&groups(&on, "", &[]));
        assert!(on.contains(&Cmd::Go(Route::Studio)));
        assert!(on.contains(&Cmd::Run(Shortcut::Assistant)));
        // In Studio (its own assistant panel) the assistant row would do nothing, so it is left out.
        let studio = Route::Studio;
        let in_studio = cmds(&groups(&Ctx { studio: true, can_dock: false, ..ctx(&studio) }, "", &[]));
        assert!(!in_studio.contains(&Cmd::Run(Shortcut::Assistant)));
    }

    #[test]
    fn the_settings_page_on_screen_is_marked() {
        let route = Route::Settings { tab: None };
        let gs = groups(&Ctx { settings_tab: Some("audio"), ..ctx(&route) }, "", &[]);
        let settings = gs.iter().find(|g| g.heading.as_ref() == "Settings").expect("settings group");
        let marked: Vec<_> = settings.rows.iter().filter(|r| r.current).map(|r| r.cmd.clone()).collect();
        assert_eq!(marked, vec![Cmd::Go(Route::Settings { tab: Some("audio".into()) })]);
    }

    #[test]
    fn library_titles_fill_only_the_last_group_while_searching() {
        let route = Route::Library;
        let titles = [title("m1", "One Piece"), title("m2", "Berserk")];
        let recent = vec![
            (SharedString::from("Frieren"), glyphs::ANIME, Route::AnimeDetail { mal_id: "1".into(), title: "Frieren".into() }),
            (SharedString::from("Berserk"), glyphs::MANGA, Route::MangaDetail { manga_id: "m2".into(), source_id: None }),
        ];
        let c = Ctx { recent, ..ctx(&route) };
        let idle = groups(&c, "", &titles);
        let search = groups(&c, "one", &titles);
        // Same groups in the same places, so the highlighted row never jumps while typing.
        let heads = |gs: &[Group]| gs.iter().map(|g| g.heading.to_string()).collect::<Vec<_>>();
        assert_eq!(heads(&idle), heads(&search));
        assert_eq!(idle[0].heading.as_ref(), "Recent");
        let last = |gs: &[Group]| gs.last().map(|g| g.rows.iter().map(|r| r.cmd.clone()).collect::<Vec<_>>()).unwrap_or_default();
        assert_eq!(idle.last().map(|g| g.heading.as_ref()), Some("Your library"));
        assert!(last(&idle).is_empty());
        // Berserk is already under Recent, so only One Piece is listed.
        assert_eq!(last(&search), vec![Cmd::Open(Section::Manga, Route::MangaDetail { manga_id: "m1".into(), source_id: Some("src".into()) })]);
    }

    #[test]
    fn back_and_forward_rows_only_when_possible() {
        let route = Route::Library;
        let none = cmds(&groups(&ctx(&route), "", &[]));
        assert!(!none.contains(&Cmd::Run(Shortcut::Back)) && !none.contains(&Cmd::Run(Shortcut::Forward)));
        let both = Ctx { can_back: true, can_forward: true, ..ctx(&route) };
        let both = cmds(&groups(&both, "", &[]));
        assert!(both.contains(&Cmd::Run(Shortcut::Back)) && both.contains(&Cmd::Run(Shortcut::Forward)));
    }
}
