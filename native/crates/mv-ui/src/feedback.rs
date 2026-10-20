//! Status and feedback: empty and error states, inline alerts and banners, skeletons, status
//! tags, tags and badges, meters and progress rings, the stepper, disclosure sections and
//! confirmations. Toasts live in [`crate::toast`].
use std::rc::Rc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::controls::{btn, ButtonKind, ClickHandler, FlagHandler, IndexHandler, WindowHandler};
use crate::icons::{ph, Ph, Weight};
use crate::interact::{element_builder, glyph, refine, render_once_element, tip, IconSrc, Interactive};
use crate::motion::{appear, shimmer, spinner};
use crate::theme::{icon as icon_px, radius, ramp, size, RampExt, Tokens};

fn name_id(prefix: &str, key: &str) -> ElementId {
    ElementId::Name(SharedString::from(format!("{prefix}-{key}")))
}

// ── Empty and error states ──────────────────────────────────────────────────────────────────

/// Empty state: a 40px duotone icon in a 64px tinted disc, a title, one line of guidance and up
/// to two actions. Say what is empty and what to do next.
/// NN/g empty states: <https://www.nngroup.com/articles/empty-state-interface-design/>
pub fn empty_view(icon: Ph, title: impl Into<SharedString>, body: impl Into<SharedString>) -> EmptyState {
    let title = title.into();
    let body: SharedString = body.into();
    EmptyState { icon, title, body: (!body.is_empty()).then_some(body), danger: false, primary: None, secondary: None, compact: false, style: StyleRefinement::default() }
}

/// Error state in the same layout, in danger colours. Add `.retry(handler)` for a Try again
/// button. Say what failed in plain words and how to recover.
/// NN/g error messages: <https://www.nngroup.com/articles/error-message-guidelines/>
pub fn error_view(message: impl Into<SharedString>) -> EmptyState {
    let mut e = empty_view(ph::WARNING_CIRCLE, "Something went wrong", message);
    e.danger = true;
    e
}

/// See [`empty_view`].
pub struct EmptyState {
    icon: Ph,
    title: SharedString,
    body: Option<SharedString>,
    danger: bool,
    primary: Option<AnyElement>,
    secondary: Option<AnyElement>,
    compact: bool,
    style: StyleRefinement,
}

impl Styled for EmptyState {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl IntoElement for EmptyState {
    type Element = Stateful<Div>;
    fn into_element(self) -> Stateful<Div> {
        self.build()
    }
}

impl EmptyState {
    /// Main action, usually `btn(.., ButtonKind::Primary)`.
    pub fn action(mut self, el: impl IntoElement) -> Self {
        self.primary = Some(el.into_any_element());
        self
    }

    /// Second action, usually a ghost button.
    pub fn secondary(mut self, el: impl IntoElement) -> Self {
        self.secondary = Some(el.into_any_element());
        self
    }

    /// Adds a "Try again" button as the main action.
    pub fn retry(self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.action(btn("mv-retry", "Try again", ButtonKind::Secondary).icon(ph::ARROW_CLOCKWISE).on_click(handler))
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = title.into();
        self
    }

    /// Smaller icon and height for panels, docks and list sections.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    pub fn build(self) -> Stateful<Div> {
        let EmptyState { icon, title, body, danger, primary, secondary, compact, style } = self;
        let (disc, ic, min_h) = if compact { (48., 28., 160.) } else { (64., icon_px::EMPTY, 260.) };
        let (disc_bg, fg) = if danger { (Tokens::danger_soft(), Tokens::danger()) } else { (Tokens::active(), Tokens::text_2()) };
        let has_actions = primary.is_some() || secondary.is_some();
        let el = div()
            .id(name_id("mv-empty", &title))
            .flex()
            .flex_col()
            .w_full()
            .min_h(px(min_h))
            .items_center()
            .justify_center()
            .gap(px(8.))
            .p(px(16.))
            .child(div().flex().items_center().justify_center().size(px(disc)).mb(px(8.)).rounded_full().bg(disc_bg).child(glyph(IconSrc::Ph(icon), Weight::Duotone, fg, ic)))
            .child(div().ramp(if compact { ramp::BODY_STRONG } else { ramp::SUBTITLE_2 }).text_color(Tokens::text_1()).text_center().child(title))
            .when_some(body, |d, b| d.child(div().max_w(px(420.)).text_center().text_size(px(size::MD)).line_height(px(20.)).text_color(Tokens::text_3()).child(b)))
            .when(has_actions, |d| d.child(div().flex().flex_wrap().justify_center().gap(px(8.)).mt(px(8.)).children(primary).children(secondary)));
        refine(el, &style)
    }
}

// ── Inline alerts and banners ───────────────────────────────────────────────────────────────

/// Severity of an alert, banner or toast. Status colours are for status only.
/// NN/g indicators: <https://www.nngroup.com/articles/indicators-validations-notifications/>
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AlertKind {
    Info,
    Success,
    Warning,
    Error,
}

impl AlertKind {
    /// (colour, soft fill, border, icon).
    pub fn colors(self) -> (Rgba, Rgba, Rgba, Ph) {
        match self {
            AlertKind::Info => (Tokens::info(), Tokens::info_soft(), Tokens::info().alpha(0.35), ph::INFO),
            AlertKind::Success => (Tokens::success(), Tokens::success_soft(), Tokens::success().alpha(0.35), ph::CHECK_CIRCLE),
            AlertKind::Warning => (Tokens::warning(), Tokens::warning_soft(), Tokens::warning_border(), ph::WARNING),
            AlertKind::Error => (Tokens::danger(), Tokens::danger_soft(), Tokens::danger_border(), ph::WARNING_CIRCLE),
        }
    }
}

/// Inline alert inside a page or panel: status icon, title, optional body and action, tinted by
/// severity. For problems that stay until fixed; use a toast for passing confirmations.
/// Fluent 2 MessageBar: <https://fluent2.microsoft.design/components/web/react/core/messagebar/usage>
pub fn inline_alert(kind: AlertKind, title: impl Into<SharedString>, body: impl Into<SharedString>) -> Alert {
    let body: SharedString = body.into();
    Alert { kind, title: title.into(), body: (!body.is_empty()).then_some(body), banner: false, action: None, dismiss: None, style: StyleRefinement::default() }
}

/// Full-width banner across the top of a page or panel (no rounding, bottom border).
pub fn banner(kind: AlertKind, text: impl Into<SharedString>) -> Alert {
    let mut a = inline_alert(kind, text, "");
    a.banner = true;
    a
}

/// See [`inline_alert`].
pub struct Alert {
    kind: AlertKind,
    title: SharedString,
    body: Option<SharedString>,
    banner: bool,
    action: Option<(SharedString, ClickHandler)>,
    dismiss: Option<ClickHandler>,
    style: StyleRefinement,
}

impl Styled for Alert {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl IntoElement for Alert {
    type Element = Stateful<Div>;
    fn into_element(self) -> Stateful<Div> {
        self.build()
    }
}

impl Alert {
    /// One text action ("Retry", "Open settings").
    pub fn action(mut self, label: impl Into<SharedString>, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.action = Some((label.into(), Rc::new(handler)));
        self
    }

    /// Adds a close button that calls `handler`.
    pub fn dismissible(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.dismiss = Some(Rc::new(handler));
        self
    }

    pub fn build(self) -> Stateful<Div> {
        let Alert { kind, title, body, banner, action, dismiss, style } = self;
        let (fg, soft, edge, icon) = kind.colors();
        let el = div()
            .id(name_id("mv-alert", &title))
            .flex()
            .items_start()
            .gap(px(10.))
            .w_full()
            .bg(soft)
            .role(if kind == AlertKind::Error { Role::Alert } else { Role::Status })
            .map(|d| if banner { d.px(px(16.)).py(px(10.)).border_b_1().border_color(edge) } else { d.p(px(12.)).rounded(px(radius::MD)).border_1().border_color(edge) })
            .child(div().pt(px(2.)).flex_none().child(glyph(IconSrc::Ph(icon), Weight::Fill, fg, icon_px::MD)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(div().text_size(px(size::MD)).line_height(px(20.)).font_weight(if body.is_some() { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }).text_color(Tokens::text_1()).child(title))
                    .when_some(body, |d, b| d.child(div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_2()).child(b))),
            )
            .when_some(action, |d, (label, f)| {
                d.child(btn("mv-alert-action", label, ButtonKind::Ghost).small().text_color(fg).on_click(move |e, w, cx| f(e, w, cx)))
            })
            .when_some(dismiss, |d, f| {
                d.child(crate::controls::icon_button("mv-alert-close", ph::X, "Dismiss").small().on_click(move |e, w, cx| f(e, w, cx)))
            });
        refine(el, &style)
    }
}

// ── Skeletons ───────────────────────────────────────────────────────────────────────────────

/// Placeholder block with a shimmer (static under reduced motion). Shape skeletons like the
/// content they stand in for. Apple HIG loading: <https://developer.apple.com/design/human-interface-guidelines/loading>;
/// NN/g skeleton screens: <https://www.nngroup.com/articles/skeleton-screens/>
pub fn skeleton(w: impl Into<Length> + Clone, h: impl Into<Length> + Clone, corner: f32) -> Div {
    div().relative().flex_none().overflow_hidden().w(w).h(h).rounded(px(corner)).bg(Tokens::surface_2()).child(shimmer())
}

/// Text with a light sweeping across it, for "Thinking…" and "Reading pages…" while the length of
/// the wait is unknown (gpui-component `ShimmerText`; still text under reduced motion).
pub fn shimmer_text(text: impl Into<SharedString>) -> gpui_kit::component::shimmer::ShimmerText {
    gpui_kit::component::shimmer::ShimmerText::new(text).highlight_color(Tokens::text_1())
}

/// One line of text (12px tall).
pub fn skeleton_line(w: impl Into<Length> + Clone) -> Div {
    skeleton(w, px(12.), radius::XS)
}

/// A block (image, chart, panel) with medium corners.
pub fn skeleton_block(w: impl Into<Length> + Clone, h: impl Into<Length> + Clone) -> Div {
    skeleton(w, h, radius::MD)
}

/// A paragraph of `lines` lines, the last one shorter.
pub fn skeleton_text(lines: usize) -> Div {
    div().flex().flex_col().gap(px(8.)).w_full().children((0..lines.max(1)).map(|i| skeleton_line(if i + 1 == lines.max(1) && lines > 1 { relative(0.6) } else { relative(1.) })))
}

/// Shaped like [`crate::cover_card`]: 150x214 poster, title and subtitle lines.
pub fn skeleton_card() -> Div {
    div().flex().flex_col().gap(px(8.)).w(px(150.)).child(skeleton(px(150.), px(214.), 10.)).child(skeleton_line(px(118.))).child(skeleton_line(px(72.)).h(px(10.)))
}

/// A list row: 40px thumbnail, two lines of text.
pub fn skeleton_row() -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .w_full()
        .h(px(56.))
        .px(px(8.))
        .child(skeleton(px(40.), px(40.), radius::SM))
        .child(div().flex().flex_col().gap(px(8.)).flex_1().child(skeleton_line(relative(0.55))).child(skeleton_line(relative(0.3)).h(px(10.))))
}

// ── Status tags, tags and badges ────────────────────────────────────────────────────────────

/// Job state for [`status_tag`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusKind {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
    Info,
}

impl StatusKind {
    pub fn label(self) -> &'static str {
        match self {
            StatusKind::Queued => "Queued",
            StatusKind::Running => "Running",
            StatusKind::Done => "Done",
            StatusKind::Failed => "Failed",
            StatusKind::Cancelled => "Cancelled",
            StatusKind::Info => "Info",
        }
    }

    /// (text colour, fill, icon). Running draws a spinner instead of its icon.
    pub fn colors(self) -> (Rgba, Rgba, Ph) {
        match self {
            StatusKind::Queued => (Tokens::text_2(), Tokens::active(), ph::HOURGLASS_MEDIUM),
            StatusKind::Running => (Tokens::accent(), Tokens::accent_soft(), ph::CIRCLE_NOTCH),
            StatusKind::Done => (Tokens::success(), Tokens::success_soft(), ph::CHECK_CIRCLE),
            StatusKind::Failed => (Tokens::danger(), Tokens::danger_soft(), ph::X_CIRCLE),
            StatusKind::Cancelled => (Tokens::text_3(), Tokens::hover(), ph::PROHIBIT),
            StatusKind::Info => (Tokens::info(), Tokens::info_soft(), ph::INFO),
        }
    }
}

/// Job status pill with an icon and a word, so status never rests on colour alone
/// (WCAG 1.4.1: <https://www.w3.org/WAI/WCAG22/Understanding/use-of-color>). `.label(..)`
/// replaces the word, e.g. "Rendering 3/8".
pub fn status_tag(kind: StatusKind) -> StatusTag {
    StatusTag { kind, label: None, style: StyleRefinement::default() }
}

/// See [`status_tag`].
pub struct StatusTag {
    kind: StatusKind,
    label: Option<SharedString>,
    style: StyleRefinement,
}

impl Styled for StatusTag {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl IntoElement for StatusTag {
    type Element = Div;
    fn into_element(self) -> Div {
        self.build()
    }
}

impl StatusTag {
    pub fn label(mut self, text: impl Into<SharedString>) -> Self {
        self.label = Some(text.into());
        self
    }

    pub fn build(self) -> Div {
        let (fg, bg, icon) = self.kind.colors();
        let el = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(5.))
            .h(px(22.))
            .px(px(8.))
            .rounded(px(radius::PILL))
            .bg(bg)
            .text_size(px(size::XS))
            .font_weight(FontWeight::MEDIUM)
            .text_color(fg)
            .whitespace_nowrap()
            .map(|d| {
                if self.kind == StatusKind::Running {
                    d.child(spinner("mv-status-spin", fg, icon_px::XS))
                } else {
                    d.child(glyph(IconSrc::Ph(icon), if self.kind == StatusKind::Done || self.kind == StatusKind::Failed { Weight::Fill } else { Weight::Bold }, fg, icon_px::XS))
                }
            })
            .child(self.label.unwrap_or_else(|| self.kind.label().into()));
        refine(el, &self.style)
    }
}

/// Colour family for [`tag`] and [`badge`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Neutral,
    Accent,
    Success,
    Info,
    Warning,
    Danger,
    Favorite,
}

impl Tone {
    /// (colour, soft fill).
    fn colors(self) -> (Rgba, Rgba) {
        match self {
            Tone::Neutral => (Tokens::text_2(), Tokens::active()),
            Tone::Accent => (Tokens::accent(), Tokens::accent_soft()),
            Tone::Success => (Tokens::success(), Tokens::success_soft()),
            Tone::Info => (Tokens::info(), Tokens::info_soft()),
            Tone::Warning => (Tokens::warning(), Tokens::warning_soft()),
            Tone::Danger => (Tokens::danger(), Tokens::danger_soft()),
            Tone::Favorite => (Tokens::favorite(), Tokens::favorite().alpha(0.14)),
        }
    }
}

fn tag_shell(tone: Tone) -> Div {
    let (fg, bg) = tone.colors();
    div().flex().flex_none().items_center().gap(px(4.)).h(px(20.)).px(px(6.)).rounded(px(radius::XS)).bg(bg).text_size(px(size::XS)).font_weight(FontWeight::MEDIUM).text_color(fg).whitespace_nowrap()
}

/// Small tonal label for metadata ("1080p", "NEW", "Local"). Soft fill, coloured text.
pub fn tag(text: impl Into<SharedString>, tone: Tone) -> Div {
    tag_shell(tone).child(text.into())
}

/// [`tag`] with a leading icon.
pub fn icon_tag(icon: Ph, text: impl Into<SharedString>, tone: Tone) -> Div {
    tag_shell(tone).pl(px(5.)).child(glyph(IconSrc::Ph(icon), Weight::Bold, tone.colors().0, icon_px::XS)).child(text.into())
}

/// Solid high-emphasis pill ("NEW" on a cover, "Default"). Use sparingly.
pub fn badge(text: impl Into<SharedString>, tone: Tone) -> Div {
    let (bg, fg) = match tone {
        Tone::Neutral => (Tokens::surface_3(), Tokens::text_1()),
        Tone::Accent => (Tokens::accent(), Tokens::on_accent()),
        other => (other.colors().0, Tokens::base()),
    };
    div().flex().flex_none().items_center().h(px(18.)).px(px(6.)).rounded(px(radius::PILL)).bg(bg).text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(fg).whitespace_nowrap().child(text.into())
}

/// Unread / pending count pill; `None` for zero so it can go straight into `.children(..)`.
/// Caps at "99+".
pub fn count_badge(n: usize) -> Option<Div> {
    (n > 0).then(|| {
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .min_w(px(18.))
            .h(px(18.))
            .px(px(5.))
            .rounded(px(radius::PILL))
            .bg(Tokens::accent())
            .text_size(px(size::XS))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(Tokens::on_accent())
            .child(count_text(n))
    })
}

pub(crate) fn count_text(n: usize) -> String {
    if n > 99 {
        "99+".into()
    } else {
        n.to_string()
    }
}

// ── Meters and rings ────────────────────────────────────────────────────────────────────────

/// Segmented rating bar (quality, speed, size): `max` segments with `value` of them lit, e.g.
/// `meter(3.0, 5)`. Halves round to the nearest segment.
pub fn meter(value: f32, max: usize) -> Div {
    let lit = value.round().clamp(0., max as f32) as usize;
    div().flex().flex_none().items_center().gap(px(2.)).children((0..max).map(|i| div().w(px(10.)).h(px(4.)).rounded_full().bg(if i < lit { Tokens::accent() } else { Tokens::active() })))
}

/// Stacked usage bar (Storage): one segment per `(amount, colour)`, each sized by its share of
/// `total` (or of the sum, when that is larger), on a quiet 8px track (free space). Use
/// `Tokens::hue(..)` for the colours and name each segment in a legend next to it.
/// Apple HIG charts (part-to-whole): <https://developer.apple.com/design/human-interface-guidelines/charts>
pub fn usage_bar(segments: &[(f32, Rgba)], total: f32) -> Div {
    let shares = usage_shares(&segments.iter().map(|s| s.0).collect::<Vec<_>>(), total);
    div()
        .flex()
        .w_full()
        .h(px(8.))
        .rounded_full()
        .overflow_hidden()
        .bg(Tokens::active())
        .children(segments.iter().zip(shares).filter(|(_, w)| *w > 0.).map(|((_, color), w)| div().flex_none().h_full().w(relative(w)).bg(*color)))
}

/// Each amount's share of `total` (or of the sum when larger), so the shares never add up past 1.
pub(crate) fn usage_shares(amounts: &[f32], total: f32) -> Vec<f32> {
    let clean: Vec<f32> = amounts.iter().map(|a| if a.is_finite() { a.max(0.) } else { 0. }).collect();
    let whole = clean.iter().sum::<f32>().max(if total.is_finite() { total } else { 0. });
    clean.into_iter().map(|a| if whole > 0. { a / whole } else { 0. }).collect()
}

/// Score with a filled gold star: "★ 8.4". `icon_size` 14 for cards and rows, 16 to 20 on detail
/// pages; the number follows at a matching size in `text_1`, semibold.
pub fn rating_badge(score: f32, icon_size: f32) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px((icon_size / 4.).round().max(3.)))
        .text_size(px((icon_size - 1.).max(size::XS)))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(Tokens::text_1())
        .whitespace_nowrap()
        .child(glyph(IconSrc::Ph(ph::STAR), Weight::Fill, Tokens::rating(), icon_size))
        .child(fmt_score(score))
}

/// "8.4" for a 0-10 score, "86" for a 0-100 one, "–" when unknown.
pub(crate) fn fmt_score(score: f32) -> String {
    if !score.is_finite() || score < 0. {
        "–".into()
    } else if score > 10. {
        format!("{}", score.round() as u32)
    } else {
        format!("{score:.1}")
    }
}

/// Determinate circular progress, 20px. For an unknown duration use [`crate::spinner`].
pub fn progress_ring(p: f32) -> AnyElement {
    progress_ring_sized(p, 20.)
}

/// [`progress_ring`] at any size (stroke scales with it).
pub fn progress_ring_sized(p: f32, px_size: f32) -> AnyElement {
    let p = if p.is_finite() { p.clamp(0., 1.) } else { 0. };
    let stroke = (px_size / 8.).max(2.);
    let (track, fill) = (Tokens::active(), Tokens::accent());
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let c = bounds.center();
            let r = (bounds.size.width.min(bounds.size.height) - px(stroke)) / 2.;
            let top = point(c.x, c.y - r);
            let bottom = point(c.x, c.y + r);
            let ring = |builder: &mut PathBuilder| {
                builder.move_to(top);
                builder.arc_to(point(r, r), px(0.), false, true, bottom);
                builder.arc_to(point(r, r), px(0.), false, true, top);
            };
            let mut b = PathBuilder::stroke(px(stroke));
            ring(&mut b);
            if let Ok(path) = b.build() {
                window.paint_path(path, Hsla::from(track));
            }
            if p <= 0. {
                return;
            }
            let mut b = PathBuilder::stroke(px(stroke));
            if p >= 0.999 {
                ring(&mut b);
            } else {
                let a = p * std::f32::consts::TAU;
                let end = point(c.x + r * a.sin(), c.y - r * a.cos());
                b.move_to(top);
                b.arc_to(point(r, r), px(0.), p > 0.5, true, end);
            }
            if let Ok(path) = b.build() {
                window.paint_path(path, Hsla::from(fill));
            }
        },
    )
    .flex_none()
    .size(px(px_size))
    .into_any_element()
}

// ── Stepper ─────────────────────────────────────────────────────────────────────────────────

/// What a step has reached. `current` (the step on screen) is separate: any state can be current.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StepState {
    Upcoming,
    Done,
    Running,
    Failed,
}

/// Step `ix`'s state: failed beats running beats done.
pub fn step_state(ix: usize, done: &[bool], running: Option<usize>, failed: Option<usize>) -> StepState {
    if failed == Some(ix) {
        StepState::Failed
    } else if running == Some(ix) {
        StepState::Running
    } else if done.get(ix).copied().unwrap_or(false) {
        StepState::Done
    } else {
        StepState::Upcoming
    }
}

/// The line between step `ix` and `ix + 1`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Connector {
    /// Step `ix` is done: solid accent.
    Done,
    /// Leads into the running or current step: half-strength accent.
    Active,
    Idle,
}

pub fn connector_state(ix: usize, current: usize, done: &[bool], running: Option<usize>, failed: Option<usize>) -> Connector {
    if step_state(ix, done, running, failed) == StepState::Done {
        Connector::Done
    } else if running == Some(ix + 1) || current == ix + 1 {
        Connector::Active
    } else {
        Connector::Idle
    }
}

/// Horizontal stepper for multi-phase work (Studio's read, story, voice, video): each step shows
/// its phase icon, or a check when done, a spinner while running and a cross when failed; the
/// current step gets a halo and strong label; connectors fill as steps complete; every step has
/// a tooltip. Carbon progress indicator: <https://carbondesignsystem.com/components/progress-indicator/usage/>
pub fn stepper<L: Into<SharedString>>(
    id: impl Into<ElementId>,
    steps: impl IntoIterator<Item = (Ph, L)>,
    current: usize,
    done: &[bool],
    running: Option<usize>,
    failed: Option<usize>,
) -> Stepper {
    Stepper {
        base: div().id(id),
        style: StyleRefinement::default(),
        steps: steps.into_iter().map(|(i, l)| (i, l.into())).collect(),
        current,
        done: done.to_vec(),
        running,
        failed,
        tooltips: Vec::new(),
        compact: false,
        on_select: None,
    }
}

/// See [`stepper`].
pub struct Stepper {
    base: Stateful<Div>,
    style: StyleRefinement,
    steps: Vec<(Ph, SharedString)>,
    current: usize,
    done: Vec<bool>,
    running: Option<usize>,
    failed: Option<usize>,
    tooltips: Vec<SharedString>,
    compact: bool,
    on_select: Option<IndexHandler>,
}

element_builder!(Stepper => Stateful<Div>);

impl Stepper {
    /// Makes steps clickable; called with the step index.
    pub fn on_select(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    /// Per-step tooltip text (defaults to "Label: state").
    pub fn tooltips(mut self, tips: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.tooltips = tips.into_iter().map(Into::into).collect();
        self
    }

    /// Icons only; labels move into the tooltips.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    pub fn build(self) -> Stateful<Div> {
        let Stepper { base, style, steps, current, done, running, failed, tooltips, compact, on_select } = self;
        let n = steps.len();
        let mut row = base.flex().items_center().w_full().min_w_0().role(Role::List);
        for (i, (icon, label)) in steps.into_iter().enumerate() {
            let state = step_state(i, &done, running, failed);
            let is_current = i == current;
            let word = match state {
                StepState::Done => "Done",
                StepState::Running => "Running",
                StepState::Failed => "Failed",
                StepState::Upcoming => "Not started",
            };
            let tip_text = tooltips.get(i).cloned().unwrap_or_else(|| format!("{label}: {word}").into());
            let disc = div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(24.))
                .rounded_full()
                .when(is_current, |d| d.shadow(vec![BoxShadow { color: Tokens::accent_a(0.35).into(), offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(3.), inset: false }]))
                .map(|d| match state {
                    StepState::Done => d.bg(Tokens::accent()).child(glyph(IconSrc::Ph(ph::CHECK), Weight::Bold, Tokens::on_accent(), icon_px::XS)),
                    StepState::Running => d.bg(Tokens::accent_soft()).child(spinner(("mv-step-spin", i), Tokens::accent(), icon_px::SM)),
                    StepState::Failed => d.bg(Tokens::danger_soft()).border_1().border_color(Tokens::danger_border()).child(glyph(IconSrc::Ph(ph::X), Weight::Bold, Tokens::danger(), icon_px::XS)),
                    StepState::Upcoming => d.border_1().border_color(if is_current { Tokens::accent() } else { Tokens::track() }).child(glyph(IconSrc::Ph(icon), Weight::Bold, if is_current { Tokens::text_1() } else { Tokens::text_3() }, icon_px::XS)),
                });
            let label_color = match (state, is_current) {
                (StepState::Failed, _) => Tokens::danger(),
                (_, true) => Tokens::text_1(),
                (StepState::Done | StepState::Running, false) => Tokens::text_2(),
                (StepState::Upcoming, false) => Tokens::text_3(),
            };
            let step = div()
                .id(("mv-step", i))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(8.))
                .px(px(6.))
                .py(px(4.))
                .rounded(px(radius::MD))
                .role(Role::ListItem)
                .aria_label(tip_text.clone())
                .aria_selected(is_current)
                .tooltip(tip(tip_text))
                .when_some(on_select.clone(), |d, f| d.interactive().on_click(move |_, w, cx| f(&i, w, cx)))
                .child(disc)
                .when(!compact, |d| d.child(div().text_size(px(size::SM)).font_weight(if is_current { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM }).text_color(label_color).whitespace_nowrap().child(label)));
            row = row.child(step);
            if i + 1 < n {
                let color = match connector_state(i, current, &done, running, failed) {
                    Connector::Done => Tokens::accent(),
                    Connector::Active => Tokens::accent_a(0.45),
                    Connector::Idle => Tokens::border_strong(),
                };
                row = row.child(div().flex_1().min_w(px(12.)).h(px(2.)).mx(px(4.)).rounded_full().bg(color));
            }
        }
        refine(row, &style)
    }
}

// ── Disclosure ──────────────────────────────────────────────────────────────────────────────

/// Collapsible section (logs, "Manual install", advanced options): a caret row that toggles its
/// content. Keeps its own open state unless you pass `.open(..)`.
/// NN/g progressive disclosure: <https://www.nngroup.com/articles/progressive-disclosure/>
pub fn disclosure(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Disclosure {
    let id = id.into();
    Disclosure { base: div().id(id.clone()), id, style: StyleRefinement::default(), title: title.into(), content: None, trailing: None, default_open: false, open: None, on_toggle: None }
}

/// See [`disclosure`].
pub struct Disclosure {
    id: ElementId,
    base: Stateful<Div>,
    style: StyleRefinement,
    title: SharedString,
    content: Option<AnyElement>,
    trailing: Option<AnyElement>,
    default_open: bool,
    open: Option<bool>,
    on_toggle: Option<FlagHandler>,
}

render_once_element!(Disclosure);

impl Disclosure {
    pub fn content(mut self, el: impl IntoElement) -> Self {
        self.content = Some(el.into_any_element());
        self
    }

    /// Element at the end of the header row (a count, a copy button).
    pub fn trailing(mut self, el: impl IntoElement) -> Self {
        self.trailing = Some(el.into_any_element());
        self
    }

    /// Initial state when it manages itself.
    pub fn default_open(mut self, open: bool) -> Self {
        self.default_open = open;
        self
    }

    /// Controlled state: you keep `open` and update it in `on_toggle`.
    pub fn open(mut self, open: bool) -> Self {
        self.open = Some(open);
        self
    }

    pub fn on_toggle(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Disclosure {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Disclosure { id, base, style, title, content, trailing, default_open, open, on_toggle } = self;
        let state = window.use_keyed_state(id.clone(), cx, |_, _| default_open);
        let controlled = open.is_some();
        let is_open = open.unwrap_or(*state.read(cx));
        let header = div()
            .id("mv-disclosure-head")
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .px(px(6.))
            .rounded(px(radius::SM))
            .text_size(px(size::SM))
            .font_weight(FontWeight::MEDIUM)
            .text_color(Tokens::text_2())
            .group("mv-disclosure")
            .role(Role::Button)
            .aria_expanded(is_open)
            .pressable()
            .hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
            .on_click(move |_, w, cx| {
                if !controlled {
                    state.update(cx, |s, cx| {
                        *s = !is_open;
                        cx.notify();
                    });
                }
                if let Some(f) = &on_toggle {
                    f(&!is_open, w, cx);
                }
            })
            .child(glyph(IconSrc::Ph(if is_open { ph::CARET_DOWN } else { ph::CARET_RIGHT }), Weight::Bold, Tokens::text_3(), icon_px::SM).group_hover("mv-disclosure", |s| s.text_color(Tokens::text_1())))
            .child(div().flex_1().min_w_0().truncate().child(title))
            .children(trailing);
        let body = content.filter(|_| is_open).map(|c| appear(("mv-disclosure-body", id_hash(&id)), div().pl(px(26.)).pt(px(4.)).child(c)));
        refine(base.flex().flex_col().child(header).children(body), &style)
    }
}

fn id_hash(id: &ElementId) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    h.finish()
}

// ── Confirmations ───────────────────────────────────────────────────────────────────────────

/// Inline two-step confirm that replaces a destructive button in place: "Delete this chat?
/// [Cancel] [Delete]". The caller keeps the "confirming" flag. Prefer Undo (a toast) for
/// reversible actions; confirm only what cannot be undone.
/// NN/g confirmation dialogs: <https://www.nngroup.com/articles/confirmation-dialog/>
pub fn confirm_inline(prompt: impl Into<SharedString>, confirm_label: impl Into<SharedString>, danger: bool) -> ConfirmInline {
    ConfirmInline { prompt: prompt.into(), confirm_label: confirm_label.into(), cancel_label: "Cancel".into(), danger, on_confirm: None, on_cancel: None, style: StyleRefinement::default() }
}

/// See [`confirm_inline`].
pub struct ConfirmInline {
    prompt: SharedString,
    confirm_label: SharedString,
    cancel_label: SharedString,
    danger: bool,
    on_confirm: Option<ClickHandler>,
    on_cancel: Option<ClickHandler>,
    style: StyleRefinement,
}

impl Styled for ConfirmInline {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl IntoElement for ConfirmInline {
    type Element = Stateful<Div>;
    fn into_element(self) -> Stateful<Div> {
        self.build()
    }
}

impl ConfirmInline {
    pub fn on_confirm(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_confirm = Some(Rc::new(handler));
        self
    }

    pub fn on_cancel(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_cancel = Some(Rc::new(handler));
        self
    }

    /// Outcome label for the way back, e.g. "Keep".
    pub fn cancel_label(mut self, label: impl Into<SharedString>) -> Self {
        self.cancel_label = label.into();
        self
    }

    pub fn build(self) -> Stateful<Div> {
        let ConfirmInline { prompt, confirm_label, cancel_label, danger, on_confirm, on_cancel, style } = self;
        let el = div()
            .id(name_id("mv-confirm", &prompt))
            .flex()
            .items_center()
            .gap(px(8.))
            .role(Role::Group)
            .when(danger, |d| d.child(glyph(IconSrc::Ph(ph::WARNING), Weight::Fill, Tokens::danger(), icon_px::SM)))
            .child(div().text_size(px(size::SM)).text_color(Tokens::text_1()).child(prompt))
            .child(btn("mv-confirm-no", cancel_label, ButtonKind::Ghost).small().when_some(on_cancel, |b, f| b.on_click(move |e, w, cx| f(e, w, cx))))
            .child(btn("mv-confirm-yes", confirm_label, if danger { ButtonKind::Danger } else { ButtonKind::Primary }).small().when_some(on_confirm, |b, f| b.on_click(move |e, w, cx| f(e, w, cx))));
        refine(el, &style)
    }
}

/// Modal confirmation on the gpui-component `Dialog`: title, one sentence of consequence and two
/// outcome-labelled buttons ("Delete chat" / "Keep"), Cancel first, the confirming button last.
/// Nothing is focused when it opens and, when `danger`, Enter does not confirm, so a stray key
/// never destroys anything. Esc, the close button and the backdrop cancel.
/// Apple HIG alerts: <https://developer.apple.com/design/human-interface-guidelines/alerts>;
/// NN/g confirmation dialogs: <https://www.nngroup.com/articles/confirmation-dialog/>
pub fn confirm_dialog(title: impl Into<SharedString>, body: impl Into<SharedString>, confirm_label: impl Into<SharedString>, danger: bool) -> ConfirmDialog {
    ConfirmDialog { title: title.into(), body: body.into(), confirm_label: confirm_label.into(), cancel_label: "Cancel".into(), danger, icon: None, on_confirm: None, on_cancel: None }
}

/// See [`confirm_dialog`]. Finish with `.open(window, cx)`.
pub struct ConfirmDialog {
    title: SharedString,
    body: SharedString,
    confirm_label: SharedString,
    cancel_label: SharedString,
    danger: bool,
    icon: Option<Ph>,
    on_confirm: Option<WindowHandler>,
    on_cancel: Option<WindowHandler>,
}

impl ConfirmDialog {
    /// Outcome label for the way back, e.g. "Keep chat".
    pub fn cancel_label(mut self, label: impl Into<SharedString>) -> Self {
        self.cancel_label = label.into();
        self
    }

    /// Icon before the title (danger dialogs default to a warning).
    pub fn icon(mut self, icon: Ph) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn on_confirm(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_confirm = Some(Rc::new(handler));
        self
    }

    pub fn on_cancel(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_cancel = Some(Rc::new(handler));
        self
    }

    pub fn open(self, window: &mut Window, cx: &mut App) {
        let ConfirmDialog { title, body, confirm_label, cancel_label, danger, icon, on_confirm, on_cancel } = self;
        let icon = icon.or(danger.then_some(ph::WARNING_CIRCLE));
        window.open_dialog(cx, move |dialog, _, _| {
            let (yes, no, enter, esc) = (on_confirm.clone(), on_cancel.clone(), on_confirm.clone(), on_cancel.clone());
            let heading = div()
                .flex()
                .items_center()
                .gap(px(10.))
                .when_some(icon, |d, i| d.child(glyph(IconSrc::Ph(i), Weight::Fill, if danger { Tokens::danger() } else { Tokens::accent() }, icon_px::LG)))
                .child(div().ramp(ramp::SUBTITLE_2).text_color(Tokens::text_1()).child(title.clone()));
            let footer = div()
                .flex()
                .justify_end()
                .gap(px(8.))
                .w_full()
                .child(btn("mv-dialog-cancel", cancel_label.clone(), ButtonKind::Secondary).on_click(move |_, w, cx| {
                    w.close_dialog(cx);
                    if let Some(f) = &no {
                        f(w, cx);
                    }
                }))
                .child(btn("mv-dialog-confirm", confirm_label.clone(), if danger { ButtonKind::Danger } else { ButtonKind::Primary }).on_click(move |_, w, cx| {
                    w.close_dialog(cx);
                    if let Some(f) = &yes {
                        f(w, cx);
                    }
                }));
            dialog
                .w(px(440.))
                .title(heading)
                .child(div().text_size(px(size::MD)).line_height(px(20.)).text_color(Tokens::text_2()).child(body.clone()))
                .footer(footer)
                .on_ok(move |_, w, cx| {
                    if danger {
                        return false;
                    }
                    if let Some(f) = &enter {
                        f(w, cx);
                    }
                    true
                })
                .on_cancel(move |_, w, cx| {
                    if let Some(f) = &esc {
                        f(w, cx);
                    }
                    true
                })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{connector_state, count_text, fmt_score, step_state, usage_shares, Connector, StepState};

    #[test]
    fn usage_shares_never_overflow() {
        assert_eq!(usage_shares(&[25., 25.], 100.), vec![0.25, 0.25]);
        // More used than the stated total: shares of the sum instead.
        assert_eq!(usage_shares(&[60., 60.], 100.), vec![0.5, 0.5]);
        assert_eq!(usage_shares(&[0., f32::NAN, -3.], 0.), vec![0., 0., 0.]);
    }

    #[test]
    fn scores_read_naturally() {
        assert_eq!(fmt_score(8.43), "8.4");
        assert_eq!(fmt_score(10.), "10.0");
        assert_eq!(fmt_score(86.4), "86");
        assert_eq!(fmt_score(f32::NAN), "–");
    }

    #[test]
    fn step_state_precedence() {
        let done = [true, true, false, false];
        assert_eq!(step_state(0, &done, None, None), StepState::Done);
        assert_eq!(step_state(2, &done, None, None), StepState::Upcoming);
        assert_eq!(step_state(2, &done, Some(2), None), StepState::Running);
        // A step that failed while running shows the failure, even if it once completed.
        assert_eq!(step_state(1, &done, Some(1), Some(1)), StepState::Failed);
        assert_eq!(step_state(1, &done, Some(1), None), StepState::Running);
        // Out of range is simply not started.
        assert_eq!(step_state(9, &done, None, None), StepState::Upcoming);
    }

    #[test]
    fn connectors_fill_behind_progress() {
        let done = [true, false, false];
        assert_eq!(connector_state(0, 0, &done, None, None), Connector::Done);
        assert_eq!(connector_state(1, 0, &done, Some(2), None), Connector::Active);
        assert_eq!(connector_state(1, 2, &done, None, None), Connector::Active);
        assert_eq!(connector_state(1, 0, &done, None, None), Connector::Idle);
    }

    #[test]
    fn counts_cap_at_99() {
        assert_eq!(count_text(7), "7");
        assert_eq!(count_text(99), "99");
        assert_eq!(count_text(100), "99+");
    }
}
