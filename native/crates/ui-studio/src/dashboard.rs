//! Studio's Dashboard: every downloaded manga as a cover card with how far its recaps are, and,
//! for one manga, its chapters as numbered tiles to pick for Generate (click, Shift-click for a
//! range, double-click to open in the editor). Picks made while Generate runs join its queue.
use std::collections::HashMap;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_kit::component::menu::ContextMenuExt as _;
use mv_recap::{PhaseStatus, Step};
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::Interactive as _;
use mv_ui::{
    btn, checkbox, cover_tile, empty_view, error_view, filter_chip, glyphs, icon_button, icon_ph, menu_button, overflow_menu, ph, ph_icon, progress_bar, progress_ring_sized, selected_badge, size, skeleton, skeleton_line,
    spinner, status_tag, tip, ButtonKind, CheckState, CoverSource, MenuItem, Ph, Route, StatusKind, Tokens, Weight,
};

use crate::data::{ChapterItem, MangaGroup};
use crate::page::{plural, Load, Phase, StudioPage};
use crate::pipeline::{self, ItemState, Queue};

/// Where a chapter stands, for its tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tile {
    NotStarted,
    /// Some steps made, no video yet.
    Partial(usize),
    Queued,
    Running(Step),
    Paused,
    Failed(Step),
    Done,
}

impl Tile {
    /// (word, icon, colour, soft fill).
    fn look(self) -> (String, Ph, Rgba, Rgba) {
        match self {
            Tile::NotStarted => ("New".into(), ph::CIRCLE_DASHED, Tokens::text_3(), Tokens::surface()),
            Tile::Partial(n) => (format!("{n}/6"), ph::CIRCLE_NOTCH, Tokens::info(), Tokens::info_soft()),
            Tile::Queued => ("Queued".into(), ph::HOURGLASS, Tokens::accent(), Tokens::accent_soft()),
            Tile::Running(s) => (s.label().into(), ph::CIRCLE_NOTCH, Tokens::accent(), Tokens::accent_soft()),
            Tile::Paused => ("Paused".into(), ph::PAUSE_CIRCLE, Tokens::warning(), Tokens::warning_soft()),
            Tile::Failed(s) => (format!("{} failed", s.label()), ph::WARNING_CIRCLE, Tokens::danger(), Tokens::danger_soft()),
            Tile::Done => ("Done".into(), ph::CHECK_CIRCLE, Tokens::success(), Tokens::success_soft()),
        }
    }
}

/// The dashboard's chapter filter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum Filter {
    #[default]
    All,
    NotStarted,
    InProgress,
    Failed,
    Done,
}

impl Filter {
    const ALL: [(Filter, &'static str, Ph); 5] = [
        (Filter::All, "All", ph::SQUARES_FOUR),
        (Filter::NotStarted, "Not started", ph::CIRCLE_DASHED),
        (Filter::InProgress, "In progress", ph::CIRCLE_NOTCH),
        (Filter::Failed, "Failed", ph::WARNING_CIRCLE),
        (Filter::Done, "Done", ph::CHECK_CIRCLE),
    ];

    pub fn matches(self, t: Tile) -> bool {
        match self {
            Filter::All => true,
            Filter::NotStarted => t == Tile::NotStarted,
            Filter::InProgress => matches!(t, Tile::Partial(_) | Tile::Queued | Tile::Running(_) | Tile::Paused),
            Filter::Failed => matches!(t, Tile::Failed(_)),
            Filter::Done => t == Tile::Done,
        }
    }
}

/// How many of the six steps a chapter has made.
pub(crate) fn steps_done(st: &PhaseStatus) -> usize {
    Phase::ALL.iter().filter(|p| p.done(st)).count()
}

/// A chapter's tile state: its Generate item first, then a run or job by hand, then its files.
pub(crate) fn tile_state(id: &str, st: Option<&PhaseStatus>, queue: &Queue, busy: Option<Step>) -> Tile {
    if let Some(it) = queue.for_chapter(id) {
        match it.state {
            ItemState::Queued => return Tile::Queued,
            ItemState::Running => return Tile::Running(it.current_step().unwrap_or(Step::Read)),
            ItemState::Paused => return Tile::Paused,
            ItemState::Failed => return Tile::Failed(it.current_step().unwrap_or(Step::Read)),
            ItemState::Done => {}
        }
    }
    if let Some(s) = busy {
        return Tile::Running(s);
    }
    let Some(st) = st else { return Tile::NotStarted };
    if st.video.is_some() {
        return Tile::Done;
    }
    match steps_done(st) {
        0 => Tile::NotStarted,
        n => Tile::Partial(n),
    }
}

/// Shift-click: the ids between `anchor` and `to` in `order` (both ends included).
pub(crate) fn range(order: &[String], anchor: &str, to: &str) -> Vec<String> {
    let (Some(a), Some(b)) = (order.iter().position(|x| x == anchor), order.iter().position(|x| x == to)) else { return vec![to.to_string()] };
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    order[lo..=hi].to_vec()
}

/// The number on a tile ("12", "3.5", or "#4" for unnumbered chapters).
/// The chapters a button acts on, by name when there are few: "Ch. 31", "Ch. 4, Ch. 7",
/// "5 chapters" (a count alone read like a chapter number: "Redo 1 chapter").
pub(crate) fn chapters_phrase(chapters: &[ChapterItem]) -> String {
    match chapters.len() {
        1..=3 => chapters.iter().map(|c| pipeline::short_label(c.number, c.title.as_deref())).collect::<Vec<_>>().join(", "),
        n => format!("{n} chapters"),
    }
}

fn tile_number(c: &ChapterItem, i: usize) -> String {
    match c.number {
        Some(n) if n.fract() == 0.0 => format!("{}", n as i64),
        Some(n) => format!("{n}"),
        None => format!("#{}", i + 1),
    }
}

impl StudioPage {
    /// Which step a run or job started by hand is making for the chapter, if any.
    fn busy_step(&self, id: &str) -> Option<Step> {
        if let Some(r) = self.run.as_ref().filter(|r| r.running() && r.chapter_id.as_deref() == Some(id)) {
            return Some(if r.kind == crate::run::RunKind::Read { Step::Read } else { Step::Story });
        }
        if let Some(j) = self.jobs.iter().find(|j| j.chapter_id == id && !j.state.finished()) {
            return Some(if crate::phases::is_render(j) { Step::Video } else { Step::Audio });
        }
        (self.music.busy.is_some() && self.music.composing.iter().any(|c| c == id)).then_some(Step::Music)
    }

    pub(crate) fn tile_of(&self, c: &ChapterItem) -> Tile {
        tile_state(&c.id, self.statuses.get(&c.id), &self.queue, self.busy_step(&c.id))
    }

    /// Opens a chapter in the editor at `step`.
    pub(crate) fn go_to(&mut self, chapter: &str, step: Option<Step>, cx: &mut Context<Self>) {
        let Load::Ready(groups) = &self.library else { return };
        let Some(item) = groups.iter().flat_map(|g| g.chapters.iter()).find(|c| c.id == chapter).cloned() else { return };
        self.view = crate::page::View::Editor;
        self.open_groups.insert(item.manga_id.clone());
        self.focus_chapter(item, cx);
        if let Some(s) = step {
            self.set_phase(Phase::of(s), cx);
        }
        cx.notify();
    }

    pub(crate) fn dashboard(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let body: AnyElement = match &self.library {
            Load::Loading => div()
                .flex()
                .flex_wrap()
                .gap(px(space::LG))
                .p(px(space::XL))
                .children((0..8).map(|_| div().flex().flex_col().gap(px(space::SM)).child(skeleton(px(160.), px(228.), radius::LG)).child(skeleton_line(px(120.)))))
                .into_any_element(),
            Load::Failed(e) => div()
                .p(px(space::XL))
                .child(error_view(format!("Could not load your chapters. {e}")).title("Library unavailable").retry(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.library = Load::Loading;
                    this.reload_library(cx);
                    cx.notify();
                })))
                .into_any_element(),
            Load::Ready(groups) if groups.is_empty() => div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p(px(space::XL))
                .child(
                    empty_view(glyphs::DOWNLOADS, "Download a chapter to begin", "Studio turns downloaded chapters into narrated recap videos. Download chapters from a manga page, then come back here.")
                        .action(btn("dash-library", "Open Library", ButtonKind::Primary).icon(glyphs::LIBRARY).on_click(|_, w, cx| mv_ui::navigate(Route::Library, w, cx))),
                )
                .into_any_element(),
            Load::Ready(groups) => {
                let open = self.dash_manga.clone().and_then(|id| groups.iter().find(|g| g.id == id).cloned());
                match open {
                    Some(g) => self.picker(&g, cx),
                    None => {
                        let groups = groups.clone();
                        self.manga_grid(&groups, cx)
                    }
                }
            }
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .children(self.queue_strip(cx))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(div().id("dash-scroll").flex().flex_col().size_full().overflow_y_scroll().track_scroll(&self.dash_scroll).child(body))
                    .child(crate::page::vbar("dash-bar", &self.dash_scroll)),
            )
            .into_any_element()
    }

    /// A one-line summary of Generate over the dashboard, while it has work.
    fn queue_strip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let q = &self.queue;
        let active = q.active();
        if active == 0 {
            return None;
        }
        let total = q.items.len();
        let done = q.items.iter().filter(|i| i.state == ItemState::Done).count();
        let running = q.items.iter().find(|i| i.state == ItemState::Running);
        let text = match (running, q.paused) {
            (_, true) => format!("Generate is paused · {active} chapter{} waiting", plural(active)),
            (Some(it), _) => format!(
                "Generating {} · {} {}",
                it.chapter.label(),
                it.current_step().map(|s| s.label()).unwrap_or("Starting"),
                self.item_detail(it).map(|d| format!("· {d}")).unwrap_or_default()
            ),
            (None, _) => format!("{active} chapter{} waiting", plural(active)),
        };
        let p = if total > 0 { q.items.iter().map(|i| i.progress()).sum::<f32>() / total as f32 } else { 0. };
        Some(
            div()
                .flex()
                .flex_none()
                .flex_wrap()
                .items_center()
                .gap(px(space::MD))
                .mx(px(space::XL))
                .mt(px(space::MD))
                .px(px(space::MD))
                .py(px(space::SM))
                .rounded(px(radius::LG))
                .border_1()
                .border_color(if q.paused { Tokens::warning_border() } else { Tokens::accent_border() })
                .bg(if q.paused { Tokens::warning_soft() } else { Tokens::accent_soft() })
                .child(if q.paused {
                    ph_icon(ph::PAUSE_CIRCLE, Weight::Fill, Tokens::warning(), icon_px::LG).into_any_element()
                } else {
                    progress_ring_sized(p, 22.)
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w(px(200.))
                        .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(text))
                        .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child(format!("{done} of {total} chapter{} made · picks you add now join the queue", plural(total)))),
                )
                .child(if q.paused {
                    btn("strip-resume", "Resume", ButtonKind::Primary).small().icon(ph::PLAY).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.sync_pipeline(cx);
                        pipeline::resume_all();
                        cx.notify();
                    }))
                } else {
                    btn("strip-pause", "Pause", ButtonKind::Secondary).small().icon(ph::PAUSE).tooltip("Pause after stopping the current step; finished work is kept").on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                        pipeline::pause_all();
                        cx.notify();
                    }))
                })
                .child(btn("strip-queue", "View queue", ButtonKind::Ghost).small().icon(ph::QUEUE).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.side_open = true;
                    this.side = crate::page::Side::Activity;
                    cx.notify();
                })))
                .into_any_element(),
        )
    }

    fn manga_grid(&mut self, groups: &[MangaGroup], cx: &mut Context<Self>) -> AnyElement {
        let chapters: usize = groups.iter().map(|g| g.chapters.len()).sum();
        let made: usize = groups.iter().flat_map(|g| g.chapters.iter()).filter(|c| self.statuses.get(&c.id).is_some_and(|s| s.video.is_some())).count();
        let filter = self.dash_search.read(cx).value().to_lowercase();
        let shown: Vec<&MangaGroup> = groups.iter().filter(|g| filter.is_empty() || g.title.to_lowercase().contains(&filter)).collect();
        let header = div()
            .flex()
            .flex_wrap()
            .items_end()
            .gap(px(space::LG))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(240.))
                    .gap(px(space::XS))
                    .child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child("Your recaps"))
                    .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(format!(
                        "{} manga · {chapters} chapter{} downloaded · {made} video{} made. Pick a manga to choose chapters for Generate.",
                        groups.len(),
                        plural(chapters),
                        plural(made)
                    ))),
            )
            .child(div().w(px(240.)).child(gpui_kit::component::input::Input::new(&self.dash_search).prefix(icon_ph(glyphs::SEARCH, Tokens::text_3(), icon_px::SM))))
            .map(|d| {
                if !self.deleting.is_empty() {
                    let n = self.deleting.len();
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::SM))
                            .h(px(32.))
                            .text_size(px(size::SM))
                            .text_color(Tokens::text_2())
                            .child(spinner("dash-deleting", Tokens::accent(), icon_px::SM))
                            .child(format!("Deleting {n} manga…")),
                    )
                } else if self.dash_select.is_none() {
                    d.child(btn("dash-select", "Select", ButtonKind::Secondary).icon(ph::CHECK_SQUARE).tooltip("Pick manga to delete from Studio").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.dash_select = Some(Default::default());
                        cx.notify();
                    })))
                } else {
                    d
                }
            });
        let shown_ids: Vec<String> = shown.iter().map(|g| g.id.clone()).collect();
        let select_bar = self.dash_select.as_ref().map(|picked| self.select_bar(&shown_ids, picked, cx));
        let cards = div().flex().flex_wrap().gap(px(space::LG)).children(shown.iter().enumerate().map(|(i, g)| self.manga_card(i, g, cx)));
        div()
            .flex()
            .flex_col()
            .gap(px(space::XL))
            .p(px(space::XL))
            .pr(px(space::XXL))
            .child(header)
            .children(select_bar)
            .map(|d| {
                if shown.is_empty() {
                    d.child(empty_view(glyphs::SEARCH, "No manga match", format!("Nothing downloaded matches \"{filter}\".")).compact())
                } else {
                    d.child(cards)
                }
            })
            .into_any_element()
    }

    fn manga_card(&self, i: usize, g: &MangaGroup, cx: &mut Context<Self>) -> AnyElement {
        let n = g.chapters.len();
        let tiles: Vec<Tile> = g.chapters.iter().map(|c| self.tile_of(c)).collect();
        // "Made" means a video exists, whatever Generate is doing to the chapter now.
        let done = g.chapters.iter().filter(|c| self.statuses.get(&c.id).is_some_and(|s| s.video.is_some())).count();
        let failed = tiles.iter().filter(|t| matches!(t, Tile::Failed(_))).count();
        let working = tiles.iter().filter(|t| matches!(t, Tile::Running(_) | Tile::Queued)).count();
        let steps: usize = g.chapters.iter().map(|c| self.statuses.get(&c.id).map(steps_done).unwrap_or(0)).sum();
        let p = if n > 0 { steps as f32 / (n * Phase::ALL.len()) as f32 } else { 0. };
        let id = g.id.clone();
        let badge = match (working, failed) {
            (w, _) if w > 0 => Some(format!("{w} in queue")),
            (_, f) if f > 0 => Some(format!("{f} failed")),
            _ => None,
        };
        // In Select mode a click ticks the manga instead of opening it.
        let ticked = self.dash_select.as_ref().map(|s| s.contains(&g.id));
        let toggle = {
            let id = g.id.clone();
            move |this: &mut StudioPage, cx: &mut Context<StudioPage>| {
                if let Some(s) = this.dash_select.as_mut() {
                    if !s.remove(&id) {
                        s.insert(id.clone());
                    }
                }
                cx.notify();
            }
        };
        let on_tile = toggle.clone();
        let mut tile = cover_tile(("dash-manga", i), g.title.clone())
            .sub(format!("{n} chapter{}", plural(n)))
            .cover(g.cover.clone().map(CoverSource::File))
            .width(160.)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if this.dash_select.is_some() {
                    on_tile(this, cx);
                    return;
                }
                this.dash_manga = Some(id.clone());
                this.dash_filter = Default::default();
                this.dash_anchor = None;
                cx.notify();
            }));
        if let Some(b) = badge {
            tile = tile.badge(b);
        }
        let (color, line) = match (failed, done) {
            (f, _) if f > 0 => (Tokens::danger(), format!("{done}/{n} made · {f} failed")),
            (_, d) if d == n => (Tokens::success(), "All made".to_string()),
            _ => (Tokens::text_2(), format!("{done}/{n} made")),
        };
        let more = (ticked.is_none() && self.deleting.is_empty()).then(|| {
            let (weak, ids) = (cx.entity().downgrade(), vec![g.id.clone()]);
            overflow_menu(
                ("dash-manga-more", i),
                vec![MenuItem::new("Delete from Studio…", move |w: &mut Window, cx: &mut App| {
                    let _ = weak.update(cx, |this, cx| this.confirm_delete_manga(ids.clone(), w, cx));
                })
                .icon(glyphs::DELETE)
                .danger()],
            )
            .small()
        });
        div()
            .relative()
            .flex()
            .flex_col()
            .w(px(160.))
            .gap(px(6.))
            .child(tile)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        div()
                            .id(("dash-manga-stats", i))
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(px(6.))
                            .tooltip(tip(format!("{:.0}% of the steps made across {n} chapter{}", p * 100., plural(n))))
                            .child(progress_ring_sized(p, 16.))
                            .child(div().flex_1().min_w_0().truncate().text_size(px(size::XS)).text_color(color).child(line))
                            .when(working > 0, |d| d.child(spinner(("dash-manga-spin", i), Tokens::accent(), icon_px::XS))),
                    )
                    .children(more),
            )
            .when_some(ticked, |d, on| {
                d.child(
                    checkbox(("dash-manga-check", i), on)
                        .absolute()
                        .top(px(6.))
                        .left(px(6.))
                        .bg(Tokens::glass())
                        .aria_label(SharedString::from(format!("Select {}", g.title)))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| toggle(this, cx))),
                )
            })
            .into_any_element()
    }

    /// Select mode's bar: Select all over the manga shown, how many are ticked, Delete, Cancel.
    fn select_bar(&self, shown: &[String], picked: &std::collections::BTreeSet<String>, cx: &mut Context<Self>) -> AnyElement {
        let n = picked.len();
        let state = crate::delete::all_state(shown, picked);
        let shown = shown.to_vec();
        let ids: Vec<String> = picked.iter().cloned().collect();
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(space::SM))
            .p(px(space::MD))
            .rounded(px(radius::LG))
            .border_1()
            .border_color(if n > 0 { Tokens::accent_border() } else { Tokens::border() })
            .bg(if n > 0 { Tokens::accent_soft() } else { Tokens::surface() })
            .child(checkbox("dash-select-all", state).child("Select all").on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if let Some(s) = this.dash_select.as_mut() {
                    if state == CheckState::Checked {
                        for id in &shown {
                            s.remove(id);
                        }
                    } else {
                        s.extend(shown.iter().cloned());
                    }
                }
                cx.notify();
            })))
            .child(div().flex_1().min_w(px(160.)).text_size(px(size::SM)).text_color(if n > 0 { Tokens::text_1() } else { Tokens::text_3() }).child(crate::delete::selected_text(n)))
            .child(
                btn("dash-select-delete", "Delete", ButtonKind::DangerGhost)
                    .icon(glyphs::DELETE)
                    .when(n == 0, |b| b.disabled("Pick the manga to delete first"))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.confirm_delete_manga(ids.clone(), window, cx))),
            )
            .child(btn("dash-select-cancel", "Cancel", ButtonKind::Ghost).tooltip("Leave Select mode (Esc)").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.dash_select = None;
                cx.notify();
            })))
            .into_any_element()
    }

    /// One manga's chapters as numbered tiles to pick for Generate.
    fn picker(&mut self, g: &MangaGroup, cx: &mut Context<Self>) -> AnyElement {
        let tiles: Vec<(ChapterItem, Tile)> = g.chapters.iter().map(|c| (c.clone(), self.tile_of(c))).collect();
        let counts: HashMap<Filter, usize> = Filter::ALL.iter().map(|(f, _, _)| (*f, tiles.iter().filter(|(_, t)| f.matches(*t)).count())).collect();
        let filter = self.dash_filter;
        let shown: Vec<(usize, ChapterItem, Tile)> = tiles.iter().enumerate().filter(|(_, (_, t))| filter.matches(*t)).map(|(i, (c, t))| (i, c.clone(), *t)).collect();
        let order: Vec<String> = shown.iter().map(|(_, c, _)| c.id.clone()).collect();
        let picked_here: Vec<ChapterItem> = g.chapters.iter().filter(|c| self.dash_pick.contains(&c.id)).cloned().collect();
        let n = g.chapters.len();
        let done = g.chapters.iter().filter(|c| self.statuses.get(&c.id).is_some_and(|s| s.video.is_some())).count();
        let failed = counts.get(&Filter::Failed).copied().unwrap_or(0);
        let steps: usize = g.chapters.iter().map(|c| self.statuses.get(&c.id).map(steps_done).unwrap_or(0)).sum();
        let p = if n > 0 { steps as f32 / (n * Phase::ALL.len()) as f32 } else { 0. };

        let head = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(space::MD))
            .child(
                div()
                    .w(px(56.))
                    .h(px(78.))
                    .flex_none()
                    .rounded(px(radius::SM))
                    .overflow_hidden()
                    .bg(Tokens::surface_2())
                    .border_1()
                    .border_color(Tokens::border())
                    .flex()
                    .items_center()
                    .justify_center()
                    .map(|d| match &g.cover {
                        Some(p) => d.child(img(p.clone()).size_full().object_fit(ObjectFit::Cover)),
                        None => d.child(ph_icon(glyphs::MANGA, Weight::Duotone, Tokens::text_3(), icon_px::LG)),
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(220.))
                    .gap(px(space::XS))
                    .child(div().text_size(px(size::XL)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).truncate().child(g.title.clone()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::SM))
                            .child(div().flex_none().text_size(px(size::XS)).text_color(Tokens::text_2()).child(format!(
                                "{n} chapter{} · {done} made{}",
                                plural(n),
                                if failed > 0 { format!(" · {failed} failed") } else { String::new() }
                            )))
                            .child(div().w(px(160.)).child(progress_bar("dash-manga-progress", p, false))),
                    ),
            )
            .child({
                let made_items: Vec<ChapterItem> = self.made_videos(g).into_iter().map(|(c, _)| c).collect();
                let made_n = made_items.len();
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(space::XS))
                    .child(
                        btn("dash-restyle-all", "Music & subtitles", ButtonKind::Secondary)
                            .small()
                            .icon(glyphs::MUSIC)
                            .tooltip("Change the music (new, quieter, or muted) and the subtitles of every made chapter, then re-make their videos")
                            .when(made_n == 0, |b| b.disabled("No chapter has a video yet"))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                let scope = format!("all {made_n} made chapter{}", plural(made_n));
                                this.open_restyle(made_items.clone(), scope, true, cx);
                            })),
                    )
                    .child(btn("dash-back", "All manga", ButtonKind::Ghost).small().icon(glyphs::BACK).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.dash_manga = None;
                        cx.notify();
                    })))
                    .child({
                        let ids = vec![g.id.clone()];
                        icon_button("dash-delete-manga", glyphs::DELETE, "Delete this manga from Studio…")
                            .small()
                            .when(!self.deleting.is_empty(), |b| b.disabled("Deleting…"))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.confirm_delete_manga(ids.clone(), window, cx)))
                    })
            });

        let all_ids: Vec<String> = order.clone();
        let none_ids: Vec<String> = g.chapters.iter().map(|c| c.id.clone()).collect();
        let all_on = !all_ids.is_empty() && all_ids.iter().all(|id| self.dash_pick.contains(id));
        // The tile legend, folded into a "?" at the end of the row.
        let legend = "New: not started\n3/6: some steps made\nQueued: waiting in Generate\nA step name, spinning: being made now\nPaused: Generate is paused\n\"Audio failed\" and the like: that step failed\nDone: video made\nThe bar under each number: one segment per step, filled when made.";
        let tools = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(space::XS))
            .children(Filter::ALL.iter().enumerate().map(|(i, &(f, label, icon))| {
                filter_chip(("dash-filter", i), label, filter == f).icon(icon).count(counts.get(&f).copied().unwrap_or(0)).h(px(24.)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.dash_filter = f;
                    cx.notify();
                }))
            }))
            .child(div().flex_1().min_w(px(space::SM)))
            .child(
                btn("dash-all", if filter == Filter::All { "Select all" } else { "Select shown" }, ButtonKind::Ghost)
                    .small()
                    .icon(ph::CHECKS)
                    .when(all_on || all_ids.is_empty(), |b| b.disabled(if all_ids.is_empty() { "No chapters shown" } else { "Every shown chapter is picked" }))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let new: Vec<String> = all_ids.iter().filter(|id| !this.dash_pick.contains(*id)).cloned().collect();
                        this.pick_many(&new, cx);
                    })),
            )
            .child(
                btn("dash-none", "Select none", ButtonKind::Ghost)
                    .small()
                    .icon(ph::X)
                    .when(picked_here.is_empty(), |b| b.disabled("Nothing is picked"))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        for id in &none_ids {
                            this.dash_pick.remove(id);
                        }
                        this.dash_anchor = None;
                        cx.notify();
                    })),
            )
            .child(div().id("dash-legend").flex().items_center().justify_center().size(px(24.)).rounded(px(radius::SM)).hover(|s| s.bg(Tokens::hover())).tooltip(tip(legend)).child(icon_ph(ph::QUESTION, Tokens::text_3(), icon_px::SM)));

        let video_summary = {
            let videos = self.made_videos(g);
            let secs: f64 = videos.iter().filter_map(|(_, p)| crate::export::video_seconds(p)).sum();
            let mut s = match videos.len() {
                0 => "no chapter video yet".to_string(),
                k => format!("{k} chapter{}", plural(k)),
            };
            if secs > 0. {
                s += &format!(" · {}", crate::export::fmt_duration(secs));
            }
            if self.export_job.as_ref().is_some_and(|j| j.manga_id == g.id) {
                s += " · working…";
            }
            s
        };
        let video = self.fold("video", ph::FILM_STRIP, "Full recap video", video_summary, |this, cx| this.video_section(g, cx), cx);
        let yt_summary = if mv_recap::Brain::dir(&g.id).join("youtube_package.json").is_file() { "researched · open to see it" } else { "not generated yet" };
        let youtube = self.fold("youtube", ph::YOUTUBE_LOGO, "YouTube package", yt_summary.to_string(), |this, cx| this.youtube_package(g, cx), cx);
        let chapters_summary = format!(
            "{n} · {done} made{}{}",
            if failed > 0 { format!(" · {failed} failed") } else { String::new() },
            if picked_here.is_empty() { String::new() } else { format!(" · {} picked", picked_here.len()) }
        );
        let chapters = self.fold(
            "chapters",
            ph::LIST_NUMBERS,
            "Chapters",
            chapters_summary,
            |this, cx| {
                let grid = div().flex().flex_wrap().gap(px(6.)).children(shown.iter().map(|(i, c, t)| this.chapter_tile(*i, c, *t, &order, cx)));
                let body = if shown.is_empty() {
                    div().py(px(space::LG)).child(empty_view(ph::FUNNEL, "No chapters here", "No chapter of this manga matches the filter.").compact()).into_any_element()
                } else {
                    grid.into_any_element()
                };
                div().flex().flex_col().gap(px(space::SM)).child(tools).child(body).into_any_element()
            },
            cx,
        );
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .p(px(space::LG))
            .pr(px(space::XXL))
            .child(head)
            .child(video)
            .child(youtube)
            .child(chapters)
            .child(self.pick_bar(&picked_here, cx))
            .into_any_element()
    }

    /// A dashboard section that folds to its header row; `body` is only built while open.
    fn fold(&mut self, key: &'static str, icon: Ph, title: &'static str, summary: String, body: impl FnOnce(&mut Self, &mut Context<Self>) -> AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let open = !self.dash_collapsed.contains(key);
        let body = open.then(|| body(self, cx));
        let head = crate::widgets::fold_head(key, icon, title, summary, open).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            if !this.dash_collapsed.remove(key) {
                this.dash_collapsed.insert(key.to_string());
            }
            let mut keys: Vec<String> = this.dash_collapsed.iter().cloned().collect();
            keys.sort();
            // Only this key: other parts of Studio and Settings write the same file.
            let mut on_disk = mv_recap::Prefs::load();
            on_disk.dash_collapsed = keys.clone();
            on_disk.save();
            this.prefs.dash_collapsed = keys;
            cx.notify();
        }));
        div().flex().flex_col().gap(px(space::XS)).child(head).children(body).into_any_element()
    }

    /// Picks chapters. Picking only selects: nothing runs until Generate or Redo is pressed.
    fn pick_many(&mut self, ids: &[String], cx: &mut Context<Self>) {
        for id in ids {
            self.dash_pick.insert(id.clone());
        }
        cx.notify();
    }

    fn chapter_tile(&self, i: usize, c: &ChapterItem, t: Tile, order: &[String], cx: &mut Context<Self>) -> AnyElement {
        let picked = self.dash_pick.contains(&c.id);
        let (word, icon, color, fill) = t.look();
        let st = self.statuses.get(&c.id).cloned().unwrap_or_default();
        let made: Vec<bool> = Phase::ALL.iter().map(|p| p.done(&st)).collect();
        let tip_text = format!(
            "{} · {}\n{}\nClick to pick · Shift-click picks a range · Double-click opens it · Right-click for more",
            c.label(),
            word,
            Phase::ALL.iter().map(|p| format!("{} {}", p.label(), if p.done(&st) { "✓" } else { "–" })).collect::<Vec<_>>().join("  ")
        );
        let (id, order) = (c.id.clone(), order.to_vec());
        let running = matches!(t, Tile::Running(_));
        div()
            .id(("dash-tile", i))
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(2.))
            .w(px(72.))
            .h(px(76.))
            .rounded(px(radius::MD))
            .bg(if picked { Tokens::accent_soft() } else { fill })
            .border_1()
            .border_color(if picked { Tokens::accent() } else { Tokens::border() })
            .when(picked, |d| d.border_2())
            .role(Role::CheckBox)
            .aria_selected(picked)
            .aria_label(SharedString::from(format!("{}, {word}", c.label())))
            .tooltip(tip(tip_text))
            .hover(|s| s.border_color(Tokens::border_strong()))
            .pressable()
            .child(div().text_size(px(18.)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child(tile_number(c, i)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .max_w(px(64.))
                    .text_size(px(size::XS))
                    .text_color(color)
                    .child(if running { spinner(("dash-tile-spin", i), color, icon_px::XS).into_any_element() } else { icon_ph(icon, color, icon_px::XS).into_any_element() })
                    .child(div().truncate().child(word)),
            )
            // One segment per step: filled when made.
            .child(div().absolute().bottom(px(5.)).left(px(7.)).right(px(7.)).flex().gap(px(2.)).children(made.into_iter().map(|m| div().flex_1().h(px(3.)).rounded(px(2.)).bg(if m { Tokens::success() } else { Tokens::track() }))))
            .when(picked, |d| d.child(div().absolute().top(px(-6.)).right(px(-6.)).child(selected_badge())))
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                if ev.click_count() >= 2 {
                    // The first click of the double-click picked it: undo that, then open it.
                    if !this.dash_pick.remove(&id) {
                        this.dash_pick.insert(id.clone());
                    }
                    this.go_to(&id, None, cx);
                    return;
                }
                if ev.modifiers().shift {
                    if let Some(anchor) = this.dash_anchor.clone() {
                        let ids = range(&order, &anchor, &id);
                        this.pick_many(&ids, cx);
                        return;
                    }
                }
                this.dash_anchor = Some(id.clone());
                if !this.dash_pick.remove(&id) {
                    this.dash_pick.insert(id.clone());
                }
                cx.notify();
            }))
            .context_menu(self.tile_menu(c, cx))
            .into_any_element()
    }

    /// The pick bar's "Redo" dropdown: tick a step to make it again, even where done. Every step
    /// after it is ticked too ("follows"), since it is built from it.
    fn redo_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let redo = self.dash_redo;
        let weak = cx.entity().downgrade();
        let set = move |r: Option<Step>| {
            let weak = weak.clone();
            move |_: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |this, cx| {
                    this.dash_redo = r;
                    cx.notify();
                });
            }
        };
        let mut items = vec![MenuItem::heading("Make again"), MenuItem::new("Nothing · only missing steps", set(None)).icon(ph::CHECKS).checked(redo.is_none()), MenuItem::separator()];
        for (s, label, icon) in crate::generate::REDO {
            let on = redo.is_some_and(|r| s.index() >= r.index());
            let text = if on && redo != Some(s) { format!("{label} · follows") } else { label.to_string() };
            // Ticking the first ticked step again clears the redo.
            let next = if redo == Some(s) { None } else { Some(s) };
            items.push(MenuItem::new(text, set(next)).icon(icon).checked(on));
        }
        let label = match redo {
            Some(r) => format!("Redo: {}", r.label()),
            None => "Redo".to_string(),
        };
        menu_button("dash-redo", ph::ARROWS_CLOCKWISE, "Make steps again, even where they are done", items)
            .label(label)
            .small()
            .min_width(240.)
            .border_1()
            .border_color(if redo.is_some() { Tokens::accent() } else { Tokens::border() })
            .into_any_element()
    }

    /// The bar under the tiles: what is picked and the Generate button.
    fn pick_bar(&self, picked: &[ChapterItem], cx: &mut Context<Self>) -> AnyElement {
        let n = picked.len();
        let bar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(space::SM))
            .px(px(space::MD))
            .py(px(space::SM))
            .rounded(px(radius::LG))
            .border_1()
            .border_color(if n > 0 { Tokens::accent_border() } else { Tokens::border() })
            .bg(if n > 0 { Tokens::accent_soft() } else { Tokens::surface() });
        if n == 0 {
            return bar
                .text_size(px(size::SM))
                .text_color(Tokens::text_3())
                .child(icon_ph(ph::LIST_NUMBERS, Tokens::text_3(), icon_px::MD))
                .child(div().flex_1().min_w(px(200.)).child("Pick the chapters to turn into recaps, then press Generate. Shift-click picks a range."))
                .into_any_element();
        }
        let queued = picked.iter().filter(|c| self.queue.for_chapter(&c.id).is_some_and(|i| i.state.active())).count();
        let fresh: Vec<ChapterItem> = picked.iter().filter(|c| !self.queue.for_chapter(&c.id).is_some_and(|i| i.state.active())).cloned().collect();
        let made = picked.iter().filter(|c| self.statuses.get(&c.id).is_some_and(|s| s.video.is_some())).count();
        let names = picked.iter().map(|c| pipeline::short_label(c.number, c.title.as_deref())).collect::<Vec<_>>();
        let names = if names.len() > 8 { format!("{} and {} more", names[..8].join(", "), names.len() - 8) } else { names.join(", ") };
        let note = match (queued, made) {
            _ if self.dash_redo.is_some() => format!("{} made again, even where done", crate::generate::redo_summary(self.dash_redo.unwrap_or(Step::Video))),
            (q, _) if q > 0 => format!("{q} already in the queue"),
            (_, m) if m > 0 => format!("{m} already ha{} a video; only missing steps run (use Redo to make steps again)", if m == 1 { "s" } else { "ve" }),
            _ => "Read → Transcript → Story → Audio → Music → Video, skipping steps already made".to_string(),
        };
        let single = (n == 1).then(|| picked[0].id.clone());
        let to_open = picked.to_vec();
        bar.child(status_tag(StatusKind::Info).label(format!("{n} picked")))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(220.))
                    .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(names))
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child(note)),
            )
            .when_some(single, |d, id| {
                d.child(btn("dash-open", "Open in editor", ButtonKind::Ghost).small().icon(glyphs::EDIT).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.go_to(&id, None, cx))))
            })
            .when(made > 0, |d| {
                let items: Vec<ChapterItem> = picked.to_vec();
                let scope = if n == 1 { picked[0].label() } else { format!("{n} chapters") };
                d.child(btn("dash-restyle", "Music & subtitles", ButtonKind::Secondary).small().icon(glyphs::MUSIC).tooltip("Change the music and subtitles of the picked chapters and re-make their videos").on_click(cx.listener(
                    move |this, _: &ClickEvent, _, cx| this.open_restyle(items.clone(), scope.clone(), false, cx),
                )))
            })
            .child(icon_button("dash-clear", ph::X, "Clear the picks").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.dash_pick.clear();
                this.dash_anchor = None;
                cx.notify();
            })))
            .child(self.redo_menu(cx))
            .child(match self.dash_redo {
                // A redo takes every pick, queued or not (a waiting chapter gets the redo added).
                Some(r) => btn("dash-generate", format!("Redo {} from {}", chapters_phrase(picked), r.label()), ButtonKind::Primary)
                    .icon(ph::ARROWS_CLOCKWISE)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let redo = this.dash_redo;
                        this.open_generate(to_open.clone(), redo, cx);
                    })),
                None => btn("dash-generate", if fresh.is_empty() { "Already queued".to_string() } else { format!("Generate {}", chapters_phrase(&fresh)) }, ButtonKind::Primary)
                    .icon(ph::ROCKET_LAUNCH)
                    .when(fresh.is_empty(), |b| b.disabled("Every picked chapter is already in the queue"))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let fresh: Vec<ChapterItem> = to_open.iter().filter(|c| !this.queue.for_chapter(&c.id).is_some_and(|i| i.state.active())).cloned().collect();
                        this.open_generate(fresh, None, cx);
                    })),
            })
            .into_any_element()
    }
}


#[cfg(test)]
mod tests {
    use super::{range, tile_state, Filter, Tile};
    use crate::pipeline::{ItemState, QChapter, Queue};
    use mv_recap::{PhaseStatus, Step};

    #[test]
    fn tiles_follow_the_queue_then_the_files() {
        let mut q = Queue::default();
        let done = PhaseStatus { read_pages: 3, total_pages: 3, transcript: true, story: true, audio_files: 3, video: Some("/v.mp4".into()), ..Default::default() };
        assert_eq!(tile_state("a", Some(&done), &q, None), Tile::Done);
        assert_eq!(tile_state("a", None, &q, None), Tile::NotStarted);
        let part = PhaseStatus { read_pages: 3, total_pages: 3, transcript: true, ..Default::default() };
        assert_eq!(tile_state("a", Some(&part), &q, None), Tile::Partial(2));
        assert_eq!(tile_state("a", Some(&part), &q, Some(Step::Story)), Tile::Running(Step::Story));
        let ch = QChapter { id: "a".into(), manga_id: "m".into(), manga_title: "M".into(), number: Some(1.0), title: None, pages_dir: "/p".into(), output_dir: "/o".into() };
        q.add(ch, Step::Video, None, "landscape", None);
        assert_eq!(tile_state("a", Some(&done), &q, None), Tile::Queued);
        q.items[0].state = ItemState::Failed;
        q.items[0].current = Some(Step::Audio.index());
        assert_eq!(tile_state("a", Some(&part), &q, None), Tile::Failed(Step::Audio));
        assert!(Filter::Failed.matches(Tile::Failed(Step::Audio)));
        assert!(Filter::InProgress.matches(Tile::Queued) && !Filter::InProgress.matches(Tile::Done));
    }

    #[test]
    fn buttons_name_the_picked_chapters() {
        use super::chapters_phrase;
        use crate::data::ChapterItem;
        let ch = |n: f64| ChapterItem {
            id: format!("c{n}"),
            manga_id: "m".into(),
            manga_title: "M".into(),
            number: Some(n),
            title: None,
            pages_dir: Default::default(),
            output_dir: Default::default(),
            page_count: 0,
            has_text: false,
            video: None,
            audio_files: 0,
        };
        // One picked chapter 31 reads "Ch. 31", never "1 chapter" (which read like chapter 1).
        assert_eq!(chapters_phrase(&[ch(31.)]), "Ch. 31");
        assert_eq!(chapters_phrase(&[ch(4.), ch(7.)]), "Ch. 4, Ch. 7");
        assert_eq!(chapters_phrase(&[ch(1.), ch(2.), ch(3.), ch(4.)]), "4 chapters");
    }

    #[test]
    fn shift_click_picks_a_range_either_way() {
        let order: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        assert_eq!(range(&order, "b", "d"), vec!["b", "c", "d"]);
        assert_eq!(range(&order, "d", "b"), vec!["b", "c", "d"]);
        assert_eq!(range(&order, "x", "c"), vec!["c"], "an anchor filtered away picks only the tile");
    }
}
