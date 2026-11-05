//! The app-wide side panel's content (Activity, Issues, Chat). The shell draws the column, owns
//! its layout and keeps the Studio and assistant views; this builds what goes inside, so the
//! shell stays about layout. The state every toggle shares lives in `mv_ui::panel`.
//!
//! Studio has the same panel built into its page (ui-studio side.rs); its Activity and Issues
//! come from `ui_studio::panel`, so both panels show the same sentences and fixes.
use std::collections::HashSet;

use gpui_kit::component::scroll::{Scrollbar, ScrollbarAxis, ScrollbarMode};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::panel::PanelTab;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::{btn, empty_view, glyphs, icon_button, icon_ph, ph, ph_icon, size, ButtonKind, Tokens, Weight};

use crate::shell::Shell;

/// The views and scroll handles the shell keeps for the panel.
pub struct Views<'a> {
    /// Studio's Activity and Issues sections (Studio mode only), created by the shell once.
    pub studio_activity: Option<&'a AnyView>,
    pub studio_issues: Option<&'a AnyView>,
    /// The assistant.
    pub chat: Option<&'a AnyView>,
    pub activity_scroll: &'a ScrollHandle,
    pub issues_scroll: &'a ScrollHandle,
}

/// The issue rows the user closed. A title's failed chapters stay in the download queue after
/// its card is closed (Retry still needs them), so the panel remembers which counts it hid.
#[derive(Default)]
struct Dismissed(HashSet<u64>);

impl Global for Dismissed {}

/// One title whose chapters failed to download.
#[derive(Clone, Debug, PartialEq)]
pub struct FailedTitle {
    pub manga_id: String,
    pub title: String,
    pub chapters: usize,
    /// The error the last failure gave, as detail under the sentence.
    pub error: Option<String>,
    /// Which count of the title this is (a new count is a new problem).
    pub serial: u64,
}

/// Titles with failed chapters the user has not closed.
pub fn failed_downloads(cx: &mut App) -> Vec<FailedTitle> {
    let dl = ui_manga::downloads(cx);
    let closed = cx.try_global::<Dismissed>().map(|d| d.0.clone()).unwrap_or_default();
    dl.read(cx)
        .activity()
        .into_iter()
        .filter(|a| a.failed > 0 && !closed.contains(&a.serial))
        .map(|a| FailedTitle { manga_id: a.manga_id, title: a.title, chapters: a.failed, error: a.last_error, serial: a.serial })
        .collect()
}

/// "Solo Leveling: 2 chapters failed to download."
fn sentence(f: &FailedTitle) -> String {
    let name = if f.title.trim().is_empty() { "A manga" } else { f.title.trim() };
    if f.chapters == 1 {
        format!("{name}: 1 chapter failed to download.")
    } else {
        format!("{name}: {} chapters failed to download.", f.chapters)
    }
}

/// The panel: the header (tabs and Hide) and the open tab's body.
pub fn content(tab: PanelTab, studio: bool, live: bool, issues: usize, views: &Views, cx: &mut Context<Shell>) -> Div {
    let body = match tab {
        PanelTab::Activity => activity(studio, views, cx),
        PanelTab::Issues => issues_tab(studio, views, cx),
        PanelTab::Chat => chat(views, cx),
    };
    div().flex().flex_col().size_full().child(header(tab, studio, live, issues)).child(body)
}

/// 52px like Studio's panel header: the tab strip and a small Hide button.
fn header(tab: PanelTab, studio: bool, live: bool, issues: usize) -> Div {
    let mut tabs = vec![(PanelTab::Activity, "Activity", ph::PULSE, live), (PanelTab::Issues, "Issues", ph::WARNING, issues > 0)];
    if studio {
        tabs.push((PanelTab::Chat, "Chat", ph::CHAT_CIRCLE_DOTS, false));
    }
    let bar = mv_ui::panel::tab_bar("app-panel-tabs", &tabs[..], tab, |t, _, cx| mv_ui::panel::set_tab(*t, cx));
    div()
        .flex_none()
        .h(px(52.))
        .flex()
        .items_center()
        .gap(px(space::SM))
        .px(px(space::MD))
        .border_b_1()
        .border_color(Tokens::border())
        .child(div().flex_1().min_w_0().child(bar))
        .child(icon_button("app-panel-close", ph::X, "Hide").kbd(mv_ui::panel::SHORTCUT).on_click(|_, _, cx| mv_ui::panel::set_open(false, cx)))
}

/// A scrolling body with an always-visible scrollbar, like Studio's.
fn scroller(id: &'static str, bar_id: &'static str, handle: &ScrollHandle, body: impl FnOnce(Stateful<Div>) -> Stateful<Div>) -> Div {
    let list = div().id(id).flex().flex_col().p(px(space::MD)).gap(px(space::MD)).overflow_y_scroll();
    let list = body(list).size_full().track_scroll(handle);
    let bar = div().absolute().inset_0().child(Scrollbar::new(handle).id(bar_id).axis(ScrollbarAxis::Vertical).mode(ScrollbarMode::Always).viewport_from_layout());
    div().relative().flex_1().min_h_0().child(list).child(bar)
}

/// A section header in the style of Studio's "Exports".
fn section_header(icon: mv_ui::Ph, text: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(space::SM))
        .pt(px(space::XS))
        .child(icon_ph(icon, Tokens::text_3(), icon_px::SM))
        .child(div().flex_1().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_2()).child(text))
}

fn activity(studio: bool, views: &Views, cx: &mut Context<Shell>) -> Div {
    let cards = ui_manga::download_cards(cx);
    let has_cards = !cards.is_empty();
    let downloads = mv_ui::activity::inline(cards);
    let studio_any = studio && ui_studio::panel::summary(cx).any;
    let idle = !has_cards && !studio_any;
    let studio_view = views.studio_activity.filter(|_| studio).cloned();
    scroller("app-panel-activity", "app-panel-activity-bar", views.activity_scroll, move |d| {
        d.when(has_cards, |d| d.child(section_header(glyphs::DOWNLOADS, "Downloads")))
            .children(downloads)
            .children(studio_view)
            .when(idle, |d| {
                d.child(empty_view(ph::PULSE, "Nothing running", "Downloads, Studio's Generate runs and exports show up here while they work.").compact())
            })
    })
}

fn issues_tab(studio: bool, views: &Views, cx: &mut Context<Shell>) -> Div {
    let failed = failed_downloads(cx);
    let studio_issues = studio && ui_studio::panel::summary(cx).issues > 0;
    let studio_view = views.studio_issues.filter(|_| studio).cloned();
    let empty = failed.is_empty() && !studio_issues;
    scroller("app-panel-issues", "app-panel-issues-bar", views.issues_scroll, move |d| {
        d.children(studio_view)
            .children(failed.into_iter().enumerate().map(|(i, f)| download_issue(i, &f)))
            .when(empty, |d| d.child(empty_view(ph::CHECK_CIRCLE, "No issues", "When something fails, it shows up here with the fix.").compact()))
    })
}

/// One title's failed downloads, drawn like Studio's issue cards: a sentence, the error as
/// detail, and the fix.
fn download_issue(ix: usize, f: &FailedTitle) -> AnyElement {
    let (retry_id, dismiss_id, serial) = (f.manga_id.clone(), f.manga_id.clone(), f.serial);
    div()
        .id(("app-dl-issue", ix))
        .flex()
        .gap(px(space::SM))
        .p(px(space::SM))
        .rounded(px(radius::MD))
        .bg(Tokens::danger_soft())
        .border_1()
        .border_color(Tokens::danger_border())
        .child(div().pt(px(1.)).child(ph_icon(ph::WARNING, Weight::Fill, Tokens::danger(), icon_px::MD)))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(6.))
                .child(div().text_size(px(size::SM)).line_height(px(18.)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(sentence(f)))
                .when_some(f.error.clone(), |d, e| d.child(div().text_size(px(size::XS)).line_height(px(16.)).text_color(Tokens::text_3()).child(e)))
                .child(
                    div().flex().gap(px(6.)).child(btn(("app-dl-retry", ix), "Retry", ButtonKind::Primary).small().icon(ph::ARROW_CLOCKWISE).on_click(move |_, _, cx| {
                        let id = retry_id.clone();
                        ui_manga::downloads(cx).update(cx, |d, cx| d.retry_manga(&id, cx));
                    })),
                ),
        )
        .child(icon_button(("app-dl-dismiss", ix), ph::X, "Dismiss").small().on_click(move |_, _, cx| {
            cx.default_global::<Dismissed>().0.insert(serial);
            let id = dismiss_id.clone();
            // Hides the title's card too, and wakes the shell so the row goes now.
            ui_manga::downloads(cx).update(cx, |d, cx| d.dismiss(&id, cx));
        }))
        .into_any_element()
}

fn chat(views: &Views, cx: &mut Context<Shell>) -> Div {
    match views.chat {
        Some(v) => div().flex_1().min_h_0().child(v.clone()),
        None => div().p(px(space::LG)).child(
            empty_view(glyphs::ASSISTANT, "Chat unavailable", "The assistant could not start.")
                .compact()
                .retry(cx.listener(|this, _: &ClickEvent, window, cx| this.retry_chat(window, cx))),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{sentence, FailedTitle};

    fn failed(title: &str, chapters: usize) -> FailedTitle {
        FailedTitle { manga_id: "m".into(), title: title.into(), chapters, error: None, serial: 1 }
    }

    #[test]
    fn the_sentence_counts_chapters() {
        assert_eq!(sentence(&failed("Solo Leveling", 2)), "Solo Leveling: 2 chapters failed to download.");
        assert_eq!(sentence(&failed("Berserk", 1)), "Berserk: 1 chapter failed to download.");
        assert_eq!(sentence(&failed(" ", 3)), "A manga: 3 chapters failed to download.");
    }
}
