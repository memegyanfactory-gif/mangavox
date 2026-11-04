//! Studio's Generate runs as activity cards in the shell's corner, next to the download cards:
//! one card per manga with chapters in the Generate queue (`ui_studio::pipeline`), and one per
//! full-video export or join running (`pipeline::exports`). The queue lives outside GPUI, so this
//! polls its version like the Studio page does.
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui_kit::*;
use mv_ui::activity::{self, Card, Lead};
use mv_ui::{btn, icon_button, ph, ButtonKind, Route};
use ui_studio::pipeline::{self, ExportActivity, ItemState, QueueItem};

const POLL: Duration = Duration::from_millis(500);
/// A run that made every chapter closes its card after this long.
const AUTO_HIDE: Duration = Duration::from_secs(6);

/// One queue chapter, reduced to what a card shows.
#[derive(Clone, Debug, PartialEq)]
struct Item {
    id: u64,
    manga_id: String,
    manga_title: String,
    /// "Ch. 12".
    label: String,
    state: ItemState,
    /// 0..=1 over the steps it makes, the running step's own progress included.
    progress: f32,
    /// What its running step does ("rendering the video").
    doing: Option<&'static str>,
}

fn item(it: &QueueItem) -> Item {
    let running = it.state == ItemState::Running;
    let steps = it.until.min(it.steps.len() - 1) + 1;
    // The audio and video steps report their own progress through their job.
    let inner = it.job.filter(|_| running).and_then(ui_studio::service::job).map_or(0., |j| j.progress() as f32);
    Item {
        id: it.id,
        manga_id: it.chapter.manga_id.clone(),
        manga_title: it.chapter.manga_title.clone(),
        label: pipeline::short_label(it.chapter.number, it.chapter.title.as_deref()),
        state: it.state,
        progress: (it.progress() + inner / steps as f32).min(1.),
        doing: it.current_step().filter(|_| running).map(|s| doing(s.label())),
    }
}

/// A step's label as the line under the manga's name.
fn doing(step: &str) -> &'static str {
    match step {
        "Read" => "reading the pages",
        "Transcript" => "writing the transcript",
        "Story" => "writing the recap",
        "Audio" => "narrating",
        "Music" => "composing the music",
        "Video" => "rendering the video",
        _ => "working",
    }
}

/// One manga's Generate run, as this session saw it start.
#[derive(Clone, Debug, PartialEq)]
struct Run {
    /// The run's chapters are the manga's queue items from this id on (older ones are earlier runs).
    from: u64,
    order: u64,
    /// When its last chapter went through.
    ended: Option<Instant>,
    /// Closed from its card; it shows again when a chapter newer than this joins.
    hidden_upto: Option<u64>,
}

/// What a run's card counts.
#[derive(Clone, Debug, Default, PartialEq)]
struct Tally {
    title: String,
    total: usize,
    done: usize,
    progress: f32,
    /// The first chapter running and what it does.
    running: Option<(String, &'static str)>,
    /// Chapters still to make, the paused ones among them, and the failed ones.
    active: Vec<u64>,
    paused: Vec<u64>,
    failed: Vec<u64>,
    newest: u64,
}

fn tally(items: &[Item], manga_id: &str, from: u64) -> Tally {
    let mut t = Tally::default();
    let mut sum = 0.;
    for i in items.iter().filter(|i| i.manga_id == manga_id && i.id >= from) {
        t.title.clone_from(&i.manga_title);
        t.total += 1;
        t.newest = t.newest.max(i.id);
        match i.state {
            ItemState::Done => {
                t.done += 1;
                sum += 1.;
            }
            ItemState::Failed => t.failed.push(i.id),
            s => {
                sum += i.progress;
                t.active.push(i.id);
                if s == ItemState::Paused {
                    t.paused.push(i.id);
                }
                if s == ItemState::Running && t.running.is_none() {
                    t.running = Some((i.label.clone(), i.doing.unwrap_or("starting")));
                }
            }
        }
    }
    t.progress = if t.total == 0 { 0. } else { (sum / t.total as f32).min(1.) };
    t
}

/// Follows the queue: a manga with chapters to make starts a run (or carries on its run); one
/// whose chapters all went through ends it. An ended run leaves after [`AUTO_HIDE`] unless a
/// chapter failed, and at once when its chapters left the queue (Studio cleared them).
fn track(runs: &mut HashMap<String, Run>, items: &[Item], now: Instant) {
    let mut first: HashMap<&str, u64> = HashMap::new();
    for i in items.iter().filter(|i| i.state.active()) {
        let f = first.entry(i.manga_id.as_str()).or_insert(i.id);
        *f = (*f).min(i.id);
    }
    for (&m, &from) in &first {
        if !runs.get(m).is_some_and(|r| r.ended.is_none()) {
            runs.insert(m.to_string(), Run { from, order: activity::next_order(), ended: None, hidden_upto: None });
        }
    }
    runs.retain(|m, r| {
        if first.contains_key(m.as_str()) {
            return true;
        }
        let t = tally(items, m, r.from);
        if t.total == 0 {
            return false;
        }
        let ended = *r.ended.get_or_insert(now);
        !t.failed.is_empty() || now < ended + AUTO_HIDE
    });
}

/// The Generate cards' state; the shell owns one and observes it.
pub struct StudioRuns {
    items: Vec<Item>,
    /// The whole queue is on hold, and whether that is because MangaVox closed.
    held: bool,
    interrupted: bool,
    runs: HashMap<String, Run>,
    /// Pipeline and job versions last read.
    seen: Option<(u64, u64)>,
    /// A queue read is running in the background.
    reading: bool,
    /// Exports running, each card's place in the stack, and the ones closed from their card.
    exports: Vec<ExportActivity>,
    export_orders: HashMap<u64, u64>,
    hidden_exports: HashSet<u64>,
    _poll: Task<()>,
}

impl StudioRuns {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL).await;
            if this.update(cx, |s, cx| s.poll(cx)).is_err() {
                break;
            }
        });
        StudioRuns { items: vec![], held: false, interrupted: false, runs: HashMap::new(), seen: None, reading: false, exports: vec![], export_orders: HashMap::new(), hidden_exports: HashSet::new(), _poll: poll }
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        // Exports report through a shared state (a lock held only to copy it): read every time.
        let exports = pipeline::exports();
        if exports != self.exports {
            let live: HashSet<u64> = exports.iter().map(|e| e.id).collect();
            self.export_orders.retain(|id, _| live.contains(id));
            self.hidden_exports.retain(|id| live.contains(id));
            for e in &exports {
                self.export_orders.entry(e.id).or_insert_with(activity::next_order);
            }
            self.exports = exports;
            cx.notify();
        }
        let seen = Some((pipeline::version(), ui_studio::service::version()));
        if seen != self.seen {
            // Read off the UI thread: the queue's lock is also held while the pipeline saves
            // the queue to disk, and every player frame would wait behind a slow write.
            if !self.reading {
                self.reading = true;
                cx.spawn(async move |this, cx| {
                    let read = mv_ui::bg::blocking(|| {
                        let q = pipeline::snapshot();
                        (q.items.iter().map(item).collect::<Vec<Item>>(), q.paused, q.interrupted)
                    })
                    .await;
                    let _ = this.update(cx, |s, cx| {
                        s.reading = false;
                        if let Ok((items, held, interrupted)) = read {
                            s.seen = seen;
                            let changed = items != s.items || (held, interrupted) != (s.held, s.interrupted);
                            (s.items, s.held, s.interrupted) = (items, held, interrupted);
                            s.retrack(changed, cx);
                        }
                    });
                })
                .detach();
            }
            return;
        }
        if self.runs.is_empty() {
            // Idle and unchanged.
            return;
        }
        self.retrack(false, cx);
    }

    fn retrack(&mut self, changed: bool, cx: &mut Context<Self>) {
        let before = self.runs.clone();
        track(&mut self.runs, &self.items, Instant::now());
        if changed || before != self.runs {
            cx.notify();
        }
    }

    /// Reads the queue now (after a button on a card changed it).
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.seen = None;
        self.poll(cx);
    }

    fn dismiss(&mut self, manga_id: &str, cx: &mut Context<Self>) {
        let ended = self.runs.get(manga_id).is_some_and(|r| r.ended.is_some());
        if ended {
            self.runs.remove(manga_id);
        } else if let Some(r) = self.runs.get_mut(manga_id) {
            r.hidden_upto = Some(tally(&self.items, manga_id, r.from).newest);
        }
        cx.notify();
    }

    /// One card per Generate run and per export to show.
    pub fn cards(&self, this: &Entity<Self>) -> Vec<Card> {
        let runs = self.runs.iter().filter_map(|(m, r)| {
            let t = tally(&self.items, m, r.from);
            let hidden = r.hidden_upto.is_some_and(|h| t.newest <= h);
            (t.total > 0 && !hidden).then(|| self.card(this, m, r, t))
        });
        let exports = self.exports.iter().filter(|e| !self.hidden_exports.contains(&e.id)).map(|e| self.export_card(this, e));
        runs.chain(exports).collect()
    }

    /// An export's card: its manga, the step and its progress; Cancel, and Open Studio (where its
    /// result is shown when it ends). Closing the card leaves the export running.
    fn export_card(&self, this: &Entity<Self>, e: &ExportActivity) -> Card {
        let id = e.id;
        let order = self.export_orders.get(&id).copied().unwrap_or(0);
        let close = {
            let this = this.clone();
            move |_: &mut Window, cx: &mut App| {
                this.update(cx, |s, cx| {
                    s.hidden_exports.insert(id);
                    cx.notify();
                })
            }
        };
        let title = if e.manga_title.is_empty() { "Full video".to_string() } else { format!("{} · full video", e.manga_title) };
        let card = Card::new(format!("export-{id}"), order, Lead::Progress(e.progress), title, e.label.clone());
        let card = if e.waiting { card.count("Queued") } else { card.progress(e.progress, true).count(format!("{}%", (e.progress * 100.).round() as u32)) };
        card
            .action(btn("cancel", "Cancel", ButtonKind::Ghost).small().icon(ph::X).tooltip("Stop this export").on_click(move |_, _, _| pipeline::cancel_export(id)))
            .action(btn("open", "Open Studio", ButtonKind::Ghost).small().trailing_icon(ph::ARROW_RIGHT).on_click(|_, w, cx| mv_ui::navigate(Route::Studio, w, cx)))
            .on_close(close)
    }

    fn card(&self, this: &Entity<Self>, manga_id: &str, r: &Run, t: Tally) -> Card {
        let key = format!("gen-{}", r.order);
        // Runs `f` on the queue, then shows the change at once.
        let act = |f: Box<dyn Fn()>| {
            let this = this.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                f();
                this.update(cx, |s, cx| s.refresh(cx));
            }
        };
        let close = {
            let (this, m) = (this.clone(), manga_id.to_string());
            move |_: &mut Window, cx: &mut App| this.update(cx, |s, cx| s.dismiss(&m, cx))
        };
        let open = btn("open", "Open Studio", ButtonKind::Ghost).small().trailing_icon(ph::ARROW_RIGHT).on_click(|_, w, cx| mv_ui::navigate(Route::Studio, w, cx));

        if r.ended.is_some() {
            if !t.failed.is_empty() {
                let ids = t.failed.clone();
                return Card::new(key, r.order, Lead::Failed, t.title, format!("{} of {} chapters failed", t.failed.len(), t.total))
                    .action(btn("retry", "Retry", ButtonKind::Ghost).small().icon(ph::ARROW_CLOCKWISE).on_click(act(Box::new(move || ids.iter().for_each(|&id| pipeline::resume(id))))))
                    .action(open)
                    .on_close(close);
            }
            let n = t.done;
            return Card::new(key, r.order, Lead::Done, t.title, format!("{n} chapter{} made", if n == 1 { "" } else { "s" })).action(open).on_close(close);
        }

        // Held as a whole, or every chapter still to make paused from its card or in Studio.
        let paused = t.running.is_none() && (self.held || t.paused.len() == t.active.len());
        let line = match &t.running {
            Some((label, doing)) => format!("{label} · {doing}"),
            None if paused && self.interrupted => "Paused when MangaVox closed".into(),
            None if paused => "Paused".into(),
            None => "Waiting for the chapters before it".into(),
        };
        let at = if t.running.is_some() { (t.done + 1).min(t.total) } else { t.done };
        let toggle: Box<dyn Fn()> = if !paused {
            let ids = t.active.clone();
            Box::new(move || ids.iter().for_each(|&id| pipeline::pause(id)))
        } else if !t.paused.is_empty() {
            let ids = t.paused.clone();
            Box::new(move || ids.iter().for_each(|&id| pipeline::resume(id)))
        } else {
            Box::new(pipeline::resume_all)
        };
        let ids = t.active.clone();
        // Auto export on: the run's card says the full recap follows, and where it goes.
        let line = match ui_studio::auto_export_folder(manga_id) {
            Some(dir) => format!("{line} · then auto export to {}", dir.file_name().map_or_else(|| dir.display().to_string(), |n| n.to_string_lossy().into_owned())),
            None => line,
        };
        Card::new(key, r.order, Lead::Progress(t.progress), t.title, line)
            .progress(t.progress, t.running.is_some())
            .count(format!("{at} of {} chapters", t.total))
            .action(icon_button("pause", if paused { ph::PLAY } else { ph::PAUSE }, if paused { "Resume" } else { "Pause" }).small().on_click(act(toggle)))
            .action(
                btn("cancel", "Cancel", ButtonKind::Ghost)
                    .small()
                    .icon(ph::X)
                    .tooltip("Take this manga's chapters out of Generate")
                    .on_click(act(Box::new(move || ids.iter().for_each(|&id| pipeline::cancel(id))))),
            )
            .on_close(close)
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in gpui's `test` macro, which shadows the built-in one.
    use super::{tally, track, Item, AUTO_HIDE};
    use std::collections::HashMap;
    use std::time::{Duration, Instant};
    use ui_studio::pipeline::ItemState;

    fn it(id: u64, manga: &str, state: ItemState) -> Item {
        Item { id, manga_id: manga.into(), manga_title: manga.to_uppercase(), label: format!("Ch. {id}"), state, progress: 0.5, doing: Some("narrating") }
    }

    #[test]
    fn one_run_per_manga() {
        let items = [it(1, "a", ItemState::Running), it(2, "a", ItemState::Queued), it(3, "b", ItemState::Queued)];
        let mut runs = HashMap::new();
        track(&mut runs, &items, Instant::now());
        assert_eq!(runs.len(), 2);
        let a = tally(&items, "a", runs["a"].from);
        assert_eq!((a.total, a.done, a.active.len()), (2, 0, 2));
        assert_eq!(a.running, Some(("Ch. 1".into(), "narrating")));
        assert_eq!(a.title, "A");
        assert!(tally(&items, "b", runs["b"].from).running.is_none());
    }

    #[test]
    fn earlier_runs_do_not_count() {
        // Chapter 1 was made in an earlier run; the new run starts at chapter 2.
        let items = [it(1, "a", ItemState::Done), it(2, "a", ItemState::Queued)];
        let mut runs = HashMap::new();
        track(&mut runs, &items, Instant::now());
        let t = tally(&items, "a", runs["a"].from);
        assert_eq!((t.total, t.done), (1, 0));
        assert!((t.progress - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_clean_run_closes_after_a_while() {
        let now = Instant::now();
        let mut runs = HashMap::new();
        track(&mut runs, &[it(1, "a", ItemState::Running)], now);
        let done = [it(1, "a", ItemState::Done)];
        track(&mut runs, &done, now + Duration::from_secs(1));
        assert!(runs["a"].ended.is_some());
        track(&mut runs, &done, now + Duration::from_secs(1) + AUTO_HIDE);
        assert!(runs.is_empty());
        // Done chapters left in the queue do not start a new run.
        track(&mut runs, &done, now + Duration::from_secs(60));
        assert!(runs.is_empty());
    }

    #[test]
    fn a_failed_run_stays() {
        let now = Instant::now();
        let mut runs = HashMap::new();
        track(&mut runs, &[it(1, "a", ItemState::Running), it(2, "a", ItemState::Queued)], now);
        let after = [it(1, "a", ItemState::Done), it(2, "a", ItemState::Failed)];
        track(&mut runs, &after, now + Duration::from_secs(600));
        track(&mut runs, &after, now + Duration::from_secs(1200));
        assert_eq!(tally(&after, "a", runs["a"].from).failed, vec![2]);
        // Retried: a new run of just the retried chapter.
        track(&mut runs, &[it(1, "a", ItemState::Done), it(2, "a", ItemState::Queued)], now + Duration::from_secs(1300));
        assert!(runs["a"].ended.is_none());
        assert_eq!(tally(&after, "a", runs["a"].from).total, 1);
    }

    #[test]
    fn cleared_chapters_end_the_run_at_once() {
        let now = Instant::now();
        let mut runs = HashMap::new();
        track(&mut runs, &[it(1, "a", ItemState::Queued)], now);
        track(&mut runs, &[], now);
        assert!(runs.is_empty());
    }

    #[test]
    fn a_new_run_after_one_ended_gets_a_new_card() {
        let now = Instant::now();
        let mut runs = HashMap::new();
        track(&mut runs, &[it(1, "a", ItemState::Running)], now);
        let first = runs["a"].order;
        track(&mut runs, &[it(1, "a", ItemState::Done)], now);
        track(&mut runs, &[it(1, "a", ItemState::Done), it(2, "a", ItemState::Queued)], now + Duration::from_secs(1));
        assert!(runs["a"].order > first);
        assert_eq!(runs["a"].from, 2);
        assert!(runs["a"].ended.is_none());
    }
}
