//! Offline chapter downloads: one app-wide queue (an entity kept in a GPUI global) that saves
//! chapters page by page into `~/.mangavox/downloads/<manga>/<chapter>/`, plus one progress card
//! per title for the shell's activity corner.
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use gpui_kit::*;
use mv_core::repo::{chapter, manga};
use mv_ui::activity::{self, Card, Lead};
use mv_ui::{btn, icon_button, ph, ButtonKind, Route};

use crate::backend;

/// Pages fetched at the same time inside one chapter.
const PAGE_PARALLEL: usize = 4;
/// A title that finished without failures closes its card after this long.
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

/// One title's share of the queue: what its card counts, from when the card appeared until the
/// title's next downloads after it finished start a new count.
#[derive(Clone, Debug, PartialEq)]
struct Group {
    manga_id: String,
    title: String,
    total: usize,
    done: usize,
    failed: Vec<Job>,
    last_error: Option<String>,
    /// Paused from its card: the title's chapters wait, other titles carry on.
    held: bool,
    /// Nothing of the title is queued or downloading any more.
    finished: bool,
    visible: bool,
    /// Unique per count ([`activity::next_order`]): sorts and keys the card, so its slide-in plays
    /// once, and tells a stale auto-hide timer apart.
    serial: u64,
}

/// Every title's [`Group`]. Pure bookkeeping, so it is tested without a window.
#[derive(Default)]
struct Groups {
    list: Vec<Group>,
}

impl Groups {
    fn get(&self, manga_id: &str) -> Option<&Group> {
        self.list.iter().find(|g| g.manga_id == manga_id)
    }

    fn get_mut(&mut self, manga_id: &str) -> Option<&mut Group> {
        self.list.iter_mut().find(|g| g.manga_id == manga_id)
    }

    fn held(&self, manga_id: &str) -> bool {
        self.get(manga_id).is_some_and(|g| g.held)
    }

    /// `n` more chapters of a title. Asking for a download runs the title again; one whose count
    /// had finished starts a new count (and a new card, the newest in the stack).
    fn add(&mut self, manga_id: &str, title: &str, n: usize) {
        if let Some(g) = self.get_mut(manga_id).filter(|g| !g.finished) {
            g.total += n;
            g.held = false;
            g.visible = true;
            return;
        }
        self.list.retain(|g| g.manga_id != manga_id);
        self.list.push(Group {
            manga_id: manga_id.into(),
            title: title.into(),
            total: n,
            done: 0,
            failed: vec![],
            last_error: None,
            held: false,
            finished: false,
            visible: true,
            serial: activity::next_order(),
        });
    }

    /// Chapters of a title that will not download after all (cancelled).
    fn drop_chapters(&mut self, manga_id: &str, n: usize) {
        if let Some(g) = self.get_mut(manga_id) {
            g.total = g.total.saturating_sub(n);
        }
    }

    fn chapter_done(&mut self, manga_id: &str) {
        if let Some(g) = self.get_mut(manga_id) {
            g.done += 1;
        }
    }

    fn chapter_failed(&mut self, job: Job, error: String) {
        if let Some(g) = self.get_mut(&job.manga_id) {
            g.last_error = Some(error);
            g.failed.push(job);
        }
    }

    /// Titles with nothing left to download finish: a fully cancelled one goes away, the others
    /// show their result. Returns the serials that finished without failures (they hide soon).
    fn settle(&mut self, pending: impl Fn(&str) -> bool) -> Vec<u64> {
        let mut clean = vec![];
        self.list.retain_mut(|g| {
            if g.finished || pending(&g.manga_id) {
                return true;
            }
            g.finished = true;
            g.held = false;
            if g.total == 0 && g.failed.is_empty() {
                return false;
            }
            if g.failed.is_empty() {
                clean.push(g.serial);
            }
            true
        });
        clean
    }

    /// The close button. A running title's card hides until the title gets more chapters; a
    /// finished one goes, unless it has failures to keep for Retry failed.
    fn dismiss(&mut self, manga_id: &str) {
        if let Some(g) = self.get_mut(manga_id) {
            g.visible = false;
        }
        self.list.retain(|g| !(g.manga_id == manga_id && g.finished && g.failed.is_empty()));
    }

    /// The auto-hide timer of count `serial` ran out. True when its card went.
    fn expire(&mut self, serial: u64) -> bool {
        let before = self.list.len();
        self.list.retain(|g| !(g.serial == serial && g.finished && g.failed.is_empty()));
        before != self.list.len()
    }

    /// Takes the failed chapters of every title (or of one) to queue them again.
    fn take_failed(&mut self, manga_id: Option<&str>) -> Vec<Job> {
        let mut out = vec![];
        for g in self.list.iter_mut().filter(|g| manga_id.is_none_or(|m| g.manga_id == m)) {
            // A title still running counts them again when they are queued.
            g.total = g.total.saturating_sub(g.failed.len());
            g.last_error = None;
            out.append(&mut g.failed);
        }
        out
    }

    fn failed(&self) -> Vec<Job> {
        self.list.iter().flat_map(|g| g.failed.iter().cloned()).collect()
    }
}

pub struct Downloads {
    queue: VecDeque<Job>,
    active: Option<Active>,
    /// The active chapter should stop after its current pages.
    cancel: bool,
    /// Last failure per chapter id, shown on its row until it downloads or is queued again.
    errors: HashMap<String, String>,
    /// The queue waits after the current chapter (Pause in Downloads).
    paused: bool,
    /// One card per title.
    groups: Groups,
}

struct DownloadsGlobal(Entity<Downloads>);
impl Global for DownloadsGlobal {}

/// The shared download queue.
pub fn downloads(cx: &mut App) -> Entity<Downloads> {
    if let Some(g) = cx.try_global::<DownloadsGlobal>() {
        return g.0.clone();
    }
    let e = cx.new(|_| Downloads { queue: VecDeque::new(), active: None, cancel: false, errors: HashMap::new(), paused: false, groups: Groups::default() });
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

    /// Chapters that failed and were not retried yet, every title (Retry failed queues them again).
    pub fn failed_jobs(&self) -> Vec<Job> {
        self.groups.failed()
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

    /// Pause one title from its card: its chapter in progress finishes, its other chapters wait
    /// and the next title starts. Resuming also lifts a Pause of the whole queue.
    pub fn set_held(&mut self, manga_id: &str, held: bool, cx: &mut Context<Self>) {
        if let Some(g) = self.groups.get_mut(manga_id) {
            g.held = held;
        }
        if !held {
            self.paused = false;
        }
        cx.notify();
        self.next(cx);
    }

    /// Drops the queued chapters `drop` picks; true when the active chapter is picked too (it
    /// stops after its current pages).
    fn remove(&mut self, drop: impl Fn(&Job) -> bool) -> bool {
        let (gone, kept): (Vec<Job>, Vec<Job>) = self.queue.drain(..).partition(|j| drop(j));
        self.queue = kept.into();
        for j in &gone {
            self.groups.drop_chapters(&j.manga_id, 1);
        }
        self.active.as_ref().is_some_and(|a| drop(&a.job))
    }

    /// Stop one chapter: drop it from the queue, or stop it after its current pages.
    pub fn cancel_job(&mut self, chapter_id: &str, cx: &mut Context<Self>) {
        if self.remove(|j| j.chapter_id == chapter_id) {
            self.cancel = true;
        }
        self.settle(cx);
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
        let mut titles: Vec<(&str, &str, usize)> = vec![];
        for j in &jobs {
            match titles.iter_mut().find(|t| t.0 == j.manga_id) {
                Some(t) => t.2 += 1,
                None => titles.push((&j.manga_id, &j.manga_title, 1)),
            }
        }
        for (mid, title, n) in titles {
            self.groups.add(mid, title, n);
        }
        self.queue.extend(jobs);
        cx.notify();
        self.next(cx);
    }

    /// Stop every queued and running chapter of one title.
    pub fn cancel_manga(&mut self, manga_id: &str, cx: &mut Context<Self>) {
        if self.remove(|j| j.manga_id == manga_id) {
            self.cancel = true;
        }
        self.settle(cx);
        cx.notify();
    }

    pub fn cancel_all(&mut self, cx: &mut Context<Self>) {
        if self.remove(|_| true) {
            self.cancel = true;
        }
        self.settle(cx);
        cx.notify();
    }

    /// Queues every failed chapter again.
    pub fn retry_failed(&mut self, cx: &mut Context<Self>) {
        let jobs = self.groups.take_failed(None);
        self.enqueue(jobs, cx);
    }

    /// Queues one title's failed chapters again (Retry on its card).
    pub fn retry_manga(&mut self, manga_id: &str, cx: &mut Context<Self>) {
        let jobs = self.groups.take_failed(Some(manga_id));
        self.enqueue(jobs, cx);
    }

    /// Hides one title's card (its downloads carry on).
    pub fn dismiss(&mut self, manga_id: &str, cx: &mut Context<Self>) {
        self.groups.dismiss(manga_id);
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
        if self.active.is_some() {
            return;
        }
        if self.queue.is_empty() {
            self.settle(cx);
            return;
        }
        // The first chapter of a title not paused from its card.
        let Some(job) = self.queue.iter().position(|j| !self.groups.held(&j.manga_id)).filter(|_| !self.paused).and_then(|i| self.queue.remove(i)) else {
            return;
        };
        self.active = Some(Active { job: job.clone(), pages_done: 0, pages_total: 0 });
        self.cancel = false;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res: Result<bool> = async {
                let (cid, mid) = (job.chapter_id.clone(), job.manga_id.clone());
                let tries = mv_ui::bg::blocking(move || {
                    backend::with_db(|c| {
                        let ch = chapter::get(c, &cid)?.ok_or_else(|| anyhow!("Chapter no longer exists"))?;
                        let m = manga::get(c, &mid)?.ok_or_else(|| anyhow!("Title no longer exists"))?;
                        chapter::update_download_status(c, &cid, "downloading", None)?;
                        let src = if ch.source_plugin.is_empty() { m.source_plugin.clone() } else { ch.source_plugin.clone() };
                        // Other sources with the same chapter, for one its own source cannot serve.
                        let alts = backend::alternate_sources(&chapter::list(c, &m.id)?, &ch);
                        Ok(std::iter::once((src, ch.source_id)).chain(alts).collect::<Vec<_>>())
                    })
                })
                .await??;
                let (source, urls) = mv_ui::bg::run(backend::page_urls(tries)).await??;
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
                        d.groups.chapter_done(&job.manga_id);
                    }
                    Ok(false) => d.groups.drop_chapters(&job.manga_id, 1),
                    Err(e) => {
                        let msg = format!("{e:#}");
                        d.errors.insert(job.chapter_id.clone(), msg.clone());
                        d.groups.chapter_failed(job, msg);
                    }
                }
                // The title whose last chapter this was shows its result.
                d.settle(cx);
                d.next(cx);
            });
        })
        .detach();
    }

    /// Titles with nothing left queued or downloading finish; the clean ones close after a while.
    fn settle(&mut self, cx: &mut Context<Self>) {
        let pending: HashSet<String> = self.queue.iter().chain(self.active.as_ref().map(|a| &a.job)).map(|j| j.manga_id.clone()).collect();
        for serial in self.groups.settle(|m| pending.contains(m)) {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(AUTO_HIDE).await;
                let _ = this.update(cx, |d, cx| {
                    if d.groups.expire(serial) {
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }
}


// ---------------------------------------------------------------- cards

/// One card per title with downloads to show, for the shell's activity corner
/// ([`mv_ui::activity::corner`]). A persistent progress card rather than a toast: it stays while
/// the title downloads and offers Pause, Cancel and Retry.
/// NN/g visibility of system status: <https://www.nngroup.com/articles/visibility-system-status/>
pub fn cards(cx: &mut App) -> Vec<Card> {
    let e = downloads(cx);
    let d = e.read(cx);
    d.groups.list.iter().filter(|g| g.visible).map(|g| card(&e, d, g)).collect()
}

fn card(e: &Entity<Downloads>, d: &Downloads, g: &Group) -> Card {
    let key = format!("dl-{}", g.serial);
    let mid = g.manga_id.clone();
    let on = |f: fn(&mut Downloads, &str, &mut Context<Downloads>)| {
        let (e, mid) = (e.clone(), mid.clone());
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| e.update(cx, |d, cx| f(d, &mid, cx))
    };
    let open = {
        let (e, mid) = (e.clone(), mid.clone());
        btn("open", "Open Downloads", ButtonKind::Ghost).small().trailing_icon(ph::ARROW_RIGHT).on_click(move |_, w, cx| {
            e.update(cx, |d, cx| d.dismiss(&mid, cx));
            mv_ui::navigate(Route::MangaDownloads, w, cx);
        })
    };
    let close = {
        let (e, mid) = (e.clone(), mid.clone());
        move |_: &mut Window, cx: &mut App| e.update(cx, |d, cx| d.dismiss(&mid, cx))
    };

    if g.finished {
        let failed = g.failed.len();
        if failed > 0 {
            let why = g.last_error.as_deref().map(|e| format!(": {e}")).unwrap_or_default();
            return Card::new(key, g.serial, Lead::Failed, g.title.clone(), format!("{failed} of {} chapters failed{why}", g.total))
                .action(btn("retry", "Retry", ButtonKind::Ghost).small().icon(ph::ARROW_CLOCKWISE).on_click(on(|d, m, cx| d.retry_manga(m, cx))))
                .action(open)
                .on_close(close);
        }
        let n = g.done;
        return Card::new(key, g.serial, Lead::Done, g.title.clone(), format!("{n} chapter{} saved for offline reading", if n == 1 { "" } else { "s" })).action(open).on_close(close);
    }

    let active = d.active.as_ref().filter(|a| a.job.manga_id == g.manga_id);
    let paused = d.paused || g.held;
    // Chapters finished plus the fraction of the one in progress.
    let partial = active.map_or(0., |a| if a.pages_total == 0 { 0. } else { a.pages_done as f32 / a.pages_total as f32 });
    let frac = ((g.done as f32 + partial) / g.total.max(1) as f32).min(1.);
    let line = match active {
        Some(a) if paused => format!("{} · pausing after this chapter", a.job.label),
        Some(a) if a.pages_total > 0 => format!("{} · page {} of {}", a.job.label, (a.pages_done + 1).min(a.pages_total), a.pages_total),
        Some(a) => format!("{} · preparing", a.job.label),
        None if paused => "Paused".into(),
        None => "Waiting for the downloads before it".into(),
    };
    let at = if active.is_some() { (g.done + 1).min(g.total) } else { g.done };
    Card::new(key, g.serial, Lead::Progress(frac), g.title.clone(), line)
        .progress(frac, active.is_some() && !paused)
        .count(format!("{at} of {} chapters", g.total))
        .action(
            icon_button("pause", if paused { ph::PLAY } else { ph::PAUSE }, if paused { "Resume" } else { "Pause after this chapter" })
                .small()
                .on_click(on(if paused { |d, m, cx| d.set_held(m, false, cx) } else { |d, m, cx| d.set_held(m, true, cx) })),
        )
        .action(btn("cancel", "Cancel", ButtonKind::Ghost).small().icon(ph::X).tooltip("Stop downloading this title").on_click(on(|d, m, cx| d.cancel_manga(m, cx))))
        .on_close(close)
}

#[cfg(test)]
mod tests {
    use super::{Groups, Job};

    fn job(manga: &str, ch: &str) -> Job {
        Job { manga_id: manga.into(), manga_title: manga.to_uppercase(), chapter_id: ch.into(), label: format!("Ch. {ch}") }
    }

    #[test]
    fn one_card_per_title() {
        let mut g = Groups::default();
        g.add("a", "A", 3);
        g.add("b", "B", 2);
        g.add("a", "A", 1);
        assert_eq!(g.list.len(), 2);
        assert_eq!(g.get("a").unwrap().total, 4);
        assert_eq!(g.get("b").unwrap().total, 2);
        // The second title's card is newer.
        assert!(g.get("b").unwrap().serial > g.get("a").unwrap().serial);
    }

    #[test]
    fn a_title_finishes_on_its_own() {
        let mut g = Groups::default();
        g.add("a", "A", 1);
        g.add("b", "B", 2);
        g.chapter_done("a");
        // Only "b" still has chapters waiting.
        let clean = g.settle(|m| m == "b");
        assert_eq!(clean, vec![g.get("a").unwrap().serial]);
        assert!(g.get("a").unwrap().finished);
        assert!(!g.get("b").unwrap().finished);
        // Its auto-hide removes it; a second expiry does nothing.
        let s = g.get("a").unwrap().serial;
        assert!(g.expire(s));
        assert!(!g.expire(s));
        assert!(g.get("b").is_some());
    }

    #[test]
    fn a_fully_cancelled_title_goes_away() {
        let mut g = Groups::default();
        g.add("a", "A", 2);
        g.drop_chapters("a", 2);
        assert!(g.settle(|_| false).is_empty());
        assert!(g.list.is_empty());
    }

    #[test]
    fn failures_stay_until_retried() {
        let mut g = Groups::default();
        g.add("a", "A", 2);
        g.chapter_done("a");
        g.chapter_failed(job("a", "2"), "timeout".into());
        assert!(g.settle(|_| false).is_empty(), "a title with failures does not hide by itself");
        let s = g.get("a").unwrap().serial;
        assert!(!g.expire(s));
        // Closing hides the card but keeps the failure for Retry failed.
        g.dismiss("a");
        assert!(!g.get("a").unwrap().visible);
        assert_eq!(g.failed(), vec![job("a", "2")]);
        // Retrying starts a new count with a new card.
        let again = g.take_failed(Some("a"));
        assert_eq!(again, vec![job("a", "2")]);
        g.add("a", "A", again.len());
        let a = g.get("a").unwrap();
        assert!(a.serial > s && a.visible && !a.finished);
        assert_eq!((a.total, a.done), (1, 0));
    }

    #[test]
    fn retrying_while_running_does_not_double_count() {
        let mut g = Groups::default();
        g.add("a", "A", 3);
        g.chapter_failed(job("a", "1"), "x".into());
        let again = g.take_failed(None);
        g.add("a", "A", again.len());
        assert_eq!(g.get("a").unwrap().total, 3);
    }

    #[test]
    fn closing_a_running_card_hides_it_until_more_chapters() {
        let mut g = Groups::default();
        g.add("a", "A", 2);
        g.dismiss("a");
        assert!(!g.get("a").unwrap().visible);
        g.add("a", "A", 1);
        assert!(g.get("a").unwrap().visible);
        // A finished, clean card is dropped when closed.
        g.chapter_done("a");
        g.settle(|_| false);
        g.dismiss("a");
        assert!(g.get("a").is_none());
    }

    #[test]
    fn asking_again_lifts_a_title_pause() {
        let mut g = Groups::default();
        g.add("a", "A", 2);
        g.get_mut("a").unwrap().held = true;
        assert!(g.held("a"));
        g.add("a", "A", 1);
        assert!(!g.held("a"));
    }
}
