//! Interaction plumbing every control shares: state layers, the keyboard focus ring, tooltips
//! with shortcut hints and key-cap chips.
//!
//! Every clickable element gets hover, pressed and focus-visible states and Enter/Space activation
//! (Material 3 states: <https://m3.material.io/foundations/interaction/states/overview>, WCAG 2.4.7
//! Focus Visible: <https://www.w3.org/WAI/WCAG22/Understanding/focus-visible>).
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::*;

use crate::icons::{Ph, Weight};
use crate::theme::{radius, size, Tokens};

/// A 2px ring just outside the element in `Tokens::focus_ring()` (accent, 3:1 or better on every
/// surface, so it meets WCAG 2.4.13 Focus Appearance).
pub fn focus_ring_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow { color: Tokens::focus_ring().into(), offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(2.), inset: false }]
}

/// State layers and keyboard behaviour for custom clickable elements (any `div().id(..)`).
pub trait Interactive: StatefulInteractiveElement + Styled + Sized {
    /// Pointer cursor, hover and pressed state layers, a Tab stop, a focus-visible ring and
    /// Enter/Space activation of `on_click`. It sets the hover style, and GPUI allows one per
    /// element: when you need your own hover, use [`Interactive::pressable`] instead.
    /// Material 3 state layers: <https://m3.material.io/foundations/interaction/states/state-layers>
    fn interactive(self) -> Self {
        self.hover(|s| s.bg(Tokens::hover())).pressable()
    }

    /// Everything [`Interactive::interactive`] adds except the hover layer, for elements that
    /// set their own `.hover(..)`.
    fn pressable(self) -> Self {
        self.cursor_pointer().active(|s| s.bg(Tokens::pressed())).focus_ring()
    }

    /// Tab stop plus a focus-visible ring. A mouse press does not move focus (only Tab does), so
    /// clicking a button never pulls focus out of a text field. GPUI turns Enter/Space on a focused
    /// element into a click.
    fn focus_ring(self) -> Self {
        self.tab_index(0).focus_visible(|s| s.shadow(focus_ring_shadow())).on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
    }
}

impl<T: StatefulInteractiveElement + Styled> Interactive for T {}

/// A ready-made custom button surface: `interactive("row-3").child(..).on_click(..)`.
pub fn interactive(id: impl Into<ElementId>) -> Stateful<Div> {
    div().id(id).interactive()
}

/// Tooltip builder: `.tooltip(tip("Copy link"))`. Icon-only controls always need one.
/// NN/g, icon usability: <https://www.nngroup.com/articles/icon-usability/>
pub fn tip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |window, cx| Tooltip::new(text.clone()).build(window, cx)
}

/// Tooltip with its keyboard shortcut, e.g. `tip_kbd("Search", "ctrl-k")`.
/// Apple HIG, keyboards (show shortcuts where the command lives):
/// <https://developer.apple.com/design/human-interface-guidelines/keyboards>
pub fn tip_kbd(text: impl Into<SharedString>, keys: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let (text, keys) = (text.into(), keys.into());
    move |window, cx| Tooltip::new(text.clone()).key_binding(parse_keys(&keys).map(Kbd::new)).build(window, cx)
}

/// Tooltip text plus an optional shortcut, as one closure (used by the builders).
pub(crate) fn tip_opt(text: SharedString, keys: Option<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |window, cx| Tooltip::new(text.clone()).key_binding(keys.as_deref().and_then(parse_keys).map(Kbd::new)).build(window, cx)
}

/// GPUI keystroke from `"ctrl-k"`, `"Ctrl+K"` or `"ctrl-shift-p"`; the first stroke of a sequence.
pub(crate) fn parse_keys(keys: &str) -> Option<Keystroke> {
    let first = keys.split_whitespace().next()?;
    let norm = if first.len() > 1 { first.to_lowercase().replace('+', "-") } else { first.to_lowercase() };
    Keystroke::parse(&norm).ok()
}

/// Display text for a shortcut in the platform style: `"ctrl-shift-p"` → `"Ctrl+Shift+P"`.
/// Text that is not a keystroke comes back unchanged.
pub fn kbd_label(keys: &str) -> String {
    let parts: Vec<String> = keys.split_whitespace().map(|k| parse_keys(k).map(|s| Kbd::format(&s)).unwrap_or_else(|| k.to_string())).collect();
    if parts.is_empty() {
        keys.to_string()
    } else {
        parts.join(" ")
    }
}

/// Key-cap chip for shortcut hints in menus, rows and empty states: `kbd("ctrl-k")` shows "Ctrl+K".
pub fn kbd(keys: impl Into<SharedString>) -> Div {
    let keys = keys.into();
    div()
        .flex_none()
        .flex()
        .items_center()
        .h(px(18.))
        .px(px(5.))
        .rounded(px(radius::XS))
        .border_1()
        .border_color(Tokens::border_strong())
        .bg(Tokens::hover())
        .text_size(px(size::XS))
        .font_weight(FontWeight::MEDIUM)
        .text_color(Tokens::text_2())
        .whitespace_nowrap()
        .child(kbd_label(&keys))
}

/// An icon given either as a Phosphor handle (weights follow state) or a fixed asset path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconSrc {
    Ph(Ph),
    Path(&'static str),
}

impl IconSrc {
    /// Asset path at a weight; a fixed path ignores the weight.
    pub fn at(self, w: Weight) -> &'static str {
        match self {
            IconSrc::Ph(p) => p.path(w),
            IconSrc::Path(p) => p,
        }
    }

    /// Bold at 14px and below (where regular strokes go faint), regular above.
    pub fn for_size(self, px_size: f32) -> &'static str {
        self.at(if px_size <= 14. { Weight::Bold } else { Weight::Regular })
    }
}

impl From<Ph> for IconSrc {
    fn from(p: Ph) -> Self {
        IconSrc::Ph(p)
    }
}

impl From<&'static str> for IconSrc {
    fn from(p: &'static str) -> Self {
        IconSrc::Path(p)
    }
}

/// Svg sized and tinted, from any [`IconSrc`] at an explicit weight.
pub(crate) fn glyph(src: IconSrc, w: Weight, color: Rgba, px_size: f32) -> Svg {
    svg().path(src.at(w)).size(px(px_size)).flex_none().text_color(color)
}

/// Applies a builder's collected `Styled` calls last, so the caller's styles win.
pub(crate) fn refine<E: Styled>(mut el: E, style: &StyleRefinement) -> E {
    el.style().refine(style);
    el
}

/// Implements the traits that let a builder be used like the `Stateful<Div>` it renders to:
/// `Styled` (collected, applied last), `InteractiveElement` / `StatefulInteractiveElement`
/// (forwarded to `base`) and `IntoElement` via the type's `build()` (which also brings GPUI's
/// `FluentBuilder`: `.when(..)`, `.map(..)`).
macro_rules! builder_traits {
    ($t:ty) => {
        impl gpui_kit::Styled for $t {
            fn style(&mut self) -> &mut gpui_kit::StyleRefinement {
                &mut self.style
            }
        }
        impl gpui_kit::InteractiveElement for $t {
            fn interactivity(&mut self) -> &mut gpui_kit::Interactivity {
                gpui_kit::InteractiveElement::interactivity(&mut self.base)
            }
        }
        impl gpui_kit::StatefulInteractiveElement for $t {}
    };
}
pub(crate) use builder_traits;

macro_rules! element_builder {
    ($t:ty => $out:ty) => {
        $crate::interact::builder_traits!($t);
        impl gpui_kit::IntoElement for $t {
            type Element = $out;
            fn into_element(self) -> Self::Element {
                self.build()
            }
        }
    };
}
pub(crate) use element_builder;

/// The same traits for a `RenderOnce` builder, with the `IntoElement` that
/// `#[derive(IntoElement)]` would generate.
macro_rules! render_once_element {
    ($t:ty) => {
        $crate::interact::builder_traits!($t);
        impl gpui_kit::IntoElement for $t {
            type Element = gpui_kit::ViewElement<Self>;
            #[track_caller]
            fn into_element(self) -> Self::Element {
                gpui_kit::ViewElement::new(self)
            }
        }
    };
}
pub(crate) use render_once_element;

#[cfg(test)]
mod tests {
    use super::{kbd_label, parse_keys};

    #[test]
    fn shortcut_labels_use_platform_style() {
        assert!(parse_keys("ctrl-k").is_some());
        assert!(parse_keys("Ctrl+K").is_some());
        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(kbd_label("ctrl-k"), "Ctrl+K");
            assert_eq!(kbd_label("Ctrl+Shift+P"), "Ctrl+Shift+P");
            assert_eq!(kbd_label("escape"), "Esc");
        }
        assert_eq!(kbd_label(""), "");
    }
}
