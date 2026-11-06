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

const READ_SYSTEM: &str = r##"You read manga pages (plus reference face crops of known characters) and write an exact, structured transcript. A narrator retells the story from it as a recap video, so accuracy and consistent character names matter most. Never invent dialogue, never skip a balloon. Use only the attached images: do not run commands or open files.

Reply with STRICT JSON only (no fences, nothing before or after): {"pages":[PAGE, ...]}, one PAGE per page image, in the order given. Leave out any key whose value would be empty, null or []. PAGE:
{"summary":"1-2 sentences: what happens, using names","scene":{"kind":"fight|dialogue|emotional|comedy|action|reveal|calm|flashback|romance|horror|other","mood":"1-2 words","intensity":0-10,"tags":["2-5 short tags"]},"memory":"one sentence, only for a key plot event, reveal, relationship change or world fact","panels":[{"bbox":[x0,y0,x1,y1],"description":"1-2 sentences: setting, action, expressions, camera angle","characters":[{"name":"Ruko","was":"old placeholder","face":[x0,y0,x1,y1],"description":"new characters only: age, hair, eyes, outfit, build, role","traits":["new characters only"]}],"lines":[{"speaker":"Ruko","kind":"speech|thought|narration|sfx","text":"verbatim balloon text"}]}]}

Rules
1. ORDER: Japanese manga reads right-to-left, top to bottom (top-right panel first); manhwa, webtoons and Western comics read left-to-right, a vertical strip strictly top to bottom. Panels and balloons in reading order.
2. BOXES are normalised 0-1 to that page's whole image, origin top-left. Panel bbox = the panel frame. face = tight forehead-to-chin box; leave it out when the face is hidden, tiny or turned away.
3. LINES: every speech balloon, thought bubble, narration box and meaningful sound effect. Write every line in English: translate any other language, sound effects too (꾸욱 -> "Press"). Merge one balloon split over several bubbles. Narration and sfx have no speaker; leave the speaker out rather than guess wildly (use the tail, facing direction and roster).
4. CHARACTERS: match people by face, hair, eyes and outfit to the character bank and reference faces, and reuse bank names exactly. Use a real name when it is written or spoken. An unnamed newcomer gets a stable descriptive placeholder ("Silver-haired swordsman"), reused exactly next time. When a placeholder is named on the page, give the real name in "name" and the placeholder in "was". No background crowds.
5. SCENE: the single best kind; honest intensity (quiet talk 1-3, decisive blow or tearful goodbye 8-10).
6. SUMMARY continues the story from the page before: say what HAPPENS (arrivals, attacks, transformations, escapes, objects that matter), not only what is said. Keep a flashback a flashback until the page returns to the present. Look closely before naming anything gruesome. A title, credits or ad page: say so and leave out "panels"."##;

/// Light read (see `ReadOptions::light`): the same answer shape, asked for only what a cheaper
/// vision model gets right and the story needs: every line and its speaker, the scene, a short
/// summary and the panel frames. No looks, traits, face boxes or panel descriptions.
const LIGHT_READ_SYSTEM: &str = r##"You read manga pages and transcribe them. A narrator retells the story from your transcript, so every line must be exact. Never invent dialogue, never skip a balloon. Use only the attached images: do not run commands or open files.

Reply with STRICT JSON only (no fences, nothing before or after): {"pages":[PAGE, ...]}, one PAGE per page image, in the order given. Leave out any key whose value would be empty. PAGE:
{"summary":"1-2 sentences: what happens, using names","scene":{"kind":"fight|dialogue|emotional|comedy|action|reveal|calm|flashback|romance|horror|other","mood":"1-2 words","intensity":0-10},"panels":[{"bbox":[x0,y0,x1,y1],"characters":[{"name":"Ruko"}],"lines":[{"speaker":"Ruko","kind":"speech|thought|narration|sfx","text":"verbatim balloon text"}]}]}

Rules
1. ORDER: Japanese manga reads right-to-left (top-right panel first); manhwa, webtoons and Western comics left-to-right, a vertical strip top to bottom. Panels and balloons in reading order.
2. bbox = the panel frame, normalised 0-1 to that page's whole image, origin top-left.
3. LINES: every speech balloon, thought bubble, narration box and meaningful sound effect, in English (translate any other language, sound effects too). Narration and sfx have no speaker; leave the speaker out when unsure.
4. NAMES: reuse the character bank's names exactly; a real name when written or spoken; otherwise a short stable description ("Silver-haired swordsman"), the same every time.
5. A title, credits or ad page: say so in the summary and leave out "panels"."##;

/// Pages read at the same time. Cloud models answer many requests at once; a CLI agent is a whole
/// process per read that spends most of its ~7 s start-up idle, so more of them overlap well; a
/// local model shares one GPU, so a second read only hides the image prep.
fn parallel_reads(provider: AiProviderId) -> usize {
    match provider {
        AiProviderId::Ollama | AiProviderId::Lmstudio | AiProviderId::Llamacpp => 2,
        // OpenCode's free models queue a burst behind their rate limit (8 at once hung for
        // minutes); Antigravity answered empty under load. Fewer at once read through steadily.
        AiProviderId::Opencode => 2,
        AiProviderId::Antigravity => 4,
        AiProviderId::ClaudeCode | AiProviderId::Codex | AiProviderId::GeminiCli | AiProviderId::GrokCli => 8,
        _ => 10,
    }
}

/// Pages (or strip parts) read in one call. Every call carries the same fixed cost: the instructions,
/// the character bank, the reference faces and (on Claude Code) about 1,300 tokens of its own, so
/// eight pages a call pay it once instead of eight times. Measured on a manhwa chapter with Claude
/// Code: a page read alone took about 8,000 input and 530 output tokens, eight in one call about
/// 1,800 and 300 each, with the same dialogue. Only readers that get the pictures inline (Claude
/// Code and the API providers) read several: the other CLI agents open each picture as a file with
/// a tool, and a local model has a small context and costs no tokens, so they read one at a time.
pub(crate) fn batch_size(provider: AiProviderId) -> usize {
    match provider {
        AiProviderId::Ollama | AiProviderId::Lmstudio | AiProviderId::Llamacpp => 1,
        AiProviderId::Codex | AiProviderId::GeminiCli | AiProviderId::Opencode | AiProviderId::GrokCli | AiProviderId::Antigravity => 1,
        _ => 8,
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
    let fit = if webtoon { (1600.0 / w as f32).min(7000.0 / h as f32) } else { 1568.0 / w.max(h) as f32 };
    // A picture costs tokens by its area (about one per 750 pixels), and balloon text stays sharp
    // well below full size: a page is sent at most ~0.86 megapixels, but never under 600 px wide.
    let budget = (SEND_PIXELS / (w as f32 * h as f32)).sqrt().max(SEND_MIN_WIDTH / w.max(1) as f32);
    let factor = fit.min(budget);
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

/// Most pixels of a page sent to the reader (~1,150 tokens on Claude). Dialogue read the same at
/// 640 px wide as at 800 on a manhwa chapter.
const SEND_PIXELS: f32 = 860_000.0;
/// Narrowest a page is shrunk to, so a tall strip part keeps readable text.
const SEND_MIN_WIDTH: f32 = 600.0;

pub(crate) fn is_webtoon(w: u32, h: u32) -> bool {
    h as f32 / w.max(1) as f32 >= 2.2
}

/// A page (or stitched strip part) of a read call. `image` (the picture its boxes refer to) is
/// `None` when it could not be opened: its pages are stored as unreadable and not asked about.
struct Slot {
    unit: Unit,
    image: Option<PathBuf>,
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
pub(crate) fn read_system(bank: &str, light: bool) -> String {
    let rules = if light { LIGHT_READ_SYSTEM } else { READ_SYSTEM };
    format!("{rules}\n\nCHARACTER BANK (reuse these exact names; a page may add characters met since):\n{bank}")
}

/// One page (or stitched strip part) of a read call.
pub(crate) struct PageItem<'a> {
    pub index: usize,
    pub file: &'a str,
    /// Size of the page (or strip part) image, in pixels.
    pub size: (u32, u32),
    /// Last page of a stitched strip part that starts at `index` (see `strip.rs`).
    pub strip_to: Option<usize>,
}

pub(crate) struct PagePrompt<'a> {
    pub brain: &'a Brain,
    pub label: &'a str,
    pub total: usize,
    /// The pages read in this call, in order: Image 1, Image 2...
    pub items: &'a [PageItem<'a>],
    /// The summary of the page before the first one.
    pub prev_summary: &'a str,
    pub faces: &'a [(String, PathBuf)],
    /// The chapter's character bank (in the system prompt, see [`chapter_bank`]): the call lists
    /// only who is new since, and who was on the page before (`recent`) when the bank does not.
    pub bank: &'a Bank,
    pub recent: &'a [String],
}

/// How much of the story memory goes with each call (its latest part).
const NOTES_CHARS: usize = 600;

pub(crate) fn page_prompt(p: &PagePrompt) -> String {
    let n = p.items.len();
    let strip = p.items.iter().any(|it| it.strip_to.is_some());
    let layout = if strip || p.items.iter().any(|it| is_webtoon(it.size.0, it.size.1)) {
        "WEBTOON / vertical strip: read strictly top-to-bottom, left-to-right."
    } else {
        "Manga pages: if this is Japanese manga read right-to-left (top-right panel first); if it is clearly left-to-right (Western comic / manhwa) read left-to-right."
    };
    let title = if p.brain.manga_title.is_empty() { "(untitled)" } else { &p.brain.manga_title };
    let last_page = p.items.last().map(|it| it.strip_to.unwrap_or(it.index)).unwrap_or(0);
    let first_page = p.items.first().map(|it| it.index).unwrap_or(0);
    let span = if first_page == last_page { format!("page {}", first_page + 1) } else { format!("pages {}-{}", first_page + 1, last_page + 1) };
    let mut s = format!("MANGA: {title}\nCHAPTER: {} - {span} of {}\nLAYOUT: {layout}\n", p.label, p.total);
    if strip {
        s += "The strip parts were stitched back together from the page files and cut only at gaps between panels, so every panel and balloon in them is complete: never call anything cut off.\n";
    }
    let notes: String = {
        let n = p.brain.notes.trim();
        let skip = n.chars().count().saturating_sub(NOTES_CHARS);
        n.chars().skip(skip).collect()
    };
    if !notes.is_empty() {
        s += &format!("\nSTORY MEMORY SO FAR (most recent last):\n{notes}\n");
    }
    s += &format!("\nTHE PAGE BEFORE: {}\n", if p.prev_summary.is_empty() { "(none: this is the first page read)" } else { p.prev_summary });
    let new = new_in_bank(p.brain, p.bank, p.recent);
    if !new.is_empty() {
        s += &format!("\nNEW IN THE CHARACTER BANK SINCE THIS CHAPTER BEGAN (reuse these exact names too):\n{new}\n");
    }
    s += "\nIMAGES:";
    for (k, it) in p.items.iter().enumerate() {
        let (w, h) = it.size;
        s += &match it.strip_to {
            Some(last) => format!("\nImage {} = strip part covering pages {}-{} ({w}x{h} px).", k + 1, it.index + 1, last + 1),
            None => format!("\nImage {} = page {} (file {}, {w}x{h} px).", k + 1, it.index + 1, it.file),
        };
    }
    for (i, (name, _)) in p.faces.iter().enumerate() {
        s += &format!("\nImage {} = reference face of {name}.", n + i + 1);
    }
    s += &format!(
        "\n\nRead {} and reply with the JSON object only: \"pages\" holds exactly {n} {}, one per page image, in order.",
        if n == 1 { "Image 1".to_string() } else { format!("Images 1-{n}") },
        if n == 1 { "entry" } else { "entries" }
    );
    s
}

/// The answer of a call that read `n` pages, page by page: `{"pages": [...]}` (or a single page's
/// object, the older shape, for one page). `None` for a page the answer left out: it is read again
/// on its own. A reply that is not JSON is kept as the reading of a page read alone (its raw text),
/// and makes every page of a bigger call be read again on its own.
pub(crate) fn split_answer(answer: std::result::Result<Value, String>, n: usize) -> Vec<Option<std::result::Result<Value, String>>> {
    let v = match answer {
        Ok(v) => v,
        Err(raw) if n == 1 => return vec![Some(Err(raw))],
        Err(_) => return vec![None; n],
    };
    match v.get("pages").and_then(Value::as_array) {
        Some(pages) => (0..n)
            .map(|k| match pages.get(k) {
                Some(p) => Some(Ok(p.clone())),
                // One page asked, none given: an empty reading, not an endless re-read.
                None if n == 1 => Some(Ok(Value::Object(Default::default()))),
                None => None,
            })
            .collect(),
        None => (0..n).map(|k| (k == 0).then(|| Ok(v.clone()))).collect(),
    }
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

    // Pages are read several to a call ([`batch_size`]) and calls several at a time: each call is
    // slow (tens of seconds), so a new one starts as soon as any one finishes instead of waiting
    // for a whole round. Answers are still folded in page order, so each call sees the brain and
    // summaries of everything before it. The first call reads one page alone so the brain has
    // faces before calls run side by side.
    let width = parallel_reads(model.provider);
    let per_call = batch_size(model.provider);
    let ask = move |slots: Vec<Slot>, req: Option<AiStepRequest>| async move {
        let answer = match req {
            Some(r) => Some(ask_json(llm, r, cancel).await),
            None => None,
        };
        (slots, answer)
    };
    let mut queue: std::collections::VecDeque<Unit> = targets.into_iter().collect();
    // Pages a call's answer left out: read again, each on its own.
    let mut alone: std::collections::VecDeque<Unit> = Default::default();
    let mut flight = futures::stream::FuturesOrdered::new();
    let mut reading: Vec<usize> = vec![];
    let mut first_done = false;
    // The character bank goes in the system prompt, the same for every call of the chapter; each
    // call lists only who joined since (see `chapter_bank`).
    let mut bank = chapter_bank(&brain.lock().unwrap());
    let file_of = |p: usize| pages[p].file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    // Each call gets its own workspace (CLI agents clear their attachments when done), and each
    // part its own folder for its prepared picture.
    let ws_of = |u: &Unit| {
        let p = u.first();
        if u.stitched { work.join(format!("p{p:04}_{}", u.spans[0].y0)) } else { work.join(format!("p{p:04}")) }
    };
    loop {
        if cancel.is_cancelled() {
            progress(Progress::Failed("Cancelled".into()));
            return Err(anyhow!("cancelled"));
        }

        // 1. Start calls until the window is full: prepare the pictures and prompt, then ask.
        let mut started = false;
        while flight.len() < if first_done { width } else { 1 } {
            let units: Vec<Unit> = match alone.pop_front() {
                Some(u) => vec![u],
                None => {
                    let take = if first_done { per_call } else { 1 }.min(queue.len());
                    queue.drain(..take).collect()
                }
            };
            if units.is_empty() {
                break;
            }
            started = true;
            let ws = ws_of(&units[0]);
            let dirs: Vec<PathBuf> = units.iter().map(ws_of).collect();
            for d in &dirs {
                std::fs::create_dir_all(d).context("create page workspace")?;
            }
            let (all, us) = (pages.clone(), units.clone());
            // Per part: (the image its boxes refer to, the copy sent, the copy's width and height).
            let prepped: Vec<Result<(PathBuf, PathBuf, u32, u32)>> = tokio::task::spawn_blocking(move || {
                us.iter()
                    .zip(dirs)
                    .map(|(u, wk)| {
                        let p = u.first();
                        let image = if u.stitched {
                            let out = wk.join("strip.jpg");
                            strip::compose(u, &all, &out)?;
                            out
                        } else {
                            all[p].clone()
                        };
                        let (send, w, h) = prep_page(&image, &wk, p)?;
                        Ok((image, send, w, h))
                    })
                    .collect()
            })
            .await
            .map_err(|e| anyhow!(e))?;
            let mut slots = vec![];
            let mut sends = vec![];
            for (unit, prep) in units.into_iter().zip(prepped) {
                reading.extend(unit.pages());
                match prep {
                    Ok((image, send, w, h)) => {
                        sends.push((send, w, h));
                        slots.push(Slot { unit, image: Some(image) });
                    }
                    Err(e) => {
                        progress(Progress::Log(format!("Page {}: {e}", unit.first() + 1)));
                        for s in unit.spans.iter().filter(|s| s.y0 == 0) {
                            upsert(&mut read, PageRead { index: s.page, file: file_of(s.page), summary: format!("(unreadable image: {e})"), ..Default::default() });
                        }
                        slots.push(Slot { unit, image: None });
                    }
                }
            }
            if sends.is_empty() {
                flight.push_back(ask(slots, None));
                continue;
            }
            let ready: Vec<&Unit> = slots.iter().filter(|s| s.image.is_some()).map(|s| &s.unit).collect();
            let i = ready[0].first();
            // A part that starts inside page i continues the reading already stored for page i.
            let upto = if ready[0].spans[0].y0 > 0 { i + 1 } else { i };
            let prev_summary = (0..upto).rev().find_map(|p| read.pages.iter().find(|x| x.index == p && !x.summary.is_empty())).map(|p| p.summary.clone()).unwrap_or_default();
            let recent = recent_names(&read, i);
            let files: Vec<String> = ready.iter().map(|u| file_of(u.first())).collect();
            let items: Vec<PageItem> = ready
                .iter()
                .zip(&sends)
                .zip(&files)
                .map(|((u, (_, w, h)), file)| PageItem { index: u.first(), file, size: (*w, *h), strip_to: u.stitched.then(|| u.spans[u.spans.len() - 1].page) })
                .collect();
            let (prompt, faces) = {
                let b = brain.lock().unwrap();
                // Many characters joined since the bank was made (a new manga's first chapter):
                // the bank is made again, so each call lists only a few newcomers.
                if bank.newcomers(&b) > BANK_NEW {
                    bank = chapter_bank(&b);
                }
                let faces = pick_faces(&b, &recent);
                let prompt = page_prompt(&PagePrompt { brain: &b, label: &ch.label, total: pages.len(), items: &items, prev_summary: &prev_summary, faces: &faces, bank: &bank, recent: &recent });
                (prompt, faces)
            };
            drop(items);
            drop(ready);
            let mut images: Vec<PathBuf> = sends.into_iter().map(|(p, _, _)| p).collect();
            images.extend(faces.iter().map(|(_, p)| p.clone()));
            let req = request(model, vec![AiMessage::system(read_system(&bank.text, opts.light)), AiMessage::user(prompt)], images, Some(ws));
            flight.push_back(ask(slots, Some(req)));
        }
        if started {
            progress(Progress::Reading { pages: reading.clone(), done: finished, total });
        }

        // 2. Fold in the next answer, in page order.
        let Some((slots, answer)) = flight.next().await else { break };
        first_done = true;
        for s in &slots {
            reading.retain(|p| !s.unit.spans.iter().any(|x| x.page == *p));
        }
        let first = slots[0].unit.first();
        let answers = match answer {
            None => vec![],
            Some(Err(e)) if is_cancelled(&e) => {
                save_read(ch, &read)?;
                progress(Progress::Failed("Cancelled".into()));
                return Err(e);
            }
            Some(Err(e)) => {
                save_read(ch, &read)?;
                return Err(fail(format!("Page {}: {e}", first + 1)));
            }
            Some(Ok(a)) => {
                let parts = split_answer(a.clone(), slots.iter().filter(|s| s.image.is_some()).count());
                // A model that cannot see pictures says so, or asks to be told what is on the page:
                // that is no reading, and every other page would go the same way.
                if blind_reply(&own_words(&a)) || parts.iter().flatten().any(|p| blind_reply(&own_words(p))) {
                    save_read(ch, &read)?;
                    return Err(fail(format!("The picked AI model ({}) can't see pictures, so it can't read pages. Pick one that can, like Claude, Gemini or Grok, for Read.", model.model)));
                }
                parts
            }
        };
        let mut answers = answers.into_iter();
        for Slot { unit, image } in slots {
            let i = unit.first();
            let Some(image) = image else {
                save_read(ch, &read)?;
                for s in &unit.spans {
                    finished += 1;
                    progress(Progress::Page { done: finished, total, page: s.page });
                }
                continue;
            };
            let Some(answer) = answers.next().flatten() else {
                progress(Progress::Log(format!("Page {}: left out of the answer; reading it again on its own.", i + 1)));
                alone.push_back(unit);
                continue;
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
        let system = read_system(&bank.text, false);
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
