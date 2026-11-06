//! READ phase: a vision model reads every page, captures characters into the brain, tags scenes and
//! writes `read.json` after each page (so a crash or cancel never loses work).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use image::{imageops::FilterType, DynamicImage};
use futures::StreamExt;
use mv_ai::{AiMessage, AiProviderId, AiStepRequest, CancellationToken, Registry};
use serde_json::Value;

use crate::brain::Brain;
use crate::json::{bbox_of, num_of, opt_str, str_list, str_of};
use crate::llm::{Patient, ask_json, is_cancelled, request, truncate, Llm};
use crate::store::*;
use crate::strip::{self, Unit};
use crate::types::*;

const MAX_FACES_PER_CHARACTER: usize = 4;
const MAX_FACE_ATTACHMENTS: usize = 6;
const FACE_PX: u32 = 192;
pub(crate) const MIN_PANEL_AREA: f32 = 0.02;
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
6. SUMMARY continues the story: use the previous page (its summary, and its image when attached) for continuity and write what is new here. Say what HAPPENS, not only what is said: who arrives, attacks, transforms, flees, what object matters (look closely before naming anything gruesome: a flying object is a severed head only when it clearly is one). Keep a flashback a flashback until the page shows the return to the present. If the page is only a title, credits or an ad, say so in the summary and return an empty "panels" list.
7. Never wrap the JSON in code fences. Never output anything but the JSON object."##;

/// Pages read at the same time. Cloud models answer many requests at once; a CLI agent is a whole
/// process per read that spends most of its ~7 s start-up idle, so more of them overlap well; a
/// local model shares one GPU, so a second read only hides the image prep.
fn parallel_reads(provider: AiProviderId) -> usize {
    match provider {
        AiProviderId::Ollama | AiProviderId::Lmstudio | AiProviderId::Llamacpp => 2,
        AiProviderId::ClaudeCode | AiProviderId::Codex | AiProviderId::GeminiCli | AiProviderId::Opencode | AiProviderId::GrokCli | AiProviderId::Antigravity => 8,
        _ => 10,
    }
}

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

/// The reply of a model that cannot see the page (a text-only model such as GPT-OSS): it says it
/// cannot view images, or asks to be told what is on the page, instead of reading it.
pub(crate) fn blind_reply(text: &str) -> bool {
    const SAID: [&str; 26] = [
        // It says it cannot see the picture...
        "does not support image",
        "doesn't support image",
        "unable to view image",
        "unable to view the image",
        "unable to see image",
        "unable to see the image",
        "cannot view image",
        "can't view image",
        "cannot view the image",
        "can't view the image",
        "cannot see the image",
        "can't see the image",
        "cannot see any image",
        "can't see any image",
        "not able to see image",
        "not able to view image",
        "cannot process images",
        "no image was provided",
        "image was not provided",
        // ...or asks to be told what is on the page (never words a page summary would use).
        "share the image",
        "upload the image",
        "describe the page",
        "describe the image",
        "provide a description of the page",
        "provide a brief description of the page",
        "what is on the page?",
    ];
    let t = text.to_lowercase().replace('\u{2019}', "'");
    SAID.iter().any(|s| t.contains(s))
}

/// What the model said itself in a page answer: its summary and other top-level text (or the
/// whole reply when it was not JSON), never the characters' lines it read.
pub(crate) fn own_words(answer: &std::result::Result<Value, String>) -> String {
    match answer {
        Ok(Value::Object(m)) => m.values().filter_map(Value::as_str).collect::<Vec<_>>().join(" "),
        Ok(v) => v.as_str().unwrap_or_default().to_string(),
        Err(raw) => raw.clone(),
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

/// Long side of the page-before image sent for context: enough to tell who is who, at a fraction of the cost.
const CONTEXT_PX: u32 = 900;

/// A small copy of the page before, in `work`, for the reader's context. `None` when it cannot be read.
pub(crate) fn prep_context(path: &Path, work: &Path) -> Option<PathBuf> {
    let img = image::open(path).ok()?;
    let small = if img.width().max(img.height()) > CONTEXT_PX { img.resize(CONTEXT_PX, CONTEXT_PX, FilterType::Triangle) } else { img };
    let out = work.join("page_before.jpg");
    save_jpeg(&small, &out).ok()?;
    Some(out)
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

/// Characters the bank lists (most seen first), and how many of a character's other names: the
/// latest ones, their newest names and looks. The older ones stay in the brain, which still
/// matches them when a reading uses one (`Brain::observe`).
const BANK_SIZE: usize = 40;
const BANK_ALIASES: usize = 4;
/// Characters new or renamed since a chapter's reading began, listed with each page.
const BANK_NEW: usize = 12;

/// One character in the bank: name, latest other names and what they look like / who they are.
/// Traits (how a character behaves) are left out: they do not help to tell who is on a page, and
/// they cost tokens with every page read.
fn bank_line(c: &crate::types::Character) -> String {
    let mut s = format!("- {}", c.name);
    let aliases: Vec<&str> = c.aliases.iter().rev().take(BANK_ALIASES).map(String::as_str).collect();
    if !aliases.is_empty() {
        s += &format!(" (also called: {})", aliases.join(", "));
    }
    if !c.description.is_empty() {
        s += &format!(": {}", truncate(&c.description, 140));
    }
    s
}

fn by_appearances(brain: &Brain) -> Vec<&crate::types::Character> {
    let mut cs: Vec<_> = brain.characters.iter().collect();
    cs.sort_by_key(|c| std::cmp::Reverse(c.appearances));
    cs
}

pub(crate) fn roster_text(brain: &Brain, limit: usize) -> String {
    let cs = by_appearances(brain);
    if cs.is_empty() {
        return "(none yet - every character you see is new)".into();
    }
    cs.iter().take(limit).map(|c| bank_line(c)).collect::<Vec<_>>().join("\n")
}

/// The character bank as it stands when a chapter's reading starts. Its text goes in the system
/// prompt, the same for every page of the chapter, so the provider reads it from its prompt cache
/// instead of billing it in full with every page; each page adds only who is new ([`new_in_bank`]).
pub(crate) struct Bank {
    /// Every character the brain knew then: id to name.
    all: std::collections::HashMap<String, String>,
    /// The ones the text lists (the most seen).
    listed: std::collections::HashSet<String>,
    pub text: String,
}

pub(crate) fn chapter_bank(brain: &Brain) -> Bank {
    Bank {
        all: brain.characters.iter().map(|c| (c.id.clone(), c.name.clone())).collect(),
        listed: by_appearances(brain).into_iter().take(BANK_SIZE).map(|c| c.id.clone()).collect(),
        text: roster_text(brain, BANK_SIZE),
    }
}

impl Bank {
    /// Met or renamed since the bank was made.
    fn is_new(&self, c: &crate::types::Character) -> bool {
        self.all.get(&c.id) != Some(&c.name)
    }

    /// How many characters were met or renamed since (many: time to make the bank again).
    pub(crate) fn newcomers(&self, brain: &Brain) -> usize {
        brain.characters.iter().filter(|c| self.is_new(c)).count()
    }
}

/// What a page adds to the bank: the characters met or renamed since it was made, and the ones
/// seen on the page before (`recent`) that it does not list, most seen first.
pub(crate) fn new_in_bank(brain: &Brain, bank: &Bank, recent: &[String]) -> String {
    by_appearances(brain)
        .into_iter()
        .filter(|c| bank.is_new(c) || (!bank.listed.contains(&c.id) && recent.iter().any(|r| r.eq_ignore_ascii_case(&c.name))))
        .take(BANK_NEW)
        .map(bank_line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The page reader's system prompt with the chapter's character bank.
pub(crate) fn read_system(bank: &str) -> String {
    format!("{READ_SYSTEM}\n\nCHARACTER BANK (reuse these exact names; a page may add characters met since):\n{bank}")
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
    /// The page before is attached (Image 2) for context.
    pub prev_image: bool,
    /// The chapter's character bank (in the system prompt, see [`chapter_bank`]): the page lists
    /// only who is new since, and who was on the page before (`recent`) when the bank does not.
    pub bank: &'a Bank,
    pub recent: &'a [String],
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
    let new = new_in_bank(p.brain, p.bank, p.recent);
    if !new.is_empty() {
        s += &format!("\nNEW IN THE CHARACTER BANK SINCE THIS CHAPTER BEGAN (reuse these exact names too):\n{new}\n");
    }
    s += if p.strip_to.is_some() { "\nATTACHED IMAGES:\nImage 1 = the strip part to read." } else { "\nATTACHED IMAGES:\nImage 1 = the page to read." };
    if p.prev_image {
        s += "\nImage 2 = the page BEFORE it, for context only: who is who, which scene and time this is (the present or a flashback), who is talking to whom. Never transcribe it; read only Image 1.";
    }
    let first_face = if p.prev_image { 3 } else { 2 };
    for (i, (name, path)) in p.faces.iter().enumerate() {
        let f = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        s += &format!("\nImage {} = reference face of {} ({}).", i + first_face, name, f);
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
    let llm = Patient::new(reg, progress.clone());
    read_chapter_with(&llm, &model, &ch, &brain, &opts, &progress, &cancel).await
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
    // Pages read from now on are snapped as they are read.
    read.panels_snapped |= read.pages.is_empty();
    read.chapter_id = ch.chapter_id.clone();
    read.model = model_label(model);
    // Pages the user cropped by hand keep their panels; frames and crops are not looked for there.
    let manual = crate::manual::load_manual(ch);
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
    // New names on the pages: the story's cast check runs again.
    read.cast_checked = false;
    let total: usize = targets.iter().map(|u| u.spans.len()).sum();
    let mut finished = 0;

    // Parts are read several at a time: each model call is slow (tens of seconds), so a new part
    // starts as soon as any one finishes instead of waiting for a whole batch. Answers are still
    // folded in page order, so each part sees the brain and summaries of everything before it.
    // The first part goes alone so the brain has faces before parts are read side by side.
    let width = parallel_reads(model.provider);
    let ask = move |unit: Unit, image: PathBuf, req: Option<AiStepRequest>| async move {
        let answer = match req {
            Some(r) => Some(ask_json(llm, r, cancel).await),
            None => None,
        };
        (unit, image, answer)
    };
    let mut queue = targets.iter();
    let mut flight = futures::stream::FuturesOrdered::new();
    let mut reading: Vec<usize> = vec![];
    let mut first_done = false;
    // The character bank goes in the system prompt, the same for every page, so it is billed in
    // full once and then read from the provider's prompt cache (see `chapter_bank`).
    let mut bank = chapter_bank(&brain.lock().unwrap());
    loop {
        if cancel.is_cancelled() {
            progress(Progress::Failed("Cancelled".into()));
            return Err(anyhow!("cancelled"));
        }

        // 1. Start parts until the window is full: prepare the image and prompt, then ask.
        let mut started = false;
        while flight.len() < if first_done { width } else { 1 } {
            let Some(unit) = queue.next() else { break };
            started = true;
            reading.extend(unit.pages());
            let i = unit.first();
            let file = pages[i].file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
            // Each read gets its own workspace: CLI agents clear their attachments when done.
            let ws = if unit.stitched { work.join(format!("p{i:04}_{}", unit.spans[0].y0)) } else { work.join(format!("p{i:04}")) };
            std::fs::create_dir_all(&ws).context("create page workspace")?;
            let (all, wk, u) = (pages.clone(), ws.clone(), unit.clone());
            let prepped = tokio::task::spawn_blocking(move || -> Result<(PathBuf, PathBuf, u32, u32, Option<PathBuf>)> {
                let image = if u.stitched {
                    let out = wk.join("strip.jpg");
                    strip::compose(&u, &all, &out)?;
                    out
                } else {
                    all[i].clone()
                };
                let (send, w, h) = prep_page(&image, &wk, i)?;
                let before = (!u.stitched && i > 0).then(|| prep_context(&all[i - 1], &wk)).flatten();
                Ok((image, send, w, h, before))
            })
            .await
            .map_err(|e| anyhow!(e))?;
            let (image, send_path, w, h, before) = match prepped {
                Ok(x) => x,
                Err(e) => {
                    progress(Progress::Log(format!("Page {}: {e}", i + 1)));
                    for s in unit.spans.iter().filter(|s| s.y0 == 0) {
                        let file = pages[s.page].file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                        upsert(&mut read, PageRead { index: s.page, file, summary: format!("(unreadable image: {e})"), ..Default::default() });
                    }
                    flight.push_back(ask(unit.clone(), PathBuf::new(), None));
                    continue;
                }
            };
            // A part that starts inside page i continues the reading already stored for page i.
            let upto = if unit.spans[0].y0 > 0 { i + 1 } else { i };
            let prev_summary = (0..upto).rev().find_map(|p| read.pages.iter().find(|x| x.index == p && !x.summary.is_empty())).map(|p| p.summary.clone()).unwrap_or_default();
            let recent = recent_names(&read, i);
            let (prompt, faces) = {
                let b = brain.lock().unwrap();
                // Many characters joined since the bank was made (a new manga's first chapter):
                // the bank is made again, so each page lists only a few newcomers.
                if bank.newcomers(&b) > BANK_NEW {
                    bank = chapter_bank(&b);
                }
                let faces = pick_faces(&b, &recent);
                let strip_to = unit.stitched.then(|| unit.spans[unit.spans.len() - 1].page);
                let prompt = page_prompt(&PagePrompt { brain: &b, label: &ch.label, index: i, total: pages.len(), file: &file, size: (w, h), prev_summary: &prev_summary, faces: &faces, strip_to, prev_image: before.is_some(), bank: &bank, recent: &recent });
                (prompt, faces)
            };
            let mut images = vec![send_path];
            images.extend(before);
            images.extend(faces.iter().map(|(_, p)| p.clone()));
            let req = request(model, vec![AiMessage::system(read_system(&bank.text)), AiMessage::user(prompt)], images, Some(ws));
            flight.push_back(ask(unit.clone(), image, Some(req)));
        }
        if started {
            progress(Progress::Reading { pages: reading.clone(), done: finished, total });
        }

        // 2. Fold in the next answer, in page order.
        let Some((unit, image, answer)) = flight.next().await else { break };
        first_done = true;
        {
            let i = unit.first();
            reading.retain(|p| !unit.spans.iter().any(|s| s.page == *p));
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
            // A model that cannot see pictures says so, or asks to be told what is on the page:
            // that is no reading, and every other page would go the same way.
            if blind_reply(&own_words(&answer)) {
                save_read(ch, &read)?;
                return Err(fail(format!("The picked AI model ({}) can't see pictures, so it can't read pages. Pick one that can, like Claude, Gemini or Grok, for Read.", model.model)));
            }
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
            let mut read_part = parsed.page;
            let hand: Vec<(usize, Vec<Bbox>)> = unit.spans.iter().filter_map(|s| manual.boxes(s.page, &file_of(s.page)).map(|b| (s.page, b.to_vec()))).collect();
            // The reader's panel boxes snap to the frames drawn on the page (or strip part).
            if read_part.panels.iter().any(|p| p.bbox.is_some()) && hand.len() < unit.spans.len() {
                let img = image.clone();
                let frames = tokio::task::spawn_blocking(move || image::open(&img).map(|i| crate::panels::detect(&i)).unwrap_or_default()).await.map_err(|e| anyhow!(e))?;
                for p in &mut read_part.panels {
                    p.bbox = p.bbox.map(|b| crate::panels::snap(&frames, b));
                }
            }
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
            crops.retain(|c| !hand.iter().any(|(p, _)| *p == c.0));

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
            if !hand.is_empty() {
                crate::manual::fold(&mut read, &hand, |p| unit.spans.iter().any(|s| s.page == p && s.y0 == 0));
                let (c, all) = (ch.clone(), pages.clone());
                if let Err(e) = tokio::task::spawn_blocking(move || crate::manual::write_all(&c, &all, &hand)).await.map_err(|e| anyhow!(e))? {
                    progress(Progress::Log(format!("Page {}: hand-cropped panels failed: {e}", i + 1)));
                }
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

/// Snap the panel boxes of a chapter read before snapping existed to the frames on its pages, and
/// redo the crops of the panels that moved (blocking). A strip chapter keeps its boxes. Returns
/// whether the reading changed.
pub(crate) fn snap_read(ch: &ChapterRef, read: &mut ChapterRead) -> bool {
    if read.panels_snapped {
        return false;
    }
    read.panels_snapped = true;
    if !read.strip_done.is_empty() {
        return true;
    }
    let pages = list_pages(&ch.pages_dir);
    for p in &mut read.pages {
        if read.manual.contains(&p.index) {
            continue;
        }
        let Some(img) = pages.get(p.index).and_then(|f| image::open(f).ok()) else { continue };
        let frames = crate::panels::detect(&img);
        for (n, pn) in p.panels.iter_mut().enumerate() {
            let Some(b) = pn.bbox else { continue };
            let s = crate::panels::snap(&frames, b);
            if s != b {
                pn.bbox = Some(s);
                if s.area() >= MIN_PANEL_AREA {
                    let _ = crop_panel(&img, s, &panel_path(ch, p.index, n));
                }
            }
        }
    }
    true
}

fn upsert(read: &mut ChapterRead, page: PageRead) {
    read.pages.retain(|p| p.index != page.index);
    read.pages.push(page);
    read.pages.sort_by_key(|p| p.index);
}

#[cfg(test)]
mod bank_tests {
    use super::*;
    use crate::types::Character;

    fn person(name: &str, seen: u32, aliases: &[&str]) -> Character {
        Character {
            id: name.into(),
            name: name.into(),
            aliases: aliases.iter().map(|a| a.to_string()).collect(),
            description: format!("{name} {}", "looks ".repeat(60)),
            traits: vec!["smug".into(), "furious".into()],
            appearances: seen,
            ..Default::default()
        }
    }

    /// The bank is sent once per chapter in the system prompt (cached by the provider), short: no
    /// traits, the latest four other names, the description's start. Each page lists only who joined.
    #[test]
    fn the_bank_is_short_and_pages_list_only_newcomers() {
        let mut b = Brain::default();
        b.characters.push(person("Junho", 600, &["a", "b", "c", "d", "e", "f"]));
        b.characters.push(person("Dahyun", 100, &[]));
        let bank = chapter_bank(&b);
        assert!(!bank.text.contains("smug") && !bank.text.contains("traits"), "{}", bank.text);
        assert!(bank.text.starts_with("- Junho (also called: f, e, d, c): Junho looks"), "{}", bank.text);
        assert!(bank.text.lines().all(|l| l.chars().count() < 200), "{}", bank.text);
        let system = read_system(&bank.text);
        assert!(system.starts_with(READ_SYSTEM) && system.contains("CHARACTER BANK") && system.contains("- Dahyun"));
        assert_eq!(new_in_bank(&b, &bank, &[]), "", "nothing new on the next page");

        // Met and renamed since: listed with the page, and counted towards making the bank again.
        b.characters.push(person("Masked robber", 1, &[]));
        b.characters[1].name = "Jung Dahyun".into();
        let new = new_in_bank(&b, &bank, &[]);
        assert!(new.contains("- Jung Dahyun") && new.contains("- Masked robber") && !new.contains("Junho"), "{new}");
        assert_eq!(bank.newcomers(&b), 2);

        // Someone the bank leaves out (not among the most seen) is listed when on the page before.
        let mut many = Brain::default();
        for k in 0..BANK_SIZE + 1 {
            many.characters.push(person(&format!("C{k}"), 100 - k as u32, &[]));
        }
        let bank = chapter_bank(&many);
        let last = format!("C{BANK_SIZE}");
        assert!(!bank.text.contains(&format!("- {last} ")));
        assert_eq!(new_in_bank(&many, &bank, &[]), "");
        assert!(new_in_bank(&many, &bank, std::slice::from_ref(&last)).starts_with(&format!("- {last}")));
    }
}
