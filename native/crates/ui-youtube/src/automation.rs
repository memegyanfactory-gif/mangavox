//! The automation runner: one app-wide entity that moves every job of `mv_youtube::automation`
//! through its stages, in the background, while the app is open. Each tick (every 15 s) works out
//! where a job stands from what is on disk (downloaded chapters, transcripts, stories, videos, the
//! upload queue), so nothing is lost when the app closes: on the next start the job carries on.
//!
//! The stages use the app's own pieces: the trends crawl (Discover), the manga download queue
//! (Download), Studio's Generate pipeline (Read, then Write and Render), Studio's full-recap join
//! and thumbnail, the release-time picker, the YouTube upload queue (private + `publishAt`, so
//! YouTube publishes with the PC off) and the channel's analytics and comments (Collect).
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::*;
use mv_sources::MangaSearchResult;
use mv_ui::bg;
use mv_ui::toast::{self, Toast};
use mv_ui::{ph, Route};
use mv_youtube::automation::{self as auto, Automation, Job, JobState, LogKind, Pick, Results, SlotInputs, Stage};
use mv_youtube::{UploadItem, UploadState};

use crate::hub::{self, now};

const TICK: Duration = Duration::from_secs(15);
/// Discover's picks older than this are fetched again by full auto.
const PICKS_STALE: i64 = 12 * 3600;
/// Results are read this long after release, then the job is done.
const COLLECT_FOR: i64 = 7 * 86_400;

pub struct Runner {
    pub state: Automation,
    store: mv_youtube::store::Store,
    pub discovering: bool,
    pub discover_step: Arc<Mutex<String>>,
    /// Jobs with background work in flight (one at a time per job).
    busy: HashSet<String>,
    /// Progress of a full-recap join: (fraction, line).
    pub join_progress: Arc<Mutex<(f32, String)>>,
    /// Chapter numbers of a job's chapters (read off the UI thread, for the title and badge).
    numbers: HashMap<String, Vec<f64>>,
    /// Download attempts per chapter id (a chapter that keeps failing stops its job).
    dl_attempts: HashMap<String, u32>,
    /// Chapters already sent to the download queue in this run of the app.
    dl_sent: HashSet<String>,
    /// When full auto last ran Discover, and how many runs in a row found nothing.
    discover_tried: i64,
    discover_misses: u32,
}

/// Download attempts before a chapter that keeps failing stops its job.
const DL_TRIES: u32 = 3;
/// Full auto skips a manga whose job failed this recently.
const FAILED_COOLDOWN: i64 = 7 * 86_400;

struct RunnerGlobal(Entity<Runner>);
impl Global for RunnerGlobal {}

pub fn runner(cx: &mut App) -> Entity<Runner> {
    if let Some(g) = cx.try_global::<RunnerGlobal>() {
        return g.0.clone();
    }
    let store = mv_youtube::store::Store::default();
    let state: Automation = store.load_json("automation.json");
    let e = cx.new(|_| Runner {
        state,
        store,
        discovering: false,
        discover_step: Arc::default(),
        busy: HashSet::new(),
        join_progress: Arc::default(),
        numbers: HashMap::new(),
        dl_attempts: HashMap::new(),
        dl_sent: HashSet::new(),
        discover_tried: 0,
        discover_misses: 0,
    });
    cx.set_global(RunnerGlobal(e.clone()));
    e
}

/// Starts the ticks (once, at app start). Jobs that were running carry on from where they were.
pub fn start(cx: &mut App) {
    let r = runner(cx);
    let resumed = r.read(cx).state.jobs.iter().filter(|j| j.state == JobState::Running).count();
    if resumed > 0 {
        r.update(cx, |r, _| {
            let n = now();
            r.state.log(n, LogKind::Info, None, format!("Picked up {resumed} job{} after the app started", if resumed == 1 { "" } else { "s" }));
            r.save();
        });
    }
    cx.spawn(async move |cx| loop {
        cx.background_executor().timer(TICK).await;
        r.update(cx, |r, cx| r.tick(cx));
    })
    .detach();
}

/// Viewers' requests from the channel's comments, kept for Discover's ranking.
pub fn learn_from_comments(threads: &[mv_youtube::comments::Thread], cx: &mut App) {
    let f = mv_youtube::comments::feedback(threads);
    if f.requests.is_empty() {
        return;
    }
    runner(cx).update(cx, |r, cx| {
        // The latest read replaces the counts (the same comments are read again and again).
        for (k, v) in f.requests {
            r.state.requests.insert(k, v);
        }
        r.save();
        cx.notify();
    });
}

fn job_id() -> String {
    auto::new_job_id(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0))
}

impl Runner {
    pub fn save(&self) {
        let _ = self.store.save_json("automation.json", &self.state);
    }

    /// Jobs waiting for the user's approval.
    pub fn waiting(&self) -> usize {
        self.state.jobs.iter().filter(|j| j.state == JobState::Waiting).count()
    }

    fn log(&mut self, kind: LogKind, job: Option<&str>, text: impl Into<String>) {
        let text = text.into();
        self.state.log(now(), kind, job, text);
    }

    fn update_job(&mut self, id: &str, cx: &mut Context<Self>, f: impl FnOnce(&mut Job)) {
        if let Some(j) = self.state.job_mut(id) {
            f(j);
            j.updated_at = now();
        }
        self.save();
        cx.notify();
    }

    fn fail(&mut self, id: &str, what: String, cx: &mut Context<Self>) {
        let title = self.state.job(id).map(|j| j.pick.title.clone()).unwrap_or_default();
        self.update_job(id, cx, |j| {
            j.state = JobState::Failed;
            j.error = Some(what.clone());
        });
        self.log(LogKind::Error, Some(id), format!("{title}: {what}"));
        self.save();
        toast::show(cx, Toast::error(format!("Automation stopped on {title}: {what}")).action("Open", |w, cx| mv_ui::navigate(Route::Automation, w, cx)));
    }

    // ------------------------------------------------------------ controls

    pub fn set_full_auto(&mut self, on: bool, cx: &mut Context<Self>) {
        self.state.full_auto = on;
        self.log(if on { LogKind::Success } else { LogKind::Info }, None, if on { "Full auto turned on: discovering, making, scheduling and publishing without asking" } else { "Full auto turned off: renders wait for your approval" });
        self.save();
        cx.notify();
        if on {
            // Renders that were waiting for approval go on too (full auto does not ask).
            for j in self.state.jobs.iter_mut().filter(|j| j.state == JobState::Waiting && j.stage == Stage::Approve) {
                j.state = JobState::Running;
            }
            self.save();
            self.tick(cx);
        }
    }

    pub fn set_options(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Automation)) {
        f(&mut self.state);
        self.save();
        cx.notify();
    }

    /// Reads YouTube now (public pages, no quota): which manga recaps viewers love, ranked with
    /// what the channel's own commenters ask for.
    pub fn discover(&mut self, cx: &mut Context<Self>) {
        if self.discovering {
            return;
        }
        self.discovering = true;
        self.discover_tried = now();
        let step = self.discover_step.clone();
        *step.lock().unwrap_or_else(|e| e.into_inner()) = "Searching YouTube for recaps".into();
        self.log(LogKind::Info, None, "Discover: reading what manga recaps people watch right now");
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = bg::run(async move { mv_youtube::trends::crawl(move |m| *step.lock().unwrap_or_else(|e| e.into_inner()) = m).await }).await;
            let _ = this.update(cx, |r, cx| {
                r.discovering = false;
                match r_result(res) {
                    Ok(t) => {
                        let picks: Vec<Pick> = t
                            .manga
                            .iter()
                            .filter(|m| !m.mangadex_id.is_empty())
                            .map(|m| {
                                let best = m.videos.first().and_then(|id| t.video(id));
                                let views: u64 = m.videos.iter().filter_map(|id| t.video(id)).map(|v| v.views).sum();
                                Pick {
                                    mangadex_id: m.mangadex_id.clone(),
                                    title: m.name.clone(),
                                    cover: m.cover.clone(),
                                    score: m.score,
                                    reason: format!("{} recap views, {:.0}/day demand, {} channel{} on it, {:.0}% of it this week", hub::fmt_num(views), m.demand, m.channels, if m.channels == 1 { "" } else { "s" }, m.growth * 100.),
                                    recap_views: views,
                                    velocity: best.map(|v| v.velocity()).unwrap_or(0.),
                                    channels: m.channels,
                                    asked: 0,
                                    example: best.map(|v| (v.title.clone(), v.url())),
                                }
                            })
                            .collect();
                        let ranked = auto::rank_picks(picks, &r.state.requests, &r.state);
                        let top = ranked.first().map(|p| p.title.clone()).unwrap_or_default();
                        r.state.picks = ranked;
                        r.state.picks_at = now();
                        let n = r.state.picks.len();
                        // Picks full auto cannot use (in production, failed lately) do not count:
                        // otherwise it would crawl again every 30 minutes for the same list.
                        let usable = r.state.picks.iter().filter(|p| r.usable(&p.mangadex_id)).count();
                        r.discover_misses = if usable == 0 { r.discover_misses + 1 } else { 0 };
                        r.log(LogKind::Success, None, format!("Discover: {n} manga ranked{}", if top.is_empty() { String::new() } else { format!(", best now: {top}") }));
                    }
                    Err(e) => {
                        r.discover_misses += 1;
                        r.log(LogKind::Error, None, format!("Discover failed: {e}"));
                    }
                }
                r.save();
                cx.notify();
            });
        })
        .detach();
        // Repaint the step line while it runs.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(500)).await;
            let alive = this.update(cx, |r, cx| {
                cx.notify();
                r.discovering
            });
            if !matches!(alive, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// Starts making a video of `pick`.
    pub fn start_job(&mut self, pick: Pick, by_auto: bool, cx: &mut Context<Self>) {
        if self.state.has_manga(&pick.mangadex_id) {
            // Off the list, so it is not offered (or picked by full auto) again.
            self.state.picks.retain(|p| p.mangadex_id != pick.mangadex_id);
            self.save();
            cx.notify();
            if !by_auto {
                toast::show(cx, Toast::info(format!("{} is already in production", pick.title)));
            }
            return;
        }
        let id = job_id();
        let title = pick.title.clone();
        self.state.jobs.push(Job { id: id.clone(), pick, stage: Stage::Download, created_at: now(), updated_at: now(), auto: by_auto, step: "Importing the manga".into(), ..Default::default() });
        self.state.picks.retain(|p| p.title != title);
        self.log(LogKind::Info, Some(&id), format!("Started a recap of {title}{}", if by_auto { " (full auto)" } else { "" }));
        self.save();
        cx.notify();
        self.advance(&id, cx);
    }

    pub fn approve(&mut self, id: &str, cx: &mut Context<Self>) {
        let title = self.state.job(id).map(|j| j.pick.title.clone()).unwrap_or_default();
        self.update_job(id, cx, |j| {
            j.approved = true;
            j.state = JobState::Running;
            j.stage = Stage::Schedule;
            j.step = "Picking the best release time".into();
        });
        self.log(LogKind::Success, Some(id), format!("You approved the recap of {title}"));
        self.advance(id, cx);
    }

    pub fn pause(&mut self, id: &str, cx: &mut Context<Self>) {
        self.update_job(id, cx, |j| j.state = JobState::Paused);
        self.log(LogKind::Info, Some(id), "Paused");
    }

    /// Resumes a paused or failed job at the stage it was on.
    pub fn resume(&mut self, id: &str, cx: &mut Context<Self>) {
        // Studio work is queued again (that replaces chapters Generate gave up on); the arc of a
        // job past Read is not written twice. Downloads get their tries back.
        let chapters = self.state.job(id).map(|j| j.chapters.clone()).unwrap_or_default();
        for c in &chapters {
            self.dl_attempts.remove(c);
            self.dl_sent.remove(c);
        }
        // An upload that failed (a network blip) is tried again rather than failing the job anew.
        if let Some(upload) = self.state.job(id).filter(|j| j.stage == Stage::Publish).and_then(|j| j.upload_id.clone()) {
            hub::hub(cx).update(cx, |h, cx| {
                h.update_item(&upload, cx, |i| {
                    if i.state == UploadState::Failed {
                        i.state = UploadState::Planned;
                        i.error = None;
                    }
                })
            });
        }
        self.update_job(id, cx, |j| {
            j.state = JobState::Running;
            j.error = None;
            j.queued_for = if matches!(j.stage, Stage::Write | Stage::Render) { Some(Stage::Read) } else { None };
        });
        self.log(LogKind::Info, Some(id), "Resumed");
        self.advance(id, cx);
    }

    pub fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        self.state.jobs.retain(|j| j.id != id);
        self.save();
        cx.notify();
    }

    // ------------------------------------------------------------ the loop

    /// Full auto may start this manga: not in production, and its job did not fail lately (it
    /// would fail the same way).
    fn usable(&self, mangadex_id: &str) -> bool {
        let t = now();
        !self.state.has_manga(mangadex_id) && !self.state.jobs.iter().any(|j| j.state == JobState::Failed && j.pick.mangadex_id == mangadex_id && t - j.updated_at < FAILED_COOLDOWN)
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.state.jobs.iter().filter(|j| j.state == JobState::Running).map(|j| j.id.clone()).collect();
        for id in ids {
            self.advance(&id, cx);
        }
        if !self.state.full_auto {
            return;
        }
        // Full auto: keep the pipeline fed, at most a few releases ahead.
        let ahead = self.state.jobs.iter().filter(|j| j.state != JobState::Failed && j.publish_at.is_some_and(|t| t > now())).count();
        if !self.state.room_for_more() || ahead >= 3 {
            return;
        }
        let t = now();
        let pick = self.state.picks.iter().find(|p| self.usable(&p.mangadex_id)).cloned();
        if pick.is_none() || t - self.state.picks_at > PICKS_STALE {
            // Not back to back: 30 min after a run, longer after runs that found nothing.
            let wait = (30 * 60_i64 << self.discover_misses.min(4)).min(8 * 3600);
            if t - self.discover_tried >= wait {
                self.discover(cx);
            }
            return;
        }
        if let Some(p) = pick {
            self.log(LogKind::Info, None, format!("Full auto picked {} ({}/100): {}", p.title, p.score, p.reason));
            self.start_job(p, true, cx);
        }
    }

    /// Moves one job on as far as it can go now (background work runs off the UI thread).
    fn advance(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.busy.contains(id) {
            return;
        }
        let Some(job) = self.state.job(id).cloned() else { return };
        if job.state != JobState::Running {
            return;
        }
        match job.stage {
            Stage::Discover | Stage::Download => self.download(job, cx),
            Stage::Read | Stage::Write | Stage::Render => self.produce(job, cx),
            Stage::Approve => {
                if self.state.full_auto || !self.state.approve_renders || job.approved {
                    let id = job.id.clone();
                    self.update_job(&id, cx, |j| {
                        j.approved = true;
                        j.stage = Stage::Schedule;
                    });
                    self.log(LogKind::Success, Some(&id), format!("{}: render approved automatically", job.pick.title));
                    self.advance(&id, cx);
                } else {
                    let first = job.step != "Waiting for your approval";
                    self.update_job(&job.id, cx, |j| {
                        j.state = JobState::Waiting;
                        j.step = "Waiting for your approval".into();
                    });
                    if first {
                        self.log(LogKind::Warning, Some(&job.id), format!("{} is rendered and waits for your approval", job.pick.title));
                        toast::show(cx, Toast::info(format!("The recap of {} is ready to check", job.pick.title)).icon(ph::FILM_SLATE).action("Review", |w, cx| mv_ui::navigate(Route::Automation, w, cx)));
                    }
                }
            }
            Stage::Schedule => self.schedule(job, cx),
            Stage::Publish => self.publish(job, cx),
            Stage::Collect => self.collect(job, cx),
            Stage::Done => {}
        }
    }

    fn run_bg<T: Send + 'static>(&mut self, id: &str, cx: &mut Context<Self>, work: impl FnOnce() -> anyhow::Result<T> + Send + 'static, then: impl FnOnce(&mut Self, &str, T, &mut Context<Self>) + 'static) {
        self.busy.insert(id.to_string());
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let r = bg::blocking(work).await;
            let _ = this.update(cx, |me, cx| {
                me.busy.remove(&id);
                match r_result(r) {
                    Ok(v) => then(me, &id, v, cx),
                    Err(e) => me.fail(&id, e, cx),
                }
            });
        })
        .detach();
    }

    // ------------------------------------------------------------ Download

    fn download(&mut self, job: Job, cx: &mut Context<Self>) {
        let id = job.id.clone();
        if job.manga_id.is_none() {
            // Import from MangaDex: details and the chapter list into the library.
            self.busy.insert(id.clone());
            let pick = job.pick.clone();
            cx.spawn(async move |this, cx| {
                let r = bg::run(async move {
                    let r = MangaSearchResult { source_id: pick.mangadex_id.clone(), title: pick.title.clone(), cover_url: Some(pick.cover.clone()).filter(|c| !c.is_empty()), ..Default::default() };
                    ui_manga::ops::prepare_from_source(mv_sources::mangadex::ID.to_string(), r).await
                })
                .await;
                let _ = this.update(cx, |me, cx| {
                    me.busy.remove(&id);
                    match r_result(r) {
                        Ok(m) => {
                            let (mid, title) = (m.id.clone(), m.title.clone());
                            me.update_job(&id, cx, |j| {
                                j.manga_id = Some(mid);
                                j.stage = Stage::Download;
                                j.step = "Listing chapters".into();
                            });
                            me.log(LogKind::Info, Some(&id), format!("Imported {title} from MangaDex"));
                            me.advance(&id, cx);
                        }
                        Err(e) => me.fail(&id, format!("Could not import it from MangaDex: {e}"), cx),
                    }
                });
            })
            .detach();
            return;
        }
        let mid = job.manga_id.clone().unwrap_or_default();
        let (max, chapters) = (self.state.max_chapters, job.chapters.clone());
        self.run_bg(
            &id,
            cx,
            move || {
                let lang = ui_manga::backend::preferred_language();
                let mut list = ui_manga::backend::with_db(|c| ui_manga::store::chapters(c, &mid, &lang))?;
                list.sort_by(|a, b| a.number.unwrap_or(f64::MAX).total_cmp(&b.number.unwrap_or(f64::MAX)));
                // One chapter per number (sources list the same chapter from several groups).
                let mut seen: Vec<f64> = vec![];
                list.retain(|c| match c.number {
                    Some(n) if seen.iter().any(|s| (s - n).abs() < 1e-6) => false,
                    Some(n) => {
                        seen.push(n);
                        true
                    }
                    None => true,
                });
                let picked: Vec<_> = if chapters.is_empty() { auto::pick_chapters(&list, max) } else { list.into_iter().filter(|c| chapters.contains(&c.id)).collect() };
                Ok(picked)
            },
            move |me, id, picked, cx| {
                if picked.is_empty() {
                    me.fail(id, "MangaDex lists no chapters in your language for it".into(), cx);
                    return;
                }
                let done = picked.iter().filter(|c| c.download_status == "complete").count();
                let total = picked.len();
                let first_time = me.state.job(id).is_some_and(|j| j.chapters.is_empty());
                let (mid, title) = (me.state.job(id).and_then(|j| j.manga_id.clone()).unwrap_or_default(), me.state.job(id).map(|j| j.pick.title.clone()).unwrap_or_default());
                let missing: Vec<(String, String, String, String)> = picked.iter().filter(|c| c.download_status != "complete").map(|c| (mid.clone(), title.clone(), c.id.clone(), ui_manga::logic::chapter_short(c))).collect();
                me.update_job(id, cx, |j| {
                    j.chapters = picked.iter().map(|c| c.id.clone()).collect();
                    j.done = done;
                    j.step = format!("Downloading chapters ({done}/{total})");
                });
                if first_time {
                    me.log(LogKind::Info, Some(id), format!("{title}: downloading all {total} chapters first"));
                }
                if done == total {
                    me.update_job(id, cx, |j| {
                        j.stage = Stage::Read;
                        j.done = 0;
                        j.step = "Reading every chapter".into();
                    });
                    me.log(LogKind::Success, Some(id), format!("{title}: all {total} chapters downloaded"));
                    me.advance(id, cx);
                } else if !missing.is_empty() {
                    // Each chapter goes to the download queue once; one that failed is tried again
                    // up to DL_TRIES times, and never while the user has the queue paused.
                    let dl = ui_manga::downloads(cx);
                    let (paused, mut send, mut stuck) = (dl.read(cx).is_paused(), vec![], None);
                    for m in missing {
                        let d = dl.read(cx);
                        if d.status(&m.2).is_some() {
                            continue;
                        }
                        if !me.dl_sent.contains(&m.2) {
                            if !paused {
                                me.dl_sent.insert(m.2.clone());
                                send.push(m);
                            }
                        } else if let Some(err) = d.error(&m.2).map(str::to_string) {
                            let n = me.dl_attempts.entry(m.2.clone()).or_insert(1);
                            if *n >= DL_TRIES {
                                stuck = Some((m.3.clone(), err));
                                break;
                            }
                            if !paused {
                                *n += 1;
                                send.push(m);
                            }
                        }
                    }
                    if let Some((label, err)) = stuck {
                        me.fail(id, format!("{label} would not download after {DL_TRIES} tries: {err}"), cx);
                        return;
                    }
                    if paused && send.is_empty() {
                        me.update_job(id, cx, |j| j.step = format!("Downloads are paused ({done}/{total})"));
                    }
                    if !send.is_empty() {
                        dl.update(cx, |d, cx| d.enqueue_chapters(send, cx));
                    }
                }
            },
        );
    }

    // ------------------------------------------------------------ Read, Write, Render

    /// The job's chapters as Studio sees them, in reading order.
    fn studio_items(job: &Job) -> anyhow::Result<Vec<ui_studio::data::ChapterItem>> {
        let groups = ui_studio::data::load()?;
        let mid = job.manga_id.clone().unwrap_or_default();
        let mut items: Vec<_> = groups.into_iter().filter(|g| g.id == mid).flat_map(|g| g.chapters).filter(|c| job.chapters.contains(&c.id)).collect();
        items.sort_by(|a, b| a.number.unwrap_or(f64::MAX).total_cmp(&b.number.unwrap_or(f64::MAX)));
        Ok(items)
    }

    fn produce(&mut self, job: Job, cx: &mut Context<Self>) {
        let id = job.id.clone();
        ensure_pipeline_context(cx);
        let j = job.clone();
        self.run_bg(
            &id,
            cx,
            move || {
                let items = Runner::studio_items(&j)?;
                let facts: Vec<mv_recap::PhaseStatus> = items.iter().map(|c| ui_studio::pipeline::Facts::load(&ui_studio::pipeline::QChapter::from_item(c).chref()).status).collect();
                Ok((items, facts))
            },
            move |me, id, (items, facts), cx| {
                let Some(job) = me.state.job(id).cloned() else { return };
                let total = job.chapters.len();
                if items.len() < total {
                    // Studio lists a chapter once its pages are on disk.
                    me.update_job(id, cx, |j| j.step = format!("Waiting for Studio to see the chapters ({}/{total})", items.len()));
                    return;
                }
                use ui_studio::pipeline::ItemState;
                let q = ui_studio::pipeline::snapshot();
                let mine: Vec<_> = q.items.iter().filter(|i| job.chapters.contains(&i.chapter.id)).collect();
                // This stage's Studio work was queued by the job itself (before that, failed or
                // held items in Studio's queue are old ones, replaced when the job queues its own).
                let ours = match job.stage {
                    Stage::Read => job.queued_for == Some(Stage::Read),
                    _ => job.queued_for == Some(Stage::Write),
                };
                // A chapter Generate gave up on stops the job (Retry queues it again).
                if let Some(bad) = mine.iter().find(|i| i.state == ItemState::Failed).filter(|_| ours) {
                    let err = bad.error.clone().unwrap_or_else(|| "a step failed".into());
                    me.fail(id, format!("{}: {err}", bad.chapter.label()), cx);
                    return;
                }
                // After a restart Studio holds its queue until Resume is pressed; the job's
                // chapters are waiting in it, so the automation carries on.
                if ours && q.paused && q.interrupted && mine.iter().any(|i| i.state.active()) {
                    ui_studio::pipeline::resume_all();
                    me.log(LogKind::Info, Some(id), format!("{}: Studio's queue was held after the restart; resumed it", job.pick.title));
                }
                // Nothing of the job left in Studio's queue (cleared, or finished short): queue again.
                let requeue = !ours || !mine.iter().any(|i| i.state.active());
                let read = facts.iter().filter(|f| f.transcript).count();
                let written = facts.iter().filter(|f| f.story).count();
                let rendered = facts.iter().filter(|f| f.video.is_some()).count();
                match job.stage {
                    Stage::Read => {
                        if read == total {
                            me.update_job(id, cx, |j| {
                                j.stage = Stage::Write;
                                j.done = 0;
                                j.queued_for = None;
                                j.step = "Writing the whole story".into();
                            });
                            me.log(LogKind::Success, Some(id), format!("{}: read all {total} chapters", job.pick.title));
                            me.advance(id, cx);
                            return;
                        }
                        if requeue {
                            let todo: Vec<_> = items.iter().zip(&facts).filter(|(_, f)| !f.transcript).map(|(c, _)| c.clone()).collect();
                            ui_studio::pipeline::enqueue(&todo, mv_recap::Step::Transcript, None, mv_recap::FORMAT_LANDSCAPE, None, true);
                            me.update_job(id, cx, |j| j.queued_for = Some(Stage::Read));
                        }
                        me.update_job(id, cx, |j| {
                            j.done = read;
                            j.step = format!("Reading chapters ({read}/{total})");
                        });
                    }
                    Stage::Write => {
                        // First the whole arc in one pass (every chapter known), then each chapter's narration.
                        if job.queued_for.is_none() {
                            me.write_arc(job.clone(), items.clone(), cx);
                            return;
                        }
                        if requeue && written < total {
                            me.queue_rest(id, &job.pick.title, &items, cx);
                        }
                        if written == total {
                            me.update_job(id, cx, |j| {
                                j.stage = Stage::Render;
                                j.done = rendered;
                                j.step = "Voicing, editing and rendering".into();
                            });
                            me.log(LogKind::Success, Some(id), format!("{}: the story of all {total} chapters is written", job.pick.title));
                        } else {
                            me.update_job(id, cx, |j| {
                                j.done = written;
                                j.step = format!("Writing chapter stories ({written}/{total})");
                            });
                        }
                    }
                    Stage::Render => {
                        if rendered < total {
                            if requeue {
                                me.queue_rest(id, &job.pick.title, &items, cx);
                            }
                            me.update_job(id, cx, |j| {
                                j.done = rendered;
                                j.step = format!("Rendering chapters ({rendered}/{total})");
                            });
                            return;
                        }
                        if job.file.as_ref().is_some_and(|f| f.is_file()) {
                            me.update_job(id, cx, |j| {
                                j.stage = Stage::Approve;
                                j.done = total;
                            });
                            me.advance(id, cx);
                            return;
                        }
                        let files: Vec<PathBuf> = facts.iter().filter_map(|f| f.video.clone()).collect();
                        me.join(job.clone(), files, cx);
                    }
                    _ => {}
                }
            },
        );
    }

    /// Queues Story → Video of the job's chapters with the saved Generate answers (chapters made
    /// already are skipped by Studio).
    /// Off the UI thread: it reads and writes every chapter's voice, music and caption settings.
    fn queue_rest(&mut self, id: &str, title: &str, items: &[ui_studio::data::ChapterItem], cx: &mut Context<Self>) {
        let (items, title) = (items.to_vec(), title.to_string());
        self.run_bg(
            id,
            cx,
            move || Ok(ui_studio::queue_saved(&items, true)),
            move |me, id, ((n, w, _), errors), cx| {
                if !errors.is_empty() {
                    me.log(LogKind::Warning, Some(id), errors.join("; "));
                }
                if n + w > 0 {
                    me.log(LogKind::Info, Some(id), format!("{title}: Studio is writing, voicing and rendering {} chapters", n + w));
                }
                me.update_job(id, cx, |j| j.queued_for = Some(Stage::Write));
            },
        );
    }

    /// The whole story in one pass: every chapter's transcript to the model at once, back an arc
    /// (who is who, what each chapter does for the whole, what pays off later). It goes into the
    /// manga's brain, which every chapter's narration reads, so each chapter is written knowing
    /// the whole story; then Studio writes, voices and renders the chapters.
    fn write_arc(&mut self, job: Job, items: Vec<ui_studio::data::ChapterItem>, cx: &mut Context<Self>) {
        let id = job.id.clone();
        let (provider, model) = ui_assistant::assistant_model(cx);
        let provider: Option<mv_ai::AiProviderId> = serde_json::from_value(serde_json::Value::String(provider)).ok();
        let registry = ui_assistant::registry(cx);
        self.update_job(&id, cx, |j| j.step = "Writing the whole story in one pass".into());
        self.busy.insert(id.clone());
        let title = job.pick.title.clone();
        let mid = job.manga_id.clone().unwrap_or_default();
        cx.spawn(async move |this, cx| {
            let r = match provider.filter(|_| !model.is_empty()) {
                Some(p) => bg::run(async move { crate::arc::write(registry, p, model, &mid, &title, &items).await }).await,
                None => Ok(Err(anyhow::anyhow!("no model picked"))),
            };
            let _ = this.update(cx, |me, cx| {
                me.busy.remove(&id);
                match r_result(r) {
                    Ok(chars) => me.log(LogKind::Success, Some(&id), format!("{}: wrote the whole arc ({chars} characters) before the chapters", job.pick.title)),
                    // The chapters still get written (with the story so far); the arc is a bonus.
                    Err(e) => me.log(LogKind::Warning, Some(&id), format!("{}: the one-pass arc was skipped ({e}); chapters are written in order with the story so far", job.pick.title)),
                }
                me.update_job(&id, cx, |j| j.queued_for = Some(Stage::Read));
                me.advance(&id, cx);
            });
        })
        .detach();
    }

    /// Joins the chapter videos into one recap and draws its thumbnail.
    fn join(&mut self, job: Job, files: Vec<PathBuf>, cx: &mut Context<Self>) {
        let id = job.id.clone();
        let mid = job.manga_id.clone().unwrap_or_default();
        let title = job.pick.title.clone();
        let progress = self.join_progress.clone();
        *progress.lock().unwrap_or_else(|e| e.into_inner()) = (0., "Joining the chapters".into());
        self.update_job(&id, cx, |j| j.step = format!("Joining {} chapters into one video", files.len()));
        self.run_bg(
            &id,
            cx,
            move || {
                let numbers: Vec<Option<f64>> = Runner::studio_items(&job).map(|v| v.iter().map(|c| c.number).collect()).unwrap_or_default();
                let file = ui_studio::join_full_recap(&mid, &files, progress, Arc::new(AtomicBool::new(false)))?;
                let dir = mv_recap::media::manga_dir(&mid);
                let label = mv_recap::thumb::badge_label(&numbers);
                let thumb = mv_recap::thumb::full_thumbnail(&dir, None, &title, &label).ok();
                let secs = ffprobe_secs(&file);
                Ok((file, thumb, secs))
            },
            |me, id, (file, thumb, secs), cx| {
                let t = me.state.job(id).map(|j| j.pick.title.clone()).unwrap_or_default();
                me.update_job(id, cx, |j| {
                    j.file = Some(file);
                    j.thumbnail = thumb;
                    j.duration_secs = secs;
                    j.stage = Stage::Approve;
                    j.step = "Rendered".into();
                });
                me.log(LogKind::Success, Some(id), format!("{t}: the full recap is rendered ({})", crate::yt::duration(secs as u64)));
                me.advance(id, cx);
            },
        );
    }

    // ------------------------------------------------------------ Schedule, Publish, Collect

    fn schedule(&mut self, job: Job, cx: &mut Context<Self>) {
        // The chapter numbers (for the title) come from Studio's library: read them off the UI thread first.
        let Some(chapters) = self.numbers.get(&job.id).cloned() else {
            let j = job.clone();
            self.run_bg(
                &job.id,
                cx,
                move || Ok(Runner::studio_items(&j).map(|v| v.iter().filter_map(|c| c.number).collect::<Vec<f64>>()).unwrap_or_default()),
                |me, id, nums, cx| {
                    me.numbers.insert(id.to_string(), nums);
                    me.advance(id, cx);
                },
            );
            return;
        };
        let h = hub::hub(cx);
        let (choice, item) = {
            let h = h.read(cx);
            let a = h.timing_analysis();
            let golden: Vec<(usize, usize, f64)> = if a.by_habit { vec![] } else { a.golden.iter().map(|g| (g.start_hour, g.hours, g.cell.lift())).collect() };
            let mut taken: Vec<i64> = h.queue.iter().filter(|i| i.state != UploadState::Failed).filter_map(|i| i.publish_at).collect();
            taken.extend(self.state.taken());
            let inp = SlotInputs {
                slots: self.state.slots.clone(),
                golden,
                own: Some(auto::own_hour_lift(&h.videos, now(), hub::utc_offset())),
                taken,
                spacing_hours: (self.state.cadence_days.max(1) as i64 * 24 - 4).max(12),
                min_lead_hours: 3,
                horizon_days: 7 + self.state.cadence_days as i64 * 3,
            };
            // In the local zone day by day, so a DST change inside the horizon keeps the wall-clock time.
            let choice = auto::best_slot_tz(now(), &chrono::Local, &inp);
            let (lo, hi) = (chapters.iter().copied().fold(f64::INFINITY, f64::min), chapters.iter().copied().fold(f64::NEG_INFINITY, f64::max));
            let range = if chapters.is_empty() { String::new() } else if lo < hi { format!(" | Chapters {}-{}", fmt_ch(lo), fmt_ch(hi)) } else { format!(" | Chapter {}", fmt_ch(lo)) };
            let item = UploadItem {
                id: mv_youtube::store::new_id(),
                file: job.file.clone().unwrap_or_default(),
                chapter_id: Some(format!("full:{}", job.manga_id.clone().unwrap_or_default())),
                manga_id: job.manga_id.clone(),
                manga_title: Some(job.pick.title.clone()),
                title: format!("{} Recap{range} Full Story", job.pick.title),
                description: format!("The full story of {}, retold from the manga.\n\nSubscribe so you don't miss the next recap.\n\n#{} #manhwa #recap", job.pick.title, job.pick.title.split_whitespace().collect::<String>().to_lowercase()),
                tags: vec![job.pick.title.clone(), format!("{} recap", job.pick.title), "manga recap".into(), "manhwa recap".into()],
                created_at: now(),
                publish_public: true,
                thumbnail: job.thumbnail.clone(),
                ..Default::default()
            };
            (choice, item)
        };
        let Some(choice) = choice else {
            self.update_job(&job.id, cx, |j| j.step = "No free release time in the next days".into());
            return;
        };
        if job.file.as_ref().is_none_or(|f| !f.is_file()) {
            self.fail(&job.id, "The rendered recap is missing on disk".into(), cx);
            return;
        }
        let why = if choice.reasons.is_empty() { "the next free slot".to_string() } else { choice.reasons.join(", ") };
        let mut item = item;
        item.publish_at = Some(choice.at);
        let upload_id = item.id.clone();
        h.update(cx, |h, cx| h.push_item(item, cx));
        let when = crate::when::when(choice.at);
        self.update_job(&job.id, cx, |j| {
            j.publish_at = Some(choice.at);
            j.why_time = why.clone();
            j.upload_id = Some(upload_id);
            j.stage = Stage::Publish;
            j.step = format!("Goes public {when}");
        });
        self.log(LogKind::Success, Some(&job.id), format!("{}: scheduled for {when} ({why})", job.pick.title));
        // Better copy from the AI, while the upload waits its turn (best effort).
        self.improve_copy(&job.id, cx);
        self.advance(&job.id, cx);
    }

    /// Asks the assistant's model for the title, description and tags of the scheduled upload.
    fn improve_copy(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(job) = self.state.job(id).cloned() else { return };
        let Some(upload) = job.upload_id.clone() else { return };
        let (provider, model) = ui_assistant::assistant_model(cx);
        let Some(provider) = serde_json::from_value::<mv_ai::AiProviderId>(serde_json::Value::String(provider)).ok().filter(|_| !model.is_empty()) else { return };
        let registry = ui_assistant::registry(cx);
        let h = hub::hub(cx);
        let ctx = {
            let h = h.read(cx);
            let mut top: Vec<_> = h.videos.iter().filter(|v| v.privacy == "public").collect();
            top.sort_by_key(|v| std::cmp::Reverse(v.views));
            mv_youtube::MetaContext {
                manga_title: job.pick.title.clone(),
                episode_label: "Full story".into(),
                channel_title: h.channel.as_ref().map(|c| c.title.clone()).unwrap_or_default(),
                top_titles: top.iter().take(8).map(|v| v.title.clone()).collect(),
                language: h.settings.language.clone(),
                ..Default::default()
            }
        };
        let jid = id.to_string();
        let (mid, title) = (job.manga_id.clone().unwrap_or_default(), job.pick.title.clone());
        cx.spawn(async move |this, cx| {
            let r = bg::run(async move {
                // The brain (with the whole arc) is read here, off the UI thread.
                let mut ctx = ctx;
                ctx.summary = mv_recap::Brain::load(&mid, &title).notes.chars().take(3500).collect();
                mv_youtube::ai::suggest(registry, provider, model, &ctx).await
            })
            .await;
            if let Ok(Ok(m)) = r {
                let _ = h.update(cx, |h, cx| {
                    h.update_item(&upload, cx, |i| {
                        // Only while it has not started uploading.
                        if i.state == UploadState::Planned {
                            if let Some(t) = m.titles.first() {
                                i.title = t.clone();
                            }
                            if !m.description.is_empty() {
                                i.description = m.description.clone();
                            }
                            if !m.tags.is_empty() {
                                i.tags = m.tags.clone();
                            }
                        }
                    })
                });
                let _ = this.update(cx, |me, cx| {
                    me.log(LogKind::Info, Some(&jid), "The AI wrote the title, description and tags");
                    me.save();
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn publish(&mut self, job: Job, cx: &mut Context<Self>) {
        let h = hub::hub(cx);
        let Some(upload) = job.upload_id.clone() else {
            self.update_job(&job.id, cx, |j| j.stage = Stage::Schedule);
            return;
        };
        let (item, connected, user_paused, blocked_until) = {
            let h = h.read(cx);
            (h.queue.iter().find(|i| i.id == upload).cloned(), h.connected(), h.user_paused, h.uploads_blocked_until)
        };
        let Some(item) = item else {
            // Removed from the queue by hand: schedule again.
            self.update_job(&job.id, cx, |j| {
                j.stage = Stage::Schedule;
                j.upload_id = None;
            });
            return;
        };
        match item.state {
            UploadState::Scheduled | UploadState::Published => {
                let vid = item.video_id.clone();
                let when = item.publish_at.map(crate::when::when).unwrap_or_default();
                self.update_job(&job.id, cx, |j| {
                    j.video_id = vid.clone();
                    j.stage = Stage::Collect;
                    j.publish_at = item.publish_at;
                    j.step = if item.state == UploadState::Scheduled { format!("On YouTube, goes public {when}") } else { "Public".into() };
                });
                self.log(LogKind::Success, Some(&job.id), format!("{}: uploaded; YouTube publishes it {when}, even with this PC off", job.pick.title));
            }
            UploadState::Failed => self.fail(&job.id, format!("Upload failed: {}", item.error.unwrap_or_default()), cx),
            UploadState::Uploading => self.update_job(&job.id, cx, |j| j.step = format!("Uploading {:.0}%", item.progress * 100.)),
            UploadState::Planned => {
                if !connected {
                    self.update_job(&job.id, cx, |j| j.step = "Waiting for your YouTube channel to be connected".into());
                } else if user_paused {
                    self.update_job(&job.id, cx, |j| j.step = "Uploads are paused (YouTube › Upload)".into());
                } else if blocked_until > now() {
                    // YouTube refused the last upload for quota: nothing to try before the reset.
                    let when = crate::when::when(blocked_until);
                    self.update_job(&job.id, cx, |j| j.step = format!("YouTube's upload limit is used up; uploads again {when}"));
                } else {
                    // Only this item: the user's own planned uploads wait for their click.
                    h.update(cx, |h, cx| h.upload_items(vec![upload.clone()], cx));
                    self.update_job(&job.id, cx, |j| j.step = "Waiting to upload".into());
                }
            }
        }
    }

    fn collect(&mut self, job: Job, cx: &mut Context<Self>) {
        let Some(vid) = job.video_id.clone() else { return };
        let released = job.publish_at.unwrap_or(job.updated_at);
        let t = now();
        if t < released + 24 * 3600 {
            self.update_job(&job.id, cx, |j| {
                if t < released {
                    j.step = format!("Goes public {}", crate::when::when(released));
                } else {
                    j.step = "Public: first numbers after a day".into();
                }
            });
            return;
        }
        let h = hub::hub(cx);
        let (video, daily) = {
            let h = h.read(cx);
            (h.video(&vid).cloned(), h.video_daily.get(&vid).cloned().unwrap_or_default())
        };
        let Some(v) = video else { return };
        let (_, avg, _) = auto::sum_daily(&daily);
        let results = Results { views: v.views, likes: v.likes, comments: v.comments, avg_view_secs: avg, retention: auto::retention(avg, v.duration_secs), collected_at: t };
        let changed = job.results.as_ref().is_none_or(|r| r.views != results.views || r.comments != results.comments);
        let finished = t > released + COLLECT_FOR;
        let title = job.pick.title.clone();
        let line = format!("{} views, {:.0}% retention, {} comments", hub::fmt_num(results.views), results.retention * 100., results.comments);
        self.update_job(&job.id, cx, |j| {
            j.results = Some(results);
            j.step = line.clone();
            if finished {
                j.stage = Stage::Done;
                j.state = JobState::Done;
            }
        });
        if changed {
            self.log(LogKind::Info, Some(&job.id), format!("{title}: {line}"));
        }
        if finished {
            self.log(LogKind::Success, Some(&job.id), format!("{title}: a week of results collected; they now shape the next picks and release times"));
            // The video's own comments feed the requests too.
            h.update(cx, |h, cx| h.load_comments(true, cx));
        }
    }
}

fn fmt_ch(n: f64) -> String {
    if n.fract() == 0. {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Length of a video in seconds (ffprobe, no console window), 0 when it cannot be read.
fn ffprobe_secs(file: &std::path::Path) -> f64 {
    mv_studio::python::quiet_command("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "default=nw=1:nk=1"])
        .arg(file)
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(0.)
}

/// Studio's Generate queue needs its context (model, Python env); the Studio page sets it, but the
/// automation can run before Studio was opened.
fn ensure_pipeline_context(cx: &mut App) {
    if ui_studio::pipeline::has_context() {
        return;
    }
    let env = ui_studio::setup::latest().map(|c| c.env);
    ui_studio::pipeline::set_context(ui_studio::pipeline::context_from_app(env, cx));
}

/// `bg::run`'s two layers of errors as one message.
fn r_result<T, E: std::fmt::Display>(r: Result<anyhow::Result<T>, E>) -> Result<T, String> {
    match r {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(format!("{e:#}")),
        Err(e) => Err(e.to_string()),
    }
}
