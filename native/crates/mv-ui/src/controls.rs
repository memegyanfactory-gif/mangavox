//! Buttons, toggles, chips and selection controls. Colours come from `Tokens`, icons from Phosphor,
//! and every control shares the state layers and focus ring in [`crate::interact`].
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::icons::{ph, Ph, Weight};
use crate::interact::{element_builder, glyph, refine, render_once_element, tip, tip_opt, IconSrc, Interactive};
use crate::motion::{reduced, spinner};
use crate::theme::{easing, icon as icon_px, radius, size, Tokens};

pub(crate) type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
/// Handler that receives a picked index (segmented control, stepper).
pub(crate) type IndexHandler = Rc<dyn Fn(&usize, &mut Window, &mut App)>;
/// Handler that receives a new on/off value (disclosure).
pub(crate) type FlagHandler = Rc<dyn Fn(&bool, &mut Window, &mut App)>;
/// Handler with no event (menu items, toast actions, dialog buttons).
pub(crate) type WindowHandler = Rc<dyn Fn(&mut Window, &mut App)>;

fn clear() -> Rgba {
    Tokens::base().alpha(0.)
}

/// Button emphasis. One `Primary` per view; `Secondary` and `Ghost` for the rest; `Danger` only
/// for the confirming step of a destructive action and `DangerGhost` for destructive actions that
/// are not the main one ("Remove" in a row).
/// Material 3 buttons: <https://m3.material.io/components/all-buttons>
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
    Danger,
    DangerGhost,
}

/// Control heights: Sm 28 (dense rows, toolbars), Md 32 (default), Lg 40 (hero and dialogs).
/// Fluent 2 control sizes: <https://fluent2.microsoft.design/components/web/react/core/button/usage>
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ControlSize {
    Sm,
    #[default]
    Md,
    Lg,
}

impl ControlSize {
    pub fn height(self) -> f32 {
        match self {
            ControlSize::Sm => 28.,
            ControlSize::Md => 32.,
            ControlSize::Lg => 40.,
        }
    }

    /// Icon size that sits in a control of this size.
    pub fn icon(self) -> f32 {
        match self {
            ControlSize::Sm => icon_px::SM,
            ControlSize::Md => icon_px::MD,
            ControlSize::Lg => icon_px::LG,
        }
    }
}

#[derive(Clone, Copy)]
struct KindColors {
    bg: Rgba,
    fg: Rgba,
    border: Option<Rgba>,
    hover_bg: Rgba,
    hover_fg: Rgba,
    hover_border: Option<Rgba>,
    pressed_bg: Rgba,
}

fn kind_colors(kind: ButtonKind) -> KindColors {
    match kind {
        ButtonKind::Primary => KindColors {
            bg: Tokens::accent(),
            fg: Tokens::on_accent(),
            border: None,
            hover_bg: Tokens::accent_hover(),
            hover_fg: Tokens::on_accent(),
            hover_border: None,
            pressed_bg: Tokens::accent_pressed(),
        },
        ButtonKind::Secondary => KindColors {
            bg: Tokens::surface_2(),
            fg: Tokens::text_1(),
            border: Some(Tokens::border()),
            hover_bg: Tokens::surface_2().blend(Tokens::hover()),
            hover_fg: Tokens::text_1(),
            hover_border: Some(Tokens::border_strong()),
            pressed_bg: Tokens::surface_2().blend(Tokens::pressed()),
        },
        ButtonKind::Ghost => KindColors {
            bg: clear(),
            fg: Tokens::text_2(),
            border: None,
            hover_bg: Tokens::hover(),
            hover_fg: Tokens::text_1(),
            hover_border: None,
            pressed_bg: Tokens::pressed(),
        },
        ButtonKind::Danger => KindColors {
            bg: Tokens::danger(),
            fg: Tokens::base(),
            border: None,
            hover_bg: Tokens::danger().blend(Tokens::active()),
            hover_fg: Tokens::base(),
            hover_border: None,
            pressed_bg: Tokens::danger().blend(Tokens::well()),
        },
        ButtonKind::DangerGhost => KindColors {
            bg: clear(),
            fg: Tokens::danger(),
            border: None,
            hover_bg: Tokens::danger_soft(),
            hover_fg: Tokens::danger(),
            hover_border: None,
            pressed_bg: Tokens::danger().alpha(0.22),
        },
    }
}

/// The on state of a toggle [`Btn`], whatever its kind.
fn selected_colors() -> KindColors {
    KindColors {
        bg: Tokens::accent_soft(),
        fg: Tokens::accent(),
        border: Some(Tokens::accent_border()),
        hover_bg: Tokens::accent_a(0.22),
        hover_fg: Tokens::accent(),
        hover_border: Some(Tokens::accent()),
        pressed_bg: Tokens::accent_a(0.3),
    }
}

/// Text button, original form. Same look as [`btn`] (which adds icons, sizes, loading and
/// disabled-with-reason); returns the plain `Stateful<Div>` so existing call sites keep working.
pub fn button(id: impl Into<ElementId>, label: impl Into<SharedString>, kind: ButtonKind) -> Stateful<Div> {
    btn(id, label, kind).build()
}

/// Button builder: `btn("save", "Save", ButtonKind::Primary).icon(ph::CHECK).on_click(..)`.
/// Leading icon 16px with an 8px gap (Material 3 icon buttons pattern); hover, pressed and
/// focus-visible states; disabled buttons drop their handler and explain why in a tooltip
/// (Smashing, hidden vs disabled: <https://www.smashingmagazine.com/2024/05/hidden-vs-disabled-ux/>).
pub fn btn(id: impl Into<ElementId>, label: impl Into<SharedString>, kind: ButtonKind) -> Btn {
    Btn {
        base: div().id(id),
        style: StyleRefinement::default(),
        label: label.into(),
        kind,
        size: ControlSize::Md,
        icon: None,
        trailing: None,
        loading: false,
        selected: false,
        disabled: None,
        tooltip: None,
        kbd: None,
        full_width: false,
        on_click: None,
    }
}

/// See [`btn`].
pub struct Btn {
    base: Stateful<Div>,
    style: StyleRefinement,
    label: SharedString,
    kind: ButtonKind,
    size: ControlSize,
    icon: Option<IconSrc>,
    trailing: Option<IconSrc>,
    loading: bool,
    selected: bool,
    disabled: Option<SharedString>,
    tooltip: Option<SharedString>,
    kbd: Option<SharedString>,
    full_width: bool,
    on_click: Option<ClickHandler>,
}

element_builder!(Btn => Stateful<Div>);

impl Btn {
    /// Leading icon (a Phosphor handle, or a fixed asset path).
    pub fn icon(mut self, icon: impl Into<IconSrc>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Trailing icon, e.g. `ph::ARROW_UP_RIGHT` for links that leave the app or `ph::CARET_DOWN` for menus.
    pub fn trailing_icon(mut self, icon: impl Into<IconSrc>) -> Self {
        self.trailing = Some(icon.into());
        self
    }

    /// Shows a spinner in place of the leading icon and ignores clicks while `on`.
    pub fn loading(mut self, on: bool) -> Self {
        self.loading = on;
        self
    }

    /// Toggle button on state ("In library", "Following"): tonal accent container, accent label
    /// and the icon in its fill weight, so the state never rests on colour alone.
    /// Material 3 toggle buttons: <https://m3.material.io/components/buttons/guidelines>
    pub fn selected(mut self, on: bool) -> Self {
        self.selected = on;
        self
    }

    /// Disables the button and says why in its tooltip ("Download a voice first").
    pub fn disabled(mut self, reason: impl Into<SharedString>) -> Self {
        self.disabled = Some(reason.into());
        self
    }

    pub fn size(mut self, size: ControlSize) -> Self {
        self.size = size;
        self
    }

    pub fn small(self) -> Self {
        self.size(ControlSize::Sm)
    }

    pub fn large(self) -> Self {
        self.size(ControlSize::Lg)
    }

    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// Shortcut shown in the tooltip (the tooltip falls back to the label).
    pub fn kbd(mut self, keys: impl Into<SharedString>) -> Self {
        self.kbd = Some(keys.into());
        self
    }

    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    pub fn build(self) -> Stateful<Div> {
        let Btn { base, style, label, kind, size: sz, icon, trailing, loading, selected, disabled, tooltip, kbd, full_width, on_click } = self;
        let enabled = disabled.is_none() && !loading;
        let c = if selected { selected_colors() } else { kind_colors(kind) };
        let (pad, text, ic, gap) = match sz {
            ControlSize::Sm => (10., size::SM, icon_px::SM, 6.),
            ControlSize::Md => (12., size::SM, icon_px::MD, 8.),
            ControlSize::Lg => (16., size::MD, 18., 8.),
        };
        let off = disabled.is_some();
        let fg = if off { Tokens::text_disabled() } else { c.fg };
        let weight = if selected {
            Weight::Fill
        } else if ic <= 14. {
            Weight::Bold
        } else {
            Weight::Regular
        };
        let glyph_at = move |src: IconSrc| glyph(src, weight, fg, ic).when(enabled, |s| s.group_hover("mv-btn", move |s| s.text_color(c.hover_fg)));
        let tip_text = disabled.clone().filter(|r| !r.is_empty()).or(tooltip).or_else(|| kbd.as_ref().map(|_| label.clone()));
        let el = base
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(px(gap))
            .h(px(sz.height()))
            .px(px(pad))
            .rounded(px(radius::MD))
            .text_size(px(text))
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .text_color(fg)
            .role(Role::Button)
            .when(selected, |d| d.aria_toggled(Toggled::True))
            .when(full_width, |d| d.w_full())
            .map(|d| match (off, kind) {
                (true, ButtonKind::Primary | ButtonKind::Danger) => d.bg(Tokens::active()),
                (true, ButtonKind::Secondary) => d.bg(Tokens::surface_2()).border_1().border_color(Tokens::border()),
                (true, _) => d,
                (false, _) => d.bg(c.bg).when_some(c.border, |d, b| d.border_1().border_color(b)),
            })
            .when(enabled, |d| {
                d.cursor_pointer()
                    .group("mv-btn")
                    .hover(move |s| {
                        let s = s.bg(c.hover_bg).text_color(c.hover_fg);
                        match c.hover_border {
                            Some(b) => s.border_color(b),
                            None => s,
                        }
                    })
                    .active(move |s| s.bg(c.pressed_bg))
                    .focus_ring()
                    .when_some(on_click, |d, f| d.on_click(move |e, w, cx| f(e, w, cx)))
            })
            .when(off, |d| d.cursor_not_allowed())
            .when_some(tip_text, |d, t| d.tooltip(tip_opt(t, if enabled { kbd } else { None })))
            .when(loading, |d| d.child(spinner("mv-btn-spin", fg, ic)))
            .when(!loading, |d| d.when_some(icon, |d, i| d.child(glyph_at(i))))
            .child(label)
            .when_some(trailing, |d, i| d.child(glyph_at(i)));
        refine(el, &style)
    }
}

/// Icon-only button. Two forms, picked by the third argument:
/// - `icon_button(id, "icons/x.svg", enabled)` returns the original 30px `Stateful<Div>`.
/// - `icon_button(id, ph::X, "Close")` returns the [`IconBtn`] builder, whose tooltip is required
///   (NN/g: icon-only buttons need a label: <https://www.nngroup.com/articles/icon-usability/>).
pub fn icon_button<A: IconButtonArg>(id: impl Into<ElementId>, icon: impl Into<IconSrc>, arg: A) -> A::Output {
    A::make(id.into(), icon.into(), arg)
}

/// The third argument of [`icon_button`]: `bool` (enabled, original form) or a tooltip string.
pub trait IconButtonArg {
    type Output;
    fn make(id: ElementId, icon: IconSrc, arg: Self) -> Self::Output;
}

impl IconButtonArg for bool {
    type Output = Stateful<Div>;
    fn make(id: ElementId, icon: IconSrc, enabled: bool) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .size(px(30.))
            .rounded(px(radius::MD))
            .role(Role::Button)
            .when(enabled, |d| d.interactive())
            .child(glyph(icon, Weight::Regular, if enabled { Tokens::text_2() } else { Tokens::text_disabled() }, icon_px::MD))
    }
}

impl<'a> IconButtonArg for &'a str {
    type Output = IconBtn;
    fn make(id: ElementId, icon: IconSrc, tooltip: &'a str) -> IconBtn {
        IconBtn::new(id, icon, SharedString::from(tooltip.to_string()))
    }
}

impl IconButtonArg for String {
    type Output = IconBtn;
    fn make(id: ElementId, icon: IconSrc, tooltip: String) -> IconBtn {
        IconBtn::new(id, icon, tooltip.into())
    }
}

impl IconButtonArg for SharedString {
    type Output = IconBtn;
    fn make(id: ElementId, icon: IconSrc, tooltip: SharedString) -> IconBtn {
        IconBtn::new(id, icon, tooltip)
    }
}

/// Icon-only button builder from [`icon_button`]. `.selected(true)` swaps to the fill weight on an
/// `active` container (a toggle's on state: Material 3 icon buttons,
/// <https://m3.material.io/components/icon-buttons/overview>).
pub struct IconBtn {
    base: Stateful<Div>,
    style: StyleRefinement,
    icon: IconSrc,
    tooltip: SharedString,
    selected: bool,
    danger: bool,
    size: ControlSize,
    kbd: Option<SharedString>,
    disabled: Option<SharedString>,
    color: Option<Rgba>,
    on_click: Option<ClickHandler>,
}

element_builder!(IconBtn => Stateful<Div>);

impl IconBtn {
    fn new(id: ElementId, icon: IconSrc, tooltip: SharedString) -> Self {
        IconBtn {
            base: div().id(id),
            style: StyleRefinement::default(),
            icon,
            tooltip,
            selected: false,
            danger: false,
            size: ControlSize::Md,
            kbd: None,
            disabled: None,
            color: None,
            on_click: None,
        }
    }

    /// On state of a toggle: fill weight icon on an `active` container.
    pub fn selected(mut self, on: bool) -> Self {
        self.selected = on;
        self
    }

    /// Destructive action: turns danger red on hover.
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    /// Sm 28px box / 14px icon, Md 32 / 16 (default), Lg 40 / 20.
    pub fn size(mut self, size: ControlSize) -> Self {
        self.size = size;
        self
    }

    pub fn small(self) -> Self {
        self.size(ControlSize::Sm)
    }

    pub fn large(self) -> Self {
        self.size(ControlSize::Lg)
    }

    /// Shortcut shown in the tooltip, e.g. `"ctrl-b"`.
    pub fn kbd(mut self, keys: impl Into<SharedString>) -> Self {
        self.kbd = Some(keys.into());
        self
    }

    /// Disables the button; the tooltip shows `reason` instead of the label.
    pub fn disabled(mut self, reason: impl Into<SharedString>) -> Self {
        self.disabled = Some(reason.into());
        self
    }

    /// Icon colour at rest (default `text_2`, `text_1` when selected).
    pub fn color(mut self, color: Rgba) -> Self {
        self.color = Some(color);
        self
    }

    /// Replaces the tooltip text.
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = text.into();
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    pub fn build(self) -> Stateful<Div> {
        let IconBtn { base, style, icon, tooltip, selected, danger, size: sz, kbd, disabled, color, on_click } = self;
        let enabled = disabled.is_none();
        let box_px = sz.height();
        let ic = sz.icon();
        let weight = if selected {
            Weight::Fill
        } else if ic <= 14. {
            Weight::Bold
        } else {
            Weight::Regular
        };
        let rest = match (enabled, selected) {
            (false, _) => Tokens::text_disabled(),
            (true, true) => color.unwrap_or(Tokens::text_1()),
            (true, false) => color.unwrap_or(Tokens::text_2()),
        };
        let hover_fg = if danger { Tokens::danger() } else if selected { rest } else { Tokens::text_1() };
        let hover_bg = if danger { Tokens::danger_soft() } else if selected { Tokens::active_hover() } else { Tokens::hover() };
        let pressed_bg = if danger { Tokens::danger().alpha(0.22) } else { Tokens::pressed() };
        let tip_text = disabled.clone().filter(|r| !r.is_empty()).unwrap_or_else(|| tooltip.clone());
        let el = base
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(box_px))
            .rounded(px(radius::MD))
            .role(Role::Button)
            .aria_label(tooltip)
            .when(selected, |d| d.bg(Tokens::active()).aria_toggled(Toggled::True))
            .when(enabled, |d| {
                d.cursor_pointer()
                    .group("mv-icon-btn")
                    .hover(move |s| s.bg(hover_bg))
                    .active(move |s| s.bg(pressed_bg))
                    .focus_ring()
                    .when_some(on_click, |d, f| d.on_click(move |e, w, cx| f(e, w, cx)))
            })
            .when(!enabled, |d| d.cursor_not_allowed())
            .tooltip(tip_opt(tip_text, if enabled { kbd } else { None }))
            .child(glyph(icon, weight, rest, ic).when(enabled, |s| s.group_hover("mv-icon-btn", move |s| s.text_color(hover_fg))));
        refine(el, &style)
    }
}

/// Two-state icon toggle (favourite, bookmark, pin): outline in `text_3` when off, FILLED in
/// `active_color` when on. Hovering an off toggle previews the on colour; the icon pops a little
/// when it changes (not under reduced motion). The tooltip names what a click will do.
/// Apple SF Symbols / Fluent 2 iconography (filled = selected):
/// <https://fluent2.microsoft.design/iconography>; Mihon favourite toggle.
pub fn icon_toggle(
    id: impl Into<ElementId>,
    icon: Ph,
    active: bool,
    active_color: Rgba,
    tooltip_on: impl Into<SharedString>,
    tooltip_off: impl Into<SharedString>,
) -> IconToggle {
    let id = id.into();
    IconToggle {
        base: div().id(id.clone()),
        id,
        style: StyleRefinement::default(),
        icon,
        active,
        color: active_color,
        tip_on: tooltip_on.into(),
        tip_off: tooltip_off.into(),
        icon_size: icon_px::MD,
        disabled: None,
        on_click: None,
    }
}

/// The favourite star used everywhere (manga, anime, movies, voices, models): gold
/// `Tokens::favorite()` and filled when on, an outline when off.
pub fn favorite_star(id: impl Into<ElementId>, on: bool) -> IconToggle {
    icon_toggle(id, ph::STAR, on, Tokens::favorite(), "Remove from favorites", "Add to favorites")
}

/// See [`icon_toggle`].
pub struct IconToggle {
    id: ElementId,
    base: Stateful<Div>,
    style: StyleRefinement,
    icon: Ph,
    active: bool,
    color: Rgba,
    tip_on: SharedString,
    tip_off: SharedString,
    icon_size: f32,
    disabled: Option<SharedString>,
    on_click: Option<ClickHandler>,
}

render_once_element!(IconToggle);

impl IconToggle {
    /// Icon size in px (default 16); the hit area is at least 24px (WCAG 2.5.8).
    pub fn size(mut self, px_size: f32) -> Self {
        self.icon_size = px_size;
        self
    }

    pub fn disabled(mut self, reason: impl Into<SharedString>) -> Self {
        self.disabled = Some(reason.into());
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for IconToggle {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let IconToggle { id, base, style, icon, active, color, tip_on, tip_off, icon_size, disabled, on_click } = self;
        // Remember the last state so the pop plays when it changes, not on first paint.
        let memo = window.use_keyed_state(id, cx, |_, _| (active, 0u64));
        let (last, mut generation) = *memo.read(cx);
        if last != active {
            generation += 1;
            memo.update(cx, |m, _| *m = (active, generation));
        }
        let enabled = disabled.is_none();
        let rest = match (enabled, active) {
            (false, _) => Tokens::text_disabled(),
            (true, true) => color,
            (true, false) => Tokens::text_3(),
        };
        let svg = glyph(IconSrc::Ph(icon), if active { Weight::Fill } else if icon_size <= 14. { Weight::Bold } else { Weight::Regular }, rest, icon_size)
            .when(enabled && !active, |s| s.group_hover("mv-toggle", move |s| s.text_color(color)));
        let glyph_el = if generation > 0 && !reduced() {
            svg.with_animation(ElementId::NamedInteger("mv-pop".into(), generation), Animation::new(Duration::from_millis(260)).with_easing(easing::standard), move |s, t| {
                let bump = (t * std::f32::consts::PI).sin();
                let k = if active { 1. + 0.3 * bump } else { 1. - 0.12 * bump };
                s.with_transformation(Transformation::scale(gpui_kit::size(k, k)))
            })
            .into_any_element()
        } else {
            svg.into_any_element()
        };
        let label = if active { tip_on } else { tip_off };
        let tip_text = disabled.filter(|r| !r.is_empty()).unwrap_or_else(|| label.clone());
        let el = base
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px((icon_size + 12.).max(24.)))
            .rounded(px(radius::SM))
            .role(Role::Button)
            .aria_label(label)
            .aria_toggled(if active { Toggled::True } else { Toggled::False })
            .when(enabled, |d| {
                d.cursor_pointer()
                    .group("mv-toggle")
                    .hover(|s| s.bg(Tokens::hover()))
                    .active(|s| s.bg(Tokens::pressed()))
                    .focus_ring()
                    .when_some(on_click, |d, f| d.on_click(move |e, w, cx| f(e, w, cx)))
            })
            .when(!enabled, |d| d.cursor_not_allowed())
            .tooltip(tip(tip_text))
            .child(glyph_el);
        refine(el, &style)
    }
}

/// Filled accent disc with a check for "in use", "default" and "done". Always filled, never an
/// outline, so it reads as on (Fluent 2: filled icons mark selected states).
pub fn selected_badge() -> Div {
    selected_badge_sized(18.)
}

/// [`selected_badge`] at a given diameter.
pub fn selected_badge_sized(px_size: f32) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(px_size))
        .rounded_full()
        .bg(Tokens::accent())
        .child(glyph(IconSrc::Ph(ph::CHECK), Weight::Bold, Tokens::on_accent(), (px_size * 0.62).round()))
}

/// Selectable filter chip: a leading check and a tonal accent container when selected, so the
/// state never rests on colour alone. Optional leading icon and trailing count.
/// Material 3 filter chips: <https://m3.material.io/components/chips/guidelines>
pub fn filter_chip(id: impl Into<ElementId>, label: impl Into<SharedString>, selected: bool) -> FilterChip {
    FilterChip { base: div().id(id), style: StyleRefinement::default(), label: label.into(), selected, icon: None, count: None, on_click: None }
}

/// See [`filter_chip`].
pub struct FilterChip {
    base: Stateful<Div>,
    style: StyleRefinement,
    label: SharedString,
    selected: bool,
    icon: Option<Ph>,
    count: Option<usize>,
    on_click: Option<ClickHandler>,
}

element_builder!(FilterChip => Stateful<Div>);

impl FilterChip {
    /// Leading icon while unselected (the check takes its place when selected).
    pub fn icon(mut self, icon: Ph) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Trailing count, e.g. results that match.
    pub fn count(mut self, n: usize) -> Self {
        self.count = Some(n);
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    pub fn build(self) -> Stateful<Div> {
        let FilterChip { base, style, label, selected, icon, count, on_click } = self;
        let lead: Option<(Ph, Weight, Rgba)> = if selected { Some((ph::CHECK, Weight::Bold, Tokens::accent())) } else { icon.map(|i| (i, Weight::Bold, Tokens::text_3())) };
        let el = base
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .pr(px(12.))
            .pl(px(if lead.is_some() { 8. } else { 12. }))
            .rounded(px(radius::PILL))
            .border_1()
            .text_size(px(size::SM))
            .whitespace_nowrap()
            .cursor_pointer()
            .group("mv-chip")
            .role(Role::Button)
            .aria_toggled(if selected { Toggled::True } else { Toggled::False })
            .map(|d| {
                if selected {
                    d.bg(Tokens::accent_soft()).border_color(Tokens::accent_border()).text_color(Tokens::accent()).font_weight(FontWeight::MEDIUM).hover(|s| s.bg(Tokens::accent_a(0.22)))
                } else {
                    d.border_color(Tokens::border()).text_color(Tokens::text_2()).hover(|s| s.bg(Tokens::hover()).border_color(Tokens::border_strong()).text_color(Tokens::text_1()))
                }
            })
            .active(|s| s.bg(Tokens::pressed()))
            .focus_ring()
            .when_some(on_click, |d, f| d.on_click(move |e, w, cx| f(e, w, cx)))
            .when_some(lead, |d, (i, w, c)| d.child(glyph(IconSrc::Ph(i), w, c, icon_px::SM).when(!selected, |s| s.group_hover("mv-chip", |s| s.text_color(Tokens::text_1())))))
            .child(label)
            .when_some(count, |d, n| d.child(div().text_size(px(size::XS)).text_color(if selected { Tokens::accent() } else { Tokens::text_3() }).child(n.to_string())));
        refine(el, &style)
    }
}

/// One option of a [`segmented`] control: optional icon plus label.
#[derive(Clone, Debug)]
pub struct SegmentItem {
    pub icon: Option<Ph>,
    pub label: SharedString,
}

impl<L: Into<SharedString>> From<(Option<Ph>, L)> for SegmentItem {
    fn from((icon, label): (Option<Ph>, L)) -> Self {
        SegmentItem { icon, label: label.into() }
    }
}

impl<'a, 'b> From<&'a (Option<Ph>, &'b str)> for SegmentItem {
    fn from((icon, label): &'a (Option<Ph>, &'b str)) -> Self {
        SegmentItem { icon: *icon, label: SharedString::from(label.to_string()) }
    }
}

impl<'a> From<&'a str> for SegmentItem {
    fn from(label: &'a str) -> Self {
        SegmentItem { icon: None, label: SharedString::from(label.to_string()) }
    }
}

/// Segmented control for one choice out of 2 to 5 (reader mode, view, speed, format): a recessed
/// track with the selected segment raised and marked with a check (or its icon in the fill
/// weight). Left/Right arrows move the selection.
/// Material 3 segmented buttons: <https://m3.material.io/components/segmented-buttons/overview>
pub fn segmented<I: Into<SegmentItem>>(id: impl Into<ElementId>, items: impl IntoIterator<Item = I>, selected: usize) -> Segmented {
    Segmented {
        base: div().id(id),
        style: StyleRefinement::default(),
        items: items.into_iter().map(Into::into).collect(),
        selected,
        size: ControlSize::Md,
        full_width: false,
        on_change: None,
    }
}

/// See [`segmented`].
pub struct Segmented {
    base: Stateful<Div>,
    style: StyleRefinement,
    items: Vec<SegmentItem>,
    selected: usize,
    size: ControlSize,
    full_width: bool,
    on_change: Option<IndexHandler>,
}

element_builder!(Segmented => Stateful<Div>);

impl Segmented {
    /// Called with the index the user picked.
    pub fn on_change(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    pub fn size(mut self, size: ControlSize) -> Self {
        self.size = size;
        self
    }

    pub fn small(self) -> Self {
        self.size(ControlSize::Sm)
    }

    /// Segments share the full width equally.
    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    pub fn build(self) -> Stateful<Div> {
        let Segmented { base, style, items, selected, size: sz, full_width, on_change } = self;
        let n = items.len();
        let seg_h = sz.height() - 6.;
        let pad = if sz == ControlSize::Sm { 8. } else { 12. };
        let keys = on_change.clone();
        let el = base
            .flex()
            .items_center()
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(radius::MD))
            .bg(Tokens::well())
            .border_1()
            .border_color(Tokens::border())
            .role(Role::RadioGroup)
            .when(full_width, |d| d.w_full())
            .when(!full_width, |d| d.flex_none())
            .when_some(keys.filter(|_| n > 1), |d, f| {
                d.on_key_down(move |ev: &KeyDownEvent, w, cx| {
                    let next = match ev.keystroke.key.as_str() {
                        "left" => (selected + n - 1) % n,
                        "right" => (selected + 1) % n,
                        _ => return,
                    };
                    cx.stop_propagation();
                    f(&next, w, cx);
                })
            })
            .children(items.into_iter().enumerate().map(|(i, item)| {
                let on = i == selected;
                let lead = match (item.icon, on) {
                    (Some(icon), true) => Some((icon, Weight::Fill, Tokens::text_1())),
                    (Some(icon), false) => Some((icon, Weight::Bold, Tokens::text_3())),
                    (None, true) => Some((ph::CHECK, Weight::Bold, Tokens::accent())),
                    (None, false) => None,
                };
                div()
                    .id(("mv-seg", i))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .h(px(seg_h))
                    .px(px(pad))
                    .rounded(px(radius::SM))
                    .text_size(px(size::SM))
                    .whitespace_nowrap()
                    .role(Role::RadioButton)
                    .aria_selected(on)
                    .aria_label(item.label.clone())
                    .when(full_width, |d| d.flex_1())
                    .map(|d| {
                        if on {
                            d.bg(Tokens::surface_3()).border_1().border_color(Tokens::border_strong()).shadow_xs().text_color(Tokens::text_1()).font_weight(FontWeight::MEDIUM)
                        } else {
                            d.text_color(Tokens::text_2()).cursor_pointer().group("mv-seg").hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1())).active(|s| s.bg(Tokens::pressed()))
                        }
                    })
                    .focus_ring()
                    .when_some(on_change.clone().filter(|_| !on), |d, f| d.on_click(move |_, w, cx| f(&i, w, cx)))
                    .when_some(lead, |d, (icon, w, c)| d.child(glyph(IconSrc::Ph(icon), w, c, icon_px::SM).when(!on, |s| s.group_hover("mv-seg", |s| s.text_color(Tokens::text_1())))))
                    .child(item.label)
            }));
        refine(el, &style)
    }
}

/// Tri-state checkbox value. `Mixed` is the parent of a partly selected group.
/// NN/g checkboxes: <https://www.nngroup.com/articles/checkboxes-design-guidelines/>
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CheckState {
    Unchecked,
    Checked,
    Mixed,
}

impl From<bool> for CheckState {
    fn from(on: bool) -> Self {
        if on {
            CheckState::Checked
        } else {
            CheckState::Unchecked
        }
    }
}

/// Checkbox with a 24x24 hit area (WCAG 2.5.8). Add the label with `.child("Label")` so the
/// whole row toggles; attach `.on_click(..)`.
pub fn checkbox(id: impl Into<ElementId>, state: impl Into<CheckState>) -> Stateful<Div> {
    let state = state.into();
    let filled = state != CheckState::Unchecked;
    let mark = match state {
        CheckState::Checked => Some(ph::CHECK),
        CheckState::Mixed => Some(ph::MINUS),
        CheckState::Unchecked => None,
    };
    let square = div()
        .flex()
        .items_center()
        .justify_center()
        .size(px(16.))
        .rounded(px(radius::XS))
        .border_1()
        .map(|d| {
            if filled {
                d.bg(Tokens::accent()).border_color(Tokens::accent()).group_hover("mv-check", |s| s.bg(Tokens::accent_hover()).border_color(Tokens::accent_hover()))
            } else {
                d.border_color(Tokens::track()).group_hover("mv-check", |s| s.border_color(Tokens::text_2()).bg(Tokens::hover()))
            }
        })
        .when_some(mark, |d, m| d.child(glyph(IconSrc::Ph(m), Weight::Bold, Tokens::on_accent(), 12.)));
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .min_h(px(24.))
        .rounded(px(radius::SM))
        .text_size(px(size::SM))
        .text_color(Tokens::text_1())
        .cursor_pointer()
        .group("mv-check")
        .role(Role::CheckBox)
        .aria_toggled(match state {
            CheckState::Checked => Toggled::True,
            CheckState::Unchecked => Toggled::False,
            CheckState::Mixed => Toggled::Mixed,
        })
        .focus_ring()
        .child(div().flex().flex_none().items_center().justify_center().size(px(24.)).child(square))
}

/// The app's one on/off switch for settings that apply immediately: gpui-component `Switch` in
/// the brand accent. Chain `.on_click(|on: &bool, window, cx| ..)` and `.label(..)`.
/// NN/g toggle switches: <https://www.nngroup.com/articles/toggle-switch-guidelines/>
pub fn switch(id: impl Into<ElementId>, on: bool) -> Switch {
    Switch::new(id).checked(on).color(Tokens::accent())
}

/// Sidebar / rail destination. Active: fill-weight accent icon, `active` container and a 3px
/// accent indicator. Collapsed it shows the icon alone and the label as a tooltip.
/// Fluent NavigationView: <https://learn.microsoft.com/en-us/windows/apps/design/controls/navigationview>
pub fn nav_item(id: impl Into<ElementId>, icon: Ph, label: impl Into<SharedString>, active: bool, collapsed: bool) -> NavItem {
    NavItem { base: div().id(id), style: StyleRefinement::default(), icon, label: label.into(), active, collapsed, badge: None, kbd: None, on_click: None }
}

/// See [`nav_item`].
pub struct NavItem {
    base: Stateful<Div>,
    style: StyleRefinement,
    icon: Ph,
    label: SharedString,
    active: bool,
    collapsed: bool,
    badge: Option<usize>,
    kbd: Option<SharedString>,
    on_click: Option<ClickHandler>,
}

element_builder!(NavItem => Stateful<Div>);

impl NavItem {
    /// Count shown at the end of the row (a dot when collapsed); 0 shows nothing.
    pub fn badge(mut self, n: usize) -> Self {
        self.badge = Some(n);
        self
    }

    /// Shortcut shown in the collapsed tooltip.
    pub fn kbd(mut self, keys: impl Into<SharedString>) -> Self {
        self.kbd = Some(keys.into());
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    pub fn build(self) -> Stateful<Div> {
        let NavItem { base, style, icon, label, active, collapsed, badge, kbd, on_click } = self;
        let badge = badge.filter(|n| *n > 0);
        let fg = if active { Tokens::text_1() } else { Tokens::text_2() };
        let el = base
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .h(px(32.))
            .rounded(px(radius::MD))
            .text_size(px(size::MD))
            .text_color(fg)
            .cursor_pointer()
            .group("mv-nav")
            .role(Role::Link)
            .aria_label(label.clone())
            .aria_selected(active)
            .map(|d| if collapsed { d.w(px(40.)).justify_center() } else { d.w_full().px(px(10.)) })
            .map(|d| {
                if active {
                    d.bg(Tokens::active()).font_weight(FontWeight::MEDIUM).hover(|s| s.bg(Tokens::active_hover()))
                } else {
                    d.hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
                }
            })
            .active(|s| s.bg(Tokens::pressed()))
            .focus_ring()
            .when(collapsed, |d| d.tooltip(tip_opt(label.clone(), kbd)))
            .when_some(on_click, |d, f| d.on_click(move |e, w, cx| f(e, w, cx)))
            .when(active, |d| d.child(div().absolute().left(px(0.)).top(px(9.)).w(px(3.)).h(px(14.)).rounded_full().bg(Tokens::accent())))
            .child(
                glyph(IconSrc::Ph(icon), if active { Weight::Fill } else { Weight::Regular }, if active { Tokens::accent() } else { Tokens::text_2() }, icon_px::MD)
                    .when(!active, |s| s.group_hover("mv-nav", |s| s.text_color(Tokens::text_1()))),
            )
            .when(!collapsed, |d| d.child(div().flex_1().min_w_0().truncate().child(label)))
            .when_some(badge, |d, n| {
                if collapsed {
                    d.child(div().absolute().top(px(6.)).right(px(8.)).size(px(7.)).rounded_full().bg(Tokens::accent()))
                } else {
                    d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(if n > 99 { "99+".to_string() } else { n.to_string() }))
                }
            });
        refine(el, &style)
    }
}

/// Selectable row for lists (chats, models, voices, history): `active` fill when selected and a
/// hover layer that goes stronger (`active_hover`) on the selected row, so hovering never makes it
/// look less selected. Pressed state, Tab stop and focus ring included; add children and
/// `.on_click(..)`. Do not call `.hover(..)` on it.
/// Material 3 lists: <https://m3.material.io/components/lists/guidelines>
pub fn list_row(id: impl Into<ElementId>, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(10.))
        .w_full()
        .min_h(px(36.))
        .px(px(10.))
        .py(px(6.))
        .rounded(px(radius::MD))
        .text_size(px(size::MD))
        .text_color(if selected { Tokens::text_1() } else { Tokens::text_2() })
        .role(Role::ListItem)
        .aria_selected(selected)
        .map(|d| if selected { d.bg(Tokens::active()).hover(|s| s.bg(Tokens::active_hover())) } else { d.hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1())) })
        .pressable()
}
