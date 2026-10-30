//! The assistant's `video_*` tools: the AI chat edits a manga's recap video the way the Video
//! mode does. Every tool reads and writes the same project file (`vedit::update_project`) and the
//! chapter's caption file, then bumps [`version`]; an open Video mode reloads the chapter on its
//! next poll (keeping the old edit as an undo step), so chat edits show on the timeline live.
//! Render queues the pipeline's Video step; Open and Export hand the Studio page a [`Request`].
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use futures::future::BoxFuture;
use futures::FutureExt;
use mv_recap::Step;
use mv_ui::Route;
use serde_json::{json, Map, Value};
use ui_assistant::ext::ToolProvider;
use ui_assistant::tools::{AppTool, Phrases, Risk, ToolOutput, ToolResult};

use crate::data::{ChapterItem, MangaGroup};
use crate::look::{Effect, Preset};
use crate::vedit::{self, Beat, ChapterEdit, Clip, Layer, LayerKind, LookEdit, Motion, MusicSource, Track};
use crate::video::{self, SubtitleSetting};

// ------------------------------------------------------------------------------------ shared state

static VERSION: AtomicU64 = AtomicU64::new(0);
/// The manga and chapter open in the Video mode (what "this chapter" means in the chat).
static CURRENT: Mutex<Option<(String, String)>> = Mutex::new(None);
static REQUEST: Mutex<Option<Request>> = Mutex::new(None);

/// Bumped by every tool edit.
pub(crate) fn version() -> u64 {
    VERSION.load(Ordering::SeqCst)
}

fn bump() {
    VERSION.fetch_add(1, Ordering::SeqCst);
}

pub(crate) fn set_current(manga: &str, chapter: &str) {
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some((manga.to_string(), chapter.to_string()));
}

fn current() -> Option<(String, String)> {
    CURRENT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// What the chat asks the Studio page to do (it needs the page: a sequence to open, a folder to pick).
#[derive(Clone, Debug)]
pub(crate) enum Request {
    Open { manga: String, chapter: String },
    Export { manga: String, prefs: crate::export::ExportPrefs },
}

pub(crate) fn take_request() -> Option<Request> {
    REQUEST.lock().unwrap_or_else(|e| e.into_inner()).take()
}

fn request(r: Request) {
    *REQUEST.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
}

/// Registers the tools with the assistant (once).
pub(crate) fn register() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| ui_assistant::ext::register("studio-video", Arc::new(VideoTools)));
}

// ------------------------------------------------------------------------------------ rendering

fn manga_dir(c: &ChapterItem) -> PathBuf {
    c.output_dir.parent().map(Path::to_path_buf).unwrap_or_else(|| c.output_dir.clone())
}

/// Queues the Video step of `chapters` (each in the format it was made in) after marking their
/// edits as rendered. Returns how many were queued and the saved project.
pub(crate) fn render(chapters: &[ChapterItem]) -> Result<(usize, vedit::Project), String> {
    let first = chapters.first().ok_or("No chapter to render")?;
    let keys: Vec<(String, u64)> = chapters
        .iter()
        .map(|c| {
            let key = vedit::extra_for(&c.output_dir, &c.id, &c.pages_dir, &video::subtitle_setting_for(c)).map(|x| vedit::render_key(&x)).unwrap_or(0);
            (c.id.clone(), key)
        })
        .collect();
    let project = vedit::update_project(&manga_dir(first), |p| {
        for (id, key) in &keys {
            p.chapters.entry(id.clone()).or_default().rendered = Some(*key);
        }
    })
    .map_err(|e| format!("Could not save the edit: {e}"))?;
    let (reels, land): (Vec<ChapterItem>, Vec<ChapterItem>) = chapters.iter().cloned().partition(|c| !mv_recap::chapter_status(&crate::page::chref(c)).reels.is_empty());
    let mut n = 0;
    for (list, format) in [(land, mv_recap::FORMAT_LANDSCAPE), (reels, mv_recap::FORMAT_REELS)] {
        if !list.is_empty() {
            let (a, w, k) = crate::pipeline::enqueue(&list, Step::Video, Some(Step::Video), format, None, true);
            n += a + w + k;
        }
    }
    bump();
    Ok((n, project))
}

// ------------------------------------------------------------------------------------ arguments

fn s<'a>(args: &'a Map<String, Value>, k: &str) -> &'a str {
    args.get(k).and_then(Value::as_str).map(str::trim).unwrap_or("")
}

fn num(args: &Map<String, Value>, k: &str) -> Option<f64> {
    match args.get(k)? {
        Value::Number(n) => n.as_f64(),
        Value::String(t) => t.trim().trim_end_matches(['%', 's']).trim().parse().ok(),
        _ => None,
    }
}

fn flag(args: &Map<String, Value>, k: &str) -> Option<bool> {
    match args.get(k)? {
        Value::Bool(b) => Some(*b),
        Value::String(t) => match t.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" => Some(true),
            "false" | "no" | "off" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// "3", "3,5", "7-9" → 0-based clip indexes (the chat counts from 1).
pub(crate) fn parse_clips(text: &str) -> Result<Vec<usize>, String> {
    let mut out = vec![];
    for part in text.split([',', ' ']).map(str::trim).filter(|p| !p.is_empty()) {
        let one = |t: &str| t.trim().parse::<usize>().ok().filter(|n| *n >= 1).map(|n| n - 1).ok_or_else(|| format!("\"{t}\" is not a clip number"));
        match part.split_once('-') {
            Some((a, b)) => {
                let (a, b) = (one(a)?, one(b)?);
                out.extend(a.min(b)..=a.max(b));
            }
            None => out.push(one(part)?),
        }
    }
    out.sort_unstable();
    out.dedup();
    if out.is_empty() {
        return Err("Give clip numbers, e.g. \"3\" or \"3,5,7-9\"".into());
    }
    Ok(out)
}

fn track_of(name: &str) -> Option<Track> {
    match name.to_ascii_lowercase().as_str() {
        "captions" | "subtitles" | "v3" => Some(Track::Captions),
        "layers" | "overlays" | "v2" => Some(Track::Layers),
        "panels" | "clips" | "video" | "v1" => Some(Track::Clips),
        "narration" | "voice" | "a1" => Some(Track::Narration),
        "music" | "a2" => Some(Track::Music),
        _ => None,
    }
}

fn secs(ms: u64) -> f64 {
    (ms as f64 / 100.).round() / 10.
}

fn ms(v: f64) -> u64 {
    (v.max(0.) * 1000.).round() as u64
}

// ------------------------------------------------------------------------------------ resolving

fn made(c: &ChapterItem) -> bool {
    c.video.as_ref().is_some_and(|p| p.is_file())
}

fn load() -> Result<Vec<MangaGroup>, String> {
    crate::data::load().map_err(|e| format!("Could not read the library: {e:#}"))
}

/// The manga named in `manga` (a title), else the one open in Video, else the first with a video.
pub(crate) fn pick_manga(groups: &[MangaGroup], manga: &str, current: Option<&str>) -> Result<MangaGroup, String> {
    let q = manga.to_lowercase();
    let hit = if !q.is_empty() {
        groups.iter().find(|g| g.title.to_lowercase() == q || g.id == manga).or_else(|| groups.iter().find(|g| g.title.to_lowercase().contains(&q)))
    } else {
        current.and_then(|id| groups.iter().find(|g| g.id == id)).or_else(|| groups.iter().find(|g| g.chapters.iter().any(made)))
    };
    let g = hit.cloned().ok_or_else(|| if q.is_empty() { "No manga has a finished chapter video yet.".to_string() } else { format!("No downloaded manga matches \"{manga}\".") })?;
    if !g.chapters.iter().any(made) {
        return Err(format!("{} has no chapter video yet. Make one with Generate on the Studio dashboard first.", g.title));
    }
    Ok(g)
}

/// The finished chapters named in `chapter`: a number ("12"), an id, part of a label, "all", or
/// empty for the one open in Video (else the first finished one).
pub(crate) fn pick_chapters(g: &MangaGroup, chapter: &str, current: Option<&str>) -> Result<Vec<ChapterItem>, String> {
    let done: Vec<ChapterItem> = g.chapters.iter().filter(|c| made(c)).cloned().collect();
    let q = chapter.trim().to_lowercase();
    let q = q.strip_prefix("chapter").or_else(|| q.strip_prefix("ch.")).or_else(|| q.strip_prefix("ch")).unwrap_or(&q).trim().to_string();
    if q == "all" || q == "every" {
        return Ok(done);
    }
    if q.is_empty() {
        return current.and_then(|id| done.iter().find(|c| c.id == id)).or(done.first()).cloned().map(|c| vec![c]).ok_or_else(|| "No finished chapter".into());
    }
    let hit = match q.parse::<f64>() {
        Ok(n) => g.chapters.iter().find(|c| c.number == Some(n)),
        Err(_) => g.chapters.iter().find(|c| c.id == chapter.trim()).or_else(|| g.chapters.iter().find(|c| c.label().to_lowercase().contains(&q))),
    };
    let c = hit.ok_or_else(|| format!("{} has no chapter \"{chapter}\".", g.title))?;
    if !made(c) {
        return Err(format!("{} has no video yet. Make it with Generate first.", c.label()));
    }
    Ok(vec![c.clone()])
}

fn resolve(args: &Map<String, Value>) -> Result<(MangaGroup, Vec<ChapterItem>), String> {
    let groups = load()?;
    let cur = current();
    let g = pick_manga(&groups, s(args, "manga"), cur.as_ref().map(|c| c.0.as_str()))?;
    let chapters = pick_chapters(&g, s(args, "chapter"), cur.as_ref().filter(|c| c.0 == g.id).map(|c| c.1.as_str()))?;
    Ok((g, chapters))
}

fn one(args: &Map<String, Value>) -> Result<(MangaGroup, ChapterItem), String> {
    let (g, mut list) = resolve(args)?;
    if list.len() != 1 {
        return Err("This works on one chapter at a time: name the chapter.".into());
    }
    Ok((g, list.remove(0)))
}

fn beats_of(c: &ChapterItem) -> Vec<Beat> {
    vedit::load_beats(&c.output_dir, &mv_recap::list_pages(&c.pages_dir))
}

/// Applies `f` to each chapter's edit and saves; the open timeline picks it up.
fn edit_chapters(chapters: &[ChapterItem], mut f: impl FnMut(&ChapterItem, &mut ChapterEdit) -> Result<(), String>) -> Result<(), String> {
    let Some(first) = chapters.first() else { return Ok(()) };
    let mut err = Ok(());
    vedit::update_project(&manga_dir(first), |p| {
        for c in chapters {
            let e = p.chapters.entry(c.id.clone()).or_default();
            if let Err(x) = f(c, e) {
                err = Err(x);
                return;
            }
        }
    })
    .map_err(|e| format!("Could not save the edit: {e}"))?;
    err?;
    bump();
    Ok(())
}

// ------------------------------------------------------------------------------------ reading

fn look_of(e: &ChapterEdit) -> Value {
    match &e.look {
        Some(l) => json!({ "preset": l.preset, "effects": l.effects, "for": "this chapter" }),
        None => {
            let g = crate::look::Look::load();
            json!({ "preset": g.preset.key(), "for": "Studio-wide look" })
        }
    }
}

fn music_of(e: &ChapterEdit) -> Value {
    let src = match &e.music.source {
        MusicSource::Score => json!("the chapter's score"),
        MusicSource::File { path } => json!(path.file_name().map(|n| n.to_string_lossy().into_owned())),
        MusicSource::None => json!("none"),
    };
    json!({ "source": src, "volumePercent": (e.music.volume * 100.).round(), "muted": e.flags(Track::Music).off })
}

fn captions_of(sub: &SubtitleSetting, e: &ChapterEdit) -> Value {
    let st = video::find_style(&sub.style);
    let y = sub.pos.map(|p| p.1).unwrap_or_else(|| sub.effective_style().pos.1);
    json!({
        "on": sub.enabled && !e.flags(Track::Captions).off, "style": st.id, "styleName": st.label,
        "place": crate::restyle::PLACES[crate::restyle::place_index(sub.pos)].0, "heightPercent": y.round(),
        "size": crate::restyle::SIZES[crate::restyle::size_index(sub.size)].0, "highlightSpokenWord": sub.emphasis,
    })
}

fn layer_json(l: &Layer) -> Value {
    let mut v = json!({ "id": l.id, "start": secs(l.start_ms), "end": secs(l.end_ms), "x": l.x, "y": l.y, "opacityPercent": (l.opacity * 100.).round() });
    match &l.kind {
        LayerKind::Image { path } => {
            v["kind"] = json!("image");
            v["file"] = json!(path.file_name().map(|n| n.to_string_lossy().into_owned()));
            v["widthPercent"] = json!((l.scale * 100.).round());
        }
        LayerKind::Text { text, color, boxed } => {
            v["kind"] = json!("text");
            v["text"] = json!(text);
            v["color"] = json!(color);
            v["box"] = json!(boxed);
            v["heightPercent"] = json!((l.scale * 1000.).round() / 10.);
        }
    }
    v
}

fn overview(args: &Map<String, Value>) -> Result<Value, String> {
    let groups = load()?;
    let cur = current();
    let g = pick_manga(&groups, s(args, "manga"), cur.as_ref().map(|c| c.0.as_str()))?;
    let project = g.chapters.first().map(|c| vedit::Project::load(&manga_dir(c))).unwrap_or_default();
    let chapters: Vec<Value> = g
        .chapters
        .iter()
        .filter(|c| made(c))
        .map(|c| {
            let e = project.chapters.get(&c.id).cloned().unwrap_or_default();
            let edited = (ChapterEdit { rendered: None, ..e.clone() }) != ChapterEdit::default();
            json!({
                "chapter": c.number, "label": c.label(), "id": c.id,
                "seconds": c.video.as_ref().and_then(|p| crate::export::video_seconds(p)).map(|s| s.round()),
                "edited": edited, "layers": e.layers.len(), "music": music_of(&e)["source"],
            })
        })
        .collect();
    let open = cur.filter(|c| c.0 == g.id).and_then(|c| g.chapters.iter().find(|x| x.id == c.1).map(|x| x.label()));
    Ok(json!({ "manga": g.title, "openInVideoEditor": open, "finishedChapters": chapters, "notFinished": g.chapters.iter().filter(|c| !made(c)).count() }))
}

fn timeline(args: &Map<String, Value>) -> Result<Value, String> {
    let (g, c) = one(args)?;
    let beats = beats_of(&c);
    let e = vedit::chapter_edit(&c.output_dir, &c.id).unwrap_or_default();
    let placed = vedit::place(&beats, &e.clips(&beats));
    let from = num(args, "from_clip").map(|n| (n.max(1.) as usize) - 1).unwrap_or(0);
    let limit = num(args, "limit").map(|n| n.clamp(1., 80.) as usize).unwrap_or(40);
    let clips: Vec<Value> = placed
        .iter()
        .skip(from)
        .take(limit)
        .map(|p| {
            let b = beats.get(p.clip.src);
            let text: String = p.clip.text.clone().or_else(|| b.map(|b| b.text.clone())).unwrap_or_default().chars().take(90).collect();
            json!({
                "clip": p.index + 1, "page": b.map(|b| b.page + 1), "start": secs(p.start_ms), "seconds": secs(p.dur_ms),
                "motion": p.clip.motion.label(), "hardCut": p.clip.cut, "trimmed": p.clip.trim_in_ms + p.clip.trim_out_ms > 0, "caption": text,
            })
        })
        .collect();
    let tracks: Map<String, Value> = Track::ALL.iter().map(|t| (t.label().to_lowercase(), json!({ "off": e.flags(*t).off, "locked": e.flags(*t).locked }))).collect();
    Ok(json!({
        "manga": g.title, "chapter": c.label(), "seconds": secs(vedit::total_ms(&placed)), "clipCount": placed.len(),
        "clipsShown": format!("{}-{} of {}", from + 1, (from + clips.len()).max(from + 1), placed.len()),
        "clips": clips, "layers": e.layers.iter().map(layer_json).collect::<Vec<_>>(),
        "captions": captions_of(&vedit::placed_captions(&c.output_dir, &video::subtitle_setting_for(&c)), &e), "music": music_of(&e), "look": look_of(&e),
        "transitions": if e.cuts { "hard cuts" } else { "the look's transitions" }, "tracks": tracks,
        "renderedWithThisEdit": e.rendered.is_some() && e.rendered == vedit::extra_for(&c.output_dir, &c.id, &c.pages_dir, &video::subtitle_setting_for(&c)).map(|x| vedit::render_key(&x)),
    }))
}

fn caption_styles() -> Value {
    Value::Array(video::styles().iter().map(|s| json!({ "style": s.id, "name": s.label, "category": s.category })).collect())
}

// ------------------------------------------------------------------------------------ editing

fn set_captions(args: &Map<String, Value>) -> Result<Value, String> {
    let (_, chapters) = resolve(args)?;
    let style = s(args, "style").to_lowercase();
    let style = if style.is_empty() {
        None
    } else {
        Some(video::styles().iter().find(|st| st.id.to_lowercase() == style || st.label.to_lowercase() == style).map(|st| st.id.clone()).ok_or_else(|| format!("No caption style \"{style}\". video_caption_styles lists them."))?)
    };
    let place = match s(args, "place").to_lowercase().as_str() {
        "" => None,
        "template" | "default" => Some(0),
        "top" => Some(1),
        "middle" | "center" | "centre" => Some(2),
        "bottom" => Some(3),
        other => return Err(format!("Unknown place \"{other}\" (top, middle, bottom or template)")),
    };
    let size = match s(args, "size").to_uppercase().as_str() {
        "" => num(args, "size").map(|v| v as f32),
        label => crate::restyle::SIZES.iter().find(|(l, _)| *l == label).map(|(_, v)| *v).or_else(|| num(args, "size").map(|v| v as f32)),
    };
    let on = flag(args, "on");
    for c in &chapters {
        let mut sub = vedit::placed_captions(&c.output_dir, &video::subtitle_setting_for(c));
        if let Some(id) = &style {
            sub.style = id.clone();
        }
        if let Some(p) = place {
            crate::restyle::set_place(&mut sub, p);
        }
        if let Some(y) = num(args, "height") {
            sub.pos = Some((sub.pos.map(|p| p.0).unwrap_or(50.), (y as f32).clamp(0., 100.)));
        }
        if let Some(v) = size {
            sub.size = v.clamp(0.4, 2.5);
        }
        if let Some(h) = flag(args, "highlight") {
            sub.emphasis = h;
        }
        if on == Some(true) {
            sub.enabled = true;
        }
        video::save_chapter_setting(&c.output_dir, Some(&sub));
        // A place is the manga's: every caption of it moves (the Video editor's project file).
        if place.is_some() || num(args, "height").is_some() {
            if let Some(manga_dir) = c.output_dir.parent() {
                vedit::update_project(manga_dir, |p| p.caption_pos = sub.pos).map_err(|e| format!("Could not save the caption place: {e}"))?;
            }
        }
    }
    if let Some(on) = on {
        edit_chapters(&chapters, |_, e| {
            e.flags_mut(Track::Captions).off = !on;
            Ok(())
        })?;
    }
    bump();
    let first = chapters.first().ok_or("No chapter")?;
    Ok(json!({ "ok": true, "chapters": chapters.len(), "captions": captions_of(&vedit::placed_captions(&first.output_dir, &video::subtitle_setting_for(first)), &vedit::chapter_edit(&first.output_dir, &first.id).unwrap_or_default()) }))
}

fn set_music(args: &Map<String, Value>) -> Result<Value, String> {
    let (_, chapters) = resolve(args)?;
    let file = s(args, "file");
    let source = match (s(args, "source").to_lowercase().as_str(), file.is_empty()) {
        ("", true) => None,
        ("score", _) => Some(MusicSource::Score),
        ("none" | "off" | "delete", _) => Some(MusicSource::None),
        ("file" | "", false) => {
            let src = PathBuf::from(file);
            if !src.is_file() {
                return Err(format!("No music file at {file}"));
            }
            let first = chapters.first().ok_or("No chapter")?;
            Some(MusicSource::File { path: vedit::import_media(&src, &manga_dir(first)).map_err(|e| format!("Could not copy the music: {e}"))? })
        }
        ("file", true) => return Err("Give the music file's path".into()),
        (other, _) => return Err(format!("Unknown music source \"{other}\" (score, file or none)")),
    };
    let volume = num(args, "volume").map(|v| (v as f32 / 100.).clamp(0., 2.));
    let mute = flag(args, "mute");
    edit_chapters(&chapters, |_, e| {
        if let Some(src) = &source {
            e.music.source = src.clone();
            if *src != MusicSource::None {
                e.flags_mut(Track::Music).off = false;
            }
        }
        if let Some(v) = volume {
            e.music.volume = v;
        }
        if let Some(m) = mute {
            e.flags_mut(Track::Music).off = m;
        }
        Ok(())
    })?;
    let e = vedit::chapter_edit(&chapters[0].output_dir, &chapters[0].id).unwrap_or_default();
    Ok(json!({ "ok": true, "chapters": chapters.len(), "music": music_of(&e) }))
}

fn set_track(args: &Map<String, Value>) -> Result<Value, String> {
    let (_, chapters) = resolve(args)?;
    let t = track_of(s(args, "track")).ok_or("track is captions, layers, panels, narration or music")?;
    let (off, locked) = (flag(args, "off"), flag(args, "locked"));
    if t == Track::Clips && off == Some(true) {
        return Err("The panels are the picture and cannot be hidden: delete or trim clips instead.".into());
    }
    edit_chapters(&chapters, |_, e| {
        let f = e.flags_mut(t);
        if let Some(o) = off {
            f.off = o;
        }
        if let Some(l) = locked {
            f.locked = l;
        }
        Ok(())
    })?;
    Ok(json!({ "ok": true, "track": t.label(), "chapters": chapters.len() }))
}

fn not_locked(e: &ChapterEdit, t: Track) -> Result<(), String> {
    if e.flags(t).locked {
        Err(format!("The {} track is locked. Unlock it first (video_set_track).", t.label().to_lowercase()))
    } else {
        Ok(())
    }
}

fn apply_layer_fields(l: &mut Layer, args: &Map<String, Value>, total: u64) -> Result<(), String> {
    if let Some(v) = num(args, "start") {
        l.start_ms = ms(v).min(total.saturating_sub(vedit::MIN_MS));
    }
    if let Some(v) = num(args, "end") {
        l.end_ms = ms(v).min(total);
    }
    if l.end_ms < l.start_ms + vedit::MIN_MS {
        l.end_ms = (l.start_ms + vedit::MIN_MS).min(total.max(l.start_ms + vedit::MIN_MS));
    }
    if let Some(v) = num(args, "x") {
        l.x = (v as f32).clamp(0., 100.);
    }
    if let Some(v) = num(args, "y") {
        l.y = (v as f32).clamp(0., 100.);
    }
    if let Some(v) = num(args, "opacity") {
        l.opacity = (v as f32 / 100.).clamp(0., 1.);
    }
    if let Some(v) = num(args, "size") {
        l.scale = match l.kind {
            LayerKind::Image { .. } => (v as f32 / 100.).clamp(0.02, 1.),
            LayerKind::Text { .. } => (v as f32 / 100.).clamp(0.01, 0.2),
        };
    }
    if let LayerKind::Text { text, color, boxed } = &mut l.kind {
        if !s(args, "text").is_empty() {
            *text = s(args, "text").to_string();
        }
        let c = s(args, "color");
        if !c.is_empty() {
            if video::parse_color(c).is_none() {
                return Err(format!("\"{c}\" is not a colour (use #rrggbb)"));
            }
            *color = c.to_string();
        }
        if let Some(b) = flag(args, "box") {
            *boxed = b;
        }
    }
    Ok(())
}

fn add_layer(args: &Map<String, Value>) -> Result<Value, String> {
    let (_, c) = one(args)?;
    let total = vedit::total_ms(&vedit::place(&beats_of(&c), &vedit::chapter_edit(&c.output_dir, &c.id).unwrap_or_default().clips(&beats_of(&c))));
    let kind = match s(args, "kind").to_lowercase().as_str() {
        "text" | "" if !s(args, "text").is_empty() => LayerKind::Text { text: String::new(), color: "#ffffff".into(), boxed: false },
        "text" => return Err("A text layer needs text".into()),
        "logo" => LayerKind::Image { path: crate::look::Look::load().logo.path.filter(|p| p.is_file()).ok_or("No logo yet: add one in Studio → Video → Look & branding, or give an image file")? },
        "image" | "" => {
            let src = PathBuf::from(s(args, "file"));
            if !src.is_file() {
                return Err(format!("No image at \"{}\"", s(args, "file")));
            }
            LayerKind::Image { path: vedit::import_media(&src, &manga_dir(&c)).map_err(|e| format!("Could not copy the image: {e}"))? }
        }
        other => return Err(format!("Unknown layer kind \"{other}\" (text, image or logo)")),
    };
    let logo = s(args, "kind").eq_ignore_ascii_case("logo");
    let mut l = match &kind {
        LayerKind::Text { .. } => Layer { id: 0, kind, start_ms: 0, end_ms: 4_000, x: 50., y: 22., scale: 0.06, opacity: 1. },
        LayerKind::Image { .. } if logo => Layer { id: 0, kind, start_ms: 0, end_ms: total, x: 90., y: 12., scale: 0.12, opacity: 0.85 },
        LayerKind::Image { .. } => Layer { id: 0, kind, start_ms: 0, end_ms: 5_000.min(total), x: 50., y: 50., scale: 0.3, opacity: 1. },
    };
    let dur = l.end_ms - l.start_ms;
    if let Some(start) = num(args, "start") {
        l.start_ms = ms(start);
        l.end_ms = l.start_ms + dur;
    }
    apply_layer_fields(&mut l, args, total)?;
    let mut out = json!(null);
    edit_chapters(std::slice::from_ref(&c), |_, e| {
        not_locked(e, Track::Layers)?;
        l.id = e.next_layer_id();
        e.flags_mut(Track::Layers).off = false;
        out = layer_json(&l);
        e.layers.push(l.clone());
        Ok(())
    })?;
    Ok(json!({ "ok": true, "chapter": c.label(), "layer": out }))
}

fn edit_layer(args: &Map<String, Value>) -> Result<Value, String> {
    let (_, c) = one(args)?;
    let id = num(args, "id").ok_or("id is required (video_timeline lists layers)")? as u64;
    let beats = beats_of(&c);
    let mut out = json!(null);
    edit_chapters(std::slice::from_ref(&c), |_, e| {
        not_locked(e, Track::Layers)?;
        let total = vedit::total_ms(&vedit::place(&beats, &e.clips(&beats)));
        let l = e.layers.iter_mut().find(|l| l.id == id).ok_or(format!("No layer {id}"))?;
        apply_layer_fields(l, args, total)?;
        out = layer_json(l);
        Ok(())
    })?;
    Ok(json!({ "ok": true, "layer": out }))
}

fn remove_layer(args: &Map<String, Value>) -> Result<Value, String> {
    let (_, c) = one(args)?;
    let which = s(args, "id").to_lowercase();
    let id = num(args, "id").map(|n| n as u64);
    let mut n = 0;
    edit_chapters(std::slice::from_ref(&c), |_, e| {
        not_locked(e, Track::Layers)?;
        let before = e.layers.len();
        match (which.as_str(), id) {
            ("all", _) => e.layers.clear(),
            (_, Some(id)) => e.layers.retain(|l| l.id != id),
            _ => return Err("id is a layer id or \"all\"".into()),
        }
        n = before - e.layers.len();
        if n == 0 {
            return Err("No such layer".into());
        }
        Ok(())
    })?;
    Ok(json!({ "ok": true, "removed": n }))
}

fn set_look(args: &Map<String, Value>) -> Result<Value, String> {
    let (_, chapters) = resolve(args)?;
    let preset = s(args, "preset").to_lowercase();
    let studio = preset == "studio";
    let p = match preset.as_str() {
        "" | "studio" => None,
        "recap duo" | "duo" => Some(Preset::Recap),
        other => Some(Preset::parse(other).ok_or_else(|| format!("Unknown look \"{other}\" (clean, cinematic, recap, dynamic or studio)"))?),
    };
    let cuts = match s(args, "transitions").to_lowercase().as_str() {
        "" => None,
        "cut" | "cuts" | "hard cuts" | "none" => Some(true),
        "look" | "transitions" | "fade" | "default" => Some(false),
        other => return Err(format!("transitions is \"cuts\" or \"look\", not \"{other}\"")),
    };
    edit_chapters(&chapters, |_, e| {
        if studio {
            e.look = None;
        }
        if let Some(p) = p {
            e.look = Some(LookEdit { preset: p.key().into(), effects: Default::default() });
        }
        for fx in Effect::ALL {
            if let Some(on) = flag(args, fx.key()) {
                let mut l = e.look.clone().unwrap_or_else(|| LookEdit { preset: crate::look::Look::load().preset.key().into(), effects: Default::default() });
                let base = Preset::parse(&l.preset).unwrap_or_default().effects().get(fx);
                if on == base {
                    l.effects.remove(fx.key());
                } else {
                    l.effects.insert(fx.key().into(), on);
                }
                e.look = Some(l);
            }
        }
        if let Some(c) = cuts {
            e.cuts = c;
        }
        Ok(())
    })?;
    let e = vedit::chapter_edit(&chapters[0].output_dir, &chapters[0].id).unwrap_or_default();
    Ok(json!({ "ok": true, "chapters": chapters.len(), "look": look_of(&e), "transitions": if e.cuts { "hard cuts" } else { "the look's transitions" } }))
}

/// Applies `f` to the chapter's clip list.
fn edit_clips(c: &ChapterItem, f: impl FnOnce(&[Beat], &mut Vec<Clip>) -> Result<Value, String>) -> Result<Value, String> {
    let beats = beats_of(c);
    let mut out = Ok(json!(null));
    let mut f = Some(f);
    edit_chapters(std::slice::from_ref(c), |_, e| {
        not_locked(e, Track::Clips)?;
        let mut clips = e.clips(&beats);
        let f = f.take().ok_or("internal")?;
        out = f(&beats, &mut clips);
        if out.is_ok() {
            e.set_clips(&beats, clips);
        }
        out.clone().map(|_| ())
    })?;
    out
}

fn clip_index(args: &Map<String, Value>, k: &str, len: usize) -> Result<usize, String> {
    let n = num(args, k).ok_or(format!("{k} is required (a clip number from video_timeline)"))? as usize;
    if n == 0 || n > len {
        return Err(format!("There is no clip {n} (the chapter has {len})"));
    }
    Ok(n - 1)
}

fn edit_clip(args: &Map<String, Value>) -> Result<Value, String> {
    let (_, c) = one(args)?;
    edit_clips(&c, |beats, clips| {
        let i = clip_index(args, "clip", clips.len())?;
        let cl = &mut clips[i];
        let motion = s(args, "motion").to_lowercase();
        if !motion.is_empty() {
            cl.motion = match motion.as_str() {
                "auto" => Motion::Auto,
                "still" | "none" => Motion::Still,
                "zoom" | "zoom in" | "push" => Motion::Zoom,
                "shake" | "punch" | "punch + shake" => Motion::Shake,
                other => return Err(format!("Unknown motion \"{other}\" (auto, still, zoom or shake)")),
            };
        }
        if let Some(t) = args.get("caption").and_then(Value::as_str) {
            let own = beats.get(cl.src).map(|b| b.text.as_str()) == Some(t);
            cl.text = (!own && !t.trim().is_empty()).then(|| t.to_string());
        }
        if let Some(b) = flag(args, "hard_cut") {
            cl.cut = b;
        }
        let beat_ms = beats.get(cl.src).map(|b| b.dur_ms).unwrap_or(0);
        if let Some(v) = num(args, "trim_start") {
            cl.trim_in_ms = ms(v);
        }
        if let Some(v) = num(args, "trim_end") {
            cl.trim_out_ms = ms(v);
        }
        if cl.trim_in_ms + cl.trim_out_ms + vedit::MIN_MS > beat_ms {
            return Err(format!("That trims away the whole clip (it is {:.1} s)", beat_ms as f64 / 1000.));
        }
        Ok(json!({ "ok": true, "clip": i + 1, "seconds": secs(vedit::clip_ms(beats, cl).unwrap_or(0)), "motion": cl.motion.label(), "hardCut": cl.cut, "caption": cl.text }))
    })
}

fn delete_clips(args: &Map<String, Value>) -> Result<Value, String> {
    let (_, c) = one(args)?;
    let which = parse_clips(s(args, "clips"))?;
    edit_clips(&c, |_, clips| {
        if let Some(bad) = which.iter().find(|i| **i >= clips.len()) {
            return Err(format!("There is no clip {} (the chapter has {})", bad + 1, clips.len()));
        }
        if which.len() >= clips.len() {
            return Err("That would delete every clip".into());
        }
        for i in which.iter().rev() {
            vedit::delete(clips, *i);
        }
        Ok(json!({ "ok": true, "deleted": which.len(), "clipsLeft": clips.len() }))
    })
}

fn move_clip(args: &Map<String, Value>) -> Result<Value, String> {
    let (_, c) = one(args)?;
    edit_clips(&c, |_, clips| {
        let from = clip_index(args, "clip", clips.len())?;
        let to = clip_index(args, "to", clips.len())?;
        vedit::reorder(clips, from, to);
        Ok(json!({ "ok": true, "moved": from + 1, "to": to + 1 }))
    })
}

fn split_clip(args: &Map<String, Value>) -> Result<Value, String> {
    let (_, c) = one(args)?;
    edit_clips(&c, |beats, clips| {
        let i = clip_index(args, "clip", clips.len())?;
        let at = ms(num(args, "at").ok_or("at is required (seconds into the clip)")?);
        if !vedit::split(beats, clips, i, at) {
            return Err("Too close to the clip's edge to cut there".into());
        }
        Ok(json!({ "ok": true, "clips": [i + 1, i + 2] }))
    })
}

fn reset(args: &Map<String, Value>) -> Result<Value, String> {
    let (_, chapters) = resolve(args)?;
    edit_chapters(&chapters, |_, e| {
        *e = ChapterEdit { rendered: e.rendered, ..ChapterEdit::default() };
        Ok(())
    })?;
    Ok(json!({ "ok": true, "chapters": chapters.len(), "note": "Back to the chapter as Generate made it. Undo in the Video editor brings the edit back." }))
}

fn render_tool(args: &Map<String, Value>) -> Result<Value, String> {
    if !crate::pipeline::has_context() {
        return Err("Studio has not checked Python and FFmpeg yet in this session: open Studio once, then ask again.".into());
    }
    let groups = load()?;
    let cur = current();
    let g = pick_manga(&groups, s(args, "manga"), cur.as_ref().map(|c| c.0.as_str()))?;
    let chapters = if s(args, "chapter").eq_ignore_ascii_case("edited") {
        let project = g.chapters.first().map(|c| vedit::Project::load(&manga_dir(c))).unwrap_or_default();
        let list: Vec<ChapterItem> = g
            .chapters
            .iter()
            .filter(|c| made(c))
            .filter(|c| {
                project.chapters.get(&c.id).is_some_and(|e| {
                    let key = vedit::extra_for(&c.output_dir, &c.id, &c.pages_dir, &video::subtitle_setting_for(c)).map(|x| vedit::render_key(&x));
                    e.rendered != key && (e.rendered.is_some() || (ChapterEdit { rendered: None, ..e.clone() }) != ChapterEdit::default())
                })
            })
            .cloned()
            .collect();
        if list.is_empty() {
            return Ok(json!({ "ok": true, "queued": 0, "note": "Every edited chapter is already rendered." }));
        }
        list
    } else {
        pick_chapters(&g, s(args, "chapter"), cur.as_ref().filter(|c| c.0 == g.id).map(|c| c.1.as_str()))?
    };
    let (n, _) = render(&chapters)?;
    Ok(json!({ "ok": true, "queued": n, "chapters": chapters.iter().map(ChapterItem::label).collect::<Vec<_>>(), "note": "Rendering in Studio's queue (Activity panel); each takes a few minutes." }))
}

fn export_tool(args: &Map<String, Value>) -> Result<(Route, Value), String> {
    let groups = load()?;
    let cur = current();
    let g = pick_manga(&groups, s(args, "manga"), cur.as_ref().map(|c| c.0.as_str()))?;
    let mut prefs = crate::export::ExportPrefs::default();
    prefs.kind = match s(args, "kind").to_lowercase().as_str() {
        "" | "whole" | "one" | "one video" => 0,
        "parts" => 1,
        "reels" | "shorts" => 2,
        other => return Err(format!("kind is whole, parts or reels, not \"{other}\"")),
    };
    if let Some(p) = num(args, "parts") {
        prefs.parts = crate::export::PART_COUNTS.iter().copied().min_by_key(|c| (*c as f64 - p).abs() as u64).unwrap_or(prefs.parts);
    }
    if let Some(m) = flag(args, "music") {
        prefs.music = m;
    }
    if let Some(h) = flag(args, "hook") {
        prefs.hook = h && prefs.kind == 0;
    }
    if let Some(sec) = num(args, "hook_seconds") {
        prefs.hook_secs = crate::export::HOOK_SECONDS.iter().copied().min_by_key(|c| (*c as f64 - sec).abs() as u64).unwrap_or(prefs.hook_secs);
    }
    request(Request::Export { manga: g.id.clone(), prefs });
    Ok((
        Route::Studio,
        json!({ "ok": true, "manga": g.title, "note": "Opened Studio → Video → Export with these choices. The user picks the folder and presses Export (it uses the rendered chapter videos; render edited chapters first)." }),
    ))
}

fn open_tool(args: &Map<String, Value>) -> Result<(Route, Value), String> {
    let (g, chapters) = resolve(args)?;
    let c = chapters.first().ok_or("No chapter")?;
    request(Request::Open { manga: g.id.clone(), chapter: c.id.clone() });
    Ok((Route::Studio, json!({ "ok": true, "opened": format!("{} · {}", g.title, c.label()) })))
}

// ------------------------------------------------------------------------------------ the table

fn obj(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

fn st(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn nu(description: &str) -> Value {
    json!({ "type": "number", "description": description })
}

fn bo(description: &str) -> Value {
    json!({ "type": "boolean", "description": description })
}

fn en(description: &str, values: &[&str]) -> Value {
    json!({ "type": "string", "description": description, "enum": values })
}

/// The `manga` and `chapter` arguments every chapter tool takes, plus `extra`.
fn with_target(extra: Value, all: bool) -> Value {
    let mut p = json!({
        "manga": st("Manga title (default: the one open in the Video editor)"),
        "chapter": st(if all { "Chapter number, or \"all\" (default: the chapter open in the Video editor)" } else { "Chapter number (default: the chapter open in the Video editor)" }),
    });
    if let (Some(p), Value::Object(e)) = (p.as_object_mut(), extra) {
        p.extend(e);
    }
    p
}

pub(crate) fn tools() -> Vec<AppTool> {
    let t = |name, description: &str, parameters, risk| AppTool { name, description: description.into(), parameters, risk };
    let fx: Map<String, Value> = Effect::ALL.iter().map(|e| (e.key().to_string(), bo(e.label()))).collect();
    let mut look = with_target(
        json!({
            "preset": en("Look (studio = use the Studio-wide look)", &["clean", "cinematic", "recap", "dynamic", "studio"]),
            "transitions": en("Between clips", &["look", "cuts"]),
        }),
        true,
    );
    if let Some(o) = look.as_object_mut() {
        o.extend(fx);
    }
    let layer_fields = json!({
        "start": nu("Start, seconds into the chapter"), "end": nu("End, seconds"),
        "x": nu("Centre across, % of the frame (0 left, 100 right)"), "y": nu("Centre down, % of the frame (0 top, 100 bottom)"),
        "size": nu("Image: width in % of the frame. Text: height in % of the frame (e.g. 6)"),
        "opacity": nu("Opacity %"), "text": st("Text of a text layer"), "color": st("Text colour #rrggbb"), "box": bo("Dark box behind the text"),
    });
    let mut add_layer = with_target(json!({ "kind": en("Layer kind (logo = the Look & branding logo over the whole chapter)", &["text", "image", "logo"]), "file": st("Image file path (kind image)") }), false);
    let mut edit_layer = with_target(json!({ "id": nu("Layer id from video_timeline") }), false);
    if let (Some(a), Some(e), Value::Object(f)) = (add_layer.as_object_mut(), edit_layer.as_object_mut(), layer_fields) {
        a.extend(f.clone());
        e.extend(f);
    }
    vec![
        t("video_overview", "The manga's finished chapter videos for the Video editor: chapter numbers, lengths, which are edited, and which is open. Start here.", obj(json!({ "manga": st("Manga title (default: the one open in the Video editor)") }), &[]), Risk::Read),
        t(
            "video_timeline",
            "One chapter's timeline: clips (numbered from 1, with page, start, length, motion and caption), image/text layers (with ids), captions, music, look and track switches.",
            obj(with_target(json!({ "from_clip": nu("First clip to list (default 1)"), "limit": nu("How many clips (default 40)") }), false), &[]),
            Risk::Read,
        ),
        t("video_caption_styles", "The caption templates (ids and names) video_set_captions takes.", obj(json!({}), &[]), Risk::Read),
        t(
            "video_set_captions",
            "Change the captions: template style, place (top/middle/bottom), exact height, size (S/M/L/XL), highlight of the spoken word, or turn them on/off.",
            obj(with_target(json!({ "style": st("Caption style id or name"), "place": en("Where", &["top", "middle", "bottom", "template"]), "height": nu("Height in the frame, % from the top"), "size": st("S, M, L, XL or a multiplier 0.4-2.5"), "highlight": bo("Highlight the spoken word"), "on": bo("Captions on or off") }), true), &[]),
            Risk::Act,
        ),
        t(
            "video_set_music",
            "Swap, mute or delete the music: the chapter's score, a music file, or none; and its volume.",
            obj(with_target(json!({ "source": en("Music", &["score", "file", "none"]), "file": st("Music file path (source file)"), "volume": nu("Volume % (100 = as made, 0-200)"), "mute": bo("Mute the music track") }), true), &[]),
            Risk::Act,
        ),
        t(
            "video_set_track",
            "Hide/mute (off) or lock a track: captions, layers, panels, narration or music.",
            obj(with_target(json!({ "track": en("Track", &["captions", "layers", "panels", "narration", "music"]), "off": bo("Hide (video) or mute (audio)"), "locked": bo("Lock against edits") }), true), &["track"]),
            Risk::Act,
        ),
        t("video_add_layer", "Add a text, image or logo layer over the picture.", obj(add_layer, &["kind"]), Risk::Act),
        t("video_edit_layer", "Change a layer's text, timing, place, size, colour or opacity.", obj(edit_layer, &["id"]), Risk::Act),
        t("video_remove_layer", "Remove a layer by id, or \"all\" layers.", obj(with_target(json!({ "id": st("Layer id, or \"all\"") }), false), &["id"]), Risk::Destructive),
        t("video_set_look", "Set the look (clean, cinematic, recap, dynamic) and its effects (grade, glow, vignette, grain, impact) for a chapter or all, and the transitions (the look's or hard cuts).", obj(look, &[]), Risk::Act),
        t(
            "video_edit_clip",
            "Change one clip: its motion (auto, still, zoom, shake), caption text (the voice is unchanged), a hard cut into it, or trim seconds off its start/end.",
            obj(with_target(json!({ "clip": nu("Clip number"), "motion": en("Motion", &["auto", "still", "zoom", "shake"]), "caption": st("Caption text (empty = the narration's)"), "hard_cut": bo("Hard cut into the clip"), "trim_start": nu("Seconds trimmed off the start"), "trim_end": nu("Seconds trimmed off the end") }), false), &["clip"]),
            Risk::Act,
        ),
        t("video_delete_clips", "Delete clips (with their narration) from a chapter.", obj(with_target(json!({ "clips": st("Clip numbers, e.g. \"3\" or \"3,5,7-9\"") }), false), &["clips"]), Risk::Destructive),
        t("video_move_clip", "Move a clip to another place in the chapter.", obj(with_target(json!({ "clip": nu("Clip number"), "to": nu("New clip number") }), false), &["clip", "to"]), Risk::Act),
        t("video_split_clip", "Cut a clip in two at a point (seconds into the clip).", obj(with_target(json!({ "clip": nu("Clip number"), "at": nu("Seconds into the clip") }), false), &["clip", "at"]), Risk::Act),
        t("video_reset", "Drop a chapter's edit (back to the video as Generate made it; the caption style stays).", obj(with_target(json!({}), true), &[]), Risk::Destructive),
        t(
            "video_render",
            "Re-make chapter videos with their edits (Studio's queue; minutes each). chapter: a number, \"all\" or \"edited\" (every edited chapter not rendered yet).",
            obj(json!({ "manga": st("Manga title"), "chapter": st("Chapter number, \"all\" or \"edited\"") }), &[]),
            Risk::Act,
        ),
        t(
            "video_export",
            "Open the full recap export for the manga with these choices: one video, parts or 1-minute reels, music, and an AI hook intro. The user picks the folder.",
            obj(json!({ "manga": st("Manga title"), "kind": en("Export as", &["whole", "parts", "reels"]), "parts": nu("Number of parts (2-10)"), "music": bo("Keep the music"), "hook": bo("Add an AI hook intro (one video only)"), "hook_seconds": nu("Hook length: 15, 20 or 30") }), &[]),
            Risk::Act,
        ),
        t("video_open", "Open a chapter in Studio's Video editor (timeline and preview).", obj(with_target(json!({}), false), &[]), Risk::Act),
    ]
}

fn phrase(name: &str, args: &Map<String, Value>) -> Option<Phrases> {
    let ch = s(args, "chapter");
    let of = if ch.is_empty() { String::new() } else if ch.eq_ignore_ascii_case("all") { " of every chapter".into() } else { format!(" of chapter {ch}") };
    let p = |ask: String, doing: String, done: String| Some(Phrases { ask, doing, done });
    let three = |a: &str, d: &str, n: &str, rest: &str| p(format!("{a}{rest}"), format!("{d}{rest}"), format!("{n}{rest}"));
    match name {
        "video_overview" => three("Check", "Checking", "Checked", " the recap videos"),
        "video_timeline" => three("Read", "Reading", "Read", &format!(" the timeline{of}")),
        "video_caption_styles" => three("List", "Listing", "Listed", " the caption styles"),
        "video_set_captions" => three("Change", "Changing", "Changed", &format!(" the captions{of}")),
        "video_set_music" => three("Change", "Changing", "Changed", &format!(" the music{of}")),
        "video_set_track" => three("Switch", "Switching", "Switched", &format!(" the {} track{of}", s(args, "track"))),
        "video_add_layer" => three("Add", "Adding", "Added", &format!(" a {} layer{of}", if s(args, "kind").is_empty() { "new" } else { s(args, "kind") })),
        "video_edit_layer" => three("Change", "Changing", "Changed", &format!(" layer {}{of}", s(args, "id"))),
        "video_remove_layer" => three("Remove", "Removing", "Removed", &format!(" layer {}{of}", s(args, "id"))),
        "video_set_look" => three("Change", "Changing", "Changed", &format!(" the look{of}")),
        "video_edit_clip" => three("Change", "Changing", "Changed", &format!(" clip {}{of}", args.get("clip").map(|v| v.to_string()).unwrap_or_default())),
        "video_delete_clips" => three("Delete", "Deleting", "Deleted", &format!(" clips {}{of}", s(args, "clips"))),
        "video_move_clip" => three("Move", "Moving", "Moved", &format!(" a clip{of}")),
        "video_split_clip" => three("Cut", "Cutting", "Cut", &format!(" a clip in two{of}")),
        "video_reset" => three("Reset", "Resetting", "Reset", &format!(" the edit{of}")),
        "video_render" => three("Render", "Rendering", "Started rendering", &if ch.is_empty() { " the chapter video".to_string() } else { format!(" chapter {ch}") }),
        "video_export" => three("Open", "Opening", "Opened", " the recap export"),
        "video_open" => three("Open", "Opening", "Opened", &format!(" the Video editor{of}")),
        _ => None,
    }
}

struct VideoTools;

impl ToolProvider for VideoTools {
    fn tools(&self) -> Vec<AppTool> {
        tools()
    }

    fn phrases(&self, name: &str, args: &Map<String, Value>) -> Option<Phrases> {
        phrase(name, args)
    }

    fn run(&self, name: &str, args: Map<String, Value>) -> Option<BoxFuture<'static, ToolResult>> {
        if !name.starts_with("video_") || !tools().iter().any(|t| t.name == name) {
            return None;
        }
        let name = name.to_string();
        Some(
            async move {
                // Library reads and file writes: off the async threads.
                tokio::task::spawn_blocking(move || -> ToolResult {
                    let value = |r: Result<Value, String>| r.map(ToolOutput::Value);
                    match name.as_str() {
                        "video_overview" => value(overview(&args)),
                        "video_timeline" => value(timeline(&args)),
                        "video_caption_styles" => Ok(ToolOutput::Value(caption_styles())),
                        "video_set_captions" => value(set_captions(&args)),
                        "video_set_music" => value(set_music(&args)),
                        "video_set_track" => value(set_track(&args)),
                        "video_add_layer" => value(add_layer(&args)),
                        "video_edit_layer" => value(edit_layer(&args)),
                        "video_remove_layer" => value(remove_layer(&args)),
                        "video_set_look" => value(set_look(&args)),
                        "video_edit_clip" => value(edit_clip(&args)),
                        "video_delete_clips" => value(delete_clips(&args)),
                        "video_move_clip" => value(move_clip(&args)),
                        "video_split_clip" => value(split_clip(&args)),
                        "video_reset" => value(reset(&args)),
                        "video_render" => value(render_tool(&args)),
                        "video_export" => export_tool(&args).map(|(r, v)| ToolOutput::Navigate(r, v)),
                        "video_open" => open_tool(&args).map(|(r, v)| ToolOutput::Navigate(r, v)),
                        other => Err(format!("Unknown tool {other}")),
                    }
                })
                .await
                .map_err(|e| e.to_string())?
            }
            .boxed(),
        )
    }

    fn prompt(&self) -> Option<String> {
        Some(
            "Studio's Video editor tools (video_*) edit a manga's finished recap videos: captions, music, image/text layers, the look and effects, clips (trim, reorder, delete, cut, motion), then render and export. \
             Chapters are named by number; without one they act on the chapter open in the Video editor. Read video_overview / video_timeline before editing clips or layers by number. \
             Edits show on the Video editor's timeline at once and the user can undo them there; they reach the video file only after video_render."
                .into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, n: f64, video: bool) -> ChapterItem {
        ChapterItem {
            id: id.into(),
            manga_id: "m".into(),
            manga_title: "Blade".into(),
            number: Some(n),
            title: None,
            pages_dir: PathBuf::new(),
            output_dir: PathBuf::new(),
            page_count: 1,
            has_text: true,
            video: video.then(|| std::env::current_exe().unwrap()),
            audio_files: 1,
        }
    }

    fn group() -> MangaGroup {
        MangaGroup { id: "m".into(), title: "The Blade".into(), cover: None, chapters: vec![item("a", 1., true), item("b", 2., false), item("c", 3., true)] }
    }

    #[test]
    fn clip_lists_parse() {
        assert_eq!(parse_clips("3").unwrap(), vec![2]);
        assert_eq!(parse_clips("3, 5,7-9").unwrap(), vec![2, 4, 6, 7, 8]);
        assert_eq!(parse_clips("9-7").unwrap(), vec![6, 7, 8]);
        assert!(parse_clips("0").is_err());
        assert!(parse_clips("x").is_err());
        assert!(parse_clips("").is_err());
    }

    #[test]
    fn chapters_resolve_by_number_label_all_and_current() {
        let g = group();
        assert_eq!(pick_chapters(&g, "3", None).unwrap()[0].id, "c");
        assert_eq!(pick_chapters(&g, "Chapter 1", None).unwrap()[0].id, "a");
        assert_eq!(pick_chapters(&g, "all", None).unwrap().len(), 2);
        assert_eq!(pick_chapters(&g, "", Some("c")).unwrap()[0].id, "c");
        assert_eq!(pick_chapters(&g, "", None).unwrap()[0].id, "a");
        assert!(pick_chapters(&g, "2", None).unwrap_err().contains("no video"));
        assert!(pick_chapters(&g, "7", None).is_err());
    }

    #[test]
    fn manga_resolves_by_title_or_current() {
        let groups = vec![MangaGroup { id: "x".into(), title: "Other".into(), cover: None, chapters: vec![item("z", 1., false)] }, group()];
        assert_eq!(pick_manga(&groups, "blade", None).unwrap().id, "m");
        assert_eq!(pick_manga(&groups, "", Some("m")).unwrap().id, "m");
        // Without a pick: the first manga that has a video.
        assert_eq!(pick_manga(&groups, "", None).unwrap().id, "m");
        assert!(pick_manga(&groups, "other", None).unwrap_err().contains("no chapter video"));
        assert!(pick_manga(&groups, "nope", None).is_err());
    }

    #[test]
    fn tool_table_is_valid_and_worded() {
        let ts = tools();
        assert!(ts.len() >= 15);
        for t in &ts {
            assert!(t.name.starts_with("video_"));
            assert_eq!(t.parameters["type"], "object");
            for r in t.parameters["required"].as_array().unwrap() {
                assert!(t.parameters["properties"].get(r.as_str().unwrap()).is_some(), "{} requires {r}", t.name);
            }
            assert!(phrase(t.name, &Map::new()).is_some(), "{} has no wording", t.name);
        }
        let mut a = Map::new();
        a.insert("chapter".into(), json!("3"));
        assert_eq!(phrase("video_set_captions", &a).unwrap().ask, "Change the captions of chapter 3");
        assert_eq!(track_of("A2"), Some(Track::Music));
    }

    #[test]
    fn arguments_accept_strings_from_small_models() {
        let mut a = Map::new();
        a.insert("volume".into(), json!("60%"));
        a.insert("mute".into(), json!("yes"));
        a.insert("at".into(), json!("2.5s"));
        assert_eq!(num(&a, "volume"), Some(60.));
        assert_eq!(flag(&a, "mute"), Some(true));
        assert_eq!(num(&a, "at"), Some(2.5));
    }

    #[test]
    fn tool_edits_write_the_shared_project() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("manga").join("ch1");
        std::fs::create_dir_all(&out).unwrap();
        let beat = |p: u64, text: &str| json!({"page": p, "text": text, "audio_file": "page_0000.wav", "start_ms": 0, "end_ms": 2000, "audio_offset_ms": 0, "duration_ms": 2000});
        std::fs::write(out.join("sync_map.json"), json!({"beat_segments": [beat(0, "One."), beat(1, "Two."), beat(2, "Three.")]}).to_string()).unwrap();
        let c = ChapterItem { output_dir: out.clone(), pages_dir: dir.path().join("pages"), ..item("ch1", 1., true) };
        let v0 = version();
        let r = edit_clips(&c, |_, clips| {
            vedit::delete(clips, 0);
            vedit::reorder(clips, 1, 0);
            Ok(json!({"left": clips.len()}))
        })
        .unwrap();
        assert_eq!(r["left"], 2);
        assert!(version() > v0, "the open timeline is told");
        let e = vedit::chapter_edit(&out, "ch1").unwrap();
        assert_eq!(e.clips(&beats_of(&c)).iter().map(|c| c.src).collect::<Vec<_>>(), vec![2, 1]);
        // A failing edit saves nothing.
        assert!(edit_clips(&c, |_, _| Err("no".into())).is_err());
        assert_eq!(vedit::chapter_edit(&out, "ch1").unwrap(), e);
        edit_chapters(std::slice::from_ref(&c), |_, e| {
            e.music.source = MusicSource::None;
            Ok(())
        })
        .unwrap();
        let e = vedit::chapter_edit(&out, "ch1").unwrap();
        assert_eq!(e.music.source, MusicSource::None);
        assert_eq!(e.clips.as_ref().map(Vec::len), Some(2), "other changes stay");
    }

    #[test]
    fn layer_fields_apply_and_clamp() {
        let mut l = Layer { id: 1, kind: LayerKind::Text { text: "a".into(), color: "#ffffff".into(), boxed: false }, start_ms: 0, end_ms: 1000, x: 50., y: 50., scale: 0.06, opacity: 1. };
        let mut a = Map::new();
        a.insert("start".into(), json!(2));
        a.insert("end".into(), json!(1));
        a.insert("x".into(), json!(150));
        a.insert("size".into(), json!(8));
        a.insert("text".into(), json!("Hi"));
        a.insert("color".into(), json!("#ff0000"));
        apply_layer_fields(&mut l, &a, 10_000).unwrap();
        assert_eq!((l.start_ms, l.end_ms), (2000, 2000 + vedit::MIN_MS));
        assert_eq!(l.x, 100.);
        assert!((l.scale - 0.08).abs() < 1e-6);
        assert!(matches!(&l.kind, LayerKind::Text { text, color, .. } if text == "Hi" && color == "#ff0000"));
        a.insert("color".into(), json!("red-ish"));
        assert!(apply_layer_fields(&mut l, &a, 10_000).is_err());
    }
}
