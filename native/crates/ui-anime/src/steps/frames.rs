//! Step 5, Frames (design 9.8) [frames]: one still per shot. W3-S4.
//!
//! The episode's shots are a grid of 16:9 tiles grouped by scene (the hero), with a filmstrip
//! list and its filters on the left. Opening a shot shows its picture large with the latest
//! variants to choose from, Approve / Reject / Lock / Upload your own, the history (restore and
//! compare) and, when an input changed since the picture was made, Update / Keep. The shared
//! Generation panel sits on the right, filled in by the engine (model, the references the slot
//! logic chose with their jobs, the compiled prompt); every edit there is saved into the shot.
//!
//! "Draw all frames" queues only shots with no picture yet (none, or rejected) that are not
//! locked; a picture waiting for review is left alone. Nothing is
//! drawn unless the user asks: every generation goes through the engine's queue.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_anime::frames::{self, Prepared};
use mv_anime::queue::{self, Job, JobKind, Lane};
use mv_anime::{hash, BibleIndex, Card, GenSpec, ProjectSettings, Scene, Shot, ShotSize, Status, Store, Take, TakeKind, TakeMeta, TakeSource, Target};
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::{
    btn, confirm_dialog, empty_view, error_view, filter_chip, icon_button, icon_ph, inline_alert, menu_button, ph, ph_icon, progress_bar, size, skeleton_block, skeleton_line, spinner, tag, tip,
    toast, AlertKind, ButtonKind, MenuItem, Toast, Tokens, Tone, Weight,
};

use super::shared::{bible_notice, cached_take, latest_round, scene_order, shot_secs, TakeCache};
use super::{Step, StepCtx};
use crate::backends;
use crate::gen_panel::{busy_of, Busy, GenPanel};
use crate::Turns;
use crate::history::{history_button, History};
use crate::queue_strip::{web_held, Eta};
use crate::style::{COLUMN_HEAD_H, POLL_MS, TILE_W};
use crate::widgets::{
    card, card_marks, column_body, column_head, count, empty_gallery, error_block, fmt_duration, hover_actions, info_line, is_narrow, label_caps, label_row, section, selectable, spec, status_label,
    thumb, workspace, LineTone,
};

/// Height of a gallery tile's picture (16:9).
const TILE_H: f32 = TILE_W * 9. / 16.;
/// The filmstrip thumbnails of the list (16:9).
const LIST_THUMB: (f32, f32) = (48., 27.);
/// The open shot's picture is never wider than four tiles.
const STILL_MAX_W: f32 = TILE_W * 4.;
/// Newest takes read per shot to find its latest round of variants.
const ROUND_LOOK: usize = frames::MAX_VARIANTS as usize + 1;

// ---------------------------------------------------------------- pure parts

/// A shot size in plain words.
fn size_label(s: ShotSize) -> &'static str {
    match s {
        ShotSize::Els => "Extreme wide",
        ShotSize::Ls => "Wide",
        ShotSize::Fs => "Full",
        ShotSize::Cowboy => "Cowboy",
        ShotSize::Ms => "Medium",
        ShotSize::Mcu => "Medium close",
        ShotSize::Cu => "Close-up",
        ShotSize::Ecu => "Extreme close",
        ShotSize::Insert => "Insert",
        ShotSize::Ots => "Over the shoulder",
        ShotSize::TwoShot => "Two-shot",
        ShotSize::Pov => "Point of view",
        ShotSize::Overhead => "Overhead",
        ShotSize::Unknown => "Shot",
    }
}

/// "2.8 s"; nothing for an unknown length.
fn secs_label(s: f32) -> Option<String> {
    (s.is_finite() && s > 0.).then(|| format!("{s:.1} s"))
}

/// The card has a picture the user approved.
fn is_approved(card: &Card) -> bool {
    card.status == Status::Approved && card.selected.is_some()
}

/// A picture (drawn or uploaded) waits for the user's review.
fn waiting_review(card: &Card) -> bool {
    card.status == Status::NeedsReview && card.selected.is_some()
}

/// "Draw all frames" may draw this card: no usable picture yet (none, or rejected). Approved,
/// locked and waiting-for-review pictures are left as they are.
fn needs_drawing(card: &Card) -> bool {
    !is_approved(card) && !card.locked && !waiting_review(card)
}

/// The filters of the list (they apply to the grid too).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Filter {
    #[default]
    All,
    ToDraw,
    Review,
    Outdated,
    Approved,
}

impl Filter {
    const ALL: [Filter; 5] = [Filter::All, Filter::ToDraw, Filter::Review, Filter::Outdated, Filter::Approved];

    fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::ToDraw => "To draw",
            Filter::Review => "Needs review",
            Filter::Outdated => "Outdated",
            Filter::Approved => "Approved",
        }
    }

    fn passes(self, card: &Card, outdated: bool) -> bool {
        match self {
            Filter::All => true,
            // Exactly what "Draw all frames" draws: a picture waiting for review (drawn or
            // uploaded) counts under "Needs review", not here.
            Filter::ToDraw => needs_drawing(card),
            Filter::Review => waiting_review(card),
            Filter::Outdated => outdated,
            Filter::Approved => is_approved(card),
        }
    }
}

/// The shot before or after `current` in `ids`.
fn neighbour(ids: &[String], current: &str, forward: bool) -> Option<String> {
    let i = ids.iter().position(|s| s == current)?;
    if forward {
        ids.get(i + 1).cloned()
    } else {
        i.checked_sub(1).and_then(|j| ids.get(j)).cloned()
    }
}

/// The next outdated shot after `current` (wrapping round), never `current` itself.
fn next_outdated(order: &[(String, bool)], current: &str) -> Option<String> {
    let start = order.iter().position(|(id, _)| id == current).map(|i| i + 1).unwrap_or(0);
    (0..order.len()).map(|k| &order[(start + k) % order.len()]).find(|(id, outdated)| *outdated && id != current).map(|(id, _)| id.clone())
}

/// The time line of a confirm dialog.
fn time_note(n: usize, mean: Option<f32>) -> String {
    match mean {
        Some(m) if m.is_finite() && m > 0. => format!("About {} in all.", fmt_duration((m * n as f32).round() as u64)),
        _ => "The time is shown once the first frame is done.".into(),
    }
}

/// The body of the "Draw all frames" confirm dialog; `waiting`: shots whose picture waits for
/// review (left alone, and said so when there are any).
fn draw_note(n: usize, waiting: usize, variants: u8, mean: Option<f32>) -> String {
    let left = match waiting {
        0 => String::new(),
        1 => "1 shot already has a picture waiting for your review and is left as it is. ".into(),
        w => format!("{w} shots already have a picture waiting for your review and are left as they are. "),
    };
    format!(
        "{left}MangaVox draws {} on Magnific, one at a time, {} each, and only while Magnific shows them as credit-free. {} Approved and locked frames are left as they are.",
        count(n, "shot"),
        count(variants.max(1) as usize, "picture"),
        time_note(n, mean)
    )
}

/// The picture types a user may use as a frame.
fn frame_ext(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_string_lossy().to_lowercase().as_str() {
        "png" => Some("png"),
        "jpg" | "jpeg" => Some("jpg"),
        "webp" => Some("webp"),
        _ => None,
    }
}

/// Where each shot's latest frame job stands (shot id → state), from the Web lane's jobs.
fn busy_map(jobs: &[Job], project: &str, episode: &str) -> HashMap<String, Busy> {
    let mut shots: Vec<&str> = jobs
        .iter()
        .filter(|j| j.project == project)
        .filter_map(|j| match &j.kind {
            JobKind::Frame { episode: e, shot, .. } if e == episode => Some(shot.as_str()),
            _ => None,
        })
        .collect();
    shots.sort_unstable();
    shots.dedup();
    shots.into_iter().map(|s| (s.to_string(), busy_of(jobs, project, &Target::ShotFrame { episode: episode.to_string(), shot: s.to_string() }))).collect()
}

fn in_queue(b: Option<&Busy>) -> bool {
    matches!(b, Some(Busy::Waiting { .. } | Busy::Working { .. }))
}

/// A background result as an error text, if it failed.
fn failure(res: anyhow::Result<anyhow::Result<()>>) -> Option<String> {
    match res {
        Ok(Ok(())) => None,
        Ok(Err(e)) | Err(e) => Some(format!("{e:#}")),
    }
}

// ---------------------------------------------------------------- reading the episode

/// One shot as the screen shows it.
#[derive(Clone)]
struct Row {
    id: String,
    /// The scene file holding the shot.
    scene: String,
    /// "3.4".
    number: String,
    shot: Shot,
    /// The file of the picture in use.
    still: Option<PathBuf>,
    outdated: bool,
    /// The next picture as the engine would make it now (spec, compiled prompt, request).
    prepared: Prepared,
    /// The latest round of pictures (take id, file), oldest first.
    round: Vec<(String, Option<PathBuf>)>,
    /// A take of the latest round made from the current request (what "Update" produced).
    fresh: Option<String>,
}

impl Row {
    /// Keeps what the screen shows in step with an edit made here, before the files are read again.
    fn after_edit(&mut self) {
        if self.shot.frame.locked {
            self.outdated = false;
        }
        if let Some(sel) = self.shot.frame.selected.as_deref() {
            if let Some((_, p)) = self.round.iter().find(|(id, _)| id == sel) {
                self.still = p.clone();
            }
        }
    }
}

struct SceneGroup {
    key: String,
    number: usize,
    title: String,
    rows: std::ops::Range<usize>,
}

struct Data {
    rows: Vec<Row>,
    scenes: Vec<SceneGroup>,
    has_story: bool,
    bible_approved: bool,
    bible_missing: Vec<String>,
    /// Pictures per shot when "Draw all frames" runs (the project's setting).
    variants: u8,
}

enum Load {
    Loading,
    Ready(Data),
    Failed(String),
}

#[cfg(feature = "bible")]
fn bible_missing(bible: &BibleIndex) -> Vec<String> {
    mv_anime::bible::gate_missing(bible)
}

#[cfg(not(feature = "bible"))]
fn bible_missing(_bible: &BibleIndex) -> Vec<String> {
    Vec::new()
}

/// Reads the episode's shots and works out each one's picture, request and state. Blocking.
fn load(store: &Store, ep: &str, cache: &TakeCache) -> anyhow::Result<Data> {
    let series = store.series()?;
    let episode = store.episode(ep)?;
    let story = store.story(ep)?;
    let all = store.all_shots(ep)?;
    let variants = if series.settings.variants == 0 { frames::DEFAULT_VARIANTS } else { series.settings.variants.min(frames::MAX_VARIANTS) };
    let mut data = Data { rows: Vec::new(), scenes: Vec::new(), has_story: !story.scenes.is_empty(), bible_approved: series.bible_approved.is_some(), bible_missing: Vec::new(), variants };
    let planned: Vec<String> = all.iter().filter(|(_, v)| !v.is_empty()).map(|(k, _)| k.clone()).collect();
    if planned.is_empty() {
        return Ok(data);
    }
    let bible = store.bible()?;
    if !data.bible_approved {
        data.bible_missing = bible_missing(&bible);
    }
    let story_ids: Vec<String> = story.scenes.iter().map(|s| s.id.clone()).collect();
    for (i, key) in scene_order(&episode.scene_order, &story_ids, &planned).iter().enumerate() {
        let Some(shots) = all.get(key).filter(|v| !v.is_empty()) else { continue };
        // The pictures the user approved as they are (the engine's record in the scene file).
        let file = store.scene_shots(ep, key).ok();
        let scene = story.scenes.iter().find(|s| &s.id == key).cloned().unwrap_or_else(|| Scene { id: key.clone(), ..Default::default() });
        let start = data.rows.len();
        for (j, shot) in shots.iter().enumerate() {
            data.rows.push(row_of(store, cache, key, format!("{}.{}", i + 1, j + 1), shot, &scene, &bible, &series.settings, file.as_ref()));
        }
        data.scenes.push(SceneGroup { key: key.clone(), number: i + 1, title: scene.title.trim().to_string(), rows: start..data.rows.len() });
    }
    Ok(data)
}

#[allow(clippy::too_many_arguments)]
fn row_of(store: &Store, cache: &TakeCache, scene_key: &str, number: String, shot: &Shot, scene: &Scene, bible: &BibleIndex, settings: &ProjectSettings, file: Option<&mv_anime::SceneShots>) -> Row {
    let prepared = frames::prepare_shot(shot, scene, bible, settings, store);
    let request_now = hash::request_hash(&prepared.request);
    let card = &shot.frame;
    let selected = card.selected.as_deref().and_then(|id| cached_take(store, cache, id));
    let still = match (&selected, card.selected.as_deref()) {
        (Some(t), _) => Some(store.path(&t.file)).filter(|p| p.is_file()),
        (None, Some(id)) => store.take_file(id),
        _ => None,
    };
    // The engine's rule: an upload is never outdated; an approved picture whose accepted request
    // is the current one is not either.
    let accepted = card.selected.as_deref().zip(file).and_then(|(take, f)| frames::accepted(f, take));
    let outdated = frames::outdated_given(card, selected.as_ref(), &request_now, accepted.as_deref());
    // The newest takes, oldest first, to find the latest round.
    let mut recent: Vec<Take> = card.takes.iter().rev().take(ROUND_LOOK).filter_map(|id| cached_take(store, cache, id)).collect();
    recent.reverse();
    let pairs: Vec<(String, Option<String>)> = recent.iter().map(|t| (t.id.clone(), t.batch.clone())).collect();
    let ids = latest_round(&pairs);
    let in_round: Vec<&Take> = recent.iter().filter(|t| ids.contains(&t.id)).collect();
    let fresh = in_round.iter().find(|t| matches!(t.target, Target::ShotFrame { .. }) && t.request_hash == request_now).map(|t| t.id.clone());
    let round = in_round.iter().map(|t| (t.id.clone(), Some(store.path(&t.file)).filter(|p| p.is_file()))).collect();
    Row { id: shot.id.clone(), scene: scene_key.to_string(), number, shot: shot.clone(), still, outdated, prepared, round, fresh }
}

/// One writer at a time for this screen's edits, so two quick changes to shots of the same scene
/// never overwrite each other (each reads the scene file fresh under it).
type WriteLock = Arc<Mutex<()>>;

/// The largest picture a user may hand in, on a side and in bytes.
const UPLOAD_MAX_SIDE: u32 = 8192;
const UPLOAD_MAX_BYTES: u64 = 50_000_000;

/// A picture's settings were saved in the background: a job for it added after the save began
/// (`since`) that already started may have read the old settings, so it starts over with the
/// saved ones ("Generate" right after a change always uses it).
fn restart_if_started(project: &str, target: &Target, since: u64) {
    let jobs: Vec<Job> = queue::snapshot().jobs.into_iter().filter(|j| j.lane == Lane::Web).collect();
    if let Busy::Working { id, .. } = busy_of(&jobs, project, target) {
        if id > since {
            queue::pause(id);
            queue::resume(id);
        }
    }
}

/// Changes some shots of one scene file (read fresh, so a job's new pictures are kept).
fn edit_shots(lock: &WriteLock, store: &Store, ep: &str, scene: &str, ids: &[String], f: &dyn Fn(&mut Shot)) -> anyhow::Result<()> {
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    // Checked first, so a shot gone meanwhile is said plainly and no empty scene file is made.
    if !store.scene_shots(ep, scene)?.shots.iter().any(|s| ids.contains(&s.id)) {
        bail!("This shot is not in its scene any more; it may have been removed when the scene was planned again.");
    }
    // Read, changed and written under the scene file's own lock, so a frame job recording its
    // pictures at the same moment never loses this change (nor this one its pictures).
    let rel = format!("episodes/{}/shots/{}.json", mv_core::paths::safe_name(ep), mv_core::paths::safe_name(scene));
    let (ep_id, scene_id) = (ep.to_string(), scene.to_string());
    store.update_json::<mv_anime::SceneShots>(&rel, |file| {
        if file.episode.is_empty() {
            file.episode = ep_id;
        }
        if file.scene.is_empty() {
            file.scene = scene_id;
        }
        for s in file.shots.iter_mut().filter(|s| ids.contains(&s.id)) {
            f(s);
        }
    })?;
    Ok(())
}

/// Stores one picture the user handed in as an Upload take, once it is checked to be a real
/// picture of a sane size. The error is a plain sentence naming the file.
fn import_upload(store: &Store, path: &Path) -> anyhow::Result<Take> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if frame_ext(path).is_none() {
        bail!("{name} is not a PNG, JPG or WebP picture.");
    }
    let ext = mv_ui::thumbs::check_upload(path, UPLOAD_MAX_SIDE, UPLOAD_MAX_BYTES).map_err(|e| anyhow!("{name} could not be used: {e}."))?;
    let bytes = std::fs::read(path).map_err(|e| anyhow!("Could not read {name}: {e}"))?;
    let meta = TakeMeta { kind: TakeKind::Image, target: Target::Upload, source: TakeSource::Upload { name }, ..Default::default() };
    store.add_take(&bytes, ext, meta)
}

/// What became of the pictures the user handed in for a shot.
struct Uploaded {
    added: usize,
    /// Files that were not usable pictures, with why (the first one is shown).
    refused: Vec<String>,
}

/// Stores the user's own pictures as takes of the shot and puts the first in use (it then needs
/// review); files that are not usable pictures are skipped. Fails when none could be used.
fn import_frames(lock: &WriteLock, store: &Store, ep: &str, scene: &str, shot: &str, paths: &[PathBuf]) -> anyhow::Result<Uploaded> {
    let mut ids = Vec::new();
    let mut refused = Vec::new();
    for p in paths {
        match import_upload(store, p) {
            Ok(t) => ids.push(t.id),
            Err(e) => {
                mv_core::logging::global().warn("anime", &format!("frames: upload refused: {e:#}"));
                refused.push(format!("{e:#}"));
            }
        }
    }
    let Some(first) = ids.first().cloned() else {
        bail!("{}", refused.first().cloned().unwrap_or_else(|| "No picture was given.".into()));
    };
    edit_shots(lock, store, ep, scene, &[shot.to_string()], &|s| {
        for id in &ids {
            if !s.frame.takes.contains(id) {
                s.frame.takes.push(id.clone());
            }
        }
        s.frame.selected = Some(first.clone());
        s.frame.status = Status::NeedsReview;
    })?;
    Ok(Uploaded { added: ids.len(), refused })
}

/// The toast after an upload to a shot.
fn uploaded_note(u: &Uploaded) -> String {
    let base = if u.added == 1 { "Your picture is now this shot's frame.".to_string() } else { format!("Added {}; the first is now this shot's frame.", count(u.added, "picture")) };
    let skipped = match u.refused.len() {
        0 => String::new(),
        1 => " 1 file was not a usable picture.".into(),
        n => format!(" {n} files were not usable pictures."),
    };
    format!("{base}{skipped} Approve it when it looks right.")
}

// ---------------------------------------------------------------- the screen

pub struct FramesStep {
    ctx: StepCtx,
    store: Store,
    takes: TakeCache,
    writes: WriteLock,
    turns: Turns,
    data: Load,
    /// A refresh failed while older data is shown.
    refresh_error: Option<String>,
    loading: bool,
    again: bool,
    /// The project changed while this step was not drawn: read it again when it is next drawn.
    stale: bool,
    /// Reads of the episode started so far (the number of the latest one).
    loads: u64,
    /// (store version, queue version) last seen.
    seen: (u64, u64),
    busy: HashMap<String, Busy>,
    /// The newest job id seen in the queue (a job above it was added later).
    newest_job: u64,
    queue_paused: bool,
    web_held: bool,
    filter: Filter,
    /// The open shot (its viewer and the Generation panel).
    selected: Option<String>,
    /// The shot opened last (outlined in the grid).
    recent: Option<String>,
    /// "Review one by one" of the outdated frames.
    reviewing: bool,
    /// Shots sent to "Update": their new picture is put in use when it arrives.
    updating: HashSet<String>,
    /// Shots whose Update job finished, with the first read that sees its new pictures (one
    /// started after the job was seen done); that read puts the new picture in use.
    arrived: HashMap<String, u64>,
    enqueuing: bool,
    panel: Option<Entity<GenPanel>>,
    panel_shot: Option<String>,
    history: Option<Entity<History>>,
    history_open: bool,
    /// Scroll the list to the open shot on the next frame.
    reveal: bool,
    /// Scroll the grid to this scene on the next frame.
    jump: Option<String>,
    grid_scroll: ScrollHandle,
    list_scroll: ScrollHandle,
    /// The window is narrow (set on every draw): the picture panel is a drawer, shown only
    /// when asked (`drawer`), so it never lies over the frame and its buttons.
    narrow: bool,
    drawer: bool,
    /// The picture last approved or taken back to review, and when (a second click on it
    /// within `STATUS_CLICK_GAP` is ignored).
    last_status_click: Option<(String, std::time::Instant)>,
}

/// How long a second approve / "Back to review" click on the same picture is ignored.
pub(crate) const STATUS_CLICK_GAP: std::time::Duration = std::time::Duration::from_millis(600);

/// Whether an approve / take-back click on `key` at `now` counts: not when the same picture
/// got one less than `STATUS_CLICK_GAP` ago. Remembers a counted click.
pub(crate) fn accept_status_click(last: &mut Option<(String, std::time::Instant)>, key: &str, now: std::time::Instant) -> bool {
    if let Some((k, at)) = last {
        if k == key && now.saturating_duration_since(*at) < STATUS_CLICK_GAP {
            return false;
        }
    }
    *last = Some((key.to_string(), now));
    true
}

pub fn open(ctx: StepCtx, window: &mut Window, cx: &mut App) -> AnyView {
    backends::check_site_once();
    cx.new(|cx| {
        let store = Store::open(&ctx.project);
        let mut step = FramesStep {
            ctx,
            store,
            takes: Arc::new(Mutex::new(HashMap::new())),
            writes: Arc::new(Mutex::new(())),
            turns: Turns::default(),
            data: Load::Loading,
            refresh_error: None,
            loading: false,
            again: false,
            stale: false,
            loads: 0,
            seen: (mv_anime::store::version(), queue::version()),
            busy: HashMap::new(),
            newest_job: 0,
            queue_paused: false,
            web_held: false,
            filter: Filter::All,
            selected: None,
            recent: None,
            reviewing: false,
            updating: HashSet::new(),
            arrived: HashMap::new(),
            enqueuing: false,
            panel: None,
            panel_shot: None,
            history: None,
            history_open: false,
            reveal: false,
            jump: None,
            grid_scroll: ScrollHandle::new(),
            list_scroll: ScrollHandle::new(),
            narrow: false,
            drawer: false,
            last_status_click: None,
        };
        step.refresh_jobs();
        step.reload(cx);
        step.poll(window, cx);
        step
    })
    .into()
}

impl FramesStep {
    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(POLL_MS)).await;
            if this.update(cx, |s, cx| s.tick(cx)).is_err() {
                break;
            }
        })
        .detach();
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let v = (mv_anime::store::version(), queue::version());
        if v == self.seen {
            return;
        }
        let (files, jobs) = (v.0 != self.seen.0, v.1 != self.seen.1);
        self.seen = v;
        if jobs {
            self.refresh_jobs();
            self.track_updates();
        }
        if files {
            // Read again when next drawn: a step that is not on screen is never drawn, so it
            // does not re-read the episode on every picture a running job writes.
            self.stale = true;
        }
        cx.notify();
    }

    fn refresh_jobs(&mut self) {
        let q = queue::snapshot();
        self.queue_paused = q.paused || q.interrupted;
        self.web_held = web_held(&q);
        self.newest_job = q.jobs.iter().map(|j| j.id).max().unwrap_or(0);
        let jobs: Vec<Job> = q.jobs.into_iter().filter(|j| j.lane == Lane::Web).collect();
        self.busy = busy_map(&jobs, &self.ctx.project, &self.ctx.episode);
    }

    /// Reads the episode again in the background (a second call while reading runs once more after).
    fn reload(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            self.again = true;
            return;
        }
        self.loading = true;
        self.loads += 1;
        let number = self.loads;
        let (store, ep, takes) = (self.store.clone(), self.ctx.episode.clone(), self.takes.clone());
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || load(&store, &ep, &takes)).await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                let read = match res {
                    Ok(Ok(d)) => {
                        this.data = Load::Ready(d);
                        this.refresh_error = None;
                        true
                    }
                    Ok(Err(e)) | Err(e) => {
                        let msg = format!("{e:#}");
                        mv_core::logging::global().warn("anime", &format!("frames: could not read the episode: {msg}"));
                        if matches!(this.data, Load::Ready(_)) {
                            this.refresh_error = Some(msg);
                        } else {
                            this.data = Load::Failed(msg);
                        }
                        false
                    }
                };
                this.after_load(read.then_some(number), cx);
                if std::mem::take(&mut this.again) {
                    this.reload(cx);
                }
                cx.notify();
                crate::redraw(cx);
            });
        })
        .detach();
    }

    /// `read`: the number of the read that just succeeded.
    fn after_load(&mut self, read: Option<u64>, cx: &mut Context<Self>) {
        if let (Load::Ready(d), Some(sel)) = (&self.data, self.selected.as_deref()) {
            if !d.rows.iter().any(|r| r.id == sel) {
                self.selected = None;
                self.reviewing = false;
            }
        }
        self.sync_panel(cx);
        if let Some(n) = read {
            self.pick_updated(n, cx);
        }
    }

    fn rows(&self) -> &[Row] {
        match &self.data {
            Load::Ready(d) => &d.rows,
            _ => &[],
        }
    }

    fn row(&self, id: &str) -> Option<&Row> {
        self.rows().iter().find(|r| r.id == id)
    }

    fn row_mut(&mut self, id: &str) -> Option<&mut Row> {
        match &mut self.data {
            Load::Ready(d) => d.rows.iter_mut().find(|r| r.id == id),
            _ => None,
        }
    }

    /// Puts the open shot into the Generation panel and the history (new entities when another
    /// shot opens, so their titles follow it).
    fn sync_panel(&mut self, cx: &mut Context<Self>) {
        let Some((shot, number, spec, compiled, takes, selected)) = self.selected.as_deref().and_then(|id| self.row(id)).map(|r| {
            (r.id.clone(), r.number.clone(), r.prepared.spec.clone(), r.prepared.compiled.clone(), r.shot.frame.takes.clone(), r.shot.frame.selected.clone())
        }) else {
            self.panel = None;
            self.panel_shot = None;
            self.history = None;
            return;
        };
        if self.panel_shot.as_deref() == Some(shot.as_str()) {
            if let Some(p) = &self.panel {
                p.update(cx, |p, cx| {
                    p.set(spec, Some(compiled));
                    cx.notify();
                });
            }
            if let Some(h) = &self.history {
                h.update(cx, |h, cx| {
                    h.set(takes, selected);
                    cx.notify();
                });
            }
            return;
        }
        let project = self.ctx.project.clone();
        let target = Target::ShotFrame { episode: self.ctx.episode.clone(), shot: shot.clone() };
        let weak = cx.entity().downgrade();
        let on = shot.clone();
        let panel = cx.new(|_| {
            GenPanel::new(target, spec.clone()).project(&project).title(format!("Frame {number}")).label(format!("Picture for shot {number}")).on_change(move |spec, _w, cx| {
                let _ = weak.update(cx, |this, cx| this.save_spec(&on, spec, cx));
            })
        });
        panel.update(cx, |p, cx| {
            p.set(spec, Some(compiled));
            cx.notify();
        });
        let (pick, close) = (cx.entity().downgrade(), cx.entity().downgrade());
        let on = shot.clone();
        let history = cx.new(|_| {
            History::new(&project, takes, selected)
                .title(format!("Frame {number}"))
                .on_select(move |take, _w, cx| {
                    let _ = pick.update(cx, |this, cx| this.choose(&on, take, false, cx));
                })
                .on_close(move |_w, cx| {
                    let _ = close.update(cx, |this, cx| {
                        this.history_open = false;
                        cx.notify();
                    });
                })
        });
        self.panel = Some(panel);
        self.history = Some(history);
        self.panel_shot = Some(shot);
    }

    fn open_shot(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if id.is_none() {
            self.reviewing = false;
        }
        if self.selected != id {
            if id.is_some() {
                self.recent = id.clone();
            }
            self.selected = id;
            self.history_open = false;
            self.reveal = true;
            self.sync_panel(cx);
        }
        cx.notify();
    }

    // ------------------------------------------------------------ changes

    /// The Generation panel's edits (prompt, references, model, size, variants): shown at once and
    /// saved in the background, in turn. A Generate pressed right after still uses them: a job
    /// for this shot that started before the save landed starts over (`restart_if_started`).
    fn save_spec(&mut self, shot: &str, spec: GenSpec, cx: &mut Context<Self>) {
        let Some(scene) = self.row(shot).map(|r| r.scene.clone()) else { return };
        if let Some(r) = self.row_mut(shot) {
            r.shot.frame.gen = spec.clone();
        }
        let since = self.newest_job;
        let (store, ep, lock, project) = (self.store.clone(), self.ctx.episode.clone(), self.writes.clone(), self.ctx.project.clone());
        let target = Target::ShotFrame { episode: ep.clone(), shot: shot.to_string() };
        let id = shot.to_string();
        let turn = self.turns.take();
        let task = mv_ui::bg::blocking(move || {
            turn.run(|| {
                let saved = edit_shots(&lock, &store, &ep, &scene, std::slice::from_ref(&id), &|s| s.frame.gen = spec.clone());
                restart_if_started(&project, &target, since);
                saved
            })
        });
        cx.spawn(async move |this, cx| {
            if let Some(e) = failure(task.await) {
                mv_core::logging::global().warn("anime", &format!("frames: settings not saved: {e}"));
                let _ = this.update(cx, |this, cx| {
                    toast::show(cx, Toast::error("The frame's settings could not be saved. Try the change again."));
                    this.reload(cx);
                });
            }
        })
        .detach();
    }

    /// Applies `f` to the shots here at once and to their scene files in the background (one
    /// write per scene, in the order the changes were made).
    fn change(&mut self, shots: &[String], what: &'static str, f: impl Fn(&mut Shot) + Send + 'static, cx: &mut Context<Self>) {
        let mut by_scene: Vec<(String, Vec<String>)> = Vec::new();
        for id in shots {
            let Some(scene) = self.row(id).map(|r| r.scene.clone()) else { continue };
            match by_scene.iter_mut().find(|(s, _)| *s == scene) {
                Some((_, ids)) => ids.push(id.clone()),
                None => by_scene.push((scene, vec![id.clone()])),
            }
            if let Some(r) = self.row_mut(id) {
                f(&mut r.shot);
                r.after_edit();
            }
        }
        if by_scene.is_empty() {
            return;
        }
        let (store, ep, lock) = (self.store.clone(), self.ctx.episode.clone(), self.writes.clone());
        let turn = self.turns.take();
        let task = mv_ui::bg::blocking(move || turn.run(|| by_scene.iter().try_for_each(|(scene, ids)| edit_shots(&lock, &store, &ep, scene, ids, &f))));
        cx.spawn(async move |this, cx| {
            let res = task.await;
            if let Some(e) = failure(res) {
                let _ = this.update(cx, |this, cx| {
                    toast::show(cx, Toast::error(format!("{what}: {e}")));
                    this.reload(cx);
                });
            }
        })
        .detach();
        cx.notify();
    }

    fn set_status(&mut self, shot: &str, status: Status, cx: &mut Context<Self>) {
        // A second approve / take-back click on the same picture right after the first (a double
        // click) is ignored, so it can never undo the approval it just made.
        if matches!(status, Status::Approved | Status::NeedsReview) && !accept_status_click(&mut self.last_status_click, shot, std::time::Instant::now()) {
            return;
        }
        if status == Status::Approved && self.row(shot).is_some_and(|r| r.shot.frame.selected.is_none()) {
            toast::show(cx, Toast::info("Make or upload a picture for this shot first, then approve it."));
            return;
        }
        if status == Status::Approved {
            self.approve_shots(vec![shot.to_string()], cx);
            return;
        }
        let what = match status {
            Status::Approved => "The frame could not be approved",
            Status::Rejected => "The frame could not be rejected",
            _ => "The frame's status could not be changed",
        };
        self.change(
            &[shot.to_string()],
            what,
            move |s| {
                if status != Status::Approved || s.frame.selected.is_some() {
                    s.frame.status = status;
                }
            },
            cx,
        );
    }

    fn set_locked(&mut self, shots: &[String], locked: bool, cx: &mut Context<Self>) {
        self.change(shots, if locked { "The frame could not be locked" } else { "The frame could not be unlocked" }, move |s| s.frame.locked = locked, cx);
    }

    /// Approves shots through the engine (`frames::approve`), which also remembers the request
    /// the picture is accepted for, so its "outdated" mark clears. Shown at once, written in turn.
    fn approve_shots(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        let mut ids_here = Vec::new();
        for id in ids {
            if let Some(r) = self.row_mut(&id).filter(|r| r.shot.frame.selected.is_some()) {
                r.shot.frame.status = Status::Approved;
                r.outdated = false;
                r.after_edit();
                ids_here.push(id);
            }
        }
        if ids_here.is_empty() {
            return;
        }
        let (store, ep) = (self.store.clone(), self.ctx.episode.clone());
        let turn = self.turns.take();
        let task = mv_ui::bg::blocking(move || turn.run(|| ids_here.iter().try_for_each(|id| frames::approve(&store, &ep, id))));
        cx.spawn(async move |this, cx| {
            if let Some(e) = failure(task.await) {
                mv_core::logging::global().warn("anime", &format!("frames: approval not saved: {e}"));
                let _ = this.update(cx, |this, cx| {
                    toast::show(cx, Toast::error(format!("The frame could not be approved: {e}")));
                    this.reload(cx);
                });
            }
        })
        .detach();
        cx.notify();
    }

    /// Puts one of the shot's pictures in use (and approves it when asked).
    fn choose(&mut self, shot: &str, take: String, approve: bool, cx: &mut Context<Self>) {
        if approve {
            // In use first, then approved by the engine, in that order.
            self.choose(shot, take, false, cx);
            self.approve_shots(vec![shot.to_string()], cx);
            return;
        }
        self.change(
            &[shot.to_string()],
            "That picture could not be put in use",
            move |s| {
                let f = &mut s.frame;
                if !f.takes.contains(&take) {
                    return;
                }
                if f.selected.as_deref() != Some(take.as_str()) {
                    f.selected = Some(take.clone());
                    if f.status == Status::Approved {
                        f.status = Status::NeedsReview;
                    }
                }
                if approve {
                    f.status = Status::Approved;
                } else if f.status == Status::Draft {
                    f.status = Status::NeedsReview;
                }
            },
            cx,
        );
    }

    /// The AI picks the shot's references again (pinned ones stay).
    fn ai_refs(&mut self, shot: &str, cx: &mut Context<Self>) {
        self.change(&[shot.to_string()], "The references could not be reset", |s| s.frame.gen.refs_user = false, cx);
    }

    /// Queues frame jobs for `ids` (0 variants = the project's setting).
    fn enqueue(&mut self, ids: Vec<String>, variants: u8, update: bool, cx: &mut Context<Self>) {
        if ids.is_empty() {
            return;
        }
        backends::ensure_runner(cx);
        self.enqueuing = true;
        let (store, ep) = (self.store.clone(), self.ctx.episode.clone());
        let sent = ids.clone();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || frames::enqueue_frames(&store, &ep, &ids, variants)).await;
            let _ = this.update(cx, |this, cx| {
                this.enqueuing = false;
                match res {
                    Ok(()) if update => {
                        // Read the queue first, so the new jobs (not an older finished one) are
                        // what these shots wait for.
                        this.refresh_jobs();
                        for id in sent {
                            this.arrived.remove(&id);
                            this.updating.insert(id);
                        }
                        this.track_updates();
                    }
                    Ok(()) => {}
                    Err(e) => toast::show(cx, Toast::error(format!("The frames could not be queued: {e:#}"))),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Follows the jobs of shots sent to "Update": a finished one waits for a read of the episode
    /// that started after it (the rows read before do not have its pictures yet); a failed or
    /// cancelled one is let go.
    fn track_updates(&mut self) {
        if self.updating.is_empty() {
            return;
        }
        let mut finished = Vec::new();
        for id in &self.updating {
            match self.busy.get(id) {
                Some(Busy::Done { .. }) => finished.push((id.clone(), true)),
                Some(Busy::Failed { .. } | Busy::Idle) | None => finished.push((id.clone(), false)),
                Some(Busy::Waiting { .. } | Busy::Working { .. }) => {}
            }
        }
        for (id, done) in finished {
            self.updating.remove(&id);
            if done {
                // The next read to start sees the new pictures.
                self.arrived.insert(id, self.loads + 1);
                self.stale = true;
            }
        }
    }

    /// After read number `read`: each finished Update that read covers puts its new picture in
    /// use (still to review).
    fn pick_updated(&mut self, read: u64, cx: &mut Context<Self>) {
        if self.arrived.is_empty() || !matches!(self.data, Load::Ready(_)) {
            return;
        }
        let ready: Vec<String> = self.arrived.iter().filter(|(_, from)| read >= **from).map(|(id, _)| id.clone()).collect();
        let mut picks = Vec::new();
        for id in ready {
            self.arrived.remove(&id);
            if let Some(take) = self.row(&id).filter(|r| r.outdated && !r.shot.frame.locked).and_then(|r| r.fresh.clone()) {
                picks.push((id, take));
            }
        }
        for (id, take) in picks {
            self.choose(&id, take, false, cx);
        }
    }

    /// The shot was sent to "Update" and its new picture is not in use yet.
    fn being_updated(&self, id: &str) -> bool {
        self.updating.contains(id) || self.arrived.contains_key(id)
    }

    fn update_one(&mut self, shot: &str, cx: &mut Context<Self>) {
        let variants = self.row(shot).map(|r| r.prepared.spec.variants).unwrap_or(0);
        self.enqueue(vec![shot.to_string()], variants, true, cx);
        self.review_next(shot, cx);
    }

    fn keep(&mut self, shot: &str, cx: &mut Context<Self>) {
        self.set_locked(&[shot.to_string()], true, cx);
        self.review_next(shot, cx);
    }

    /// While reviewing one by one: the next outdated frame, or the end of the review.
    fn review_next(&mut self, done: &str, cx: &mut Context<Self>) {
        if !self.reviewing {
            return;
        }
        let order: Vec<(String, bool)> = self.rows().iter().map(|r| (r.id.clone(), r.outdated && !self.being_updated(&r.id) && r.id != done)).collect();
        match next_outdated(&order, done) {
            Some(next) => self.open_shot(Some(next), cx),
            None => {
                self.reviewing = false;
                toast::show(cx, Toast::success("That was the last outdated frame."));
                cx.notify();
            }
        }
    }

    fn start_review(&mut self, cx: &mut Context<Self>) {
        let first = self.rows().iter().find(|r| r.outdated).map(|r| r.id.clone());
        if let Some(first) = first {
            self.filter = Filter::Outdated;
            self.reviewing = true;
            self.open_shot(Some(first), cx);
        }
    }

    fn keep_all(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.rows().iter().filter(|r| r.outdated).map(|r| r.id.clone()).collect();
        self.set_locked(&ids, true, cx);
        if !ids.is_empty() {
            toast::show(cx, Toast::success(format!("Kept {} as they are. Unlock a frame to update it later.", count(ids.len(), "frame"))));
        }
    }

    /// Shots "Draw all frames" would queue: no usable picture yet (none or rejected), not
    /// locked, not already queued.
    fn draw_ids(&self) -> Vec<String> {
        self.rows().iter().filter(|r| needs_drawing(&r.shot.frame) && !in_queue(self.busy.get(&r.id))).map(|r| r.id.clone()).collect()
    }

    fn confirm_draw_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.draw_ids();
        let waiting = self.rows().iter().filter(|r| waiting_review(&r.shot.frame) && !r.shot.frame.locked).count();
        let Load::Ready(d) = &self.data else { return };
        if ids.is_empty() {
            toast::show(cx, Toast::info("Every shot has a picture (approved, locked or waiting for your review), or is already in the queue."));
            return;
        }
        let body = draw_note(ids.len(), waiting, d.variants, Eta::current().mean("frame"));
        let weak = cx.entity().downgrade();
        confirm_dialog(format!("Draw {}?", count(ids.len(), "frame")), body, "Draw frames", false)
            .icon(ph::SPARKLE)
            .on_confirm(move |_w, cx| {
                let ids = ids.clone();
                let _ = weak.update(cx, |this, cx| this.enqueue(ids, 0, false, cx));
            })
            .open(window, cx);
    }

    fn confirm_update_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.rows().iter().filter(|r| r.outdated && !in_queue(self.busy.get(&r.id))).map(|r| r.id.clone()).collect();
        if ids.is_empty() {
            toast::show(cx, Toast::info("The outdated frames are already in the queue."));
            return;
        }
        let body = format!(
            "Each is drawn again with what changed, and the new picture is put in use for you to review. The pictures they have now stay in Earlier versions. {}",
            time_note(ids.len(), Eta::current().mean("frame"))
        );
        let weak = cx.entity().downgrade();
        confirm_dialog(format!("Update {}?", count(ids.len(), "outdated frame")), body, "Update all", false)
            .icon(ph::ARROWS_CLOCKWISE)
            .on_confirm(move |_w, cx| {
                let ids = ids.clone();
                let _ = weak.update(cx, |this, cx| this.enqueue(ids, 0, true, cx));
            })
            .open(window, cx);
    }

    fn pick_upload(&mut self, shot: String, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Use as this shot's frame".into()) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let _ = this.update(cx, |this, cx| this.upload(shot, paths, cx));
        })
        .detach();
    }

    /// The user's own pictures for a shot (one picked, or several dropped): all are kept as its
    /// pictures and the first goes in use.
    fn upload(&mut self, shot: String, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let Some(scene) = self.row(&shot).map(|r| r.scene.clone()) else { return };
        let (store, ep, lock) = (self.store.clone(), self.ctx.episode.clone(), self.writes.clone());
        let turn = self.turns.take();
        let task = mv_ui::bg::blocking(move || turn.run(|| import_frames(&lock, &store, &ep, &scene, &shot, &paths)));
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(Ok(u)) => toast::show(cx, Toast::success(uploaded_note(&u))),
                    Ok(Err(e)) | Err(e) => {
                        mv_core::logging::global().warn("anime", &format!("frames: upload failed: {e:#}"));
                        toast::show(cx, Toast::error(format!("Your picture was not used: {e}")));
                    }
                }
                // The new picture (and the filter counts) show now, not at the next mouse move.
                this.reload(cx);
                cx.notify();
                crate::redraw(cx);
            });
        })
        .detach();
    }

    fn retry_failed(&mut self, cx: &mut Context<Self>) {
        backends::ensure_runner(cx);
        for b in self.busy.values() {
            if let Busy::Failed { id, .. } = b {
                queue::retry(*id);
            }
        }
    }
}

// ---------------------------------------------------------------- view

/// A whole-step state centred in the page area.
fn centred(el: impl IntoElement) -> Div {
    div().size_full().flex().items_center().justify_center().p(px(space::XXL)).child(el)
}

/// A small chip over a picture's top-left corner.
fn corner_chip() -> Div {
    div()
        .absolute()
        .top(px(6.))
        .left(px(6.))
        .flex()
        .items_center()
        .gap(px(space::XS))
        .h(px(20.))
        .px(px(6.))
        .rounded(px(radius::SM))
        .bg(Tokens::popover())
        .border_1()
        .border_color(Tokens::border())
        .text_size(px(size::XS))
        .text_color(Tokens::text_1())
        .whitespace_nowrap()
}

/// What a picture shows while its job waits, draws or failed (`held`: Magnific needs the user,
/// so waiting pictures wait for them).
fn busy_marks(i: usize, busy: &Busy, held: bool) -> Vec<AnyElement> {
    match busy {
        Busy::Working { progress, line, .. } => {
            let pct = (progress.clamp(0., 1.) * 100.).round() as u32;
            let hint = if line.trim().is_empty() { "Drawing on Magnific".to_string() } else { line.clone() };
            vec![
                div().absolute().inset_0().bg(Tokens::canvas().alpha(0.35)).into_any_element(),
                corner_chip().id(("fr-working", i)).tooltip(tip(hint)).child(spinner(("fr-spin", i), Tokens::accent(), icon_px::XS)).child(format!("Drawing {pct}%")).into_any_element(),
                div().absolute().left_0().right_0().bottom_0().child(progress_bar(("fr-progress", i), *progress, !held)).into_any_element(),
            ]
        }
        Busy::Waiting { ahead, .. } => {
            let text = if held {
                "Waiting for Magnific".to_string()
            } else if *ahead > 0 {
                format!("Waiting · {ahead} ahead")
            } else {
                "Next in line".to_string()
            };
            vec![corner_chip().child(icon_ph(ph::HOURGLASS, Tokens::text_2(), icon_px::XS)).child(text).into_any_element()]
        }
        Busy::Failed { error, .. } => vec![div()
            .id(("fr-failed", i))
            .absolute()
            .top(px(6.))
            .left(px(6.))
            .flex()
            .items_center()
            .justify_center()
            .size(px(24.))
            .rounded_full()
            .bg(Tokens::danger_soft())
            .border_1()
            .border_color(Tokens::danger_border())
            .tooltip(tip(format!("Could not be drawn: {error}")))
            .child(ph_icon(ph::WARNING_CIRCLE, Weight::Fill, Tokens::danger(), icon_px::SM))
            .into_any_element()],
        _ => Vec::new(),
    }
}

/// A small mark for a row of the list: what the shot needs now, with its words in a tooltip.
fn row_mark(i: usize, card: &Card, outdated: bool, busy: &Busy) -> Stateful<Div> {
    let base = div().id(("fr-mark", i)).flex().flex_none().items_center().gap(px(space::XXS));
    let (icon, colour, text): (Option<mv_ui::Ph>, Rgba, String) = match busy {
        Busy::Working { .. } => (None, Tokens::accent(), "Drawing now".into()),
        Busy::Waiting { .. } => (Some(ph::HOURGLASS), Tokens::text_2(), "Waiting in the queue".into()),
        Busy::Failed { error, .. } => (Some(ph::WARNING_CIRCLE), Tokens::danger(), format!("Could not be drawn: {error}")),
        _ if outdated => (Some(ph::ARROWS_CLOCKWISE), Tokens::warning(), "Outdated: something it uses changed".into()),
        _ => match card.status {
            Status::Approved if card.selected.is_some() => (Some(ph::CHECK), Tokens::success(), "Approved".into()),
            Status::Rejected => (Some(ph::X), Tokens::danger(), "Rejected".into()),
            Status::NeedsReview if card.selected.is_some() => (Some(ph::EYE), Tokens::info(), "Needs review".into()),
            _ if card.selected.is_none() => (Some(ph::CIRCLE_DASHED), Tokens::text_3(), "No picture yet".into()),
            s => (Some(ph::CIRCLE_DASHED), Tokens::text_3(), status_label(s).to_string()),
        },
    };
    let text = if card.locked { format!("{text} · locked") } else { text };
    base.tooltip(tip(text))
        .when(card.locked, |d| d.child(icon_ph(ph::LOCK_SIMPLE, Tokens::text_3(), icon_px::XS)))
        .map(|d| match icon {
            Some(i) => d.child(icon_ph(i, colour, icon_px::SM)),
            None => d.child(spinner(("fr-row-spin", i), colour, icon_px::SM)),
        })
}

impl Render for FramesStep {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.stale) {
            self.reload(cx);
        }
        self.narrow = is_narrow(window);
        let reveal = std::mem::take(&mut self.reveal);
        let jump = self.jump.take();
        let body = match &self.data {
            Load::Loading => self.skeleton(window).into_any_element(),
            Load::Failed(e) => centred(error_view(format!("The frames could not be read: {e}")).retry(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.data = Load::Loading;
                this.reload(cx);
                cx.notify();
            })))
            .into_any_element(),
            Load::Ready(d) if d.rows.is_empty() => self.no_shots(d).into_any_element(),
            Load::Ready(d) => self.ready(d, window, reveal, jump, cx).into_any_element(),
        };
        div().size_full().child(body)
    }
}

impl FramesStep {
    fn skeleton(&self, window: &Window) -> Div {
        let list = div().flex().flex_col().size_full().child(column_head("Shots")).child(
            div().flex().flex_col().gap(px(space::SM)).p(px(space::MD)).children((0..8).map(|_| {
                div().flex().items_center().gap(px(space::SM)).child(skeleton_block(px(LIST_THUMB.0), px(LIST_THUMB.1))).child(skeleton_line(px(TILE_W / 2.)))
            })),
        );
        let grid = div()
            .flex()
            .flex_col()
            .size_full()
            .child(column_head("Frames"))
            .child(div().flex().flex_wrap().gap(px(space::MD)).p(px(space::MD)).children((0..8).map(|_| skeleton_block(px(TILE_W), px(TILE_H)))));
        workspace(window, Some(list.into_any_element()), grid, None)
    }

    fn no_shots(&self, d: &Data) -> Div {
        let go = self.ctx.go.clone();
        let body = if d.has_story {
            "Frames draws one picture for every shot. Plan the shots of this episode in the Shots step first, then come back."
        } else {
            "Frames draws one picture for every shot. Write the story, then plan its shots in the Shots step."
        };
        centred(empty_view(ph::IMAGE, "No shots planned yet", body).action(btn("fr-go-shots", "Go to Shots", ButtonKind::Primary).icon(Step::Shots.icon()).on_click(move |_, w, cx| go(Step::Shots, w, cx))))
    }

    fn ready(&self, d: &Data, window: &Window, reveal: bool, jump: Option<String>, cx: &mut Context<Self>) -> Div {
        let visible: Vec<usize> = (0..d.rows.len()).filter(|&i| self.filter.passes(&d.rows[i].shot.frame, d.rows[i].outdated)).collect();
        let list = self.list(d, &visible, reveal, cx);
        let open = self.selected.as_deref().and_then(|id| d.rows.iter().position(|r| r.id == id));
        let center = match open {
            Some(i) => self.viewer(d, i, &visible, cx).into_any_element(),
            None => self.grid(d, &visible, jump, cx).into_any_element(),
        };
        // Narrow window: the panel shows only when asked ("Picture settings"), never on its own.
        let panel = open.filter(|_| !self.narrow || self.drawer).and(self.panel.clone()).map(|p| p.into_any_element());
        workspace(window, Some(list.into_any_element()), center, panel)
    }

    // ------------------------------------------------------------ the list

    fn list(&self, d: &Data, visible: &[usize], reveal: bool, cx: &mut Context<Self>) -> Div {
        let mut chips = div().flex().flex_wrap().gap(px(space::XS)).pb(px(space::SM));
        for (k, f) in Filter::ALL.into_iter().enumerate() {
            let n = d.rows.iter().filter(|r| f.passes(&r.shot.frame, r.outdated)).count();
            if f != Filter::All && n == 0 && self.filter != f {
                continue;
            }
            chips = chips.child(filter_chip(("fr-filter", k), f.label(), self.filter == f).count(n).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.filter = f;
                if f != Filter::Outdated {
                    this.reviewing = false;
                }
                cx.notify();
            })));
        }
        let mut body = column_body("fr-list").gap(px(space::XXS)).track_scroll(&self.list_scroll).child(chips);
        let mut ix = 1;
        let mut first = true;
        for (k, g) in d.scenes.iter().enumerate() {
            let rows: Vec<usize> = visible.iter().copied().filter(|i| g.rows.contains(i)).collect();
            if rows.is_empty() {
                continue;
            }
            let approved = d.rows[g.rows.clone()].iter().filter(|r| is_approved(&r.shot.frame)).count();
            let key = g.key.clone();
            body = body.child(
                div()
                    .id(("fr-scene", k))
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .px(px(space::XS))
                    .pt(px(if first { 0. } else { space::MD }))
                    .pb(px(space::XS))
                    .cursor_pointer()
                    .tooltip(tip("Show this scene's frames"))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.jump = Some(key.clone());
                        this.open_shot(None, cx);
                    }))
                    .child(label_caps(&format!("Scene {}", g.number)))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(g.title.clone()))
                    .child(div().flex_none().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{approved} / {}", g.rows.len()))),
            );
            ix += 1;
            first = false;
            for i in rows {
                if reveal && self.selected.as_deref() == Some(d.rows[i].id.as_str()) {
                    self.list_scroll.scroll_to_item(ix);
                }
                body = body.child(self.list_row(i, &d.rows[i], cx));
                ix += 1;
            }
        }
        if visible.is_empty() {
            body = body.child(empty_view(ph::FUNNEL, "Nothing here", "No shot matches this filter.").compact());
        }
        div().flex().flex_col().size_full().child(column_head("Shots").child(spec(count(d.rows.len(), "shot")))).child(body)
    }

    fn list_row(&self, i: usize, r: &Row, cx: &mut Context<Self>) -> Stateful<Div> {
        let on = self.selected.as_deref() == Some(r.id.as_str());
        let busy = self.busy.get(&r.id).cloned().unwrap_or(Busy::Idle);
        let detail = match secs_label(shot_secs(&r.shot)) {
            Some(s) => format!("{} · {s}", size_label(r.shot.camera.size)),
            None => size_label(r.shot.camera.size).to_string(),
        };
        let id = r.id.clone();
        div()
            .id(("fr-row", i))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(space::SM))
            .p(px(space::XS))
            .rounded(px(radius::MD))
            .cursor_pointer()
            .map(|d| if on { d.bg(Tokens::active()) } else { d.hover(|s| s.bg(Tokens::hover())) })
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_shot(Some(id.clone()), cx)))
            .child(thumb(r.still.clone(), LIST_THUMB.0, LIST_THUMB.1))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(space::XXS))
                    .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(r.number.clone()))
                    .child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(detail)),
            )
            .child(row_mark(i, &r.shot.frame, r.outdated, &busy))
    }

    // ------------------------------------------------------------ the grid

    fn grid(&self, d: &Data, visible: &[usize], jump: Option<String>, cx: &mut Context<Self>) -> Div {
        let approved = d.rows.iter().filter(|r| is_approved(&r.shot.frame)).count();
        let outdated = d.rows.iter().filter(|r| r.outdated).count();
        let to_draw = self.draw_ids().len();
        let mut head = column_head("Frames").child(spec(format!("{approved} of {} approved", d.rows.len())));
        if outdated > 0 {
            head = head.child(self.outdated_menu(outdated, cx));
        }
        let draw = btn("fr-draw-all", "Draw all frames", ButtonKind::Primary).small().icon(ph::SPARKLE).loading(self.enqueuing);
        let all_done = !d.rows.is_empty() && approved == d.rows.len() && outdated == 0;
        let draw = if all_done && !self.enqueuing {
            // Every shot has its frame: the next step (Animate comes in a later update) is Sound.
            let go = self.ctx.go.clone();
            btn("fr-next", "Continue to Sound", ButtonKind::Primary).small().icon(Step::Sound.icon()).tooltip("Every shot has an approved frame; give the lines their voices next").on_click(move |_, w, cx| go(Step::Sound, w, cx))
        } else if !d.bible_approved {
            draw.disabled("Approve the bible first, so every frame uses the same faces, places and style")
        } else if to_draw == 0 {
            draw.disabled("Every shot has a picture (approved, locked or waiting for your review), or is already in the queue")
        } else {
            draw.tooltip(format!("Draws the {} that have no picture yet", count(to_draw, "shot"))).on_click(cx.listener(|this, _: &ClickEvent, w, cx| this.confirm_draw_all(w, cx)))
        };
        head = head.child(draw);

        let mut body = column_body("fr-grid").track_scroll(&self.grid_scroll);
        let mut ix = 0;
        if let Some(n) = self.notices(d, cx) {
            body = body.child(n);
            ix += 1;
        }
        let mut any = false;
        for (k, g) in d.scenes.iter().enumerate() {
            let rows: Vec<usize> = visible.iter().copied().filter(|i| g.rows.contains(i)).collect();
            if rows.is_empty() {
                continue;
            }
            if jump.as_deref() == Some(g.key.as_str()) {
                self.grid_scroll.scroll_to_top_of_item(ix);
            }
            body = body.child(self.scene_section(k, g, &rows, d, any, cx));
            ix += 1;
            any = true;
        }
        if !any {
            body = body.child(empty_gallery(ph::FUNNEL, "No shot matches this filter", "Pick another filter on the left, or All."));
        }
        div().flex().flex_col().size_full().child(head).child(body)
    }

    fn outdated_menu(&self, n: usize, cx: &mut Context<Self>) -> AnyElement {
        let (a, b, c) = (cx.entity().downgrade(), cx.entity().downgrade(), cx.entity().downgrade());
        let items = vec![
            MenuItem::new("Update all", move |w, cx| {
                let _ = a.update(cx, |this, cx| this.confirm_update_all(w, cx));
            })
            .icon(ph::SPARKLE),
            MenuItem::new("Review one by one", move |_, cx| {
                let _ = b.update(cx, |this, cx| this.start_review(cx));
            })
            .icon(ph::EYE),
            MenuItem::separator(),
            MenuItem::new("Keep all as they are", move |_, cx| {
                let _ = c.update(cx, |this, cx| this.keep_all(cx));
            })
            .icon(ph::LOCK_SIMPLE),
        ];
        menu_button("fr-outdated", ph::ARROWS_CLOCKWISE, "Frames whose references or settings changed: update, review or keep", items).label(format!("{n} outdated")).small().into_any_element()
    }

    /// What stops or needs the user before the grid: the bible gate, failures, a paused queue, a
    /// failed refresh.
    fn notices(&self, d: &Data, cx: &mut Context<Self>) -> Option<Div> {
        let mut out: Vec<AnyElement> = Vec::new();
        if let Some(e) = &self.refresh_error {
            out.push(
                error_block(format!("The latest changes could not be read: {e}"))
                    .child(btn("fr-refresh", "Try again", ButtonKind::Secondary).small().icon(ph::ARROW_CLOCKWISE).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.reload(cx))))
                    .into_any_element(),
            );
        }
        if !d.bible_approved {
            out.push(bible_notice("fr-bible", d.bible_missing.len(), "faces, places and style", self.ctx.go.clone()).into_any_element());
        }
        let failed = self.busy.values().filter(|b| matches!(b, Busy::Failed { .. })).count();
        if failed > 0 {
            out.push(
                inline_alert(AlertKind::Warning, format!("{} could not be drawn", count(failed, "frame")), "Point at the red mark on a frame to see why.")
                    .action("Try them again", cx.listener(|this, _: &ClickEvent, _, cx| this.retry_failed(cx)))
                    .into_any_element(),
            );
        }
        let waiting = self.busy.values().filter(|b| matches!(b, Busy::Waiting { .. })).count();
        if self.queue_paused && waiting > 0 {
            // Why they wait; the one Resume is the queue strip's.
            out.push(inline_alert(AlertKind::Info, "The queue is paused", format!("{} waiting. They are drawn once the queue is resumed.", count(waiting, "frame"))).into_any_element());
        }
        (!out.is_empty()).then(|| div().flex().flex_col().gap(px(space::SM)).children(out))
    }

    fn scene_section(&self, k: usize, g: &SceneGroup, rows: &[usize], d: &Data, spaced: bool, cx: &mut Context<Self>) -> Div {
        let approved = d.rows[g.rows.clone()].iter().filter(|r| is_approved(&r.shot.frame)).count();
        let head = div()
            .id(("fr-section", k))
            .flex()
            .items_center()
            .gap(px(space::SM))
            .child(label_caps(&format!("Scene {}", g.number)))
            .when(!g.title.is_empty(), |h| h.child(div().min_w_0().truncate().text_size(px(size::SM)).text_color(Tokens::text_2()).child(g.title.clone())))
            .child(div().flex_1())
            .child(spec(format!("{approved} / {} approved", g.rows.len())));
        let mut tiles = div().flex().flex_wrap().gap(px(space::MD));
        for &i in rows {
            tiles = tiles.child(self.tile(i, &d.rows[i], cx));
        }
        div().flex().flex_col().gap(px(space::SM)).when(spaced, |s| s.pt(px(space::XS))).child(head).child(tiles)
    }

    fn tile(&self, i: usize, r: &Row, cx: &mut Context<Self>) -> Stateful<Div> {
        let busy = self.busy.get(&r.id).cloned().unwrap_or(Busy::Idle);
        let group = SharedString::from(format!("fr-tile-{i}"));
        let card = &r.shot.frame;
        let has = card.selected.is_some();
        let picture = div()
            .relative()
            .w_full()
            .h(px(TILE_H))
            .overflow_hidden()
            .bg(Tokens::surface_2())
            .map(|d| match &r.still {
                Some(p) => d.child(img(mv_ui::thumbs::cover_source(p.clone())).size_full().object_fit(ObjectFit::Cover)),
                None => d
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(space::XS))
                    .child(icon_ph(ph::IMAGE, Tokens::text_3(), icon_px::MD))
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(if card.locked { "Locked without a picture" } else { "No picture yet" })),
            })
            .children(busy_marks(i, &busy, self.web_held));
        let mut actions = hover_actions(group.clone()).p(px(space::XXS)).rounded(px(radius::MD)).bg(Tokens::popover());
        if has && card.status != Status::Approved {
            let id = r.id.clone();
            actions = actions.child(icon_button(("fr-t-approve", i), ph::CHECK, "Approve this frame").small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                this.set_status(&id, Status::Approved, cx);
            })));
        } else if has {
            // Approved: an inert "Approved" mark keeps Approve's place, so the second click of a
            // double click lands on it rather than on Reject.
            actions = actions.child(icon_button(("fr-t-approved", i), ph::CHECK, "Approved").small().selected(true).on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.stop_propagation())));
        }
        if has && card.status != Status::Rejected {
            let id = r.id.clone();
            actions = actions.child(icon_button(("fr-t-reject", i), ph::X, "Reject this frame").small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                this.set_status(&id, Status::Rejected, cx);
            })));
        }
        if let Busy::Failed { id: job, .. } = busy {
            actions = actions.child(icon_button(("fr-t-retry", i), ph::ARROW_CLOCKWISE, "Try again").small().on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                backends::ensure_runner(cx);
                queue::retry(job);
            })));
        }
        let id = r.id.clone();
        actions = actions.child(icon_button(("fr-t-upload", i), ph::UPLOAD_SIMPLE, "Use your own picture for this shot").small().on_click(cx.listener(move |this, _: &ClickEvent, w, cx| {
            cx.stop_propagation();
            this.pick_upload(id.clone(), w, cx);
        })));
        let caption = div()
            .flex()
            .flex_col()
            .gap(px(space::XS))
            .p(px(space::SM))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::XS))
                    .child(div().pr(px(space::XXS)).text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(r.number.clone()))
                    .child(spec(size_label(r.shot.camera.size)))
                    .when_some(secs_label(shot_secs(&r.shot)), |d, s| d.child(spec(s))),
            )
            .child(card_marks(card.status, card.locked, r.outdated));
        let (open, drop) = (r.id.clone(), r.id.clone());
        selectable(("fr-tile", i), self.recent.as_deref() == Some(r.id.as_str()))
            .relative()
            .group(group)
            .w(px(TILE_W))
            .overflow_hidden()
            .drag_over::<ExternalPaths>(|s, _, _, _| s.border_color(Tokens::accent()).bg(Tokens::accent_soft()))
            .on_drop(cx.listener(move |this, p: &ExternalPaths, _, cx| this.upload(drop.clone(), p.paths().to_vec(), cx)))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_shot(Some(open.clone()), cx)))
            .child(picture)
            .child(caption)
            .child(actions)
    }

    // ------------------------------------------------------------ the open shot

    fn viewer(&self, d: &Data, i: usize, visible: &[usize], cx: &mut Context<Self>) -> Div {
        let r = &d.rows[i];
        let busy = self.busy.get(&r.id).cloned().unwrap_or(Busy::Idle);
        let nav: Vec<String> = if visible.contains(&i) { visible.iter().map(|&k| d.rows[k].id.clone()).collect() } else { d.rows.iter().map(|r| r.id.clone()).collect() };
        let scene_title = d.scenes.iter().find(|g| g.rows.contains(&i)).map(|g| if g.title.is_empty() { format!("Scene {}", g.number) } else { format!("Scene {} · {}", g.number, g.title) }).unwrap_or_default();

        let prev = match neighbour(&nav, &r.id, false) {
            Some(p) => icon_button("fr-prev", ph::CARET_LEFT, "Previous shot").small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_shot(Some(p.clone()), cx))),
            None => icon_button("fr-prev", ph::CARET_LEFT, "Previous shot").small().disabled("This is the first shot here"),
        };
        let next = match neighbour(&nav, &r.id, true) {
            Some(n) => icon_button("fr-next", ph::CARET_RIGHT, "Next shot").small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_shot(Some(n.clone()), cx))),
            None => icon_button("fr-next", ph::CARET_RIGHT, "Next shot").small().disabled("This is the last shot here"),
        };
        let left = d.rows.iter().filter(|r| r.outdated && !self.being_updated(&r.id)).count();
        let head = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(space::SM))
            .h(px(COLUMN_HEAD_H))
            .px(px(space::MD))
            .border_b_1()
            .border_color(Tokens::border())
            .child(btn("fr-back", "All shots", ButtonKind::Ghost).small().icon(ph::ARROW_LEFT).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.open_shot(None, cx))))
            .child(label_caps(&format!("Shot {}", r.number)))
            .child(div().flex_1().min_w_0().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(scene_title))
            .when(self.reviewing, |h| {
                h.child(tag(format!("Reviewing outdated · {left} left"), Tone::Warning)).child(btn("fr-review-stop", "Stop", ButtonKind::Ghost).small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.reviewing = false;
                    cx.notify();
                })))
            })
            .child(prev)
            .child(next)
            .child(history_button("fr-history", self.history_open).small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.history_open = !this.history_open;
                cx.notify();
            })));

        let mut body = column_body("fr-viewer").gap(px(space::LG)).child(self.toolbar(r, cx)).child(self.still(i, r, &busy, cx));
        // Sheets this shot would use but cannot yet, because they are not approved (one quiet line).
        if !r.prepared.left_out.is_empty() {
            let text = r.prepared.left_out.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
            body = body.child(info_line(ph::INFO, LineTone::Neutral, text));
        }
        if r.outdated {
            body = body.child(self.outdated_block(r, &busy, cx));
        }
        if let Some(round) = self.round(r, cx) {
            body = body.child(round);
        }
        if self.history_open {
            if let Some(h) = &self.history {
                body = body.child(card().child(h.clone()));
            }
        }
        body = body.child(self.about(r, cx));
        div().flex().flex_col().size_full().child(head).child(body)
    }

    fn toolbar(&self, r: &Row, cx: &mut Context<Self>) -> Div {
        let card = &r.shot.frame;
        let has = card.selected.is_some();
        let approved = card.status == Status::Approved && has;
        let rejected = card.status == Status::Rejected;
        let (a, b, c, u) = (r.id.clone(), r.id.clone(), r.id.clone(), r.id.clone());
        let approve = btn("fr-approve", if approved { "Approved" } else { "Approve" }, ButtonKind::Secondary).small().icon(ph::CHECK).selected(approved);
        // Approve only ever approves and shows "Approved" in its place (a double click leaves it
        // approved); taking an approval back is its own small button on the other side of the
        // row, never under the pointer.
        let approve = if !has {
            approve.disabled("Make or upload a picture first")
        } else if approved {
            approve.tooltip("The animatic uses this picture")
        } else {
            approve.tooltip("Use this picture in the animatic").on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_status(&a, Status::Approved, cx)))
        };
        let back = approved.then(|| {
            let id = r.id.clone();
            btn("fr-unapprove", "Back to review", ButtonKind::Ghost)
                .small()
                .icon(ph::ARROW_COUNTER_CLOCKWISE)
                .tooltip("Take the approval back: the frame waits for your review again")
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_status(&id, Status::NeedsReview, cx)))
        });
        let reject = btn("fr-reject", if rejected { "Rejected" } else { "Reject" }, ButtonKind::Ghost).small().icon(ph::X).selected(rejected);
        let reject = if !has {
            reject.disabled("There is no picture to reject yet")
        } else {
            reject
                .tooltip(if rejected { "Press again to review it later" } else { "Not usable: Draw all frames draws this shot again" })
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_status(&b, if rejected { Status::NeedsReview } else { Status::Rejected }, cx)))
        };
        let locked = card.locked;
        let lock = btn("fr-lock", if locked { "Locked" } else { "Lock" }, ButtonKind::Ghost)
            .small()
            .icon(if locked { ph::LOCK_SIMPLE } else { ph::LOCK_SIMPLE_OPEN })
            .selected(locked)
            .tooltip(if locked { "Locked: Draw all frames skips it and it is never marked outdated. Press to unlock" } else { "Keep this frame as it is: Draw all frames skips it" })
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_locked(&[c.clone()], !locked, cx)));
        let upload = btn("fr-upload", "Upload your own", ButtonKind::Ghost)
            .small()
            .icon(ph::UPLOAD_SIMPLE)
            .tooltip("Use your own picture for this shot (or drop a file on the picture)")
            .on_click(cx.listener(move |this, _: &ClickEvent, w, cx| this.pick_upload(u.clone(), w, cx)));
        // Narrow window: the picture panel opens from here as a drawer (on the left of the row,
        // so the open drawer never covers its own switch).
        let settings = self.narrow.then(|| {
            icon_button("fr-drawer", ph::SLIDERS_HORIZONTAL, if self.drawer { "Close the picture settings" } else { "Picture settings" }).small().selected(self.drawer).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.drawer = !this.drawer;
                cx.notify();
            }))
        });
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(space::SM))
            .children(settings)
            .child(card_marks(card.status, card.locked, r.outdated))
            .children(back)
            .child(div().flex_1())
            .child(approve)
            .child(reject)
            .child(lock)
            .child(upload)
    }

    fn still(&self, i: usize, r: &Row, busy: &Busy, cx: &mut Context<Self>) -> Div {
        let drop = r.id.clone();
        let frame = div()
            .id("fr-still")
            .relative()
            .w_full()
            .max_w(px(STILL_MAX_W))
            .aspect_ratio(16. / 9.)
            .rounded(px(radius::LG))
            .overflow_hidden()
            .bg(Tokens::well())
            .border_1()
            .border_color(Tokens::border())
            .drag_over::<ExternalPaths>(|s, _, _, _| s.border_color(Tokens::accent()).bg(Tokens::accent_soft()))
            .on_drop(cx.listener(move |this, p: &ExternalPaths, _, cx| this.upload(drop.clone(), p.paths().to_vec(), cx)))
            .map(|d| match &r.still {
                Some(p) => d.child(img(mv_ui::thumbs::display_source(p.clone())).size_full().object_fit(ObjectFit::Contain)),
                None => d
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(space::SM))
                    .p(px(space::LG))
                    .child(ph_icon(ph::IMAGE, Weight::Duotone, Tokens::text_3(), icon_px::HERO))
                    .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("No picture yet"))
                    .child(div().max_w(px(TILE_W * 1.5)).text_center().text_size(px(size::XS)).text_color(Tokens::text_3()).whitespace_normal().child("Press Generate in the panel to draw it, or drop your own picture here.")),
            })
            .children(busy_marks(i, busy, self.web_held));
        div().flex().justify_center().child(frame)
    }

    fn outdated_block(&self, r: &Row, busy: &Busy, cx: &mut Context<Self>) -> Div {
        let (u, k) = (r.id.clone(), r.id.clone());
        let queued = in_queue(Some(busy)) || self.being_updated(&r.id);
        let update = btn("fr-od-update", "Update", ButtonKind::Secondary).small().icon(ph::SPARKLE);
        let update = if queued {
            update.disabled("Already being drawn again")
        } else {
            update.tooltip("Draw it again with what changed; this picture stays in Earlier versions").on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.update_one(&u, cx)))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .p(px(space::MD))
            .rounded(px(radius::LG))
            .bg(Tokens::warning_soft())
            .border_1()
            .border_color(Tokens::warning_border())
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(space::SM))
                    .child(div().pt(px(1.)).child(icon_ph(ph::ARROWS_CLOCKWISE, Tokens::warning(), icon_px::SM)))
                    .child(div().flex_1().min_w_0().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_1()).whitespace_normal().child(if queued {
                        "Outdated: it is being drawn again with what changed. The new picture is put in use when it arrives."
                    } else {
                        "Outdated: a reference or a picture setting of this shot changed after this picture was made."
                    })),
            )
            .child(
                div().flex().gap(px(space::SM)).child(update).child(
                    btn("fr-od-keep", "Keep", ButtonKind::Ghost)
                        .small()
                        .icon(ph::LOCK_SIMPLE)
                        .tooltip("Keep this picture and lock it, so it is no longer marked outdated")
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.keep(&k, cx))),
                ),
            )
    }

    /// The latest round of variants, side by side, to pick from.
    fn round(&self, r: &Row, cx: &mut Context<Self>) -> Option<Div> {
        let selected = r.shot.frame.selected.as_deref();
        let worth = r.round.len() > 1 || r.round.first().is_some_and(|(id, _)| Some(id.as_str()) != selected);
        if !worth {
            return None;
        }
        let mut tiles = div().flex().flex_wrap().gap(px(space::MD));
        for (k, (take, path)) in r.round.iter().enumerate() {
            let in_use = selected == Some(take.as_str());
            let group = SharedString::from(format!("fr-var-{k}"));
            let (pick, ok, shot, shot2) = (take.clone(), take.clone(), r.id.clone(), r.id.clone());
            let approve_hint = if in_use { "Approve this one" } else { "Use this one and approve it" };
            tiles = tiles.child(
                selectable(("fr-var", k), in_use)
                    .relative()
                    .group(group.clone())
                    .w(px(TILE_W))
                    .overflow_hidden()
                    .tooltip(tip(if in_use { "In use" } else { "Use this one" }))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.choose(&shot, pick.clone(), false, cx)))
                    .child(div().w_full().h(px(TILE_H)).overflow_hidden().bg(Tokens::surface_2()).flex().items_center().justify_center().map(|d| match path {
                        Some(p) => d.child(img(mv_ui::thumbs::cover_source(p.clone())).size_full().object_fit(ObjectFit::Cover)),
                        None => d.child(icon_ph(ph::IMAGE_BROKEN, Tokens::text_3(), icon_px::MD)),
                    }))
                    .child(div().flex().items_center().h(px(36.)).px(px(space::SM)).map(|d| {
                        if in_use {
                            d.child(tag("In use", Tone::Accent))
                        } else {
                            d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("Picture {}", k + 1)))
                        }
                    }))
                    .child(hover_actions(group).p(px(space::XXS)).rounded(px(radius::MD)).bg(Tokens::popover()).child(icon_button(("fr-var-ok", k), ph::CHECK, approve_hint).small().on_click(cx.listener(
                        move |this, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            this.choose(&shot2, ok.clone(), true, cx);
                        },
                    )))),
            );
        }
        Some(div().flex().flex_col().gap(px(space::SM)).child(label_row("Latest pictures", "Click one to use it")).child(tiles))
    }

    /// What the shot tells, and the way back to the AI's references.
    fn about(&self, r: &Row, cx: &mut Context<Self>) -> Div {
        let tells = r.shot.tells.trim();
        let mut s = section("What it shows").child(div().text_size(px(size::SM)).line_height(px(18.)).text_color(if tells.is_empty() { Tokens::text_3() } else { Tokens::text_2() }).whitespace_normal().child(if tells.is_empty() {
            "The shot plan says nothing about this shot yet.".to_string()
        } else {
            tells.to_string()
        }));
        if r.shot.frame.gen.refs_user {
            let id = r.id.clone();
            s = s.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::XS))
                    .child(info_line(ph::INFO, LineTone::Neutral, "You chose this shot's references by hand, so the AI no longer picks them."))
                    .child(div().child(btn("fr-ai-refs", "Let the AI pick them again", ButtonKind::Ghost).small().icon(ph::ARROW_COUNTER_CLOCKWISE).tooltip("Pinned references stay").on_click(cx.listener(
                        move |this, _: &ClickEvent, _, cx| this.ai_refs(&id, cx),
                    )))),
            );
        }
        s
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: with test-support, `gpui_kit::*` carries a `test` macro that shadows `#[test]`.
    use super::{
        accept_status_click, busy_map, draw_note, frame_ext, is_approved, latest_round, needs_drawing, neighbour, next_outdated, scene_order, secs_label, shot_secs, size_label, time_note,
        Filter, STATUS_CLICK_GAP,
    };
    use crate::gen_panel::Busy;
    // The rule is the engine's; the screen passes the hash of the request it built.
    use mv_anime::frames::outdated_for as is_outdated;
    use mv_anime::queue::{Job, JobKind, JobState, Lane};
    use mv_anime::{Card, Shot, ShotSize, Status, Take, Target};
    use std::path::Path;

    fn card(status: Status, selected: bool, locked: bool) -> Card {
        Card { status, selected: selected.then(|| "t1".to_string()), locked, ..Default::default() }
    }

    fn frame_take(hash: &str) -> Take {
        Take { request_hash: hash.into(), target: Target::ShotFrame { episode: "ep01".into(), shot: "sh1".into() }, ..Default::default() }
    }

    #[test]
    fn a_double_click_never_undoes_an_approval() {
        let t0 = std::time::Instant::now();
        let mut last = None;
        assert!(accept_status_click(&mut last, "s1", t0));
        // The second click of a double click on the same picture is ignored.
        assert!(!accept_status_click(&mut last, "s1", t0 + std::time::Duration::from_millis(250)));
        // Another picture is not held back.
        assert!(accept_status_click(&mut last, "s2", t0 + std::time::Duration::from_millis(300)));
        // A later, deliberate click counts.
        assert!(accept_status_click(&mut last, "s2", t0 + std::time::Duration::from_millis(300) + STATUS_CLICK_GAP));
    }

    #[test]
    fn only_unapproved_unlocked_shots_are_drawn() {
        assert!(needs_drawing(&card(Status::Draft, false, false)));
        // A picture waiting for review (drawn or uploaded) is left alone.
        assert!(!needs_drawing(&card(Status::NeedsReview, true, false)));
        assert!(needs_drawing(&card(Status::NeedsReview, false, false)));
        assert!(needs_drawing(&card(Status::Rejected, true, false)));
        // "Approved" without a picture is not an approved still.
        assert!(needs_drawing(&card(Status::Approved, false, false)));
        assert!(!needs_drawing(&card(Status::Approved, true, false)));
        assert!(!needs_drawing(&card(Status::Draft, false, true)));
        assert!(is_approved(&card(Status::Approved, true, false)));
        assert!(!is_approved(&card(Status::Approved, false, false)));
    }

    #[test]
    fn outdated_follows_the_request_of_the_picture_in_use() {
        let c = card(Status::Approved, true, false);
        assert!(!is_outdated(&c, Some(&frame_take("abc")), "abc"));
        assert!(is_outdated(&c, Some(&frame_take("abc")), "def"));
        // Kept (locked), nothing in use, or the user's own upload: never outdated.
        assert!(!is_outdated(&card(Status::Approved, true, true), Some(&frame_take("abc")), "def"));
        assert!(!is_outdated(&c, None, "def"));
        let upload = Take { target: Target::Upload, ..Default::default() };
        assert!(!is_outdated(&c, Some(&upload), "def"));
    }

    #[test]
    fn filters() {
        let review = card(Status::NeedsReview, true, false);
        assert!(Filter::All.passes(&review, false));
        assert!(Filter::Review.passes(&review, false));
        // A picture waiting for review (an upload too) has left "To draw".
        assert!(!Filter::ToDraw.passes(&review, false));
        assert!(Filter::ToDraw.passes(&card(Status::Draft, false, false), false));
        assert!(Filter::ToDraw.passes(&card(Status::Rejected, true, false), false));
        assert!(!Filter::Outdated.passes(&review, false));
        assert!(Filter::Outdated.passes(&review, true));
        assert!(Filter::Approved.passes(&card(Status::Approved, true, false), false));
        assert!(!Filter::Review.passes(&card(Status::NeedsReview, false, false), false));
        assert_eq!(Filter::ALL.len(), 5);
        assert_eq!(Filter::ToDraw.label(), "To draw");
    }

    #[test]
    fn the_latest_round_is_the_newest_batch() {
        let t = |id: &str, b: Option<&str>| (id.to_string(), b.map(str::to_string));
        assert!(latest_round(&[]).is_empty());
        assert_eq!(latest_round(&[t("a", Some("x")), t("b", Some("y")), t("c", Some("y"))]), vec!["b", "c"]);
        // An upload after a round stands alone.
        assert_eq!(latest_round(&[t("a", Some("x")), t("b", Some("x")), t("u", None)]), vec!["u"]);
    }

    #[test]
    fn scenes_keep_the_episode_order_then_the_rest() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(scene_order(&s(&["b", "a"]), &s(&["a", "b", "c"]), &s(&["a", "z"])), s(&["b", "a", "z"]));
        assert_eq!(scene_order(&[], &s(&["a", "b"]), &s(&["b"])), s(&["a", "b"]));
        assert_eq!(scene_order(&[], &[], &s(&["q"])), s(&["q"]));
    }

    #[test]
    fn plain_words() {
        assert_eq!(size_label(ShotSize::Cu), "Close-up");
        assert_eq!(size_label(ShotSize::Unknown), "Shot");
        assert_eq!(secs_label(2.84).as_deref(), Some("2.8 s"));
        assert_eq!(secs_label(0.), None);
        assert_eq!(secs_label(f32::NAN), None);
        let shot = Shot { planned_s: 3., locked_s: Some(4.5), ..Default::default() };
        assert_eq!(shot_secs(&shot), 4.5);
        let shot = Shot { planned_s: 3., locked_s: Some(0.), ..Default::default() };
        assert_eq!(shot_secs(&shot), 3.);
        assert_eq!(frame_ext(Path::new("a/b.JPEG")), Some("jpg"));
        assert_eq!(frame_ext(Path::new("b.webp")), Some("webp"));
        assert_eq!(frame_ext(Path::new("b.gif")), None);
        assert_eq!(frame_ext(Path::new("noext")), None);
    }

    #[test]
    fn moving_between_shots() {
        let ids: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(neighbour(&ids, "b", true).as_deref(), Some("c"));
        assert_eq!(neighbour(&ids, "b", false).as_deref(), Some("a"));
        assert_eq!(neighbour(&ids, "a", false), None);
        assert_eq!(neighbour(&ids, "c", true), None);
        assert_eq!(neighbour(&ids, "x", true), None);
        let o = |v: &[(&str, bool)]| v.iter().map(|(a, b)| (a.to_string(), *b)).collect::<Vec<_>>();
        let order = o(&[("a", true), ("b", false), ("c", true), ("d", false)]);
        assert_eq!(next_outdated(&order, "a").as_deref(), Some("c"));
        // Wraps round, never the current one.
        assert_eq!(next_outdated(&order, "c").as_deref(), Some("a"));
        assert_eq!(next_outdated(&o(&[("a", true)]), "a"), None);
        assert_eq!(next_outdated(&[], "a"), None);
    }

    #[test]
    fn confirm_texts() {
        let t = draw_note(3, 0, 2, Some(40.));
        assert!(t.contains("3 shots") && t.contains("2 pictures each") && t.contains("About 2 min in all."));
        assert!(!t.contains("waiting for your review"));
        assert!(draw_note(1, 0, 0, None).contains("1 shot on Magnific") && draw_note(1, 0, 0, None).contains("1 picture each"));
        assert!(draw_note(10, 3, 1, None).starts_with("3 shots already have a picture waiting for your review and are left as they are."));
        assert_eq!(time_note(2, Some(f32::NAN)), "The time is shown once the first frame is done.");
    }

    #[test]
    fn busy_per_shot_of_this_episode_only() {
        let job = |id: u64, ep: &str, shot: &str, state: JobState| Job { id, project: "p".into(), lane: Lane::Web, state, kind: JobKind::Frame { episode: ep.into(), shot: shot.into(), variants: 2 }, ..Default::default() };
        let jobs = vec![job(1, "ep01", "s1", JobState::Running), job(2, "ep01", "s2", JobState::Queued), job(3, "ep02", "s3", JobState::Queued), job(4, "ep01", "s4", JobState::Failed)];
        let m = busy_map(&jobs, "p", "ep01");
        assert_eq!(m.len(), 3);
        assert!(matches!(m.get("s1"), Some(Busy::Working { id: 1, .. })));
        assert!(matches!(m.get("s2"), Some(Busy::Waiting { id: 2, ahead: 1 })));
        assert!(matches!(m.get("s4"), Some(Busy::Failed { id: 4, .. })));
        assert!(busy_map(&jobs, "other", "ep01").is_empty());
    }
}
