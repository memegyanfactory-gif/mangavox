//! The app update in the sidebar footer, built from the kit on the shared `mv_ui::update` state.
//!
//! Quiet unless there is something to do: nothing while up to date (the version lives in
//! Settings → About), an offer card when an update is out, live progress while it downloads, and
//! a remedy when it fails. In the 60px rail the offer opens a confirmation first, because it
//! downloads about 200 MB and restarts the app.
//! NN/g progress indicators: <https://www.nngroup.com/articles/progress-indicators/>;
//! NN/g confirmation dialogs: <https://www.nngroup.com/articles/confirmation-dialog/>
use gpui_kit::*;
use mv_ui::theme::{icon as icon_px, radius};
use mv_ui::update::{updater, version, Status};
use mv_ui::{btn, confirm_dialog, glyphs, icon_button, icon_ph, ph, progress_bar, progress_ring_sized, size, spinner, tip, ButtonKind, Tokens};

/// Update UI for the sidebar footer, or `None` when there is nothing to say.
pub fn footer(rail: bool, cx: &mut App) -> Option<AnyElement> {
    let u = updater(cx);
    let (st, open, pct) = {
        let r = u.read(cx);
        (r.status.clone(), r.prompt, r.progress())
    };
    let percent = (pct * 100.).round() as i32;
    if rail {
        return rail_item(&st, open, percent, pct);
    }
    let a = u.clone();
    let b = u.clone();
    let el = match st {
        Status::Available(release) if open => card(ph::ARROW_CIRCLE_UP, Tokens::accent(), "Update available")
            .border_color(Tokens::accent_border())
            .child(body(format!("MangaVox v{} is out (you have v{}). It downloads in the background and the app restarts when it is done.", release.version, version())))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .child(btn("update-later", "Later", ButtonKind::Ghost).small().on_click(move |_, _, cx| a.update(cx, |u, cx| u.dismiss(cx))))
                    .child(btn("update-now", "Download", ButtonKind::Primary).small().icon(glyphs::DOWNLOAD).on_click(move |_, _, cx| b.update(cx, |u, cx| u.install(cx)))),
            )
            .into_any_element(),
        // Offer dismissed with "Later": a quiet row brings the card back.
        Status::Available(release) => btn("update-open", format!("Update to v{}", release.version), ButtonKind::Secondary)
            .small()
            .full_width()
            .icon(ph::ARROW_CIRCLE_UP)
            .tooltip("Download the new version and restart")
            .on_click(move |_, _, cx| {
                a.update(cx, |u, cx| {
                    u.prompt = true;
                    cx.notify();
                })
            })
            .into_any_element(),
        Status::Downloading(release) => card(glyphs::DOWNLOAD, Tokens::accent(), format!("Downloading v{}", release.version))
            .child(progress_bar("update-progress", pct, true))
            .child(body(format!("{percent}% · MangaVox restarts when it is done.")))
            .into_any_element(),
        Status::Installing(release) => div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(12.))
            .rounded(px(radius::LG))
            .bg(Tokens::surface())
            .border_1()
            .border_color(Tokens::border())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(spinner("update-installing", Tokens::accent(), icon_px::SM))
                    .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("Installing v{}", release.version))),
            )
            .child(body("Restarting MangaVox…".into()))
            .into_any_element(),
        Status::Failed(e) if open => card(ph::WARNING_CIRCLE, Tokens::danger(), "Update failed")
            .border_color(Tokens::danger_border())
            .child(body(reason(&e).to_string()))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .child(btn("update-close", "Close", ButtonKind::Ghost).small().on_click(move |_, _, cx| a.update(cx, |u, cx| u.dismiss(cx))))
                    .child(btn("update-retry", "Try again", ButtonKind::Secondary).small().icon(glyphs::RETRY).on_click(move |_, _, cx| b.update(cx, |u, cx| u.check(true, true, cx)))),
            )
            .into_any_element(),
        _ => return None,
    };
    Some(el)
}

/// The rail shows one 32px icon with the whole story in its tooltip. A failure shows while the
/// offer is open, as the expanded card does (so both sidebars agree).
fn rail_item(st: &Status, open: bool, percent: i32, pct: f32) -> Option<AnyElement> {
    let el = match st {
        Status::Available(release) => {
            let v = release.version.clone();
            icon_button("update-rail", ph::ARROW_CIRCLE_UP, format!("MangaVox v{v} is available"))
                .color(Tokens::accent())
                .on_click(move |_, window, cx| confirm_install(&v, window, cx))
                .into_any_element()
        }
        Status::Downloading(_) => div()
            .id("update-rail-progress")
            .flex()
            .items_center()
            .justify_center()
            .size(px(32.))
            .tooltip(tip(format!("Downloading update · {percent}%")))
            .child(progress_ring_sized(pct, icon_px::LG))
            .into_any_element(),
        Status::Installing(_) => div()
            .id("update-rail-installing")
            .flex()
            .items_center()
            .justify_center()
            .size(px(32.))
            .tooltip(tip("Installing the update · MangaVox restarts by itself"))
            .child(spinner("update-rail-spin", Tokens::accent(), icon_px::MD))
            .into_any_element(),
        Status::Failed(e) if open => icon_button("update-rail-failed", ph::WARNING_CIRCLE, format!("Update failed: {}. Click to try again.", reason(e)))
            .color(Tokens::danger())
            .on_click(|_, _, cx| updater(cx).update(cx, |u, cx| u.check(true, true, cx)))
            .into_any_element(),
        _ => return None,
    };
    Some(el)
}

/// Asks before the ~200 MB download and restart (the expanded card asks with its own buttons).
/// Confirming opens the offer too, so a failed download shows its "Update failed" card with
/// Try again in the expanded sidebar, not only in the rail.
pub fn confirm_install(new_version: &str, window: &mut Window, cx: &mut App) {
    confirm_dialog(
        format!("Update to MangaVox v{new_version}?"),
        format!("You have v{}. The update downloads in the background and MangaVox restarts by itself when it is done, so finish any work in progress first.", version()),
        "Download and restart",
        false,
    )
    .cancel_label("Not now")
    .icon(ph::ARROW_CIRCLE_UP)
    .on_confirm(|_, cx| {
        updater(cx).update(cx, |u, cx| {
            u.prompt = true;
            u.install(cx);
        })
    })
    .open(window, cx);
}

fn card(ic: mv_ui::Ph, color: Rgba, title: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .p(px(12.))
        .rounded(px(radius::LG))
        .bg(Tokens::surface())
        .border_1()
        .border_color(Tokens::border())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(icon_ph(ic, color, icon_px::SM))
                .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title.into())),
        )
}

/// The error without the "Update failed: " the updater puts in front of install errors, which
/// the card title and tooltip already say.
fn reason(e: &str) -> &str {
    e.strip_prefix("Update failed: ").unwrap_or(e)
}

fn body(text: String) -> Div {
    div().text_size(px(size::XS)).line_height(px(16.)).text_color(Tokens::text_2()).child(text)
}

#[cfg(test)]
mod tests {
    use super::reason;

    #[test]
    fn failure_text_does_not_repeat_the_title() {
        assert_eq!(reason("Update failed: disk full"), "disk full");
        assert_eq!(reason("GitHub did not answer"), "GitHub did not answer");
    }
}
