//! Settings › Brain: a browsable map of what the app has learned about YouTube and thumbnails
//! (`mv_core::brain`). The areas and their kinds filter one list of entries, strongest first;
//! each entry can be edited (which pins it) or removed, and the whole Brain reset. A light timer
//! re-reads it when its revision moves, so learning elsewhere shows up while the page is open.
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::brain::{self, Area, Brain, Entry, Kind};
use mv_ui::settings::{SETTINGS_GROUP_GAP, SETTINGS_ROW_X};
use mv_ui::theme::{icon as icon_px, radius};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, confirm_inline, disclosure, empty_view, filter_chip, icon_button, icon_tag, ph, ph_icon, settings_block, settings_group, settings_note, size, AlertKind, ButtonKind, Hue, Ph, SettingsGroup, Tokens, Tone, Weight};

use crate::ui::{hue_tile, path_row};

/// Entries listed before "Show more".
const PAGE: usize = 50;
/// How often the page looks for new learning.
const POLL: Duration = Duration::from_secs(2);

/// What the map has selected: everything, one area or one kind.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sel {
    All,
    Area(Area),
    Kind(Kind),
}

impl Sel {
    fn includes(self, k: Kind) -> bool {
        match self {
            Sel::All => true,
            Sel::Area(a) => k.area() == a,
            Sel::Kind(x) => x == k,
        }
    }
}

/// The Brain as the page shows it, read off the UI thread.
struct Snapshot {
    rev: u64,
    entries: Vec<Entry>,
    hints: Vec<(Area, String)>,
}

fn snapshot() -> Snapshot {
    brain::read(|b| Snapshot { rev: b.revision, entries: b.entries.clone(), hints: Area::ALL.iter().map(|a| (*a, b.hints(*a))).collect() })
}

fn area_hue(a: Area) -> Hue {
    match a {
        Area::Niche => Hue::Rose,
        Area::Thumbnails => Hue::Amber,
        Area::Taste => Hue::Cyan,
    }
}

fn area_icon(a: Area) -> Ph {
    match a {
        Area::Niche => ph::YOUTUBE_LOGO,
        Area::Thumbnails => ph::IMAGES,
        Area::Taste => ph::HEART,
    }
}

fn kind_icon(k: Kind) -> Ph {
    match k {
        Kind::TitleWord => ph::TEXT_AA,
        Kind::Length => ph::TIMER,
        Kind::Channel => ph::USER_CIRCLE,
        Kind::Trend => ph::TREND_UP,
        Kind::Prompt => ph::NOTE_PENCIL,
        Kind::Style => ph::SLIDERS_HORIZONTAL,
        Kind::Pick => ph::THUMBS_UP,
    }
}

/// The sort key `Brain::top` uses: score times how often it was seen.
fn strength(e: &Entry) -> f32 {
    e.score * (1. + (e.count.max(1) as f32).ln())
}

fn plural(n: i64, unit: &str) -> String {
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// "3 min ago", "2 days ago" for a unix time.
fn ago(t: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    match (now - t).max(0) {
        0..=59 => "just now".into(),
        s @ 60..=3_599 => format!("{} min ago", s / 60),
        s @ 3_600..=86_399 => format!("{} h ago", s / 3_600),
        s @ 86_400..=2_591_999 => plural(s / 86_400, "day"),
        s @ 2_592_000..=31_535_999 => plural(s / 2_592_000, "month"),
        s => plural(s / 31_536_000, "year"),
    }
}

fn seen(count: u32) -> String {
    match count {
        0 | 1 => "Seen once".into(),
        n => format!("Seen {n} times"),
    }
}

/// Diverging bar from the centre: green to the right for "do more", red to the left for "avoid",
/// with the signed value beside it so it never rests on colour alone.
fn score_bar(score: f32) -> Div {
    let half = 36.;
    let w = (score.abs().min(1.) * half).max(if score.abs() > 0.005 { 2. } else { 0. });
    let color = if score > 0.05 {
        Tokens::success()
    } else if score < -0.05 {
        Tokens::danger()
    } else {
        Tokens::text_3()
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .relative()
                .w(px(2. * half))
                .h(px(6.))
                .rounded_full()
                .bg(Tokens::active())
                .child(div().absolute().top_0().h_full().w(px(w)).rounded_full().bg(color).left(px(if score >= 0. { half } else { half - w })))
                .child(div().absolute().top(px(-2.)).left(px(half - 0.5)).w(px(1.)).h(px(10.)).bg(Tokens::border_strong())),
        )
        .child(div().w(px(38.)).text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(color).child(format!("{score:+.2}")))
}

/// Label column of the edit form.
fn field(label: &'static str, el: impl IntoElement) -> Div {
    div().flex().items_center().gap(px(12.)).min_w_0().child(div().w(px(64.)).flex_none().text_size(px(size::SM)).text_color(Tokens::text_2()).child(label)).child(div().flex_1().min_w_0().child(el))
}

pub struct BrainPanel {
    /// `None` until the first read.
    data: Option<Snapshot>,
    sel: Sel,
    search: Entity<InputState>,
    query: String,
    shown: usize,
    /// Entry whose edit form is open, and the form's fields.
    editing: Option<String>,
    subject: Entity<InputState>,
    detail: Entity<InputState>,
    score: Entity<SliderState>,
    score_value: f32,
    /// Entry whose Remove was pressed (the inline confirm is showing).
    confirm_remove: Option<String>,
    confirm_reset: bool,
    _subs: Vec<Subscription>,
}

pub fn create(window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| BrainPanel::new(window, cx)).into()
}

impl BrainPanel {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search the Brain"));
        let subject = cx.new(|cx| InputState::new(window, cx).placeholder("What it is about"));
        let detail = cx.new(|cx| InputState::new(window, cx).placeholder("What was seen, in words"));
        let score = cx.new(|_| SliderState::new().min(-1.).max(1.).step(0.05).default_value(0.));
        let mut subs = vec![];
        subs.push(cx.subscribe_in(&search, window, |this, state, ev: &InputEvent, _, cx| {
            if let InputEvent::Change = ev {
                this.query = state.read(cx).value().to_string();
                this.shown = PAGE;
                cx.notify();
            }
        }));
        for input in [&subject, &detail] {
            subs.push(cx.subscribe_in(input, window, |this, _, ev: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.save_edit(cx);
                }
            }));
        }
        subs.push(cx.subscribe_in(&score, window, |this, _, ev: &SliderEvent, _, cx| match ev {
            SliderEvent::Change(v) | SliderEvent::Release(v) => {
                this.score_value = (v.start() * 20.).round() / 20.;
                cx.notify();
            }
        }));
        // Reads now, then again whenever the revision moves (a YouTube search, a kept thumbnail).
        cx.spawn(async move |this, cx| {
            let mut first = true;
            loop {
                if !first {
                    cx.background_executor().timer(POLL).await;
                }
                first = false;
                let Ok(had) = this.update(cx, |v, _| v.data.as_ref().map(|d| d.rev)) else { break };
                if let Ok(Some(snap)) = mv_ui::bg::blocking(move || (had != Some(brain::revision())).then(snapshot)).await {
                    if this.update(cx, |v, cx| v.apply(snap, cx)).is_err() {
                        break;
                    }
                }
            }
        })
        .detach();
        BrainPanel { data: None, sel: Sel::All, search, query: String::new(), shown: PAGE, editing: None, subject, detail, score, score_value: 0., confirm_remove: None, confirm_reset: false, _subs: subs }
    }

    fn apply(&mut self, snap: Snapshot, cx: &mut Context<Self>) {
        // An entry that went away (removed, reset, trimmed) closes its form.
        if let Some(id) = &self.editing {
            if !snap.entries.iter().any(|e| &e.id == id) {
                self.editing = None;
            }
        }
        self.data = Some(snap);
        cx.notify();
    }

    /// Runs a change to the Brain off the UI thread (it saves the file), then shows the result.
    fn change(&mut self, cx: &mut Context<Self>, done: &'static str, f: impl FnOnce() + Send + 'static) {
        cx.spawn(async move |this, cx| {
            let r = mv_ui::bg::blocking(move || {
                f();
                snapshot()
            })
            .await;
            let _ = this.update(cx, |v, cx| match r {
                Ok(s) => {
                    v.apply(s, cx);
                    toast::show(cx, Toast::success(done));
                }
                Err(_) => toast::show(cx, Toast::error("Couldn't change the Brain.")),
            });
        })
        .detach();
    }

    fn select(&mut self, sel: Sel, cx: &mut Context<Self>) {
        self.sel = sel;
        self.shown = PAGE;
        cx.notify();
    }

    fn start_edit(&mut self, e: &Entry, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = Some(e.id.clone());
        self.confirm_remove = None;
        let (s, d, v) = (e.subject.clone(), e.detail.clone(), e.score);
        self.subject.update(cx, |st, cx| st.set_value(s, window, cx));
        self.detail.update(cx, |st, cx| st.set_value(d, window, cx));
        self.score.update(cx, |st, cx| st.set_value(v, window, cx));
        self.score_value = v;
        cx.notify();
    }

    fn save_edit(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.editing.clone() else { return };
        let subject = self.subject.read(cx).value().trim().to_string();
        if subject.is_empty() {
            toast::show(cx, Toast::warning("Give the entry a subject first."));
            return;
        }
        let detail = self.detail.read(cx).value().trim().to_string();
        let score = self.score_value;
        self.editing = None;
        self.change(cx, "Saved. Learning leaves this entry alone now.", move || {
            brain::write(|b| b.edit(&id, &subject, &detail, score));
        });
    }

    fn remove(&mut self, id: String, cx: &mut Context<Self>) {
        self.confirm_remove = None;
        self.change(cx, "Removed from the Brain", move || {
            brain::write(|b| b.remove(&id));
        });
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        self.confirm_reset = false;
        self.editing = None;
        self.sel = Sel::All;
        self.change(cx, "Brain reset", brain::reset);
    }

    /// The file it lives in, with Show in folder and Reset Brain.
    fn render_file(&self, n: usize, cx: &mut Context<Self>) -> SettingsGroup {
        let control: AnyElement = if self.confirm_reset {
            confirm_inline(format!("Forget all {n}?"), "Reset", true)
                .cancel_label("Keep")
                .on_cancel(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.confirm_reset = false;
                    cx.notify();
                }))
                .on_confirm(cx.listener(|this, _: &ClickEvent, _, cx| this.reset(cx)))
                .into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(btn("brain-reveal", "Show in folder", ButtonKind::Secondary).small().icon(ph::FOLDER_OPEN).on_click(|_, _, cx| {
                    let path = Brain::path();
                    if path.exists() {
                        cx.reveal_path(&path);
                    } else if let Some(dir) = path.parent() {
                        let _ = std::fs::create_dir_all(dir);
                        cx.reveal_path(dir);
                    }
                }))
                .child(
                    btn("brain-reset", "Reset Brain", ButtonKind::DangerGhost)
                        .small()
                        .icon(ph::ARROW_COUNTER_CLOCKWISE)
                        .when(n == 0, |b| b.disabled("Nothing to forget"))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.confirm_reset = true;
                            cx.notify();
                        })),
                )
                .into_any_element()
        };
        let hint = match n {
            0 => "Nothing learned yet. Stays on this PC, never uploaded.".to_string(),
            1 => "1 thing learned. Stays on this PC, never uploaded.".to_string(),
            n => format!("{n} things learned. Stays on this PC, never uploaded."),
        };
        settings_group("Where it lives", None).child(path_row(ph::FILE_TEXT, "Brain file", hint, Brain::path().display().to_string(), control))
    }

    /// The brain map: the Brain at the top, its three areas as branches and their kinds under
    /// each. Every node is a filter for the list below.
    fn render_map(&self, d: &Snapshot, cx: &mut Context<Self>) -> SettingsGroup {
        let of_kind = |k: Kind| d.entries.iter().filter(|e| e.kind == k).count();
        let line = Tokens::border_strong();
        let all = self.sel == Sel::All;
        let root = div()
            .id("brain-root")
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(40.))
            .pl(px(8.))
            .pr(px(14.))
            .rounded(px(radius::PILL))
            .border_1()
            .border_color(if all { Tokens::accent_border() } else { Tokens::border() })
            .bg(if all { Tokens::accent_soft() } else { Tokens::surface() })
            .cursor_pointer()
            .hover(|s| s.border_color(Tokens::border_strong()))
            .role(Role::Button)
            .aria_selected(all)
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.select(Sel::All, cx)))
            .child(hue_tile(ph::BRAIN, Hue::Green, 26., all))
            .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Brain"))
            .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(d.entries.len().to_string()));
        let last = Area::ALL.len() - 1;
        let branches = Area::ALL.iter().copied().enumerate().map(|(i, a)| {
            let picked = self.sel == Sel::Area(a);
            let inside = matches!(self.sel, Sel::Kind(k) if k.area() == a);
            let n: usize = Kind::ALL.iter().filter(|k| k.area() == a).map(|k| of_kind(*k)).sum();
            // Joins this branch to the bar under the Brain node.
            let connector = div()
                .relative()
                .w_full()
                .h(px(16.))
                .child(div().absolute().top_0().h(px(1.)).bg(line).left(if i == 0 { relative(0.5) } else { relative(0.) }).right(if i == last { relative(0.5) } else { relative(0.) }))
                .child(div().absolute().top_0().left(relative(0.5)).w(px(1.)).h_full().bg(line));
            let head = div()
                .id(("brain-area", i))
                .flex()
                .items_center()
                .gap(px(10.))
                .p(px(6.))
                .rounded(px(radius::MD))
                .cursor_pointer()
                .hover(|s| s.bg(Tokens::hover()))
                .role(Role::Button)
                .aria_selected(picked)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select(Sel::Area(a), cx)))
                .child(hue_tile(area_icon(a), area_hue(a), 28., picked))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).truncate().child(a.label()))
                        .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(if n == 1 { "1 entry".to_string() } else { format!("{n} entries") })),
                );
            let kinds = Kind::ALL.iter().copied().enumerate().filter(|(_, k)| k.area() == a).map(|(j, k)| {
                let on = self.sel == Sel::Kind(k);
                filter_chip(("brain-kind", j), k.label(), on)
                    .icon(kind_icon(k))
                    .count(of_kind(k))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select(if on { Sel::Area(a) } else { Sel::Kind(k) }, cx)))
            });
            let card = div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .p(px(6.))
                .rounded(px(radius::LG))
                .border_1()
                .border_color(if picked || inside { Tokens::accent_border() } else { Tokens::border() })
                .bg(Tokens::surface())
                .child(head)
                .child(div().flex().flex_col().items_start().gap(px(6.)).ml(px(20.)).pl(px(12.)).pb(px(4.)).border_l_1().border_color(Tokens::border()).children(kinds));
            // No gap between branches, so the bar over them stays one line.
            div().flex_1().min_w_0().flex().flex_col().child(connector).child(div().px(px(4.)).child(card))
        });
        let map = div()
            .flex()
            .flex_col()
            .items_center()
            .w_full()
            .min_w_0()
            .child(root)
            .child(div().w(px(1.)).h(px(14.)).bg(line))
            .child(div().flex().w_full().min_w_0().children(branches));
        settings_group("Brain map", Some("Pick the Brain, an area or a kind to see what it knows there.".into())).child(settings_block(map))
    }

    fn render_entry(&self, ix: usize, e: &Entry, show_kind: bool, cx: &mut Context<Self>) -> Div {
        let editing = self.editing.as_deref() == Some(e.id.as_str());
        let confirming = self.confirm_remove.as_deref() == Some(e.id.as_str());
        let meta = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(8.))
            .gap_y(px(4.))
            .text_size(px(size::SM))
            .text_color(Tokens::text_3())
            .when(show_kind, |d| d.child(icon_tag(kind_icon(e.kind), e.kind.label(), Tone::Neutral)))
            .when(e.pinned, |d| d.child(icon_tag(ph::LOCK_SIMPLE, "Edited by you", Tone::Accent)))
            .child(format!("{} · last {}", seen(e.count), ago(e.last_seen)))
            .when(!e.source.is_empty(), |d| d.child(format!("· {}", e.source)));
        let text = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(2.))
            .child(div().text_size(px(size::MD)).line_height(px(20.)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(e.subject.clone()))
            .when(!e.detail.is_empty(), |d| d.child(div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_2()).child(e.detail.clone())))
            .child(div().pt(px(4.)).child(meta));
        let actions: AnyElement = if confirming {
            let id = e.id.clone();
            confirm_inline("Remove it?", "Remove", true)
                .cancel_label("Keep")
                .on_cancel(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.confirm_remove = None;
                    cx.notify();
                }))
                .on_confirm(cx.listener(move |this, _: &ClickEvent, _, cx| this.remove(id.clone(), cx)))
                .into_any_element()
        } else {
            let entry = e.clone();
            let id = e.id.clone();
            div()
                .flex()
                .items_center()
                .gap(px(2.))
                .child(icon_button(("brain-edit", ix), ph::PENCIL_SIMPLE, "Edit").small().selected(editing).on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    if this.editing.as_deref() == Some(entry.id.as_str()) {
                        this.editing = None;
                        cx.notify();
                    } else {
                        this.start_edit(&entry, window, cx);
                    }
                })))
                .child(icon_button(("brain-rm", ix), ph::TRASH, "Remove").small().danger().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.confirm_remove = Some(id.clone());
                    cx.notify();
                })))
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .px(px(SETTINGS_ROW_X))
            .py(px(12.))
            .min_w_0()
            .child(div().flex().items_start().gap(px(16.)).min_w_0().child(text).child(div().flex_none().flex().items_center().gap(px(12.)).pt(px(2.)).child(score_bar(e.score)).child(actions)))
            .when(editing, |d| d.child(self.render_form(cx)))
    }

    fn render_form(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(12.))
            .rounded(px(radius::MD))
            .bg(Tokens::well())
            .border_1()
            .border_color(Tokens::border())
            .child(field("Subject", Input::new(&self.subject).small()))
            .child(field("Detail", Input::new(&self.detail).small()))
            .child(field(
                "Score",
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .text_size(px(size::SM))
                    .text_color(Tokens::text_3())
                    .child("Avoid")
                    .child(div().flex_1().min_w(px(120.)).max_w(px(260.)).child(Slider::new(&self.score)))
                    .child("Do more")
                    .child(div().w(px(44.)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("{:+.2}", self.score_value))),
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child("Saving pins it: learning won't change it again."))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(btn("brain-edit-cancel", "Cancel", ButtonKind::Ghost).small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.editing = None;
                                cx.notify();
                            })))
                            .child(btn("brain-edit-save", "Save", ButtonKind::Primary).small().icon(ph::CHECK).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.save_edit(cx)))),
                    ),
            )
    }

    /// Entries in the selection that match the search, strongest first.
    fn render_list(&self, d: &Snapshot, cx: &mut Context<Self>) -> SettingsGroup {
        let q = self.query.trim().to_lowercase();
        let mut v: Vec<&Entry> =
            d.entries.iter().filter(|e| self.sel.includes(e.kind)).filter(|e| q.is_empty() || [&e.subject, &e.detail, &e.source].iter().any(|t| t.to_lowercase().contains(&q))).collect();
        v.sort_by(|a, b| strength(b).total_cmp(&strength(a)));
        let (title, desc): (SharedString, SharedString) = match self.sel {
            Sel::All => ("Everything learned".into(), "Strongest first: how much it helps, times how often it was seen.".into()),
            Sel::Area(a) => (a.label().into(), a.about().into()),
            Sel::Kind(k) => (format!("{} › {}", k.area().label(), k.label()).into(), k.area().about().into()),
        };
        let search = div().w(px(220.)).child(Input::new(&self.search).small().cleanable(true).prefix(ph_icon(mv_ui::glyphs::SEARCH, Weight::Bold, Tokens::text_3(), icon_px::SM)));
        let mut g = settings_group(title, Some(desc)).trailing(search);
        if v.is_empty() {
            let msg = if q.is_empty() { "Nothing learned here yet.".to_string() } else { format!("Nothing here matches \u{201c}{}\u{201d}.", self.query.trim()) };
            return g.child(settings_block(div().py(px(8.)).text_size(px(size::SM)).text_color(Tokens::text_3()).child(msg)));
        }
        let show_kind = !matches!(self.sel, Sel::Kind(_));
        for (ix, e) in v.iter().take(self.shown).enumerate() {
            g = g.child(self.render_entry(ix, e, show_kind, cx));
        }
        if v.len() > self.shown {
            let more = (v.len() - self.shown).min(PAGE);
            g = g.child(settings_block(
                div().flex().items_center().justify_between().gap(px(8.)).child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(format!("{} of {}", self.shown, v.len()))).child(
                    btn("brain-more", format!("Show {more} more"), ButtonKind::Ghost).small().icon(ph::CARET_DOWN).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.shown += PAGE;
                        cx.notify();
                    })),
                ),
            ));
        }
        g
    }

    /// What `brain::hints` adds to AI prompts, per area.
    fn render_hints(&self, d: &Snapshot) -> SettingsGroup {
        let mut g = settings_group("What the app tells the AI", Some("The lines the Brain adds to AI prompts for titles, thumbnails and suggestions.".into()));
        for (i, (a, h)) in d.hints.iter().enumerate() {
            let body = if h.is_empty() {
                div().text_size(px(size::SM)).text_color(Tokens::text_3()).child("Nothing learned yet")
            } else {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .p(px(10.))
                    .rounded(px(radius::SM))
                    .bg(Tokens::well())
                    .font_family(mv_ui::MONO_FONT)
                    .text_size(px(size::SM))
                    .line_height(px(18.))
                    .text_color(Tokens::text_2())
                    .children(h.lines().map(|l| div().child(l.to_string())))
            };
            let lines = h.lines().count();
            g = g.child(settings_block(
                disclosure(("brain-hints", i), a.label())
                    .trailing(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(match lines {
                        0 => "Empty".to_string(),
                        1 => "1 line".to_string(),
                        n => format!("{n} lines"),
                    }))
                    .content(body),
            ));
        }
        g
    }
}

impl Render for BrainPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = div()
            .id("brain-panel")
            .flex()
            .flex_col()
            .gap(px(SETTINGS_GROUP_GAP))
            .w_full()
            .min_w_0()
            .child(
                settings_note(
                    AlertKind::Info,
                    "The Brain is what MangaVox learns as you use it: what gets views around your manga on YouTube, how you make thumbnails and what you pick. The app uses it to rank suggestions and to guide its AI prompts. It is one file on this PC and is never uploaded.",
                )
                .icon(ph::BRAIN),
            )
            .when(mv_core::privacy::active(), |d| d.child(settings_note(AlertKind::Warning, "Private mode is on: the Brain learns nothing new until you turn it off.")));
        let Some(d) = self.data.as_ref() else {
            return page.children((0..4).map(|_| mv_ui::skeleton_row()));
        };
        let page = page.child(self.render_file(d.entries.len(), cx));
        if d.entries.is_empty() {
            return page.child(empty_view(ph::BRAIN, "Nothing learned yet", "The Brain fills up as you use YouTube suggestions, Thumbnails and YouTube videos. What it learns shows up here."));
        }
        page.child(self.render_map(d, cx)).child(self.render_list(d, cx)).child(self.render_hints(d))
    }
}
