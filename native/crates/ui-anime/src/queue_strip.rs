//! The bottom queue strip (design 5.7, 9.2) and the engine's corner activity cards.
//!
//! Folded, the strip is one 40 px row: the counts, the running jobs with progress and time left,
//! "Pause all" / "Resume all", "Hide failed" and the caret. Opened, it lists every job with
//! Pause / Resume, Retry and Cancel, plus "Clear finished". After a restart that stopped work, a
//! banner asks to resume. Everything reads `mv_anime::queue` (its statics outlive the page).

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_anime::queue::{self, Job, JobState, Lane, Queue};
use mv_ui::activity::{Card, Lead};
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::{banner, btn, icon_button, icon_ph, ph, ph_icon, progress_bar, size, spinner, switch, tip, AlertKind, ButtonKind, Route, Tokens, Weight};

use crate::page::EnginePage;
use crate::style::{QUEUE_H, QUEUE_OPEN_H};
use crate::widgets::{fmt_duration, now_secs, unix_secs};

// ---------------------------------------------------------------- pure parts

/// Job counts for the header chip and the strip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub running: usize,
    pub waiting: usize,
    pub paused: usize,
    /// Failed and not hidden.
    pub failed: usize,
    pub done: usize,
}

impl Summary {
    /// Jobs still to do (running, waiting or paused).
    pub fn active(&self) -> usize {
        self.running + self.waiting + self.paused
    }
}

/// Counts of `q`, of one project's jobs when `project` is given.
pub fn summarize(q: &Queue, project: Option<&str>) -> Summary {
    let mut s = Summary::default();
    for j in q.jobs.iter().filter(|j| project.is_none_or(|p| j.project == p)) {
        match j.state {
            JobState::Running => s.running += 1,
            JobState::Queued => s.waiting += 1,
            JobState::Paused => s.paused += 1,
            JobState::Failed if !j.hidden => s.failed += 1,
            JobState::Failed => {}
            JobState::Done => s.done += 1,
        }
    }
    s
}

/// "2 running · 5 waiting · 1 failed"; "Paused · 5 waiting" while the queue is on hold;
/// "Nothing in the queue" when empty.
pub fn summary_text(s: &Summary, held: bool) -> String {
    let mut parts: Vec<String> = Vec::new();
    if held && s.active() > 0 {
        parts.push("Paused".into());
    }
    if s.running > 0 {
        parts.push(format!("{} running", s.running));
    }
    if s.waiting > 0 {
        parts.push(format!("{} waiting", s.waiting));
    }
    if s.paused > 0 {
        parts.push(format!("{} paused", s.paused));
    }
    if s.failed > 0 {
        parts.push(format!("{} failed", s.failed));
    }
    if parts.is_empty() {
        if s.done > 0 {
            return format!("{} done", s.done);
        }
        return "Nothing in the queue".into();
    }
    parts.join(" · ")
}

/// The strip's order: running, then waiting and paused (oldest first), then failed, then done
/// (newest first). Hidden failures are left out when `hide_failed`.
pub fn ordered(q: &Queue, hide_failed: bool) -> Vec<&Job> {
    let rank = |j: &Job| match j.state {
        JobState::Running => 0,
        JobState::Queued | JobState::Paused => 1,
        JobState::Failed => 2,
        JobState::Done => 3,
    };
    let mut v: Vec<&Job> = q.jobs.iter().filter(|j| !(j.state == JobState::Failed && (hide_failed || j.hidden))).collect();
    v.sort_by_key(|j| (rank(j), if j.state == JobState::Done { u64::MAX - j.id } else { j.id }));
    v
}

/// Time words of a job: "38 s · about 1 min left" while running, "about 45 s" while waiting.
pub fn time_text(j: &Job, mean: Option<f32>, now: i64) -> String {
    match j.state {
        JobState::Running => {
            let elapsed = j.started_at.map(|t| (now - unix_secs(t)).max(0) as u64).unwrap_or(0);
            match mean {
                Some(m) if m > elapsed as f32 + 2. => format!("{} · about {} left", fmt_duration(elapsed), fmt_duration((m - elapsed as f32).round() as u64)),
                Some(_) => format!("{} · almost done", fmt_duration(elapsed)),
                None => fmt_duration(elapsed),
            }
        }
        // A paused job takes no time until it is resumed.
        JobState::Paused => "paused".into(),
        JobState::Queued => match mean {
            Some(m) => format!("about {}", fmt_duration(m.round().max(1.) as u64)),
            None => "waiting".into(),
        },
        JobState::Failed => "failed".into(),
        JobState::Done => j.finished_at.map(|t| crate::widgets::ago(t, now)).unwrap_or_else(|| "done".into()),
    }
}

/// Mean finished durations per job kind (`settings.json`'s `eta`), for the time left.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Eta(BTreeMap<String, f32>);

/// The last estimates read and the queue version they were read at; `reading` while a re-read
/// runs in the background.
struct EtaCache {
    seen: u64,
    eta: Eta,
    reading: bool,
}

static ETA: std::sync::Mutex<Option<EtaCache>> = std::sync::Mutex::new(None);

impl Eta {
    /// Reads `settings.json`. Blocking: the screens use [`Eta::current`].
    pub fn load() -> Eta {
        Eta::from_table(&mv_anime::settings::load().eta)
    }

    /// The estimates as last read, without touching the disk on the UI thread: when a job has
    /// finished since (the queue changed), they are read again in the background and the next
    /// frame shows them.
    pub fn current() -> Eta {
        let v = queue::version();
        let Ok(mut guard) = ETA.lock() else { return Eta::default() };
        let cache = guard.get_or_insert_with(|| EtaCache { seen: u64::MAX, eta: Eta::default(), reading: false });
        if cache.seen != v && !cache.reading {
            cache.reading = true;
            mv_ui::bg::runtime().spawn_blocking(move || {
                let eta = Eta::load();
                if let Ok(mut guard) = ETA.lock() {
                    if let Some(c) = guard.as_mut() {
                        c.eta = eta;
                        c.seen = v;
                        c.reading = false;
                    }
                }
            });
        }
        cache.eta.clone()
    }

    pub fn from_table(t: &BTreeMap<String, Vec<f32>>) -> Eta {
        Eta(t.iter().filter_map(|(k, v)| mean(v).map(|m| (k.clone(), m))).collect())
    }

    /// Mean seconds of a `JobKind::name()`.
    pub fn mean(&self, kind: &str) -> Option<f32> {
        self.0.get(kind).copied()
    }
}

fn mean(v: &[f32]) -> Option<f32> {
    let v: Vec<f32> = v.iter().copied().filter(|x| x.is_finite() && *x > 0.).collect();
    (!v.is_empty()).then(|| v.iter().sum::<f32>() / v.len() as f32)
}

/// The strip's own queue button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    /// The restart notice above carries the one Resume; the strip shows only the state
    /// ("Paused · 1 waiting").
    None,
    Pause,
    Resume,
}

/// One resume action at a time: while the restart notice is on screen it has the button,
/// otherwise the strip has "Resume all" (held) or "Pause all".
pub fn toggle_of(q: &Queue) -> Toggle {
    if q.interrupted {
        Toggle::None
    } else if q.paused {
        Toggle::Resume
    } else {
        Toggle::Pause
    }
}

/// The Web lane is on hold (a sign-in, a human check or a paid label stopped it).
pub fn web_held(q: &Queue) -> bool {
    q.paused_lanes.contains(&Lane::Web)
}

// ---------------------------------------------------------------- the strip

/// The strip's own state (kept by the page).
#[derive(Default)]
pub struct Strip {
    pub expanded: bool,
    pub hide_failed: bool,
    scroll: ScrollHandle,
}

// Project titles for job rows and activity cards (filled by the page from the project list),
// the cards' stable order, and the queue snapshot of the last version seen.
thread_local! {
    static ORDERS: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
    static CACHE: RefCell<Option<(u64, Queue)>> = const { RefCell::new(None) };
}

/// Project titles by id: filled by the page, and at start-up (before the tab was opened) read
/// once from the projects on disk in the background.
static TITLES: std::sync::Mutex<Option<HashMap<String, String>>> = std::sync::Mutex::new(None);
static TITLES_READ: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn remember_title(project: &str, title: &str) {
    if let Ok(mut t) = TITLES.lock() {
        t.get_or_insert_with(HashMap::new).insert(project.to_string(), title.to_string());
    }
}

/// A project's title for cards and job rows; never its id. Unknown yet: the projects are read
/// in the background once, and a plain name stands in meanwhile.
fn title_of(project: &str) -> String {
    let known = TITLES.lock().ok().and_then(|t| t.as_ref().and_then(|m| m.get(project).cloned()));
    if let Some(t) = known.filter(|t| !t.trim().is_empty()) {
        return t;
    }
    if !TITLES_READ.swap(true, std::sync::atomic::Ordering::SeqCst) {
        mv_ui::bg::runtime().spawn_blocking(|| {
            for s in mv_anime::store::list_projects() {
                if !s.manga_title.trim().is_empty() {
                    remember_title(&s.id, &s.manga_title);
                }
            }
        });
    }
    "Anime Engine project".into()
}

/// The queue, snapshotted once per version.
fn cached_snapshot() -> Queue {
    let v = queue::version();
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        match c.as_ref() {
            Some((seen, q)) if *seen == v => q.clone(),
            _ => {
                let q = queue::snapshot();
                *c = Some((v, q.clone()));
                q
            }
        }
    })
}

fn state_icon(j: &Job, k: usize) -> AnyElement {
    match j.state {
        JobState::Running => spinner(("aq-spin", k), Tokens::accent(), icon_px::SM).into_any_element(),
        JobState::Queued => icon_ph(ph::HOURGLASS, Tokens::text_3(), icon_px::SM).into_any_element(),
        JobState::Paused => icon_ph(ph::PAUSE, Tokens::text_3(), icon_px::SM).into_any_element(),
        JobState::Failed => ph_icon(ph::WARNING_CIRCLE, Weight::Fill, Tokens::danger(), icon_px::SM).into_any_element(),
        JobState::Done => ph_icon(ph::CHECK_CIRCLE, Weight::Fill, Tokens::success(), icon_px::SM).into_any_element(),
    }
}

/// Asks before removing every job that has not started (queued or paused, of every project).
fn confirm_cancel_waiting(n: usize, window: &mut Window, cx: &mut App) {
    if n == 0 {
        return;
    }
    mv_ui::confirm_dialog(
        if n == 1 { "Cancel the waiting job?".to_string() } else { format!("Cancel the {n} waiting jobs?") },
        format!("{} What is running now carries on, and nothing already made is touched.", cancel_waiting_line(n)),
        "Cancel waiting jobs",
        true,
    )
    .cancel_label("Keep them")
    .on_confirm(|_, cx| {
        let gone = queue::cancel_waiting(None);
        if gone > 0 {
            mv_ui::toast::show(cx, mv_ui::Toast::success(format!("Cancelled {}.", crate::widgets::count(gone, "waiting job"))));
        }
    })
    .open(window, cx);
}

/// "The job that has not started yet leaves the queue." / "The 3 jobs that have … leave …".
fn cancel_waiting_line(n: usize) -> String {
    if n == 1 {
        "The job that has not started yet leaves the queue.".into()
    } else {
        format!("The {n} jobs that have not started yet leave the queue.")
    }
}

/// Starts the runner (if needed) and resumes the whole queue.
fn resume_everything(cx: &mut App) {
    crate::backends::ensure_runner(cx);
    queue::resume_all();
}

impl Strip {
    /// The strip under the step body: `project` is the open project (its jobs first; other
    /// projects' jobs are named).
    pub fn view(&self, q: &Queue, eta: &Eta, project: Option<&str>, cx: &mut Context<EnginePage>) -> Div {
        let all = summarize(q, None);
        let waiting = all.waiting + all.paused;
        let now = now_secs();
        let running: Vec<&Job> = q.jobs.iter().filter(|j| j.state == JobState::Running).collect();
        let held = q.paused || q.interrupted;
        let header = div()
            .id("aq-head")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(space::MD))
            .h(px(QUEUE_H))
            .px(px(space::LG))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(space::SM))
                    .child(icon_ph(ph::QUEUE, Tokens::text_2(), icon_px::SM))
                    .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Queue"))
                    .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(summary_text(&all, held))),
            )
            .child(div().flex().flex_1().min_w_0().items_center().gap(px(space::LG)).overflow_hidden().when(!self.expanded, |d| {
                d.children(running.iter().take(2).enumerate().map(|(k, j)| {
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(space::SM))
                        .max_w(px(360.))
                        .child(state_icon(j, k))
                        .child(div().min_w_0().truncate().text_size(px(size::SM)).text_color(Tokens::text_1()).child(j.label.clone()))
                        .child(div().flex_none().w(px(72.)).child(progress_bar(("aq-bar", k), j.progress, !held)))
                        .child(div().flex_none().text_size(px(size::XS)).text_color(Tokens::text_3()).child(time_text(j, eta.mean(j.kind.name()), now)))
                }))
            }))
            .child(
                btn("aq-cancel-waiting", "Cancel all waiting", ButtonKind::Ghost)
                    .small()
                    .icon(ph::X_CIRCLE)
                    .disabled_if(all.waiting + all.paused == 0, "Nothing is waiting")
                    .on_click(move |_, w, cx| confirm_cancel_waiting(waiting, w, cx)),
            )
            // After a restart the notice above asks to resume; one Resume is enough.
            .map(|d| match toggle_of(q) {
                Toggle::None => d,
                Toggle::Resume => d.child(btn("aq-resume", "Resume all", ButtonKind::Secondary).small().icon(ph::PLAY).on_click(|_, _, cx| resume_everything(cx))),
                Toggle::Pause => d.child(btn("aq-pause", "Pause all", ButtonKind::Ghost).small().icon(ph::PAUSE).disabled_if(all.active() == 0, "Nothing is running").on_click(|_, _, _| queue::pause_all())),
            })
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(space::SM))
                    .text_size(px(size::SM))
                    .text_color(Tokens::text_2())
                    .child(switch("aq-hide-failed", self.hide_failed).on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.strip.hide_failed = *on;
                        queue::set_hidden_failed(*on);
                        cx.notify();
                    })))
                    .child("Hide failed"),
            )
            .child(icon_button("aq-toggle", if self.expanded { ph::CARET_DOWN } else { ph::CARET_UP }, if self.expanded { "Fold the queue" } else { "Show everything in the queue" }).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.strip.expanded = !this.strip.expanded;
                cx.notify();
            })));
        let interrupted = q.interrupted.then(|| {
            div().px(px(space::LG)).pt(px(space::SM)).child(banner(AlertKind::Info, "The queue was stopped when MangaVox closed. Resume?").action("Resume", |_, _, cx| resume_everything(cx)))
        });
        let list = self.expanded.then(|| self.list(q, eta, project, now, held, cx));
        div().flex().flex_col().flex_none().border_t_1().border_color(Tokens::border()).bg(Tokens::canvas()).children(interrupted).child(header).children(list)
    }

    fn list(&self, q: &Queue, eta: &Eta, project: Option<&str>, now: i64, held: bool, _cx: &mut Context<EnginePage>) -> Stateful<Div> {
        let jobs = ordered(q, self.hide_failed);
        let done = jobs.iter().any(|j| j.state == JobState::Done);
        let body = if jobs.is_empty() {
            div().flex().items_center().justify_center().h_full().text_size(px(size::SM)).text_color(Tokens::text_3()).child("Nothing in the queue. What you start in any step waits here.").into_any_element()
        } else {
            div().flex().flex_col().gap(px(2.)).children(jobs.iter().enumerate().map(|(k, j)| job_row(k, j, eta, project, now, held))).into_any_element()
        };
        div()
            .id("aq-list")
            .flex()
            .flex_col()
            .h(px(QUEUE_OPEN_H - QUEUE_H))
            .px(px(space::MD))
            .pb(px(space::SM))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .when(done, |d| d.child(div().flex().justify_end().px(px(space::SM)).pb(px(space::XS)).child(btn("aq-clear", "Clear finished", ButtonKind::Ghost).small().icon(ph::BROOM).on_click(|_, _, _| queue::clear_finished()))))
            .child(body)
    }
}

fn job_row(k: usize, j: &Job, eta: &Eta, project: Option<&str>, now: i64, held: bool) -> Stateful<Div> {
    let id = j.id;
    let other = project.is_some_and(|p| p != j.project);
    let detail = match (&j.error, j.line.trim()) {
        (Some(e), _) if j.state == JobState::Failed => e.clone(),
        (_, "") => String::new(),
        (_, line) => line.to_string(),
    };
    let actions = div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(2.))
        .when(matches!(j.state, JobState::Running | JobState::Queued), |d| d.child(icon_button(("aq-p", k), ph::PAUSE, "Pause this job").small().on_click(move |_, _, _| queue::pause(id))))
        .when(j.state == JobState::Paused, |d| {
            d.child(icon_button(("aq-r", k), ph::PLAY, "Resume this job").small().on_click(move |_, _, cx| {
                crate::backends::ensure_runner(cx);
                queue::resume(id);
            }))
        })
        .when(j.state == JobState::Failed, |d| {
            d.child(icon_button(("aq-t", k), ph::ARROW_CLOCKWISE, "Try again").small().on_click(move |_, _, cx| {
                crate::backends::ensure_runner(cx);
                queue::retry(id);
            }))
        })
        .when(j.state != JobState::Done, |d| d.child(icon_button(("aq-x", k), ph::X, if j.state == JobState::Failed { "Remove from the queue" } else { "Cancel this job" }).small().on_click(move |_, _, _| queue::cancel(id))));
    div()
        .id(("aq-row", k))
        .flex()
        .items_center()
        .gap(px(space::SM))
        .min_h(px(36.))
        .px(px(space::SM))
        .rounded(px(radius::MD))
        .hover(|s| s.bg(Tokens::hover()))
        .child(state_icon(j, k + 1000))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .child(div().min_w_0().truncate().text_size(px(size::SM)).text_color(if j.state == JobState::Failed { Tokens::danger() } else { Tokens::text_1() }).child(j.label.clone()))
                        .when(other, |d| d.child(div().flex_none().text_size(px(size::XS)).text_color(Tokens::text_3()).child(title_of(&j.project)))),
                )
                .when(!detail.is_empty(), |d| d.child(div().id(("aq-detail", k)).truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).tooltip(tip(detail.clone())).child(detail.clone()))),
        )
        .when(j.state == JobState::Running, |d| d.child(div().flex_none().w(px(120.)).child(progress_bar(("aq-row-bar", k), j.progress, !held))))
        .child(div().flex_none().w(px(170.)).text_right().text_size(px(size::XS)).text_color(Tokens::text_3()).child(time_text(j, eta.mean(j.kind.name()), now)))
        .child(actions)
}

/// Disabled state with a reason only when `off`.
trait DisabledIf {
    fn disabled_if(self, off: bool, reason: &'static str) -> Self;
}

impl DisabledIf for mv_ui::Btn {
    fn disabled_if(self, off: bool, reason: &'static str) -> Self {
        if off {
            self.disabled(reason)
        } else {
            self
        }
    }
}

// ---------------------------------------------------------------- activity cards

/// The engine's corner cards (shell, design 1.4 #11-12): one per project with work in the queue
/// (progress, what runs, how many wait, Pause / Resume and Open), or with failures to look at.
pub fn activity_cards(_cx: &mut App) -> Vec<Card> {
    let q = cached_snapshot();
    let mut projects: Vec<String> = Vec::new();
    for j in &q.jobs {
        if !projects.contains(&j.project) {
            projects.push(j.project.clone());
        }
    }
    let held = q.paused || q.interrupted;
    let now = now_secs();
    let mut cards = Vec::new();
    for p in projects {
        let s = summarize(&q, Some(&p));
        if s.active() == 0 && s.failed == 0 {
            continue;
        }
        let key = format!("anime-{p}");
        let order = ORDERS.with(|o| *o.borrow_mut().entry(key.clone()).or_insert_with(mv_ui::activity::next_order));
        let title = title_of(&p);
        let open = btn("open", "Open", ButtonKind::Ghost).small().on_click(|_, w, cx| mv_ui::navigate(Route::AnimeEngine, w, cx));
        if s.active() > 0 {
            let run = q.jobs.iter().find(|j| j.project == p && j.state == JobState::Running);
            let p_done = run.map(|j| j.progress).unwrap_or(0.);
            let line = match run {
                Some(j) => format!("{} · {}", j.label, time_text(j, None, now)),
                None if held => "Paused".into(),
                None => "Waiting to start".into(),
            };
            let waiting = s.waiting + s.paused;
            let mut card = Card::new(key, order, Lead::Progress(p_done), title, line).progress(p_done, run.is_some() && !held);
            if waiting > 0 {
                card = card.count(format!("{waiting} waiting"));
            }
            let toggle = if held {
                btn("resume", "Resume", ButtonKind::Ghost).small().icon(ph::PLAY).on_click(|_, _, cx| resume_everything(cx))
            } else {
                btn("pause", "Pause", ButtonKind::Ghost).small().icon(ph::PAUSE).on_click(|_, _, _| queue::pause_all())
            };
            cards.push(card.action(toggle).action(open));
        } else {
            let first = q.jobs.iter().find(|j| j.project == p && j.state == JobState::Failed && !j.hidden);
            let line = first.map(|j| format!("{}: {}", j.label, j.error.clone().unwrap_or_else(|| "it stopped".into()))).unwrap_or_default();
            let failed: Vec<u64> = q.jobs.iter().filter(|j| j.project == p && j.state == JobState::Failed && !j.hidden).map(|j| j.id).collect();
            let retry = btn("retry", "Try again", ButtonKind::Ghost).small().icon(ph::ARROW_CLOCKWISE).on_click(move |_, _, cx| {
                crate::backends::ensure_runner(cx);
                for id in &failed {
                    queue::retry(*id);
                }
            });
            cards.push(Card::new(key, order, Lead::Failed, title, line).count(format!("{} failed", s.failed)).action(retry).action(open).on_close(|_, _| queue::set_hidden_failed(true)));
        }
    }
    cards
}

#[cfg(test)]
mod tests {
    use super::{ordered, summarize, summary_text, time_text, Eta, Summary};
    use mv_anime::queue::{Job, JobKind, JobState, Queue};
    use std::collections::BTreeMap;

    fn job(id: u64, project: &str, state: JobState) -> Job {
        Job { id, project: project.into(), state, label: format!("job {id}"), kind: JobKind::Story { episode: "ep01".into() }, ..Default::default() }
    }

    #[test]
    fn counts_and_words() {
        let mut q = Queue::default();
        q.jobs = vec![job(1, "a", JobState::Running), job(2, "a", JobState::Queued), job(3, "b", JobState::Queued), job(4, "a", JobState::Failed), job(5, "a", JobState::Done)];
        q.jobs[3].hidden = false;
        let all = summarize(&q, None);
        assert_eq!(all, Summary { running: 1, waiting: 2, paused: 0, failed: 1, done: 1 });
        assert_eq!(summarize(&q, Some("b")).waiting, 1);
        assert_eq!(summary_text(&all, false), "1 running · 2 waiting · 1 failed");
        assert_eq!(summary_text(&all, true), "Paused · 1 running · 2 waiting · 1 failed");
        assert_eq!(summary_text(&Summary::default(), false), "Nothing in the queue");
        assert_eq!(summary_text(&Summary { done: 3, ..Default::default() }, false), "3 done");
        q.jobs[3].hidden = true;
        assert_eq!(summarize(&q, None).failed, 0);
    }

    #[test]
    fn running_first_failed_hidden_on_request() {
        let mut q = Queue::default();
        q.jobs = vec![job(1, "a", JobState::Done), job(2, "a", JobState::Failed), job(3, "a", JobState::Queued), job(4, "a", JobState::Running), job(5, "a", JobState::Done)];
        let ids: Vec<u64> = ordered(&q, false).iter().map(|j| j.id).collect();
        assert_eq!(ids, vec![4, 3, 2, 5, 1]);
        let ids: Vec<u64> = ordered(&q, true).iter().map(|j| j.id).collect();
        assert_eq!(ids, vec![4, 3, 5, 1]);
    }

    #[test]
    fn time_left_from_the_mean() {
        let mut j = job(1, "a", JobState::Running);
        j.started_at = Some(1_000);
        assert_eq!(time_text(&j, Some(120.), 1_030), "30 s · about 2 min left");
        assert_eq!(time_text(&j, Some(31.), 1_030), "30 s · almost done");
        assert_eq!(time_text(&j, None, 1_030), "30 s");
        // Milliseconds read the same.
        j.started_at = Some(1_000_000_000_000);
        assert_eq!(time_text(&j, None, 1_000_000_045), "45 s");
        let w = job(2, "a", JobState::Queued);
        assert_eq!(time_text(&w, Some(44.6), 0), "about 45 s");
        assert_eq!(time_text(&w, None, 0), "waiting");
        // A paused job takes no time until it is resumed, and is not "waiting".
        let p = job(3, "a", JobState::Paused);
        assert_eq!(time_text(&p, Some(44.6), 0), "paused");
        assert_eq!(summary_text(&Summary { waiting: 2, paused: 1, ..Default::default() }, false), "2 waiting · 1 paused");
    }

    #[test]
    fn one_resume_at_a_time() {
        use super::{toggle_of, Toggle};
        let mut q = Queue { jobs: vec![job(1, "a", JobState::Queued)], ..Default::default() };
        assert_eq!(toggle_of(&q), Toggle::Pause);
        q.paused = true;
        assert_eq!(toggle_of(&q), Toggle::Resume);
        // After a restart the notice carries Resume; the strip shows only the state.
        q.interrupted = true;
        assert_eq!(toggle_of(&q), Toggle::None);
        assert_eq!(summary_text(&summarize(&q, None), q.paused || q.interrupted), "Paused · 1 waiting");
    }

    #[test]
    fn eta_means_skip_junk() {
        let mut t = BTreeMap::new();
        t.insert("frame".to_string(), vec![40., 50., f32::NAN, -1.]);
        t.insert("story".to_string(), vec![]);
        let e = Eta::from_table(&t);
        assert_eq!(e.mean("frame"), Some(45.));
        assert_eq!(e.mean("story"), None);
    }
}
