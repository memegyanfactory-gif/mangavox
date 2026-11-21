//! Anime Engine (Beta): the tab that turns a downloaded manga into an animatic, step by step
//! (Manga, Story, Bible, Shots, Frames, Animate, Sound, Cut). This crate only renders, edits
//! model structs through `mv_anime::store` and enqueues `mv_anime::queue` jobs; the engine
//! itself has no GPUI. The shell calls [`open`] for every route; this crate returns
//! `Some(view)` for `Route::AnimeEngine`.
// A build with some steps compiled out leaves parts of the shared kit (widgets, map, Generation
// panel, history, Magnific helpers) without a caller; the full build must use all of it.
#![cfg_attr(
    not(all(feature = "story", feature = "bible", feature = "shots", feature = "frames", feature = "sound", feature = "cut")),
    allow(dead_code)
)]
use gpui_kit::*;
use mv_ui::Route;

mod backends;
mod gen_panel;
mod history;
mod home;
mod map;
mod page;
mod queue_strip;
mod stepbar;
mod steps;
mod style;
mod widgets;

/// The one Anime Engine page of the session: leaving the tab and coming back finds it as it
/// was (the section is also parked by the shell). Running jobs live in `mv_anime::queue`.
struct Kept(Entity<page::EnginePage>);
impl Global for Kept {}

pub fn open(route: &Route, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    if !is_engine(route) {
        return None;
    }
    warm_queue(cx);
    if let Some(Kept(page)) = cx.try_global::<Kept>() {
        let page = page.clone();
        // Projects may have changed on disk meanwhile; the page takes the keyboard back.
        page.update(cx, |p, cx| {
            p.reload(cx);
            window.focus(&p.focus_handle, cx);
        });
        return Some(page.into());
    }
    let page = cx.new(|cx| page::EnginePage::new(window, cx));
    cx.set_global(Kept(page.clone()));
    Some(page.into())
}

/// `route` is the Anime Engine tab.
fn is_engine(route: &Route) -> bool {
    matches!(route, Route::AnimeEngine)
}

/// The queue's file has been read (in the background, see [`warm_queue`]).
static QUEUE_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The first time the tab's code runs, the job queue's file is read on a background thread, so
/// no draw ever waits for the data drive; every window is drawn again once it is in.
fn warm_queue(cx: &mut App) {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        cx.spawn(async move |cx| {
            if let Err(e) = mv_ui::bg::blocking(mv_anime::queue::warm).await {
                mv_core::logging::global().warn("anime", &format!("the job queue could not be read ahead: {e:#}"));
            }
            QUEUE_READY.store(true, std::sync::atomic::Ordering::Release);
            let _ = cx.update(|cx| cx.refresh_windows());
        })
        .detach();
    });
}

/// The engine's corner activity cards (one per project with work in the queue or failures to
/// look at), gathered by the shell. None until the queue's file has been read in the background.
pub fn activity_cards(cx: &mut App) -> Vec<mv_ui::activity::Card> {
    warm_queue(cx);
    if !QUEUE_READY.load(std::sync::atomic::Ordering::Acquire) {
        return Vec::new();
    }
    queue_strip::activity_cards(cx)
}

/// A background result landed on a screen: every window draws again now, not at the next
/// mouse move (a notify alone did not always bring a frame on Windows).
pub(crate) fn redraw(cx: &mut App) {
    cx.refresh_windows();
}

/// A screen's file changes run one at a time in the order they were asked for: each takes its
/// turn on the UI thread when the user acts, and waits for it in the background (a plain lock
/// lets a later click overtake an earlier one, so "Approve" then "undo" could end approved).
#[derive(Clone, Default)]
pub(crate) struct Turns(std::sync::Arc<(std::sync::Mutex<TurnState>, std::sync::Condvar)>);

#[derive(Default)]
pub(crate) struct TurnState {
    next: u64,
    serving: u64,
    /// Turns given up without running.
    skipped: std::collections::HashSet<u64>,
}

/// One change's place in line; the next change goes when it is dropped (run or not).
pub(crate) struct Turn {
    turns: Turns,
    n: u64,
}

impl Turns {
    /// The next place in line. Brief: call it on the UI thread when the user acts.
    pub(crate) fn take(&self) -> Turn {
        let mut s = self.0 .0.lock().unwrap_or_else(|e| e.into_inner());
        let n = s.next;
        s.next += 1;
        Turn { turns: self.clone(), n }
    }
}

impl Turn {
    /// Waits until every earlier change is done, then runs `f`. Blocking.
    pub(crate) fn run<R>(self, f: impl FnOnce() -> R) -> R {
        {
            let (m, cv) = &*self.turns.0;
            let mut s = m.lock().unwrap_or_else(|e| e.into_inner());
            while s.serving < self.n {
                s = cv.wait(s).unwrap_or_else(|e| e.into_inner());
            }
        }
        f()
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let (m, cv) = &*self.turns.0;
        let mut s = m.lock().unwrap_or_else(|e| e.into_inner());
        if s.serving == self.n {
            s.serving += 1;
            loop {
                let k = s.serving;
                if !s.skipped.remove(&k) {
                    break;
                }
                s.serving += 1;
            }
        } else {
            s.skipped.insert(self.n);
        }
        cv.notify_all();
    }
}

/// MangaVox is closing: the queue's journal (written in the background) gets its last state onto
/// disk first, waiting at most 2 seconds. Called where the app asks to quit (before the quit, off
/// the UI thread) and again where the app's event loop has ended.
pub fn flush_on_exit() {
    if !mv_anime::queue::flush_journal(std::time::Duration::from_secs(2)) {
        mv_core::logging::global().warn("anime", "the job queue's last change may not have reached the disk before MangaVox closed");
    }
}

static TAB_SHOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The shell says whether the tab's page is the one on screen (on every draw). While it is
/// not, the page and its panels read nothing; they catch up when it is shown again.
pub fn set_tab_shown(shown: bool) {
    TAB_SHOWN.store(shown, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn tab_shown() -> bool {
    TAB_SHOWN.load(std::sync::atomic::Ordering::Relaxed)
}

/// Studio mode was turned off: the tab is gone, so nothing of the engine may run where the user
/// cannot see or stop it. The whole queue is paused (every lane); with Studio mode back on, the
/// tab shows the paused queue with its Resume.
pub fn studio_mode_off() {
    use mv_anime::queue::{self, JobState};
    set_tab_shown(false);
    let q = queue::snapshot();
    if q.jobs.iter().any(|j| matches!(j.state, JobState::Queued | JobState::Running)) {
        queue::pause_all();
        // A running job stops too (it starts over on Resume): a picture's page routine is told
        // to stop before its next action.
        for j in q.jobs.iter().filter(|j| j.state == JobState::Running) {
            queue::pause(j.id);
        }
    }
}
