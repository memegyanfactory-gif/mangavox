//! The tab home (design 9.1): the header with the Beta marker, the user's engine projects and
//! the "Start from a manga" grid of downloaded manga (`ui_studio::data::load`). Pressing a manga
//! creates its project (episode 1 with its first downloaded chapter) and opens the Manga step;
//! a project card opens it again; "Remove from list" sets the folder aside without deleting it.

use anyhow::{Context as _, Result};
use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_anime::{ChapterPick, Episode, EpisodeRef, ProjectSettings, Series, Store, SCHEMA};
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::{badge, btn, empty_view, error_view, glyphs, icon_ph, overflow_menu, ph, ph_icon, section_header, size, skeleton_block, spinner, tag, tip, ButtonKind, MenuItem, Route, Tokens, Tone, Weight};
use ui_studio::data::{ChapterItem, MangaGroup};

use crate::page::{EnginePage, Load, ProjectRow};
use crate::steps::Step;
use crate::widgets::{count, thumb};

/// A downloaded manga and how many of its chapters Studio has read.
#[derive(Clone, Debug)]
pub struct MangaRow {
    pub group: MangaGroup,
    pub read: usize,
}

// ---------------------------------------------------------------- data (blocking)

/// The downloaded manga, each with its read chapters counted. Blocking.
pub fn load_library() -> Result<Vec<MangaRow>> {
    let groups = ui_studio::data::load()?;
    Ok(groups
        .into_iter()
        .filter(|g| !g.chapters.is_empty())
        .map(|g| {
            let read = g.chapters.iter().filter(|c| c.output_dir.join("studio").join("read.json").is_file()).count();
            MangaRow { group: g, read }
        })
        .collect())
}

/// The manga's chapter with the lowest number (the first one when none has a number).
pub fn first_chapter(chapters: &[ChapterItem]) -> Option<&ChapterItem> {
    chapters.iter().min_by(|a, b| a.number.unwrap_or(f64::MAX).total_cmp(&b.number.unwrap_or(f64::MAX)))
}

/// The downloaded chapter right after `after` (by number), if any.
pub fn next_chapter(chapters: &[ChapterItem], after: Option<f64>) -> Option<&ChapterItem> {
    let after = after?;
    chapters.iter().filter(|c| c.number.is_some_and(|n| n > after)).min_by(|a, b| a.number.unwrap_or(f64::MAX).total_cmp(&b.number.unwrap_or(f64::MAX)))
}

/// What an episode stores for a downloaded chapter.
pub fn pick_of(c: &ChapterItem) -> ChapterPick {
    let chref = ui_studio::pipeline::QChapter::from_item(c).chref();
    let mut pick = mv_anime::source::pick_from(&chref, c.number);
    // The source adapter fills the dirs; the ids and label never depend on it.
    pick.chapter_id = c.id.clone();
    pick.manga_id = c.manga_id.clone();
    pick.label = c.label();
    pick.number = c.number;
    pick
}

/// Creates the manga's project (episode 1 with its first downloaded chapter) unless it exists;
/// returns its id. Blocking.
pub fn create_project(group: &MangaGroup) -> Result<String> {
    let store = Store::for_manga(&group.id);
    if store.path("project.json").is_file() {
        return Ok(store.id);
    }
    let first = first_chapter(&group.chapters).context("This manga has no downloaded chapter yet.")?;
    let ep = Episode { schema: SCHEMA, id: "ep01".into(), title: "Episode 1".into(), chapters: vec![pick_of(first)], ..Default::default() };
    let series = Series {
        schema: SCHEMA,
        id: store.id.clone(),
        manga_id: group.id.clone(),
        manga_title: group.title.clone(),
        created_at: crate::widgets::now_secs(),
        episodes: vec![EpisodeRef { id: ep.id.clone(), title: ep.title.clone() }],
        settings: ProjectSettings::default(),
        ..Default::default()
    };
    store.save_episode(&ep).context("Could not save the first episode")?;
    store.save_series(&series).context("Could not save the project")?;
    mv_core::logging::global().info("anime", &format!("created project {} for {}", series.id, series.manga_title));
    Ok(store.id)
}

/// "Remove from list": renames `projects/<id>` to `projects/.removed-<id>-<unix>`; nothing is
/// deleted. Blocking. The project's jobs must have stopped first
/// (`mv_anime::queue::cancel_project_wait`).
pub fn remove_project(id: &str) -> Result<()> {
    let store = Store::open(id);
    let to = mv_anime::store::projects_dir().join(format!(".removed-{id}-{}", crate::widgets::now_secs()));
    if let Err(e) = std::fs::rename(&store.dir, &to) {
        // The path stays in the log; the user reads what to do.
        mv_core::logging::global().warn("anime", &format!("could not set {} aside: {e}", store.dir.display()));
        anyhow::bail!("The project's folder is in use, so it could not be set aside. Close anything that has a file of it open and try again.");
    }
    mv_core::logging::global().info("anime", &format!("removed project {id} from the list (kept at {})", to.display()));
    Ok(())
}

/// The manga whose title contains every word of `query` (case-insensitive).
pub fn filter<'a>(rows: &'a [MangaRow], query: &str) -> Vec<&'a MangaRow> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    rows.iter().filter(|r| {
        let t = r.group.title.to_lowercase();
        words.iter().all(|w| t.contains(w.as_str()))
    })
    .collect()
}

/// "2 episodes · Shots" under a project card.
pub fn project_line(series: &Series, step: Option<Step>) -> String {
    let eps = count(series.episodes.len(), "episode");
    match step {
        Some(s) => format!("{eps} · {}", s.label()),
        None => eps,
    }
}

/// "12 chapters · 3 read".
pub fn manga_line(chapters: usize, read: usize) -> String {
    if read == 0 {
        format!("{} · none read yet", count(chapters, "chapter"))
    } else {
        format!("{} · {read} read", count(chapters, "chapter"))
    }
}

// ---------------------------------------------------------------- view

/// The page title with the BETA badge right of it (design 1.5).
pub fn title_block() -> Div {
    let tile = div().flex().flex_none().items_center().justify_center().size(px(32.)).rounded(px(radius::MD)).bg(Tokens::accent_soft()).child(ph_icon(ph::FILM_STRIP, Weight::Duotone, Tokens::accent(), icon_px::LG));
    let title = div()
        .flex()
        .items_center()
        .gap(px(space::SM))
        .child(div().text_size(px(size::XL)).line_height(px(28.)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).truncate().child("Anime Engine"))
        .child(badge("BETA", Tone::Warning));
    let subtitle = div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_3()).truncate().child("Turn a manga from your library into an animatic, step by step.");
    div().flex().flex_1().min_w_0().items_center().gap(px(space::MD)).child(tile).child(div().flex().flex_col().flex_1().min_w_0().child(title).child(subtitle))
}

/// One line per step for the "How it works" card.
const GUIDE: [(Step, &str); 8] = [
    (Step::Manga, "Pick the chapters of the episode; Studio reads their pages and speech."),
    (Step::Story, "The AI adapts the chapters into scenes, beats and spoken lines."),
    (Step::Bible, "Characters, places, props, the art style and voices, approved once."),
    (Step::Shots, "The AI plans every shot like a director; you can change any camera."),
    (Step::Frames, "One still per shot, drawn credit-free on the Magnific website."),
    (Step::Animate, "Motion comes in the next update."),
    (Step::Sound, "Rough voices for every line, in each character's voice."),
    (Step::Cut, "Stills, camera moves and voices become an animatic MP4."),
];

fn guide_card() -> Div {
    let mut grid = div().flex().flex_wrap().gap(px(space::MD));
    for (i, (step, text)) in GUIDE.iter().enumerate() {
        grid = grid.child(
            div()
                .flex()
                .items_start()
                .gap(px(space::SM))
                .w(px(260.))
                .child(div().flex().flex_none().items_center().justify_center().size(px(24.)).rounded_full().border_1().border_color(Tokens::border_strong()).child(icon_ph(step.icon(), Tokens::text_2(), icon_px::XS)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("{}. {}", i + 1, step.label())))
                        .child(div().text_size(px(size::XS)).line_height(px(16.)).text_color(Tokens::text_3()).whitespace_normal().child(*text)),
                ),
        );
    }
    crate::widgets::card().child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).whitespace_normal().child("Eight steps, each with its own screen. The AI does the first pass of every step; you can change anything, approve what you like, and nothing you approved changes by itself.")).child(grid)
}

/// Cover of a project card, at a book's shape.
const PROJECT_COVER: (f32, f32) = (72., 102.);
/// Cover of a manga card.
const MANGA_COVER: (f32, f32) = (48., 68.);
/// Narrowest project and manga cards; the grids fit as many columns as these allow.
const PROJECT_MIN_W: f32 = 320.;
const MANGA_MIN_W: f32 = 236.;
/// The app's own sidebar beside the page, for counting the columns that fit.
const APP_SIDEBAR_W: f32 = 256.;

/// Columns of cards at least `min_w` wide that fit in `width`, `gap` apart (1 to 6).
pub fn grid_columns(width: f32, min_w: f32, gap: f32) -> u16 {
    (((width + gap) / (min_w + gap)).floor() as i64).clamp(1, 6) as u16
}

/// "Ch. 1–45" for a manga's downloaded chapters ("12 chapters" when they have no numbers).
pub fn chapter_span(chapters: &[ChapterItem]) -> String {
    let nums: Vec<f64> = chapters.iter().filter_map(|c| c.number).collect();
    if nums.is_empty() {
        return count(chapters.len(), "chapter");
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

/// The source a manga came from when its id names one ("mangadex:…" → "mangadex").
pub fn source_of(id: &str) -> Option<String> {
    let (src, rest) = id.split_once(':')?;
    (!rest.is_empty() && !src.is_empty() && src.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')).then(|| src.replace(['-', '_'], " "))
}

impl EnginePage {
    pub(crate) fn home_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        self.ensure_search(window, cx);
        let width = (f32::from(window.viewport_size().width) - APP_SIDEBAR_W - 2. * size::PAGE_X).max(MANGA_MIN_W);
        let header = div().flex().items_center().gap(px(space::MD)).child(title_block()).child(
            btn("ae-guide", if self.guide { "Hide the guide" } else { "How it works" }, ButtonKind::Ghost).icon(ph::QUESTION).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.guide = !this.guide;
                cx.notify();
            })),
        );
        div().id("ae-home").size_full().overflow_y_scroll().track_scroll(&self.home_scroll).child(
            div()
                .flex()
                .flex_col()
                .gap(px(space::XXL))
                .px(px(size::PAGE_X))
                .py(px(size::PAGE_Y))
                .child(header)
                .when(self.guide, |d| d.child(guide_card()))
                .child(self.projects_section(width, cx))
                .child(self.manga_section(width, cx)),
        )
    }

    fn projects_section(&self, width: f32, cx: &mut Context<Self>) -> Div {
        let head = |n: Option<usize>| {
            let h = section_header("Your projects", Some(ph::FILM_STRIP)).mt(px(0.));
            match n {
                Some(n) => h.count(n),
                None => h,
            }
        };
        let cols = grid_columns(width, PROJECT_MIN_W, space::MD);
        let body: AnyElement = match &self.projects {
            Load::Loading => div().grid().grid_cols(cols).gap(px(space::MD)).children((0..cols.min(3)).map(|_| skeleton_block(relative(1.), px(PROJECT_COVER.1 + 2. * space::MD)))).into_any_element(),
            Load::Failed(e) => error_view(format!("Your projects could not be listed. {e}")).compact().retry(cx.listener(|this, _: &ClickEvent, _, cx| this.reload(cx))).into_any_element(),
            Load::Ready(rows) if rows.is_empty() => div()
                .flex()
                .items_center()
                .gap(px(space::LG))
                .p(px(space::XL))
                .rounded(px(radius::LG))
                .border_1()
                .border_dashed()
                .border_color(Tokens::border_strong())
                .bg(Tokens::surface())
                .child(div().flex().flex_none().items_center().justify_center().size(px(56.)).rounded(px(radius::LG)).bg(Tokens::accent_soft()).child(ph_icon(ph::FILM_STRIP, Weight::Duotone, Tokens::accent(), icon_px::HERO)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(space::XS))
                        .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Make your first animatic"))
                        .child(div().text_size(px(size::SM)).line_height(px(20.)).text_color(Tokens::text_2()).whitespace_normal().child("Pick a downloaded manga below. Its project opens on the Manga step, and each step after it is one screen with the AI doing the first pass.")),
                )
                .child(div().flex_none().child(icon_ph(ph::ARROW_DOWN, Tokens::text_3(), icon_px::MD)))
                .into_any_element(),
            Load::Ready(rows) => {
                let mut grid = div().grid().grid_cols(cols).gap(px(space::MD));
                for (i, row) in rows.iter().enumerate() {
                    grid = grid.child(self.project_card(i, row, cx));
                }
                grid.into_any_element()
            }
        };
        let n = match &self.projects {
            Load::Ready(r) => Some(r.len()),
            _ => None,
        };
        div().flex().flex_col().child(head(n)).child(body)
    }

    fn project_card(&self, i: usize, row: &ProjectRow, cx: &mut Context<Self>) -> Stateful<Div> {
        let s = &row.series;
        let id = s.id.clone();
        let group = SharedString::from(format!("ae-proj-{i}"));
        let cover = self.cover_of(&s.manga_id);
        let opening = self.opening.as_deref() == Some(s.id.as_str());
        let page = cx.entity().downgrade();
        let (open_id, folder, remove_id, title) = (id.clone(), Store::open(&id).dir, id.clone(), s.manga_title.clone());
        let page2 = page.clone();
        let menu = overflow_menu(
            ("ae-proj-more", i),
            vec![
                MenuItem::new("Open", move |w, cx| {
                    let _ = page.update(cx, |p, cx| p.open_project(open_id.clone(), w, cx));
                })
                .icon(ph::ARROW_RIGHT),
                MenuItem::new("Open project folder", move |_, cx| cx.open_with_system(&folder)).icon(ph::FOLDER_OPEN),
                MenuItem::separator(),
                MenuItem::new("Remove from list", move |w, cx| confirm_remove(page2.clone(), remove_id.clone(), title.clone(), w, cx)).icon(ph::TRASH).danger(),
            ],
        );
        let step = Step::from_key(&row.ui.step);
        let done = row.done.iter().filter(|d| **d).count();
        let progress = div().flex().gap(px(2.)).children(Step::ALL.iter().map(|st| {
            let on = row.done.get(st.index()).copied().unwrap_or(false);
            let here = step == Some(*st);
            div().flex_1().h(px(4.)).rounded_full().bg(if on { Tokens::accent() } else if here { Tokens::accent_border() } else { Tokens::track() })
        }));
        div()
            .id(("ae-proj", i))
            .relative()
            .group(group.clone())
            .flex()
            .items_center()
            .gap(px(space::MD))
            .min_w_0()
            .p(px(space::SM))
            .rounded(px(radius::LG))
            .border_1()
            .border_color(Tokens::border())
            .bg(Tokens::surface())
            .cursor_pointer()
            .hover(|st| st.bg(Tokens::hover()).border_color(Tokens::border_strong()))
            .tooltip(tip(format!("Open {}", s.manga_title)))
            .on_click(cx.listener(move |this, _: &ClickEvent, w, cx| this.open_project(id.clone(), w, cx)))
            .child(thumb(cover, PROJECT_COVER.0, PROJECT_COVER.1))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(space::SM))
                    .pr(px(space::XL))
                    .child(div().text_size(px(size::MD)).line_height(px(20.)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).text_ellipsis().line_clamp(2).child(s.manga_title.clone()))
                    .child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(project_line(s, step)))
                    .child(div().id(("ae-proj-steps", i)).flex().flex_col().gap(px(space::XXS)).tooltip(tip(format!("{done} of 8 steps done"))).child(progress))
                    .when(opening, |d| d.child(div().flex().items_center().gap(px(space::XS)).child(spinner(("ae-proj-spin", i), Tokens::accent(), icon_px::XS)).child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child("Opening…")))),
            )
            .child(crate::widgets::hover_actions(group).child(menu))
    }

    fn manga_section(&self, width: f32, cx: &mut Context<Self>) -> Div {
        let search = self.search.clone().map(|s| div().w(px(240.)).child(Input::new(&s).prefix(icon_ph(glyphs::SEARCH, Tokens::text_3(), icon_px::SM))));
        let head = section_header("Start from a manga", Some(glyphs::LIBRARY)).when_some(search, |h, s| h.trailing(s));
        let query = self.search.as_ref().map(|s| s.read(cx).value().to_string()).unwrap_or_default();
        let cols = grid_columns(width, MANGA_MIN_W, space::SM);
        let body: AnyElement = match &self.library {
            Load::Loading => div().grid().grid_cols(cols).gap(px(space::SM)).children((0..cols * 2).map(|_| skeleton_block(relative(1.), px(MANGA_COVER.1 + 2. * space::SM)))).into_any_element(),
            Load::Failed(e) => error_view(format!("Your downloaded manga could not be read. {e}")).compact().retry(cx.listener(|this, _: &ClickEvent, _, cx| this.reload(cx))).into_any_element(),
            Load::Ready(rows) if rows.is_empty() => empty_view(ph::BOOK_OPEN, "No downloaded manga yet", "Download chapters in Manga › Library, then come back.")
                .action(btn("ae-open-library", "Open Library", ButtonKind::Primary).icon(glyphs::LIBRARY).on_click(|_, w, cx| mv_ui::navigate(Route::Library, w, cx)))
                .into_any_element(),
            Load::Ready(rows) => {
                let shown = filter(rows, &query);
                if shown.is_empty() {
                    div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(format!("No downloaded manga matches “{}”.", query.trim())).into_any_element()
                } else {
                    let mut grid = div().grid().grid_cols(cols).gap(px(space::SM));
                    for (i, row) in shown.into_iter().enumerate() {
                        let twin = rows.iter().filter(|r| r.group.title.trim().eq_ignore_ascii_case(row.group.title.trim())).count() > 1;
                        grid = grid.child(self.manga_card(i, row, twin, cx));
                    }
                    grid.into_any_element()
                }
            }
        };
        div().flex().flex_col().child(head).child(body)
    }

    fn manga_card(&self, i: usize, row: &MangaRow, twin: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let g = &row.group;
        let has_project = match &self.projects {
            Load::Ready(p) => p.iter().any(|p| p.series.manga_id == g.id),
            _ => false,
        };
        let creating = self.creating.as_deref() == Some(g.id.as_str());
        let id = g.id.clone();
        // Two manga with the same name: where each comes from and which chapters it has.
        let tell_apart = twin.then(|| match source_of(&g.id) {
            Some(src) => format!("{} · {src}", chapter_span(&g.chapters)),
            None => chapter_span(&g.chapters),
        });
        let mark: AnyElement = if creating {
            spinner(("ae-manga-spin", i), Tokens::accent(), icon_px::SM).into_any_element()
        } else if has_project {
            tag("Project", Tone::Accent).into_any_element()
        } else {
            icon_ph(ph::PLUS, Tokens::text_3(), icon_px::SM).into_any_element()
        };
        div()
            .id(("ae-manga", i))
            .flex()
            .items_center()
            .gap(px(space::MD))
            .min_w_0()
            .h_full()
            .p(px(space::SM))
            .rounded(px(radius::LG))
            .border_1()
            .border_color(Tokens::border())
            .bg(Tokens::surface())
            .cursor_pointer()
            .hover(|s| s.bg(Tokens::hover()).border_color(Tokens::border_strong()))
            .tooltip(tip(if has_project { format!("{}: open its project", g.title.trim()) } else { format!("{}: start a project from it", g.title.trim()) }))
            .on_click(cx.listener(move |this, _: &ClickEvent, w, cx| this.start_from(id.clone(), w, cx)))
            .child(thumb(g.cover.clone(), MANGA_COVER.0, MANGA_COVER.1))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(space::XXS))
                    .child(div().text_size(px(size::SM)).line_height(px(18.)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).text_ellipsis().line_clamp(2).child(g.title.clone()))
                    .when_some(tell_apart, |d, t| d.child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_2()).child(t)))
                    .child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(manga_line(g.chapters.len(), row.read))),
            )
            // Its own corner: never on top of the title.
            .child(div().flex_none().self_start().child(mark))
    }
    pub(crate) fn cover_of(&self, manga_id: &str) -> Option<std::path::PathBuf> {
        match &self.library {
            Load::Ready(rows) => rows.iter().find(|r| r.group.id == manga_id).and_then(|r| r.group.cover.clone()),
            _ => None,
        }
    }

    /// A manga card was pressed: open its project, or create one first.
    fn start_from(&mut self, manga_id: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Load::Ready(p) = &self.projects {
            if let Some(row) = p.iter().find(|p| p.series.manga_id == manga_id) {
                let id = row.series.id.clone();
                self.open_project(id, window, cx);
                return;
            }
        }
        let Load::Ready(rows) = &self.library else { return };
        let Some(group) = rows.iter().find(|r| r.group.id == manga_id).map(|r| r.group.clone()) else { return };
        if self.creating.is_some() {
            return;
        }
        self.creating = Some(manga_id);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let res = mv_ui::bg::blocking(move || create_project(&group)).await.and_then(|r| r);
            let _ = this.update_in(cx, |this, window, cx| {
                this.creating = None;
                match res {
                    Ok(id) => {
                        this.open_project(id, window, cx);
                        this.reload(cx);
                    }
                    Err(e) => {
                        mv_core::logging::global().warn("anime", &format!("create project failed: {e:#}"));
                        mv_ui::toast::show(cx, mv_ui::Toast::error(format!("Could not start the project: {e:#}")));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// Asks before "Remove from list", then sets the folder aside, takes the project's unfinished
/// jobs out of the queue and goes home.
pub(crate) fn confirm_remove(page: WeakEntity<EnginePage>, id: String, title: String, window: &mut Window, cx: &mut App) {
    let body = format!(
        "{} leaves your projects, and its jobs that have not finished leave the queue. Nothing is deleted: the project's folder stays in your data folder, and your manga and Studio work are not touched.",
        title.trim()
    );
    mv_ui::confirm_dialog("Remove this project from the list?", body, "Remove from list", false)
        .icon(ph::TRASH)
        .on_confirm(move |window, cx| {
            let _ = window;
            let id = id.clone();
            let page = page.clone();
            mv_ui::toast::show(cx, mv_ui::Toast::info("Stopping its jobs…"));
            cx.spawn(async move |cx| {
                let target = id.clone();
                // Its jobs stop first, and the folder is set aside only once none is still
                // writing into it (off the UI thread: this waits).
                let res = mv_ui::bg::blocking(move || -> Result<()> {
                    if !mv_anime::queue::cancel_project_wait(&target, std::time::Duration::from_secs(20)) {
                        anyhow::bail!("A job of this project is still stopping. Try again in a moment.");
                    }
                    remove_project(&target)
                })
                .await
                .and_then(|r| r);
                let _ = page.update(cx, |p, cx| {
                    match res {
                        Ok(()) => {
                            if p.open.as_ref().is_some_and(|o| o.store.id == id) {
                                p.close_project(cx);
                            }
                            mv_ui::toast::show(cx, mv_ui::Toast::success("Removed from the list. The folder is kept."));
                        }
                        Err(e) => mv_ui::toast::show(cx, mv_ui::Toast::error(format!("{e:#}"))),
                    }
                    p.reload(cx);
                });
            })
            .detach();
        })
        .open(window, cx);
}

#[cfg(test)]
mod tests {
    use super::{chapter_span, filter, first_chapter, grid_columns, manga_line, next_chapter, project_line, source_of, MangaRow};
    use crate::steps::Step;
    use mv_anime::{EpisodeRef, Series};
    use std::path::PathBuf;
    use ui_studio::data::{ChapterItem, MangaGroup};

    fn ch(id: &str, n: Option<f64>) -> ChapterItem {
        ChapterItem { id: id.into(), manga_id: "m".into(), manga_title: "M".into(), number: n, title: None, pages_dir: PathBuf::new(), output_dir: PathBuf::new(), page_count: 0, has_text: false, video: None, audio_files: 0 }
    }

    fn row(title: &str) -> MangaRow {
        MangaRow { group: MangaGroup { id: title.to_lowercase(), title: title.into(), cover: None, chapters: vec![] }, read: 0 }
    }

    #[test]
    fn chapters_in_order() {
        let chs = vec![ch("c3", Some(3.)), ch("c1", Some(1.)), ch("cx", None), ch("c2", Some(2.5))];
        assert_eq!(first_chapter(&chs).map(|c| c.id.as_str()), Some("c1"));
        assert_eq!(next_chapter(&chs, Some(1.)).map(|c| c.id.as_str()), Some("c2"));
        assert_eq!(next_chapter(&chs, Some(3.)).map(|c| c.id.as_str()), None);
        assert_eq!(next_chapter(&chs, None).map(|c| c.id.as_str()), None);
        assert!(first_chapter(&[]).is_none());
    }

    #[test]
    fn search_matches_every_word() {
        let rows = vec![row("Solo Leveling"), row("A Wimp's Strategy Guide"), row("Leveling Up")];
        let names = |q: &str| filter(&rows, q).iter().map(|r| r.group.title.clone()).collect::<Vec<_>>();
        assert_eq!(names("leveling").len(), 2);
        assert_eq!(names("solo LEV"), vec!["Solo Leveling".to_string()]);
        assert_eq!(names("  ").len(), 3);
    }

    #[test]
    fn card_lines() {
        let s = Series { episodes: vec![EpisodeRef::default(), EpisodeRef::default()], ..Default::default() };
        assert_eq!(project_line(&s, Some(Step::Shots)), "2 episodes · Shots");
        assert_eq!(project_line(&Series::default(), None), "0 episodes");
        assert_eq!(manga_line(12, 3), "12 chapters · 3 read");
        assert_eq!(manga_line(1, 0), "1 chapter · none read yet");
    }

    #[test]
    fn grids_and_twins() {
        // The columns fill the width: edges line up with the header and the search box.
        assert_eq!(grid_columns(1600., 236., 8.), 6);
        assert_eq!(grid_columns(500., 236., 8.), 2);
        assert_eq!(grid_columns(100., 236., 8.), 1);
        let chs = vec![ch("a", Some(3.)), ch("b", Some(1.)), ch("c", Some(45.))];
        assert_eq!(chapter_span(&chs), "Ch. 1–45");
        assert_eq!(chapter_span(&[ch("x", None)]), "1 chapter");
        assert_eq!(source_of("mangadex:1234").as_deref(), Some("mangadex"));
        assert_eq!(source_of("solo-leveling"), None);
    }
}
