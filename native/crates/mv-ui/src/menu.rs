//! Menus: an icon or text button that opens a gpui-component `PopupMenu` (keyboard navigation,
//! Escape and click-outside come from the library), plus `menu_row` and `popup_surface` for
//! custom popovers such as pickers.
use std::rc::Rc;

use gpui_kit::base::Selectable;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::controls::{ClickHandler, ControlSize, WindowHandler};
use crate::icons::{ph, Ph, Weight};
use crate::interact::{element_builder, glyph, kbd_label, refine, tip, IconSrc, Interactive};
use crate::theme::{icon as icon_px, radius, size, Tokens};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Action,
    Separator,
    Heading,
}

/// One entry of a [`menu_button`]: an action, a separator or a group heading.
#[derive(Clone)]
pub struct MenuItem {
    kind: Kind,
    label: SharedString,
    icon: Option<Ph>,
    checked: bool,
    danger: bool,
    kbd: Option<SharedString>,
    disabled: bool,
    handler: Option<WindowHandler>,
}

impl MenuItem {
    /// An action. Name it by its outcome ("Delete chapter", not "OK").
    pub fn new(label: impl Into<SharedString>, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        MenuItem { kind: Kind::Action, label: label.into(), icon: None, checked: false, danger: false, kbd: None, disabled: false, handler: Some(Rc::new(handler)) }
    }

    pub fn separator() -> Self {
        MenuItem { kind: Kind::Separator, label: SharedString::default(), icon: None, checked: false, danger: false, kbd: None, disabled: false, handler: None }
    }

    /// Non-interactive group heading ("Sort by").
    pub fn heading(label: impl Into<SharedString>) -> Self {
        MenuItem { kind: Kind::Heading, ..Self::separator() }.with_label(label)
    }

    fn with_label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }

    /// Leading icon; items without one still line up.
    pub fn icon(mut self, icon: Ph) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Marks the current choice (sort order, view) with a trailing check.
    /// Material 3 menus: <https://m3.material.io/components/menus/guidelines>
    pub fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }

    /// Destructive action in danger red; put it last, after a separator.
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    /// Shortcut hint shown at the end, e.g. `"ctrl-d"`.
    pub fn kbd(mut self, keys: impl Into<SharedString>) -> Self {
        self.kbd = Some(keys.into());
        self
    }

    pub fn disabled(mut self, off: bool) -> Self {
        self.disabled = off;
        self
    }
}

fn item_row(item: &MenuItem, lead: bool) -> Div {
    let fg = if item.disabled {
        Tokens::text_disabled()
    } else if item.danger {
        Tokens::danger()
    } else {
        Tokens::text_1()
    };
    let icon_fg = if item.disabled || item.danger { fg } else { Tokens::text_2() };
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .w_full()
        .min_h(px(30.))
        .text_size(px(size::MD))
        .text_color(fg)
        .when(lead, |d| {
            d.child(match item.icon {
                Some(i) => glyph(IconSrc::Ph(i), Weight::Regular, icon_fg, icon_px::MD).into_any_element(),
                None => div().size(px(icon_px::MD)).flex_none().into_any_element(),
            })
        })
        .child(div().flex_1().min_w_0().truncate().child(item.label.clone()))
        .when_some(item.kbd.clone(), |d, k| d.child(div().ml(px(16.)).flex_none().text_size(px(size::XS)).text_color(Tokens::text_3()).child(kbd_label(&k))))
        .when(item.checked, |d| d.child(glyph(IconSrc::Ph(ph::CHECK), Weight::Bold, Tokens::accent(), icon_px::SM)))
}

/// Icon button that opens a menu: `menu_button("sort", ph::SORT_ASCENDING, "Sort", items)`.
pub fn menu_button(id: impl Into<ElementId>, icon: Ph, tooltip: impl Into<SharedString>, items: Vec<MenuItem>) -> MenuButton {
    MenuButton { id: id.into(), icon: Some(icon), label: None, tooltip: tooltip.into(), items: Rc::new(items), anchor: Anchor::TopRight, size: ControlSize::Md, min_w: 200., style: StyleRefinement::default() }
}

/// The "More actions" (three dots) menu for rows, cards and headers.
pub fn overflow_menu(id: impl Into<ElementId>, items: Vec<MenuItem>) -> MenuButton {
    menu_button(id, ph::DOTS_THREE, "More actions", items)
}

/// See [`menu_button`].
pub struct MenuButton {
    id: ElementId,
    icon: Option<Ph>,
    label: Option<SharedString>,
    tooltip: SharedString,
    items: Rc<Vec<MenuItem>>,
    anchor: Anchor,
    size: ControlSize,
    min_w: f32,
    style: StyleRefinement,
}

impl Styled for MenuButton {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl IntoElement for MenuButton {
    type Element = AnyElement;
    fn into_element(self) -> AnyElement {
        self.build()
    }
}

impl MenuButton {
    /// Text trigger with a caret ("Sort: Title ▾") instead of an icon-only one.
    pub fn label(mut self, text: impl Into<SharedString>) -> Self {
        self.label = Some(text.into());
        self
    }

    /// Which corner of the trigger the menu hangs from (default top-right, opening downward).
    pub fn anchor(mut self, anchor: Anchor) -> Self {
        self.anchor = anchor;
        self
    }

    pub fn size(mut self, size: ControlSize) -> Self {
        self.size = size;
        self
    }

    pub fn small(self) -> Self {
        self.size(ControlSize::Sm)
    }

    pub fn min_width(mut self, px_w: f32) -> Self {
        self.min_w = px_w;
        self
    }

    pub fn build(self) -> AnyElement {
        let MenuButton { id, icon, label, tooltip, items, anchor, size: sz, min_w, style } = self;
        let ic = sz.icon();
        let base = div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .h(px(sz.height()))
            .rounded(px(radius::MD))
            .text_size(px(size::SM))
            .font_weight(FontWeight::MEDIUM)
            .text_color(Tokens::text_2())
            .role(Role::Button)
            .aria_label(tooltip.clone())
            .group("mv-menu")
            .pressable()
            .hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
            .map(|d| if label.is_some() { d.px(px(10.)) } else { d.w(px(sz.height())) })
            .when_some(icon, |d, i| d.child(glyph(IconSrc::Ph(i), if ic <= 14. { Weight::Bold } else { Weight::Regular }, Tokens::text_2(), ic).group_hover("mv-menu", |s| s.text_color(Tokens::text_1()))))
            .when_some(label, |d, l| d.child(l).child(glyph(IconSrc::Ph(ph::CARET_DOWN), Weight::Bold, Tokens::text_3(), icon_px::XS)));
        let lead = items.iter().any(|i| i.icon.is_some());
        MenuTrigger { el: refine(base, &style), tooltip, open: false }
            .dropdown_menu_with_anchor(anchor, move |menu, _, _| {
                let mut m = menu.min_w(px(min_w));
                for item in items.iter() {
                    m = match item.kind {
                        Kind::Separator => m.separator(),
                        Kind::Heading => m.label(item.label.clone()),
                        Kind::Action => {
                            let row = item.clone();
                            let handler = item.handler.clone();
                            m.item(
                                PopupMenuItem::element(move |_, _| item_row(&row, lead))
                                    .disabled(item.disabled)
                                    .when_some(handler, |p, h| p.on_click(move |_, w, cx| h(w, cx))),
                            )
                        }
                    };
                }
                m
            })
            .into_any_element()
    }
}

/// The trigger handed to gpui-component's dropdown: our styled div, shown pressed while open.
/// The tooltip is attached only while closed so it never sits on top of the menu.
struct MenuTrigger {
    el: Stateful<Div>,
    tooltip: SharedString,
    open: bool,
}

impl Styled for MenuTrigger {
    fn style(&mut self) -> &mut StyleRefinement {
        self.el.style()
    }
}

impl InteractiveElement for MenuTrigger {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.el.interactivity()
    }
}

impl Selectable for MenuTrigger {
    fn selected(mut self, on: bool) -> Self {
        self.open = on;
        self
    }

    fn is_selected(&self) -> bool {
        self.open
    }
}

impl IntoElement for MenuTrigger {
    type Element = Stateful<Div>;
    fn into_element(self) -> Stateful<Div> {
        let MenuTrigger { el, tooltip, open } = self;
        el.aria_expanded(open).map(|d| if open { d.bg(Tokens::active()).text_color(Tokens::text_1()) } else { d.tooltip(tip(tooltip)) })
    }
}

impl DropdownMenu for MenuTrigger {}

/// Row for custom popovers and pickers: optional leading icon, label with an optional hint line,
/// shortcut, and a check for the selected row. Hover is the only highlight, so the selected row
/// never looks hovered. Attach `.on_click(..)`.
pub fn menu_row(id: impl Into<ElementId>, label: impl Into<SharedString>) -> MenuRow {
    MenuRow { base: div().id(id), style: StyleRefinement::default(), label: label.into(), hint: None, icon: None, kbd: None, selected: false, danger: false, disabled: false, on_click: None }
}

/// See [`menu_row`].
pub struct MenuRow {
    base: Stateful<Div>,
    style: StyleRefinement,
    label: SharedString,
    hint: Option<SharedString>,
    icon: Option<Ph>,
    kbd: Option<SharedString>,
    selected: bool,
    danger: bool,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

element_builder!(MenuRow => Stateful<Div>);

impl MenuRow {
    /// Second, muted line under the label.
    pub fn hint(mut self, text: impl Into<SharedString>) -> Self {
        self.hint = Some(text.into());
        self
    }

    pub fn icon(mut self, icon: Ph) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn kbd(mut self, keys: impl Into<SharedString>) -> Self {
        self.kbd = Some(keys.into());
        self
    }

    pub fn selected(mut self, on: bool) -> Self {
        self.selected = on;
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn disabled(mut self, off: bool) -> Self {
        self.disabled = off;
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    pub fn build(self) -> Stateful<Div> {
        let MenuRow { base, style, label, hint, icon, kbd, selected, danger, disabled, on_click } = self;
        let fg = if disabled {
            Tokens::text_disabled()
        } else if danger {
            Tokens::danger()
        } else {
            Tokens::text_1()
        };
        let el = base
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(10.))
            .py(px(7.))
            .rounded(px(radius::SM))
            .role(Role::MenuItem)
            .aria_selected(selected)
            .when(!disabled, |d| d.interactive().when_some(on_click, |d, f| d.on_click(move |e, w, cx| f(e, w, cx))))
            .when_some(icon, |d, i| d.child(glyph(IconSrc::Ph(i), Weight::Regular, if danger || disabled { fg } else { Tokens::text_2() }, icon_px::MD)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().truncate().text_size(px(size::SM)).text_color(fg).child(label))
                    .when_some(hint, |d, h| d.child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(h))),
            )
            .when_some(kbd, |d, k| d.child(div().flex_none().text_size(px(size::XS)).text_color(Tokens::text_3()).child(kbd_label(&k))))
            .when(selected, |d| d.child(glyph(IconSrc::Ph(ph::CHECK), Weight::Bold, Tokens::accent(), icon_px::SM)));
        refine(el, &style)
    }
}

/// Floating surface for custom popovers: the highest elevation (`surface_3`), a strong hairline,
/// large radius and shadow, and it blocks clicks to what is underneath. Wrap it in
/// `deferred(anchored().child(..))` and fade it in with [`crate::motion::appear`].
pub fn popup_surface() -> Div {
    div()
        .flex()
        .flex_col()
        .p(px(4.))
        .rounded(px(radius::LG))
        .border_1()
        .border_color(Tokens::border_strong())
        .bg(Tokens::surface_3())
        .shadow_lg()
        .occlude()
}
