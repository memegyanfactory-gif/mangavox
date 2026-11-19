//! The competitor tracker: runs `mv_youtube::competitors::run` once an hour while the app is open
//! (public pages only, no API quota), so the data on every recap channel in the niche keeps
//! growing: new channels, every upload's exact time, views hour by hour, and each title,
//! description and thumbnail change. The Competitors section and the chat's competitors_* tools
//! read what it stores.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::*;
use mv_ui::bg;
use mv_youtube::competitors::{self, Budget, CompChannel, CompStore, Overview, RunSummary};

use crate::hub::now;

/// One research pass an hour.
pub const EVERY: i64 = 3600;
/// How often the timer looks whether a pass is due.
const CHECK: Duration = Duration::from_secs(5 * 60);

pub struct Tracker {
    pub store: CompStore,
    pub channels: Vec<CompChannel>,
    pub runs: Vec<RunSummary>,
    pub running: bool,
    pub step: Arc<Mutex<String>>,
    pub error: Option<String>,
    /// Bumped after every pass, so views know to re-read a channel's videos.
    pub version: u64,
    /// The whole niche at a glance, with the version it was worked out at (see [`Tracker::overview`]).
    overview: Option<(u64, Arc<Overview>)>,
    overview_busy: bool,
}

struct TrackerGlobal(Entity<Tracker>);
impl Global for TrackerGlobal {}

pub fn tracker(cx: &mut App) -> Entity<Tracker> {
    if let Some(g) = cx.try_global::<TrackerGlobal>() {
        return g.0.clone();
    }
    let store = CompStore::default();
    let e = cx.new(|_| Tracker { channels: store.channels(), runs: store.runs(), store, running: false, step: Arc::default(), error: None, version: 0, overview: None, overview_busy: false });
    cx.set_global(TrackerGlobal(e.clone()));
    e
}

/// Starts the hourly research (once, at app start); the first pass runs when the last one is
/// more than an hour old.
pub fn start(cx: &mut App) {
    let t = tracker(cx);
    cx.spawn(async move |cx| {
        // Let the app settle first.
        cx.background_executor().timer(Duration::from_secs(60)).await;
        loop {
            t.update(cx, |t, cx| if t.due() { t.run_now(cx) });
            cx.background_executor().timer(CHECK).await;
        }
    })
    .detach();
}

impl Tracker {
    pub fn last_run(&self) -> i64 {
        self.runs.last().map(|r| r.at).unwrap_or(0)
    }

    pub fn next_run(&self) -> i64 {
        self.last_run() + EVERY
    }

    fn due(&self) -> bool {
        !self.running && now() >= self.next_run()
    }

    /// One research pass now.
    pub fn run_now(&mut self, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        self.running = true;
        self.error = None;
        *self.step.lock().unwrap_or_else(|e| e.into_inner()) = "Searching for recap channels".into();
        cx.notify();
        let (store, step) = (CompStore::new(self.store.root().to_path_buf()), self.step.clone());
        cx.spawn(async move |this, cx| {
            let r = bg::run(async move { competitors::run(&store, Budget::default(), move |m| *step.lock().unwrap_or_else(|e| e.into_inner()) = m).await }).await;
            let _ = this.update(cx, |t, cx| {
                t.running = false;
                match r {
                    Ok(Ok(s)) => t.error = s.error.clone(),
                    Ok(Err(e)) => t.error = Some(format!("{e:#}")),
                    Err(e) => t.error = Some(format!("{e:#}")),
                }
                t.channels = t.store.channels();
                t.runs = t.store.runs();
                t.version += 1;
                cx.notify();
            });
        })
        .detach();
        // Repaint the step line while it runs.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(700)).await;
            let alive = this.update(cx, |t, cx| {
                cx.notify();
                t.running
            });
            if !matches!(alive, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// The niche overview of everything tracked. Reading every channel's videos takes a moment, so
    /// it is worked out in the background: the last one is returned meanwhile (`None` the first
    /// time), and the views are told when the new one is ready.
    pub fn overview(&mut self, cx: &mut Context<Self>) -> Option<Arc<Overview>> {
        let fresh = self.overview.as_ref().is_some_and(|(v, _)| *v == self.version);
        if !fresh && !self.overview_busy {
            self.overview_busy = true;
            let (store, version) = (CompStore::new(self.store.root().to_path_buf()), self.version);
            cx.spawn(async move |this, cx| {
                let o = bg::run(async move { competitors::overview(&store, now(), crate::hub::utc_offset()) }).await;
                let _ = this.update(cx, |t, cx| {
                    t.overview_busy = false;
                    if let Ok(o) = o {
                        t.overview = Some((version, Arc::new(o)));
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        self.overview.as_ref().map(|(_, o)| o.clone())
    }

    /// Takes a channel that is not a recap channel off the list and out of the overview.
    pub fn set_hidden(&mut self, id: &str, hidden: bool, cx: &mut Context<Self>) {
        let _ = self.store.set_hidden(id, hidden);
        self.channels = self.store.channels();
        self.version += 1;
        cx.notify();
    }

    pub fn set_pinned(&mut self, id: &str, pinned: bool, cx: &mut Context<Self>) {
        let _ = self.store.set_pinned(id, pinned);
        self.channels = self.store.channels();
        cx.notify();
    }
}
