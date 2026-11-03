//! The run: READ -> BREAKDOWN -> for each scene ASSETS -> BLOCKING -> PREVIZ + RENDER -> REVIEW.
//! A run stops after each rendered scene so the user can look at it (unless they chose to let
//! it carry on), and continues from `episode.json` the next time. When every scene is approved
//! the clips are joined into the episode video.
//!
//! The UI follows a run through [`live`] (polled) and reads the saved [`Episode`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Context, Result};
use mv_ai::{CancellationToken, Registry};
use mv_recap::{Brain, ChapterRead, ChapterRef, ModelChoice, Progress, ReadOptions};

use crate::ai::Patient;
use crate::build::{Builder, Got};
use crate::library::{Category, Library};
use crate::plan::{Episode, SceneStatus};
use crate::stage::{self, RenderSettings};

/// Everything a run needs from the app.
#[derive(Clone)]
pub struct Ctx {
    pub reg: Arc<Registry>,
    pub model: ModelChoice,
    /// Overrides [`crate::blender::find`].
    pub blender: Option<PathBuf>,
    /// Look for generic props and skies on Poly Haven.
    pub online: bool,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub samples: u32,
    /// Where the library lives (tests); `None` = the app's library.
    pub library: Option<PathBuf>,
    /// Where episodes live (tests); `None` = [`crate::paths::episode_dir`].
    pub episodes: Option<PathBuf>,
}

impl Ctx {
    pub fn new(reg: Arc<Registry>, model: ModelChoice) -> Ctx {
        Ctx { reg, model, blender: None, online: true, width: 1280, height: 720, fps: 24, samples: 16, library: None, episodes: None }
    }

    pub fn episode_dir(&self, ch: &ChapterRef) -> PathBuf {
        match &self.episodes {
            Some(root) => root.join(mv_core::paths::safe_name(&ch.manga_id)).join(mv_core::paths::safe_name(&ch.chapter_id)),
            None => crate::paths::episode_dir(&ch.manga_id, &ch.chapter_id),
        }
    }

    fn open_library(&self) -> Library {
        match &self.library {
            Some(p) => Library::open(p),
            None => Library::open_default(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Keep going after each scene instead of stopping for review (scenes stay "Ready for review").
    pub keep_going: bool,
    /// Only this scene (0-based), e.g. a redo.
    pub only: Option<usize>,
    /// Plan the episode again from scratch (keeps the library).
    pub replan: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Stage(String),
    Log(String),
    Waiting(String),
    Scene { index: usize, status: SceneStatus },
    Done,
    Failed(String),
}

/// What a running (or the last) run of a chapter is doing, for the UI.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Live {
    pub running: bool,
    pub stage: String,
    pub log: Vec<String>,
    pub waiting: Option<String>,
    pub error: Option<String>,
    /// Bumped on every change, so the UI knows to reload `episode.json`.
    pub rev: u64,
}

struct Slot {
    live: Live,
    cancel: CancellationToken,
}

fn slots() -> &'static Mutex<HashMap<String, Slot>> {
    static S: std::sync::OnceLock<Mutex<HashMap<String, Slot>>> = std::sync::OnceLock::new();
    S.get_or_init(Default::default)
}

/// The live state of a chapter's run (`None` if it never ran in this session).
pub fn live(chapter_id: &str) -> Option<Live> {
    slots().lock().unwrap_or_else(|e| e.into_inner()).get(chapter_id).map(|s| s.live.clone())
}

/// Stops a chapter's run after the step it is in.
pub fn stop(chapter_id: &str) {
    if let Some(s) = slots().lock().unwrap_or_else(|e| e.into_inner()).get(chapter_id) {
        s.cancel.cancel();
    }
}

fn emit(chapter_id: &str, e: &Event) {
    let mut g = slots().lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = g.get_mut(chapter_id) else { return };
    let l = &mut s.live;
    l.rev += 1;
    match e {
        Event::Stage(t) => {
            l.stage = t.clone();
            l.waiting = None;
            l.log.push(t.clone());
        }
        Event::Log(t) => l.log.push(t.clone()),
        Event::Waiting(t) => l.waiting = Some(t.clone()),
        Event::Scene { .. } => l.waiting = None,
        Event::Done => {
            l.running = false;
            l.waiting = None;
        }
        Event::Failed(m) => {
            l.running = false;
            l.error = Some(m.clone());
            l.log.push(format!("Stopped: {m}"));
        }
    }
    if l.log.len() > 300 {
        let cut = l.log.len() - 300;
        l.log.drain(..cut);
    }
}

pub fn load_episode(dir: &Path) -> Option<Episode> {
    Episode::load(dir)
}

/// The joined episode video, if it was made.
pub fn episode_video(ep: &Episode) -> Option<PathBuf> {
    ep.video.clone().filter(|p| p.is_file())
}

/// The user is happy with a scene.
pub fn approve(dir: &Path, index: usize) -> Result<Episode> {
    let mut ep = Episode::load(dir).ok_or_else(|| anyhow!("no episode"))?;
    let s = ep.scenes.get_mut(index).ok_or_else(|| anyhow!("no scene {index}"))?;
    if s.render.is_none() {
        bail!("scene {} has not been rendered yet", index + 1);
    }
    s.status = SceneStatus::Approved;
    ep.save(dir)?;
    Ok(ep)
}

/// The user wants a scene made again, optionally saying what to change.
pub fn redo(dir: &Path, index: usize, note: Option<String>) -> Result<Episode> {
    let mut ep = Episode::load(dir).ok_or_else(|| anyhow!("no episode"))?;
    let s = ep.scenes.get_mut(index).ok_or_else(|| anyhow!("no scene {index}"))?;
    s.status = SceneStatus::Planned;
    s.note = note.filter(|n| !n.trim().is_empty());
    s.blocking = None;
    s.error = None;
    ep.video = None;
    ep.save(dir)?;
    Ok(ep)
}

/// Runs the chapter's pipeline (see module docs). Returns the saved episode.
pub async fn run(ctx: Ctx, ch: ChapterRef, opts: RunOptions) -> Result<Episode> {
    let id = ch.chapter_id.clone();
    let cancel = {
        let mut g = slots().lock().unwrap_or_else(|e| e.into_inner());
        if g.get(&id).is_some_and(|s| s.live.running) {
            bail!("this chapter is already running");
        }
        let cancel = CancellationToken::new();
        g.insert(id.clone(), Slot { live: Live { running: true, ..Default::default() }, cancel: cancel.clone() });
        cancel
    };
    let ev = {
        let id = id.clone();
        move |e: Event| emit(&id, &e)
    };
    let r = run_inner(&ctx, &ch, &opts, &cancel, &ev).await;
    match &r {
        Ok(_) => ev(Event::Done),
        Err(e) if e.to_string() == "cancelled" => {
            ev(Event::Log("Stopped.".into()));
            ev(Event::Done);
        }
        Err(e) => ev(Event::Failed(format!("{e:#}"))),
    }
    r
}

async fn run_inner(ctx: &Ctx, ch: &ChapterRef, opts: &RunOptions, cancel: &CancellationToken, ev: &(dyn Fn(Event) + Send + Sync)) -> Result<Episode> {
    let blender = ctx.blender.clone().or_else(crate::blender::find).ok_or_else(|| anyhow!(crate::blender::SETUP_MESSAGE))?;
    let scripts = crate::scripts::install()?;
    let ffmpeg = crate::blender::ffmpeg();
    if ffmpeg.is_none() {
        ev(Event::Log("ffmpeg was not found: scenes are kept as frames, without a clip.".into()));
    }
    let dir = ctx.episode_dir(ch);
    std::fs::create_dir_all(&dir)?;
    let ev_arc: Arc<dyn Fn(String) + Send + Sync> = {
        let id = ch.chapter_id.clone();
        Arc::new(move |m: String| emit(&id, &Event::Waiting(m)))
    };
    let llm = Patient { reg: ctx.reg.clone(), on_wait: ev_arc };
    let log = |m: String| ev(Event::Log(m));

    // READ: what mv-recap knows about the pages; read the chapter first if it never was.
    let brain_arc = Arc::new(Mutex::new(Brain::load(&ch.manga_id, &ch.manga_title)));
    let read = ensure_read(ctx, ch, &brain_arc, cancel, ev).await?;
    let brain = brain_arc.lock().unwrap_or_else(|e| e.into_inner()).clone();

    let mut lib = ctx.open_library();
    let mut ep = match Episode::load(&dir) {
        Some(ep) if !opts.replan && !ep.scenes.is_empty() => ep,
        _ => {
            ev(Event::Stage("Breaking the chapter into scenes and assets".into()));
            let (title, logline, assets, scenes) = crate::breakdown::run(&llm, &ctx.model, ch, &read, &brain, &lib.catalog_for(&ch.manga_id), cancel).await?;
            let mut ep = Episode {
                manga_id: ch.manga_id.clone(),
                manga_title: ch.manga_title.clone(),
                chapter_id: ch.chapter_id.clone(),
                label: ch.label.clone(),
                title: if title.is_empty() { ch.label.clone() } else { title },
                logline,
                assets,
                scenes,
                model: format!("{}/{}", ctx.model.provider.as_str(), ctx.model.model),
                created: crate::util::now(),
                ..Default::default()
            };
            log(format!("{} scenes, {} assets to make or reuse", ep.scenes.len(), ep.assets.len()));
            // Mark what the library already has, so the UI shows it right away.
            for n in ep.assets.iter_mut() {
                n.asset_id = crate::build::existing(&lib, n, &ch.manga_id);
            }
            ep.save(&dir)?;
            ep
        }
    };
    let builder = Builder { llm: &llm, model: &ctx.model, blender: &blender, scripts: &scripts, cancel, log: &log, online: ctx.online };

    // The world map of the manga, with this episode's locations pinned.
    if ep.world_map.is_none() {
        let locs: Vec<(String, Option<[f32; 2]>)> = ep.assets.iter().filter(|a| a.category == Category::Environments).map(|a| (a.name.clone(), a.map_pos)).collect();
        match builder.world_map(&mut lib, ch, &locs).await {
            Ok(id) => {
                ep.world_map = Some(id);
                ep.save(&dir)?;
            }
            Err(e) if cancel.is_cancelled() => return Err(e),
            Err(e) => log(format!("The world map could not be drawn: {e:#}")),
        }
    }

    let rs = RenderSettings { width: ctx.width, height: ctx.height, fps: ctx.fps, samples: ctx.samples };
    let order: Vec<usize> = match opts.only {
        Some(i) if i < ep.scenes.len() => vec![i],
        Some(i) => bail!("there is no scene {}", i + 1),
        None => (0..ep.scenes.len()).collect(),
    };
    for i in order {
        let st = ep.scenes[i].status;
        if opts.only.is_none() && matches!(st, SceneStatus::Approved | SceneStatus::Review) {
            continue;
        }
        if cancel.is_cancelled() {
            bail!("cancelled");
        }
        match make_scene(ctx, ch, &mut ep, &dir, i, &read, &brain, &mut lib, &builder, &rs, ffmpeg.as_deref(), &scripts, &blender, cancel, ev).await {
            Ok(()) => {}
            Err(e) if cancel.is_cancelled() => {
                ep.scenes[i].status = SceneStatus::Planned;
                ep.save(&dir)?;
                return Err(e);
            }
            Err(e) => {
                let s = &mut ep.scenes[i];
                s.status = SceneStatus::Failed;
                s.error = Some(format!("{e:#}"));
                ep.save(&dir)?;
                ev(Event::Scene { index: i, status: SceneStatus::Failed });
                return Err(e.context(format!("scene {}", i + 1)));
            }
        }
        if !opts.keep_going {
            ev(Event::Stage(format!("Scene {} is ready for review", i + 1)));
            break;
        }
    }
    finish_episode(&mut ep, &dir, ffmpeg.as_deref(), ev).await?;
    Ok(ep)
}

async fn ensure_read(ctx: &Ctx, ch: &ChapterRef, brain: &Arc<Mutex<Brain>>, cancel: &CancellationToken, ev: &(dyn Fn(Event) + Send + Sync)) -> Result<ChapterRead> {
    let total = mv_recap::list_pages(&ch.pages_dir).len();
    if let Some(r) = mv_recap::load_read(ch) {
        if !r.pages.is_empty() && r.pages.len() + 1 >= total {
            return Ok(r);
        }
    }
    ev(Event::Stage(format!("Reading the chapter's {total} pages")));
    let id = ch.chapter_id.clone();
    let progress: mv_recap::ProgressFn = Arc::new(move |p| match p {
        Progress::Page { done, total, .. } => emit(&id, &Event::Log(format!("Read page {done} of {total}"))),
        Progress::Waiting { message, .. } => emit(&id, &Event::Waiting(message)),
        Progress::Character { name, new: true } => emit(&id, &Event::Log(format!("Met {name}"))),
        _ => {}
    });
    let r = mv_recap::read_chapter(ctx.reg.clone(), ctx.model.clone(), ch.clone(), brain.clone(), ReadOptions::default(), progress, cancel.clone()).await?;
    if let Ok(b) = brain.lock() {
        let _ = b.save();
    }
    Ok(r)
}

#[allow(clippy::too_many_arguments)]
async fn make_scene(
    ctx: &Ctx,
    ch: &ChapterRef,
    ep: &mut Episode,
    dir: &Path,
    i: usize,
    read: &ChapterRead,
    brain: &Brain,
    lib: &mut Library,
    builder: &Builder<'_>,
    rs: &RenderSettings,
    ffmpeg: Option<&Path>,
    scripts: &Path,
    blender: &Path,
    cancel: &CancellationToken,
    ev: &(dyn Fn(Event) + Send + Sync),
) -> Result<()> {
    let set = |ep: &mut Episode, st: SceneStatus| -> Result<()> {
        ep.scenes[i].status = st;
        ep.save(dir)?;
        ev(Event::Scene { index: i, status: st });
        Ok(())
    };
    let title = ep.scenes[i].title.clone();
    ev(Event::Stage(format!("Scene {}: {} - assets", i + 1, title)));
    set(ep, SceneStatus::Assets)?;
    // Pick up what other runs added to the library meanwhile.
    lib.save()?;
    for key in ep.scene_needs(i) {
        let need = ep.need(&key).cloned().context("need")?;
        let got = builder.ensure(lib, &need, ch, read, brain).await?;
        match &got {
            Got::Reused(id) => ev(Event::Log(format!("Reused {} \"{}\" from the library", need.category.label().to_lowercase(), lib.get(id).map(|a| a.name.as_str()).unwrap_or(id)))),
            Got::Built(_) => ev(Event::Log(format!("Added \"{}\" to the library", need.name))),
        }
        lib.mark_used(got.id(), &ch.chapter_id);
        if let Some(n) = ep.assets.iter_mut().find(|n| n.key == key) {
            n.asset_id = Some(got.id().to_string());
        }
        ep.save(dir)?;
    }
    lib.save()?;

    let scene = ep.scenes[i].clone();
    let actors = stage::actors(ep, &scene, lib);
    let env = ep.need(&scene.location).and_then(|n| n.asset_id.as_deref()).and_then(|id| lib.get(id)).filter(|a| lib.ready(a)).cloned();
    let env_info = env.as_ref().map(|a| (a.name.as_str(), a.dims));

    ev(Event::Stage(format!("Scene {}: {} - blocking the shots", i + 1, title)));
    set(ep, SceneStatus::Blocking)?;
    let blocking = match stage::block(builder.llm, &ctx.model, ep, &scene, read, brain, &actors, env_info, cancel).await {
        Ok(b) => b,
        Err(e) if cancel.is_cancelled() => return Err(e),
        Err(e) => {
            ev(Event::Log(format!("The AI's blocking was unusable ({e:#}); using a simple one")));
            stage::fallback_blocking(&actors, &scene)
        }
    };
    ep.scenes[i].blocking = Some(blocking.clone());
    ep.save(dir)?;

    // Exteriors get a real sky.
    let exterior = ep.need(&scene.location).and_then(|n| n.interior) == Some(false) || !matches!(scene.time_of_day.as_str(), "" | "interior");
    let sky = if exterior && ep.need(&scene.location).and_then(|n| n.interior) != Some(true) {
        let words = ep.need(&scene.location).map(|n| crate::util::tokens(&format!("{} {}", n.name, n.tags.join(" ")))).unwrap_or_default();
        match builder.sky(lib, &scene.time_of_day, &words).await {
            Ok(s) => s,
            Err(e) => {
                ev(Event::Log(format!("No online sky ({e:#}); using a plain one")));
                None
            }
        }
    } else {
        None
    };

    ev(Event::Stage(format!("Scene {}: {} - previz and render", i + 1, title)));
    set(ep, SceneStatus::Rendering)?;
    // Each take renders into its own folder; the last good take stays whole until a new one succeeds.
    let scene_dir = dir.join("scenes").join(format!("s{:02}", i + 1));
    let out = scene_dir.join(format!("take_{}", ep.scenes[i].takes + 1));
    let _ = std::fs::remove_dir_all(&out);
    let env_blend = env.as_ref().map(|a| lib.blend_of(a));
    let spec = stage::scene_spec(&blocking, &actors, env_blend.as_deref(), sky.as_deref(), &scene, rs, &out);
    let render = match stage::render(blender, scripts, ffmpeg, &spec, &out, rs.fps, cancel).await {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::copy(out.join("blender.log"), scene_dir.join("failed_take.log"));
            let _ = std::fs::remove_dir_all(&out);
            return Err(e);
        }
    };
    ev(Event::Log(format!("Rendered {:.1} s in {} shots", render.seconds, render.shots.len())));
    let s = &mut ep.scenes[i];
    if let Some(old) = s.render.replace(render) {
        // The previous take's folder (older episodes rendered straight into the scene folder).
        if let Some(old_dir) = old.frames_dir.parent().filter(|d| *d != out.as_path() && d.starts_with(&scene_dir) && *d != scene_dir.as_path()) {
            let _ = std::fs::remove_dir_all(old_dir);
        }
    }
    s.takes += 1;
    s.error = None;
    set(ep, SceneStatus::Review)?;
    Ok(())
}

/// Joins the approved scenes' clips into `episode.mp4` once every scene is approved.
async fn finish_episode(ep: &mut Episode, dir: &Path, ffmpeg: Option<&Path>, ev: &(dyn Fn(Event) + Send + Sync)) -> Result<()> {
    if !ep.all_approved() || ep.video.as_ref().is_some_and(|v| v.is_file()) {
        return Ok(());
    }
    let Some(ff) = ffmpeg else { return Ok(()) };
    let clips: Vec<PathBuf> = ep.scenes.iter().filter_map(|s| s.render.as_ref()?.clip.clone()).filter(|c| c.is_file()).collect();
    if clips.len() != ep.scenes.len() {
        return Ok(());
    }
    ev(Event::Stage("Joining the scenes into the episode".into()));
    let out = dir.join("episode.mp4");
    crate::blender::concat_clips(ff, &clips, &out).await?;
    ep.video = Some(out);
    ep.save(dir)?;
    Ok(())
}

/// Every approved scene is in; join the episode now (the UI calls this after the last approval).
pub async fn join_episode(dir: PathBuf) -> Result<Episode> {
    let mut ep = Episode::load(&dir).ok_or_else(|| anyhow!("no episode"))?;
    let ff = crate::blender::ffmpeg().ok_or_else(|| anyhow!("ffmpeg is needed to join the scenes"))?;
    ep.video = None;
    finish_episode(&mut ep, &dir, Some(&ff), &|_| {}).await?;
    Ok(ep)
}

