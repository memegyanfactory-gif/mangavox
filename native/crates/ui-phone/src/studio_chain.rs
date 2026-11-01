//! "Make in Studio" from the phone (API.md "Browse"): one request adds the manga to the library,
//! queues the picked chapters (default: all) in the desktop download manager, and hands each one
//! to Studio's Generate queue (`ui_studio::pipeline::enqueue`, Read → Transcript → Story → Audio →
//! Music → Video) as soon as it is saved. Chapters already downloaded go in at once.
//!
//! The chaining: a chain per manga lists the chapters still downloading. A watcher (one task,
//! running while any chain waits) looks every few seconds: a chapter the download manager no
//! longer holds and the database marks saved is enqueued; one it no longer holds but that is not
//! saved failed. Generate's Read never starts before the pages are on disk. Chains live in memory,
//! like the download queue itself. The Activity tab shows each chain ([`activity`]).
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::Path as UrlPath;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use mv_recap::Step;
use serde_json::{json, Value};
use ui_manga::backend;
use ui_studio::pipeline;

use crate::api_browse::{picked, queue_downloads, ChaptersBody};
use crate::api_studio::{readiness, studio_env, studio_mode};
use crate::server::{blocking, ApiError, ApiResult};

pub fn routes() -> Router {
    Router::new().route("/api/manga/{id}/studio", post(make))
}

/// How often the watcher looks at the downloads.
const TICK: Duration = Duration::from_secs(3);
/// A finished chain is still reported this long.
const KEEP: Duration = Duration::from_secs(30 * 60);

/// One manga's chapters on their way from download to Generate.
#[derive(Clone, Debug, PartialEq)]
pub struct Chain {
    pub manga_id: String,
    pub title: String,
    /// Chapters still downloading.
    pub waiting: Vec<String>,
    /// Handed to Generate, and lost on the way (download failed, or Generate refused them).
    pub sent: usize,
    pub failed: usize,
    /// When the last waiting chapter went through.
    pub ended: Option<Instant>,
}

impl Chain {
    pub fn total(&self) -> usize {
        self.waiting.len() + self.sent + self.failed
    }
}

static CHAINS: Mutex<Vec<Chain>> = Mutex::new(Vec::new());
static WATCHING: AtomicBool = AtomicBool::new(false);

fn chains() -> std::sync::MutexGuard<'static, Vec<Chain>> {
    CHAINS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Chains for the Activity tab (finished ones for [`KEEP`]).
pub(crate) fn activity() -> Vec<Chain> {
    let mut g = chains();
    g.retain(|c| c.ended.is_none_or(|t| t.elapsed() < KEEP));
    g.clone()
}

/// Adds chapters to a manga's chain (a new chain when it had none running).
fn add(manga_id: &str, title: &str, waiting: &[String], sent: usize, failed: usize) {
    let mut g = chains();
    match g.iter_mut().find(|c| c.manga_id == manga_id && c.ended.is_none()) {
        Some(c) => {
            for id in waiting {
                if !c.waiting.contains(id) {
                    c.waiting.push(id.clone());
                }
            }
            c.sent += sent;
            c.failed += failed;
            if c.waiting.is_empty() {
                c.ended = Some(Instant::now());
            }
        }
        None => {
            g.retain(|c| c.manga_id != manga_id);
            g.push(Chain { manga_id: manga_id.into(), title: title.into(), waiting: waiting.to_vec(), sent, failed, ended: waiting.is_empty().then(Instant::now) });
        }
    }
}

/// What became of the waiting chapters: `ready` (saved), `lost` (neither saved nor queued).
pub fn sort_out(waiting: &[String], still_queued: &HashSet<String>, saved: &HashSet<String>) -> (Vec<String>, Vec<String>) {
    let mut ready = vec![];
    let mut lost = vec![];
    for id in waiting {
        if saved.contains(id) {
            ready.push(id.clone());
        } else if !still_queued.contains(id) {
            lost.push(id.clone());
        }
    }
    (ready, lost)
}

/// Hands saved chapters to Generate (the whole pipeline). Returns how many went in.
fn to_generate(ids: &[String]) -> usize {
    if ids.is_empty() {
        return 0;
    }
    let Ok(groups) = ui_studio::data::load() else { return 0 };
    let items: Vec<ui_studio::data::ChapterItem> = groups.into_iter().flat_map(|g| g.chapters).filter(|c| ids.contains(&c.id)).collect();
    if items.is_empty() {
        return 0;
    }
    let prefs = mv_recap::Prefs::load();
    let (new, widened, already) = pipeline::enqueue(&items, Step::Video, None, &prefs.format, None, true);
    (new + widened + already).min(items.len())
}

/// The watcher: runs while any chain waits.
fn ensure_watcher() {
    if WATCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async {
        loop {
            tokio::time::sleep(TICK).await;
            let waiting: Vec<String> = chains().iter().filter(|c| c.ended.is_none()).flat_map(|c| c.waiting.clone()).collect();
            if waiting.is_empty() {
                WATCHING.store(false, Ordering::SeqCst);
                // A chain added between the look and the store starts its own watcher.
                if chains().iter().any(|c| c.ended.is_none()) && !WATCHING.swap(true, Ordering::SeqCst) {
                    continue;
                }
                break;
            }
            // The download manager first, then the database: a chapter that finishes in between is
            // then seen as saved rather than lost.
            let ids = waiting.clone();
            let queued = crate::bridge::on_ui(move |_, cx| {
                let e = ui_manga::downloads(cx);
                let d = e.read(cx);
                ids.into_iter().filter(|id| d.status(id).is_some()).collect::<HashSet<String>>()
            })
            .await;
            // Without the app's UI the queue cannot be read: try again later.
            let Ok(queued) = queued else { continue };
            let ids = waiting.clone();
            let Ok(Ok(saved)) = tokio::task::spawn_blocking(move || {
                backend::with_db(|c| {
                    let mut s = HashSet::new();
                    for id in &ids {
                        if mv_core::repo::chapter::get(c, id)?.is_some_and(|ch| ch.download_status == "complete") {
                            s.insert(id.clone());
                        }
                    }
                    Ok(s)
                })
            })
            .await
            else {
                continue;
            };
            let (ready, lost) = sort_out(&waiting, &queued, &saved);
            if ready.is_empty() && lost.is_empty() {
                continue;
            }
            let r2 = ready.clone();
            let sent = tokio::task::spawn_blocking(move || to_generate(&r2)).await.unwrap_or(0);
            let mut g = chains();
            for c in g.iter_mut().filter(|c| c.ended.is_none()) {
                let mine_ready = c.waiting.iter().filter(|id| ready.contains(id)).count();
                let mine_lost = c.waiting.iter().filter(|id| lost.contains(id)).count();
                if mine_ready + mine_lost == 0 {
                    continue;
                }
                c.waiting.retain(|id| !ready.contains(id) && !lost.contains(id));
                // Generate took `sent` of all ready chapters; share it out in order.
                let ok = mine_ready.min(sent);
                c.sent += ok;
                c.failed += mine_lost + (mine_ready - ok);
                if c.waiting.is_empty() {
                    c.ended = Some(Instant::now());
                }
            }
        }
    });
}

// ---------------------------------------------------------------- POST /api/manga/<id>/studio

async fn make(UrlPath(id): UrlPath<String>, raw: axum::body::Bytes) -> ApiResult<Json<Value>> {
    let b: ChaptersBody = serde_json::from_slice(&raw).map_err(|e| ApiError::bad_request(format!("Bad request body: {e}")))?;
    let (m, list) = picked(id, b.chapters).await?;
    // Studio must be able to run the whole pipeline before anything is queued.
    let mode = studio_mode().await;
    let env = studio_env().await;
    let (ready, reason) = readiness(mode, env.as_ref());
    let Some(env) = env.filter(|_| ready) else {
        return Err(ApiError(StatusCode::CONFLICT, reason.unwrap_or_else(|| "Studio isn't ready".into())));
    };
    if !env.ffmpeg {
        return Err(ApiError(StatusCode::CONFLICT, "FFmpeg is missing on the PC, so videos can't be made yet".into()));
    }
    // Generate runs only once it has the Studio page's context; set it like the page does.
    if !pipeline::has_context() {
        crate::bridge::on_ui(move |_, cx| pipeline::set_context(pipeline::context_from_app(Some(env), cx))).await.map_err(ApiError::unavailable)?;
    }
    let mid = m.id.clone();
    blocking(move || {
        backend::set_in_library(&mid, true);
        Ok(())
    })
    .await?;
    backend::bump_data();

    let (saved, missing): (Vec<_>, Vec<_>) = list.iter().partition(|c| c.download_status == "complete");
    let saved_ids: Vec<String> = saved.iter().map(|c| c.id.clone()).collect();
    let missing: Vec<mv_core::repo::chapter::Chapter> = missing.into_iter().cloned().collect();
    let queued = queue_downloads(&m, &missing).await?;
    let ids = saved_ids.clone();
    let sent = blocking(move || Ok(to_generate(&ids))).await?;
    let waiting: Vec<String> = missing.iter().map(|c| c.id.clone()).collect();
    add(&m.id, &m.title, &waiting, sent, saved_ids.len().saturating_sub(sent));
    if !waiting.is_empty() {
        ensure_watcher();
    }
    Ok(Json(json!({ "chapters": list.len(), "downloading": queued, "sent": sent, "waiting": waiting.len() })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(v: &[&str]) -> HashSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn saved_chapters_go_on_lost_ones_are_counted() {
        let waiting: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        // a saved, b still downloading, c gone from the queue unsaved, d saved just now (the queue
        // let go of it between the two looks).
        let (ready, lost) = sort_out(&waiting, &set(&["b"]), &set(&["a", "d"]));
        assert_eq!(ready, ["a", "d"]);
        assert_eq!(lost, ["c"]);
        assert_eq!(sort_out(&waiting, &set(&["a", "b", "c", "d"]), &set(&[])), (vec![], vec![]));
    }

    #[test]
    fn chains_merge_per_manga_and_end() {
        chains().clear();
        add("m", "M", &["c1".into(), "c2".into()], 1, 0);
        add("m", "M", &["c2".into(), "c3".into()], 0, 0);
        let c = activity().into_iter().find(|c| c.manga_id == "m").unwrap();
        assert_eq!((c.waiting.len(), c.sent, c.total(), c.ended.is_none()), (3, 1, 4, true));
        // Everything already downloaded: a chain that is over at once.
        add("n", "N", &[], 2, 0);
        assert!(activity().iter().find(|c| c.manga_id == "n").unwrap().ended.is_some());
        chains().clear();
    }
}
