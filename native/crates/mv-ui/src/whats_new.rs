//! "What's new": the release notes of this version, shown once after an update (and from
//! Settings → About). The notes are the GitHub release's body, which the release workflow
//! writes from the commits: `### Title` sections with `- ` items.
use gpui_kit::component::WindowExt as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::controls::{btn, ButtonKind};
use crate::icons::{ph, Weight};
use crate::interact::{glyph, IconSrc};
use crate::theme::{icon as icon_px, ramp, size, RampExt, Tokens};

/// config.json key: the version whose notes were last shown.
const SEEN_KEY: &str = "update.seenVersion";

/// One part of the notes: a title (none for notes without headings) and its items.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Section {
    pub title: String,
    pub items: Vec<String>,
}

/// Reads release notes into sections: `###`/`##` headings start one (the "What's new in"
/// banner is skipped), `- `/`* ` items belong to the last one (indented lines continue an
/// item), other text becomes an item of its own. The changelog link and empty sections go.
pub fn parse_notes(md: &str) -> Vec<Section> {
    let mut out: Vec<Section> = vec![];
    for raw in md.lines() {
        let line = raw.trim_end();
        let t = line.trim();
        if t.is_empty() || t.to_lowercase().contains("full changelog") {
            continue;
        }
        if let Some(h) = t.strip_prefix("###").or_else(|| t.strip_prefix("##")).or_else(|| t.strip_prefix('#')) {
            let h = h.trim().trim_start_matches('#').trim();
            if !h.to_lowercase().starts_with("what's new") {
                out.push(Section { title: tidy(h), items: vec![] });
            }
            continue;
        }
        if out.is_empty() {
            out.push(Section::default());
        }
        let sec = out.last_mut().expect("a section");
        if let Some(item) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            sec.items.push(tidy(item));
        } else if line.starts_with(' ') && !sec.items.is_empty() {
            let last = sec.items.last_mut().expect("an item");
            last.push(' ');
            last.push_str(&plain(t));
        } else {
            sec.items.push(tidy(t));
        }
    }
    out.retain(|s| !s.title.is_empty() || !s.items.is_empty());
    out
}

/// Without markdown emphasis or code marks.
fn plain(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, '*' | '`')).collect::<String>().trim().to_string()
}

/// [`plain`], with a leading capital.
fn tidy(s: &str) -> String {
    let s = plain(s);
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Whether this version's notes are still to be shown: the app was updated since they were
/// last shown. The very first run only remembers the version (nothing was "updated").
fn due(seen: Option<&str>, current: &str) -> bool {
    seen.is_some_and(|s| s != current)
}

fn seen() -> Option<String> {
    mv_core::config::Config::open_default().get(SEEN_KEY).and_then(|v| v.as_str().map(str::to_string))
}

fn mark_seen() {
    let _ = mv_core::config::Config::open_default().set(SEEN_KEY, serde_json::json!(crate::update::version()));
}

/// At startup: after an update, fetches this version's notes and shows them once.
pub fn check_on_start(window: &mut Window, cx: &mut App) {
    let current = crate::update::version();
    let seen = seen();
    if seen.is_none() {
        mark_seen();
    }
    if !due(seen.as_deref(), current) {
        return;
    }
    show(window, cx, true);
}

/// Fetches this version's notes and opens them (`once`: remembered as shown, and nothing is
/// said when they cannot be fetched).
pub fn show(window: &mut Window, cx: &mut App, once: bool) {
    let version = crate::update::version().to_string();
    window
        .spawn(cx, async move |cx| {
            let v = version.clone();
            let res = crate::bg::run(async move { mv_update::notes_for(&v).await }).await.and_then(|r| r);
            let _ = cx.update(|window, cx| match res {
                Ok((notes, url)) => {
                    if once {
                        mark_seen();
                    }
                    open(&version, parse_notes(&notes), url, window, cx);
                }
                Err(e) if !once => crate::toast::show(cx, crate::toast::Toast::error(format!("Could not load what's new: {e:#}"))),
                Err(_) => {}
            });
        })
        .detach();
}

/// The "What's new in MangaVox vX" dialog.
pub fn open(version: &str, sections: Vec<Section>, url: String, window: &mut Window, cx: &mut App) {
    let title = format!("What's new in MangaVox v{version}");
    let sections = if sections.is_empty() { vec![Section { title: String::new(), items: vec!["Fixes and improvements.".into()] }] } else { sections };
    window.open_dialog(cx, move |dialog, _, _| {
        let heading = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(glyph(IconSrc::Ph(ph::SPARKLE), Weight::Fill, Tokens::accent(), icon_px::LG))
            .child(div().ramp(ramp::SUBTITLE_2).text_color(Tokens::text_1()).child(title.clone()));
        let body = div().id("whats-new-body").flex().flex_col().gap(px(16.)).max_h(px(440.)).overflow_y_scroll().pr(px(4.)).children(sections.iter().map(|s| {
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .when(!s.title.is_empty(), |d| d.child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(s.title.clone())))
                .children(s.items.iter().map(|i| {
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(div().flex_none().mt(px(7.)).size(px(5.)).rounded_full().bg(Tokens::accent()))
                        .child(div().flex_1().min_w_0().text_size(px(size::SM)).line_height(px(19.)).text_color(Tokens::text_2()).child(i.clone()))
                }))
        }));
        let link = url.clone();
        let footer = div()
            .flex()
            .justify_end()
            .gap(px(8.))
            .w_full()
            .when(!link.is_empty(), |d| d.child(btn("whats-new-web", "View on GitHub", ButtonKind::Secondary).on_click(move |_, _, cx| cx.open_url(&link))))
            .child(btn("whats-new-ok", "Got it", ButtonKind::Primary).on_click(|_, w, cx| w.close_dialog(cx)));
        dialog.w(px(560.)).title(heading).child(body).footer(footer).on_ok(|_, _, _| true).on_cancel(|_, _, _| true)
    });
}

#[cfg(test)]
mod tests {
    use super::{due, parse_notes, Section};

    #[test]
    fn notes_read_into_sections() {
        let md = "## What's new in MangaVox v8.0.5039\n\n### Stacked bottom-right notifications, phone touchpad\n- Notifications: solid cards stacked in the bottom-right, one per manga\n  download (pause/cancel per card).\n- **Phone**: a Touchpad screen\n\n### fix: quitting ends the process\nAn old copy no longer stays.\n\n**Full changelog**: https://github.com/x/y/compare/a...b\n";
        let s = parse_notes(md);
        assert_eq!(
            s,
            vec![
                Section {
                    title: "Stacked bottom-right notifications, phone touchpad".into(),
                    items: vec!["Notifications: solid cards stacked in the bottom-right, one per manga download (pause/cancel per card).".into(), "Phone: a Touchpad screen".into()]
                },
                Section { title: "Fix: quitting ends the process".into(), items: vec!["An old copy no longer stays.".into()] },
            ]
        );
        // Only a changelog link (the old notes): nothing to show.
        assert!(parse_notes("**Full Changelog**: https://github.com/x/y/compare/a...b").is_empty());
        // No headings: one untitled section.
        assert_eq!(parse_notes("- one\n- two")[0].items, vec!["One", "Two"]);
    }

    #[test]
    fn shown_once_after_an_update_never_on_the_first_run() {
        assert!(!due(None, "8.0.5040"), "first run");
        assert!(!due(Some("8.0.5040"), "8.0.5040"), "already shown");
        assert!(due(Some("8.0.5039"), "8.0.5040"), "updated");
    }
}
