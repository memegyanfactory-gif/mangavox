//! The paint queue: character sheets, expression sheets, locations, pages, panel clips and the
//! stitched chapter video, run one at a time so sheets exist before the pages that use them.
//! Each task is built from the project as it is when its turn comes (so a page gets the sheets
//! painted a moment ago), runs off the UI thread, and comes back as an [`Outcome`] to store.
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Context as _, Result};
use futures::future::BoxFuture;
use futures::FutureExt as _;
use mv_create::layout::{self, PanelArt, PAGE_ASPECT};
use mv_create::video::{self, VideoVia};
use mv_create::{existing, prompts, Project};
use ui_studio::images::{self, ImageModel, Keys, Reference, Shape, Via, CATALOG};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Task {
    Sheet(String),
    Expressions(String),
    Location(String),
    Page { chapter: u32, page: u32 },
    Clip { chapter: u32, page: u32, panel: usize },
    Stitch { chapter: u32 },
}

pub(crate) enum Outcome {
    Sheet { id: String, rel: String },
    Expressions { id: String, rel: String },
    Location { id: String, rel: String },
    Page { chapter: u32, page: u32, rel: String, panels: Vec<Option<String>>, model: String },
    Clip { chapter: u32, page: u32, panel: usize, rel: String },
    Stitch { chapter: u32, rel: String },
}

pub(crate) struct Job {
    pub label: String,
    /// Credits it costs on Magnific (0: free or billed by another service).
    pub credits: f64,
    pub model: Option<&'static ImageModel>,
    pub fut: BoxFuture<'static, Result<Outcome>>,
}

/// Aspect ratios a model can be asked for.
pub(crate) fn aspects(m: &ImageModel) -> &'static [&'static str] {
    match m.shape {
        Shape::Banana => &["2:3", "3:4", "4:5", "1:1", "5:4", "4:3", "3:2", "16:9", "9:16", "21:9"],
        Shape::Gemini => &["1:1"],
        _ => &["16:9", "9:16", "1:1", "4:3"],
    }
}

/// It can paint a whole portrait page.
pub(crate) fn paints_pages(m: &ImageModel) -> bool {
    aspects(m).contains(&PAGE_ASPECT)
}

/// The project's image model, else Nano Banana 2.1 with a Magnific key, else any with a key.
pub(crate) fn image_model(p: &Project, keys: &Keys) -> &'static ImageModel {
    CATALOG.iter().find(|m| m.id == p.settings.image_model).or_else(|| CATALOG.iter().find(|m| keys.has(m))).unwrap_or(&CATALOG[0])
}

pub(crate) fn resolution(p: &Project, m: &ImageModel) -> &'static str {
    m.resolutions.iter().copied().find(|r| *r == p.settings.resolution).or_else(|| m.resolutions.iter().copied().find(|r| *r == "2K")).or_else(|| m.resolutions.first().copied()).unwrap_or("")
}

/// Whole pages, or panel by panel, for this model.
pub(crate) fn whole_pages(p: &Project, m: &ImageModel) -> bool {
    // Live action is filmed scene by scene: each panel is its own film still.
    if p.settings.live_action {
        return false;
    }
    match p.settings.mode {
        mv_create::RenderMode::Page => paints_pages(m),
        mv_create::RenderMode::Panels => false,
        mv_create::RenderMode::Auto => paints_pages(m),
    }
}

/// How many images a page costs with this model.
pub(crate) fn images_per_page(p: &Project, m: &ImageModel, panels: usize) -> usize {
    if whole_pages(p, m) {
        1
    } else {
        panels.max(1)
    }
}

/// References for a page or panel, most important first, at most `cap`: each character's model
/// sheet, then their expression sheets, the style pages, the location and the previous page.
pub(crate) fn refs_for(p: &Project, names: &[String], location: &str, prev: Option<PathBuf>, cap: usize) -> Vec<Reference> {
    let mut out: Vec<Reference> = vec![];
    let push = |out: &mut Vec<Reference>, file: PathBuf, what: String| {
        if out.len() < cap {
            let caption = format!("Image {}: {what}", out.len() + 1);
            out.push(Reference { file, caption });
        }
    };
    let cast: Vec<_> = names.iter().filter_map(|n| p.character(n)).collect();
    for c in &cast {
        if let Some(f) = existing(p, c.sheet.as_ref()) {
            push(&mut out, f, format!("{} model sheet (face, hair, outfit must match)", c.name));
        } else if let Some(f) = c.user_refs.first().map(|r| p.file(r)).filter(|f| f.is_file()) {
            push(&mut out, f, format!("{} as designed by the author", c.name));
        }
    }
    for c in &cast {
        if let Some(f) = existing(p, c.expressions.as_ref()) {
            push(&mut out, f, format!("{} expression sheet", c.name));
        }
    }
    for r in p.style_refs.iter().take(2) {
        let f = p.file(r);
        if f.is_file() {
            push(&mut out, f, "art style reference: match its linework, inking and screentone, NOT its characters".into());
        }
    }
    if let Some(l) = p.location(location) {
        if let Some(f) = existing(p, l.sheet.as_ref()) {
            push(&mut out, f, format!("the location {}", l.name));
        }
    }
    if let Some(f) = prev.filter(|f| f.is_file()) {
        push(&mut out, f, "the previous page, for continuity of outfits, lighting and places".into());
    }
    out
}

fn captions(refs: &[Reference]) -> Vec<String> {
    refs.iter().map(|r| r.caption.clone()).collect()
}

fn key(m: &ImageModel) -> Result<String> {
    images::key_for(m).ok_or_else(|| match m.via {
        Via::Magnific => anyhow!("Connect Magnific first: add the API key in Settings › Magnific."),
        Via::Service(_) => anyhow!("{} needs a {} key: add it in Settings › AI services.", m.name, m.service_label()),
    })
}

fn set(status: &Arc<Mutex<String>>, s: impl Into<String>) {
    *status.lock().unwrap_or_else(|e| e.into_inner()) = s.into();
}

/// Builds the job for `task` from the project as it is now.
pub(crate) fn build(p: &Project, task: &Task, keys: &Keys, status: Arc<Mutex<String>>) -> Result<Job> {
    let m = image_model(p, keys);
    let res = resolution(p, m);
    let one = m.cost_at(res);
    let image_job = |label: String, prompt: String, refs: Vec<Reference>, aspect: &'static str, out: PathBuf, done: Box<dyn FnOnce(String) -> Outcome + Send>, rel_of: Box<dyn Fn(&std::path::Path) -> String + Send>| -> Result<Job> {
        let k = key(m)?;
        let st = status.clone();
        let fut = async move {
            images::generate(m, &k, &prompt, &refs, aspect, res, &out, move |s| set(&st, s)).await?;
            Ok(done(rel_of(&out)))
        };
        Ok(Job { label, credits: one, model: Some(m), fut: fut.boxed() })
    };
    let dir = p.dir();
    let rel = move |f: &std::path::Path| f.strip_prefix(&dir).unwrap_or(f).to_string_lossy().replace('\\', "/");
    match task {
        Task::Sheet(id) | Task::Expressions(id) => {
            let c = p.cast.iter().find(|c| &c.id == id).ok_or_else(|| anyhow!("that character is gone"))?.clone();
            let expr = matches!(task, Task::Expressions(_));
            let mut refs: Vec<Reference> = vec![];
            if expr {
                if let Some(f) = existing(p, c.sheet.as_ref()) {
                    refs.push(Reference { file: f, caption: format!("Image 1: {} model sheet", c.name) });
                }
            }
            for r in c.user_refs.iter().take(3) {
                refs.push(Reference { file: p.file(r), caption: format!("Image {}: {} as designed by the author — keep this design", refs.len() + 1, c.name) });
            }
            for r in p.style_refs.iter().take(2) {
                refs.push(Reference { file: p.file(r), caption: format!("Image {}: art style reference (style only, not its characters)", refs.len() + 1) });
            }
            refs.retain(|r| r.file.is_file());
            refs.truncate(m.refs);
            let prompt = if expr { prompts::expressions(p, &c) } else { prompts::sheet(p, &c, !refs.is_empty()) };
            let aspect = layout::nearest(if expr { 1. } else { 16. / 9. }, aspects(m));
            let out = p.new_image("sheets", &format!("{}-{}", c.id, if expr { "expressions" } else { "sheet" }));
            let id = c.id.clone();
            let label = format!("{} · {}", c.name, if expr { "expression sheet" } else { "model sheet" });
            image_job(label, prompt, refs, aspect, out, Box::new(move |rel| if expr { Outcome::Expressions { id, rel } } else { Outcome::Sheet { id, rel } }), Box::new(rel))
        }
        Task::Location(id) => {
            let l = p.locations.iter().find(|l| &l.id == id).ok_or_else(|| anyhow!("that location is gone"))?.clone();
            let refs: Vec<Reference> = p.style_refs.iter().take(2).map(|r| Reference { file: p.file(r), caption: "art style reference (style only)".into() }).filter(|r| r.file.is_file()).take(m.refs).collect();
            let out = p.new_image("sheets", &format!("loc-{}", l.id));
            let id = l.id.clone();
            image_job(format!("{} · location", l.name), prompts::location(p, &l), refs, layout::nearest(16. / 9., aspects(m)), out, Box::new(move |rel| Outcome::Location { id, rel }), Box::new(rel))
        }
        Task::Page { chapter, page } => page_job(p, m, res, *chapter, *page, status),
        Task::Clip { chapter, page, panel } => clip_job(p, *chapter, *page, *panel, status),
        Task::Stitch { chapter } => {
            let ch = p.chapter(*chapter).ok_or_else(|| anyhow!("no chapter {chapter}"))?;
            let clips: Vec<PathBuf> = ch.pages.iter().flat_map(|pg| &pg.panels).filter_map(|x| existing(p, x.clip.as_ref())).collect();
            if clips.is_empty() {
                bail!("chapter {chapter} has no clips yet");
            }
            let aspect = p.settings.video_aspect.clone();
            let work = p.dir().join("clips").join(format!("ch{chapter}-work"));
            let out = p.dir().join("video").join(format!("{}-chapter-{chapter}-{}.mp4", mv_create::slug(&p.title), mv_create::now()));
            let chapter = *chapter;
            let st = status.clone();
            let fut = async move {
                set(&st, format!("Stitching {} clips", clips.len()));
                let o = out.clone();
                tokio::task::spawn_blocking(move || {
                    let r = video::stitch(&clips, &aspect, None, &work, &o);
                    let _ = std::fs::remove_dir_all(&work);
                    r
                })
                .await??;
                Ok(Outcome::Stitch { chapter, rel: rel(&out) })
            };
            Ok(Job { label: format!("Chapter {chapter} video"), credits: 0., model: None, fut: fut.boxed() })
        }
    }
}

fn page_job(p: &Project, m: &'static ImageModel, res: &'static str, chapter: u32, page: u32, status: Arc<Mutex<String>>) -> Result<Job> {
    let ch = p.chapter(chapter).ok_or_else(|| anyhow!("no chapter {chapter}"))?;
    let pg = ch.pages.iter().find(|x| x.number == page).ok_or_else(|| anyhow!("no page {page} in chapter {chapter}"))?.clone();
    let k = key(m)?;
    let prev = ch.pages.iter().filter(|x| x.number < page).max_by_key(|x| x.number).and_then(|x| existing(p, x.image.as_ref()));
    let label = format!("Chapter {chapter} · page {page}");
    let dir = p.dir();
    let rel = move |f: &std::path::Path| f.strip_prefix(&dir).unwrap_or(f).to_string_lossy().replace('\\', "/");
    let out = p.new_image("pages", &format!("ch{chapter:02}-p{page:03}"));
    let rtl = p.settings.rtl;
    let st = status.clone();
    if whole_pages(p, m) {
        // Everyone on the page, most panels first.
        let mut names: Vec<(String, usize)> = vec![];
        for c in pg.panels.iter().flat_map(|x| &x.characters) {
            match names.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(&c.name)) {
                Some(e) => e.1 += 1,
                None => names.push((c.name.clone(), 1)),
            }
        }
        names.sort_by_key(|(_, k)| std::cmp::Reverse(*k));
        let names: Vec<String> = names.into_iter().map(|(n, _)| n).collect();
        let location = pg.panels.first().map(|x| x.location.clone()).unwrap_or_default();
        let refs = refs_for(p, &names, &location, prev, m.refs);
        let prompt = prompts::page(p, chapter, &pg, &captions(&refs));
        let raw = out.with_extension("raw.png");
        let n = pg.panels.len();
        let model = m.name.to_string();
        let fut = async move {
            images::generate(m, &k, &prompt, &refs, PAGE_ASPECT, res, &raw, move |s| set(&st, s)).await?;
            let (r, o) = (raw.clone(), out.clone());
            tokio::task::spawn_blocking(move || layout::fit_page(&r, &o)).await??;
            let _ = std::fs::remove_file(&raw);
            Ok(Outcome::Page { chapter, page, rel: rel(&out), panels: vec![None; n], model })
        };
        return Ok(Job { label, credits: m.cost_at(res), model: Some(m), fut: fut.boxed() });
    }
    // Panel by panel: each at the model's nearest shape to its place on the page, then laid out
    // and lettered here.
    let rects = layout::rects(&pg.layout, pg.panels.len(), rtl);
    let mut work = vec![];
    for (i, panel) in pg.panels.iter().enumerate() {
        let (_, _, w, h) = layout::place(rects[i], layout::PAGE_W, layout::PAGE_H);
        let aspect = layout::nearest(w as f32 / h as f32, aspects(m));
        let names: Vec<String> = panel.characters.iter().map(|c| c.name.clone()).collect();
        let refs = refs_for(p, &names, &panel.location, prev.clone(), m.refs);
        let prompt = prompts::panel(p, panel, aspect, &captions(&refs));
        let file = p.new_image("panels", &format!("ch{chapter:02}-p{page:03}-{}", i + 1));
        // Film stills carry no balloons: their lines are spoken in the clip.
        work.push((prompt, refs, aspect, file, if p.settings.live_action { vec![] } else { panel.lines.clone() }));
    }
    let layout_id = pg.layout.clone();
    let count = work.len();
    let model = m.name.to_string();
    let fut = async move {
        let mut arts = vec![];
        let mut panels = vec![];
        for (i, (prompt, refs, aspect, file, lines)) in work.into_iter().enumerate() {
            let st2 = st.clone();
            images::generate(m, &k, &prompt, &refs, aspect, res, &file, move |s| set(&st2, format!("Panel {} of {count} · {s}", i + 1))).await.with_context(|| format!("panel {}", i + 1))?;
            panels.push(Some(rel(&file)));
            arts.push(PanelArt { image: Some(file), lines });
        }
        set(&st, "Lettering");
        let o = out.clone();
        tokio::task::spawn_blocking(move || layout::compose(&layout_id, &arts, rtl, &o)).await??;
        Ok(Outcome::Page { chapter, page, rel: rel(&out), panels, model })
    };
    Ok(Job { label, credits: m.cost_at(res) * count as f64, model: Some(m), fut: fut.boxed() })
}

fn clip_job(p: &Project, chapter: u32, page: u32, i: usize, status: Arc<Mutex<String>>) -> Result<Job> {
    let ch = p.chapter(chapter).ok_or_else(|| anyhow!("no chapter {chapter}"))?;
    let pg = ch.pages.iter().find(|x| x.number == page).ok_or_else(|| anyhow!("no page {page}"))?;
    let panel = pg.panels.get(i).ok_or_else(|| anyhow!("no panel {}", i + 1))?.clone();
    let page_img = existing(p, pg.image.as_ref()).ok_or_else(|| anyhow!("paint page {page} first"))?;
    let panel_img = existing(p, panel.image.as_ref());
    let crop = p.dir().join("panels").join(format!("ch{chapter:02}-p{page:03}-{}-crop.png", i + 1));
    let (layout_id, n, rtl) = (pg.layout.clone(), pg.panels.len(), p.settings.rtl);
    let vm = video::video_model(&p.settings.video_model);
    let seconds = vm.fit_seconds(p.settings.clip_seconds);
    let aspect = p.settings.video_aspect.clone();
    let prompt = prompts::motion(p, &panel);
    let out = p.dir().join("clips").join(format!("ch{chapter:02}-p{page:03}-{}-{}.mp4", i + 1, mv_create::now()));
    let dir = p.dir();
    let rel = move |f: &std::path::Path| f.strip_prefix(&dir).unwrap_or(f).to_string_lossy().replace('\\', "/");
    let key = match vm.via {
        VideoVia::Magnific => Some(images::key().ok_or_else(|| anyhow!("Connect Magnific first: add the API key in Settings › Magnific."))?),
        VideoVia::Service(id) => Some(mv_ai::service_key(mv_core::paths::config(), id).ok_or_else(|| anyhow!("{} needs a {} key: add it in Settings › AI services.", vm.name, mv_ai::service_def(id).map(|d| d.label).unwrap_or(id)))?),
        _ => None,
    };
    let comfy = (p.settings.comfy_url.clone(), PathBuf::from(p.settings.comfy_workflow.clone()));
    // LTX with voice: the scene's lines are spoken first, and the clip is as long as they are.
    let spoken: String = prompts::spoken_lines(&panel).into_iter().map(|(_, t)| t).collect::<Vec<_>>().join(" ");
    let voice_pick = p.settings.voice.clone();
    let voice_dir = p.dir().join("voice").join(format!("ch{chapter:02}-p{page:03}-{}-{}", i + 1, mv_create::now()));
    if vm.via == VideoVia::ComfyUi && !comfy.1.is_file() {
        bail!("Pick your ComfyUI workflow (API format JSON) on the Video tab first");
    }
    let st = status.clone();
    let fut = async move {
        let src = match panel_img {
            Some(f) => f,
            None => {
                let c = crop.clone();
                tokio::task::spawn_blocking(move || layout::crop_panel(&page_img, &layout_id, n, i, rtl, &c)).await??;
                crop
            }
        };
        match vm.via {
            VideoVia::Local => {
                set(&st, "Moving the camera");
                let (s, o) = (src.clone(), out.clone());
                tokio::task::spawn_blocking(move || video::local_motion(&s, seconds, &aspect, i + page as usize, &o)).await??;
            }
            VideoVia::ComfyUi => video::comfy_clip(&comfy.0, &comfy.1, &src, &prompt, seconds, &out, |s| set(&st, s)).await?,
            VideoVia::ComfyLtx => {
                let (audio, secs) = scene_voice(spoken, voice_pick, voice_dir, seconds, &st).await?;
                video::ltx_clip(&comfy.0, &src, &audio, &prompt, secs, &aspect, &out, |s| set(&st, format!("LTX 2.3 · {s}"))).await?;
            }
            _ => video::api_clip(vm, key.as_deref().unwrap_or(""), &src, &prompt, seconds, &aspect, &out, |s| set(&st, s)).await?,
        }
        Ok(Outcome::Clip { chapter, page, panel: i, rel: rel(&out) })
    };
    Ok(Job { label: format!("Chapter {chapter} · page {page} · panel {}", i + 1), credits: 0., model: None, fut: fut.boxed() })
}

/// The sound of a scene for LTX: its lines spoken with the project's voice (Studio's voice when
/// none is picked), or silence of `seconds` when nobody speaks. Returns the file and how long the
/// clip should be (the voice and a moment after it, at most [`LONGEST_SCENE`]).
async fn scene_voice(spoken: String, voice: String, dir: PathBuf, seconds: u32, st: &Arc<Mutex<String>>) -> Result<(PathBuf, f64)> {
    if spoken.trim().is_empty() {
        set(st, "Nobody speaks: a silent track");
        let out = dir.join("silence.wav");
        let o = out.clone();
        tokio::task::spawn_blocking(move || video::silence(seconds as f64, &o)).await??;
        return Ok((out, seconds as f64));
    }
    set(st, "Voicing the scene");
    // Studio's Python as Studio last checked it, else found the way Studio finds it.
    let (python, layout, voice) = tokio::task::spawn_blocking(move || -> Result<_> {
        let env = ui_studio::setup::latest().map(|c| c.env).and_then(|e| Some((e.python.ok()?, e.scripts.ok()?)));
        let (python, layout) = match env {
            Some(x) => x,
            None => (
                mv_studio::python::voice_python().map_err(|e| anyhow!("The scene is voiced by Studio's Python, which is not set up yet (open Studio and finish Setup): {e}"))?,
                mv_studio::PythonLayout::resolve(None).map_err(|e| anyhow!("Studio's scripts were not found: {e}"))?,
            ),
        };
        let voice = if voice.trim().is_empty() { mv_studio::voices::default_voice() } else { voice };
        Ok((python, layout, voice))
    })
    .await??;
    // The picked voice, then the voices that stand in for it when its engine is not installed
    // (as Studio's narration does): a scene is never left silent for want of one engine.
    let mut tries = vec![voice.clone()];
    tries.extend(mv_studio::tts::fallback_candidates(&voice).into_iter().map(str::to_string));
    let mut sample = None;
    let mut why = String::new();
    for v in &tries {
        let mut req = mv_studio::tts::LabRequest::new(v, "dialogue");
        req.text = Some(spoken.clone());
        req.intensities = vec![5];
        match mv_studio::tts::voice_lab(&python, &layout, &dir, &std::env::temp_dir(), &req).await {
            Ok(res) => {
                if let Some(s) = res.samples.into_iter().find(|s| s.path.is_file()) {
                    sample = Some(s);
                    break;
                }
            }
            // The first reason is the one about the voice the user picked.
            Err(e) if why.is_empty() => why = e.to_string(),
            Err(_) => {}
        }
    }
    let sample = sample.ok_or_else(|| anyhow!("The scene could not be voiced: {}", if why.is_empty() { "the voice made no sound" } else { &why }))?;
    let file = sample.path.clone();
    let heard = if sample.duration > 0. { sample.duration } else { tokio::task::spawn_blocking(move || video::media_seconds(&file)).await?.unwrap_or(seconds as f64) };
    Ok((sample.path, (heard + 0.6).clamp(2., LONGEST_SCENE)))
}

/// The longest clip LTX is asked for (a 10 GB card manages about this at 720p); a longer voice is
/// cut there, so long lines belong in two scenes.
const LONGEST_SCENE: f64 = 10.;

/// Stores a finished job's result in the project.
pub(crate) fn apply(p: &mut Project, o: Outcome) {
    match o {
        Outcome::Sheet { id, rel } => {
            if let Some(c) = p.cast.iter_mut().find(|c| c.id == id) {
                c.sheet = Some(rel);
            }
        }
        Outcome::Expressions { id, rel } => {
            if let Some(c) = p.cast.iter_mut().find(|c| c.id == id) {
                c.expressions = Some(rel);
            }
        }
        Outcome::Location { id, rel } => {
            if let Some(l) = p.locations.iter_mut().find(|l| l.id == id) {
                l.sheet = Some(rel);
            }
        }
        Outcome::Page { chapter, page, rel, panels, model } => {
            if let Some(pg) = p.chapter_mut(chapter).pages.iter_mut().find(|x| x.number == page) {
                pg.image = Some(rel);
                pg.model = model;
                for (panel, img) in pg.panels.iter_mut().zip(panels) {
                    panel.image = img;
                    // A repainted page needs new clips.
                    panel.clip = None;
                }
            }
        }
        Outcome::Clip { chapter, page, panel, rel } => {
            if let Some(x) = p.chapter_mut(chapter).pages.iter_mut().find(|x| x.number == page).and_then(|pg| pg.panels.get_mut(panel)) {
                x.clip = Some(rel);
            }
        }
        Outcome::Stitch { chapter, rel } => p.chapter_mut(chapter).video = Some(rel),
    }
}

/// The tasks a card starts: missing sheets first, then the pages or clips.
pub(crate) fn plan(p: &Project, proposal: &mv_create::Proposal) -> Vec<Task> {
    let mut out = vec![];
    let sheets_for = |out: &mut Vec<Task>, names: &[String]| {
        for n in names {
            if let Some(c) = p.character(n) {
                if c.sheet.is_none() && !out.contains(&Task::Sheet(c.id.clone())) {
                    out.push(Task::Sheet(c.id.clone()));
                }
                if c.expressions.is_none() && !out.contains(&Task::Expressions(c.id.clone())) {
                    out.push(Task::Expressions(c.id.clone()));
                }
            }
        }
    };
    match proposal {
        mv_create::Proposal::Sheets { names, locations } => {
            let names: Vec<String> = if names.is_empty() { p.cast.iter().filter(|c| c.intro_chapter <= 1).map(|c| c.name.clone()).collect() } else { names.clone() };
            sheets_for(&mut out, &names);
            for l in locations {
                if let Some(l) = p.location(l).filter(|l| l.sheet.is_none()) {
                    out.push(Task::Location(l.id.clone()));
                }
            }
        }
        mv_create::Proposal::Pages { chapter, pages } => {
            if let Some(ch) = p.chapter(*chapter) {
                sheets_for(&mut out, &ch.cast_names());
                for pg in &ch.pages {
                    if (pages.is_empty() && pg.image.is_none()) || pages.contains(&pg.number) {
                        out.push(Task::Page { chapter: *chapter, page: pg.number });
                    }
                }
            }
        }
        mv_create::Proposal::Video { chapter } => {
            if let Some(ch) = p.chapter(*chapter) {
                for pg in ch.pages.iter().filter(|pg| pg.image.is_some()) {
                    for (i, x) in pg.panels.iter().enumerate() {
                        if x.clip.is_none() {
                            out.push(Task::Clip { chapter: *chapter, page: pg.number, panel: i });
                        }
                    }
                }
                out.push(Task::Stitch { chapter: *chapter });
            }
        }
    }
    out
}

/// What a set of tasks costs in images and Magnific credits with the project's model.
pub(crate) fn estimate(p: &Project, keys: &Keys, tasks: &[Task]) -> (usize, f64) {
    let m = image_model(p, keys);
    let res = resolution(p, m);
    let mut images = 0;
    for t in tasks {
        images += match t {
            Task::Sheet(_) | Task::Expressions(_) | Task::Location(_) => 1,
            Task::Page { chapter, page } => p.chapter(*chapter).and_then(|c| c.pages.iter().find(|x| x.number == *page)).map(|pg| images_per_page(p, m, pg.panels.len())).unwrap_or(1),
            _ => 0,
        };
    }
    (images, images as f64 * m.cost_at(res))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mv_create::{CastMember, Chapter, Page, Panel, PanelChar, Proposal};

    fn project() -> Project {
        let mut p = Project::default();
        p.cast.push(CastMember { name: "Kai".into(), intro_chapter: 1, ..Default::default() });
        p.cast.push(CastMember { name: "Rin".into(), intro_chapter: 1, sheet: Some("s.png".into()), expressions: Some("e.png".into()), ..Default::default() });
        let panel = |n: &str| Panel { characters: vec![PanelChar { name: n.into(), ..Default::default() }], ..Default::default() };
        p.chapters.push(Chapter { number: 1, pages: vec![Page { number: 1, panels: vec![panel("kai"), panel("Rin")], ..Default::default() }, Page { number: 2, panels: vec![panel("Rin")], image: Some("x.png".into()), ..Default::default() }], ..Default::default() });
        p.normalize();
        p
    }

    #[test]
    fn pages_paint_missing_sheets_first() {
        let p = project();
        let t = plan(&p, &Proposal::Pages { chapter: 1, pages: vec![] });
        assert_eq!(t, vec![Task::Sheet("kai".into()), Task::Expressions("kai".into()), Task::Page { chapter: 1, page: 1 }]);
        // Asking for a painted page by number repaints it.
        let t = plan(&p, &Proposal::Pages { chapter: 1, pages: vec![2] });
        assert_eq!(t.last(), Some(&Task::Page { chapter: 1, page: 2 }));
        let v = plan(&p, &Proposal::Video { chapter: 1 });
        assert_eq!(v, vec![Task::Clip { chapter: 1, page: 2, panel: 0 }, Task::Stitch { chapter: 1 }]);
    }

    /// The real thing, end to end: voices a line with Studio's voice, then has LTX 2.3 on the
    /// local ComfyUI film `MV_LTX_IMAGE` with it. Needs ComfyUI open and takes minutes on the GPU,
    /// so it only runs when asked: `cargo test -p ui-creative ltx_films -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn ltx_films_a_voiced_scene() {
        // The voice scripts find the engines through this, as the app sets it at start.
        std::env::set_var("MANGAVOX_DATA", mv_core::paths::root());
        let image = PathBuf::from(std::env::var("MV_LTX_IMAGE").expect("set MV_LTX_IMAGE to a picture"));
        let dir = std::env::temp_dir().join(format!("mangavox-ltx-test-{}", mv_create::now()));
        let out = dir.join("scene.mp4");
        let st = Arc::new(Mutex::new(String::new()));
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (o, d, s) = (out.clone(), dir.clone(), st.clone());
        rt.block_on(async move {
            let (audio, secs) = scene_voice("Hello. This is a test of the voice.".into(), String::new(), d.join("voice"), 3, &s).await.unwrap();
            println!("voice: {} ({secs:.1} s clip)", audio.display());
            video::ltx_clip("http://127.0.0.1:8188", &image, &audio, "A live-action film scene. The character looks at the camera and says, lips in sync with the voice: \"Hello. This is a test of the voice.\"", secs, "16:9", &o, |x| println!("status: {x}")).await.unwrap();
        });
        assert!(std::fs::metadata(&out).unwrap().len() > 10_000);
        assert!(video::has_audio(&out), "the clip kept its voice");
        println!("clip: {} ({:?} s)", out.display(), video::media_seconds(&out));
    }

    #[test]
    fn whole_pages_only_on_portrait_models() {
        let p = Project::default();
        let banana = CATALOG.iter().find(|m| m.id == "nano-banana-2-1").unwrap();
        let flux = CATALOG.iter().find(|m| m.id == "flux-2-pro").unwrap();
        assert!(whole_pages(&p, banana) && !whole_pages(&p, flux));
        assert_eq!(images_per_page(&p, flux, 5), 5);
        let mut panels = Project::default();
        panels.settings.mode = mv_create::RenderMode::Panels;
        assert!(!whole_pages(&panels, banana));
    }
}
