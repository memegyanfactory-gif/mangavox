//! The anime's jobs (see `mv_create::anime`): the chapter's pages cut into panels, the animatic
//! (each cut voiced and shown from its manga panel), the plates, the effects bank, each cut's
//! character layer, its animation and its compositing, and the episode. They run in the same
//! queue as everything else in Creative, one at a time, each built from the project as it is when
//! its turn comes (a cut's layer is drawn with the plate painted a moment ago as its light
//! reference). [`stage`] says what a chapter needs next, in the order a studio works.
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Context as _, Result};
use futures::FutureExt as _;
use mv_create::anime::{self, pacing, CompParts, Cut, SourcePanel};
use mv_create::layout;
use mv_create::video::{self, VideoVia};
use mv_create::{existing, prompts, Project};
use ui_studio::images::{Keys, Reference};

use crate::jobs::{self, set, Job, Outcome, Task};

/// What an anime job made, to store in the project.
pub(crate) enum Made {
    Panels { chapter: u32, panels: Vec<SourcePanel> },
    /// A cut of the animatic: its file, its voice when it was spoken now, and the length the
    /// cut needs for that voice.
    Rough { chapter: u32, cut: u32, rel: String, voice: Option<String>, seconds: f32 },
    Animatic { chapter: u32, rel: String },
    Plate { id: String, rel: String },
    Fx { id: String, rel: String },
    Prop { id: String, rel: String },
    /// `layer`: the characters on the screen; `key`: the whole frame (a cut without the cast);
    /// `bg` and `layout`: the cut's own background and the frame drawn into it (layout first).
    Layer { chapter: u32, cut: u32, layer: Option<String>, key: Option<String>, bg: Option<String>, layout: Option<String> },
    Anim { chapter: u32, cut: u32, rel: String, key: Option<String>, voice: Option<String> },
    Comp { chapter: u32, cut: u32, rel: String, voice: Option<String>, seconds: f32 },
    Episode { chapter: u32, rel: String },
}

fn cut_of(p: &Project, chapter: u32, n: u32) -> Result<Cut> {
    p.chapter(chapter).and_then(|c| c.anime.cuts.iter().find(|c| c.number == n)).cloned().ok_or_else(|| anyhow!("chapter {chapter} has no cut {n} (was the storyboard redone?)"))
}

/// The kind of scene cut `c` of `chapter` is in.
fn kind_of(p: &Project, chapter: u32, c: &Cut) -> String {
    p.chapter(chapter).map(|ch| ch.anime.kind_of(c).to_string()).unwrap_or_else(|| "dialogue".into())
}

fn rel_in(dir: &Path, f: &Path) -> String {
    f.strip_prefix(dir).unwrap_or(f).to_string_lossy().replace('\\', "/")
}

/// The video model animates (FFmpeg's pan and zoom does not: every cut is then held drawings
/// moved by the camera, which is still real anime).
pub(crate) fn animates(p: &Project) -> bool {
    video::video_model(&p.settings.video_model).via != VideoVia::Local
}

/// The cut needs an animation pass.
pub(crate) fn needs_anim(p: &Project, c: &Cut) -> bool {
    animates(p)
        && match c.render_kind() {
            "layered" => c.has_cast(),
            "full" => true,
            _ => false,
        }
}

// ------------------------------------------------------------------ Python

/// The VFX pack's Python and `anime_comp.py`.
fn comp_tools() -> Result<(PathBuf, PathBuf)> {
    let python = mv_studio::vfx_pack::python_in(&mv_core::paths::root()).ok_or_else(|| anyhow!("Compositing needs the VFX pack: install it in Studio › Setup › Layered motion (PyTorch with CUDA)."))?;
    let layout = ui_studio::setup::latest().and_then(|c| c.env.scripts.ok()).or_else(|| mv_studio::PythonLayout::resolve(None).ok()).ok_or_else(|| anyhow!("Studio's scripts were not found (open Studio and finish Setup)"))?;
    let script = layout.script("anime_comp.py").map_err(|e| anyhow!("anime_comp.py was not found: {e}"))?;
    Ok((python, script))
}

/// Runs `anime_comp.py <cmd>` on `job` and returns its last answer. Progress lines become the
/// status as "<what> · n of m".
async fn run_comp(cmd: &str, job: serde_json::Value, work: &Path, what: &str, st: &Arc<Mutex<String>>) -> Result<serde_json::Value> {
    use tokio::io::AsyncBufReadExt as _;
    let (python, script) = tokio::task::spawn_blocking(comp_tools).await??;
    tokio::fs::create_dir_all(work).await?;
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let file = work.join(format!("{cmd}-{}-{}.json", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    tokio::fs::write(&file, serde_json::to_vec(&job)?).await?;
    let mut c = tokio::process::Command::new(&python);
    c.arg(&script).arg(cmd).arg(&file).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    c.creation_flags(0x0800_0000);
    let mut child = c.spawn().with_context(|| format!("could not start {}", python.display()))?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no output from the compositor"))?;
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let mut last = serde_json::Value::Null;
    while let Some(line) = lines.next_line().await? {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else { continue };
        if v.get("type").and_then(|t| t.as_str()) == Some("progress") {
            let (d, t) = (v["done"].as_u64().unwrap_or(0), v["total"].as_u64().unwrap_or(0));
            set(st, format!("{what} · {d} of {t}"));
        } else if v.get("ok").is_some() {
            last = v;
        }
    }
    let out = child.wait_with_output().await?;
    let _ = tokio::fs::remove_file(&file).await;
    if last.get("ok").and_then(|o| o.as_bool()) != Some(true) {
        let why = last.get("message").and_then(|m| m.as_str()).map(str::to_string).unwrap_or_else(|| String::from_utf8_lossy(&out.stderr).lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it stopped without an answer").to_string());
        bail!("the compositor could not {what}: {why}");
    }
    Ok(last)
}

// ------------------------------------------------------------------ building

/// Builds the job for an anime task from the project as it is now.
pub(crate) fn build(p: &Project, task: &Task, keys: &Keys, status: Arc<Mutex<String>>) -> Result<Job> {
    match task {
        Task::Panels { chapter } => panels_job(p, *chapter, status),
        Task::Rough { chapter, cut } => rough_job(p, *chapter, *cut, status),
        Task::Animatic { chapter } => edit_job(p, *chapter, true, status),
        Task::Plate(id) => {
            let pl = p.plates.iter().find(|x| &x.id == id).cloned().ok_or_else(|| anyhow!("that plate is gone"))?;
            // The manga's own drawings of this place: its architecture and props.
            let mut refs: Vec<Reference> = vec![];
            if let Some(f) = p.location(&pl.location).and_then(|l| existing(p, l.sheet.as_ref())) {
                refs.push(Reference { file: f, caption: String::new() });
            }
            let place = pl.location.to_lowercase();
            for ch in &p.chapters {
                for sp in ch.anime.panels.iter().filter(|sp| sp.story() && !place.is_empty() && sp.location.to_lowercase().contains(&place)).take(2) {
                    if refs.len() < 3 {
                        refs.push(Reference { file: p.file(&sp.file), caption: String::new() });
                    }
                }
            }
            // The place is often worded differently in the panels than in the plate: the panels its
            // own cuts adapt show it all the same.
            if refs.len() < 2 {
                for ch in &p.chapters {
                    for i in ch.anime.cuts.iter().filter(|c| c.plate == pl.id).flat_map(|c| c.from.iter().take(1)) {
                        if let Some(sp) = ch.anime.panels.get(*i).filter(|sp| sp.story()) {
                            let f = p.file(&sp.file);
                            if refs.len() < 3 && !refs.iter().any(|r| r.file == f) {
                                refs.push(Reference { file: f, caption: "the manga's drawing of this place (ignore its people, balloons and text)".into() });
                            }
                        }
                    }
                }
            }
            let refs = numbered(refs.into_iter().filter(|r| r.file.is_file()).collect(), |i| if i == 0 { format!("{} as the manga draws it", pl.location) } else { "the manga's drawing of this place".to_string() });
            let prompt = anime::plate(p, &pl, &captions(&refs));
            let out = p.new_image("plates", &format!("plate-{}", pl.id));
            let id = pl.id.clone();
            image_job(p, keys, format!("Plate · {}", pl.location), prompt, refs, 16. / 9., out, status, move |rel| Made::Plate { id, rel })
        }
        Task::FxEl(id) => {
            let f = p.fx.iter().find(|x| &x.id == id).cloned().ok_or_else(|| anyhow!("that effect is gone"))?;
            let out = p.new_image("fx", &format!("fx-{}", f.id));
            let id = f.id.clone();
            image_job(p, keys, format!("Effect · {}", f.what), anime::fx(&f), vec![], 1., out, status, move |rel| Made::Fx { id, rel })
        }
        Task::PropEl(id) => {
            let x = p.props.iter().find(|x| &x.id == id).cloned().ok_or_else(|| anyhow!("that prop is gone"))?;
            // The panels of the cuts where someone rests on it: the manga's design of it.
            let mut refs: Vec<Reference> = vec![];
            for ch in &p.chapters {
                for c in ch.anime.cuts.iter().filter(|c| c.plate == x.plate && c.touched().as_deref() == Some(x.what.as_str())) {
                    if let Some(sp) = c.from.first().and_then(|i| ch.anime.panels.get(*i)) {
                        let f = p.file(&sp.file);
                        if refs.len() < 3 && !refs.iter().any(|r| r.file == f) {
                            refs.push(Reference { file: f, caption: String::new() });
                        }
                    }
                }
            }
            let refs = numbered(refs.into_iter().filter(|r| r.file.is_file()).collect(), |_| format!("the {} as the manga draws it (ignore its people, balloons and text)", x.what));
            let prompt = anime::prop_sheet(p, &x, &captions(&refs));
            let out = p.new_image("props", &format!("prop-{}", x.id));
            let id = x.id.clone();
            image_job(p, keys, format!("Prop · {}", x.what), prompt, refs, 16. / 9., out, status, move |rel| Made::Prop { id, rel })
        }
        Task::Layer { chapter, cut } if cut_of(p, *chapter, *cut)?.has_cast() => layout_job(p, keys, *chapter, *cut, status),
        Task::Layer { chapter, cut } => {
            let c = cut_of(p, *chapter, *cut)?;
            let aspect_f = if p.settings.video_aspect == "9:16" { 9. / 16. } else { 16. / 9. };
            let m = jobs::image_model(p, keys);
            let cap = jobs::ref_cap(p, m).max(1);
            let mut refs: Vec<(PathBuf, String)> = vec![];
            let cast: Vec<_> = c.characters.iter().filter_map(|x| p.character(&x.name)).collect();
            if c.has_cast() {
                for m in &cast {
                    if let Some(f) = existing(p, m.sheet.as_ref()) {
                        refs.push((f, format!("{} model sheet (face, hair, eyes, outfit and colours must match)", m.name)));
                    }
                }
                for m in &cast {
                    if let Some(f) = existing(p, m.expressions.as_ref()) {
                        refs.push((f, format!("{} expression sheet", m.name)));
                    }
                }
            }
            if c.has_cast() {
                for x in p.props.iter().filter(|x| x.plate == c.plate) {
                    if let Some(f) = existing(p, x.image.as_ref()) {
                        refs.push((f, format!("the {} prop sheet: draw it exactly like this, on the layer", x.what)));
                    }
                }
            }
            let ch = p.chapter(*chapter).cloned().unwrap_or_default();
            for i in c.from.iter().take(2) {
                if let Some(sp) = ch.anime.panels.get(*i) {
                    refs.push((p.file(&sp.file), "the manga panel this shot adapts: keep its people, their outfits and acting, redrawn as a colour anime frame in the framing asked for (ignore its balloons, text, sound effects and screentone)".into()));
                }
            }
            if let Some(f) = p.plates.iter().find(|x| x.id == c.plate).and_then(|x| existing(p, x.image.as_ref())) {
                refs.push((f, if c.has_cast() { "the background plate this frame sits on: match its light direction, colour and perspective, but do NOT draw it".into() } else { "the background of this shot: the place, its light and colours".into() }));
            }
            let mut refs: Vec<Reference> = refs.into_iter().filter(|(f, _)| f.is_file()).map(|(file, caption)| Reference { file, caption }).collect();
            refs.truncate(cap);
            let refs = numbered(refs, |_| String::new());
            let aspect = layout::nearest(aspect_f, jobs::shapes(p, m));
            let has_cast = c.has_cast();
            let prompt = if has_cast { anime::layer(p, &c, aspect, &captions(&refs)) } else { anime::frame(p, &c, aspect, &captions(&refs)) };
            let out = p.new_image("layers", &format!("ch{chapter:03}-c{cut:03}-{}", if has_cast { "layer" } else { "frame" }));
            let (chapter, cut) = (*chapter, *cut);
            image_job(p, keys, format!("Chapter {chapter} · cut {cut} · {}", if has_cast { "characters" } else { "frame" }), prompt, refs, aspect_f, out, status, move |rel| if has_cast { Made::Layer { chapter, cut, layer: Some(rel), key: None, bg: None, layout: None } } else { Made::Layer { chapter, cut, layer: None, key: Some(rel), bg: None, layout: None } })
        }
        Task::Anim { chapter, cut } => anim_job(p, *chapter, *cut, status),
        Task::Comp { chapter, cut } => comp_job(p, *chapter, *cut, status),
        Task::Episode { chapter } => edit_job(p, *chapter, false, status),
        _ => bail!("not an anime task"),
    }
}

fn captions(refs: &[Reference]) -> Vec<String> {
    refs.iter().map(|r| r.caption.clone()).collect()
}

/// Numbers every caption ("Image 2: ..."), giving one to a reference that has none.
fn numbered(mut refs: Vec<Reference>, default: impl Fn(usize) -> String) -> Vec<Reference> {
    for (i, r) in refs.iter_mut().enumerate() {
        let what = if r.caption.is_empty() { default(i) } else { r.caption.clone() };
        r.caption = format!("Image {}: {what}", i + 1);
    }
    refs
}

/// One picture with the project's image route (Magnific's website or the model's API).
#[allow(clippy::too_many_arguments)]
fn image_job(p: &Project, keys: &Keys, label: String, prompt: String, refs: Vec<Reference>, aspect: f32, out: PathBuf, status: Arc<Mutex<String>>, done: impl FnOnce(String) -> Made + Send + 'static) -> Result<Job> {
    let m = jobs::image_model(p, keys);
    let res = jobs::resolution(p, m);
    let web = p.settings.web_images;
    let k = if web { String::new() } else { jobs::key(m)? };
    let aspect = layout::nearest(aspect, jobs::shapes(p, m));
    let dir = p.dir();
    let st = status.clone();
    let fut = async move {
        jobs::paint(web, m, &k, &prompt, &refs, aspect, res, &out, move |s| set(&st, s)).await?;
        Ok(Outcome::Anime(done(rel_in(&dir, &out))))
    };
    Ok(Job { label, credits: jobs::cost(p, m, res), model: (!web).then_some(m), fut: fut.boxed() })
}

/// A cut with the cast, drawn layout first as a TV anime cut is: (1) the place painted again from
/// this cut's camera (its own background, horizon and perspective), (2) the characters and the
/// props they touch drawn INTO it, lit by its light, with cast and contact shadows, (3) the
/// characters lifted onto the chroma screen from that frame, for animation. The background is
/// kept when it was made before (a redraw of the characters reuses it).
fn layout_job(p: &Project, keys: &Keys, chapter: u32, n: u32, status: Arc<Mutex<String>>) -> Result<Job> {
    let c = cut_of(p, chapter, n)?;
    let m = jobs::image_model(p, keys);
    let res = jobs::resolution(p, m);
    let web = p.settings.web_images;
    let k = if web { String::new() } else { jobs::key(m)? };
    let aspect_f = if p.settings.video_aspect == "9:16" { 9. / 16. } else { 16. / 9. };
    let aspect = layout::nearest(aspect_f, jobs::shapes(p, m));
    let cap = jobs::ref_cap(p, m).max(2);
    let ch = p.chapter(chapter).cloned().unwrap_or_default();
    let panel = c.from.first().and_then(|i| ch.anime.panels.get(*i)).map(|sp| p.file(&sp.file)).filter(|f| f.is_file());
    let plate = p.plates.iter().find(|x| x.id == c.plate).and_then(|x| existing(p, x.image.as_ref()));
    // (1) The cut's background: the plate re-seen from this camera, the panel's framing.
    let mut bg_refs: Vec<Reference> = vec![];
    if let Some(f) = &plate {
        bg_refs.push(Reference { file: f.clone(), caption: "the background plate: the place to paint again from this cut's camera".into() });
    }
    if let Some(f) = &panel {
        bg_refs.push(Reference { file: f.clone(), caption: "the manga panel this cut adapts: its framing and camera angle (ignore its people, balloons and text)".into() });
    }
    let bg_refs = numbered(bg_refs, |_| String::new());
    let bg_prompt = anime::cut_bg(p, &c, aspect, &captions(&bg_refs));
    let dir = p.dir();
    let made_bg = existing(p, c.bg.as_ref());
    let bg_out = made_bg.clone().unwrap_or_else(|| p.new_image("layers", &format!("ch{chapter:03}-c{n:03}-bg")));
    // (2) The layout: Image 1 is that background; then the sheets, prop sheets and the panel.
    let mut more: Vec<Reference> = vec![];
    for x in c.characters.iter().filter_map(|x| p.character(&x.name)) {
        if let Some(f) = existing(p, x.sheet.as_ref()) {
            more.push(Reference { file: f, caption: format!("{} model sheet (face, hair, eyes, outfit and colours must match)", x.name) });
        }
    }
    for x in p.props.iter().filter(|x| x.plate == c.plate) {
        if let Some(f) = existing(p, x.image.as_ref()) {
            more.push(Reference { file: f, caption: format!("the {} prop sheet: draw it exactly like this", x.what) });
        }
    }
    if let Some(f) = &panel {
        more.push(Reference { file: f.clone(), caption: "the manga panel this shot adapts: its people, their acting and the framing (ignore its balloons, text and screentone)".into() });
    }
    more.truncate(cap - 1);
    let mut lay_refs = vec![Reference { file: bg_out.clone(), caption: "this cut's background (keep it exactly, draw into it)".into() }];
    lay_refs.extend(more);
    let lay_refs = numbered(lay_refs, |_| String::new());
    let lay_prompt = anime::layout_frame(p, &c, aspect, &captions(&lay_refs));
    let lay_out = p.new_image("layers", &format!("ch{chapter:03}-c{n:03}-layout"));
    // (3) The cel lifted from the layout.
    let cel_prompt = anime::cel(p, &c);
    let cel_out = p.new_image("layers", &format!("ch{chapter:03}-c{n:03}-layer"));
    let images = if made_bg.is_some() { 2. } else { 3. };
    let credits = jobs::cost(p, m, res) * images;
    let st = status.clone();
    let fut = async move {
        if made_bg.is_none() {
            set(&st, "Painting the cut's background from its camera");
            jobs::paint(web, m, &k, &bg_prompt, &bg_refs, aspect, res, &bg_out, |s| set(&st, format!("Background · {s}"))).await?;
        }
        set(&st, "Drawing the characters into it, lit and shadowed");
        jobs::paint(web, m, &k, &lay_prompt, &lay_refs, aspect, res, &lay_out, |s| set(&st, format!("Layout · {s}"))).await?;
        set(&st, "Lifting the characters onto their layer");
        let cel_refs = vec![Reference { file: lay_out.clone(), caption: "Image 1: the cut's layout frame".into() }];
        jobs::paint(web, m, &k, &cel_prompt, &cel_refs, aspect, res, &cel_out, |s| set(&st, format!("Cel · {s}"))).await?;
        Ok(Outcome::Anime(Made::Layer { chapter, cut: n, layer: Some(rel_in(&dir, &cel_out)), key: None, bg: Some(rel_in(&dir, &bg_out)), layout: Some(rel_in(&dir, &lay_out)) }))
    };
    Ok(Job { label: format!("Chapter {chapter} · cut {n} · layout, characters and layer"), credits, model: (!web).then_some(m), fut: fut.boxed() })
}

/// The chapter's real pages, from the downloaded manga, cut into panels inside the project.
fn panels_job(p: &Project, chapter: u32, status: Arc<Mutex<String>>) -> Result<Job> {
    let title = p.adapts.clone().ok_or_else(|| anyhow!("this anime does not adapt a downloaded manga"))?;
    let dir = p.dir();
    let out_dir = dir.join("source").join(format!("ch{chapter:03}"));
    let rtl = p.settings.rtl;
    let st = status.clone();
    let fut = async move {
        set(&st, "Finding the chapter's pages");
        let t2 = title.clone();
        let pages = tokio::task::spawn_blocking(move || chapter_pages(&t2, chapter)).await??;
        if pages.is_empty() {
            bail!("chapter {chapter} of {title} is not downloaded (download it in the Manga reader first)");
        }
        let _ = tokio::fs::remove_dir_all(&out_dir).await;
        let v = run_comp("panels", serde_json::json!({ "pages": pages, "out_dir": out_dir, "rtl": rtl }), &dir.join("anim").join("work"), "cut the pages into panels", &st).await?;
        let panels: Vec<SourcePanel> = v["panels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| {
                let f = PathBuf::from(x["file"].as_str()?);
                Some(SourcePanel { file: rel_in(&dir, &f), page: x["page"].as_u64().unwrap_or(0) as u32 + 1, w: x["w"].as_u64().unwrap_or(0) as u32, h: x["h"].as_u64().unwrap_or(0) as u32, ..Default::default() })
            })
            .collect();
        if panels.is_empty() {
            bail!("no panels were found on the pages of chapter {chapter}");
        }
        Ok(Outcome::Anime(Made::Panels { chapter, panels }))
    };
    Ok(Job { label: format!("Chapter {chapter} · cutting the pages into panels"), credits: 0., model: None, fut: fut.boxed() })
}

/// The page images of chapter `n` of the downloaded manga `title`, in order.
pub(crate) fn chapter_pages(title: &str, n: u32) -> Result<Vec<PathBuf>> {
    let norm = |s: &str| s.trim().to_lowercase();
    let groups = ui_studio::data::load().map_err(|e| anyhow!("could not read the library: {e}"))?;
    let t = norm(title);
    // The copy of the manga that has this chapter downloaded (a title can be in the library twice).
    for g in groups.iter().filter(|g| norm(&g.title) == t) {
        for c in g.chapters.iter().filter(|c| c.number.is_some_and(|x| (x - n as f64).abs() < 0.01)) {
            let mut pages: Vec<PathBuf> = std::fs::read_dir(&c.pages_dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_lowercase().as_str(), "jpg" | "jpeg" | "png" | "webp")))
                .collect();
            pages.sort();
            if !pages.is_empty() {
                return Ok(pages);
            }
        }
    }
    Ok(vec![])
}

/// A cut's voice: its lines spoken by each character's own voice (none when nobody speaks).
async fn voice_for(p_cast: Vec<jobs::Speaker>, c: &Cut, cast_dir: PathBuf, voice_pick: String, dir: PathBuf, st: &Arc<Mutex<String>>) -> Result<Option<(PathBuf, f64)>> {
    if c.lines.iter().all(|l| l.text.trim().is_empty()) {
        return Ok(None);
    }
    let panel = mv_create::Panel { lines: c.lines.clone(), characters: c.characters.clone(), ..Default::default() };
    let spoken = prompts::delivery(&panel);
    if spoken.is_empty() {
        return Ok(None);
    }
    let made = jobs::scene_voice(spoken, p_cast, cast_dir, voice_pick, dir, c.seconds.ceil() as u32, st).await?;
    Ok(Some(made))
}

fn voice_setup(p: &Project, chapter: u32, n: u32) -> (Vec<jobs::Speaker>, PathBuf, String, PathBuf) {
    (p.cast.iter().map(jobs::Speaker::of).collect(), p.dir().join("voice"), p.settings.voice.clone(), p.dir().join("voice").join(format!("anime-ch{chapter:03}-c{n:03}-{}", mv_create::now())))
}

/// How long a cut must be for a voice of `heard` seconds: the lead before the first line, the
/// voice, a moment after it.
fn fit_voice(seconds: f32, heard: f64) -> f32 {
    if heard <= 0. {
        return seconds;
    }
    seconds.max(pacing::LINE_LEAD + heard as f32 + 0.2).min(pacing::CUT_MAX)
}

/// A cut of the animatic: its lines are voiced (once: the finished cut uses the same voice), and
/// the manga panel it adapts is shown under the cut's camera move, as long as the cut runs.
fn rough_job(p: &Project, chapter: u32, n: u32, status: Arc<Mutex<String>>) -> Result<Job> {
    let mut c = cut_of(p, chapter, n)?;
    let kind = kind_of(p, chapter, &c);
    let dir = p.dir();
    let panel: Option<(PathBuf, u32, u32)> = p.chapter(chapter).and_then(|ch| c.from.first().and_then(|i| ch.anime.panels.get(*i)).map(|sp| (p.file(&sp.file), sp.w, sp.h))).filter(|(f, ..)| f.is_file());
    let voice_done = existing(p, c.voice.as_ref());
    let (cast, cast_dir, pick, vdir) = voice_setup(p, chapter, n);
    let aspect = p.settings.video_aspect.clone();
    let out = dir.join("rough").join(format!("ch{chapter:03}-c{n:03}-{}.mp4", mv_create::now()));
    let st = status.clone();
    let fut = async move {
        let mut voice_rel = None;
        let mut heard = 0.;
        match voice_done {
            Some(v) => heard = tokio::task::spawn_blocking(move || video::media_seconds(&v)).await?.unwrap_or(0.),
            None => {
                set(&st, "Voicing the cut");
                // An animatic without a voice is still an animatic: a cut that cannot be voiced
                // now is shown silent, and voiced when the finished cut is made.
                match voice_for(cast, &c, cast_dir, pick, vdir, &st).await {
                    Ok(Some((v, secs))) => {
                        voice_rel = Some(rel_in(&dir, &v));
                        // (`secs` is the voice and a moment after it.)
                        heard = (secs - 0.6).max(0.3);
                    }
                    Ok(None) => {}
                    Err(e) => eprintln!("[creative] cut {n} could not be voiced for the animatic: {e:#}"),
                }
            }
        }
        c.seconds = fit_voice(c.seconds, heard);
        let mut job = anime::rough_job(&c, &kind, &out, panel.as_ref().map(|(f, w, h)| (f.as_path(), *w, *h)), &aspect);
        job["quality"] = "rough".into();
        run_comp("comp", job, &dir.join("anim").join("work"), "draw the animatic's cut", &st).await?;
        Ok(Outcome::Anime(Made::Rough { chapter, cut: n, rel: rel_in(&dir, &out), voice: voice_rel, seconds: c.seconds }))
    };
    Ok(Job { label: format!("Chapter {chapter} · cut {n} · animatic"), credits: 0., model: None, fut: fut.boxed() })
}

/// The cuts joined with their transitions, voices and music: the animatic (`rough`: every cut,
/// finished or from its panel, at a small size) or the episode (the finished cuts).
fn edit_job(p: &Project, chapter: u32, rough: bool, status: Arc<Mutex<String>>) -> Result<Job> {
    let ch = p.chapter(chapter).ok_or_else(|| anyhow!("no chapter {chapter}"))?;
    let dir = p.dir();
    let cuts: Vec<(PathBuf, String, Option<PathBuf>)> = ch
        .anime
        .cuts
        .iter()
        .filter_map(|c| {
            let file = existing(p, c.comp.as_ref()).or_else(|| existing(p, c.rough.as_ref()).filter(|_| rough))?;
            Some((file, c.transition.clone(), existing(p, c.voice.as_ref())))
        })
        .collect();
    if cuts.is_empty() {
        bail!("chapter {chapter} has no {} cuts yet", if rough { "animatic" } else { "finished" });
    }
    let music = Some(PathBuf::from(&p.settings.music)).filter(|m| !p.settings.music.is_empty() && m.is_file());
    let aspect = p.settings.video_aspect.clone();
    let out = dir.join("video").join(format!("{}-{}-ch{chapter:03}-{}.mp4", mv_create::slug(&p.title), if rough { "animatic" } else { "anime" }, mv_create::now()));
    let work = dir.join("anim").join("work");
    let st = status.clone();
    let fut = async move {
        set(&st, format!("Editing {} cuts", cuts.len()));
        let list: Vec<serde_json::Value> = tokio::task::spawn_blocking(move || cuts.iter().map(|(v, tr, voice)| serde_json::json!({ "video": v, "seconds": video::media_seconds(v).unwrap_or(3.), "transition": tr, "voice": voice, "voice_at": pacing::LINE_LEAD })).collect()).await?;
        let (w, h) = video::frame(&aspect);
        let (w, h) = if rough { (w * 2 / 3, h * 2 / 3) } else { (w, h) };
        run_comp("edit", serde_json::json!({ "out": out, "width": w, "height": h, "fps": 24, "quality": if rough { "rough" } else { "final" }, "cuts": list, "music": music, "music_db": -17 }), &work, if rough { "edit the animatic" } else { "edit the episode" }, &st).await?;
        let rel = rel_in(&dir, &out);
        Ok(Outcome::Anime(if rough { Made::Animatic { chapter, rel } } else { Made::Episode { chapter, rel } }))
    };
    Ok(Job { label: format!("Chapter {chapter} {}", if rough { "animatic" } else { "anime episode" }), credits: 0., model: None, fut: fut.boxed() })
}

/// The cut animated: its character layer on the screen (`layered`), or the whole frame
/// (`full`: plate and characters composited into a key frame first, unless it was drawn whole).
fn anim_job(p: &Project, chapter: u32, n: u32, status: Arc<Mutex<String>>) -> Result<Job> {
    let c = cut_of(p, chapter, n)?;
    let kind = kind_of(p, chapter, &c);
    let vm = video::video_model(&p.settings.video_model);
    if vm.via == VideoVia::Local {
        bail!("the video model is FFmpeg pan and zoom, which does not animate; pick a video model, or the cuts stay held drawings");
    }
    let dir = p.dir();
    let full = c.render_kind() == "full";
    let layer = existing(p, c.layer.as_ref());
    let drawn_key = existing(p, c.key.as_ref()).filter(|_| !c.has_cast());
    let plate = p.plates.iter().find(|x| x.id == c.plate).and_then(|x| existing(p, x.image.as_ref()));
    if !full && layer.is_none() {
        bail!("cut {n} has no character layer yet");
    }
    let key_out = dir.join("layers").join(format!("ch{chapter:03}-c{n:03}-key-{}.png", mv_create::now()));
    // The key frame is the cut's first frame: before the camera has moved.
    let still_cut = Cut { camera: "fix".into(), shake: 0., ..c.clone() };
    let laid = existing(p, c.layout.as_ref());
    let still_job = match &laid {
        // Drawn layout first, the frame is whole already: lit, shadowed, in perspective.
        Some(l) => anime::comp_job(&still_cut, &kind, &key_out, CompParts { plate: Some(l), ..Default::default() }, &p.settings.video_aspect, false),
        None => anime::comp_job(&still_cut, &kind, &key_out, CompParts { plate: plate.as_deref(), layer: layer.as_deref().map(|l| (l, false)), ..Default::default() }, &p.settings.video_aspect, false),
    };
    let key = match vm.via {
        VideoVia::Magnific => Some(ui_studio::images::key().ok_or_else(|| anyhow!("Connect Magnific first: add the API key in Settings › Magnific."))?),
        VideoVia::Service(id) => Some(mv_ai::service_key(mv_core::paths::config(), id).ok_or_else(|| anyhow!("{} needs a {} key: add it in Settings › AI services.", vm.name, mv_ai::service_def(id).map(|d| d.label).unwrap_or(id)))?),
        _ => None,
    };
    if vm.via == VideoVia::ComfyUi && !Path::new(&p.settings.comfy_workflow).is_file() {
        bail!("Pick your ComfyUI workflow (API format JSON) under How it is made first");
    }
    let comfy = (p.settings.comfy_url.clone(), PathBuf::from(p.settings.comfy_workflow.clone()));
    let prompt = anime::motion(&c);
    let seconds = vm.fit_seconds(c.seconds.ceil() as u32);
    let aspect = p.settings.video_aspect.clone();
    let out = dir.join("anim").join(format!("ch{chapter:03}-c{n:03}-{}.mp4", mv_create::now()));
    let voice_done = existing(p, c.voice.as_ref());
    let (cast, cast_dir, pick, vdir) = voice_setup(p, chapter, n);
    let st = status.clone();
    let fut = async move {
        // The picture the video model starts from.
        let (src, made_key) = if full {
            match drawn_key {
                Some(k) => (k, None),
                None => {
                    if plate.is_none() && layer.is_none() {
                        bail!("cut {n} has neither its plate nor its characters yet");
                    }
                    run_comp("still", still_job, &dir.join("anim").join("work"), "composite the key frame", &st).await?;
                    (key_out.clone(), Some(rel_in(&dir, &key_out)))
                }
            }
        } else {
            (layer.clone().unwrap_or_default(), None)
        };
        let mut voice = None;
        match vm.via {
            VideoVia::ComfyUi => video::comfy_clip(&comfy.0, &comfy.1, &src, &prompt, seconds, &out, |s| set(&st, s)).await?,
            VideoVia::ComfyLtx => {
                // LTX moves the mouths with the voice: the cut's voice (the animatic's, when it
                // was made) comes first.
                let (audio, secs) = match voice_done {
                    Some(v) => {
                        let v2 = v.clone();
                        let heard = tokio::task::spawn_blocking(move || video::media_seconds(&v2)).await?.unwrap_or(c.seconds as f64);
                        (v, heard)
                    }
                    None => match voice_for(cast, &c, cast_dir, pick, vdir.clone(), &st).await? {
                        Some((v, secs)) => {
                            voice = Some(rel_in(&dir, &v));
                            (v, secs)
                        }
                        None => {
                            let s = vdir.join("silence.wav");
                            let s2 = s.clone();
                            let secs = c.seconds as f64;
                            tokio::task::spawn_blocking(move || video::silence(secs, &s2)).await??;
                            (s, secs)
                        }
                    },
                };
                video::ltx_clip(&comfy.0, &src, &audio, &prompt, secs.max(c.seconds as f64), &aspect, &out, |s| set(&st, format!("LTX 2.3 · {s}"))).await?;
            }
            _ => video::api_clip(vm, key.as_deref().unwrap_or(""), &src, &prompt, seconds, &aspect, &out, |s| set(&st, s)).await?,
        }
        Ok(Outcome::Anime(Made::Anim { chapter, cut: n, rel: rel_in(&dir, &out), key: made_key, voice }))
    };
    Ok(Job { label: format!("Chapter {chapter} · cut {n} · animation"), credits: 0., model: None, fut: fut.boxed() })
}

/// The cut voiced and composited: every layer keyed, placed, lit and moved by the camera, the
/// effects over it, the shared finish on top. A cut is as long as its board says, or as its
/// voice needs.
fn comp_job(p: &Project, chapter: u32, n: u32, status: Arc<Mutex<String>>) -> Result<Job> {
    let mut c = cut_of(p, chapter, n)?;
    let kind = kind_of(p, chapter, &c);
    let dir = p.dir();
    let plate = p.plates.iter().find(|x| x.id == c.plate).and_then(|x| existing(p, x.image.as_ref()));
    let drawn_key = existing(p, c.key.as_ref()).filter(|_| !c.has_cast());
    let layer = existing(p, c.layer.as_ref());
    let anim = existing(p, c.anim.as_ref()).filter(|_| needs_anim(p, &c));
    let laid = existing(p, c.layout.as_ref());
    let cut_bg = existing(p, c.bg.as_ref());
    let fx: Vec<(PathBuf, mv_create::anime::FxUse, mv_create::anime::FxElement)> = c
        .fx
        .iter()
        .filter_map(|u| {
            let e = p.fx.iter().find(|x| x.id == u.id)?;
            Some((existing(p, e.clip.as_ref()).or_else(|| existing(p, e.image.as_ref()))?, u.clone(), e.clone()))
        })
        .collect();
    let voice_done = existing(p, c.voice.as_ref());
    let (cast, cast_dir, pick, vdir) = voice_setup(p, chapter, n);
    let aspect = p.settings.video_aspect.clone();
    let out = dir.join("cuts").join(format!("ch{chapter:03}-c{n:03}-{}.mp4", mv_create::now()));
    if plate.is_none() && drawn_key.is_none() && layer.is_none() && anim.is_none() {
        bail!("cut {n} has nothing drawn yet");
    }
    let st = status.clone();
    let fut = async move {
        let mut voice_rel = None;
        let mut heard = 0.;
        match voice_done {
            Some(v) => heard = tokio::task::spawn_blocking(move || video::media_seconds(&v)).await?.unwrap_or(0.),
            None => {
                set(&st, "Voicing the cut");
                if let Some((v, secs)) = voice_for(cast, &c, cast_dir, pick, vdir, &st).await? {
                    voice_rel = Some(rel_in(&dir, &v));
                    heard = (secs - 0.6).max(0.3);
                }
            }
        }
        c.seconds = fit_voice(c.seconds, heard);
        let fx_refs: Vec<(PathBuf, &mv_create::anime::FxUse, &mv_create::anime::FxElement)> = fx.iter().map(|(f, u, e)| (f.clone(), u, e)).collect();
        let job = if c.render_kind() == "full" && anim.is_some() {
            anime::comp_job(&c, &kind, &out, CompParts { base: anim.as_deref(), fx: &fx_refs, ..Default::default() }, &aspect, true)
        } else {
            match (&laid, &anim) {
                // A held drawing: the layout frame is the picture, under the camera.
                (Some(l), None) => anime::comp_job(&c, &kind, &out, CompParts { plate: Some(l), fx: &fx_refs, ..Default::default() }, &aspect, true),
                // Animated characters over the cut's own background, with the layout's shadows.
                (Some(l), Some(a)) => anime::comp_job(&c, &kind, &out, CompParts { plate: cut_bg.as_deref().or(plate.as_deref()), layout: Some(l), layer: Some((a.as_path(), true)), base: None, fx: &fx_refs }, &aspect, true),
                (None, _) => {
                    let bg = drawn_key.as_deref().or(plate.as_deref());
                    let fg = match (&anim, &layer) {
                        (Some(a), _) => Some((a.as_path(), true)),
                        (None, Some(l)) => Some((l.as_path(), false)),
                        _ => None,
                    };
                    anime::comp_job(&c, &kind, &out, CompParts { plate: bg, layer: fg, fx: &fx_refs, ..Default::default() }, &aspect, true)
                }
            }
        };
        run_comp("comp", job, &dir.join("anim").join("work"), "composite the cut", &st).await?;
        Ok(Outcome::Anime(Made::Comp { chapter, cut: n, rel: rel_in(&dir, &out), voice: voice_rel, seconds: c.seconds }))
    };
    Ok(Job { label: format!("Chapter {chapter} · cut {n} · compositing"), credits: 0., model: None, fut: fut.boxed() })
}

// ------------------------------------------------------------------ storing and planning

pub(crate) fn apply(p: &mut Project, made: Made) {
    match made {
        Made::Panels { chapter, panels } => {
            let a = &mut p.chapter_mut(chapter).anime;
            // New panels: everything read and boarded from the old ones goes.
            *a = mv_create::anime::AnimeChapter { panels, ..Default::default() };
        }
        Made::Rough { chapter, cut, rel, voice, seconds } => {
            let a = &mut p.chapter_mut(chapter).anime;
            if let Some(c) = a.cuts.iter_mut().find(|c| c.number == cut) {
                c.rough = Some(rel);
                c.seconds = seconds;
                if voice.is_some() {
                    c.voice = voice;
                }
            }
            a.animatic = None;
        }
        Made::Animatic { chapter, rel } => p.chapter_mut(chapter).anime.animatic = Some(rel),
        Made::Plate { id, rel } => {
            if let Some(x) = p.plates.iter_mut().find(|x| x.id == id) {
                x.image = Some(rel);
            }
            // Every cut on this plate is composited again.
            for ch in &mut p.chapters {
                for c in ch.anime.cuts.iter_mut().filter(|c| c.plate == id) {
                    c.comp = None;
                }
            }
        }
        Made::Prop { id, rel } => {
            // A new design: the layers that carry it are drawn again.
            let plate = p.props.iter().find(|x| x.id == id).map(|x| x.plate.clone());
            if let Some(x) = p.props.iter_mut().find(|x| x.id == id) {
                let redo = x.image.is_some();
                x.image = Some(rel);
                if redo {
                    for ch in &mut p.chapters {
                        for c in ch.anime.cuts.iter_mut().filter(|c| Some(&c.plate) == plate.as_ref() && c.has_cast()) {
                            c.layer = None;
                            c.anim = None;
                            c.comp = None;
                        }
                    }
                }
            }
        }
        Made::Fx { id, rel } => {
            if let Some(x) = p.fx.iter_mut().find(|x| x.id == id) {
                x.image = Some(rel);
            }
        }
        Made::Layer { chapter, cut, layer, key, bg, layout } => {
            if let Some(c) = p.chapter_mut(chapter).anime.cuts.iter_mut().find(|c| c.number == cut) {
                c.layer = layer;
                c.key = key;
                c.bg = bg;
                c.layout = layout;
                c.anim = None;
                c.comp = None;
            }
        }
        Made::Anim { chapter, cut, rel, key, voice } => {
            if let Some(c) = p.chapter_mut(chapter).anime.cuts.iter_mut().find(|c| c.number == cut) {
                c.anim = Some(rel);
                if key.is_some() {
                    c.key = key;
                }
                if voice.is_some() {
                    c.voice = voice;
                }
                c.comp = None;
            }
        }
        Made::Comp { chapter, cut, rel, voice, seconds } => {
            let ch = p.chapter_mut(chapter);
            if let Some(c) = ch.anime.cuts.iter_mut().find(|c| c.number == cut) {
                c.comp = Some(rel);
                c.seconds = seconds;
                if voice.is_some() {
                    c.voice = voice;
                }
            }
            ch.anime.video = None;
        }
        Made::Episode { chapter, rel } => p.chapter_mut(chapter).anime.video = Some(rel),
    }
}

/// Puts a board the director drew up on a copy of the project (`done`) into the open project:
/// the chapter's scenes, cuts and notes, and the plates and effects the board added (what is
/// already painted keeps its picture).
pub(crate) fn merge_board(live: &mut Project, done: &Project, n: u32) {
    for pl in &done.plates {
        if !live.plates.iter().any(|x| x.id == pl.id) {
            live.plates.push(pl.clone());
        }
    }
    for f in &done.fx {
        if !live.fx.iter().any(|x| x.id == f.id) {
            live.fx.push(f.clone());
        }
    }
    for x in &done.props {
        if !live.props.iter().any(|y| y.id == x.id) {
            live.props.push(x.clone());
        }
    }
    let Some(a) = done.chapter(n).map(|c| c.anime.clone()) else { return };
    let mine = &mut live.chapter_mut(n).anime;
    mine.treatment = a.treatment;
    mine.scenes = a.scenes;
    mine.cuts = a.cuts;
    mine.issues = a.issues;
    mine.animatic = a.animatic;
    mine.approved = a.approved;
    mine.video = a.video;
}

/// Puts the cast and places gathered from the pages (on a copy of the project, `done`) into
/// the open project, with the chapter's panels now naming them.
pub(crate) fn merge_cast(live: &mut Project, done: &Project, n: u32) {
    live.cast = done.cast.clone();
    live.locations = done.locations.clone();
    live.bible = done.bible.clone();
    if let Some(ch) = done.chapter(n).cloned() {
        let mine = live.chapter_mut(n);
        mine.title = ch.title;
        mine.summary = ch.summary;
        mine.anime.panels = ch.anime.panels;
        mine.anime.cast_done = ch.anime.cast_done;
    }
}

/// Where chapter `n`'s anime stands: the next thing to make, in the order a studio makes it.
/// Everything up to the animatic needs only the text and vision model; nothing is painted until
/// the animatic has been watched and approved.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AStage {
    /// The pages have to be cut into panels.
    Panels,
    /// The panels have to be read by the vision model.
    Read,
    /// The cast and places have to be gathered from the pages.
    Cast,
    /// The chapter has to be divided into scenes.
    Scenes,
    /// The storyboard has to be drawn up.
    Board,
    /// The animatic: each cut voiced and shown from its panel, then joined.
    Rough(Vec<Task>),
    /// The animatic waits to be watched: the user says to draw it.
    Approve,
    /// The design sheets of everyone in the board.
    Sheets(Vec<Task>),
    /// Plates and effects.
    Assets(Vec<Task>),
    /// Each cut's layer, animation and compositing.
    Cuts(Vec<Task>),
    /// The episode.
    Edit,
    Done,
}

impl AStage {
    /// The step of the page it belongs to (0 to 7; 8 when everything is made).
    pub(crate) fn index(&self) -> usize {
        match self {
            AStage::Panels | AStage::Read => 0,
            AStage::Cast => 1,
            AStage::Scenes | AStage::Board => 2,
            AStage::Rough(_) | AStage::Approve => 3,
            AStage::Sheets(_) => 4,
            AStage::Assets(_) => 5,
            AStage::Cuts(_) => 6,
            AStage::Edit => 7,
            AStage::Done => 8,
        }
    }

    pub(crate) fn tasks(&self) -> Vec<Task> {
        match self {
            AStage::Rough(t) | AStage::Sheets(t) | AStage::Assets(t) | AStage::Cuts(t) => t.clone(),
            _ => vec![],
        }
    }

    /// Each stage's own place in the order (two stages of one page step still differ), so an
    /// automatic run can tell a stage it is stuck on from the next one.
    pub(crate) fn order(&self) -> usize {
        match self {
            AStage::Panels => 0,
            AStage::Read => 1,
            AStage::Cast => 2,
            AStage::Scenes => 3,
            AStage::Board => 4,
            AStage::Rough(_) => 5,
            AStage::Approve => 6,
            AStage::Sheets(_) => 7,
            AStage::Assets(_) => 8,
            AStage::Cuts(_) => 9,
            AStage::Edit => 10,
            AStage::Done => 11,
        }
    }

    /// The step asks the AI (the others paint, composite or wait).
    pub(crate) fn asks_ai(&self) -> bool {
        matches!(self, AStage::Read | AStage::Cast | AStage::Scenes | AStage::Board)
    }
}

/// Sheets missing for `names`.
fn sheets_for(p: &Project, names: &[String]) -> Vec<Task> {
    let mut out = vec![];
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
    out
}

/// What is left to make of a cut (so the first finished cuts can be watched while the rest
/// are made).
pub(crate) fn cut_tasks(p: &Project, chapter: u32, c: &Cut) -> Vec<Task> {
    let mut t = vec![];
    let n = c.number;
    let drawn = if c.has_cast() { c.layer.is_some() } else { c.plate_only || c.key.is_some() };
    if !drawn {
        t.push(Task::Layer { chapter, cut: n });
    }
    if needs_anim(p, c) && (c.anim.is_none() || !drawn) {
        t.push(Task::Anim { chapter, cut: n });
    }
    if c.comp.is_none() || !t.is_empty() {
        t.push(Task::Comp { chapter, cut: n });
    }
    t
}

/// What is left of the animatic: the cuts not yet shown from their panels, then the join.
pub(crate) fn rough_tasks(p: &Project, n: u32) -> Vec<Task> {
    let Some(ch) = p.chapter(n) else { return vec![] };
    let mut t: Vec<Task> = ch.anime.cuts.iter().filter(|c| c.rough.is_none() && c.comp.is_none()).map(|c| Task::Rough { chapter: n, cut: c.number }).collect();
    if !t.is_empty() || ch.anime.animatic.is_none() {
        t.push(Task::Animatic { chapter: n });
    }
    t
}

pub(crate) fn stage(p: &Project, n: u32) -> AStage {
    let ch = p.chapter(n).cloned().unwrap_or_default();
    let a = &ch.anime;
    if a.panels.is_empty() {
        return AStage::Panels;
    }
    if !a.read() {
        return AStage::Read;
    }
    if !a.cast_done || p.cast.is_empty() {
        return AStage::Cast;
    }
    if a.scenes.is_empty() {
        return AStage::Scenes;
    }
    if !a.boarded() {
        return AStage::Board;
    }
    let rough = rough_tasks(p, n);
    if !rough.is_empty() {
        return AStage::Rough(rough);
    }
    if !a.approved {
        return AStage::Approve;
    }
    // Everyone the board shows needs a design before they are drawn.
    let mut names: Vec<String> = vec![];
    for name in a.cuts.iter().flat_map(|c| c.characters.iter().map(|x| x.name.clone())) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    let sheets = sheets_for(p, &names);
    if !sheets.is_empty() {
        return AStage::Sheets(sheets);
    }
    let mut assets = vec![];
    for pl in &p.plates {
        if pl.image.is_none() && a.cuts.iter().any(|c| c.plate == pl.id) {
            assets.push(Task::Plate(pl.id.clone()));
        }
    }
    for f in &p.fx {
        if f.image.is_none() && a.cuts.iter().any(|c| c.fx.iter().any(|u| u.id == f.id)) {
            assets.push(Task::FxEl(f.id.clone()));
        }
    }
    // Props before any layer: every layer of their plate draws them from the sheet.
    for x in &p.props {
        if x.image.is_none() && a.cuts.iter().any(|c| c.plate == x.plate) {
            assets.push(Task::PropEl(x.id.clone()));
        }
    }
    if !assets.is_empty() {
        return AStage::Assets(assets);
    }
    let cuts: Vec<Task> = a.cuts.iter().flat_map(|c| cut_tasks(p, n, c)).collect();
    if !cuts.is_empty() {
        return AStage::Cuts(cuts);
    }
    if a.video.is_none() {
        return AStage::Edit;
    }
    AStage::Done
}

#[cfg(test)]
mod tests {
    use super::*;
    use mv_create::anime::{FxElement, FxUse, Plate, Scene};
    use mv_create::{CastMember, Chapter, PanelChar};

    #[test]
    fn the_anime_is_made_in_studio_order() {
        let mut p = Project::default();
        p.chapters.push(Chapter { number: 1, ..Default::default() });
        assert_eq!(stage(&p, 1), AStage::Panels);
        p.chapter_mut(1).anime.panels = vec![SourcePanel { file: "source/a.jpg".into(), ..Default::default() }];
        assert_eq!(stage(&p, 1), AStage::Read);
        p.chapter_mut(1).anime.panels[0].read = true;
        // The cast comes from the pages: nothing is known of anyone before they are read.
        assert_eq!(stage(&p, 1), AStage::Cast);
        p.cast.push(CastMember { name: "Kai".into(), role: "protagonist".into(), intro_chapter: 1, ..Default::default() });
        p.normalize();
        p.chapter_mut(1).anime.cast_done = true;
        assert_eq!(stage(&p, 1), AStage::Scenes);
        p.chapter_mut(1).anime.scenes = vec![Scene { number: 1, kind: "dialogue".into(), to: 1, ..Default::default() }];
        assert_eq!(stage(&p, 1), AStage::Board);
        assert!(stage(&p, 1).asks_ai());
        p.plates = vec![Plate { id: "street".into(), ..Default::default() }, Plate { id: "unused".into(), ..Default::default() }];
        p.fx = vec![FxElement { id: "aura".into(), ..Default::default() }];
        let kai = PanelChar { name: "Kai".into(), ..Default::default() };
        p.chapter_mut(1).anime.cuts = vec![
            Cut { number: 1, scene: 1, plate: "street".into(), plate_only: true, motion: "camera".into(), ..Default::default() },
            Cut { number: 2, scene: 1, plate: "street".into(), motion: "lip".into(), characters: vec![kai.clone()], fx: vec![FxUse { id: "aura".into(), ..Default::default() }], ..Default::default() },
            Cut { number: 3, scene: 1, plate: "street".into(), motion: "full".into(), characters: vec![kai], ..Default::default() },
        ];
        // The animatic first: every cut from its panel, then the join. Nothing is painted yet.
        assert_eq!(stage(&p, 1), AStage::Rough(vec![Task::Rough { chapter: 1, cut: 1 }, Task::Rough { chapter: 1, cut: 2 }, Task::Rough { chapter: 1, cut: 3 }, Task::Animatic { chapter: 1 }]));
        for c in &mut p.chapter_mut(1).anime.cuts {
            c.rough = Some("rough/a.mp4".into());
        }
        assert_eq!(stage(&p, 1), AStage::Rough(vec![Task::Animatic { chapter: 1 }]));
        apply(&mut p, Made::Animatic { chapter: 1, rel: "video/animatic.mp4".into() });
        assert_eq!(stage(&p, 1), AStage::Approve);
        assert_eq!((stage(&p, 1).index(), stage(&p, 1).asks_ai()), (3, false));
        p.chapter_mut(1).anime.approved = true;
        assert_eq!(stage(&p, 1), AStage::Sheets(vec![Task::Sheet("kai".into()), Task::Expressions("kai".into())]));
        p.cast[0].sheet = Some("s.png".into());
        p.cast[0].expressions = Some("e.png".into());
        assert_eq!(stage(&p, 1), AStage::Assets(vec![Task::Plate("street".into()), Task::FxEl("aura".into())]));
        p.plates[0].image = Some("plates/street.png".into());
        p.fx[0].image = Some("fx/aura.png".into());
        // Pan and zoom does not animate: every cut is held drawings, composited.
        let t = stage(&p, 1).tasks();
        assert_eq!(t, vec![Task::Comp { chapter: 1, cut: 1 }, Task::Layer { chapter: 1, cut: 2 }, Task::Comp { chapter: 1, cut: 2 }, Task::Layer { chapter: 1, cut: 3 }, Task::Comp { chapter: 1, cut: 3 }]);
        // With a video model, lip, pose and full cuts are animated between drawing and compositing.
        p.settings.video_model = "mg-kling-3-turbo-720p".into();
        let t = stage(&p, 1).tasks();
        assert_eq!(&t[1..4], &[Task::Layer { chapter: 1, cut: 2 }, Task::Anim { chapter: 1, cut: 2 }, Task::Comp { chapter: 1, cut: 2 }]);
        for c in &mut p.chapter_mut(1).anime.cuts {
            c.layer = Some("l.png".into());
            c.anim = Some("a.mp4".into());
            c.comp = Some("c.mp4".into());
        }
        assert_eq!(stage(&p, 1), AStage::Edit);
        // A new plate picture sends its cuts back to compositing, and only that.
        apply(&mut p, Made::Plate { id: "street".into(), rel: "plates/new.png".into() });
        assert_eq!(stage(&p, 1).tasks(), vec![Task::Comp { chapter: 1, cut: 1 }, Task::Comp { chapter: 1, cut: 2 }, Task::Comp { chapter: 1, cut: 3 }]);
        // The animatic's voice set the cut's length; a finished cut keeps both.
        apply(&mut p, Made::Rough { chapter: 1, cut: 2, rel: "rough/b.mp4".into(), voice: Some("voice/b.wav".into()), seconds: 6.5 });
        let c = &p.chapter(1).unwrap().anime.cuts[1];
        assert_eq!((c.seconds, c.voice.as_deref(), p.chapter(1).unwrap().anime.animatic.as_deref()), (6.5, Some("voice/b.wav"), None));
        assert_eq!((fit_voice(3., 0.), fit_voice(3., 4.)), (3., 4.7));
        // New panels throw everything read and boarded away.
        apply(&mut p, Made::Panels { chapter: 1, panels: vec![SourcePanel::default()] });
        let a = &p.chapter(1).unwrap().anime;
        assert!(a.cuts.is_empty() && a.scenes.is_empty() && !a.cast_done && !a.approved);
    }

    #[test]
    fn a_board_drawn_on_a_copy_joins_the_open_project() {
        let mut live = Project::default();
        live.chapters.push(Chapter { number: 2, ..Default::default() });
        live.plates = vec![Plate { id: "street".into(), image: Some("plates/street.png".into()), ..Default::default() }];
        let mut done = live.clone();
        done.plates[0].image = None;
        done.plates.push(Plate { id: "roof".into(), ..Default::default() });
        done.fx.push(FxElement { id: "aura".into(), ..Default::default() });
        {
            let a = &mut done.chapter_mut(2).anime;
            a.treatment = "Open on the roof.".into();
            a.scenes = vec![Scene { number: 1, ..Default::default() }];
            a.cuts = vec![Cut { number: 1, scene: 1, ..Default::default() }];
        }
        merge_board(&mut live, &done, 2);
        // The painted plate keeps its picture; the new plate and effect arrive.
        assert_eq!(live.plates.iter().map(|x| (x.id.as_str(), x.image.is_some())).collect::<Vec<_>>(), vec![("street", true), ("roof", false)]);
        assert_eq!(live.fx.len(), 1);
        let a = &live.chapter(2).unwrap().anime;
        assert_eq!((a.treatment.as_str(), a.scenes.len(), a.cuts.len()), ("Open on the roof.", 1, 1));
    }
}
