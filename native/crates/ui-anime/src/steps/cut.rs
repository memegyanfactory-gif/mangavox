//! Step 8, Cut (design 9.11) [cut]: the episode as an animatic. W3-S5.
//!
//! The centre is a large preview that plays the cut in the window (each shot's still with its
//! planned camera move from `animatic::camera_at`, cut on time, the voices in sync), with play,
//! pause, a scrub bar and the strip of shots under it (each with its still, length and move; the
//! one playing is outlined). The right column makes the MP4 (and its subtitle file) through the
//! engine's queue, shows its progress, and opens the result in the app's player or in its folder.
//!
//! The preview's sound is the episode's dialogue track, mixed once per cut by the engine
//! (`animatic::mix_dialogue`) and kept in `work/cut/preview/`; the audio player cannot seek, so
//! playing from the middle plays a copy that starts there.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ai::CancellationToken;
use mv_anime::animatic::{self, CutPlan, CutShot};
use mv_anime::queue::{self, Job, JobKind, JobState};
use mv_anime::{Status, Store};
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::{
    btn, empty_view, error_view, icon_button, icon_ph, icon_tag, ph, progress_bar, size, skeleton_block, skeleton_line, spinner, switch, tip, toast, ButtonKind, Route, Toast, Tokens, Tone,
};

use super::preview::{self, Clock, Moment};
use super::shared::scene_order;
use super::{Step, StepCtx};
use crate::backends;
use crate::style::{COLUMN_HEAD_H, GUTTER, HEADER_H, HISTORY_THUMB, POLL_MS, QUEUE_H, STEPBAR_H};
use crate::widgets::{ago, card, column_body, column_head, count, error_block, fmt_duration, fmt_secs, info_line, is_narrow, label_row, notice, now_secs, section, selectable, spec, workspace, LineTone};

/// The preview never gets shorter than this, whatever the window.
const STAGE_MIN_H: f32 = 220.;
/// Height of the transport row under the preview.
const TRANSPORT_H: f32 = 32.;
/// The shot label at the end of the transport row ("Shot 12.14 · Background slide").
const SHOT_LABEL_W: f32 = 184.;
/// Window parts outside the step that the stage must leave room for: the app's own title bar
/// and the line about the preview's sound under the transport.
const OUTSIDE_H: f32 = 40. + 32. + space::MD;
/// Height of the scrub bar's hit area; its track and knob sit in the middle.
const SCRUB_H: f32 = space::XXL;
/// A playing preview that has not been drawn for this long is off screen: it pauses.
const HIDDEN_PAUSE: Duration = Duration::from_millis(900);
/// Playing within this of the end starts again from the beginning.
const END_SLACK: f32 = 0.05;
/// A play copy this old belongs to no play of this session that could still start.
const LEFTOVER_AGE: Duration = Duration::from_secs(120);

// ---------------------------------------------------------------- pure parts

/// The latest Animatic job of this episode.
fn latest_job(jobs: &[Job], project: &str, episode: &str) -> Option<Job> {
    jobs.iter().filter(|j| j.project == project && matches!(&j.kind, JobKind::Animatic { episode: e } if e == episode)).max_by_key(|j| j.id).cloned()
}

/// A finished animatic in the exports folder (not one still being written).
fn is_export(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".mp4") && !n.ends_with(".part.mp4")
}

/// The cache name of the preview's dialogue mix: it changes only when what is heard changes
/// (which takes, where they start, how long the cut is), not when a picture changes.
fn mix_key(plan: &CutPlan) -> String {
    let lines: Vec<serde_json::Value> =
        plan.shots.iter().flat_map(|s| s.lines.iter().map(move |(f, o)| serde_json::json!([f, ((s.start_s + o) as f64 * 1000.).round() as i64]))).collect();
    let h = mv_anime::hash::value_hash(&serde_json::json!({ "v": 1, "frames": animatic::frames(plan.seconds), "lines": lines }));
    h.chars().take(16).collect()
}

/// "4 shots have no approved frame".
fn no_frame_title(n: usize) -> String {
    format!("{} no approved frame", if n == 1 { "1 shot has".to_string() } else { format!("{n} shots have") })
}

/// "3 lines have no voice yet".
fn no_voice_title(n: usize) -> String {
    format!("{} no voice yet", if n == 1 { "1 line has".to_string() } else { format!("{n} lines have") })
}

/// The widest the preview may be so it, its controls and the strip fit the window's height.
fn stage_max_w(viewport_h: f32) -> f32 {
    let strip = HISTORY_THUMB.1 + size::SM + size::XS + space::XXL + space::LG;
    let chrome = OUTSIDE_H + HEADER_H + STEPBAR_H + QUEUE_H + COLUMN_HEAD_H + 2. * GUTTER + 2. * space::MD + TRANSPORT_H + 3. * space::LG + strip;
    (viewport_h - chrome).max(STAGE_MIN_H) * 16. / 9.
}

/// The stage's size in the preview column measured last frame: `center` is the whole column
/// (w, h, its head included), `strip_h` the shot strip's height, `below_h` the controls under
/// the stage. As large as fits once those are in, 16:9, never shorter than [`STAGE_MIN_H`]
/// unless the column is too narrow. `None` before the first layout.
fn stage_size(center: (f32, f32), strip_h: f32, below_h: f32) -> Option<(f32, f32)> {
    let ok = |v: f32| v.is_finite() && v > 0.;
    if !ok(center.0) || !ok(center.1) || !ok(strip_h) || !ok(below_h) {
        return None;
    }
    // The body's padding, its gap before the strip, and the gap between the stage and controls.
    let chrome = COLUMN_HEAD_H + 2. * space::MD + space::LG + space::MD;
    let h = (center.1 - chrome - strip_h - below_h).max(STAGE_MIN_H);
    let w = (h * 16. / 9.).min(center.0 - 2. * space::MD).max(1.);
    Some((w, w * 9. / 16.))
}

/// An invisible layer that keeps `cell` at its parent's size (give the parent `.relative()`),
/// redrawing when the size changes.
fn measure(cell: Rc<Cell<(f32, f32)>>) -> impl IntoElement {
    canvas(
        move |b, window, _| {
            let now = (f32::from(b.size.width), f32::from(b.size.height));
            let old = cell.get();
            if (now.0 - old.0).abs() > 0.5 || (now.1 - old.1).abs() > 0.5 {
                cell.set(now);
                window.refresh();
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
}

/// A background result as an error text, if it failed.
fn failure(res: anyhow::Result<anyhow::Result<()>>) -> Option<String> {
    match res {
        Ok(Ok(())) => None,
        Ok(Err(e)) | Err(e) => Some(format!("{e:#}")),
    }
}

// ---------------------------------------------------------------- reading the episode

/// A finished animatic on disk.
#[derive(Clone)]
struct Export {
    path: PathBuf,
    name: String,
    /// Unix seconds it was written.
    at: i64,
}

struct Data {
    plan: CutPlan,
    /// Per shot of the plan: "3.4", the still's file, whether its frame is approved.
    numbers: Vec<String>,
    stills: Vec<Option<PathBuf>>,
    approved: Vec<bool>,
    no_frame: usize,
    /// Spoken lines with no voice yet / with one.
    no_voice: usize,
    voiced: usize,
    ffmpeg: bool,
    latest: Option<Export>,
    /// The timing hash of the last export (`cut.json`).
    exported: String,
    /// The last export matches the cut as it would be made now: its timing and the settings it
    /// was made with (subtitles…), `animatic::export_current`.
    current: bool,
    /// "Episode 1".
    episode_name: String,
    /// "Solo Leveling · Episode 1 animatic".
    player_title: String,
    /// Learned length of an export, in seconds.
    eta: Option<f32>,
    mix_key: String,
    /// The preview's dialogue mix for this cut, when it is ready.
    mix: Option<PathBuf>,
}

enum Load {
    Loading,
    Ready(Box<Data>),
    Failed(String),
}

fn cut_file(store: &Store, ep: &str) -> PathBuf {
    store.path(&format!("episodes/{}/cut.json", mv_core::paths::safe_name(ep)))
}

fn exports_dir(store: &Store, ep: &str) -> PathBuf {
    store.path(&format!("episodes/{}/exports", mv_core::paths::safe_name(ep)))
}

/// `work/cut/preview/`: the preview's dialogue mix and the copies it plays from the middle.
fn preview_dir(store: &Store) -> PathBuf {
    store.work_dir().join("cut").join("preview")
}

/// The newest finished animatic of a folder.
fn latest_export(dir: &Path) -> Option<Export> {
    let rd = std::fs::read_dir(dir).ok()?;
    rd.flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            if !is_export(&name) || !path.is_file() {
                return None;
            }
            let at = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
            Some(Export { path, name, at })
        })
        .max_by(|a, b| a.at.cmp(&b.at).then_with(|| a.name.cmp(&b.name)))
}

/// Plans the cut and reads what the screen shows. Blocking.
fn load(store: &Store, ep: &str) -> anyhow::Result<Data> {
    let plan = animatic::plan_cut(store, ep)?;
    let series = store.series()?;
    let episode = store.episode(ep)?;
    let story = store.story(ep)?;
    let all = store.all_shots(ep)?;
    let story_ids: Vec<String> = story.scenes.iter().map(|s| s.id.clone()).collect();
    let planned: Vec<String> = all.iter().filter(|(_, v)| !v.is_empty()).map(|(k, _)| k.clone()).collect();
    let mut numbers: HashMap<&str, String> = HashMap::new();
    let mut approved_of: HashMap<&str, bool> = HashMap::new();
    for (i, key) in scene_order(&episode.scene_order, &story_ids, &planned).iter().enumerate() {
        let Some(shots) = all.get(key) else { continue };
        for (j, s) in shots.iter().enumerate() {
            numbers.insert(s.id.as_str(), format!("{}.{}", i + 1, j + 1));
            approved_of.insert(s.id.as_str(), s.frame.status == Status::Approved && s.frame.selected.is_some());
        }
    }
    let numbers: Vec<String> = plan.shots.iter().enumerate().map(|(k, s)| numbers.get(s.shot.as_str()).cloned().unwrap_or_else(|| (k + 1).to_string())).collect();
    let stills: Vec<Option<PathBuf>> = plan.shots.iter().map(|s| s.still.as_deref().map(|f| store.path(f)).filter(|p| p.is_file())).collect();
    let approved: Vec<bool> = plan.shots.iter().map(|s| approved_of.get(s.shot.as_str()).copied().unwrap_or(false)).collect();
    let no_frame = approved.iter().filter(|a| !**a).count();
    let no_voice = plan.shots.iter().map(|s| s.missing.iter().filter(|m| m.starts_with("voice")).count()).sum();
    let voiced = plan.shots.iter().map(|s| s.lines.len()).sum();
    let saved: CutPlan = mv_anime::store::read_json(&cut_file(store, ep)).unwrap_or_default();
    let key = mix_key(&plan);
    let mix = Some(preview_dir(store).join(format!("mix-{key}.wav"))).filter(|p| p.is_file());
    let episode_name = if episode.title.trim().is_empty() { ep.to_string() } else { episode.title.trim().to_string() };
    let player_title = if series.manga_title.trim().is_empty() { format!("{episode_name} animatic") } else { format!("{} · {episode_name} animatic", series.manga_title.trim()) };
    Ok(Data {
        numbers,
        stills,
        approved,
        no_frame,
        no_voice,
        voiced,
        ffmpeg: mv_anime::ffmpeg::ffmpeg().is_some(),
        latest: latest_export(&exports_dir(store, ep)),
        current: animatic::export_current(&saved, &plan),
        exported: saved.timing_hash,
        episode_name,
        player_title,
        eta: queue::eta(&JobKind::Animatic { episode: ep.to_string() }),
        mix_key: key,
        mix,
        plan,
    })
}

/// Mixes the cut's voices with the engine and keeps the track for the preview.
async fn mix_for_preview(store: &Store, plan: &CutPlan, key: &str, cancel: &CancellationToken) -> anyhow::Result<PathBuf> {
    let wav = animatic::mix_dialogue(store, plan, |_| {}, cancel).await?;
    let dir = preview_dir(store);
    std::fs::create_dir_all(&dir).map_err(|e| anyhow!("Could not create the preview folder ({e}). Check that the data drive has free space."))?;
    let out = dir.join(format!("mix-{key}.wav"));
    let part = dir.join(format!("mix-{key}.part.wav"));
    std::fs::copy(&wav, &part).map_err(|e| anyhow!("Could not keep the preview's voices ({e}). Check that the data drive has free space."))?;
    std::fs::rename(&part, &out).map_err(|e| anyhow!("Could not keep the preview's voices ({e}). Close other programs using the project folder and try again."))?;
    // Mixes of earlier cuts are no longer needed (one still playing stays until next time).
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p != out && e.file_name().to_string_lossy().starts_with("mix-") {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    Ok(out)
}

/// The play number of a copy named `play-{n}.wav`.
fn play_seq_of(name: &str) -> Option<u64> {
    name.strip_prefix("play-")?.strip_suffix(".wav")?.parse().ok()
}

/// The sound to play from `t` seconds: the mix itself from the start, else a copy of it that
/// starts at `t` (copies made for earlier plays are removed). Blocking.
fn clip_from(mix: &Path, dir: &Path, t: f32, seq: u64) -> anyhow::Result<PathBuf> {
    if t < END_SLACK {
        return Ok(mix.to_path_buf());
    }
    let bytes = std::fs::read(mix).map_err(|e| anyhow!("Could not read the preview's voices ({e})."))?;
    let Some(tail) = preview::wav_tail(&bytes, t) else { bail!("The preview's voices could not be read as a sound file.") };
    let out = dir.join(format!("play-{seq}.wav"));
    // Only copies made for earlier plays go (and any left from an earlier session): a newer
    // play's copy, written moments ago, may be about to start.
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let Some(n) = play_seq_of(&e.file_name().to_string_lossy()) else { continue };
            let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|m| m.elapsed().ok()).is_some_and(|age| age > LEFTOVER_AGE);
            if n < seq || old {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    std::fs::write(&out, tail).map_err(|e| anyhow!("Could not prepare the preview's voices ({e}). Check that the data drive has free space."))?;
    Ok(out)
}

fn open_player(path: &Path, title: String, window: &mut Window, cx: &mut App) {
    mv_ui::navigate(
        Route::Player {
            request: mv_ui::nav::PlayRequest { title, source: path.to_string_lossy().into_owned(), file_idx: None, imdb_id: None, mal_id: None, season: None, episode: None },
        },
        window,
        cx,
    );
}

// ---------------------------------------------------------------- the screen

pub struct CutStep {
    ctx: StepCtx,
    store: Store,
    data: Load,
    /// A refresh failed while older data is shown.
    refresh_error: Option<String>,
    loading: bool,
    again: bool,
    /// The project changed while this step was not drawn: plan again when it is next drawn.
    stale: bool,
    /// (store version, queue version) last seen.
    seen: (u64, u64),
    /// The latest Animatic job of this episode, and whether the queue is held.
    job: Option<Job>,
    held: bool,
    /// The job being watched, to say when it is done.
    watch: Option<u64>,
    /// "Make the animatic" was pressed and its job has not shown up as waiting or running yet:
    /// the button already says "Starting…" and takes no second press.
    starting_export: bool,
    // The preview.
    clock: Clock,
    playing: bool,
    /// Waiting for the sound before the clock runs.
    starting: bool,
    /// Bumped on every play, pause and seek, so a late sound never starts.
    play_seq: u64,
    /// The sound file playing now.
    clip: Option<PathBuf>,
    /// The scrub bar is held; it was playing when pressed.
    scrubbing: Option<bool>,
    scrub_area: Rc<Cell<Bounds<Pixels>>>,
    /// Sizes measured last frame (w, h): the preview column, the shot strip, the controls under
    /// the stage. The stage is sized from them (`stage_size`).
    center_size: Rc<Cell<(f32, f32)>>,
    strip_size: Rc<Cell<(f32, f32)>>,
    below_size: Rc<Cell<(f32, f32)>>,
    last_render: Instant,
    /// The shot shown last (the strip follows it while playing).
    shown: Option<usize>,
    strip_scroll: ScrollHandle,
    // The preview's sound.
    mixing: bool,
    mix_cancel: Option<CancellationToken>,
    /// (mix key, message) of a mix that failed.
    mix_error: Option<(String, String)>,
}

pub fn open(ctx: StepCtx, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| {
        let store = Store::open(&ctx.project);
        let mut step = CutStep {
            ctx,
            store,
            data: Load::Loading,
            refresh_error: None,
            loading: false,
            again: false,
            stale: false,
            seen: (mv_anime::store::version(), queue::version()),
            job: None,
            held: false,
            watch: None,
            starting_export: false,
            clock: Clock::default(),
            playing: false,
            starting: false,
            play_seq: 0,
            clip: None,
            scrubbing: None,
            scrub_area: Rc::new(Cell::new(Bounds::default())),
            center_size: Rc::new(Cell::new((0., 0.))),
            strip_size: Rc::new(Cell::new((0., 0.))),
            below_size: Rc::new(Cell::new((0., 0.))),
            last_render: Instant::now(),
            shown: None,
            strip_scroll: ScrollHandle::new(),
            mixing: false,
            mix_cancel: None,
            mix_error: None,
        };
        step.refresh_job(cx);
        step.reload(cx);
        step.poll(window, cx);
        step
    })
    .into()
}

impl CutStep {
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
        // Another step is showing (or the window is hidden): the preview stops with its sound.
        if self.playing && self.last_render.elapsed() > HIDDEN_PAUSE {
            self.pause();
            cx.notify();
        }
        let v = (mv_anime::store::version(), queue::version());
        if v == self.seen {
            return;
        }
        let (files, jobs) = (v.0 != self.seen.0, v.1 != self.seen.1);
        self.seen = v;
        if jobs {
            self.refresh_job(cx);
        }
        if files {
            // Planned again when next drawn: a step that is not on screen is never drawn, so
            // it does not re-read the project on every write a running job makes.
            self.stale = true;
        }
        cx.notify();
    }

    fn refresh_job(&mut self, cx: &mut Context<Self>) {
        let q = queue::snapshot();
        self.held = q.paused || q.interrupted;
        let job = latest_job(&q.jobs, &self.ctx.project, &self.ctx.episode);
        if let Some(j) = &job {
            if matches!(j.state, JobState::Queued | JobState::Running | JobState::Paused) {
                self.watch = Some(j.id);
            } else if self.watch == Some(j.id) {
                self.watch = None;
                if j.state == JobState::Done {
                    self.announce(cx);
                }
            }
        }
        // The pressed export is now in the queue (or already finished): its own state shows.
        if self.starting_export && self.job_active_or_newer(job.as_ref()) {
            self.starting_export = false;
        }
        self.job = job;
        // A finished export frees the work folder for the preview's sound.
        self.prepare_sound(cx);
    }

    /// The export just finished: say so, with a way to watch it.
    fn announce(&mut self, cx: &mut Context<Self>) {
        let dir = exports_dir(&self.store, &self.ctx.episode);
        let title = self.player_title();
        toast::show(
            cx,
            Toast::success("The animatic is ready").icon(ph::FILM_STRIP).action("Play", move |w, cx| {
                if let Some(e) = latest_export(&dir) {
                    open_player(&e.path, title.clone(), w, cx);
                }
            }),
        );
    }

    fn player_title(&self) -> String {
        match &self.data {
            Load::Ready(d) => d.player_title.clone(),
            _ => "Animatic".into(),
        }
    }

    /// Plans the cut again in the background (a second call while reading runs once more after).
    fn reload(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            self.again = true;
            return;
        }
        self.loading = true;
        let (store, ep) = (self.store.clone(), self.ctx.episode.clone());
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || load(&store, &ep)).await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match res {
                    Ok(Ok(d)) => {
                        let heard_changed = matches!(&this.data, Load::Ready(old) if old.mix_key != d.mix_key);
                        let resume = heard_changed && this.playing;
                        if resume {
                            // The voices moved: the old sound no longer fits.
                            this.pause();
                        }
                        this.data = Load::Ready(Box::new(d));
                        this.refresh_error = None;
                        if resume {
                            // Carry on from here; silent until the new sound is mixed, which
                            // then joins in (see `prepare_sound`).
                            this.play(cx);
                        }
                    }
                    Ok(Err(e)) | Err(e) => {
                        let msg = format!("{e:#}");
                        mv_core::logging::global().warn("anime", &format!("cut: could not plan the cut: {msg}"));
                        if matches!(this.data, Load::Ready(_)) {
                            this.refresh_error = Some(msg);
                        } else {
                            this.data = Load::Failed(msg);
                        }
                    }
                }
                this.prepare_sound(cx);
                if std::mem::take(&mut this.again) {
                    this.reload(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn total(&self) -> f32 {
        match &self.data {
            Load::Ready(d) => d.plan.seconds,
            _ => 0.,
        }
    }

    fn export_busy(&self) -> bool {
        self.starting_export || self.job.as_ref().is_some_and(|j| matches!(j.state, JobState::Queued | JobState::Running | JobState::Paused))
    }

    /// The watched export job is in the queue, or done since it was pressed.
    fn job_active_or_newer(&self, job: Option<&Job>) -> bool {
        job.is_some_and(|j| self.watch.is_some_and(|w| j.id >= w) || matches!(j.state, JobState::Queued | JobState::Running | JobState::Paused))
    }

    // ------------------------------------------------------------ the preview's sound

    /// Mixes the cut's voices for the preview when they are not ready yet. Never while an export
    /// waits or runs: it writes the same work files.
    fn prepare_sound(&mut self, cx: &mut Context<Self>) {
        let Load::Ready(d) = &self.data else { return };
        if self.mixing || d.mix.is_some() || !d.ffmpeg || d.voiced == 0 || self.export_busy() {
            return;
        }
        if self.mix_error.as_ref().is_some_and(|(k, _)| *k == d.mix_key) {
            return;
        }
        let (store, plan, key) = (self.store.clone(), d.plan.clone(), d.mix_key.clone());
        let token = CancellationToken::new();
        self.mix_cancel = Some(token.clone());
        self.mixing = true;
        cx.spawn(async move |this, cx| {
            let (k, t) = (key.clone(), token.clone());
            let res = mv_ui::bg::run(async move { mix_for_preview(&store, &plan, &k, &t).await }).await;
            let _ = this.update(cx, |this, cx| {
                this.mixing = false;
                this.mix_cancel = None;
                match res {
                    Ok(Ok(p)) => {
                        let mut joined = false;
                        if let Load::Ready(d) = &mut this.data {
                            if d.mix_key == key {
                                d.mix = Some(p);
                                joined = true;
                            }
                        }
                        // Playing silent while it was mixed: the voices join in from here.
                        if joined && this.playing && !this.starting && this.clip.is_none() {
                            let t = this.clock.now();
                            this.seek(t, cx);
                        }
                    }
                    Ok(Err(e)) | Err(e) => {
                        if !token.is_cancelled() {
                            let msg = format!("{e:#}");
                            mv_core::logging::global().warn("anime", &format!("cut: the preview's voices could not be mixed: {msg}"));
                            this.mix_error = Some((key, msg));
                        }
                    }
                }
                // The cut may have changed meanwhile.
                this.prepare_sound(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn retry_sound(&mut self, cx: &mut Context<Self>) {
        self.mix_error = None;
        self.prepare_sound(cx);
        cx.notify();
    }

    // ------------------------------------------------------------ playing

    fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.playing {
            self.pause();
        } else {
            self.play(cx);
        }
        cx.notify();
    }

    fn play(&mut self, cx: &mut Context<Self>) {
        let total = self.total();
        if total <= 0. {
            return;
        }
        let mut t = self.clock.now();
        if t >= total - END_SLACK {
            t = 0.;
            self.clock.set(0.);
        }
        self.playing = true;
        self.play_seq += 1;
        let seq = self.play_seq;
        let mix = match &self.data {
            Load::Ready(d) => d.mix.clone(),
            _ => None,
        };
        let Some(mix) = mix else {
            self.clock.run();
            return;
        };
        self.starting = true;
        let dir = preview_dir(&self.store);
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || clip_from(&mix, &dir, t, seq)).await;
            let _ = this.update(cx, |this, cx| {
                // Paused or moved meanwhile: that sound is no longer wanted.
                if this.play_seq != seq || !this.playing {
                    return;
                }
                this.starting = false;
                match res {
                    Ok(Ok(clip)) => {
                        ui_studio::audio::play(&clip);
                        this.clip = Some(clip);
                    }
                    Ok(Err(e)) | Err(e) => mv_core::logging::global().warn("anime", &format!("cut: the preview plays without its voices: {e:#}")),
                }
                this.clock.run();
                cx.notify();
            });
        })
        .detach();
    }

    fn pause(&mut self) {
        self.clock.hold();
        self.playing = false;
        self.starting = false;
        self.play_seq += 1;
        if let Some(c) = self.clip.take() {
            if ui_studio::audio::now_playing().as_deref() == Some(c.as_path()) {
                ui_studio::audio::stop();
            }
        }
    }

    /// Jumps to `t` and keeps playing if it was.
    fn seek(&mut self, t: f32, cx: &mut Context<Self>) {
        let was = self.playing;
        if was {
            self.pause();
        }
        self.clock.set(t.clamp(0., self.total().max(0.)));
        if was {
            self.play(cx);
        }
        cx.notify();
    }

    fn scrub_time(&self, x: Pixels) -> f32 {
        let b = self.scrub_area.get();
        preview::time_at(f32::from(x), f32::from(b.origin.x), f32::from(b.size.width), self.total())
    }

    fn scrub_down(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let was = self.playing;
        if was {
            self.pause();
        }
        self.scrubbing = Some(was);
        self.clock.set(self.scrub_time(x));
        cx.notify();
    }

    fn scrub_move(&mut self, x: Pixels, cx: &mut Context<Self>) {
        if self.scrubbing.is_some() {
            self.clock.set(self.scrub_time(x));
            cx.notify();
        }
    }

    fn scrub_up(&mut self, cx: &mut Context<Self>) {
        if let Some(was) = self.scrubbing.take() {
            if was {
                self.play(cx);
            }
            cx.notify();
        }
    }

    // ------------------------------------------------------------ export

    fn make(&mut self, cx: &mut Context<Self>) {
        // One export at a time: a second press while it starts, waits or runs does nothing.
        if self.export_busy() {
            return;
        }
        self.starting_export = true;
        cx.notify();
        // The export mixes the same voices into the same work folder.
        if let Some(t) = self.mix_cancel.take() {
            t.cancel();
        }
        backends::ensure_runner(cx);
        let name = match &self.data {
            Load::Ready(d) => d.episode_name.clone(),
            _ => self.ctx.episode.clone(),
        };
        let id = queue::enqueue(JobKind::Animatic { episode: self.ctx.episode.clone() }, &self.ctx.project, format!("Make the animatic of {name}"));
        self.watch = Some(id);
        self.refresh_job(cx);
        cx.notify();
    }

    fn set_subtitles(&mut self, on: bool, cx: &mut Context<Self>) {
        if let Load::Ready(d) = &mut self.data {
            d.plan.subtitles = on;
        }
        let store = self.store.clone();
        let rel = format!("episodes/{}/cut.json", mv_core::paths::safe_name(&self.ctx.episode));
        cx.spawn(async move |this, cx| {
            // Read, changed and written under the file's own lock, so an export writing the cut
            // at the same moment never loses this (or loses its own).
            let res = mv_ui::bg::blocking(move || store.update_json::<CutPlan>(&rel, |saved| saved.subtitles = on).map(|_| ())).await;
            if let Some(e) = failure(res) {
                let _ = this.update(cx, |this, cx| {
                    toast::show(cx, Toast::error(format!("The subtitle setting could not be saved: {e}")));
                    this.reload(cx);
                });
            }
        })
        .detach();
        cx.notify();
    }

    fn copy_command(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(mv_anime::ffmpeg::install_cmd().to_string()));
        toast::show(cx, Toast::success("Copied. Paste it into a terminal (PowerShell or Command Prompt) and press Enter."));
    }
}

/// The project was closed or another one opened: the preview's sound stops with the step (it
/// only stops its own clip), and a preview mix still being made is no longer wanted.
impl Drop for CutStep {
    fn drop(&mut self) {
        self.pause();
        if let Some(t) = self.mix_cancel.take() {
            t.cancel();
        }
    }
}

// ---------------------------------------------------------------- view

/// A whole-step state centred in the page area.
fn centred(el: impl IntoElement) -> Div {
    div().size_full().flex().items_center().justify_center().p(px(space::XXL)).child(el)
}

impl Render for CutStep {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.last_render = Instant::now();
        if std::mem::take(&mut self.stale) {
            self.reload(cx);
        }
        let total = self.total();
        let mut t = self.clock.now();
        if self.playing && self.clock.running() && t >= total {
            self.pause();
            self.clock.set(total);
            t = total;
        }
        if self.playing {
            window.request_animation_frame();
        }
        let index = match &self.data {
            Load::Ready(d) => preview::shot_at(&d.plan.shots, t).map(|x| x.0),
            _ => None,
        };
        if index.is_some() && index != self.shown {
            if let (true, Some(i)) = (self.playing, index) {
                self.strip_scroll.scroll_to_item(i);
            }
            self.shown = index;
        }
        let body = match &self.data {
            Load::Loading => self.skeleton(window).into_any_element(),
            Load::Failed(e) => centred(error_view(format!("The cut could not be planned: {e}")).retry(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.data = Load::Loading;
                this.reload(cx);
                cx.notify();
            })))
            .into_any_element(),
            Load::Ready(d) if d.plan.shots.is_empty() => self.no_shots().into_any_element(),
            Load::Ready(d) => self.ready(d, t, window, cx).into_any_element(),
        };
        div().size_full().child(body)
    }
}

impl CutStep {
    fn skeleton(&self, window: &Window) -> Div {
        let w = stage_max_w(f32::from(window.viewport_size().height)).min(f32::from(window.viewport_size().width) * 0.5);
        let center = div().flex().flex_col().size_full().child(column_head("Preview")).child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(space::LG))
                .p(px(space::MD))
                .child(skeleton_block(px(w), px(w * 9. / 16.)))
                .child(skeleton_line(px(w)))
                .child(div().flex().gap(px(space::SM)).children((0..6).map(|_| skeleton_block(px(HISTORY_THUMB.0), px(HISTORY_THUMB.1))))),
        );
        let panel = (!is_narrow(window)).then(|| {
            div()
                .flex()
                .flex_col()
                .size_full()
                .child(column_head("Animatic"))
                .child(div().flex().flex_col().gap(px(space::MD)).p(px(space::MD)).child(skeleton_line(px(160.))).child(skeleton_block(relative(1.), px(40.))).child(skeleton_block(relative(1.), px(96.))))
                .into_any_element()
        });
        workspace(window, None, center, panel)
    }

    fn no_shots(&self) -> Div {
        let go = self.ctx.go.clone();
        centred(
            empty_view(ph::SCISSORS, "No shots to cut yet", "Cut plays your shots one after another, each with its camera move and its voices, and makes the video. Plan the shots in the Shots step first.")
                .action(btn("cu-go-shots", "Go to Shots", ButtonKind::Primary).icon(Step::Shots.icon()).on_click(move |_, w, cx| go(Step::Shots, w, cx))),
        )
    }

    fn ready(&self, d: &Data, t: f32, window: &Window, cx: &mut Context<Self>) -> Div {
        let narrow = is_narrow(window);
        let max_w = stage_max_w(f32::from(window.viewport_size().height));
        let mut body = column_body("cu-body").gap(px(space::LG));
        if let Some(e) = &self.refresh_error {
            body = body.child(
                error_block(format!("The latest changes could not be read: {e}"))
                    .child(btn("cu-refresh", "Try again", ButtonKind::Secondary).small().icon(ph::ARROW_CLOCKWISE).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.reload(cx)))),
            );
        }
        // The stage takes what is left of the column once the controls and the strip are in, so
        // the strip is always on screen; before the first layout it guesses from the window.
        let (stage_w, stage_h) = match stage_size(self.center_size.get(), self.strip_size.get().1, self.below_size.get().1) {
            Some(s) => s,
            None => {
                let w = max_w.min(f32::from(window.viewport_size().width) * 0.5);
                (w, w * 9. / 16.)
            }
        };
        body = body.child(self.player(d, t, stage_w, stage_h, cx)).child(self.strip(d, t, cx).relative().child(measure(self.strip_size.clone())));
        if narrow {
            body = body.child(card().child(self.export(d, cx)));
        }
        let center = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .child(measure(self.center_size.clone()))
            .child(column_head("Preview").child(spec(count(d.plan.shots.len(), "shot"))).child(spec(fmt_secs(d.plan.seconds))))
            .child(body);
        let panel = (!narrow).then(|| div().flex().flex_col().size_full().child(column_head("Animatic")).child(column_body("cu-export").child(self.export(d, cx))).into_any_element());
        workspace(window, None, center, panel)
    }

    // ------------------------------------------------------------ the player

    fn player(&self, d: &Data, t: f32, stage_w: f32, stage_h: f32, cx: &mut Context<Self>) -> Div {
        let at = preview::shot_at(&d.plan.shots, t);
        let moment = at.map(|(i, local)| {
            let shot = &d.plan.shots[i];
            Moment {
                shot,
                local,
                still: d.stills[i].as_deref(),
                number: d.numbers[i].as_str(),
                next: d.stills.get(i + 1).and_then(|p| p.as_deref()),
                caption: if d.plan.subtitles { preview::caption_at(shot, local) } else { None },
            }
        });
        // A fixed 16:9 size: an aspect ratio on a full-width box made the column as tall as the
        // unshrunk box, leaving a gap that pushed the strip off screen.
        let stage = div()
            .id("cu-stage")
            .flex_none()
            .w(px(stage_w))
            .h(px(stage_h))
            .rounded(px(radius::LG))
            .overflow_hidden()
            .border_1()
            .border_color(Tokens::border())
            .cursor_pointer()
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle(cx)))
            .child(preview::stage(moment));
        let below = div()
            .relative()
            .flex()
            .flex_col()
            .gap(px(space::MD))
            .w_full()
            .max_w(px(stage_w))
            .child(measure(self.below_size.clone()))
            .child(self.transport(d, t, at, cx).w_full())
            .children(self.sound_line(d, cx).map(|l| l.w_full()));
        div().flex().flex_col().items_center().gap(px(space::MD)).child(stage).child(below)
    }

    fn transport(&self, d: &Data, t: f32, at: Option<(usize, f32)>, cx: &mut Context<Self>) -> Div {
        let total = d.plan.seconds;
        let play = if self.playing {
            icon_button("cu-pause", ph::PAUSE, "Pause").selected(true).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle(cx)))
        } else {
            icon_button("cu-play", ph::PLAY, "Play the animatic").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle(cx)))
        };
        let restart = icon_button("cu-restart", ph::SKIP_BACK, "Back to the start").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.seek(0., cx)));
        let time = div().flex_none().text_size(px(size::SM)).text_color(Tokens::text_2()).whitespace_nowrap().child(format!("{} / {}", fmt_secs(t), fmt_secs(total)));
        // A fixed-width slot, so the scrub track never jumps as the label changes.
        let label = div()
            .flex()
            .flex_none()
            .justify_end()
            .w(px(SHOT_LABEL_W))
            .overflow_hidden()
            .children(at.map(|(i, _)| spec(format!("Shot {} · {}", d.numbers[i], preview::move_label(d.plan.shots[i].mv)))));
        div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .h(px(TRANSPORT_H))
            .child(play)
            .child(restart)
            .child(time)
            .child(self.scrub_bar(d, t, cx))
            .child(label)
    }

    fn scrub_bar(&self, d: &Data, t: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let total = d.plan.seconds;
        let done = preview::share(t, total);
        let area = self.scrub_area.clone();
        let track_top = (SCRUB_H - space::XS) / 2.;
        let knob = space::MD;
        let mut bar = div()
            .id("cu-scrub")
            .relative()
            .flex_1()
            .min_w(px(HISTORY_THUMB.0))
            .h(px(SCRUB_H))
            .cursor_pointer()
            .child(canvas(move |b, _, _| area.set(b), |_, _, _, _| {}).absolute().inset_0())
            .child(div().absolute().left_0().right_0().top(px(track_top)).h(px(space::XS)).rounded_full().bg(Tokens::track()));
        // A tick at every cut.
        for s in d.plan.shots.iter().skip(1) {
            bar = bar.child(div().absolute().left(relative(preview::share(s.start_s, total))).top(px(track_top - space::XXS)).w(px(1.)).h(px(space::XS + 2. * space::XXS)).bg(Tokens::border_strong()));
        }
        bar.child(div().absolute().left_0().top(px(track_top)).h(px(space::XS)).w(relative(done)).rounded_full().bg(Tokens::accent()))
            .child(
                div()
                    .absolute()
                    .left(relative(done))
                    .ml(px(-knob / 2.))
                    .top(px((SCRUB_H - knob) / 2.))
                    .size(px(knob))
                    .rounded_full()
                    .bg(Tokens::accent())
                    .border_2()
                    .border_color(Tokens::surface()),
            )
            .tooltip(tip("Drag to move through the animatic"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.scrub_down(ev.position.x, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if ev.pressed_button == Some(MouseButton::Left) {
                    this.scrub_move(ev.position.x, cx);
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.scrub_up(cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.scrub_up(cx)))
    }

    /// What the preview's sound is doing, when it is not simply ready.
    fn sound_line(&self, d: &Data, cx: &mut Context<Self>) -> Option<Div> {
        if d.voiced == 0 {
            return Some(info_line(ph::SPEAKER_SLASH, LineTone::Neutral, "No voices yet, so the preview is silent. Voice the lines in the Sound step."));
        }
        if !d.ffmpeg {
            return Some(info_line(ph::SPEAKER_SLASH, LineTone::Neutral, "The preview plays without voices until ffmpeg is installed."));
        }
        if d.mix.is_some() {
            return None;
        }
        if self.mixing {
            return Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .px(px(space::MD))
                    .py(px(space::XS))
                    .rounded(px(radius::MD))
                    .bg(Tokens::surface_2())
                    .child(spinner("cu-mix-spin", Tokens::accent(), icon_px::XS))
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child("Preparing the voices for the preview…")),
            );
        }
        if self.export_busy() {
            return Some(info_line(ph::SPEAKER_SLASH, LineTone::Neutral, "The voices join the preview once the animatic is made."));
        }
        if let Some((_, e)) = self.mix_error.as_ref().filter(|(k, _)| *k == d.mix_key) {
            return Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .child(div().flex_1().min_w_0().child(info_line(ph::WARNING, LineTone::Warning, format!("The preview plays without voices: {e}"))))
                    .child(btn("cu-mix-retry", "Try again", ButtonKind::Ghost).small().icon(ph::ARROW_CLOCKWISE).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.retry_sound(cx)))),
            );
        }
        None
    }

    // ------------------------------------------------------------ the strip

    fn strip(&self, d: &Data, t: f32, cx: &mut Context<Self>) -> Div {
        let current = preview::shot_at(&d.plan.shots, t).map(|x| x.0);
        let tiles: Vec<Stateful<Div>> = d.plan.shots.iter().enumerate().map(|(i, s)| self.strip_tile(i, s, d, current == Some(i), cx)).collect();
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .child(label_row("Shots", "Click a shot to jump to it"))
            .child(div().id("cu-strip").flex().gap(px(space::SM)).pb(px(space::XS)).overflow_x_scroll().track_scroll(&self.strip_scroll).children(tiles))
    }

    fn strip_tile(&self, i: usize, s: &CutShot, d: &Data, on: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let start = s.start_s;
        let mark = (!d.approved[i]).then(|| {
            let why = if d.stills[i].is_some() { "No approved frame yet: it shows its current picture" } else { "No picture yet: a blank card stands in" };
            div()
                .id(("cu-mark", i))
                .absolute()
                .top(px(space::XS))
                .left(px(space::XS))
                .flex()
                .items_center()
                .justify_center()
                .size(px(space::XL))
                .rounded_full()
                .bg(Tokens::popover())
                .border_1()
                .border_color(Tokens::warning_border())
                .tooltip(tip(why))
                .child(icon_ph(ph::WARNING, Tokens::warning(), icon_px::XS))
        });
        let picture = div()
            .relative()
            .w_full()
            .h(px(HISTORY_THUMB.1))
            .overflow_hidden()
            .bg(Tokens::surface_2())
            .flex()
            .items_center()
            .justify_center()
            .map(|p| match &d.stills[i] {
                Some(path) => p.child(img(mv_ui::thumbs::cover_source(path.clone())).size_full().object_fit(ObjectFit::Cover)),
                None => p.child(icon_ph(ph::IMAGE, Tokens::text_3(), icon_px::MD)),
            })
            .children(mark);
        selectable(("cu-shot", i), on)
            .flex_none()
            .w(px(preview::tile_width(s.seconds)))
            .overflow_hidden()
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.seek(start, cx)))
            .child(picture)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::XS))
                    .px(px(space::SM))
                    .pt(px(space::XS))
                    .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(d.numbers[i].clone()))
                    .child(div().flex_1())
                    .child(div().flex_none().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{:.1} s", s.seconds))),
            )
            .child(div().px(px(space::SM)).pb(px(space::SM)).truncate().text_size(px(size::XS)).text_color(Tokens::text_2()).child(preview::move_label(s.mv)))
    }

    // ------------------------------------------------------------ export

    fn export(&self, d: &Data, cx: &mut Context<Self>) -> Div {
        let (w, h) = d.plan.size;
        let subtitles = div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(space::XXS))
                    .child(div().text_size(px(size::SM)).text_color(Tokens::text_1()).child("Subtitles in the video"))
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).whitespace_normal().child("A subtitle file is always saved beside it.")),
            )
            .child(switch("cu-subs", d.plan.subtitles).on_click(cx.listener(|this, on: &bool, _, cx| this.set_subtitles(*on, cx))));
        let video = section("Video")
            .child(div().flex().flex_wrap().gap(px(space::XS)).child(spec(format!("{w}×{h}"))).child(spec(format!("{} fps", mv_anime::ffmpeg::FPS))).child(spec(fmt_secs(d.plan.seconds))).child(spec(count(d.plan.shots.len(), "shot"))))
            .child(subtitles);
        let mut col = div().flex().flex_col().gap(px(space::LG)).child(video);
        for n in self.notices(d, cx) {
            col = col.child(n);
        }
        col = col.child(self.make_block(d, cx));
        if let Some(e) = &d.latest {
            col = col.child(self.latest(d, e, cx));
        }
        col
    }

    fn notices(&self, d: &Data, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out: Vec<AnyElement> = Vec::new();
        if !d.ffmpeg {
            let command = div()
                .flex()
                .items_center()
                .gap(px(space::SM))
                .pl(px(space::MD))
                .pr(px(space::XS))
                .py(px(space::XS))
                .rounded(px(radius::MD))
                .bg(Tokens::surface_2())
                .border_1()
                .border_color(Tokens::border())
                .child(div().flex_1().min_w_0().truncate().text_size(px(size::XS)).text_color(Tokens::text_1()).child(mv_anime::ffmpeg::install_cmd()))
                .child(icon_button("cu-copy", ph::COPY, "Copy the command").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.copy_command(cx))));
            out.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::SM))
                    .child(
                        notice(ph::WARNING, LineTone::Warning, "ffmpeg is not installed", "MangaVox needs ffmpeg to make the video. Run this command in a terminal, then press Check again. The preview works without it, but silent.")
                            .child(command)
                            .child(btn("cu-ffmpeg-check", "Check again", ButtonKind::Secondary).small().icon(ph::ARROW_CLOCKWISE).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.reload(cx)))),
                    )
                    .into_any_element(),
            );
        }
        if d.no_frame > 0 {
            let go = self.ctx.go.clone();
            out.push(
                notice(ph::WARNING, LineTone::Warning, no_frame_title(d.no_frame), "They show their current picture, or a blank card when they have none. Approve them in the Frames step.")
                    .child(btn("cu-go-frames", "Go to Frames", ButtonKind::Ghost).small().icon(Step::Frames.icon()).on_click(move |_, w, cx| go(Step::Frames, w, cx)))
                    .into_any_element(),
            );
        }
        if d.no_voice > 0 {
            let go = self.ctx.go.clone();
            out.push(
                notice(ph::INFO, LineTone::Info, no_voice_title(d.no_voice), "They stay silent in the animatic.")
                    .child(btn("cu-go-sound", "Go to Sound", ButtonKind::Ghost).small().icon(Step::Sound.icon()).on_click(move |_, w, cx| go(Step::Sound, w, cx)))
                    .into_any_element(),
            );
        }
        out
    }

    /// The main action, or the export's progress, wait or failure.
    fn make_block(&self, d: &Data, cx: &mut Context<Self>) -> AnyElement {
        if let Some(j) = &self.job {
            let id = j.id;
            match j.state {
                JobState::Running => {
                    let pct = (j.progress.clamp(0., 1.) * 100.).round() as u32;
                    let line = if j.line.trim().is_empty() { "Starting…".to_string() } else { j.line.clone() };
                    return progress_card()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::SM))
                                .child(spinner("cu-job-spin", Tokens::accent(), icon_px::SM))
                                .child(div().flex_1().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Making the animatic"))
                                .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child(format!("{pct}%"))),
                        )
                        .child(progress_bar("cu-job-progress", j.progress, !self.held))
                        .child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(line))
                        .child(div().child(btn("cu-stop", "Stop", ButtonKind::Ghost).small().icon(ph::STOP).tooltip("Stop making it; nothing is lost").on_click(move |_, _, _| queue::cancel(id))))
                        .into_any_element();
                }
                JobState::Queued | JobState::Paused => {
                    let paused = j.state == JobState::Paused;
                    let text = if paused {
                        "Paused in the queue"
                    } else if self.held {
                        "Waiting: the queue is paused"
                    } else {
                        "Waiting for its turn in the queue"
                    };
                    let mut row = div().flex().gap(px(space::SM));
                    if paused {
                        row = row.child(btn("cu-resume", "Resume", ButtonKind::Secondary).small().icon(ph::PLAY).on_click(move |_, _, cx| {
                            backends::ensure_runner(cx);
                            queue::resume(id);
                        }));
                    } else if self.held {
                        row = row.child(btn("cu-resume-all", "Resume the queue", ButtonKind::Secondary).small().icon(ph::PLAY).on_click(|_, _, cx| {
                            backends::ensure_runner(cx);
                            queue::resume_all();
                        }));
                    }
                    row = row.child(btn("cu-cancel", "Cancel", ButtonKind::Ghost).small().on_click(move |_, _, _| queue::cancel(id)));
                    return progress_card()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::SM))
                                .child(icon_ph(ph::HOURGLASS, Tokens::text_2(), icon_px::SM))
                                .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(text)),
                        )
                        .child(row)
                        .into_any_element();
                }
                JobState::Failed if !j.hidden => {
                    let error = j.error.clone().unwrap_or_else(|| "It stopped without saying why.".into());
                    return div()
                        .flex()
                        .flex_col()
                        .gap(px(space::SM))
                        .child(error_block(format!("The animatic could not be made: {error}")))
                        .child(div().child(btn("cu-retry", "Try again", ButtonKind::Primary).icon(ph::ARROW_CLOCKWISE).on_click(move |_, _, cx| {
                            backends::ensure_runner(cx);
                            queue::retry(id);
                        })))
                        .into_any_element();
                }
                _ => {}
            }
        }
        if self.starting_export {
            return btn("cu-make", "Starting…", ButtonKind::Primary).large().full_width().loading(true).into_any_element();
        }
        let current = d.latest.is_some() && d.current;
        let make = btn("cu-make", if current { "Make the animatic again" } else { "Make the animatic" }, ButtonKind::Primary).large().full_width().icon(ph::FILM_STRIP);
        let make = if !d.ffmpeg {
            make.disabled("Install ffmpeg first")
        } else {
            make.tooltip("Renders the stills, camera moves and voices into an MP4 video, with a subtitle file").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.make(cx)))
        };
        let note = match d.eta {
            Some(s) if s.is_finite() && s > 0. => format!("Takes about {} on this PC. It runs in the queue, so you can keep working.", fmt_duration(s.round() as u64)),
            _ => "It runs in the queue, so you can keep working. Saved in the episode's exports folder.".to_string(),
        };
        div().flex().flex_col().gap(px(space::SM)).child(make).child(info_line(ph::INFO, LineTone::Neutral, note)).into_any_element()
    }

    fn latest(&self, d: &Data, e: &Export, cx: &mut Context<Self>) -> Div {
        let outdated = !d.exported.is_empty() && !d.current;
        let (play, folder, title) = (e.path.clone(), e.path.clone(), d.player_title.clone());
        let state = if outdated {
            info_line(ph::ARROWS_CLOCKWISE, LineTone::Warning, "The cut or its settings changed after this was made. Make it again to see the changes.").into_any_element()
        } else if d.exported.is_empty() {
            div().into_any_element()
        } else {
            div().child(icon_tag(ph::CHECK, "Up to date", Tone::Success)).into_any_element()
        };
        section("Latest animatic").child(
            card()
                .gap(px(space::SM))
                .p(px(space::MD))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .child(icon_ph(ph::FILM_STRIP, Tokens::accent(), icon_px::MD))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(e.name.clone())),
                )
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("Made {}", ago(e.at, now_secs()))))
                .child(state)
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(space::SM))
                        .child(btn("cu-open", "Play", ButtonKind::Secondary).small().icon(ph::PLAY).tooltip("Watch it in MangaVox's player").on_click(cx.listener(move |this, _: &ClickEvent, w, cx| {
                            this.pause();
                            open_player(&play, title.clone(), w, cx);
                        })))
                        .child(btn("cu-folder", "Show in folder", ButtonKind::Ghost).small().icon(ph::FOLDER_OPEN).on_click(move |_, _, cx| cx.reveal_path(&folder))),
                ),
        )
    }
}

/// The box that shows a running or waiting export.
fn progress_card() -> Div {
    div().flex().flex_col().gap(px(space::SM)).p(px(space::MD)).rounded(px(radius::LG)).bg(Tokens::surface()).border_1().border_color(Tokens::border())
}

#[cfg(test)]
mod tests {
    // Explicit imports: with test-support, `gpui_kit::*` carries a `test` macro that shadows `#[test]`.
    use super::{is_export, latest_job, mix_key, no_frame_title, no_voice_title, scene_order, stage_max_w, stage_size, STAGE_MIN_H};
    use mv_anime::animatic::{CutPlan, CutShot};
    use mv_anime::queue::{Job, JobKind, JobState};

    #[test]
    fn scenes_keep_the_episode_order_then_the_rest() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(scene_order(&[], &s(&["a", "b"]), &s(&["b", "c"])), s(&["a", "b", "c"]));
        assert_eq!(scene_order(&s(&["b", "a"]), &s(&["a", "b"]), &s(&["c"])), s(&["b", "a", "c"]));
    }

    #[test]
    fn the_latest_job_of_this_episode() {
        let job = |id, project: &str, ep: &str, state| Job { id, project: project.into(), kind: JobKind::Animatic { episode: ep.into() }, state, ..Default::default() };
        let jobs = vec![
            job(1, "p", "ep01", JobState::Failed),
            job(2, "p", "ep01", JobState::Running),
            job(3, "p", "ep02", JobState::Queued),
            job(4, "q", "ep01", JobState::Queued),
            Job { id: 5, project: "p".into(), kind: JobKind::Story { episode: "ep01".into() }, ..Default::default() },
        ];
        assert_eq!(latest_job(&jobs, "p", "ep01").map(|j| j.id), Some(2));
        assert_eq!(latest_job(&jobs, "p", "ep03").map(|j| j.id), None);
    }

    #[test]
    fn only_finished_videos_count_as_exports() {
        assert!(is_export("animatic-20261121-101500.mp4"));
        assert!(is_export("Animatic.MP4"));
        assert!(!is_export("animatic-20261121-101500.part.mp4"));
        assert!(!is_export("animatic-20261121-101500.srt"));
    }

    #[test]
    fn the_preview_mix_changes_only_with_what_is_heard() {
        let shot = |still: &str, lines: Vec<(String, f32)>| CutShot { still: Some(still.into()), start_s: 0., seconds: 3., lines, ..Default::default() };
        let plan = |shots: Vec<CutShot>| CutPlan { seconds: 3., shots, ..Default::default() };
        let a = plan(vec![shot("a.png", vec![("v1.wav".into(), 0.9)])]);
        let b = plan(vec![shot("b.png", vec![("v1.wav".into(), 0.9)])]);
        let c = plan(vec![shot("a.png", vec![("v2.wav".into(), 0.9)])]);
        let d = plan(vec![shot("a.png", vec![("v1.wav".into(), 1.2)])]);
        assert_eq!(mix_key(&a), mix_key(&b), "another picture is heard the same");
        assert_ne!(mix_key(&a), mix_key(&c), "another take sounds different");
        assert_ne!(mix_key(&a), mix_key(&d), "a moved line sounds different");
        assert_eq!(mix_key(&a).len(), 16);
    }

    #[test]
    fn plain_words() {
        assert_eq!(no_frame_title(1), "1 shot has no approved frame");
        assert_eq!(no_frame_title(4), "4 shots have no approved frame");
        assert_eq!(no_voice_title(1), "1 line has no voice yet");
        assert_eq!(no_voice_title(3), "3 lines have no voice yet");
    }

    #[test]
    fn the_preview_fits_the_window() {
        assert_eq!(stage_max_w(100.), STAGE_MIN_H * 16. / 9.);
        assert!(stage_max_w(1400.) > stage_max_w(1000.));
    }

    #[test]
    fn the_stage_leaves_room_for_the_strip() {
        // Before the first layout: no size yet.
        assert_eq!(stage_size((0., 0.), 0., 0.), None);
        // A 1920x1080 window's column: stage, controls and strip all fit its height.
        let (center, strip, below) = ((1300., 900.), 160., 40.);
        let (w, h) = stage_size(center, strip, below).unwrap_or_default();
        assert!((w / h - 16. / 9.).abs() < 1e-3);
        let used = super::COLUMN_HEAD_H + 2. * super::space::MD + super::space::LG + super::space::MD + h + strip + below;
        assert!(used <= center.1 + 0.5, "{used} > {}", center.1);
        // A narrow column: the width wins and the stage stays 16:9.
        let (w, h) = stage_size((500., 2000.), strip, below).unwrap_or_default();
        assert!(w <= 500. && (w / h - 16. / 9.).abs() < 1e-3);
        // A very short window: never shorter than the minimum.
        assert!(stage_size((1300., 300.), strip, below).unwrap_or_default().1 >= STAGE_MIN_H - 0.5);
    }
}
