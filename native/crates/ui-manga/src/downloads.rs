//! Offline chapter downloads: one app-wide queue (an entity kept in a GPUI global) that saves
//! chapters page by page into `~/.mangavox/downloads/<manga>/<chapter>/`, plus the sliding
//! progress notification the shell shows over every page.
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::{chapter, manga};
use mv_ui::theme::{icon as icon_px, radius};
use mv_ui::{btn, icon_button, ph, ph_icon, progress_bar, progress_ring_sized, size, ButtonKind, Route, Tokens, Weight};

use crate::backend;

/// Pages fetched at the same time inside one chapter.
const PAGE_PARALLEL: usize = 4;
/// A finished batch without failures closes its notification after this long.
const AUTO_HIDE: Duration = Duration::from_secs(6);

#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub manga_id: String,
    pub manga_title: String,
    pub chapter_id: String,
    /// "Ch. 12" (shown in the notification).
    pub label: String,
}

/// Live state of one chapter in the queue.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChapterDl {
    Queued,
    /// Fraction of pages saved (0.0 until the page list is known).
    Active(f32),
}

struct Active {
    job: Job,
    pages_done: usize,
    pages_total: usize,
}

pub struct Downloads {
    queue: VecDeque<Job>,
    active: Option<Active>,
    /// The active chapter should stop after its current pages.
    cancel: bool,
    // Counters for the notification; a new batch starts when the previous one has finished.
    batch_total: usize,
    batch_done: usize,
    failed: Vec<Job>,
    last_error: Option<String>,
    /// Last failure per chapter id, shown on its row until it downloads or is queued again.
    errors: HashMap<String, String>,
    /// The queue waits after the current chapter (Pause in Downloads).
    paused: bool,
    finished: bool,
    visible: bool,
    /// Bumped per batch so the slide-in animation plays once per notification.
    serial: u64,
}

struct DownloadsGlobal(Entity<Downloads>);
impl Global for DownloadsGlobal {}

/// The shared download queue.
pub fn downloads(cx: &mut App) -> Entity<Downloads> {
    if let Some(g) = cx.try_global::<DownloadsGlobal>() {
        return g.0.clone();
    }
    let e = cx.new(|_| Downloads {
        queue: VecDeque::new(),
        active: None,
        cancel: false,
        batch_total: 0,
        batch_done: 0,
        failed: vec![],
        last_error: None,
        errors: HashMap::new(),
        paused: false,
        finished: false,
        visible: false,
        serial: 0,
    });
    cx.set_global(DownloadsGlobal(e.clone()));
    e
}

fn chapter_dir(manga_id: &str, chapter_id: &str) -> PathBuf {
    backend::downloads_dir().join(manga_id).join(chapter_id)
}

impl Downloads {
    pub fn status(&self, chapter_id: &str) -> Option<ChapterDl> {
        if let Some(a) = self.active.as_ref().filter(|a| a.job.chapter_id == chapter_id) {
            return Some(ChapterDl::Active(if a.pages_total == 0 { 0. } else { a.pages_done as f32 / a.pages_total as f32 }));
        }
        self.queue.iter().any(|j| j.chapter_id == chapter_id).then_some(ChapterDl::Queued)
    }

    /// Chapters of `manga_id` still queued or downloading.
    pub fn pending_for(&self, manga_id: &str) -> usize {
        self.queue.iter().filter(|j| j.manga_id == manga_id).count() + self.active.as_ref().filter(|a| a.job.manga_id == manga_id).map_or(0, |_| 1)
    }

    pub fn is_busy(&self) -> bool {
        self.active.is_some() || !self.queue.is_empty()
    }

    /// Everything still waiting, the active chapter first.
    pub fn pending(&self) -> Vec<(Job, ChapterDl)> {
        let mut v = vec![];
        if let Some(a) = &self.active {
            v.push((a.job.clone(), self.status(&a.job.chapter_id).unwrap_or(ChapterDl::Queued)));
        }
        v.extend(self.queue.iter().map(|j| (j.clone(), ChapterDl::Queued)));
        v
    }

    /// Why the last download of `chapter_id` failed, until it is queued again or succeeds.
    pub fn error(&self, chapter_id: &str) -> Option<&str> {
        self.errors.get(chapter_id).map(String::as_str)
    }

    /// Chapters of the last batch that failed (Retry failed queues them again).
    pub fn failed_jobs(&self) -> &[Job] {
        &self.failed
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Pause: the chapter in progress finishes, the rest waits. Resume picks the queue up again.
    pub fn set_paused(&mut self, paused: bool, cx: &mut Context<Self>) {
        self.paused = paused;
        cx.notify();
        if !paused {
            self.next(cx);
        }
    }

    /// Stop one chapter: drop it from the queue, or stop it after its current pages.
    pub fn cancel_job(&mut self, chapter_id: &str, cx: &mut Context<Self>) {
        let before = self.queue.len();
        self.queue.retain(|j| j.chapter_id != chapter_id);
        self.batch_total = self.batch_total.saturating_sub(before - self.queue.len());
        if self.active.as_ref().is_some_and(|a| a.job.chapter_id == chapter_id) {
            self.cancel = true;
        } else if self.active.is_none() && self.queue.is_empty() {
            self.finish(cx);
        }
        cx.notify();
    }

    pub fn enqueue(&mut self, jobs: Vec<Job>, cx: &mut Context<Self>) {
        let jobs: Vec<Job> = jobs.into_iter().filter(|j| self.status(&j.chapter_id).is_none()).collect();
        if jobs.is_empty() {
            return;
        }
        for j in &jobs {
            self.errors.remove(&j.chapter_id);
        }
        // Asking for a download is a clear intent to run the queue.
        self.paused = false;
        if self.finished || !self.is_busy() {
            self.batch_total = 0;
            self.batch_done = 0;
            self.failed.clear();
            self.last_error = None;
            self.finished = false;
            self.serial += 1;
        }
        self.batch_total += jobs.len();
        self.visible = true;
        self.queue.extend(jobs);
        cx.notify();
        self.next(cx);
    }

    /// Stop every queued and running chapter of one title.
    pub fn cancel_manga(&mut self, manga_id: &str, cx: &mut Context<Self>) {
        let before = self.queue.len();
        self.queue.retain(|j| j.manga_id != manga_id);
        self.batch_total = self.batch_total.saturating_sub(before - self.queue.len());
        if self.active.as_ref().is_some_and(|a| a.job.manga_id == manga_id) {
            self.cancel = true;
        } else if self.active.is_none() && self.queue.is_empty() {
            self.finish(cx);
        }
        cx.notify();
    }

    pub fn cancel_all(&mut self, cx: &mut Context<Self>) {
        self.batch_total = self.batch_total.saturating_sub(self.queue.len());
        self.queue.clear();
        if self.active.is_some() {
            self.cancel = true;
        } else {
            self.finish(cx);
        }
        cx.notify();
    }

    pub fn retry_failed(&mut self, cx: &mut Context<Self>) {
        let jobs = std::mem::take(&mut self.failed);
        self.finished = true;
        self.enqueue(jobs, cx);
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.visible = false;
        cx.notify();
    }

    /// Delete downloaded chapters (`(manga_id, chapter_id)`) and their Studio work from disk and
    /// mark them online-only. `whole_manga` also drops the manga's Studio brain (characters, faces).
    pub fn delete(&mut self, chapters: Vec<(String, String)>, whole_manga: bool, cx: &mut Context<Self>) {
        cx.spawn(async move |_, _| {
            let _ = mv_ui::bg::blocking(move || {
                // Chapters saved before the manga folder changed live where the database says.
                let stored: Vec<PathBuf> = backend::with_db(|c| {
                    let mut out = vec![];
                    for (_, cid) in &chapters {
                        let p: Option<String> = c.query_row("SELECT pages_dir FROM chapter WHERE id = ?1", [cid], |r| r.get(0)).ok().flatten();
                        out.extend(p.map(PathBuf::from).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy() == *cid)));
                    }
                    Ok(out)
                })
                .unwrap_or_default();
                for p in &stored {
                    let _ = std::fs::remove_dir_all(p);
                }
                for (mid, cid) in &chapters {
                    let _ = std::fs::remove_dir_all(chapter_dir(mid, cid));
                    let _ = std::fs::remove_dir_all(mv_core::paths::studio_chapter(mid, cid));
                }
                if whole_manga {
                    for mid in chapters.iter().map(|(m, _)| m).collect::<std::collections::BTreeSet<_>>() {
                        let _ = std::fs::remove_dir_all(mv_core::paths::studio_projects().join(mv_core::paths::safe_name(mid)));
                        let _ = std::fs::remove_dir_all(mv_core::paths::studio_manga(mid));
                    }
                }
                // Titles left without any downloads lose their (now empty) folder too.
                for (mid, _) in &chapters {
                    let d = backend::downloads_dir().join(mid);
                    if std::fs::read_dir(&d).map(|mut r| r.next().is_none()).unwrap_or(false) {
                        let _ = std::fs::remove_dir(&d);
                    }
                }
                backend::with_db(|c| {
                    for (_, cid) in &chapters {
                        c.execute("UPDATE chapter SET download_status = 'none', pages_dir = NULL WHERE id = ?1", [cid])?;
                    }
                    Ok(())
                })
            })
            .await;
            backend::bump_data();
        })
        .detach();
    }

    fn next(&mut self, cx: &mut Context<Self>) {
        if self.active.is_some() || (self.paused && !self.queue.is_empty()) {
            return;
        }
        let Some(job) = self.queue.pop_front() else {
            self.finish(cx);
            return;
        };
        self.active = Some(Active { job: job.clone(), pages_done: 0, pages_total: 0 });
        self.cancel = false;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res: Result<bool> = async {
                let (cid, mid) = (job.chapter_id.clone(), job.manga_id.clone());
                let (source, chapter_source_id) = mv_ui::bg::blocking(move || {
                    backend::with_db(|c| {
                        let ch = chapter::get(c, &cid)?.ok_or_else(|| anyhow!("Chapter no longer exists"))?;
                        let m = manga::get(c, &mid)?.ok_or_else(|| anyhow!("Title no longer exists"))?;
                        chapter::update_download_status(c, &cid, "downloading", None)?;
                        let src = if ch.source_plugin.is_empty() { m.source_plugin } else { ch.source_plugin };
                        Ok((src, ch.source_id))
                    })
                })
                .await??;
                let src_id = source.clone();
                let urls = mv_ui::bg::run(async move {
                    let src = backend::source(&src_id).await?;
                    src.get_page_urls(&chapter_source_id).await.map_err(|e| anyhow!("{e}"))
                })
                .await??;
                if urls.is_empty() {
                    bail!("The source returned no pages");
                }
                let dir = chapter_dir(&job.manga_id, &job.chapter_id);
                let d2 = dir.clone();
                mv_ui::bg::blocking(move || {
                    let _ = std::fs::remove_dir_all(&d2);
                    std::fs::create_dir_all(&d2)
                })
                .await??;
                this.update(cx, |d, cx| {
                    if let Some(a) = &mut d.active {
                        a.pages_total = urls.len();
                    }
                    cx.notify();
                })?;
                for (ci, chunk) in urls.chunks(PAGE_PARALLEL).enumerate() {
                    if this.read_with(cx, |d, _| d.cancel)? {
                        return Ok(false);
                    }
                    let futs = chunk.iter().enumerate().map(|(k, url)| {
                        let (url, src, dir) = (url.clone(), source.clone(), dir.clone());
                        let index = ci * PAGE_PARALLEL + k;
                        mv_ui::bg::run(async move {
                            match backend::save_page(url.clone(), Some(src.clone()), dir.clone(), index).await {
                                Ok(p) => Ok(p),
                                // One retry: image hosts often drop a request under load.
                                Err(_) => {
                                    tokio::time::sleep(Duration::from_millis(800)).await;
                                    backend::save_page(url, Some(src), dir, index).await
                                }
                            }
                        })
                    });
                    for r in futures::future::join_all(futs).await {
                        r??;
                    }
                    this.update(cx, |d, cx| {
                        if let Some(a) = &mut d.active {
                            a.pages_done += chunk.len();
                        }
                        cx.notify();
                    })?;
                }
                let (cid, dir_s) = (job.chapter_id.clone(), dir.to_string_lossy().to_string());
                mv_ui::bg::blocking(move || backend::with_db(|c| chapter::update_download_status(c, &cid, "complete", Some(&dir_s)))).await??;
                Ok(true)
            }
            .await;
            if !matches!(res, Ok(true)) {
                // Cancelled or failed: leave nothing half-saved behind.
                let (mid, cid) = (job.manga_id.clone(), job.chapter_id.clone());
                let _ = mv_ui::bg::blocking(move || {
                    let _ = std::fs::remove_dir_all(chapter_dir(&mid, &cid));
                    backend::with_db(|c| chapter::update_download_status(c, &cid, "none", None))
                })
                .await;
            }
            backend::bump_data();
            let _ = this.update(cx, |d, cx| {
                d.active = None;
                match res {
                    Ok(true) => {
                        d.errors.remove(&job.chapter_id);
                        d.batch_done += 1
                    }
                    Ok(false) => d.batch_total = d.batch_total.saturating_sub(1),
                    Err(e) => {
                        let msg = format!("{e:#}");
                        d.errors.insert(job.chapter_id.clone(), msg.clone());
                        d.last_error = Some(msg);
                        d.failed.push(job);
                    }
                }
                d.next(cx);
            });
        })
        .detach();
    }

    fn finish(&mut self, cx: &mut Context<Self>) {
        self.finished = true;
        if self.batch_total == 0 {
            // Everything was cancelled.
            self.visible = false;
        }
        cx.notify();
        if self.failed.is_empty() {
            let serial = self.serial;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(AUTO_HIDE).await;
                let _ = this.update(cx, |d, cx| {
                    if d.serial == serial && d.finished {
                        d.visible = false;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }
}

// ---------------------------------------------------------------- notification

/// The sliding download notification (top right of the page area), or nothing when idle.
/// The shell observes [`downloads`] and draws this over the current page. A persistent progress
/// panel rather than a toast: it stays while work runs and offers Pause, Cancel and Retry.
/// NN/g visibility of system status: <https://www.nngroup.com/articles/visibility-system-status/>
pub fn toast(cx: &mut App) -> Option<AnyElement> {
    let e = downloads(cx);
    let d = e.read(cx);
    if !d.visible {
        return None;
    }
    let busy = d.is_busy();
    let paused = d.paused;
    let done = d.batch_done;
    let total = d.batch_total.max(1);
    // Chapters finished plus the fraction of the one in progress.
    let partial = d.active.as_ref().map_or(0., |a| if a.pages_total == 0 { 0. } else { a.pages_done as f32 / a.pages_total as f32 });
    let frac = ((done as f32 + partial) / total as f32).min(1.);
    let failed = d.failed.len();

    let (lead, title, line): (AnyElement, String, String) = if busy {
        let a = d.active.as_ref();
        let title = a.map(|a| a.job.manga_title.clone()).unwrap_or_else(|| "Downloads".into());
        let line = match a {
            Some(a) if a.pages_total > 0 => format!("{} · page {} of {}", a.job.label, (a.pages_done + 1).min(a.pages_total), a.pages_total),
            Some(a) => format!("{} · preparing", a.job.label),
            None if paused => "Paused".into(),
            None => "Starting".into(),
        };
        (progress_ring_sized(frac, icon_px::LG), title, line)
    } else if failed > 0 {
        let line = d.last_error.clone().unwrap_or_default();
        (ph_icon(ph::WARNING, Weight::Fill, Tokens::warning(), icon_px::LG).into_any_element(), format!("{failed} of {} chapters failed", d.batch_total), line)
    } else {
        let n = d.batch_done;
        (
            ph_icon(ph::CHECK_CIRCLE, Weight::Fill, Tokens::success(), icon_px::LG).into_any_element(),
            "Download complete".into(),
            format!("{n} chapter{} saved for offline reading", if n == 1 { "" } else { "s" }),
        )
    };

    let (e1, e2, e3, e4, e5) = (e.clone(), e.clone(), e.clone(), e.clone(), e.clone());
    let el = div()
        .id("dl-toast")
        .absolute()
        .top(px(16.))
        .right(px(16.))
        .w(px(340.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .p(px(14.))
        .rounded(px(radius::LG))
        .bg(Tokens::glass_strong())
        .border_1()
        .border_color(Tokens::glass_border())
        .shadow_lg()
        .occlude()
        .role(Role::Status)
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(10.))
                .child(div().mt(px(1.)).flex_none().child(lead))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(div().truncate().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title))
                        .child(div().truncate().text_size(px(size::SM)).text_color(Tokens::text_3()).child(line)),
                )
                .child(icon_button("dl-close", ph::X, "Hide").small().on_click(move |_, _, cx| e1.update(cx, |d, cx| d.dismiss(cx)))),
        )
        .when(busy, |el| {
            el.child(progress_bar("dl-toast-bar", frac, !paused)).child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .text_size(px(size::SM))
                    .text_color(Tokens::text_2())
                    .child(div().flex_1().child(format!("{} of {} chapters", (done + 1).min(d.batch_total), d.batch_total)))
                    .child(
                        icon_button("dl-pause", if paused { ph::PLAY } else { ph::PAUSE }, if paused { "Resume downloads" } else { "Pause after this chapter" })
                            .small()
                            .on_click(move |_, _, cx| e2.update(cx, |d, cx| d.set_paused(!paused, cx))),
                    )
                    .child(btn("dl-cancel", "Cancel all", ButtonKind::Ghost).small().icon(ph::X).on_click(move |_, _, cx| e3.update(cx, |d, cx| d.cancel_all(cx)))),
            )
        })
        .when(!busy, |el| {
            el.child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(4.))
                    .when(failed > 0, |r| r.child(btn("dl-retry", "Retry", ButtonKind::Ghost).small().icon(ph::ARROW_CLOCKWISE).on_click(move |_, _, cx| e4.update(cx, |d, cx| d.retry_failed(cx)))))
                    .child(btn("dl-open", "Open Downloads", ButtonKind::Ghost).small().trailing_icon(ph::ARROW_RIGHT).on_click(move |_, w, cx| {
                        e5.update(cx, |d, cx| d.dismiss(cx));
                        mv_ui::navigate(Route::MangaDownloads, w, cx);
                    })),
            )
        });
    if mv_ui::motion::reduced() {
        return Some(el.into_any_element());
    }
    Some(
        el.with_animation(
            SharedString::from(format!("dl-toast-{}", d.serial)),
            Animation::new(Duration::from_millis(320)).with_easing(ease_out_quint()),
            |el, t| el.opacity(t).mr(px((1.0 - t) * -360.0)),
        )
        .into_any_element(),
    )
}

