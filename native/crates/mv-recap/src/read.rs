//! READ phase: a vision model reads every page, captures characters into the brain, tags scenes and
//! writes `read.json` after each page (so a crash or cancel never loses work).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use image::{imageops::FilterType, DynamicImage};
use mv_ai::{AiMessage, AiStepRequest, CancellationToken, Registry};
use serde_json::Value;

use crate::brain::Brain;
use crate::json::{bbox_of, num_of, opt_str, str_list, str_of};
use crate::llm::{ask_json, is_cancelled, request, truncate, Llm};
use crate::store::*;
use crate::strip::{self, Unit};
use crate::types::*;

const MAX_FACES_PER_CHARACTER: usize = 4;
const MAX_FACE_ATTACHMENTS: usize = 6;
const FACE_PX: u32 = 192;
const MIN_PANEL_AREA: f32 = 0.02;
const SCENE_KINDS: &[&str] = &["fight", "dialogue", "emotional", "comedy", "action", "reveal", "calm", "flashback", "romance", "horror", "other"];

const READ_SYSTEM: &str = r##"You are an expert manga reader and script supervisor. You look at one manga page (plus reference face crops of known characters) and produce an exact, structured transcript. A narrator will later retell the story from your output as an anime-style recap video, so accuracy and consistent character names matter more than anything else. You never invent dialogue and never skip a speech balloon. Do not run commands, open other files or write files: use only the attached images and answer directly.

Reply with STRICT JSON only: one JSON object, no markdown fences, no commentary before or after it. Schema (every key present; use "" / [] / null when empty):

{
  "summary": "1-2 sentences: what happens on this page, in plain English, using character names",
  "scene": {
    "kind": "fight|dialogue|emotional|comedy|action|reveal|calm|flashback|romance|horror|other",
    "mood": "one or two words, e.g. tense, heartbroken, playful",
    "intensity": 0-10 (0 = quiet, 10 = climax),
    "tags": ["short tags such as 'sword duel', 'rain', 'confession', 'betrayal', 'training'"]
  },
  "memory": "ONE short sentence if a key plot event, reveal, relationship change or world fact happens on this page, else null",
  "panels": [
    {
      "bbox": [x0, y0, x1, y1],
      "description": "what is drawn: setting, action, expressions, camera angle (1-2 sentences)",
      "characters": [
        {
          "name": "Ruko",
          "was": null,
          "face": [x0, y0, x1, y1],
          "description": "only when first seen: age, hair, eyes, outfit, build, role",
          "traits": ["only when first seen or newly revealed: personality words"]
        }
      ],
      "lines": [
        {"speaker": "Ruko", "kind": "speech", "text": "verbatim text of the balloon"}
      ]
    }
  ]
}

Rules
1. READING ORDER. Japanese manga reads right-to-left, top-to-bottom: the top-right panel is first, and within a row go right to left. If the page is a webtoon / vertical strip (the prompt says so) read strictly top-to-bottom. List panels in that order. Balloons inside a panel are also ordered by reading order.
2. COORDINATES. All boxes are NORMALISED to the WHOLE page image: [x0, y0, x1, y1] with values between 0 and 1, origin at the top-left, x to the right, y downward. Panel bbox = the panel frame. Face box = the character's face only, tight (forehead to chin), also relative to the whole page, never to the panel. Omit "face" (null) when the face is hidden, tiny or turned away.
3. LINES. Transcribe EVERY speech balloon, thought bubble, narration box and meaningful sound effect. kind is one of "speech", "thought", "narration", "sfx". Keep the original meaning; if the text is not English, translate it into natural English. Do not add stage directions. Merge a balloon that is split across several bubbles into one line. Narration boxes and sfx have speaker null. If you cannot tell who speaks, use null rather than guessing wildly; use the tail of the balloon, the facing direction and the roster to decide.
4. CHARACTERS. A roster of known characters (the "character bank") is given with reference face crops attached after the page. Identify people by face, hair, eyes and outfit, matching them to the roster, and ALWAYS reuse the roster name exactly. Use a real name when it is written or spoken on the page. If someone is not in the roster and has no name yet, give a stable descriptive placeholder such as "Silver-haired swordsman" or "Girl with red ribbon" and reuse exactly the same placeholder next time you see them. If a character that is listed under a placeholder name is named on this page, return the real name in "name" and the old placeholder in "was" (otherwise "was" is null). Do not list background crowds. Only fill "description" and "traits" for a character who is new or whose details were missing; leave them empty for known characters.
5. SCENE. Judge the depth of the page, not just the surface: fights, tension, grief, comedy beats, reveals, flashbacks. Pick the single best "kind", set intensity honestly (a quiet conversation is 1-3, a decisive blow or tearful goodbye is 8-10), and add 2-5 tags.
6. SUMMARY continues the story: use the previous page's summary for continuity and write what is new here. If the page is only a title, credits or an ad, say so in the summary and return an empty "panels" list.
7. Never wrap the JSON in code fences. Never output anything but the JSON object."##;

/// Pages read at the same time.
const PARALLEL: usize = 3;

// ── parsing ──────────────────────────────────────────────────────────────────────────────────────

pub(crate) struct Extra {
    pub panel: usize,
    pub seen: usize,
    pub was: Option<String>,
    pub description: String,
    pub traits: Vec<String>,
}

pub(crate) struct Parsed {
    pub page: PageRead,
    pub extras: Vec<Extra>,
    pub memory: Option<String>,
}

fn norm_kind(k: &str) -> &'static str {
    match k.trim().to_lowercase().as_str() {
        "thought" | "thinking" | "monologue" | "inner" => "thought",
        "narration" | "narrator" | "caption" | "box" => "narration",
        "sfx" | "sound" | "sound effect" | "onomatopoeia" => "sfx",
        _ => "speech",
    }
}

/// Lenient conversion of the model's JSON into a [`PageRead`]; missing or odd fields never fail.
pub(crate) fn parse_page(v: &Value, index: usize, file: &str) -> Parsed {
    let scene_v = v.get("scene").cloned().unwrap_or(Value::Null);
    let kind = str_of(&scene_v, "kind").to_lowercase();
    let scene = Scene {
        kind: if SCENE_KINDS.contains(&kind.as_str()) { kind } else { "other".into() },
        mood: str_of(&scene_v, "mood"),
        intensity: num_of(&scene_v, "intensity").map(|n| n.round().clamp(0.0, 10.0) as u8).unwrap_or(0),
        tags: str_list(&scene_v, "tags"),
    };
    let mut panels = vec![];
    let mut extras = vec![];
    for pv in v.get("panels").and_then(Value::as_array).into_iter().flatten() {
        let pi = panels.len();
        let mut panel = Panel { bbox: bbox_of(pv, "bbox"), description: str_of(pv, "description"), ..Default::default() };
        for cv in pv.get("characters").and_then(Value::as_array).into_iter().flatten() {
            let name = str_of(cv, "name");
            if name.is_empty() || name.eq_ignore_ascii_case("unknown") {
                continue;
            }
            extras.push(Extra { panel: pi, seen: panel.characters.len(), was: opt_str(cv, "was"), description: str_of(cv, "description"), traits: str_list(cv, "traits") });
            panel.characters.push(CharacterSeen { name, face: bbox_of(cv, "face").or_else(|| bbox_of(cv, "face_bbox")), is_new: false });
        }
        for lv in pv.get("lines").and_then(Value::as_array).into_iter().flatten() {
            let text = str_of(lv, "text");
            if text.is_empty() {
                continue;
            }
            let kind = norm_kind(&str_of(lv, "kind"));
            let speaker = if matches!(kind, "narration" | "sfx") { None } else { opt_str(lv, "speaker") };
            panel.lines.push(Line { speaker, text, kind: kind.into() });
        }
        panels.push(panel);
    }
    Parsed { page: PageRead { index, file: file.into(), summary: str_of(v, "summary"), scene, panels }, extras, memory: opt_str(v, "memory") }
}

// ── brain update ─────────────────────────────────────────────────────────────────────────────────

pub(crate) struct FaceJob {
    pub id: String,
    pub bbox: Bbox,
    pub path: PathBuf,
}

/// Fold one parsed page into the brain: names are canonicalised in place, new characters created,
/// placeholders renamed, and face crops scheduled. Returns `(name, is_new)` events and face jobs.
pub(crate) fn apply_to_brain(brain: &mut Brain, parsed: &mut Parsed, chapter_id: &str, label: &str) -> (Vec<(String, bool)>, Vec<FaceJob>) {
    let page = parsed.page.index;
    let mut events: Vec<(String, bool)> = vec![];
    let mut seen_ids: Vec<String> = vec![];
    let mut jobs: Vec<FaceJob> = vec![];
    for e in &parsed.extras {
        let cs = &mut parsed.page.panels[e.panel].characters[e.seen];
        let (id, created) = brain.observe(&cs.name, e.was.as_deref(), &e.description, &e.traits);
        let name = brain.character(&id).map(|c| c.name.clone()).unwrap_or_else(|| cs.name.clone());
        cs.name = name.clone();
        cs.is_new = created;
        if !seen_ids.contains(&id) {
            seen_ids.push(id.clone());
            brain.mark_seen(&id, chapter_id, page);
            events.push((name, created));
            let have = brain.character(&id).map(|c| c.faces.len()).unwrap_or(0);
            if let (Some(bbox), true) = (cs.face, have < MAX_FACES_PER_CHARACTER) {
                let reserved: Vec<PathBuf> = jobs.iter().map(|j| j.path.clone()).collect();
                let path = brain.next_face_path(&id, &reserved);
                jobs.push(FaceJob { id, bbox, path });
            }
        }
    }
    for panel in &mut parsed.page.panels {
        for l in &mut panel.lines {
            if let Some(s) = &l.speaker {
                l.speaker = Some(brain.canonical(s));
            }
        }
    }
    if let Some(m) = &parsed.memory {
        brain.add_note(&format!("[{label} p{}] {m}", page + 1));
    }
    (events, jobs)
}

// ── image helpers (blocking) ─────────────────────────────────────────────────────────────────────

fn save_jpeg(img: &DynamicImage, out: &Path) -> Result<()> {
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    img.to_rgb8().save_with_format(out, image::ImageFormat::Jpeg).with_context(|| format!("write {}", out.display()))
}

/// Square-ish face thumbnail: bbox + 15% padding, squared up, at most 192px.
pub(crate) fn crop_face(img: &DynamicImage, b: Bbox, out: &Path) -> Result<()> {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let [x0, y0, x1, y1] = b.0;
    let (cx, cy) = ((x0 + x1) / 2.0 * w, (y0 + y1) / 2.0 * h);
    let side = ((x1 - x0) * w * 1.3).max((y1 - y0) * h * 1.3).max(8.0).min(w).min(h);
    let left = (cx - side / 2.0).clamp(0.0, w - side);
    let top = (cy - side / 2.0).clamp(0.0, h - side);
    let mut c = img.crop_imm(left as u32, top as u32, side.max(1.0) as u32, side.max(1.0) as u32);
    if c.width() > FACE_PX {
        c = c.resize(FACE_PX, FACE_PX, FilterType::Triangle);
    }
    save_jpeg(&c, out)
}

/// Full-resolution panel crop with a hair of padding.
pub(crate) fn crop_panel(img: &DynamicImage, b: Bbox, out: &Path) -> Result<()> {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let [x0, y0, x1, y1] = b.0;
    let (px, py) = (((x1 - x0) * 0.01).max(0.0), ((y1 - y0) * 0.01).max(0.0));
    let (l, t) = ((x0 - px).max(0.0) * w, (y0 - py).max(0.0) * h);
    let (r, bt) = ((x1 + px).min(1.0) * w, (y1 + py).min(1.0) * h);
    let c = img.crop_imm(l as u32, t as u32, ((r - l) as u32).max(1), ((bt - t) as u32).max(1));
    save_jpeg(&c, out)
}

/// Page image to send to the model: downscaled copy in `work` when huge, else the original.
/// Returns `(path, width, height)` of the ORIGINAL image dimensions.
pub(crate) fn prep_page(path: &Path, work: &Path, index: usize) -> Result<(PathBuf, u32, u32)> {
    let (w, h) = image::image_dimensions(path).with_context(|| format!("cannot read image {}", path.display()))?;
    let webtoon = is_webtoon(w, h);
    // Claude scales anything past 1568 px down anyway, so larger pages only cost upload time.
    let factor = if webtoon { (1600.0 / w as f32).min(7000.0 / h as f32) } else { 1568.0 / w.max(h) as f32 };
    // Images go inline in the request, which caps their size: re-encode heavy files even when small enough.
    let heavy = std::fs::metadata(path).map(|m| m.len() > 3_000_000).unwrap_or(false);
    if factor >= 1.0 && !heavy {
        return Ok((path.to_path_buf(), w, h));
    }
    let img = image::open(path)?;
    let factor = factor.min(1.0);
    let small = img.resize(((w as f32 * factor) as u32).max(1), ((h as f32 * factor) as u32).max(1), FilterType::Triangle);
    let out = work.join(format!("page_{index:04}.jpg"));
    save_jpeg(&small, &out)?;
    Ok((out, w, h))
}

pub(crate) fn is_webtoon(w: u32, h: u32) -> bool {
    h as f32 / w.max(1) as f32 >= 2.2
}

struct Assets {
    /// The image the boxes refer to: the page, or a stitched strip part.
    image: PathBuf,
    /// Pages whose old panel crops are stale.
    clear: Vec<usize>,
    /// (page, panel index on that page, box on `image`)
    panels: Vec<(usize, usize, Bbox)>,
    faces: Vec<FaceJob>,
    panels_dir: PathBuf,
}

/// Write panel crops (replacing stale ones) and face thumbnails. Returns written faces.
fn write_assets(a: Assets) -> Result<Vec<(String, PathBuf)>> {
    if let Ok(rd) = std::fs::read_dir(&a.panels_dir) {
        let prefixes: Vec<String> = a.clear.iter().map(|p| format!("p{p:04}_")).collect();
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if prefixes.iter().any(|p| name.starts_with(p)) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    if a.panels.is_empty() && a.faces.is_empty() {
        return Ok(vec![]);
    }
    let img = image::open(&a.image).with_context(|| format!("open {}", a.image.display()))?;
    for (page, n, b) in &a.panels {
        if b.area() >= MIN_PANEL_AREA {
            let _ = crop_panel(&img, *b, &a.panels_dir.join(format!("p{page:04}_{n:02}.jpg")));
        }
    }
    let mut out = vec![];
    for j in a.faces {
        if crop_face(&img, j.bbox, &j.path).is_ok() {
            out.push((j.id, j.path));
        }
    }
    Ok(out)
}

// ── prompts ──────────────────────────────────────────────────────────────────────────────────────

/// Known characters with reference faces: recently-seen first, then by appearances. At most 6.
pub(crate) fn pick_faces(brain: &Brain, recent: &[String]) -> Vec<(String, PathBuf)> {
    let mut cs: Vec<_> = brain.characters.iter().filter_map(|c| c.faces.iter().find(|p| p.is_file()).map(|p| (c, p))).collect();
    cs.sort_by_key(|(c, _)| (!recent.iter().any(|r| r.eq_ignore_ascii_case(&c.name)), std::cmp::Reverse(c.appearances)));
    cs.into_iter().take(MAX_FACE_ATTACHMENTS).map(|(c, p)| (c.name.clone(), p.clone())).collect()
}

pub(crate) fn roster_text(brain: &Brain, limit: usize) -> String {
    let mut cs: Vec<_> = brain.characters.iter().collect();
    cs.sort_by_key(|c| std::cmp::Reverse(c.appearances));
    if cs.is_empty() {
        return "(none yet - every character you see is new)".into();
    }
    cs.iter()
        .take(limit)
        .map(|c| {
            let mut s = format!("- {}", c.name);
            if !c.aliases.is_empty() {
                s += &format!(" (also called: {})", c.aliases.join(", "));
            }
            if !c.description.is_empty() {
                s += &format!(": {}", truncate(&c.description, 200));
            }
            if !c.traits.is_empty() {
                s += &format!(" [traits: {}]", c.traits.join(", "));
            }
            s
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) struct PagePrompt<'a> {
    pub brain: &'a Brain,
    pub label: &'a str,
    pub index: usize,
    pub total: usize,
    pub file: &'a str,
    pub size: (u32, u32),
    pub prev_summary: &'a str,
    pub faces: &'a [(String, PathBuf)],
    /// Last page of a stitched strip part that starts at `index` (see `strip.rs`).
    pub strip_to: Option<usize>,
}

pub(crate) fn page_prompt(p: &PagePrompt) -> String {
    let (w, h) = p.size;
    let layout = if p.strip_to.is_some() || is_webtoon(w, h) {
        "WEBTOON / vertical strip: read strictly top-to-bottom, left-to-right."
    } else {
        "Manga page: if this is Japanese manga read right-to-left (top-right panel first); if it is clearly left-to-right (Western comic / manhwa page) read left-to-right."
    };
    let title = if p.brain.manga_title.is_empty() { "(untitled)" } else { &p.brain.manga_title };
    let mut s = match p.strip_to {
        Some(last) => format!(
            "MANGA: {title}\nCHAPTER: {} - part of a vertical strip covering pages {}-{} of {} ({w}x{h} px). The page files were stitched back together and cut only at gaps between panels, so every panel and balloon in this image is complete: never call anything cut off.\nLAYOUT: {layout}\n",
            p.label,
            p.index + 1,
            last + 1,
            p.total,
        ),
        None => format!("MANGA: {title}\nCHAPTER: {} - page {} of {} (file {}, {w}x{h} px)\nLAYOUT: {layout}\n", p.label, p.index + 1, p.total, p.file),
    };
    let notes: String = {
        let n = p.brain.notes.trim();
        let skip = n.chars().count().saturating_sub(1200);
        n.chars().skip(skip).collect()
    };
    if !notes.is_empty() {
        s += &format!("\nSTORY MEMORY SO FAR (most recent last):\n{notes}\n");
    }
    s += &format!("\nPREVIOUS {}: {}\n", if p.strip_to.is_some() { "PART" } else { "PAGE" }, if p.prev_summary.is_empty() { "(this is the first page read)" } else { p.prev_summary });
    s += &format!("\nCHARACTER BANK (reuse these exact names):\n{}\n", roster_text(p.brain, 40));
    s += if p.strip_to.is_some() { "\nATTACHED IMAGES:\nImage 1 = the strip part to read." } else { "\nATTACHED IMAGES:\nImage 1 = the page to read." };
    for (i, (name, path)) in p.faces.iter().enumerate() {
        let f = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        s += &format!("\nImage {} = reference face of {} ({}).", i + 2, name, f);
    }
    s += "\n\nRead Image 1 now and reply with the JSON object only.";
    s
}

fn model_label(m: &ModelChoice) -> String {
    format!("{}/{}", m.provider.as_str(), m.model)
}

fn recent_names(read: &ChapterRead, index: usize) -> Vec<String> {
    let Some(prev) = index.checked_sub(1).and_then(|p| read.pages.iter().find(|x| x.index == p)) else { return vec![] };
    prev.panels.iter().flat_map(|p| p.characters.iter().map(|c| c.name.clone())).collect()
}

// ── main loop ────────────────────────────────────────────────────────────────────────────────────

/// Read a chapter (see module docs). Cancelling returns `Err("cancelled")` after the finished pages are saved.
pub async fn read_chapter(
    reg: Arc<Registry>,
    model: ModelChoice,
    ch: ChapterRef,
    brain: Arc<Mutex<Brain>>,
    opts: ReadOptions,
    progress: ProgressFn,
    cancel: CancellationToken,
) -> Result<ChapterRead> {
    read_chapter_with(&reg, &model, &ch, &brain, &opts, &progress, &cancel).await
}

pub(crate) async fn read_chapter_with(
    llm: &dyn Llm,
    model: &ModelChoice,
    ch: &ChapterRef,
    brain: &Mutex<Brain>,
    opts: &ReadOptions,
    progress: &ProgressFn,
    cancel: &CancellationToken,
) -> Result<ChapterRead> {
    let fail = |msg: String| -> anyhow::Error {
        progress(Progress::Failed(msg.clone()));
        anyhow!(msg)
    };
    let pages = list_pages(&ch.pages_dir);
    if pages.is_empty() {
        return Err(fail(format!("No page images found in {}", ch.pages_dir.display())));
    }
    let mut read = load_read(ch).unwrap_or_default();
    read.chapter_id = ch.chapter_id.clone();
    read.model = model_label(model);
    // A chapter delivered as slices of one long strip is read in whole parts cut at panel gaps
    // (see `strip.rs`), so no panel or balloon is ever split between two reads; ordinary pages go one by one.
    let units = {
        let p = pages.clone();
        tokio::task::spawn_blocking(move || strip::plan(&p)).await.map_err(|e| anyhow!(e))?
    };
    if units.first().is_some_and(|u| u.stitched) {
        progress(Progress::Log(format!("The pages are slices of one long strip: reading it as {} whole parts so nothing is cut off.", units.len())));
    } else {
        read.strip_done.clear();
    }
    let targets: Vec<Unit> = units
        .into_iter()
        .filter(|u| match &opts.pages {
            Some(want) => u.spans.iter().any(|s| want.contains(&s.page)),
            None if opts.force => true,
            None if u.stitched => !read.strip_done.contains(&u.key()),
            None => !read.pages.iter().any(|p| p.index == u.first()),
        })
        .collect();

    let work = chapter_dir(ch).join("work");
    std::fs::create_dir_all(&work).context("create studio work dir")?;
    std::fs::create_dir_all(panels_dir(ch)).context("create panels dir")?;
    if targets.is_empty() {
        progress(Progress::Log("All pages are already read.".into()));
        progress(Progress::Done);
        return Ok(read);
    }
    let total: usize = targets.iter().map(|u| u.spans.len()).sum();
    let mut finished = 0;

    // Parts are read a few at a time: each model call is slow (tens of seconds), and a batch
    // still sees the brain and the summaries of everything read before it.
    // The first part goes alone so the brain has faces before parts are read side by side.
    let (first, rest) = targets.split_at(1);
    for batch in std::iter::once(first).chain(rest.chunks(PARALLEL)) {
        if cancel.is_cancelled() {
            progress(Progress::Failed("Cancelled".into()));
            return Err(anyhow!("cancelled"));
        }
        progress(Progress::Reading { pages: batch.iter().flat_map(Unit::pages).collect(), done: finished, total });

        // 1. Prepare images and prompts.
        let mut jobs: Vec<(&Unit, PathBuf, Option<AiStepRequest>)> = vec![];
        for unit in batch {
            let i = unit.first();
            let file = pages[i].file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
            // Each read gets its own workspace: CLI agents clear their attachments when done.
            let ws = if unit.stitched { work.join(format!("p{i:04}_{}", unit.spans[0].y0)) } else { work.join(format!("p{i:04}")) };
            std::fs::create_dir_all(&ws).context("create page workspace")?;
            let (all, wk, u) = (pages.clone(), ws.clone(), unit.clone());
            let prepped = tokio::task::spawn_blocking(move || -> Result<(PathBuf, PathBuf, u32, u32)> {
                let image = if u.stitched {
                    let out = wk.join("strip.jpg");
                    strip::compose(&u, &all, &out)?;
                    out
                } else {
                    all[i].clone()
                };
                let (send, w, h) = prep_page(&image, &wk, i)?;
                Ok((image, send, w, h))
            })
            .await
            .map_err(|e| anyhow!(e))?;
            let (image, send_path, w, h) = match prepped {
                Ok(x) => x,
                Err(e) => {
                    progress(Progress::Log(format!("Page {}: {e}", i + 1)));
                    for s in unit.spans.iter().filter(|s| s.y0 == 0) {
                        let file = pages[s.page].file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                        upsert(&mut read, PageRead { index: s.page, file, summary: format!("(unreadable image: {e})"), ..Default::default() });
                    }
                    jobs.push((unit, PathBuf::new(), None));
                    continue;
                }
            };
            // A part that starts inside page i continues the reading already stored for page i.
            let upto = if unit.spans[0].y0 > 0 { i + 1 } else { i };
            let prev_summary = (0..upto).rev().find_map(|p| read.pages.iter().find(|x| x.index == p && !x.summary.is_empty())).map(|p| p.summary.clone()).unwrap_or_default();
            let recent = recent_names(&read, i);
            let (prompt, faces) = {
                let b = brain.lock().unwrap();
                let faces = pick_faces(&b, &recent);
                let strip_to = unit.stitched.then(|| unit.spans[unit.spans.len() - 1].page);
                let prompt = page_prompt(&PagePrompt { brain: &b, label: &ch.label, index: i, total: pages.len(), file: &file, size: (w, h), prev_summary: &prev_summary, faces: &faces, strip_to });
                (prompt, faces)
            };
            let mut images = vec![send_path];
            images.extend(faces.iter().map(|(_, p)| p.clone()));
            let req = request(model, vec![AiMessage::system(READ_SYSTEM), AiMessage::user(prompt)], images, Some(ws));
            jobs.push((unit, image, Some(req)));
        }

        // 2. Ask the model about every part of the batch at once.
        let answers = futures::future::join_all(jobs.iter().map(|(_, _, req)| async move {
            match req {
                Some(r) => Some(ask_json(llm, r.clone(), cancel).await),
                None => None,
            }
        }))
        .await;

        // 3. Fold the answers in, in page order.
        for ((unit, image, _), answer) in jobs.into_iter().zip(answers) {
            let i = unit.first();
            let file_of = |p: usize| pages[p].file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
            let Some(answer) = answer else {
                save_read(ch, &read)?;
                for s in &unit.spans {
                    finished += 1;
                    progress(Progress::Page { done: finished, total, page: s.page });
                }
                continue;
            };
            let answer = match answer {
                Ok(a) => a,
                Err(e) if is_cancelled(&e) => {
                    save_read(ch, &read)?;
                    progress(Progress::Failed("Cancelled".into()));
                    return Err(e);
                }
                Err(e) => {
                    save_read(ch, &read)?;
                    return Err(fail(format!("Page {}: {e}", i + 1)));
                }
            };
            let mut parsed = match answer {
                Ok(v) => parse_page(&v, i, &file_of(i)),
                Err(raw) => {
                    progress(Progress::Log(format!("Page {}: the model did not return valid JSON; stored its raw text instead.", i + 1)));
                    Parsed { page: PageRead { index: i, file: file_of(i), summary: truncate(&raw, 600), ..Default::default() }, extras: vec![], memory: None }
                }
            };

            let (events, face_jobs) = {
                let mut b = brain.lock().unwrap();
                apply_to_brain(&mut b, &mut parsed, &ch.chapter_id, &ch.label)
            };
            for (name, new) in events {
                progress(Progress::Character { name, new });
            }

            // Sync the reading back onto the page files: a page the part starts at the top of is replaced,
            // a page it continues (the part above was read before) gets the rest of its panels appended.
            let read_part = parsed.page;
            let mut clear = vec![];
            let mut out: Vec<PageRead> = unit
                .spans
                .iter()
                .enumerate()
                .map(|(n, s)| {
                    let fresh = s.y0 == 0;
                    let mut p = match read.pages.iter().find(|x| x.index == s.page) {
                        Some(old) if !fresh => old.clone(),
                        _ => PageRead { index: s.page, file: file_of(s.page), ..Default::default() },
                    };
                    if fresh {
                        clear.push(s.page);
                    }
                    if fresh || p.scene.kind.is_empty() {
                        p.scene = read_part.scene.clone();
                    }
                    if n == 0 {
                        p.summary = if p.summary.is_empty() { read_part.summary.clone() } else { format!("{} {}", p.summary, read_part.summary) };
                    }
                    p
                })
                .collect();
            let mut crops = vec![];
            let mut last = 0;
            for mut panel in read_part.panels {
                let (n, page_box) = panel.bbox.and_then(|b| unit.to_page(b)).unwrap_or((last, None));
                last = n;
                let part_box = panel.bbox;
                panel.bbox = page_box;
                for c in &mut panel.characters {
                    c.face = c.face.and_then(|f| unit.box_on_span(n, f));
                }
                let page = &mut out[n];
                if let Some(b) = part_box {
                    crops.push((page.index, page.panels.len(), b));
                }
                page.panels.push(panel);
            }

            let assets = Assets { image, clear, panels: crops, faces: face_jobs, panels_dir: panels_dir(ch) };
            match tokio::task::spawn_blocking(move || write_assets(assets)).await.map_err(|e| anyhow!(e))? {
                Ok(faces) => {
                    let mut b = brain.lock().unwrap();
                    for (id, p) in faces {
                        b.add_face(&id, p);
                    }
                }
                Err(e) => progress(Progress::Log(format!("Page {}: crops failed: {e}", i + 1))),
            }
            {
                let b = brain.lock().unwrap().clone();
                if let Err(e) = b.save() {
                    progress(Progress::Log(format!("Could not save the brain: {e}")));
                }
            }
            for p in out {
                upsert(&mut read, p);
            }
            if unit.stitched && !read.strip_done.contains(&unit.key()) {
                read.strip_done.push(unit.key());
            }
            save_read(ch, &read)?;
            for s in &unit.spans {
                finished += 1;
                progress(Progress::Page { done: finished, total, page: s.page });
            }
        }
    }
    progress(Progress::Done);
    Ok(read)
}

fn upsert(read: &mut ChapterRead, page: PageRead) {
    read.pages.retain(|p| p.index != page.index);
    read.pages.push(page);
    read.pages.sort_by_key(|p| p.index);
}
