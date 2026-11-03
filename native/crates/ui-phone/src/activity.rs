//! Activity (API.md "Activity"): what the PC is doing, one card per manga: chapter downloads,
//! Studio Generate (with its steps), Python narration / video jobs, exports and full-recap joins,
//! and Studio's AI reading runs; plus a short log of what finished, failed or hit an AI limit, for
//! the phone's Activity tab and its in-app notifications.
//!
//! [`build`] turns plain snapshots of the desktop's queues into cards and [`Tracker`] remembers
//! runs, keeps finished work on show for a while and logs events; both are pure and unit tested.
//! A background tick (started by the first request, ended [`IDLE`] after the last one) keeps the
//! log complete while no phone is looking, so the tray shows what happened while the app was in
//! the background.
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::routing::get;
use axum::{Json, Router};
use mv_recap::Step;
use serde::Serialize;
use ui_studio::pipeline::{self, AiActivity, ExportActivity, ItemState, QueueItem, StepState};
use ui_studio::service::{self, JobInfo, JobView};

/// Finished and failed work stays on the cards this long (the event log keeps it after).
const LINGER: Duration = Duration::from_secs(30 * 60);
/// Events kept for the tray.
const KEEP_EVENTS: usize = 40;
/// A request within this long of the last build gets that build (several phones, quick polls).
const FRESH: Duration = Duration::from_millis(800);
/// The background tick, and how long it runs after the last request.
const TICK: Duration = Duration::from_secs(2);
const IDLE: Duration = Duration::from_secs(30 * 60);

pub fn routes() -> Router {
    Router::new().route("/api/activity", get(activity))
}

// ---------------------------------------------------------------- inputs (plain data)

/// One title's downloads (`ui_manga::Downloads::activity`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dl {
    pub manga_id: String,
    pub title: String,
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    pub last_error: Option<String>,
    pub paused: bool,
    pub finished: bool,
    /// The chapter downloading now ("Ch. 12") and its (saved, total) pages.
    pub chapter: Option<String>,
    pub pages: Option<(usize, usize)>,
}

/// A full-recap join started from the phone (`api_studio`).
#[derive(Clone, Debug, PartialEq)]
pub struct Join {
    /// Its job id ("r3"), which `/api/studio/jobs/{id}/cancel` takes.
    pub id: String,
    pub manga_id: String,
    pub title: String,
    /// "queued", "running", "done" or "failed".
    pub state: &'static str,
    pub progress: f32,
    pub line: String,
    pub error: Option<String>,
}

/// Everything [`build`] reads.
#[derive(Clone, Debug, Default)]
pub struct Sources {
    pub downloads: Vec<Dl>,
    /// The Generate queue, and whether it is held as a whole (and because MangaVox closed).
    pub queue: Vec<QueueItem>,
    pub held: bool,
    pub interrupted: bool,
    /// Every Python job (the ones a Generate chapter runs show as its steps instead).
    pub jobs: Vec<JobInfo>,
    pub ai: Option<AiActivity>,
    pub exports: Vec<ExportActivity>,
    pub joins: Vec<Join>,
    /// "Make in Studio" chains from the phone (download, then Generate).
    pub chains: Vec<crate::studio_chain::Chain>,
    /// Manga id -> (title, has a cover).
    pub manga: HashMap<String, (String, bool)>,
    /// Chapter id -> manga id, for AI runs and Python jobs.
    pub chapters: HashMap<String, String>,
}

// ---------------------------------------------------------------- output

/// One step of a Generate chapter.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StepChip {
    pub name: &'static str,
    /// "pending", "skipped", "running", "done" or "failed".
    pub state: &'static str,
}

/// One kind of work on one manga.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Task {
    /// Unique per kind and manga ("dl:<manga id>").
    pub key: String,
    /// "download", "generate", "voice", "read" or "export".
    pub kind: &'static str,
    /// "Downloading", "Generate", ...
    pub label: &'static str,
    /// "running", "limit" (waiting for an AI usage limit), "waiting", "paused", "failed" or "done".
    pub state: &'static str,
    /// "Ch. 12 · page 4 of 20".
    pub line: String,
    /// 0..=1.
    pub progress: f32,
    /// Chapters at (the one in progress counts) and in this run; total 0 when not counted in chapters.
    pub at: usize,
    pub total: usize,
    /// Generate: the steps of the chapter in focus.
    pub steps: Vec<StepChip>,
    /// When an AI limit lifts (unix seconds).
    pub resume_at: Option<i64>,
    pub error: Option<String>,
    /// Job ids `/api/studio/jobs/{id}/cancel` takes to stop this work.
    pub cancel: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MangaCard {
    pub id: String,
    pub title: String,
    pub cover: Option<String>,
    /// The most pressing task state.
    pub state: &'static str,
    pub tasks: Vec<Task>,
}

/// Something worth telling: work finished, failed or hit an AI limit.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Event {
    pub id: u64,
    /// Unix milliseconds (the PC's clock; compare with [`Activity::now`]).
    pub at: u64,
    pub manga_id: String,
    pub title: String,
    pub kind: &'static str,
    pub label: &'static str,
    /// "done", "failed" or "limit".
    pub state: &'static str,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Activity {
    /// Unix milliseconds on the PC.
    pub now: u64,
    /// Manga with work running, waiting or paused.
    pub busy: usize,
    pub manga: Vec<MangaCard>,
    /// Oldest first.
    pub events: Vec<Event>,
    /// Private mode is on (read on every request, so the phone follows the PC's switch).
    pub private: bool,
}

fn is_active(state: &str) -> bool {
    matches!(state, "running" | "limit" | "waiting" | "paused")
}

fn rank(state: &str) -> u8 {
    match state {
        "running" => 0,
        "limit" => 1,
        "waiting" => 2,
        "paused" => 3,
        "failed" => 4,
        _ => 5,
    }
}

fn kind_order(kind: &str) -> u8 {
    match kind {
        "download" => 0,
        "studio" | "generate" => 1,
        "voice" => 2,
        "read" => 3,
        _ => 4,
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Percent-encodes one URL path segment.
fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// "Chapter 3 · The Duel" -> "Ch. 3"; other labels as they are.
fn short_chapter(label: &str) -> String {
    let head = label.split(" · ").next().unwrap_or(label);
    match head.strip_prefix("Chapter ") {
        Some(n) => format!("Ch. {n}"),
        None => head.to_string(),
    }
}

/// What a Generate step is doing, for the line under the manga.
fn doing(step: Step) -> &'static str {
    match step {
        Step::Read => "reading the pages",
        Step::Transcript => "writing the transcript",
        Step::Story => "writing the recap",
        Step::Audio => "narrating",
        Step::Music => "composing the music",
        Step::Video => "rendering the video",
    }
}

fn step_state(s: StepState) -> &'static str {
    match s {
        StepState::Pending => "pending",
        StepState::Skipped => "skipped",
        StepState::Running => "running",
        StepState::Done => "done",
        StepState::Failed => "failed",
    }
}

fn task(key: String, kind: &'static str, label: &'static str, state: &'static str, line: String, progress: f32) -> Task {
    Task { key, kind, label, state, line, progress: progress.clamp(0., 1.), at: 0, total: 0, steps: vec![], resume_at: None, error: None, cancel: vec![] }
}

// ---------------------------------------------------------------- runs

/// A run of one kind of work on one manga: its items are the ones from `from` on (older ones were
/// earlier runs), so "3 of 5" counts this run only. Like the desktop's Generate cards.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Run {
    from: u64,
    ended: bool,
}

pub type Runs = HashMap<String, Run>;

/// Follows a run: items still to make start one (or carry it on); none left ends it. A run is only
/// ever started by work still to make, so finished work from before is never shown as new.
fn track(runs: &mut Runs, key: &str, first_active: Option<u64>, any: bool) -> Option<u64> {
    match first_active {
        Some(min) => {
            let r = runs.entry(key.to_string()).or_insert(Run { from: min, ended: false });
            if r.ended {
                *r = Run { from: min, ended: false };
            }
            r.from = r.from.min(min);
            Some(r.from)
        }
        None if any => runs.get_mut(key).map(|r| {
            r.ended = true;
            r.from
        }),
        None => {
            runs.remove(key);
            None
        }
    }
}

// ---------------------------------------------------------------- build

/// A "Make in Studio" chain: chapters downloading, then handed to Generate.
fn chain_task(c: &crate::studio_chain::Chain) -> Task {
    let key = format!("chain:{}", c.manga_id);
    let (total, w) = (c.total(), c.waiting.len());
    let progress = c.sent as f32 / total.max(1) as f32;
    let mut t = if w > 0 {
        task(key, "studio", "Make in Studio", "running", format!("Downloading {w} chapter{}, then Generate", plural(w)), progress)
    } else if c.failed > 0 {
        let mut t = task(key, "studio", "Make in Studio", "failed", format!("{} of {total} chapters sent to Generate", c.sent), progress);
        t.error = Some(format!("{} chapter{} could not be downloaded or queued", c.failed, plural(c.failed)));
        t
    } else {
        task(key, "studio", "Make in Studio", "done", format!("{} chapter{} sent to Generate", c.sent, plural(c.sent)), 1.)
    };
    (t.at, t.total) = (c.sent, total);
    t
}

fn download_task(d: &Dl) -> Task {
    let key = format!("dl:{}", d.manga_id);
    if d.finished {
        let mut t = if d.failed > 0 {
            let mut t = task(key, "download", "Download", "failed", format!("{} of {} chapters failed", d.failed, d.total), d.done as f32 / d.total.max(1) as f32);
            t.error.clone_from(&d.last_error);
            t
        } else {
            task(key, "download", "Download", "done", format!("{} chapter{} saved for offline reading", d.done, plural(d.done)), 1.)
        };
        (t.at, t.total) = (d.done, d.total);
        return t;
    }
    let partial = d.pages.map_or(0., |(p, n)| if n == 0 { 0. } else { p as f32 / n as f32 });
    let (state, line) = match (&d.chapter, d.pages) {
        (Some(c), _) if d.paused => ("running", format!("{c} · pausing after this chapter")),
        (Some(c), Some((p, n))) if n > 0 => ("running", format!("{c} · page {} of {n}", (p + 1).min(n))),
        (Some(c), _) => ("running", format!("{c} · preparing")),
        (None, _) if d.paused => ("paused", "Paused".to_string()),
        (None, _) => ("waiting", "Waiting for the downloads before it".to_string()),
    };
    let mut t = task(key, "download", "Downloading", state, line, (d.done as f32 + partial) / d.total.max(1) as f32);
    t.at = if d.chapter.is_some() { (d.done + 1).min(d.total) } else { d.done };
    t.total = d.total;
    if d.failed > 0 {
        t.error = Some(format!("{} chapter{} failed", d.failed, plural(d.failed)));
    }
    t
}

/// The AI limit a running chapter waits on: (why, until).
fn limit_for<'a>(ai: Option<&'a AiActivity>, chapter_id: &str) -> Option<&'a (String, i64)> {
    ai.filter(|a| a.state == "running" && a.chapter_id.as_deref() == Some(chapter_id)).and_then(|a| a.waiting.as_ref())
}

fn generate_task(manga_id: &str, items: &[&QueueItem], src: &Sources) -> Task {
    let key = format!("gen:{manga_id}");
    let (mut done, mut failed, mut sum) = (0usize, 0usize, 0f32);
    let mut active = vec![];
    let mut paused = 0;
    let mut running: Option<&QueueItem> = None;
    for it in items {
        match it.state {
            ItemState::Done => {
                done += 1;
                sum += 1.;
            }
            ItemState::Failed => failed += 1,
            s => {
                active.push(*it);
                if s == ItemState::Paused {
                    paused += 1;
                }
                if s == ItemState::Running && running.is_none() {
                    running = Some(it);
                }
                let steps = it.until.min(it.steps.len() - 1) + 1;
                // The audio / video steps report through their job, the AI steps through the run.
                let inner = if s != ItemState::Running {
                    0.
                } else if let Some(j) = it.job.and_then(|id| src.jobs.iter().find(|j| j.id == id)).filter(|j| !j.state.finished()) {
                    j.progress() as f32
                } else {
                    src.ai.as_ref().filter(|a| a.state == "running" && a.chapter_id.as_deref() == Some(it.chapter.id.as_str()) && a.pages_total > 0).map_or(0., |a| a.pages_done as f32 / a.pages_total as f32)
                };
                sum += (it.progress() + inner / steps as f32).min(1.);
            }
        }
    }
    let total = items.len();
    let progress = if total == 0 { 0. } else { sum / total as f32 };
    let focus = running.or(active.first().copied()).or(items.iter().find(|i| i.state == ItemState::Failed).copied()).or(items.last().copied());
    let steps = focus
        .map(|it| Step::ALL.iter().zip(it.steps.iter()).take(it.until.min(Step::ALL.len() - 1) + 1).map(|(s, st)| StepChip { name: s.label(), state: step_state(*st) }).collect())
        .unwrap_or_default();
    let label = |it: &QueueItem| pipeline::short_label(it.chapter.number, it.chapter.title.as_deref());

    let mut t = if active.is_empty() {
        if failed > 0 {
            let mut t = task(key, "generate", "Generate", "failed", format!("{failed} of {total} chapters failed"), progress);
            t.error = items.iter().find(|i| i.state == ItemState::Failed).and_then(|i| i.error.clone());
            t
        } else {
            task(key, "generate", "Generate", "done", format!("{done} chapter{} made", plural(done)), 1.)
        }
    } else if let Some(it) = running {
        let what = it.current_step().map_or("starting", doing);
        match limit_for(src.ai.as_ref(), &it.chapter.id) {
            Some((why, until)) => {
                let mut t = task(key, "generate", "Generate", "limit", format!("{} · waiting for the AI limit", label(it)), progress);
                t.resume_at = Some(*until);
                t.error = Some(why.clone());
                t
            }
            None => {
                let pages = src
                    .ai
                    .as_ref()
                    .filter(|a| a.state == "running" && a.chapter_id.as_deref() == Some(it.chapter.id.as_str()) && a.pages_total > 0)
                    .map(|a| format!(" · {}/{} pages", a.pages_done.min(a.pages_total), a.pages_total))
                    .unwrap_or_default();
                task(key, "generate", "Generate", "running", format!("{} · {what}{pages}", label(it)), progress)
            }
        }
    } else if src.held || paused == active.len() {
        let line = if src.held && src.interrupted { "Paused when MangaVox closed" } else { "Paused" };
        task(key, "generate", "Generate", "paused", line.into(), progress)
    } else {
        task(key, "generate", "Generate", "waiting", "Waiting for the chapters before it".into(), progress)
    };
    t.at = if running.is_some() { (done + 1).min(total) } else { done };
    t.total = total;
    t.steps = steps;
    t.cancel = active.iter().map(|i| format!("p{}", i.id)).collect();
    if failed > 0 && t.error.is_none() {
        t.error = Some(format!("{failed} chapter{} failed", plural(failed)));
    }
    t
}

/// Python jobs Studio started outside Generate (its Audio / Video phases, the phone's "audio" /
/// "video"), for one manga.
fn voice_task(key_manga: &str, jobs: &[&JobInfo]) -> Task {
    let key = format!("job:{key_manga}");
    let done = jobs.iter().filter(|j| j.state == JobView::Done).count();
    let failed: Vec<&&JobInfo> = jobs.iter().filter(|j| matches!(j.state, JobView::Failed(_))).collect();
    let active: Vec<&&JobInfo> = jobs.iter().filter(|j| !j.state.finished()).collect();
    let running = jobs.iter().find(|j| j.state == JobView::Running);
    let total = jobs.len();
    let progress = (done as f32 + running.map_or(0., |j| j.progress() as f32)) / total.max(1) as f32;
    let last_error = failed.last().and_then(|j| match &j.state {
        JobView::Failed(e) => Some(e.clone()),
        _ => None,
    });
    let mut t = if let Some(j) = running {
        let stage = j.stage.as_deref().map(service::stage_label).unwrap_or("Starting");
        task(key, "voice", "Narration & video", "running", format!("{} · {}", short_chapter(&j.chapter_label), stage.to_lowercase()), progress)
    } else if !active.is_empty() {
        task(key, "voice", "Narration & video", "waiting", "Waiting in the queue".into(), progress)
    } else if !failed.is_empty() {
        let mut t = task(key, "voice", "Narration & video", "failed", format!("{} of {total} chapters failed", failed.len()), progress);
        t.error.clone_from(&last_error);
        t
    } else {
        task(key, "voice", "Narration & video", "done", format!("{done} chapter{} voiced", plural(done)), 1.)
    };
    t.at = if running.is_some() { (done + 1).min(total) } else { done };
    t.total = total;
    t.cancel = active.iter().map(|j| format!("s{}", j.id)).collect();
    t
}

fn ai_task(key_manga: &str, a: &AiActivity) -> Option<Task> {
    let key = format!("ai:{key_manga}");
    let label = if a.kind == "story" { "AI story" } else { "AI reading" };
    let (pos, n) = a.chapter_pos;
    let mut t = match a.state {
        "running" => match &a.waiting {
            Some((why, until)) => {
                let mut t = task(key, "read", label, "limit", "Waiting for the AI limit".into(), a.progress);
                t.resume_at = Some(*until);
                t.error = Some(why.clone());
                t
            }
            None => task(key, "read", label, "running", a.status.clone(), a.progress),
        },
        "done" => task(key, "read", label, "done", format!("{n} chapter{} done", plural(n)), 1.),
        "failed" => {
            let mut t = task(key, "read", label, "failed", "Stopped with an error".into(), a.progress);
            t.error.clone_from(&a.error);
            t
        }
        _ => return None,
    };
    (t.at, t.total) = (if a.state == "done" { n } else { pos.min(n) }, n);
    Some(t)
}

/// One card per manga from the snapshots. `runs` remembers where each run started.
pub fn build(src: &Sources, runs: &mut Runs) -> Vec<MangaCard> {
    // manga key -> (title, tasks), in first-seen order.
    let mut order: Vec<String> = vec![];
    let mut cards: HashMap<String, (String, Vec<Task>)> = HashMap::new();
    let mut put = |manga: &str, title: &str, t: Task| {
        let e = cards.entry(manga.to_string()).or_insert_with(|| {
            order.push(manga.to_string());
            (title.to_string(), vec![])
        });
        if e.0.is_empty() {
            e.0 = title.to_string();
        }
        e.1.push(t);
    };

    for d in &src.downloads {
        put(&d.manga_id, &d.title, download_task(d));
    }
    for c in &src.chains {
        put(&c.manga_id, &c.title, chain_task(c));
    }

    // Generate, per manga.
    let mut mangas: Vec<&str> = vec![];
    for it in &src.queue {
        if !mangas.contains(&it.chapter.manga_id.as_str()) {
            mangas.push(&it.chapter.manga_id);
        }
    }
    let mut live: HashSet<String> = HashSet::new();
    for m in mangas {
        let mine: Vec<&QueueItem> = src.queue.iter().filter(|i| i.chapter.manga_id == m).collect();
        let key = format!("gen:{m}");
        live.insert(key.clone());
        let first = mine.iter().filter(|i| i.state.active()).map(|i| i.id).min();
        let Some(from) = track(runs, &key, first, true) else { continue };
        let run: Vec<&QueueItem> = mine.into_iter().filter(|i| i.id >= from).collect();
        if let Some(first) = run.first() {
            put(m, &first.chapter.manga_title.clone(), generate_task(m, &run, src));
        }
    }

    // Python jobs Generate did not start, per manga.
    let queued: HashSet<&str> = src.queue.iter().map(|i| i.chapter.id.as_str()).collect();
    let mut by_manga: Vec<(String, String, Vec<&JobInfo>)> = vec![];
    for j in src.jobs.iter().filter(|j| !queued.contains(j.chapter_id.as_str()) && j.state != JobView::Cancelled) {
        let m = src.chapters.get(&j.chapter_id).cloned().unwrap_or_else(|| format!("t:{}", j.manga_title));
        match by_manga.iter_mut().find(|(k, _, _)| *k == m) {
            Some((_, _, list)) => list.push(j),
            None => by_manga.push((m, j.manga_title.clone(), vec![j])),
        }
    }
    for (m, title, jobs) in by_manga {
        let key = format!("job:{m}");
        live.insert(key.clone());
        let first = jobs.iter().filter(|j| !j.state.finished()).map(|j| j.id).min();
        let Some(from) = track(runs, &key, first, true) else { continue };
        let run: Vec<&JobInfo> = jobs.into_iter().filter(|j| j.id >= from).collect();
        if !run.is_empty() {
            put(&m, &title, voice_task(&m, &run));
        }
    }
    // Runs whose items all left the queues.
    runs.retain(|k, _| live.contains(k));

    // Studio's AI run when Generate did not start it (a Read or Story from the Studio page).
    if let Some(a) = src.ai.as_ref().filter(|a| a.chapter_id.as_deref().is_none_or(|c| !queued.contains(c))) {
        let m = a.chapter_id.as_ref().and_then(|c| src.chapters.get(c)).cloned().unwrap_or_else(|| "studio".to_string());
        if let Some(t) = ai_task(&m, a) {
            let title = src.manga.get(&m).map(|(t, _)| t.clone()).unwrap_or_else(|| "Studio".into());
            put(&m, &title, t);
        }
    }

    let mut exporting: HashSet<&str> = HashSet::new();
    for e in &src.exports {
        // One export per manga at a time (they take turns); the first one is the running one.
        if !exporting.insert(&e.manga_id) {
            continue;
        }
        let key = format!("export:{}", e.manga_id);
        let title = src.manga.get(&e.manga_id).map(|(t, _)| t.clone()).unwrap_or_default();
        let line = if e.label.is_empty() { "Exporting".to_string() } else { e.label.clone() };
        put(&e.manga_id, &title, task(key, "export", "Export", "running", line, e.progress));
    }

    for j in &src.joins {
        let state = match j.state {
            "queued" => "waiting",
            "running" => "running",
            "done" => "done",
            _ => "failed",
        };
        let mut t = task(format!("join:{}", j.manga_id), "export", "Full recap", state, j.line.clone(), if state == "done" { 1. } else { j.progress });
        t.error.clone_from(&j.error);
        if is_active(state) {
            t.cancel = vec![j.id.clone()];
        }
        put(&j.manga_id, &j.title, t);
    }

    order
        .into_iter()
        .filter_map(|m| {
            let (title, mut tasks) = cards.remove(&m)?;
            tasks.sort_by_key(|t| kind_order(t.kind));
            let (title, cover) = match src.manga.get(&m) {
                Some((t, has)) => (if t.is_empty() { title } else { t.clone() }, has.then(|| format!("/api/manga/{}/cover", pct(&m)))),
                None => (title, None),
            };
            let title = if title.is_empty() { "Untitled".to_string() } else { title };
            Some(MangaCard { id: m, title, cover, state: "done", tasks })
        })
        .collect()
}

// ---------------------------------------------------------------- tracker

/// Remembers runs and what each task last looked like: logs finished / failed / AI-limit events,
/// keeps finished tasks on show for [`LINGER`] (also after their source dropped them, like a
/// download card that closes itself after a few seconds).
#[derive(Default)]
pub struct Tracker {
    pub runs: Runs,
    seen: HashMap<String, &'static str>,
    /// Finished tasks on show: since when, the card they belong to (id, title, cover), the task.
    finished: HashMap<String, (Instant, (String, String, Option<String>), Task)>,
    events: VecDeque<Event>,
    next: u64,
}

impl Tracker {
    fn log(&mut self, wall_ms: u64, card: &MangaCard, t: &Task, state: &'static str) {
        self.next += 1;
        let text = if state == "limit" { "paused, the AI limit is reached".to_string() } else { t.line.clone() };
        self.events.push_back(Event { id: self.next, at: wall_ms, manga_id: card.id.clone(), title: card.title.clone(), kind: t.kind, label: t.label, state, text });
        while self.events.len() > KEEP_EVENTS {
            self.events.pop_front();
        }
    }

    /// The cards to show now, and the events so far.
    pub fn observe(&mut self, cards: Vec<MangaCard>, now: Instant, wall_ms: u64) -> Activity {
        let mut out: Vec<MangaCard> = vec![];
        let mut seen: HashMap<String, &'static str> = HashMap::new();
        for mut card in cards {
            let tasks = std::mem::take(&mut card.tasks);
            for t in tasks {
                let prev = self.seen.get(&t.key).copied();
                seen.insert(t.key.clone(), t.state);
                if is_active(t.state) {
                    self.finished.remove(&t.key);
                    if t.state == "limit" && prev.is_some_and(|p| p != "limit") {
                        self.log(wall_ms, &card, &t, "limit");
                    }
                    card.tasks.push(t);
                    continue;
                }
                if prev.is_some_and(is_active) {
                    self.log(wall_ms, &card, &t, t.state);
                }
                let meta = (card.id.clone(), card.title.clone(), card.cover.clone());
                let since = self.finished.get(&t.key).map_or(now, |f| f.0);
                self.finished.insert(t.key.clone(), (since, meta, t.clone()));
                if now.duration_since(since) < LINGER {
                    card.tasks.push(t);
                }
            }
            out.push(card);
        }
        // Finished work its source no longer reports stays until it is old.
        self.finished.retain(|_, f| now.duration_since(f.0) < LINGER);
        let mut kept: Vec<(&String, &(Instant, (String, String, Option<String>), Task))> = self.finished.iter().filter(|(k, _)| !seen.contains_key(*k)).collect();
        kept.sort_by_key(|(k, f)| (f.0, (*k).clone()));
        for (k, (_, (id, title, cover), t)) in kept {
            seen.insert(k.clone(), t.state);
            match out.iter_mut().find(|c| c.id == *id) {
                Some(c) => c.tasks.push(t.clone()),
                None => out.push(MangaCard { id: id.clone(), title: title.clone(), cover: cover.clone(), state: "done", tasks: vec![t.clone()] }),
            }
        }
        self.seen = seen;

        out.retain(|c| !c.tasks.is_empty());
        for c in &mut out {
            c.tasks.sort_by_key(|t| kind_order(t.kind));
            c.state = c.tasks.iter().map(|t| t.state).min_by_key(|s| rank(s)).unwrap_or("done");
        }
        out.sort_by(|a, b| rank(a.state).cmp(&rank(b.state)).then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase())).then_with(|| a.id.cmp(&b.id)));
        let busy = out.iter().filter(|c| is_active(c.state)).count();
        Activity { now: wall_ms, busy, manga: out, events: self.events.iter().cloned().collect(), private: false }
    }
}

// ---------------------------------------------------------------- live state

struct Shared {
    tracker: Tracker,
    last: Option<(Instant, Activity)>,
    asked: Option<Instant>,
    ticking: bool,
}

fn shared() -> MutexGuard<'static, Shared> {
    static S: OnceLock<Mutex<Shared>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Shared { tracker: Tracker::default(), last: None, asked: None, ticking: false })).lock().unwrap_or_else(|e| e.into_inner())
}

fn wall_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Titles and covers of `manga`, and the manga of `chapters`, from the library database.
fn lookup(s: &mut Sources) {
    let queued: HashSet<&str> = s.queue.iter().map(|i| i.chapter.id.as_str()).collect();
    let mut chapters: Vec<String> = s.jobs.iter().map(|j| j.chapter_id.clone()).filter(|c| !queued.contains(c.as_str())).collect();
    chapters.extend(s.ai.as_ref().and_then(|a| a.chapter_id.clone()));
    chapters.sort();
    chapters.dedup();
    chapters.truncate(500);
    let mut manga: Vec<String> = s.downloads.iter().map(|d| d.manga_id.clone()).chain(s.queue.iter().map(|i| i.chapter.manga_id.clone())).chain(s.exports.iter().map(|e| e.manga_id.clone())).chain(s.joins.iter().map(|j| j.manga_id.clone())).collect();
    let res = ui_manga::backend::with_db(|c| {
        let mut found: HashMap<String, String> = HashMap::new();
        if !chapters.is_empty() {
            let q = format!("SELECT id, manga_id FROM chapter WHERE id IN ({})", vec!["?"; chapters.len()].join(","));
            let mut st = c.prepare(&q)?;
            let rows = st.query_map(rusqlite::params_from_iter(chapters.iter()), |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for r in rows.flatten() {
                found.insert(r.0, r.1);
            }
        }
        manga.extend(found.values().cloned());
        manga.sort();
        manga.dedup();
        manga.truncate(500);
        let mut info: HashMap<String, (String, bool)> = HashMap::new();
        if !manga.is_empty() {
            let q = format!("SELECT id, title, cover_path FROM manga WHERE id IN ({})", vec!["?"; manga.len()].join(","));
            let mut st = c.prepare(&q)?;
            let rows = st.query_map(rusqlite::params_from_iter(manga.iter()), |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)))?;
            for (id, title, cover) in rows.flatten() {
                info.insert(id, (title, cover.is_some_and(|c| !c.trim().is_empty())));
            }
        }
        Ok((found, info))
    });
    if let Ok((found, info)) = res {
        s.chapters = found;
        s.manga = info;
    }
}

/// Reads every queue now.
async fn gather() -> Sources {
    // The download queue lives in the app (a GPUI entity); without the UI (tests, still starting)
    // there are no downloads to show.
    let ui = crate::bridge::on_ui(|_, cx| {
        let e = ui_manga::downloads(cx);
        e.read(cx)
            .activity()
            .into_iter()
            .map(|d| Dl { manga_id: d.manga_id, title: d.title, total: d.total, done: d.done, failed: d.failed, last_error: d.last_error, paused: d.paused, finished: d.finished, chapter: d.chapter, pages: d.pages })
            .collect::<Vec<_>>()
    });
    let downloads = tokio::time::timeout(Duration::from_secs(2), ui).await.ok().and_then(Result::ok).unwrap_or_default();
    tokio::task::spawn_blocking(move || {
        let q = pipeline::snapshot();
        let mut s = Sources { downloads, queue: q.items, held: q.paused, interrupted: q.interrupted, jobs: service::snapshot(), ai: pipeline::ai_activity(), exports: pipeline::exports(), joins: crate::api_studio::recap_activity(), chains: crate::studio_chain::activity(), ..Default::default() };
        lookup(&mut s);
        s
    })
    .await
    .unwrap_or_default()
}

async fn refresh() -> Activity {
    let src = gather().await;
    let mut g = shared();
    let cards = build(&src, &mut g.tracker.runs);
    let a = g.tracker.observe(cards, Instant::now(), wall_ms());
    g.last = Some((Instant::now(), a.clone()));
    a
}

/// Starts the background tick when it is not running. It reads the queues every [`TICK`] (when no
/// request just did) until [`IDLE`] after the last request or until the server stops.
fn ensure_ticker() {
    let start = {
        let mut g = shared();
        g.asked = Some(Instant::now());
        !std::mem::replace(&mut g.ticking, true)
    };
    if !start {
        return;
    }
    tokio::spawn(async {
        loop {
            tokio::time::sleep(TICK).await;
            let (stop, stale) = {
                let mut g = shared();
                let stop = g.asked.is_none_or(|t| t.elapsed() > IDLE) || !crate::state::status().running;
                if stop {
                    g.ticking = false;
                }
                (stop, g.last.as_ref().is_none_or(|(t, _)| t.elapsed() + Duration::from_millis(300) >= TICK))
            };
            if stop {
                break;
            }
            if stale {
                refresh().await;
            }
        }
    });
}

/// GET /api/activity
async fn activity() -> Json<Activity> {
    ensure_ticker();
    let cached = shared().last.as_ref().filter(|(t, _)| t.elapsed() < FRESH).map(|(_, a)| a.clone());
    let mut a = match cached {
        Some(a) => a,
        None => refresh().await,
    };
    a.private = mv_core::privacy::active();
    Json(a)
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use ui_studio::pipeline::QChapter;

    fn dl(m: &str) -> Dl {
        Dl { manga_id: m.into(), title: m.to_uppercase(), total: 4, done: 1, ..Default::default() }
    }

    fn item(id: u64, manga: &str, state: ItemState, done_steps: usize) -> QueueItem {
        let mut steps = [StepState::Pending; 6];
        for s in steps.iter_mut().take(done_steps) {
            *s = StepState::Done;
        }
        if state == ItemState::Running {
            steps[done_steps] = StepState::Running;
        }
        if state == ItemState::Failed {
            steps[done_steps] = StepState::Failed;
        }
        if state == ItemState::Done {
            steps = [StepState::Done; 6];
        }
        QueueItem {
            id,
            chapter: QChapter { id: format!("c{id}"), manga_id: manga.into(), manga_title: manga.to_uppercase(), number: Some(id as f64), title: None, pages_dir: PathBuf::new(), output_dir: PathBuf::new() },
            until: 5,
            force: None,
            format: mv_recap::FORMAT_LANDSCAPE.into(),
            captions: None,
            state,
            steps,
            current: (state == ItemState::Running || state == ItemState::Failed).then_some(done_steps),
            error: (state == ItemState::Failed).then(|| "No AI model is picked".to_string()),
            note: None,
            job: None,
        }
    }

    fn job(id: u64, chapter: &str, state: JobView) -> JobInfo {
        JobInfo {
            id,
            chapter_id: chapter.into(),
            manga_title: "Solo".into(),
            chapter_label: "Chapter 3 · The Duel".into(),
            output_dir: PathBuf::new(),
            voice: "v".into(),
            stages: vec!["tts".into()],
            state,
            stage: Some("tts".into()),
            stage_percent: 50.0,
            status: String::new(),
            logs: VecDeque::new(),
            narrator: None,
            regen_pages: vec![],
        }
    }

    fn ai(chapter: &str, waiting: bool) -> AiActivity {
        AiActivity {
            kind: "read",
            chapter_id: Some(chapter.into()),
            chapter_pos: (1, 2),
            pages_done: 10,
            pages_total: 40,
            progress: 0.125,
            status: "Chapter 1 · reading page 11".into(),
            waiting: waiting.then(|| ("Usage limit reached".to_string(), 1_900_000_000)),
            state: "running",
            error: None,
        }
    }

    fn card<'a>(cards: &'a [MangaCard], id: &str) -> &'a MangaCard {
        cards.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("no card {id}"))
    }

    #[test]
    fn downloads_show_chapter_pages_and_counts() {
        let mut d = dl("a");
        d.chapter = Some("Ch. 2".into());
        d.pages = Some((4, 20));
        let t = download_task(&d);
        assert_eq!((t.state, t.line.as_str(), t.at, t.total), ("running", "Ch. 2 · page 5 of 20", 2, 4));
        assert!((t.progress - 1.2 / 4.0).abs() < 1e-6, "{}", t.progress);
        d.paused = true;
        assert_eq!(download_task(&d).line, "Ch. 2 · pausing after this chapter");
        let mut p = dl("a");
        p.paused = true;
        assert_eq!((download_task(&p).state, download_task(&p).line.as_str()), ("paused", "Paused"));
        assert_eq!(download_task(&dl("a")).state, "waiting");
        let mut f = dl("a");
        (f.finished, f.failed, f.last_error) = (true, 2, Some("timeout".into()));
        let t = download_task(&f);
        assert_eq!((t.state, t.line.as_str(), t.error.as_deref()), ("failed", "2 of 4 chapters failed", Some("timeout")));
        let mut ok = dl("a");
        (ok.finished, ok.done) = (true, 4);
        assert_eq!((download_task(&ok).state, download_task(&ok).line.as_str()), ("done", "4 chapters saved for offline reading"));
    }

    #[test]
    fn generate_groups_per_manga_with_steps() {
        let src = Sources { queue: vec![item(1, "a", ItemState::Done, 6), item(2, "a", ItemState::Running, 3), item(3, "a", ItemState::Queued, 0), item(4, "b", ItemState::Queued, 0)], ..Default::default() };
        let mut runs = Runs::new();
        let cards = build(&src, &mut runs);
        assert_eq!(cards.len(), 2);
        let a = &card(&cards, "a").tasks[0];
        // Chapter 1 was made before this run started.
        assert_eq!((a.kind, a.state, a.at, a.total), ("generate", "running", 1, 2));
        assert_eq!(a.line, "Ch. 2 · narrating");
        assert_eq!(a.cancel, vec!["p2", "p3"]);
        let names: Vec<&str> = a.steps.iter().map(|s| s.name).collect();
        assert_eq!(names, ["Read", "Transcript", "Story", "Audio", "Music", "Video"]);
        assert_eq!(a.steps[3].state, "running");
        assert_eq!(a.steps[0].state, "done");
        assert!((a.progress - 0.25).abs() < 1e-6, "{}", a.progress);
        let b = &card(&cards, "b").tasks[0];
        assert_eq!((b.state, b.line.as_str()), ("waiting", "Waiting for the chapters before it"));

        // Held queue.
        let held = Sources { queue: vec![item(4, "b", ItemState::Queued, 0)], held: true, interrupted: true, ..Default::default() };
        assert_eq!(build(&held, &mut Runs::new())[0].tasks[0].line, "Paused when MangaVox closed");
    }

    #[test]
    fn generate_shows_the_ai_limit_and_reading_progress() {
        let mut src = Sources { queue: vec![item(1, "a", ItemState::Running, 0)], ai: Some(ai("c1", false)), ..Default::default() };
        let t = build(&src, &mut Runs::new())[0].tasks[0].clone();
        assert_eq!(t.line, "Ch. 1 · reading the pages · 10/40 pages");
        assert!((t.progress - 0.25 / 6.0).abs() < 1e-6, "{}", t.progress);
        src.ai = Some(ai("c1", true));
        let t = build(&src, &mut Runs::new())[0].tasks[0].clone();
        assert_eq!((t.state, t.resume_at), ("limit", Some(1_900_000_000)));
        assert_eq!(t.error.as_deref(), Some("Usage limit reached"));
        // A run inside Generate is not shown twice.
        assert_eq!(build(&src, &mut Runs::new())[0].tasks.len(), 1);
    }

    #[test]
    fn finished_generate_runs_report_failures() {
        let mut runs = Runs::new();
        build(&Sources { queue: vec![item(1, "a", ItemState::Running, 2), item(2, "a", ItemState::Queued, 0)], ..Default::default() }, &mut runs);
        let after = Sources { queue: vec![item(1, "a", ItemState::Done, 6), item(2, "a", ItemState::Failed, 1)], ..Default::default() };
        let t = build(&after, &mut runs)[0].tasks[0].clone();
        assert_eq!((t.state, t.line.as_str(), t.error.as_deref()), ("failed", "1 of 2 chapters failed", Some("No AI model is picked")));
        assert!(t.cancel.is_empty());
        // Done chapters left from before this session start no run.
        assert!(build(&Sources { queue: vec![item(1, "a", ItemState::Done, 6)], ..Default::default() }, &mut Runs::new()).is_empty());
        // Cleared from the queue: the run goes.
        build(&Sources::default(), &mut runs);
        assert!(runs.is_empty());
    }

    #[test]
    fn standalone_ai_runs_jobs_exports_and_joins() {
        let mut src = Sources {
            ai: Some(ai("c9", false)),
            jobs: vec![job(7, "c8", JobView::Running), job(8, "c8", JobView::Queued), job(6, "c8", JobView::Cancelled)],
            exports: vec![ExportActivity { id: 1, manga_id: "m".into(), manga_title: "M".into(), progress: 0.5, label: "Rendering part 2".into() }],
            joins: vec![Join { id: "r1".into(), manga_id: "m".into(), title: "Solo".into(), state: "queued", progress: 0., line: "Waiting for the chapter videos".into(), error: None }],
            ..Default::default()
        };
        src.chapters.insert("c9".into(), "m".into());
        src.chapters.insert("c8".into(), "m".into());
        src.manga.insert("m".into(), ("Solo Leveling".into(), true));
        let cards = build(&src, &mut Runs::new());
        assert_eq!(cards.len(), 1);
        let c = &cards[0];
        assert_eq!((c.title.as_str(), c.cover.as_deref()), ("Solo Leveling", Some("/api/manga/m/cover")));
        let kinds: Vec<&str> = c.tasks.iter().map(|t| t.kind).collect();
        assert_eq!(kinds, ["voice", "read", "export", "export"]);
        let v = &c.tasks[0];
        assert_eq!((v.line.as_str(), v.at, v.total), ("Ch. 3 · narrating", 1, 2));
        assert_eq!(v.cancel, vec!["s7", "s8"]);
        assert_eq!(c.tasks[1].line, "Chapter 1 · reading page 11");
        assert_eq!(c.tasks[3].cancel, vec!["r1"]);
        assert_eq!(c.tasks[3].state, "waiting");
    }

    #[test]
    fn tracker_logs_transitions_and_lingers() {
        let t0 = Instant::now();
        let mut tr = Tracker::default();
        let mut d = dl("a");
        d.chapter = Some("Ch. 2".into());
        let a = tr.observe(build(&Sources { downloads: vec![d.clone()], ..Default::default() }, &mut Runs::new()), t0, 1000);
        assert_eq!((a.busy, a.events.len()), (1, 0));
        // It finishes: one event.
        (d.finished, d.done, d.chapter) = (true, 4, None);
        let a = tr.observe(build(&Sources { downloads: vec![d.clone()], ..Default::default() }, &mut Runs::new()), t0 + Duration::from_secs(1), 2000);
        assert_eq!(a.busy, 0);
        assert_eq!(a.events.len(), 1);
        let e = &a.events[0];
        assert_eq!((e.id, e.at, e.manga_id.as_str(), e.title.as_str(), e.state, e.kind), (1, 2000, "a", "A", "done", "download"));
        assert_eq!(e.text, "4 chapters saved for offline reading");
        // Its source drops it (the desktop card closed itself): still shown, no second event.
        let a = tr.observe(vec![], t0 + Duration::from_secs(60), 3000);
        assert_eq!(a.manga.len(), 1);
        assert_eq!(a.manga[0].tasks[0].state, "done");
        assert_eq!(a.events.len(), 1);
        // Until it is old.
        let a = tr.observe(vec![], t0 + LINGER + Duration::from_secs(2), 4000);
        assert!(a.manga.is_empty());
        assert_eq!(a.events.len(), 1, "the log keeps it");
        // Finished work seen for the first time (after a restart) logs nothing.
        let mut tr = Tracker::default();
        let a = tr.observe(build(&Sources { downloads: vec![d], ..Default::default() }, &mut Runs::new()), t0, 1);
        assert!(a.events.is_empty());
        assert_eq!(a.manga.len(), 1);
    }

    #[test]
    fn tracker_logs_ai_limits_and_sorts_cards() {
        let mut tr = Tracker::default();
        let t0 = Instant::now();
        let mut runs = Runs::new();
        let mut src = Sources { queue: vec![item(1, "z", ItemState::Running, 0)], downloads: vec![dl("b")], ai: Some(ai("c1", false)), ..Default::default() };
        let a = tr.observe(build(&src, &mut runs), t0, 1);
        // Running before waiting.
        assert_eq!(a.manga.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["z", "b"]);
        src.ai = Some(ai("c1", true));
        let a = tr.observe(build(&src, &mut runs), t0, 2);
        assert_eq!(a.events.len(), 1);
        assert_eq!((a.events[0].state, a.events[0].manga_id.as_str()), ("limit", "z"));
        assert_eq!(a.manga[0].state, "limit");
        // Still waiting: no new event.
        assert_eq!(tr.observe(build(&src, &mut runs), t0, 3).events.len(), 1);
    }

    #[test]
    fn studio_chains_show_downloading_then_sent() {
        use crate::studio_chain::Chain;
        let mut c = Chain { manga_id: "m".into(), title: "Solo".into(), waiting: vec!["c2".into(), "c3".into()], sent: 1, failed: 0, ended: None };
        let cards = build(&Sources { chains: vec![c.clone()], ..Default::default() }, &mut Runs::new());
        let t = &cards[0].tasks[0];
        assert_eq!((t.kind, t.label, t.state, t.at, t.total), ("studio", "Make in Studio", "running", 1, 3));
        assert_eq!(t.line, "Downloading 2 chapters, then Generate");
        (c.waiting, c.sent, c.failed) = (vec![], 2, 1);
        let t = build(&Sources { chains: vec![c.clone()], ..Default::default() }, &mut Runs::new())[0].tasks[0].clone();
        assert_eq!((t.state, t.line.as_str()), ("failed", "2 of 3 chapters sent to Generate"));
        c.failed = 0;
        let t = build(&Sources { chains: vec![c], ..Default::default() }, &mut Runs::new())[0].tasks[0].clone();
        assert_eq!((t.state, t.line.as_str()), ("done", "2 chapters sent to Generate"));
    }

    #[test]
    fn json_shape() {
        let mut tr = Tracker::default();
        let mut d = dl("a b");
        d.chapter = Some("Ch. 2".into());
        let src = Sources { downloads: vec![d], queue: vec![item(1, "a b", ItemState::Running, 4)], ..Default::default() };
        let a = tr.observe(build(&src, &mut Runs::new()), Instant::now(), 42);
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["now"], 42);
        assert_eq!(v["busy"], 1);
        assert!(v["events"].as_array().unwrap().is_empty());
        let m = &v["manga"][0];
        for k in ["id", "title", "cover", "state", "tasks"] {
            assert!(m.get(k).is_some(), "card has {k}");
        }
        assert_eq!(m["id"], "a b");
        let t = &m["tasks"][1];
        for k in ["key", "kind", "label", "state", "line", "progress", "at", "total", "steps", "resume_at", "error", "cancel"] {
            assert!(t.get(k).is_some(), "task has {k}");
        }
        assert_eq!(t["kind"], "generate");
        assert_eq!(t["steps"][4]["name"], "Music");
        assert_eq!(t["steps"][4]["state"], "running");
        assert_eq!(m["tasks"][0]["kind"], "download");
        assert_eq!(pct("a b"), "a%20b");
        assert_eq!(short_chapter("Oneshot"), "Oneshot");
    }
}
