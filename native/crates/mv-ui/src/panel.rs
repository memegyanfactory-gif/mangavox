//! The app-wide side panel: Activity (what is running now), Issues (what failed, with its fix)
//! and Chat (the assistant), next to every page, the reader and the player included. The shell
//! draws it and owns its layout; this module is the state they share, so a page that covers the
//! shell's title bar (the player draws its own) can offer the same toggle and leave room for it.
//!
//! - Any toggle (the title bar, the player's top bar, Ctrl+J) calls [`toggle`], [`show`] or
//!   [`set_tab`].
//! - The shell publishes [`set_docked`] (the width the panel takes beside the page) and
//!   [`set_status`] (the live dot and the issue count every toggle shows).
//! - Follow it with `cx.observe_global::<SidePanel>(..)`. The global is only replaced when a
//!   value really changes, so observers never run for nothing and publishing from a render
//!   cannot loop.
//!
//! HIG split views (supplementary content on the trailing side):
//! <https://developer.apple.com/design/human-interface-guidelines/split-views>;
//! NN/g visibility of system status: <https://www.nngroup.com/articles/visibility-system-status/>
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::icons::{ph_icon, Ph, Weight};
use crate::interact::Interactive as _;
use crate::motion::pulse_dot;
use crate::theme::{icon as icon_px, radius, size, Tokens};

/// Shows or hides the panel on every page. The shell binds it; pages name it in tooltips.
pub const SHORTCUT: &str = "ctrl-j";

/// The panel's tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PanelTab {
    /// Downloads, Studio's runs, jobs and exports while they work.
    #[default]
    Activity,
    /// Failures and missing pieces, each with its fix.
    Issues,
    /// The assistant (Studio mode only: the assistant is a Studio feature).
    Chat,
}

impl PanelTab {
    pub const ALL: [PanelTab; 3] = [PanelTab::Activity, PanelTab::Issues, PanelTab::Chat];

    pub fn label(self) -> &'static str {
        match self {
            PanelTab::Activity => "Activity",
            PanelTab::Issues => "Issues",
            PanelTab::Chat => "Chat",
        }
    }

    /// Stable id for saved settings.
    pub fn id(self) -> &'static str {
        match self {
            PanelTab::Activity => "activity",
            PanelTab::Issues => "issues",
            PanelTab::Chat => "chat",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.id() == id)
    }
}

/// The panel as the shell last published it. Read it with [`state`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidePanel {
    /// The user wants it open. A page with a panel of its own (Studio) shows that one instead.
    pub open: bool,
    pub tab: PanelTab,
    /// The width (logical px) the panel takes beside the page right now: 0 while it is closed,
    /// not shown on this page, or floating over the page. The player keeps its video out of it.
    pub docked: f32,
    /// Something is running (a download, a Studio step, an export): the toggles' live dot.
    pub live: bool,
    /// Problems waiting for a fix: the toggles' count.
    pub issues: usize,
}

impl Global for SidePanel {}

pub fn state(cx: &App) -> SidePanel {
    cx.try_global::<SidePanel>().cloned().unwrap_or_default()
}

/// Applies `f`, and replaces the global only when that changed something.
fn update(cx: &mut App, f: impl FnOnce(&mut SidePanel)) {
    let before = state(cx);
    let mut next = before.clone();
    f(&mut next);
    if next != before {
        cx.set_global(next);
    }
}

/// Opens the panel when it is closed, closes it when it is open.
pub fn toggle(cx: &mut App) {
    update(cx, |s| s.open = !s.open);
}

pub fn set_open(open: bool, cx: &mut App) {
    update(cx, |s| s.open = open);
}

/// Opens the panel on `tab`.
pub fn show(tab: PanelTab, cx: &mut App) {
    update(cx, |s| {
        s.open = true;
        s.tab = tab;
    });
}

pub fn set_tab(tab: PanelTab, cx: &mut App) {
    update(cx, |s| s.tab = tab);
}

/// The shell: the width the docked panel takes beside the page (0 when it takes none).
pub fn set_docked(width: f32, cx: &mut App) {
    let w = if width.is_finite() { width.max(0.).round() } else { 0. };
    update(cx, |s| s.docked = w);
}

/// The shell: whether something runs, and how many issues wait for a fix.
pub fn set_status(live: bool, issues: usize, cx: &mut App) {
    update(cx, |s| {
        s.live = live;
        s.issues = issues;
    });
}

/// The panel is open over the page (a narrow window) instead of beside it. A page that uses
/// Escape itself (the reader) closes the panel first: the shell never sees that key. Only
/// meaningful on pages that show the panel (all but Studio and the welcome setup).
pub fn floats_over_page(cx: &App) -> bool {
    let s = state(cx);
    s.open && s.docked <= 0.
}

/// A toggle's tooltip: what a click does, and the news when there is some.
pub fn toggle_label(s: &SidePanel) -> String {
    if s.open {
        return "Hide activity, issues and chat".into();
    }
    match (s.issues, s.live) {
        (0, false) => "Show activity, issues and chat".into(),
        (0, true) => "Show activity: something is running".into(),
        (1, _) => "Show activity and issues: 1 issue to fix".into(),
        (n, _) => format!("Show activity and issues: {n} issues to fix"),
    }
}

/// A toggle's corner mark: the issue count while there are issues, otherwise a breathing dot
/// while something runs, otherwise nothing. Put it over the button's top-right corner
/// (`div().relative().child(button).children(mark.map(|m| div().absolute().top(..).right(..).child(m)))`).
pub fn status_mark(id: impl Into<ElementId>, s: &SidePanel) -> Option<AnyElement> {
    if s.issues > 0 {
        let text = if s.issues > 9 { "9+".to_string() } else { s.issues.to_string() };
        // The same soft warning colours as Studio's issues pill.
        return Some(
            div()
                .flex()
                .items_center()
                .justify_center()
                .min_w(px(16.))
                .h(px(16.))
                .px(px(4.))
                .rounded_full()
                .bg(Tokens::warning_soft())
                .border_1()
                .border_color(Tokens::warning_border())
                .text_size(px(10.))
                .line_height(px(14.))
                .font_weight(FontWeight::BOLD)
                .text_color(Tokens::text_1())
                .child(text)
                .into_any_element(),
        );
    }
    s.live.then(|| pulse_dot(id, Tokens::accent(), 7.))
}

/// The panel's tab strip, also used by Studio's own side panel: one segment per tab (`label`,
/// icon, and `live` for a breathing dot while that tab has news and is not the one open).
pub fn tab_bar<T: Copy + PartialEq + 'static>(id: &'static str, tabs: &[(T, &'static str, Ph, bool)], current: T, on_pick: impl Fn(&T, &mut Window, &mut App) + 'static) -> Div {
    let on_pick = Rc::new(on_pick);
    div().flex().items_center().gap(px(2.)).p(px(3.)).rounded(px(radius::MD)).bg(Tokens::well()).border_1().border_color(Tokens::border()).children(tabs.iter().enumerate().map(
        |(i, &(t, label, icon, live))| {
            let on = t == current;
            let f = on_pick.clone();
            let fg = if on { Tokens::text_1() } else { Tokens::text_2() };
            div()
                .id((id, i))
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .h(px(28.))
                .rounded(px(radius::SM))
                .text_size(px(size::SM))
                .text_color(fg)
                .role(Role::Tab)
                .aria_selected(on)
                .map(|d| if on { d.bg(Tokens::surface_3()).border_1().border_color(Tokens::border_strong()).font_weight(FontWeight::MEDIUM) } else { d.hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1())) })
                .pressable()
                .on_click(move |_, w, cx| f(&t, w, cx))
                .child(ph_icon(icon, if on { Weight::Fill } else { Weight::Bold }, if on { Tokens::accent() } else { Tokens::text_3() }, icon_px::SM))
                .child(label)
                .when(live && !on, |d| d.child(pulse_dot((id, 100 + i), Tokens::accent(), 6.)))
        },
    ))
}

#[cfg(test)]
mod tests {
    // Explicit imports: a glob of `super` would pull in GPUI's `test` macro over Rust's.
    use super::{toggle_label, PanelTab, SidePanel};

    #[test]
    fn tab_ids_round_trip() {
        for t in PanelTab::ALL {
            assert_eq!(PanelTab::from_id(t.id()), Some(t));
        }
        assert_eq!(PanelTab::from_id("cast"), None);
        assert_eq!(PanelTab::default(), PanelTab::Activity);
    }

    #[test]
    fn the_tooltip_says_what_a_click_does_and_the_news() {
        let mut s = SidePanel::default();
        assert_eq!(toggle_label(&s), "Show activity, issues and chat");
        s.live = true;
        assert!(toggle_label(&s).contains("running"));
        s.issues = 3;
        assert!(toggle_label(&s).contains("3 issues"));
        s.open = true;
        assert!(toggle_label(&s).starts_with("Hide"));
    }
}
