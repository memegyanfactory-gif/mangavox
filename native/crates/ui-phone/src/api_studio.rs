//! Studio + Listen: what Studio made (narration clips, chapter videos, full recaps), starting
//! generation from the phone, and the job list. See API.md "Studio + Listen".
//!
//! Generation goes through the same queues the desktop uses: `ui_studio::service` (one Python
//! job per chapter: narration, or narration + video) and `ui_studio::pipeline` (the whole
//! Read → Video run). Jobs are listed with prefixed ids: `s<n>` service, `p<n>` pipeline,
//! `r<n>` a full-recap join started from the phone.
//!
//! Media files are addressed by an opaque id (the path relative to `studio/projects`,
//! base64url); a request only ever resolves to a media file inside that folder.
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, UNIX_EPOCH};

use axum::extract::{Path as UrlPath, Query, Request};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use mv_recap::Step;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use ui_studio::data::{self, ChapterItem, MangaGroup};
use ui_studio::pipeline::{self, ItemState, QChapter, QueueItem};
use ui_studio::service::{self, JobInfo, JobView, NarrationRequest};
use ui_studio::setup::{self, Env};

use crate::bridge;

pub fn routes() -> Router {
    Router::new()
        .route("/api/studio", get(studio))
        .route("/api/studio/generate", post(generate))
        .route("/api/studio/jobs", get(jobs))
        .route("/api/studio/jobs/{id}/cancel", post(cancel))
        .route("/api/media", get(media_list))
        .route("/api/media/{id}", get(media_file))
}

// ---------------------------------------------------------------- errors / helpers

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

fn err(status: StatusCode, msg: impl Into<String>) -> ApiError {
    ApiError(status, msg.into())
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> ApiResult<T> {
    tokio::task::spawn_blocking(f).await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("Background task failed: {e}")))
}

async fn library() -> ApiResult<Vec<MangaGroup>> {
    blocking(data::load).await?.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("Could not read the library: {e:#}")))
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

/// The manga cover route (api_manga.rs).
fn cover_url(manga_id: &str) -> String {
    format!("/api/manga/{}/cover", pct(manga_id))
}

/// "Chapter 12" (the number only), else the chapter's title.
fn chapter_name(number: Option<f64>, title: Option<&str>) -> String {
    match number {
        Some(_) => data::chapter_label(number, None),
        None => data::chapter_label(None, title),
    }
}

/// "Chapter 12 · The Duel" -> "Chapter 12" (service jobs only keep the full label).
fn short_chapter(label: &str) -> &str {
    match label.split_once(" · ") {
        Some((head, _)) if head.starts_with("Chapter ") => head,
        _ => label,
    }
}

fn plural(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 {
        one
    } else {
        many
    }
}

// ---------------------------------------------------------------- setup

/// The last Studio setup check, or a fresh one when none ran yet (Studio was never opened).
async fn studio_env() -> Option<Env> {
    if let Some(c) = setup::latest() {
        return Some(c.env);
    }
    static CHECK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    let _one = CHECK.get_or_init(|| tokio::sync::Mutex::new(())).lock().await;
    if let Some(c) = setup::latest() {
        return Some(c.env);
    }
    mv_ui::bg::run(setup::check()).await.ok()
}

/// (ready, reason): Studio mode, Python + scripts, FFmpeg.
fn readiness(studio_mode: bool, env: Option<&Env>) -> (bool, Option<String>) {
    if !studio_mode {
        return (false, Some("Studio mode is off on the PC. Turn it on in Settings to make recaps.".into()));
    }
    let Some(env) = env else { return (false, Some("Could not check the Studio setup on the PC".into())) };
    if !env.runnable() {
        let detail = env.python.as_ref().err().or(env.scripts.as_ref().err()).cloned().unwrap_or_default();
        let mut msg = "Studio needs Python and the pipeline scripts. Set them up in Studio on the PC.".to_string();
        if !detail.is_empty() {
            msg.push_str(&format!(" ({detail})"));
        }
        return (false, Some(msg));
    }
    if !env.ffmpeg {
        return (true, Some("FFmpeg is missing on the PC: narration works, videos can't be made yet.".into()));
    }
    (true, None)
}

async fn studio_mode() -> bool {
    // Without the UI bridge (still starting) the flag is unknown: don't block on it.
    bridge::on_ui(|_, cx| mv_ui::mode::studio_enabled(cx)).await.unwrap_or(true)
}

// ---------------------------------------------------------------- GET /api/studio

async fn studio() -> ApiResult<Json<Value>> {
    let mode = studio_mode().await;
    let env = studio_env().await;
    let groups = library().await?;
    let (ready, reason) = readiness(mode, env.as_ref());
    let manga: Vec<Value> = groups
        .iter()
        .map(|g| {
            json!({
                "id": g.id,
                "title": g.title,
                "cover": g.cover.as_ref().map(|_| cover_url(&g.id)),
                "chapters": g.chapters.iter().map(|c| json!({
                    "id": c.id,
                    "title": c.label(),
                    "number": c.number,
                    "audio": c.audio_files > 0,
                    "video": c.video.is_some(),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(Json(json!({ "ready": ready, "reason": reason, "manga": manga })))
}

// ---------------------------------------------------------------- POST /api/studio/generate

#[derive(Deserialize)]
struct GenerateBody {
    chapters: Vec<String>,
    what: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum What {
    Audio,
    Video,
    Full,
}

/// How one chapter gets made.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Plan {
    /// One Python job with these stages.
    Service(&'static [&'static str]),
    /// The whole pipeline up to this step (the earlier steps it still needs run first).
    Pipeline(Step),
    /// Can't be made from here (the reason).
    Skip(&'static str),
}

const TTS: &[&str] = &["tts"];
const TTS_VIDEO: &[&str] = &["tts", "sync_map", "video"];
const VIDEO: &[&str] = &["sync_map", "video"];

/// Which queue makes `what` for a chapter, from what it already has. The service runs only the
/// Python stages, so a chapter without a story needs the pipeline (Read / Story first).
fn plan(what: What, story: bool, audio: bool, pipeline_ok: bool) -> Plan {
    match what {
        What::Full if pipeline_ok => Plan::Pipeline(Step::Video),
        What::Audio if story => Plan::Service(TTS),
        What::Video | What::Full if audio => Plan::Service(VIDEO),
        What::Video | What::Full if story => Plan::Service(TTS_VIDEO),
        What::Audio if pipeline_ok => Plan::Pipeline(Step::Audio),
        What::Video if pipeline_ok => Plan::Pipeline(Step::Video),
        _ => Plan::Skip("no story yet (open Studio on the PC to read the chapter first)"),
    }
}

async fn generate(raw: axum::body::Bytes) -> ApiResult<Json<Value>> {
    // Parsed here (not with the Json extractor) so a bad body still gets a JSON error.
    let body: GenerateBody = serde_json::from_slice(&raw).map_err(|e| err(StatusCode::BAD_REQUEST, format!("Bad request body: {e}")))?;
    let what = match body.what.as_str() {
        "audio" => What::Audio,
        "video" => What::Video,
        "full" => What::Full,
        _ => return Err(err(StatusCode::BAD_REQUEST, "\"what\" must be \"audio\", \"video\" or \"full\"")),
    };
    if body.chapters.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Pick at least one chapter"));
    }
    let mode = studio_mode().await;
    let env = studio_env().await;
    let (ready, reason) = readiness(mode, env.as_ref());
    let Some(env) = env.filter(|_| ready) else {
        return Err(err(StatusCode::CONFLICT, reason.unwrap_or_else(|| "Studio isn't ready".into())));
    };
    if what != What::Audio && !env.ffmpeg {
        return Err(err(StatusCode::CONFLICT, "FFmpeg is missing on the PC, so videos can't be made yet"));
    }

    // The chapters, in reading order, with whether their story is written.
    let groups = library().await?;
    let wanted = body.chapters.clone();
    let picked: Vec<(ChapterItem, bool)> = blocking(move || {
        groups
            .into_iter()
            .flat_map(|g| g.chapters)
            .filter(|c| wanted.contains(&c.id))
            .map(|c| {
                let story = mv_recap::chapter_status(&QChapter::from_item(&c).chref()).story;
                (c, story)
            })
            .collect()
    })
    .await?;
    if picked.is_empty() {
        return Err(err(StatusCode::NOT_FOUND, "None of these chapters are downloaded"));
    }
    let mut notes: Vec<String> = vec![];
    let missing = body.chapters.len().saturating_sub(picked.len());
    if missing > 0 {
        notes.push(format!("{missing} {} not downloaded, skipped", plural(missing, "chapter is", "chapters are")));
    }

    // The pipeline runs only once it has the Studio page's context; set it like the page does.
    let mut pipeline_ok = pipeline::has_context();
    if !pipeline_ok {
        let e = env.clone();
        pipeline_ok = bridge::on_ui(move |_, cx| pipeline::set_context(pipeline::context_from_app(Some(e), cx))).await.is_ok();
    }
    if what == What::Full && !pipeline_ok {
        notes.push("The full Read → Video run isn't available yet (the app is still starting), so chapter videos were queued; the full recap is joined once they are made".into());
    }

    let prefs = mv_recap::Prefs::load();
    let mut ids: Vec<String> = vec![];
    let mut by_plan: Vec<(Plan, Vec<ChapterItem>)> = vec![];
    for (c, story) in &picked {
        match plan(what, *story, c.audio_files > 0, pipeline_ok) {
            Plan::Skip(why) => notes.push(format!("{}: {why}", c.label())),
            p => match by_plan.iter_mut().find(|(k, _)| *k == p) {
                Some((_, list)) => list.push(c.clone()),
                None => by_plan.push((p, vec![c.clone()])),
            },
        }
    }
    for (p, chapters) in by_plan {
        match p {
            Plan::Service(stages) => ids.extend(queue_service(&env, &prefs, stages, &chapters, &mut notes)),
            Plan::Pipeline(until) => {
                // captions: None keeps each chapter's own caption setting (the Video step's).
                let (_, _, already) = pipeline::enqueue(&chapters, until, None, &prefs.format, None, true);
                if already > 0 {
                    notes.push(format!("{already} {} already in the queue", plural(already, "chapter was", "chapters were")));
                }
                let q = pipeline::snapshot();
                ids.extend(chapters.iter().filter_map(|c| q.for_chapter(&c.id).filter(|i| i.state.active()).map(|i| format!("p{}", i.id))));
            }
            Plan::Skip(_) => {}
        }
    }

    // "full": once the chapters' videos are made, join each manga's videos into its full recap.
    if what == What::Full {
        let mut mangas: Vec<(String, String)> = vec![];
        for (c, _) in &picked {
            if !mangas.iter().any(|(id, _)| *id == c.manga_id) {
                mangas.push((c.manga_id.clone(), c.manga_title.clone()));
            }
        }
        for (manga_id, title) in mangas {
            let chapter_ids: Vec<String> = picked.iter().filter(|(c, _)| c.manga_id == manga_id).map(|(c, _)| c.id.clone()).collect();
            ids.push(start_recap(manga_id, title, chapter_ids));
        }
    }

    let note = (!notes.is_empty()).then(|| notes.join(". "));
    Ok(Json(json!({ "jobs": ids, "note": note })))
}

/// Queues one Python job per chapter (each with its own narrator, as Studio's Audio / Video
/// phases do) and returns the new ids.
fn queue_service(env: &Env, prefs: &mv_recap::Prefs, stages: &[&str], chapters: &[ChapterItem], notes: &mut Vec<String>) -> Vec<String> {
    let mut by_voice: Vec<(String, Vec<ChapterItem>)> = vec![];
    for c in chapters {
        let v = mv_studio::voices::voice_for_chapter(&c.id);
        match by_voice.iter_mut().find(|(g, _)| *g == v) {
            Some((_, list)) => list.push(c.clone()),
            None => by_voice.push((v, vec![c.clone()])),
        }
    }
    let mut ids = vec![];
    for (voice, chapters) in by_voice {
        if stages.contains(&"tts") {
            let info = mv_studio::tts::find_voice(&voice);
            if !info.is_some_and(|v| setup::voice_ready(&env.engines, v).usable()) {
                let label = info.map(|v| v.label).unwrap_or(voice.as_str());
                let names = chapters.iter().map(ChapterItem::label).collect::<Vec<_>>().join(", ");
                notes.push(format!("{label} isn't ready yet (get it in Settings → Audio on the PC), skipped: {names}"));
                continue;
            }
        }
        let req = NarrationRequest {
            voice,
            speed: prefs.speed,
            video: stages.contains(&"video"),
            resolution: prefs.resolution.clone(),
            format: prefs.format.clone(),
            stages: Some(stages.iter().map(|s| s.to_string()).collect()),
            force_regen: false,
            regen_pages: vec![],
            new_take: false,
        };
        match service::enqueue_with(env, &req, &chapters, None) {
            Ok(new) => {
                let n = chapters.len().saturating_sub(new.len());
                if n > 0 {
                    notes.push(format!("{n} {} already being made", plural(n, "chapter is", "chapters are")));
                }
                ids.extend(new.into_iter().map(|id| format!("s{id}")));
            }
            Err(e) => notes.push(format!("Could not queue: {e:#}")),
        }
    }
    ids
}

// ---------------------------------------------------------------- full-recap joins

#[derive(Clone, Copy, Debug, PartialEq)]
enum RecapState {
    Waiting,
    Joining,
    Done,
    Failed,
    Cancelled,
}

struct Recap {
    id: u64,
    manga_title: String,
    state: RecapState,
    error: Option<String>,
    progress: Arc<Mutex<(f32, String)>>,
    cancel: Arc<AtomicBool>,
}

static RECAPS: Mutex<Vec<Recap>> = Mutex::new(Vec::new());
static RECAP_ID: AtomicU64 = AtomicU64::new(0);
const KEEP_RECAPS: usize = 20;

fn recaps() -> std::sync::MutexGuard<'static, Vec<Recap>> {
    RECAPS.lock().unwrap_or_else(|e| e.into_inner())
}

fn set_recap(id: u64, state: RecapState, error: Option<String>) {
    if let Some(r) = recaps().iter_mut().find(|r| r.id == id) {
        r.state = state;
        r.error = error;
    }
}

/// A chapter is still being made by the pipeline or a Python job.
fn chapters_busy(ids: &[String]) -> bool {
    let q = pipeline::snapshot();
    let jobs = service::snapshot();
    ids.iter().any(|c| q.for_chapter(c).is_some_and(|i| i.state.active()) || jobs.iter().any(|j| j.chapter_id == *c && !j.state.finished()))
}

/// Waits until `chapter_ids` are made, then joins the manga's made chapter videos (reading
/// order, as Studio's Play full / Export do) into `full_recap.mp4`. Returns the job id.
fn start_recap(manga_id: String, manga_title: String, chapter_ids: Vec<String>) -> String {
    let id = RECAP_ID.fetch_add(1, Ordering::SeqCst) + 1;
    let progress = Arc::new(Mutex::new((0.0, String::new())));
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut list = recaps();
        list.push(Recap { id, manga_title, state: RecapState::Waiting, error: None, progress: progress.clone(), cancel: cancel.clone() });
        while list.len() > KEEP_RECAPS {
            match list.iter().position(|r| matches!(r.state, RecapState::Done | RecapState::Failed | RecapState::Cancelled)) {
                Some(i) => drop(list.remove(i)),
                None => break,
            }
        }
    }
    mv_ui::bg::runtime().spawn(async move {
        loop {
            if cancel.load(Ordering::Relaxed) {
                return set_recap(id, RecapState::Cancelled, None);
            }
            let ids = chapter_ids.clone();
            if !tokio::task::spawn_blocking(move || chapters_busy(&ids)).await.unwrap_or(false) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        let groups = match tokio::task::spawn_blocking(data::load).await {
            Ok(Ok(g)) => g,
            Ok(Err(e)) => return set_recap(id, RecapState::Failed, Some(format!("Could not read the library: {e:#}"))),
            Err(e) => return set_recap(id, RecapState::Failed, Some(e.to_string())),
        };
        let chapters: Vec<ChapterItem> = groups.into_iter().find(|g| g.id == manga_id).map(|g| g.chapters).unwrap_or_default();
        if !chapters.iter().any(|c| chapter_ids.contains(&c.id) && c.video.is_some()) {
            return set_recap(id, RecapState::Failed, Some("The chapter videos were not made".into()));
        }
        let files: Vec<PathBuf> = chapters.into_iter().filter_map(|c| c.video).collect();
        set_recap(id, RecapState::Joining, None);
        let (p, c) = (progress.clone(), cancel.clone());
        let res = tokio::task::spawn_blocking(move || ui_studio::join_full_recap(&manga_id, &files, p, c)).await;
        match res {
            Ok(Ok(_)) => set_recap(id, RecapState::Done, None),
            _ if cancel.load(Ordering::Relaxed) => set_recap(id, RecapState::Cancelled, None),
            Ok(Err(e)) => set_recap(id, RecapState::Failed, Some(format!("Could not join the chapter videos: {e:#}"))),
            Err(e) => set_recap(id, RecapState::Failed, Some(e.to_string())),
        }
    });
    format!("r{id}")
}

// ---------------------------------------------------------------- jobs

#[derive(Clone, Debug, PartialEq, Serialize)]
struct JobRow {
    id: String,
    title: String,
    stage: String,
    progress: f64,
    state: &'static str,
    error: Option<String>,
}

fn step_name(step: Step) -> &'static str {
    match step {
        Step::Read => "Reading the pages",
        Step::Transcript => "Writing the transcript",
        Step::Story => "Writing the story",
        Step::Audio => "Narrating",
        Step::Music => "Composing music",
        Step::Video => "Rendering the video",
    }
}

/// One Python job (Studio's Audio / Video phases, or "audio" / "video" from the phone).
fn service_row(j: &JobInfo) -> JobRow {
    let (state, error) = match &j.state {
        JobView::Queued => ("queued", None),
        JobView::Running => ("running", None),
        JobView::Done => ("done", None),
        JobView::Failed(e) => ("failed", Some(e.clone())),
        JobView::Cancelled => ("cancelled", None),
    };
    let stage = match (&j.state, j.stage.as_deref()) {
        (JobView::Running, Some(s)) => service::stage_label(s).to_string(),
        (JobView::Queued, _) => "Waiting".into(),
        (JobView::Done, _) => "Done".into(),
        (JobView::Cancelled, _) => "Cancelled".into(),
        _ => j.status.clone(),
    };
    JobRow { id: format!("s{}", j.id), title: format!("{} · {}", j.manga_title, short_chapter(&j.chapter_label)), stage, progress: j.progress().clamp(0.0, 1.0), state, error }
}

/// One chapter of the pipeline queue; `job` is the Python job its running step waits on.
fn pipeline_row(it: &QueueItem, queue_paused: bool, job: Option<&JobInfo>) -> JobRow {
    let n = (it.until + 1).min(Step::ALL.len()) as f64;
    let step = it.current_step();
    let running_stage = || {
        job.filter(|j| j.state == JobView::Running)
            .and_then(|j| j.stage.as_deref())
            .map(|s| service::stage_label(s).to_string())
            .or(step.map(|s| step_name(s).to_string()))
            .unwrap_or_else(|| "Starting".into())
    };
    let (state, stage) = match it.state {
        ItemState::Running => ("running", running_stage()),
        ItemState::Queued if queue_paused => ("queued", "Paused".into()),
        ItemState::Queued => ("queued", "Waiting".into()),
        ItemState::Paused => ("queued", it.note.as_ref().map(|n| format!("Paused: {n}")).unwrap_or_else(|| "Paused".into())),
        ItemState::Done => ("done", "Done".into()),
        ItemState::Failed => ("failed", step.map(|s| step_name(s).to_string()).unwrap_or_else(|| "Failed".into())),
    };
    // Finished steps, plus how far the running step's job is.
    let partial = match (it.state, job) {
        (ItemState::Running, Some(j)) if !j.state.finished() => j.progress(),
        _ => 0.0,
    };
    let progress = if it.state == ItemState::Done { 1.0 } else { ((it.finished_steps() as f64 + partial) / n).clamp(0.0, 1.0) };
    let c = &it.chapter;
    JobRow {
        id: format!("p{}", it.id),
        title: format!("{} · {}", c.manga_title, chapter_name(c.number, c.title.as_deref())),
        stage,
        progress,
        state,
        error: if it.state == ItemState::Failed { it.error.clone() } else { None },
    }
}

fn recap_row(r: &Recap) -> JobRow {
    let (p, label) = r.progress.lock().map(|s| s.clone()).unwrap_or_default();
    let (state, stage, progress) = match r.state {
        RecapState::Waiting => ("queued", "Waiting for the chapter videos".to_string(), 0.0),
        RecapState::Joining => ("running", if label.is_empty() { "Joining the chapter videos".into() } else { label }, p as f64),
        RecapState::Done => ("done", "Done".into(), 1.0),
        RecapState::Failed => ("failed", "Joining the chapter videos".into(), p as f64),
        RecapState::Cancelled => ("cancelled", "Cancelled".into(), p as f64),
    };
    JobRow { id: format!("r{}", r.id), title: format!("{} · Full recap", r.manga_title), stage, progress: progress.clamp(0.0, 1.0), state, error: r.error.clone() }
}

/// Every job: the pipeline queue, then Python jobs it did not start (newest first), then joins
/// (newest first).
fn all_jobs() -> Vec<JobRow> {
    let q = pipeline::snapshot();
    let jobs = service::snapshot();
    let mut rows: Vec<JobRow> = q.items.iter().map(|it| pipeline_row(it, q.paused, it.job.and_then(|id| jobs.iter().find(|j| j.id == id)))).collect();
    // A pipeline chapter's own audio / video jobs show as its steps, not twice.
    rows.extend(jobs.iter().filter(|j| !q.items.iter().any(|it| it.chapter.id == j.chapter_id)).map(service_row));
    rows.extend(recaps().iter().rev().map(recap_row));
    rows
}

async fn jobs() -> ApiResult<Json<Vec<JobRow>>> {
    // The pipeline queue loads from disk on first use.
    Ok(Json(blocking(all_jobs).await?))
}

/// Stops a job: `p<n>` leaves the pipeline queue, `s<n>` a Python job, `r<n>` a recap join.
/// Finished jobs are left as they are. False when there is no such job.
fn cancel_job(id: &str) -> bool {
    let (kind, n) = id.split_at(id.char_indices().nth(1).map_or(id.len(), |(i, _)| i));
    let Ok(n) = n.parse::<u64>() else { return false };
    match kind {
        "p" => match pipeline::snapshot().get(n).map(|i| i.state.active()) {
            Some(true) => {
                pipeline::cancel(n);
                true
            }
            Some(false) => true,
            None => false,
        },
        "s" => service::job(n).is_some_and(|j| j.state.finished() || service::cancel(n)),
        "r" => recaps().iter().find(|r| r.id == n).map(|r| r.cancel.store(true, Ordering::Relaxed)).is_some(),
        _ => false,
    }
}

async fn cancel(UrlPath(id): UrlPath<String>) -> ApiResult<Json<Value>> {
    if blocking(move || cancel_job(&id)).await? {
        Ok(Json(json!({})))
    } else {
        Err(err(StatusCode::NOT_FOUND, "No such job"))
    }
}

// ---------------------------------------------------------------- media

/// File types served from the projects folder (narration clips, videos).
const MEDIA_EXTS: [&str; 5] = ["mp4", "wav", "mp3", "ogg", "m4a"];

fn is_media(p: &Path) -> bool {
    p.extension().and_then(|x| x.to_str()).is_some_and(|x| MEDIA_EXTS.iter().any(|m| x.eq_ignore_ascii_case(m)))
}

/// The opaque id of `rel` (a path relative to the projects folder).
fn encode_id(rel: &Path) -> Option<String> {
    let parts: Vec<&str> = rel
        .components()
        .map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect::<Option<_>>()?;
    (!parts.is_empty()).then(|| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(parts.join("/")))
}

/// The media file `id` names inside `base`, or `None`: only plain relative names (no `..`,
/// roots, drive letters or streams), and the resolved file (symlinks followed) must still sit
/// inside `base`.
fn resolve_id(base: &Path, id: &str) -> Option<PathBuf> {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(id.trim_end_matches('=')).ok()?;
    let rel = String::from_utf8(raw).ok()?;
    if rel.is_empty() || rel.contains(['\\', ':', '\0']) {
        return None;
    }
    let mut path = base.to_path_buf();
    for seg in rel.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return None;
        }
        let mut comps = Path::new(seg).components();
        if !matches!((comps.next(), comps.next()), (Some(Component::Normal(_)), None)) {
            return None;
        }
        path.push(seg);
    }
    let real = path.canonicalize().ok()?;
    let root = base.canonicalize().ok()?;
    (real.starts_with(&root) && real.is_file() && is_media(&real)).then_some(real)
}

#[derive(Clone, Debug, Serialize)]
struct MediaItem {
    id: String,
    title: String,
    manga: String,
    chapter: Option<String>,
    kind: &'static str,
    url: String,
    size: u64,
    /// Unix seconds.
    modified: u64,
    /// Narration clips: the page (1-based) the clip voices.
    page: Option<u32>,
    /// The manga's cover (lock-screen artwork), when the library has one.
    cover: Option<String>,
}

/// `page_0007.wav` -> 8.
fn clip_page(name: &str) -> Option<u32> {
    let stem = Path::new(name).file_stem()?.to_str()?;
    stem.strip_prefix("page_")?.parse::<u32>().ok().map(|n| n + 1)
}

fn file_info(p: &Path) -> Option<(u64, u64)> {
    let m = std::fs::metadata(p).ok().filter(|m| m.is_file() && m.len() > 0)?;
    let t = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    Some((m.len(), t))
}

fn sorted_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    v.sort();
    v
}

/// Names for the folders under `projects`: manga folder -> title, (manga, chapter) folders ->
/// chapter name, manga folder -> cover URL.
type Names = (HashMap<String, String>, HashMap<(String, String), String>, HashMap<String, String>);

fn names(groups: &[MangaGroup]) -> Names {
    let (mut manga, mut chapters, mut covers) = (HashMap::new(), HashMap::new(), HashMap::new());
    for g in groups {
        let m = mv_core::paths::safe_name(&g.id);
        manga.insert(m.clone(), g.title.clone());
        if g.cover.is_some() {
            covers.insert(m.clone(), cover_url(&g.id));
        }
        for c in &g.chapters {
            chapters.insert((m.clone(), mv_core::paths::safe_name(&c.id)), chapter_name(c.number, c.title.as_deref()));
        }
    }
    (manga, chapters, covers)
}

/// Everything Studio made under `base`, newest first (a chapter's video before its clips, the
/// clips in page order).
fn scan_media(base: &Path, names: &Names) -> Vec<MediaItem> {
    let item = |rel: PathBuf, title: String, manga: &str, chapter: Option<&str>, kind: &'static str, page: Option<u32>| -> Option<MediaItem> {
        let (size, modified) = file_info(&base.join(&rel))?;
        let id = encode_id(&rel)?;
        let cover = rel.components().next().and_then(|c| c.as_os_str().to_str()).and_then(|m| names.2.get(m)).cloned();
        Some(MediaItem { url: format!("/api/media/{id}"), id, title, manga: manga.to_string(), chapter: chapter.map(str::to_string), kind, size, modified, page, cover })
    };
    let mut groups: Vec<Vec<MediaItem>> = vec![];
    for mdir in sorted_dirs(base) {
        let Some(m) = mdir.file_name().and_then(|n| n.to_str()).map(str::to_string) else { continue };
        let manga = names.0.get(&m).cloned().unwrap_or_else(|| m.clone());
        if let Some(it) = item(PathBuf::from(&m).join("full_recap.mp4"), format!("{manga} · Full recap"), &manga, None, "video", None) {
            groups.push(vec![it]);
        }
        for cdir in sorted_dirs(&mdir) {
            let Some(c) = cdir.file_name().and_then(|n| n.to_str()).map(str::to_string) else { continue };
            let chapter = names.1.get(&(m.clone(), c.clone())).cloned().unwrap_or_else(|| c.clone());
            let rel = PathBuf::from(&m).join(&c);
            let mut g = vec![];
            g.extend(item(rel.join(data::VIDEO_FILE), format!("{manga} · {chapter}"), &manga, Some(&chapter), "video", None));
            let mut clips: Vec<(Option<u32>, String)> = std::fs::read_dir(cdir.join("audio"))
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| e.file_name().to_str().map(str::to_string))
                .filter(|n| !n.starts_with('.') && is_media(Path::new(n)) && !n.to_lowercase().ends_with(".mp4"))
                .map(|n| (clip_page(&n), n))
                .collect();
            clips.sort();
            for (page, name) in clips {
                let title = match page {
                    Some(p) => format!("{manga} · {chapter} · Page {p}"),
                    None => format!("{manga} · {chapter} · {name}"),
                };
                g.extend(item(rel.join("audio").join(&name), title, &manga, Some(&chapter), "audio", page));
            }
            if !g.is_empty() {
                groups.push(g);
            }
        }
    }
    groups.sort_by_key(|g| std::cmp::Reverse(g.iter().map(|i| i.modified).max().unwrap_or(0)));
    groups.into_iter().flatten().collect()
}

async fn media_list() -> ApiResult<Json<Vec<MediaItem>>> {
    // Names come from the library; files of a manga no longer in it list under their folder name.
    let groups = blocking(data::load).await?.unwrap_or_default();
    Ok(Json(blocking(move || scan_media(&mv_core::paths::studio_projects(), &names(&groups))).await?))
}

/// `inline` / `attachment` with an ASCII fallback name and the UTF-8 one.
fn disposition(download: bool, name: &str) -> HeaderValue {
    let ascii: String = name.chars().map(|c| if (c.is_ascii_graphic() && c != '"' && c != '\\') || c == ' ' { c } else { '_' }).collect();
    let kind = if download { "attachment" } else { "inline" };
    HeaderValue::from_str(&format!("{kind}; filename=\"{ascii}\"; filename*=UTF-8''{}", pct(name))).unwrap_or_else(|_| HeaderValue::from_static("inline"))
}

/// A download name like "Solo Leveling - Chapter 3.mp4" for a file under the projects folder.
fn download_name(real: &Path, base: &Path, names: &Names) -> Option<String> {
    let rel = real.strip_prefix(base.canonicalize().ok()?).ok()?;
    let parts: Vec<&str> = rel.components().filter_map(|c| c.as_os_str().to_str()).collect();
    let ext = real.extension()?.to_str()?;
    let manga = names.0.get(*parts.first()?).cloned().unwrap_or_else(|| parts[0].to_string());
    let chapter = |m: &str, c: &str| names.1.get(&(m.to_string(), c.to_string())).cloned().unwrap_or_else(|| c.to_string());
    let label = match parts.as_slice() {
        [_, "full_recap.mp4"] => format!("{manga} - Full recap"),
        [m, c, f] if *f == data::VIDEO_FILE => format!("{manga} - {}", chapter(m, c)),
        [m, c, "audio", f] => format!("{manga} - {} - {}", chapter(m, c), clip_page(f).map(|p| format!("Page {p}")).unwrap_or_else(|| f.to_string())),
        _ => return None,
    };
    let safe: String = label.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { ' ' } else { c }).collect();
    Some(format!("{}.{ext}", safe.split_whitespace().collect::<Vec<_>>().join(" ")))
}

async fn media_file(UrlPath(id): UrlPath<String>, Query(q): Query<HashMap<String, String>>, req: Request) -> ApiResult<Response> {
    let base = mv_core::paths::studio_projects();
    let download = q.get("download").is_some_and(|v| v == "1" || v == "true");
    let (path, name) = blocking(move || {
        let path = resolve_id(&base, &id)?;
        let fallback = path.file_name().and_then(|n| n.to_str()).unwrap_or("media").to_string();
        let name = if download { data::load().ok().and_then(|g| download_name(&path, &base, &names(&g))).unwrap_or(fallback) } else { fallback };
        Some((path, name))
    })
    .await?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "No such file"))?;
    // ServeFile answers Range / If-Modified-Since / HEAD and sets the type from the extension.
    let res = tower_http::services::ServeFile::new(&path).try_call(req).await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("Could not read the file: {e}")))?;
    let (mut parts, body) = res.into_parts();
    parts.headers.insert(header::CONTENT_DISPOSITION, disposition(download, &name));
    parts.headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    Ok(Response::from_parts(parts, axum::body::Body::new(body)))
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use ui_studio::pipeline::StepState;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mv-phone-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"data").unwrap();
    }

    fn id_of(s: &str) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s)
    }

    #[test]
    fn opaque_ids_round_trip_and_stay_inside() {
        let d = temp_dir("ids");
        let base = d.join("projects");
        put(&base.join("m1/c1/video_final.mp4"));
        put(&base.join("m1/c1/audio/page_0000.wav"));
        put(&base.join("m1/c1/story.json"));
        put(&d.join("secret.mp4"));

        let id = encode_id(Path::new("m1/c1/video_final.mp4")).unwrap();
        assert_eq!(resolve_id(&base, &id).unwrap(), base.join("m1/c1/video_final.mp4").canonicalize().unwrap());
        assert!(resolve_id(&base, &encode_id(Path::new("m1/c1/audio/page_0000.wav")).unwrap()).is_some());
        // Padding from other encoders is fine.
        assert!(resolve_id(&base, &format!("{id}==")).is_some());

        for bad in ["../secret.mp4", "m1/../../secret.mp4", "m1/./c1/video_final.mp4", "/m1/c1/video_final.mp4", "m1//c1/video_final.mp4", "m1\\c1\\video_final.mp4", "C:/x.mp4", "m1/c1/story.json", "m1/c1", "", "m1/c1/missing.mp4"] {
            assert!(resolve_id(&base, &id_of(bad)).is_none(), "{bad} must be rejected");
        }
        assert!(resolve_id(&base, "not base64 !!").is_none());
        let abs = d.join("secret.mp4");
        assert!(resolve_id(&base, &id_of(&abs.to_string_lossy())).is_none());
        assert!(encode_id(Path::new("../x.mp4")).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_out_of_the_projects_folder_are_rejected() {
        let d = temp_dir("link");
        let base = d.join("projects");
        put(&d.join("outside/x.mp4"));
        std::fs::create_dir_all(base.join("m")).unwrap();
        std::os::unix::fs::symlink(d.join("outside"), base.join("m/c")).unwrap();
        assert!(resolve_id(&base, &id_of("m/c/x.mp4")).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn media_lists_videos_clips_and_recaps() {
        let d = temp_dir("scan");
        let base = d.join("projects");
        put(&base.join("m1/full_recap.mp4"));
        put(&base.join("m1/c1/video_final.mp4"));
        put(&base.join("m1/c1/audio/page_0001.wav"));
        put(&base.join("m1/c1/audio/page_0000.wav"));
        put(&base.join("m1/c1/audio/.loudness.json"));
        put(&base.join("m1/c1/audio/narration_params.json"));
        let mut names: Names = Default::default();
        names.0.insert("m1".into(), "Solo".into());
        names.1.insert(("m1".into(), "c1".into()), "Chapter 3".into());
        names.2.insert("m1".into(), cover_url("m 1"));
        let items = scan_media(&base, &names);
        let titles: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(items.len(), 4, "{titles:?}");
        assert!(titles.contains(&"Solo · Full recap"));
        let pos = |t: &str| titles.iter().position(|x| *x == t).unwrap();
        assert!(pos("Solo · Chapter 3") < pos("Solo · Chapter 3 · Page 1"));
        assert!(pos("Solo · Chapter 3 · Page 1") < pos("Solo · Chapter 3 · Page 2"));
        for i in &items {
            assert_eq!(i.url, format!("/api/media/{}", i.id));
            assert!(resolve_id(&base, &i.id).is_some());
            assert_eq!(i.cover.as_deref(), Some("/api/manga/m%201/cover"));
        }
        let real = resolve_id(&base, &encode_id(Path::new("m1/c1/audio/page_0001.wav")).unwrap()).unwrap();
        assert_eq!(download_name(&real, &base, &names).as_deref(), Some("Solo - Chapter 3 - Page 2.wav"));
        let recap = resolve_id(&base, &encode_id(Path::new("m1/full_recap.mp4")).unwrap()).unwrap();
        assert_eq!(download_name(&recap, &base, &names).as_deref(), Some("Solo - Full recap.mp4"));
        let _ = std::fs::remove_dir_all(&d);
    }

    fn job(state: JobView, stage: Option<&str>, percent: f64) -> JobInfo {
        JobInfo {
            id: 7,
            chapter_id: "c1".into(),
            manga_title: "Solo".into(),
            chapter_label: "Chapter 3 · The Duel".into(),
            output_dir: PathBuf::new(),
            voice: "v".into(),
            stages: vec!["tts".into(), "sync_map".into()],
            state,
            stage: stage.map(str::to_string),
            stage_percent: percent,
            status: "Waiting in queue".into(),
            logs: VecDeque::new(),
            narrator: None,
            regen_pages: vec![],
        }
    }

    #[test]
    fn service_jobs_map_to_rows() {
        let r = service_row(&job(JobView::Running, Some("tts"), 50.0));
        assert_eq!((r.id.as_str(), r.title.as_str(), r.stage.as_str(), r.state), ("s7", "Solo · Chapter 3", "Narrating", "running"));
        assert!((r.progress - 0.25).abs() < 1e-9);
        let f = service_row(&job(JobView::Failed("boom".into()), Some("tts"), 10.0));
        assert_eq!((f.state, f.error.as_deref()), ("failed", Some("boom")));
        assert_eq!(service_row(&job(JobView::Done, None, 0.0)).progress, 1.0);
        assert_eq!(service_row(&job(JobView::Cancelled, None, 0.0)).state, "cancelled");
    }

    fn item(state: ItemState) -> QueueItem {
        let mut steps = [StepState::Pending; 6];
        steps[0] = StepState::Skipped;
        steps[1] = StepState::Skipped;
        steps[2] = StepState::Done;
        QueueItem {
            id: 3,
            chapter: QChapter { id: "c1".into(), manga_id: "m1".into(), manga_title: "Solo".into(), number: Some(3.0), title: Some("The Duel".into()), pages_dir: PathBuf::new(), output_dir: PathBuf::new() },
            until: 5,
            force: None,
            format: mv_recap::FORMAT_LANDSCAPE.into(),
            captions: None,
            state,
            steps,
            current: Some(3),
            error: Some("No AI model is picked".into()),
            note: None,
            job: Some(7),
        }
    }

    #[test]
    fn pipeline_items_map_to_rows() {
        let j = job(JobView::Running, Some("tts"), 50.0);
        let r = pipeline_row(&item(ItemState::Running), false, Some(&j));
        assert_eq!((r.id.as_str(), r.title.as_str(), r.stage.as_str(), r.state), ("p3", "Solo · Chapter 3", "Narrating", "running"));
        // 3 of 6 steps finished plus a quarter of the running one.
        assert!((r.progress - (3.25 / 6.0)).abs() < 1e-9, "{}", r.progress);
        assert_eq!(r.error, None);
        assert_eq!(pipeline_row(&item(ItemState::Running), false, None).stage, "Narrating");
        assert_eq!(pipeline_row(&item(ItemState::Queued), true, None).stage, "Paused");
        let f = pipeline_row(&item(ItemState::Failed), false, None);
        assert_eq!((f.state, f.error.as_deref()), ("failed", Some("No AI model is picked")));
        let d = pipeline_row(&item(ItemState::Done), false, None);
        assert_eq!((d.state, d.progress), ("done", 1.0));
    }

    #[test]
    fn plans_follow_what_a_chapter_has() {
        assert_eq!(plan(What::Audio, true, false, false), Plan::Service(TTS));
        assert_eq!(plan(What::Audio, false, false, true), Plan::Pipeline(Step::Audio));
        assert!(matches!(plan(What::Audio, false, false, false), Plan::Skip(_)));
        assert_eq!(plan(What::Video, true, true, true), Plan::Service(VIDEO));
        assert_eq!(plan(What::Video, true, false, true), Plan::Service(TTS_VIDEO));
        assert_eq!(plan(What::Video, false, false, true), Plan::Pipeline(Step::Video));
        assert_eq!(plan(What::Full, false, false, true), Plan::Pipeline(Step::Video));
        // Without the pipeline, "full" falls back to chapter videos.
        assert_eq!(plan(What::Full, true, true, false), Plan::Service(VIDEO));
        assert!(matches!(plan(What::Full, false, false, false), Plan::Skip(_)));
    }

    #[test]
    fn small_helpers() {
        assert_eq!(pct("a b/é"), "a%20b%2F%C3%A9");
        assert_eq!(short_chapter("Chapter 3 · The Duel"), "Chapter 3");
        assert_eq!(short_chapter("Oneshot · Part"), "Oneshot · Part");
        assert_eq!(clip_page("page_0007.wav"), Some(8));
        assert_eq!(chapter_name(Some(3.0), Some("The Duel")), "Chapter 3");
        assert_eq!(chapter_name(None, Some("Oneshot")), "Oneshot");
        let h = disposition(true, "Solo – 1.mp4");
        assert!(h.to_str().unwrap().starts_with("attachment; filename=\"Solo _ 1.mp4\"; filename*=UTF-8''Solo%20%E2%80%93%201.mp4"));
        assert!(!cancel_job("x1") && !cancel_job("p") && !cancel_job(""));
    }
}
