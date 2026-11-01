//! Whether a chapter's rendered video still shows what Studio would make now, and the one
//! question every way of watching or exporting asks before it uses the videos.
//!
//! The rendered files are the one source of truth: the dashboard's Play and Export, the Video
//! editor's monitor and the manga section (Watch video, Play all) all use the same
//! `video_final.mp4` of each chapter. A video is current when its look stamp
//! (`vedit_manga::STAMP_FILE`, written when its render finished) equals the key of what a render
//! would make now (`vedit_manga::current`: the Studio look, the chapter's music switch, its
//! captions and its Video-editor edit). Before a video plays or exports, the chapters that are not
//! current are offered once: "N chapters changed since they were rendered — Render them first /
//! Use the current videos". Nothing is re-rendered without asking, and only those chapters are.
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Mutex;
use std::time::Duration;

use gpui_kit::*;
use mv_ui::toast::{self, Toast};
use mv_ui::{confirm_dialog, ph};

use super::ChapterItem;
use crate::vedit::ChapterEdit;
use crate::vedit_manga as vm;

/// Why a chapter's video is not current.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// Its Video-editor edit (clips, layers, captions, music, its own look) changed since the render.
    Edit,
    /// The Studio Look, its captions or its music changed since the render (its edit did not).
    Look,
    /// Made before Studio stamped its videos, so what it was made with is not known.
    Unknown,
}

/// What decides a chapter's state, read from its files ([`facts`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Facts {
    /// The key its video was made with (`None`: made before videos were stamped).
    pub stamp: Option<u64>,
    /// The key a render would make now.
    pub key: u64,
    /// Its Video-editor edit, when it has one.
    pub edit: Option<ChapterEdit>,
    /// `vedit::render_key` of that edit (with the chapter's captions) now.
    pub edit_key: u64,
}

/// `None` when the video is current; else why not.
pub fn why_stale(f: &Facts) -> Option<Why> {
    if f.stamp == Some(f.key) {
        return None;
    }
    // An edit that was never rendered, or was changed since its last render.
    let edit_changed = f.edit.as_ref().is_some_and(|e| {
        let edited = e.rendered.is_some() || (ChapterEdit { rendered: None, ..e.clone() }) != ChapterEdit::default();
        edited && e.rendered != Some(f.edit_key)
    });
    Some(if edit_changed {
        Why::Edit
    } else if f.stamp.is_none() {
        Why::Unknown
    } else {
        Why::Look
    })
}

/// A chapter whose video is not current.
#[derive(Clone, Debug, PartialEq)]
pub struct Stale {
    pub item: ChapterItem,
    pub why: Why,
    /// The key a render would make now ("Use the current videos" is remembered against it).
    pub key: u64,
}

/// Reads what decides `ch`'s state (blocking); `look` is `look::video_config()`.
pub fn facts(ch: &ChapterItem, look: &serde_json::Value) -> Facts {
    let (key, edit, edit_key) = vm::current(ch, look);
    Facts { stamp: vm::stamp(&ch.output_dir), key, edit, edit_key }
}

/// The chapters of `chapters` that have a video which is not current, in order (blocking).
pub fn stale_of(chapters: &[ChapterItem]) -> Vec<Stale> {
    let look = crate::look::video_config();
    chapters
        .iter()
        .filter(|c| c.output_dir.join(super::VIDEO_FILE).metadata().is_ok_and(|m| m.len() > 0))
        .filter_map(|c| {
            let f = facts(c, &look);
            why_stale(&f).map(|why| Stale { item: c.clone(), why, key: f.key })
        })
        .collect()
}

// ------------------------------------------------------------------------------------ asking

/// (chapter, key) pairs the user chose to use as they are this session: not asked about again
/// until the chapter changes again.
static KEPT: Mutex<Option<HashSet<(String, u64)>>> = Mutex::new(None);

fn kept() -> HashSet<(String, u64)> {
    KEPT.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
}

fn keep(stale: &[Stale]) {
    let mut g = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    let set = g.get_or_insert_with(HashSet::new);
    set.extend(stale.iter().map(|s| (s.item.id.clone(), s.key)));
}

/// What to do before playing or exporting.
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    /// Every video is current (or the user already chose to use the ones that are not).
    Go,
    /// Ask about these chapters.
    Ask(Vec<Stale>),
}

/// The decision for `stale`, leaving out what the user chose to keep (`kept`).
pub fn decide(stale: Vec<Stale>, kept: &HashSet<(String, u64)>) -> Decision {
    let ask: Vec<Stale> = stale.into_iter().filter(|s| !kept.contains(&(s.item.id.clone(), s.key))).collect();
    if ask.is_empty() {
        Decision::Go
    } else {
        Decision::Ask(ask)
    }
}

/// The dialog's title and body for `stale` (not empty), before `verb` ("playing", "exporting").
pub fn question(stale: &[Stale], verb: &str) -> (String, String) {
    let n = stale.len();
    let count = |w: Why| stale.iter().filter(|s| s.why == w).count();
    let (edits, looks, unknown) = (count(Why::Edit), count(Why::Look), count(Why::Unknown));
    let title = if n == 1 { "1 chapter changed since it was rendered".to_string() } else { format!("{n} chapters changed since they were rendered") };
    let all = |k: usize| k == n;
    let mut why = vec![];
    if edits > 0 {
        why.push(if all(edits) {
            if n == 1 { "It has Video editor changes that aren't rendered yet.".to_string() } else { "They have Video editor changes that aren't rendered yet.".to_string() }
        } else {
            format!("{edits} {} Video editor changes that aren't rendered yet.", if edits == 1 { "has" } else { "have" })
        });
    }
    if looks > 0 {
        why.push(if all(looks) {
            format!("The Look, captions or music changed since {} rendered.", if n == 1 { "it was" } else { "they were" })
        } else {
            format!("The Look, captions or music changed since {looks} of them {} rendered.", if looks == 1 { "was" } else { "were" })
        });
    }
    if unknown > 0 {
        why.push(if all(unknown) {
            format!("{} made before Studio kept track of the Look, so {} may not match it.", if n == 1 { "It was" } else { "They were" }, if n == 1 { "it" } else { "they" })
        } else {
            format!("{unknown} {} made before Studio kept track of the Look.", if unknown == 1 { "was" } else { "were" })
        });
    }
    let them = if n == 1 { "it" } else { "them" };
    let body = format!("{} Render {them} first to see exactly what the editor shows before {verb}, or use the current videos.", why.join(" "));
    (title, body)
}

// ------------------------------------------------------------------------------------ the guard

/// What happens once the chosen renders are done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum After {
    /// Carry on at once (an export, which runs in the background anyway).
    Auto,
    /// Offer a button with this label (playing: a player that opens minutes later, out of the
    /// blue, would surprise).
    Offer(&'static str),
}

type Then = Rc<RefCell<Option<Box<dyn FnOnce(&mut Window, &mut App)>>>>;

fn run(then: &Then, window: &mut Window, cx: &mut App) {
    if let Some(f) = then.borrow_mut().take() {
        f(window, cx);
    }
}

/// Before playing or exporting the videos of `chapters`: works out which of them changed since
/// they were rendered (in the background) and, when any did, asks once whether to render those
/// first. Then runs `then` (after the renders when they were chosen, as `after` says).
/// `verb` finishes the question: "playing", "exporting".
pub fn guard(chapters: Vec<ChapterItem>, verb: &'static str, after: After, then: impl FnOnce(&mut Window, &mut App) + 'static, window: &mut Window, cx: &mut App) {
    let then: Then = Rc::new(RefCell::new(Some(Box::new(then))));
    let work = mv_ui::bg::blocking(move || stale_of(&chapters));
    window
        .spawn(cx, async move |cx| {
            let stale = work.await.unwrap_or_default();
            let _ = cx.update(|window, cx| match decide(stale, &kept()) {
                Decision::Go => run(&then, window, cx),
                Decision::Ask(stale) => ask(stale, verb, after, then, window, cx),
            });
        })
        .detach();
}

fn ask(stale: Vec<Stale>, verb: &'static str, after: After, then: Then, window: &mut Window, cx: &mut App) {
    let (title, body) = question(&stale, verb);
    let (keep_list, render_list) = (stale.clone(), stale);
    let (t1, t2) = (then.clone(), then);
    confirm_dialog(title, body, "Render them first", false)
        .icon(ph::FILM_STRIP)
        .cancel_label("Use the current videos")
        .on_cancel(move |window, cx| {
            keep(&keep_list);
            run(&t1, window, cx);
        })
        .on_confirm(move |window, cx| render_then(render_list.clone(), after, t2.clone(), window, cx))
        .open(window, cx);
}

/// Queues the Video step of the stale chapters (each with its edit, in the format it was made in),
/// waits until none is in the queue any more, then carries on as `after` says.
fn render_then(stale: Vec<Stale>, after: After, then: Then, window: &mut Window, cx: &mut App) {
    let items: Vec<ChapterItem> = stale.iter().map(|s| s.item.clone()).collect();
    let ids: Vec<String> = items.iter().map(|c| c.id.clone()).collect();
    let n = items.len();
    window
        .spawn(cx, async move |cx| {
            // The queue needs Studio's context; the manga section can get here before Studio opened.
            if !crate::pipeline::has_context() {
                let env = match crate::setup::latest() {
                    Some(c) => Some(c.env),
                    None => mv_ui::bg::run(crate::setup::check()).await.ok(),
                };
                let _ = cx.update(|_, cx| crate::pipeline::set_context(crate::pipeline::context_from_app(env, cx)));
            }
            let queued = cx.update(|_, cx| match crate::vedit_tools::render(&items) {
                Ok(_) => {
                    let what = if n == 1 { "1 chapter".to_string() } else { format!("{n} chapters") };
                    let next = match after {
                        After::Auto => "then carrying on",
                        After::Offer(_) => "you can play it when they're done",
                    };
                    toast::show(cx, Toast::info(format!("Rendering {what} first ({next})")).icon(ph::FILM_STRIP));
                    true
                }
                Err(e) => {
                    toast::show(cx, Toast::error(format!("{e}: using the current videos")));
                    false
                }
            });
            if matches!(queued, Ok(true)) {
                loop {
                    cx.background_executor().timer(Duration::from_millis(1000)).await;
                    let q = crate::pipeline::snapshot();
                    if !ids.iter().any(|id| q.for_chapter(id).is_some_and(|i| i.state.active())) {
                        break;
                    }
                }
            }
            // Chapters whose render failed or was cancelled keep their current video.
            let check = items.clone();
            let still = mv_ui::bg::blocking(move || stale_of(&check).len()).await.unwrap_or(0);
            let _ = cx.update(|window, cx| {
                if still > 0 {
                    toast::show(cx, Toast::warning(format!("{still} chapter{} could not be rendered: {} current video{} {} used", if still == 1 { "" } else { "s" }, if still == 1 { "its" } else { "their" }, if still == 1 { "" } else { "s" }, if still == 1 { "is" } else { "are" })));
                }
                match after {
                    After::Auto => run(&then, window, cx),
                    After::Offer(label) => {
                        let t = then.clone();
                        let done = if n == 1 { "Rendered 1 chapter".to_string() } else { format!("Rendered {n} chapters") };
                        toast::show(cx, Toast::success(done).icon(ph::CHECK_CIRCLE).sticky().action(label, move |window, cx| run(&t, window, cx)));
                    }
                }
            });
        })
        .detach();
}

/// [`guard`] for chapter videos handed in as files (`video_final.mp4` paths: the manga section's
/// Watch video and Play all, which know files, not Studio chapters). Files that are not a Studio
/// chapter's video are played as they are. Registered with `ui_manga::recap::set_guard`.
pub fn guard_files(videos: Vec<PathBuf>, then: Box<dyn FnOnce(&mut Window, &mut App)>, window: &mut Window, cx: &mut App) {
    let then: Then = Rc::new(RefCell::new(Some(then)));
    let work = mv_ui::bg::blocking(move || super::chapters_with_videos(&videos).unwrap_or_default());
    window
        .spawn(cx, async move |cx| {
            let chapters = work.await.unwrap_or_default();
            let _ = cx.update(|window, cx| {
                let t = then.clone();
                guard(chapters, "playing", After::Offer("Play"), move |w, cx| run(&t, w, cx), window, cx);
            });
        })
        .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    // The glob above brings GPUI's own `test` attribute in: the standard one wins explicitly.
    use ::core::prelude::v1::test;
    use crate::vedit::{Clip, LookEdit};
    use std::path::PathBuf;

    fn item(id: &str) -> ChapterItem {
        ChapterItem {
            id: id.into(),
            manga_id: "m".into(),
            manga_title: "M".into(),
            number: Some(1.),
            title: None,
            pages_dir: PathBuf::from("pages"),
            output_dir: PathBuf::from("out").join(id),
            page_count: 1,
            has_text: true,
            video: None,
            audio_files: 1,
        }
    }

    fn edited() -> ChapterEdit {
        let mut e = ChapterEdit::default();
        e.set_clips(&[], vec![Clip::of(1), Clip::of(0)]);
        e
    }

    #[test]
    fn unchanged_chapters_are_current() {
        // No edit: the stamp matches the Studio-wide key.
        assert_eq!(why_stale(&Facts { stamp: Some(7), key: 7, edit: None, edit_key: 1 }), None);
        // An edit rendered as it is now.
        let e = ChapterEdit { rendered: Some(5), ..edited() };
        assert_eq!(why_stale(&Facts { stamp: Some(9), key: 9, edit: Some(e), edit_key: 5 }), None);
    }

    #[test]
    fn a_changed_edit_is_stale() {
        // Edited after its render.
        let e = ChapterEdit { rendered: Some(5), ..edited() };
        assert_eq!(why_stale(&Facts { stamp: Some(1), key: 2, edit: Some(e), edit_key: 6 }), Some(Why::Edit));
        // Edited and never rendered (even on a video from before stamps).
        assert_eq!(why_stale(&Facts { stamp: None, key: 2, edit: Some(edited()), edit_key: 6 }), Some(Why::Edit));
        // An edit reset after a render still needs one.
        let reset = ChapterEdit { rendered: Some(5), ..ChapterEdit::default() };
        assert_eq!(why_stale(&Facts { stamp: Some(1), key: 2, edit: Some(reset), edit_key: 6 }), Some(Why::Edit));
    }

    #[test]
    fn a_changed_look_is_stale() {
        // No edit: the Studio Look (or captions, or music) changed.
        assert_eq!(why_stale(&Facts { stamp: Some(1), key: 2, edit: None, edit_key: 0 }), Some(Why::Look));
        // An edit that is rendered as it is: the change is the Look's.
        let e = ChapterEdit { rendered: Some(5), look: Some(LookEdit { preset: "recap".into(), effects: Default::default() }), ..edited() };
        assert_eq!(why_stale(&Facts { stamp: Some(1), key: 2, edit: Some(e), edit_key: 5 }), Some(Why::Look));
        // An empty edit entry (never edited, never rendered) is no edit.
        assert_eq!(why_stale(&Facts { stamp: Some(1), key: 2, edit: Some(ChapterEdit::default()), edit_key: 5 }), Some(Why::Look));
        // A video from before stamps, never edited: unknown.
        assert_eq!(why_stale(&Facts { stamp: None, key: 2, edit: None, edit_key: 0 }), Some(Why::Unknown));
    }

    fn stale(id: &str, why: Why, key: u64) -> Stale {
        Stale { item: item(id), why, key }
    }

    #[test]
    fn play_and_export_ask_only_about_what_changed() {
        let none = HashSet::new();
        assert_eq!(decide(vec![], &none), Decision::Go);
        let list = vec![stale("a", Why::Edit, 1), stale("b", Why::Look, 2)];
        assert_eq!(decide(list.clone(), &none), Decision::Ask(list.clone()));
        // "Use the current videos" for a: not asked again while a stays as it is.
        let kept: HashSet<(String, u64)> = [("a".to_string(), 1)].into();
        assert_eq!(decide(list.clone(), &kept), Decision::Ask(vec![list[1].clone()]));
        // a changed again (a new key): asked again.
        let again = vec![stale("a", Why::Edit, 3)];
        assert_eq!(decide(again.clone(), &kept), Decision::Ask(again));
        let both: HashSet<(String, u64)> = [("a".to_string(), 1), ("b".to_string(), 2)].into();
        assert_eq!(decide(list, &both), Decision::Go);
    }

    #[test]
    fn the_question_says_what_changed() {
        let (t, b) = question(&[stale("a", Why::Look, 1)], "playing");
        assert_eq!(t, "1 chapter changed since it was rendered");
        assert!(b.starts_with("The Look, captions or music changed since it was rendered."), "{b}");
        assert!(b.contains("Render it first") && b.ends_with("before playing, or use the current videos."), "{b}");
        let (t, b) = question(&[stale("a", Why::Look, 1), stale("b", Why::Look, 2)], "exporting");
        assert_eq!(t, "2 chapters changed since they were rendered");
        assert!(b.starts_with("The Look, captions or music changed since they were rendered.") && b.contains("Render them first"), "{b}");
        let (t, b) = question(&[stale("a", Why::Edit, 1), stale("b", Why::Look, 2), stale("c", Why::Unknown, 3)], "exporting");
        assert_eq!(t, "3 chapters changed since they were rendered");
        assert!(b.contains("1 has Video editor changes") && b.contains("since 1 of them was rendered") && b.contains("1 was made before"), "{b}");
        let (_, b) = question(&[stale("a", Why::Edit, 1), stale("b", Why::Edit, 2)], "playing");
        assert!(b.starts_with("They have Video editor changes"), "{b}");
    }

    #[test]
    fn only_chapters_with_a_video_are_checked() {
        let t = tempfile::tempdir().unwrap();
        let mut a = item("a");
        a.output_dir = t.path().join("m").join("a");
        let mut b = item("b");
        b.output_dir = t.path().join("m").join("b");
        std::fs::create_dir_all(&a.output_dir).unwrap();
        std::fs::create_dir_all(&b.output_dir).unwrap();
        std::fs::write(a.output_dir.join(super::super::VIDEO_FILE), "mp4").unwrap();
        // b has no video: nothing to render. a has one made before stamps: unknown.
        let s = stale_of(&[a.clone(), b]);
        assert_eq!(s.iter().map(|s| (s.item.id.as_str(), s.why)).collect::<Vec<_>>(), vec![("a", Why::Unknown)]);
        // Stamped with what a render makes now: current.
        std::fs::write(a.output_dir.join(vm::STAMP_FILE), serde_json::json!({ "key": s[0].key }).to_string()).unwrap();
        assert!(stale_of(&[a]).is_empty());
    }
}
