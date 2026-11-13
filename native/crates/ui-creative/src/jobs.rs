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
    let mut spoken = prompts::delivery(&panel);
    // Each speaker under their cast name ("Kai" is "Kai Tanaka"), so they get their own voice.
    for d in &mut spoken {
        if let Some(c) = p.character(&d.speaker) {
            d.speaker = c.name.clone();
        }
    }
    let cast: Vec<Speaker> = p.cast.iter().map(Speaker::of).collect();
    let cast_dir = p.dir().join("voice");
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
                let (audio, secs) = scene_voice(spoken, cast, cast_dir, voice_pick, voice_dir, seconds, &st).await?;
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
async fn scene_voice(spoken: Vec<prompts::Delivery>, cast: Vec<Speaker>, cast_dir: PathBuf, voice: String, dir: PathBuf, seconds: u32, st: &Arc<Mutex<String>>) -> Result<(PathBuf, f64)> {
    if spoken.is_empty() {
        set(st, "Nobody speaks: a silent track");
        let out = dir.join("silence.wav");
        let o = out.clone();
        tokio::task::spawn_blocking(move || video::silence(seconds as f64, &o)).await??;
        return Ok((out, seconds as f64));
    }
    // Qwen3-TTS when its engine is installed: each line by a voice described in words (who the
    // character is and how they say it). Without it, or should it fail, Studio's voice speaks.
    match qwen_voice(&spoken, &cast, &dir, st).await {
        Ok(Some(made)) => return Ok(made),
        Ok(None) => {}
        Err(e) => eprintln!("[creative] Qwen3 voice failed, Studio's voice speaks instead: {e:#}"),
    }
    set(st, "Voicing the scene");
    // Studio's Python as Studio last checked it, else found the way Studio finds it.
    let names: Vec<String> = spoken.iter().filter(|d| d.line_kind != "narrator").map(|d| d.speaker.clone()).collect();
    let (python, layout, voice, voices) = tokio::task::spawn_blocking(move || -> Result<_> {
        let voices = cast_voices(&cast_dir, &cast, &names);
        let env = ui_studio::setup::latest().map(|c| c.env).and_then(|e| Some((e.python.ok()?, e.scripts.ok()?)));
        let (python, layout) = match env {
            Some(x) => x,
            None => (
                mv_studio::python::voice_python().map_err(|e| anyhow!("The scene is voiced by Studio's Python, which is not set up yet (open Studio and finish Setup): {e}"))?,
                mv_studio::PythonLayout::resolve(None).map_err(|e| anyhow!("Studio's scripts were not found: {e}"))?,
            ),
        };
        let voice = if voice.trim().is_empty() { mv_studio::voices::default_voice() } else { voice };
        Ok((python, layout, voice, voices))
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
        // Every line with its own feeling (a shout, a sob, a whisper), in one pass of the voice.
        req.units = spoken
            .iter()
            .map(|d| {
                let mut u = serde_json::json!({ "text": d.text, "kind": d.kind, "intensity": d.intensity, "line_kind": d.line_kind, "mood": d.mood, "speaker": d.speaker });
                // A character's line is said in that character's own voice; the narrator keeps the main one.
                if let Some(v) = voices.iter().find(|v| d.line_kind != "narrator" && v.name.eq_ignore_ascii_case(&d.speaker)) {
                    u["voice_ref"] = serde_json::json!(v.clip);
                    u["voice_make_with"] = serde_json::json!(v.base);
                    u["voice_pitch"] = serde_json::json!(v.pitch);
                }
                u
            })
            .collect();
        req.text = Some(spoken.iter().map(|d| d.text.as_str()).collect::<Vec<_>>().join(" "));
        req.intensities = vec![spoken.iter().map(|d| d.intensity).max().unwrap_or(5)];
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

// ------------------------------------------------------------------ Qwen3-TTS scene voice

/// The Qwen3-TTS VoiceDesign engine in the data folder (`engines/qwen3tts`: its own Python and
/// the model), when it is installed.
fn qwen_engine() -> Option<(PathBuf, PathBuf)> {
    let dir = mv_core::paths::root().join("engines").join("qwen3tts");
    let python = if cfg!(windows) { dir.join(".venv").join("Scripts").join("python.exe") } else { dir.join(".venv").join("bin").join("python3") };
    let model = dir.join("model-voicedesign");
    (python.is_file() && model.join("config.json").is_file()).then_some((python, model))
}

/// A number of a character's own, from their name: the same voice is asked for in every scene.
fn voice_seed(name: &str) -> u32 {
    name.trim().to_lowercase().bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619)) & 0x7fff_ffff
}

/// Timbres that set one voice apart from the next of the same age and gender.
const TIMBRES: [&str; 8] = ["deep and slightly rough", "clear and bright", "soft and breathy", "warm and low", "sharp and crisp", "husky", "smooth and even", "light and quick"];

/// Who is speaking, in words the voice model casts a voice from: their gender and age (a child
/// sounds like a child, an old man like one), a timbre of their own, and their nature.
fn voice_of(s: &Speaker) -> String {
    let female = if s.gender.is_empty() { mv_recap::series::guess_gender(&s.about) == mv_recap::series::Gender::Female } else { s.gender.starts_with('f') };
    let timbre = TIMBRES[(voice_seed(&s.name) as usize / 7) % TIMBRES.len()];
    let who = match (s.years, female) {
        (Some(y), true) if y <= 12 => format!("A {y}-year-old little girl with a small, high, soft child's voice"),
        (Some(y), false) if y <= 12 => format!("A {y}-year-old little boy with a small, high, bright child's voice"),
        (Some(y), true) if y <= 17 => format!("A teenage girl of {y} with a young, {timbre} voice"),
        (Some(y), false) if y <= 17 => format!("A teenage boy of {y} with a young, {timbre} voice that has only just broken"),
        (Some(y), true) if y >= 60 => format!("An elderly woman of {y} with an aged, thin, {timbre} voice"),
        (Some(y), false) if y >= 60 => format!("An elderly man of {y} with an aged, gravelly, {timbre} voice"),
        (Some(y), true) => format!("A woman of {y} with a {timbre} adult female voice"),
        (Some(y), false) => format!("A man of {y} with a {timbre} adult male voice"),
        (None, true) => format!("A young woman with a {timbre} female voice"),
        (None, false) => format!("A young man with a {timbre} male voice"),
    };
    let nature: String = s.personality.split(['.', ';']).next().unwrap_or("").trim().chars().take(110).collect();
    if nature.is_empty() {
        format!("{who}.")
    } else {
        format!("{who}. Character: {nature}.")
    }
}

/// How a line is said, in words: the feeling and how strong it is.
fn feeling(d: &prompts::Delivery) -> &'static str {
    let strong = d.intensity >= 8;
    match d.kind {
        "fight" if strong => "Furious and shouting, loud, sharp and forceful.",
        "fight" => "Angry and tense, voice hard and clipped.",
        "horror" if strong => "Terrified, voice trembling and breathless, close to panic.",
        "horror" => "Afraid and uneasy, voice unsteady.",
        "emotional" if strong => "Crying, voice shaking and breaking, sniffling between words.",
        "emotional" => "Sad and quiet, close to tears.",
        "reveal" if strong => "Shocked, gasping in disbelief.",
        "reveal" => "Surprised and taken aback.",
        "action" => "Determined, firm and intense.",
        "comedy" if strong => "Laughing, bright and full of joy.",
        "comedy" => "Cheerful and light, smiling while speaking.",
        "romance" => "Tender and warm, a little shy.",
        "calm" => "Very soft and quiet, almost a whisper.",
        _ => "Speaking naturally, as in an ordinary conversation.",
    }
}

/// The whole direction for one line: the speaker's voice, then how this line is said.
fn instruct(d: &prompts::Delivery, cast: &[Speaker]) -> String {
    if d.line_kind == "narrator" {
        let how = match d.kind {
            "emotional" | "horror" => "Quiet, slow and grave.",
            "fight" | "action" | "reveal" => "Tense and gripping, a little faster.",
            _ => "Even and unhurried.",
        };
        return format!("A calm, warm adult narrator with a low, steady documentary voice. {how}");
    }
    let voice = match cast.iter().find(|c| c.name.eq_ignore_ascii_case(&d.speaker)) {
        Some(c) => voice_of(c),
        // Someone the cast does not list: cast from their name alone.
        None => voice_of(&Speaker { name: d.speaker.clone(), gender: String::new(), years: None, about: d.speaker.clone(), personality: String::new() }),
    };
    let mut s = format!("{voice} {}", feeling(d));
    if d.line_kind == "inner" {
        s.push_str(" Said inwardly, as a quiet thought to oneself.");
    }
    if !d.mood.is_empty() {
        s.push_str(&format!(" Expression: {}.", d.mood.chars().take(80).collect::<String>()));
    }
    s
}

/// The scene spoken by Qwen3-TTS: every line with its own speaker's voice and feeling, joined
/// into one WAV. `None` when the engine is not installed.
async fn qwen_voice(spoken: &[prompts::Delivery], cast: &[Speaker], dir: &std::path::Path, st: &Arc<Mutex<String>>) -> Result<Option<(PathBuf, f64)>> {
    let found = tokio::task::spawn_blocking(|| -> Option<(PathBuf, PathBuf, PathBuf)> {
        let (python, model) = qwen_engine()?;
        let layout = ui_studio::setup::latest().and_then(|c| c.env.scripts.ok()).or_else(|| mv_studio::PythonLayout::resolve(None).ok())?;
        Some((python, model, layout.script("qwen_scene_tts.py").ok()?))
    })
    .await?;
    let Some((python, model, script)) = found else { return Ok(None) };
    set(st, format!("Voicing the scene with Qwen3 ({} line{})", spoken.len(), if spoken.len() == 1 { "" } else { "s" }));
    let out = dir.join("scene.wav");
    let units: Vec<serde_json::Value> = spoken
        .iter()
        .map(|d| serde_json::json!({ "text": d.text, "instruct": instruct(d, cast), "seed": voice_seed(if d.line_kind == "narrator" { "narrator" } else { &d.speaker }) }))
        .collect();
    let args = serde_json::json!({ "model_dir": model, "out": out, "language": "English", "gap_ms": 300, "units": units });
    tokio::fs::create_dir_all(dir).await?;
    let args_file = dir.join("qwen-args.json");
    tokio::fs::write(&args_file, serde_json::to_vec(&args)?).await?;
    let mut cmd = tokio::process::Command::new(&python);
    cmd.arg(&script).arg(&args_file).stdin(std::process::Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let run = cmd.output().await.with_context(|| format!("could not start {}", python.display()))?;
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    let answer: serde_json::Value = stdout.lines().rev().find(|l| !l.trim().is_empty()).and_then(|l| serde_json::from_str(l).ok()).unwrap_or(serde_json::Value::Null);
    if answer.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        let why = answer.get("message").and_then(serde_json::Value::as_str).map(str::to_string).unwrap_or_else(|| String::from_utf8_lossy(&run.stderr).lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it gave no answer").to_string());
        bail!("{why}");
    }
    let heard = answer.get("duration").and_then(serde_json::Value::as_f64).unwrap_or(0.);
    if !out.is_file() || heard <= 0. {
        bail!("it made no sound");
    }
    Ok(Some((out, (heard + 0.6).clamp(2., LONGEST_SCENE))))
}

/// What a character's voice is cast from.
#[derive(Clone, Debug)]
pub(crate) struct Speaker {
    name: String,
    /// "male", "female" or empty (then guessed from `about`).
    gender: String,
    years: Option<u32>,
    about: String,
    personality: String,
}

impl Speaker {
    fn of(c: &mv_create::CastMember) -> Speaker {
        Speaker { name: c.name.clone(), gender: c.gender.trim().to_lowercase(), years: c.years(), about: format!("{} {} {} {}", c.role, c.age, c.appearance, c.personality), personality: c.personality.clone() }
    }
}

/// A character's own voice: the clip Chatterbox clones, and what it is made from when missing.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CharacterVoice {
    name: String,
    clip: PathBuf,
    base: String,
    pitch: f32,
}

/// How far a voice is raised or lowered for the character's age, in semitones.
fn age_pitch(years: Option<u32>) -> f32 {
    match years {
        Some(y) if y <= 7 => 4.5,
        Some(y) if y <= 12 => 3.5,
        Some(y) if y <= 16 => 1.5,
        Some(y) if y >= 60 => -2.0,
        _ => 0.0,
    }
}

/// Gives every speaker of a scene their own voice, kept for the whole project in
/// `<project>/voice/voice_cast.json` (the way Studio's series mode casts a manga): a man gets a
/// male voice, a woman a female one, no two characters the same voice, a child a high, light one
/// and an old character a lower one. A voice once given never changes, so a character sounds the
/// same in every scene.
fn cast_voices(dir: &std::path::Path, cast: &[Speaker], speakers: &[String]) -> Vec<CharacterVoice> {
    use mv_recap::series::VoiceCast;
    // What the casting reads of each character. Their stated gender leads; a young child is
    // given a woman's voice raised, as children are voiced in dubbing.
    let known: Vec<serde_json::Value> = cast
        .iter()
        .map(|c| {
            let child = c.years.is_some_and(|y| y <= 10);
            let lead = if c.gender.starts_with('f') || (child && !c.gender.is_empty()) {
                "she her woman female. "
            } else if c.gender.starts_with('m') {
                "he him man male. "
            } else {
                ""
            };
            // With a stated gender the description's own words must not outvote it.
            serde_json::json!({ "name": c.name, "description": if lead.is_empty() { c.about.clone() } else { lead.to_string() } })
        })
        .collect();
    let brain: mv_recap::Brain = serde_json::from_value(serde_json::json!({ "characters": known })).unwrap_or_default();
    let _ = std::fs::create_dir_all(dir);
    let mut vc = VoiceCast::load_in(dir.to_path_buf());
    let before = vc.voices.len();
    if vc.assign(speakers, &brain) > 0 {
        for v in vc.voices.iter_mut().skip(before) {
            let years = cast.iter().find(|c| c.name.eq_ignore_ascii_case(&v.name)).and_then(|c| c.years);
            v.pitch = (v.pitch + age_pitch(years)).clamp(-6., 6.);
        }
        let _ = vc.save();
    }
    speakers.iter().filter_map(|s| vc.find(s).map(|v| CharacterVoice { name: s.clone(), clip: vc.clip_path(v), base: v.base.clone(), pitch: v.pitch })).collect()
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
            let names: Vec<String> = if names.is_empty() { p.opening_cast() } else { names.clone() };
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
            let (audio, secs) = scene_voice(
                vec![
                    prompts::Delivery { speaker: "Cactus".into(), text: "You left me out here!".into(), kind: "fight", intensity: 9, line_kind: "speech", mood: "furious".into() },
                    prompts::Delivery { speaker: "Cactus".into(), text: "I waited all night for you...".into(), kind: "emotional", intensity: 8, line_kind: "speech", mood: "heartbroken".into() },
                ],
                vec![],
                d.join("cast"),
                String::new(),
                d.join("voice"),
                3,
                &s,
            ).await.unwrap();
            println!("voice: {} ({secs:.1} s clip)", audio.display());
            video::ltx_clip("http://127.0.0.1:8188", &image, &audio, "A live-action film scene. The character turns to the camera and says furious, lips in sync with the voice: \"You left me out here!\" Then says in grief: \"I waited all night for you...\"", secs, "16:9", &o, |x| println!("status: {x}")).await.unwrap();
        });
        assert!(std::fs::metadata(&out).unwrap().len() > 10_000);
        assert!(video::has_audio(&out), "the clip kept its voice");
        println!("clip: {} ({:?} s)", out.display(), video::media_seconds(&out));
    }

    /// The real voice, no video: a man shouts, a little girl answers in tears, a narrator closes.
    /// `cargo test -p ui-creative voices_a_scene -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn voices_a_scene_with_feeling_and_a_voice_each() {
        std::env::set_var("MANGAVOX_DATA", mv_core::paths::root());
        let dir = std::env::temp_dir().join(format!("mangavox-voice-test-{}", mv_create::now()));
        let who = |name: &str, gender: &str, age: &str| Speaker::of(&CastMember { name: name.into(), gender: gender.into(), age: age.into(), ..Default::default() });
        let line = |speaker: &str, text: &str, kind: &'static str, intensity: u8, line_kind: &'static str| prompts::Delivery { speaker: speaker.into(), text: text.into(), kind, intensity, line_kind, mood: String::new() };
        let st = Arc::new(Mutex::new(String::new()));
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let d = dir.clone();
        let (audio, secs) = rt
            .block_on(async move {
                scene_voice(
                    vec![line("Ren", "Where were you? I told you to stay inside!", "fight", 9, "speech"), line("Mio", "I'm sorry... I just wanted to find my mom.", "emotional", 8, "speech"), line("", "Neither of them spoke again that night.", "dialogue", 5, "narrator")],
                    vec![who("Ren", "male", "34"), who("Mio", "female", "7")],
                    d.join("cast"),
                    String::new(),
                    d.join("voice"),
                    3,
                    &st,
                )
                .await
            })
            .unwrap();
        println!("voice: {} ({secs:.1} s clip)", audio.display());
        for f in std::fs::read_dir(dir.join("cast").join("voices")).into_iter().flatten().flatten() {
            println!("character clip: {}", f.path().display());
        }
        assert!(std::fs::metadata(&audio).unwrap().len() > 10_000);
    }

    #[test]
    fn a_line_is_directed_by_who_says_it_and_how() {
        let who = |name: &str, gender: &str, age: &str, nature: &str| Speaker::of(&CastMember { name: name.into(), gender: gender.into(), age: age.into(), personality: nature.into(), ..Default::default() });
        let cast = vec![who("Ren", "male", "34", "Stern but caring. Hides his fear."), who("Mio", "female", "7", ""), who("Jiro", "male", "72", ""), who("Aya", "", "", "")];
        let line = |speaker: &str, kind: &'static str, intensity: u8, line_kind: &'static str| prompts::Delivery { speaker: speaker.into(), text: "x".into(), kind, intensity, line_kind, mood: String::new() };
        let ren = instruct(&line("Ren", "fight", 9, "speech"), &cast);
        assert!(ren.starts_with("A man of 34 with a ") && ren.contains("adult male voice. Character: Stern but caring. Furious and shouting"), "{ren}");
        let mio = instruct(&line("Mio", "emotional", 8, "speech"), &cast);
        assert!(mio.starts_with("A 7-year-old little girl with a small, high, soft child's voice. Crying"), "{mio}");
        assert!(instruct(&line("Jiro", "calm", 3, "inner"), &cast).contains("An elderly man of 72") && instruct(&line("Jiro", "calm", 3, "inner"), &cast).ends_with("as a quiet thought to oneself."));
        assert!(instruct(&line("", "dialogue", 5, "narrator"), &cast).starts_with("A calm, warm adult narrator"));
        // The same character is always asked for the same voice; two characters for different ones.
        assert_eq!(voice_of(&cast[0]), voice_of(&cast[0]));
        assert_eq!(voice_seed("Ren"), voice_seed(" ren "));
        assert_ne!(voice_seed("Ren"), voice_seed("Mio"));
    }

    #[test]
    fn every_character_gets_a_voice_of_their_own() {
        let t = std::env::temp_dir().join(format!("mangavox-cast-test-{}-{}", std::process::id(), mv_create::now()));
        let who = |name: &str, gender: &str, age: &str| Speaker::of(&CastMember { name: name.into(), gender: gender.into(), age: age.into(), ..Default::default() });
        let cast = vec![who("Kai", "male", "17"), who("Rin", "female", "16"), who("Old Jiro", "male", "72"), who("Mio", "female", "6"), who("Ren", "male", "30"), who("Timmy", "male", "5")];
        let names: Vec<String> = cast.iter().map(|c| c.name.clone()).collect();
        let v = cast_voices(&t, &cast, &names);
        assert_eq!(v.len(), 6);
        let is_f = |b: &str| b.starts_with("af_") || b.starts_with("bf_");
        assert!(!is_f(&v[0].base) && is_f(&v[1].base) && !is_f(&v[2].base) && is_f(&v[3].base) && !is_f(&v[4].base), "{v:?}");
        // A small boy is voiced by a raised woman's voice; a child is high, an old man low.
        assert!(is_f(&v[5].base) && v[5].pitch >= 4.0 && v[3].pitch >= 4.0 && v[2].pitch < v[4].pitch, "{v:?}");
        // No two characters share a voice.
        for (i, a) in v.iter().enumerate() {
            for b in &v[i + 1..] {
                assert!(a.base != b.base || (a.pitch - b.pitch).abs() >= 0.6, "{a:?} and {b:?}");
            }
        }
        // Cast once, the same ever after.
        assert_eq!(cast_voices(&t, &cast, &names), v);
        let _ = std::fs::remove_dir_all(&t);
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
