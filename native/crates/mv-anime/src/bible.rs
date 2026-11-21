//! BIBLE extraction (design 6.2): one planner call drafts the recurring characters, places
//! (floor plan, landmarks, 5–9 camera stations including one overhead, light states, constant
//! forces), props and the style; the draft is merged into the stored bible keeping everything the
//! user locked. A second run only adds what is new and fills what is still empty.
//!
//! Model-free parts are deterministic and tested: the reply is validated into model types (unknown
//! panels and chapters dropped, colours made distinct, coordinates clamped into the plan, an
//! overhead station always present), and the approval gate lists what still blocks gate 1.
//!
//! What the reply carries beyond the model's fields lives in each entity's `extra` map under the
//! `EXTRA_*` keys (the LLM's best panels, the voice brief, whether a character speaks, a place's
//! constant forces), so it survives round-trips and the crops step can read it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use anyhow::{anyhow, bail, Result};
use mv_ai::AiEffort;
use mv_recap::{Brain, ChapterRead, ChapterRef};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::llm::BadReply;
use crate::merge::merge_unlocked;
use crate::model::{
    BibleIndex, CharVoice, Character, FloorPlan, Issue, Landmark, LandmarkKind, Look, Outfit, Palette, Place, PlaceKind, PlaceState, Prop, Role, Severity, SheetKind, SourceRef,
    Station, Status, Story, Style, Swatch, TimeOfDay, Wall,
};
use crate::queue::{JobCtx, Tier};
use crate::store::Store;
use crate::{crops, source};

/// The look the style line falls back to when the draft has none.
pub const DEFAULT_STYLE_LINE: &str = "Thin dark-brown lines, two-tone hard cel shading, painted backgrounds without outlines, glow on light sources, soft gradient from the key-light side.";

/// `Character.extra` key: the LLM's best panels (`[{chapter, page, panel, angle, framing}]`),
/// one of the two signals of the reference crops.
pub const EXTRA_BEST_PANELS: &str = "best_panels";
/// `Character.extra` key: the voice brief (`{gender, age, type}`) for the Sound step.
pub const EXTRA_VOICE: &str = "voice_brief";
/// `Character.extra` key: `true` when the character has lines in the read chapters.
pub const EXTRA_SPEAKS: &str = "speaks";
/// `Place.extra` / `Prop.extra` key: panels that show it (`[{chapter, page, panel}]`).
pub const EXTRA_PANELS: &str = "panels";
/// `Place.extra` key: what is the same in every shot (`{wind_deg, wind, moving}`).
pub const EXTRA_FORCES: &str = "forces";
/// Draft-only `Place.extra` key: script scene ids set in the place (taken out before the merge).
const EXTRA_SCENES: &str = "scenes";
/// Bookkeeping key in every bible entity's `extra`: for each list (by its path, `/look/keep`,
/// `/outfits`, `/plan/landmarks`, ...) the items the AI has offered so far. An item offered
/// before and missing now was removed by the user, so a rebuild does not bring it back.
pub const EXTRA_OFFERED: &str = "ai_offered";

/// Most stations a place gets (design: 5–9, always one overhead).
pub const MAX_STATIONS: usize = 9;
/// A new place is filled up to this many stations with standard views.
pub const MIN_STATIONS: usize = 5;
/// The id of the overhead station the merge adds when a place has none.
pub const OVERHEAD_ID: &str = "st_overhead";

const SYSTEM: &str = include_str!("prompts/bible.txt");
/// Characters of manga digest per call (design 6.2), shared between the chapters.
const DIGEST_CHARS: usize = 30_000;
const LENSES: [u16; 6] = [14, 24, 35, 50, 85, 135];
const DEFAULT_NEGATIVE: [&str; 3] = ["text", "speech bubbles", "watermark"];
/// Two signature colours closer than this (CIE76 ΔE) are too alike to tell apart small.
const MIN_COLOUR_DELTA: f32 = 25.;
/// Saturated, mutually distinct colours handed out when the AI's colour is missing or too close.
const DISTINCT: [&str; 16] = [
    "#D7263D", "#1B6FE0", "#2BA84A", "#F2A900", "#8E3BD6", "#00A6A6", "#F26419", "#E83F9F", "#5C3D2E", "#7FB800", "#0D2C54", "#C0C0C0", "#FFD23F", "#3BCEAC", "#B5179E", "#6D6875",
];
const NO_AI: &str = "No AI is set up for the Anime Engine. Pick a provider and model in the assistant's settings, then build the bible again.";
const CANCELLED: &str = "cancelled";

/// What one extraction proposes, already mapped into model types.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub characters: Vec<Character>,
    pub places: Vec<Place>,
    pub props: Vec<Prop>,
    pub style: Option<Style>,
}

/// What a merge changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MergeReport {
    /// Entity ids added as Draft.
    pub added: Vec<String>,
    /// Entity ids whose unlocked fields changed.
    pub updated: Vec<String>,
    pub issues: Vec<Issue>,
}

// ---------------------------------------------------------------- the job

/// The Bible job for an episode: extraction, merge, then crops for the new characters.
pub async fn run_job(cx: &JobCtx<'_>, episode: &str) -> Result<()> {
    let store = cx.store;
    let say = |f: f32, line: &str| (cx.progress)(f, line);
    say(0.02, "Reading the chapters…");
    let series = store.series()?;
    let ep = store.episode(episode)?;
    if ep.chapters.is_empty() {
        bail!("This episode has no chapters yet. Add chapters in the Manga step, then build the bible.");
    }
    let title = series.manga_title.clone();
    let chapters: Vec<(ChapterRef, ChapterRead)> = ep.chapters.iter().filter_map(|p| source::read(p, &title).map(|r| (source::chapter_ref(p, &title), r))).collect();
    if chapters.is_empty() {
        bail!("None of this episode's chapters has been read yet. Read them in the Manga step, then build the bible.");
    }
    // Crops scored by an older version are scored again first (local, no AI).
    let st = store.clone();
    match tokio::task::spawn_blocking(move || upgrade_crops(&st)).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => mv_core::logging::global().warn("anime", &format!("bible: the reference pictures were not scored again: {e:#}")),
        Err(_) => mv_core::logging::global().warn("anime", "bible: scoring the reference pictures again stopped unexpectedly"),
    }
    let manga_id = if series.manga_id.is_empty() { ep.chapters[0].manga_id.clone() } else { series.manga_id.clone() };
    let src = Sources { brain: source::brain(&manga_id, &title), story: store.story(episode).unwrap_or_default(), bible: store.bible()?, chapters };

    let ai = (cx.run.ai)(Tier::Planner).ok_or_else(|| anyhow!(NO_AI))?;
    say(0.08, "The AI is designing the cast, the places and the props…");
    let progress = |line: String| (cx.progress)(0.1, &line);
    let reply = match ai.ask_json(SYSTEM, user_text(&src), Vec::new(), AiEffort::High, &progress, cx.cancel).await {
        Ok(v) => v,
        Err(e) => {
            if let Some(bad) = e.downcast_ref::<BadReply>() {
                save_raw(store, cx.job.id, &bad.raw);
            }
            return Err(e);
        }
    };
    if cx.cancel.is_cancelled() {
        bail!(CANCELLED);
    }

    say(0.55, "Checking the AI's bible…");
    let mut issues = Vec::new();
    let mut draft = draft_from_reply(&reply, &src, &mut issues);
    if draft.characters.is_empty() && draft.places.is_empty() && draft.props.is_empty() {
        save_raw(store, cx.job.id, &reply.to_string());
        bail!("The AI's answer held no characters, places or props. Try again or pick another model.");
    }
    let links = take_scene_links(&mut draft);
    let mut report = merge(store, draft)?;
    issues.append(&mut report.issues);
    link_scenes(store, episode, &links)?;
    update_cast(store)?;
    #[cfg(feature = "story")]
    relink_speakers(store, episode, &src.brain);

    say(0.65, "Cutting reference pictures from the pages…");
    let made = cut_crops(store, &report.added, &src.chapters, &links, cx).await?;
    for i in &issues {
        mv_core::logging::global().info("anime", &format!("bible: {}", i.message));
    }
    let bible = store.bible()?;
    let notes = if issues.is_empty() { String::new() } else { format!(" · {} notes", issues.len()) };
    say(
        1.,
        &format!(
            "Bible ready: {} characters, {} places, {} props · {} new, {} updated · {made} reference pictures{notes}",
            bible.characters.len(),
            bible.places.len(),
            bible.props.len(),
            report.added.len(),
            report.updated.len()
        ),
    );
    Ok(())
}

/// Keeps a reply the engine could not use, for the user or a bug report (`work/ai/<job>.txt`).
fn save_raw(store: &Store, job: u64, raw: &str) {
    let file = store.work_dir().join("ai").join(format!("{job}.txt"));
    let ok = file.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) && std::fs::write(&file, raw).is_ok();
    if !ok {
        mv_core::logging::global().warn("anime", "could not keep the AI's unreadable answer in work/ai");
    }
}

/// Cuts reference crops for every character, place and prop that was added or has none yet;
/// returns how many crops were kept.
async fn cut_crops(store: &Store, added: &[String], reads: &[(ChapterRef, ChapterRead)], links: &[(String, String)], cx: &JobCtx<'_>) -> Result<usize> {
    let bible = store.bible()?;
    let added: BTreeSet<&str> = added.iter().map(String::as_str).collect();
    let chars: Vec<Character> = bible.characters.iter().filter(|c| added.contains(c.id.as_str()) || c.crops.is_empty()).cloned().collect();
    let story_pages = scene_pages(store, links);
    let places: Vec<(String, Vec<(SourceRef, u8)>)> = bible
        .places
        .iter()
        .filter(|p| added.contains(p.id.as_str()) || p.crops.is_empty())
        .map(|p| (p.id.clone(), place_picks(p, reads, story_pages.get(&p.id))))
        .collect();
    let props: Vec<(String, Vec<(SourceRef, u8)>)> =
        bible.props.iter().filter(|p| added.contains(p.id.as_str()) || p.crops.is_empty()).map(|p| (p.id.clone(), prop_picks(p, reads))).collect();
    if cx.cancel.is_cancelled() {
        bail!(CANCELLED);
    }
    let (st, reads) = (store.clone(), reads.to_vec());
    let cut = tokio::task::spawn_blocking(move || cut_all(&st, &chars, &places, &props, &reads))
        .await
        .map_err(|_| anyhow!("Cutting the reference pictures stopped unexpectedly. Build the bible again."))?;
    save_cuts(store, cut, false)
}

/// What one cut made: per character, place (with its cover panels) and prop.
type Cuts = (Vec<(String, Vec<crops::Cut>)>, Vec<(String, Vec<crops::Cut>, Vec<String>)>, Vec<(String, Vec<crops::Cut>)>);

/// Cuts and scores the crops of these entities from the Reads (local only: pages and the Read).
fn cut_all(st: &Store, chars: &[Character], places: &[(String, Vec<(SourceRef, u8)>)], props: &[(String, Vec<(SourceRef, u8)>)], reads: &[(ChapterRef, ChapterRead)]) -> Cuts {
    let c = chars.iter().map(|c| (c.id.clone(), crops::pick_cut(st, c, reads))).collect();
    let p = places
        .iter()
        .map(|(id, picks)| {
            let (cut, covers) = crops::place_crops(st, id, picks, reads);
            (id.clone(), cut, covers)
        })
        .collect();
    let r = props.iter().map(|(id, picks)| (id.clone(), crops::prop_crops(st, id, picks, reads))).collect();
    (c, p, r)
}

/// Saves a cut onto the entities as they are now (the user may have edited them meanwhile):
/// approved and uploaded crops stay, dropped ones stay out ([`crops::merge_cut`]). Returns how
/// many crops the cut made. Only entities whose content really changes are written; `quiet`
/// (a score-only upgrade the user did not ask for) keeps no "earlier version" copies.
fn save_cuts(store: &Store, cuts: Cuts, quiet: bool) -> Result<usize> {
    // The read and the saves under the project's bible lock (the cut itself ran before it).
    store.with_bible(|| save_cuts_locked(store, cuts, quiet))
}

fn save_cuts_locked(store: &Store, (c, p, r): Cuts, quiet: bool) -> Result<usize> {
    use crate::store::Quiet;
    let bible = store.bible()?;
    let mut made = 0;
    for (id, new) in c {
        if let Some(cur) = bible.character(&id) {
            made += new.len();
            let mut next = cur.clone();
            next.crops = crops::merge_cut(&cur.crops, new);
            if next != *cur {
                if quiet { store.save_quiet(Quiet::Character(&next))? } else { store.save_character(&next)? }
            }
        }
    }
    for (id, new, covers) in p {
        if let Some(cur) = bible.place(&id) {
            made += new.len();
            let mut next = cur.clone();
            if !new.is_empty() {
                next.extra.insert(crops::EXTRA_COVERS.into(), json!(covers));
            }
            next.crops = crops::merge_cut(&cur.crops, new);
            if next != *cur {
                if quiet { store.save_quiet(Quiet::Place(&next))? } else { store.save_place(&next)? }
            }
        }
    }
    for (id, new) in r {
        if let Some(cur) = bible.prop(&id) {
            made += new.len();
            let mut next = cur.clone();
            next.crops = crops::merge_cut(&cur.crops, new);
            if next != *cur {
                if quiet { store.save_quiet(Quiet::Prop(&next))? } else { store.save_prop(&next)? }
            }
        }
    }
    Ok(made)
}

/// "Cut the crops again": cuts and scores the crops of every character, place and prop again
/// from the pages and the Studio Read, with no AI call (the AI's earlier panel picks, kept in
/// each entity, are reused). Every crop the user approved or uploaded stays exactly as it is, a
/// crop the user dropped stays out, the others take their new scores, and the place covers are
/// picked again. Stamps the bible with the current scoring. Blocking (decodes pages): run it off
/// the UI thread. Returns how many crops the cut made.
pub fn recut_crops(store: &Store) -> Result<usize> {
    recut(store, false)
}

fn recut(store: &Store, quiet: bool) -> Result<usize> {
    let reads = crops::series_reads(store)?;
    if reads.is_empty() {
        bail!("No chapter of this project has been read yet. Read the chapters in the Manga step first.");
    }
    let bible = store.bible()?;
    let pages = scene_pages(store, &[]);
    let chars: Vec<Character> = bible.characters.iter().filter(|c| c.status != Status::Rejected).cloned().collect();
    let places: Vec<(String, Vec<(SourceRef, u8)>)> = bible.places.iter().filter(|p| p.status != Status::Rejected).map(|p| (p.id.clone(), place_picks(p, &reads, pages.get(&p.id)))).collect();
    let props: Vec<(String, Vec<(SourceRef, u8)>)> = bible.props.iter().filter(|p| p.status != Status::Rejected).map(|p| (p.id.clone(), prop_picks(p, &reads))).collect();
    let made = save_cuts(store, cut_all(store, &chars, &places, &props, &reads), quiet)?;
    stamp_scoring(store, quiet)?;
    Ok(made)
}

/// Marks the bible's crops as scored with the current [`crops::SCORING_VERSION`] (written only
/// when the stamp changes).
fn stamp_scoring(store: &Store, quiet: bool) -> Result<()> {
    store.with_bible(|| {
        let cur = store.bible()?.style;
        let mut style = cur.clone();
        style.extra.insert(crops::EXTRA_SCORING.into(), json!(crops::SCORING_VERSION));
        if style == cur {
            return Ok(());
        }
        if quiet {
            store.save_quiet(crate::store::Quiet::Style(&style))
        } else {
            store.save_style(&style)
        }
    })
}

/// When the bible's crops were scored by an older version (or never stamped), scores them again
/// locally (no AI). Cheap and quiet: it reads only the style file when the stamp is current,
/// writes only the files whose content changes, and keeps no "earlier version" copies (the user
/// did not make this change). Blocking: run it off the UI thread, after the bible is shown.
/// `Ok(true)` when it re-cut.
pub fn upgrade_crops(store: &Store) -> Result<bool> {
    let style: crate::model::Style = crate::store::read_json(&store.path("bible/style.json"))?;
    if !crops::scoring_outdated(&style) {
        return Ok(false);
    }
    let bible = store.bible()?;
    let nothing = bible.characters.is_empty() && bible.places.is_empty() && bible.props.is_empty();
    if nothing {
        // A new bible: its crops will be cut with the current scoring.
        stamp_scoring(store, true)?;
        return Ok(false);
    }
    if crops::series_reads(store)?.is_empty() {
        // Nothing to cut from yet; try again on a later load.
        return Ok(false);
    }
    recut(store, true)?;
    Ok(true)
}
/// (chapter id, page) of every script page set in each place, from the story and the AI's links.
fn scene_pages(store: &Store, links: &[(String, String)]) -> BTreeMap<String, BTreeSet<(String, usize)>> {
    let mut out: BTreeMap<String, BTreeSet<(String, usize)>> = BTreeMap::new();
    let Ok(series) = store.series() else { return out };
    for ep in &series.episodes {
        let Ok(story) = store.story(&ep.id) else { continue };
        for scene in &story.scenes {
            let place = scene.place.clone().or_else(|| links.iter().find(|(s, _)| s == &scene.id).map(|(_, p)| p.clone()));
            if let Some(place) = place {
                let pages = out.entry(place).or_default();
                for s in scene.source.iter().chain(scene.beats.iter().flat_map(|b| b.source.iter())) {
                    pages.insert((s.chapter_id.clone(), s.page));
                }
            }
        }
    }
    out
}

/// A place's panel picks with their signal count: the AI's pick, plus the script setting a scene
/// in this place on that page, or the panel's own description naming the place or a landmark.
fn place_picks(place: &Place, reads: &[(ChapterRef, ChapterRead)], pages: Option<&BTreeSet<(String, usize)>>) -> Vec<(SourceRef, u8)> {
    let mut words: Vec<String> = vec![place.name.to_lowercase()];
    words.extend(place.plan.landmarks.iter().map(|l| l.name.to_lowercase()).filter(|n| n.chars().count() >= 4));
    panel_refs(place.extra.get(EXTRA_PANELS))
        .into_iter()
        .map(|at| {
            let by_story = pages.is_some_and(|p| p.contains(&(at.chapter_id.clone(), at.page)));
            let named = panel_of(reads, &at).is_some_and(|panel| {
                let d = panel.description.to_lowercase();
                words.iter().any(|w| !w.is_empty() && d.contains(w.as_str()))
            });
            (at, 1 + u8::from(by_story || named))
        })
        .collect()
}

/// A prop's panel picks: the AI's pick, plus the panel's description or lines naming the prop.
fn prop_picks(prop: &Prop, reads: &[(ChapterRef, ChapterRead)]) -> Vec<(SourceRef, u8)> {
    let name = prop.name.to_lowercase();
    let last = name.split_whitespace().last().unwrap_or_default().to_string();
    panel_refs(prop.extra.get(EXTRA_PANELS))
        .into_iter()
        .map(|at| {
            let named = panel_of(reads, &at).is_some_and(|panel| {
                let mut text = panel.description.to_lowercase();
                for l in &panel.lines {
                    text.push(' ');
                    text.push_str(&l.text.to_lowercase());
                }
                (!name.is_empty() && text.contains(name.as_str())) || (last.chars().count() >= 3 && text.contains(last.as_str()))
            });
            (at, 1 + u8::from(named))
        })
        .collect()
}

/// `[{chapter, page, panel}]` from an extra value.
pub(crate) fn panel_refs(v: Option<&Value>) -> Vec<SourceRef> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|p| Some(SourceRef { chapter_id: text(p, "chapter"), page: index(p, "page")?, panel: index(p, "panel") })).filter(|s| !s.chapter_id.is_empty()).collect())
        .unwrap_or_default()
}

/// The Read's panel at a source position.
pub(crate) fn panel_of<'a>(reads: &'a [(ChapterRef, ChapterRead)], at: &SourceRef) -> Option<&'a mv_recap::Panel> {
    let (_, read) = reads.iter().find(|(c, _)| c.chapter_id == at.chapter_id)?;
    read.pages.iter().find(|p| p.index == at.page)?.panels.get(at.panel?)
}

/// Sets the script's scenes that have no place (and whose place the user did not lock) to the
/// place the AI set them in.
fn link_scenes(store: &Store, episode: &str, links: &[(String, String)]) -> Result<()> {
    if links.is_empty() {
        return Ok(());
    }
    let bible = store.bible()?;
    // On the story as it is on disk now, under its own lock: a voice batch or a script edit
    // saved at the same moment is kept.
    let link = |story: &mut crate::model::Story| {
        for scene in &mut story.scenes {
            let known = scene.place.as_deref().is_some_and(|p| bible.place(p).is_some());
            if known || scene.locked.contains("/place") {
                continue;
            }
            if let Some((_, place)) = links.iter().find(|(s, p)| s == &scene.id && bible.place(p).is_some()) {
                scene.place = Some(place.clone());
            }
        }
    };
    // Only touched when there is something to link (never makes a story file that is not there).
    let mut probe = store.story(episode)?;
    let before = probe.clone();
    link(&mut probe);
    if probe != before {
        store.update_story(episode, link)?;
    }
    Ok(())
}

/// Links the story's speakers to the bible's characters (a speaker stored by name while nobody
/// had that name becomes the new character's id; a locked speaker stays). A failure is only
/// logged: the bible itself is saved.
#[cfg(feature = "story")]
fn relink_speakers(store: &Store, episode: &str, brain: &Brain) {
    let result = (|| -> Result<usize> {
        let bible = store.bible()?;
        let cast = crate::story::Cast::new(&bible, brain);
        // A dry run first (the story file is only touched when something links), then the same
        // change on the story as it is on disk now, under its own lock.
        let mut probe = store.story(episode)?;
        let before = probe.clone();
        crate::story::relink_speakers(&mut probe, &cast);
        if probe == before {
            return Ok(0);
        }
        let mut n = 0;
        store.update_story(episode, |story| n = crate::story::relink_speakers(story, &cast))?;
        Ok(n)
    })();
    match result {
        Ok(0) => {}
        Ok(n) => mv_core::logging::global().info("anime", &format!("bible: {n} lines linked to their character")),
        Err(e) => mv_core::logging::global().warn("anime", &format!("bible: the story's speakers were not linked to the new characters: {e:#}")),
    }
}

/// Every speaking character gets a row in the voice list (`cast.json`); the Sound step picks the
/// voice. Existing rows (and the user's choices in them) are never touched.
fn update_cast(store: &Store) -> Result<()> {
    let bible = store.bible()?;
    let speaking: Vec<String> = bible.characters.iter().filter(|c| c.status != Status::Rejected && speaks(c)).map(|c| c.id.clone()).collect();
    if speaking.iter().all(|id| bible.cast.voices.iter().any(|v| &v.character == id)) {
        return Ok(());
    }
    // Rows added on the cast as it is on disk now, under its own lock: a voice the user picked
    // in the Sound step at the same moment is kept.
    store.update_cast(|cast| {
        for id in speaking {
            if !cast.voices.iter().any(|v| v.character == id) {
                cast.voices.push(CharVoice { character: id, speed: 1., ..Default::default() });
            }
        }
    })?;
    Ok(())
}

/// The character has lines in the read chapters (as found by the bible step).
pub fn speaks(c: &Character) -> bool {
    c.extra.get(EXTRA_SPEAKS).and_then(Value::as_bool).unwrap_or(false)
}

// ---------------------------------------------------------------- the prompt

/// Everything the extraction reads.
struct Sources {
    chapters: Vec<(ChapterRef, ChapterRead)>,
    brain: Brain,
    story: Story,
    bible: BibleIndex,
}

/// How often a name is seen and heard in the reads.
#[derive(Default)]
struct NameStats {
    shown: String,
    panels: u32,
    lines: u32,
}

impl Sources {
    /// Per lower-cased name: panels it is seen in, lines it speaks.
    fn stats(&self) -> BTreeMap<String, NameStats> {
        let mut out: BTreeMap<String, NameStats> = BTreeMap::new();
        for (_, read) in &self.chapters {
            for page in &read.pages {
                for panel in &page.panels {
                    let mut seen = BTreeSet::new();
                    for c in &panel.characters {
                        let n = flat(&c.name);
                        if !n.is_empty() && seen.insert(n.to_lowercase()) {
                            let e = out.entry(n.to_lowercase()).or_default();
                            e.shown = n;
                            e.panels += 1;
                        }
                    }
                    for l in &panel.lines {
                        let Some(s) = l.speaker.as_deref().map(flat).filter(|s| !s.is_empty()) else { continue };
                        if l.kind.trim().eq_ignore_ascii_case("sfx") || l.text.trim().is_empty() {
                            continue;
                        }
                        let e = out.entry(s.to_lowercase()).or_default();
                        if e.shown.is_empty() {
                            e.shown = s;
                        }
                        e.lines += 1;
                    }
                }
            }
        }
        out
    }

    fn panel(&self, at: &SourceRef) -> Option<&mv_recap::Panel> {
        panel_of(&self.chapters, at)
    }

    /// (panels, lines) of a character by its names.
    fn counts(&self, stats: &BTreeMap<String, NameStats>, names: &[String]) -> Option<(u32, u32)> {
        let mut hit = None;
        for n in names {
            if let Some(s) = stats.get(&n.trim().to_lowercase()) {
                let (p, l) = hit.unwrap_or((0, 0));
                hit = Some((p.max(s.panels), l.max(s.lines)));
            }
        }
        hit
    }
}

/// The user message: known characters, other names seen, the bible so far, the script, the digest.
fn user_text(src: &Sources) -> String {
    let stats = src.stats();
    let mut out = String::new();
    let mut known: BTreeSet<String> = BTreeSet::new();
    out.push_str("KNOWN CHARACTERS (Studio's character brain; use these ids as brain_id)\n");
    if src.brain.characters.is_empty() {
        out.push_str("(none yet)\n");
    }
    for c in &src.brain.characters {
        let mut names = vec![c.name.clone()];
        names.extend(c.aliases.iter().cloned());
        names.iter().for_each(|n| {
            known.insert(n.trim().to_lowercase());
        });
        let (panels, lines) = src.counts(&stats, &names).unwrap_or((0, 0));
        let _ = write!(out, "- id={} | {}", c.id, flat(&c.name));
        if !c.aliases.is_empty() {
            let _ = write!(out, " (also: {})", c.aliases.iter().map(|a| flat(a)).collect::<Vec<_>>().join(", "));
        }
        let _ = write!(out, " | in these chapters: {panels} panels, {lines} lines");
        if !c.description.trim().is_empty() {
            let _ = write!(out, " | {}", flat(&c.description));
        }
        if !c.traits.is_empty() {
            let _ = write!(out, " | traits: {}", c.traits.iter().map(|t| flat(t)).collect::<Vec<_>>().join(", "));
        }
        out.push('\n');
    }
    let others: Vec<&NameStats> = stats.iter().filter(|(k, _)| !known.contains(*k)).map(|(_, s)| s).collect();
    if !others.is_empty() {
        out.push_str("\nOTHER NAMES IN THE PANELS (seen in panels / lines spoken)\n");
        for s in others {
            let _ = writeln!(out, "- {} | {} panels, {} lines", s.shown, s.panels, s.lines);
        }
    }
    let b = &src.bible;
    if !(b.characters.is_empty() && b.places.is_empty() && b.props.is_empty()) {
        out.push_str("\nTHE BIBLE SO FAR (reuse these names exactly; add only what is missing)\n");
        for c in &b.characters {
            let _ = writeln!(out, "- character {} ({})", flat(&c.name), c.id);
        }
        for p in &b.places {
            let _ = writeln!(out, "- place {} ({}): {} x {} m, {} landmarks, {} stations", flat(&p.name), p.id, p.plan.w_m, p.plan.d_m, p.plan.landmarks.len(), p.stations.len());
        }
        for p in &b.props {
            let _ = writeln!(out, "- prop {} ({})", flat(&p.name), p.id);
        }
    }
    if !src.story.scenes.is_empty() {
        out.push_str("\nTHE SCRIPT (scene id | title | time | place | manga pages | summary)\n");
        for s in &src.story.scenes {
            let place = s.place.as_deref().and_then(|p| b.place(p)).map(|p| flat(&p.name)).unwrap_or_else(|| "not set".into());
            let time = serde_json::to_value(s.time_of_day).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
            let _ = writeln!(out, "- {} | {} | {} | {} | {} | {}", s.id, flat(&s.title), time, place, pages_text(&s.source), flat(&s.summary));
        }
    }
    out.push_str("\nTHE MANGA\n");
    let per = (DIGEST_CHARS / src.chapters.len().max(1)).max(2_000);
    for (chref, read) in &src.chapters {
        let label = if chref.label.trim().is_empty() { String::new() } else { format!(" ({})", flat(&chref.label)) };
        let _ = writeln!(out, "=== chapter {}{label} ===", chref.chapter_id);
        out.push_str(&source::digest(read, per));
    }
    out.push_str("\nWrite the bible for these chapters now, as JSON only.");
    out
}

/// "c1 p0-p3" for a scene's sources.
fn pages_text(src: &[SourceRef]) -> String {
    let mut by: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for s in src {
        let e = by.entry(s.chapter_id.as_str()).or_insert((s.page, s.page));
        e.0 = e.0.min(s.page);
        e.1 = e.1.max(s.page);
    }
    if by.is_empty() {
        return "-".into();
    }
    by.iter().map(|(c, (a, b))| if a == b { format!("{c} p{a}") } else { format!("{c} p{a}-p{b}") }).collect::<Vec<_>>().join(", ")
}

// ---------------------------------------------------------------- the reply

fn flat(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn text(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => flat(s),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn num(v: &Value, k: &str) -> Option<f32> {
    let x = v.get(k)?;
    x.as_f64().or_else(|| x.as_str().and_then(|s| s.trim().trim_end_matches('m').trim().parse().ok())).map(|f| f as f32).filter(|f| f.is_finite())
}

fn index(v: &Value, k: &str) -> Option<usize> {
    num(v, k).filter(|f| *f >= 0. && *f < 1e6).map(|f| f.round() as usize)
}

fn list<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

fn strings(v: &Value, k: &str) -> Vec<String> {
    match v.get(k) {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(flat).filter(|s| !s.is_empty()).collect(),
        Some(Value::String(s)) if !s.trim().is_empty() => vec![flat(s)],
        _ => Vec::new(),
    }
}

/// A plain word into one of the model's enums; unknown words give `fallback`.
fn word<T: DeserializeOwned + PartialEq>(s: &str, unknown: T, fallback: T) -> T {
    let w: String = s.trim().to_lowercase().chars().map(|c| if c == ' ' || c == '-' { '_' } else { c }).collect();
    match serde_json::from_value::<T>(Value::String(w)) {
        Ok(v) if v != unknown => v,
        _ => fallback,
    }
}

/// A short id part: lower-case ASCII letters and digits joined by `_`, at most 32 characters.
pub(crate) fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    let out: String = out.trim_end_matches('_').chars().take(32).collect();
    let out = out.trim_end_matches('_').to_string();
    if out.is_empty() {
        "item".into()
    } else {
        out
    }
}

/// `base`, or `base_2`, `base_3`, … if taken; the result is marked taken.
fn unique(base: String, taken: &mut BTreeSet<String>) -> String {
    let mut id = base.clone();
    let mut n = 2;
    while taken.contains(&id) {
        id = format!("{base}_{n}");
        n += 1;
    }
    taken.insert(id.clone());
    id
}

/// "#RRGGBB" (also "RRGGBB", "#RGB") to RGB.
pub(crate) fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    let h: String = match h.len() {
        3 => h.chars().flat_map(|c| [c, c]).collect(),
        6 => h.to_string(),
        _ => return None,
    };
    let v = u32::from_str_radix(&h, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

/// CIE L*a*b* of an sRGB colour (D65).
fn lab(c: [u8; 3]) -> [f32; 3] {
    let lin = |u: u8| {
        let v = u as f32 / 255.;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (lin(c[0]), lin(c[1]), lin(c[2]));
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
    let f = |t: f32| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16. / 116. };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116. * fy - 16., 500. * (fx - fy), 200. * (fy - fz)]
}

/// CIE76 colour difference.
pub(crate) fn delta_e(a: [u8; 3], b: [u8; 3]) -> f32 {
    let (p, q) = (lab(a), lab(b));
    ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()
}

/// Gives every character without a fixed colour one that differs from all the others by at least
/// [`MIN_COLOUR_DELTA`]. `fixed` are colours already in use (existing characters).
fn distinct_colours(chars: &mut [Character], fixed: &[[u8; 3]], keep: &BTreeSet<String>) {
    let mut used: Vec<[u8; 3]> = fixed.to_vec();
    for c in chars.iter_mut() {
        if keep.contains(&c.id) {
            continue;
        }
        let own = parse_hex(&c.look.signature_colour);
        let ok = own.filter(|o| used.iter().all(|u| delta_e(*o, *u) >= MIN_COLOUR_DELTA));
        let pick = ok.or_else(|| {
            let options: Vec<[u8; 3]> = DISTINCT.iter().filter_map(|h| parse_hex(h)).collect();
            options.iter().copied().find(|o| used.iter().all(|u| delta_e(*o, *u) >= MIN_COLOUR_DELTA)).or_else(|| {
                // Everything is close to something: the option farthest from its nearest neighbour.
                options.iter().copied().max_by(|a, b| {
                    let near = |x: &[u8; 3]| used.iter().map(|u| delta_e(*x, *u)).fold(f32::MAX, f32::min);
                    near(a).total_cmp(&near(b))
                })
            })
        });
        if let Some(p) = pick {
            c.look.signature_colour = hex(p);
            used.push(p);
        }
    }
}

/// The source position of a reply entry, if the Read has that panel.
fn checked_panel(src: &Sources, v: &Value) -> Option<SourceRef> {
    let mut chapter = text(v, "chapter");
    if chapter.is_empty() && src.chapters.len() == 1 {
        chapter = src.chapters[0].0.chapter_id.clone();
    }
    let at = SourceRef { chapter_id: chapter, page: index(v, "page")?, panel: Some(index(v, "panel")?) };
    src.panel(&at).map(|_| at)
}

fn issue(code: &str, severity: Severity, message: String) -> Issue {
    Issue { code: code.into(), severity, message, ..Default::default() }
}

/// The reply in model types. Unknown panels and chapters are dropped (reported as issues).
fn draft_from_reply(v: &Value, src: &Sources, issues: &mut Vec<Issue>) -> Draft {
    let stats = src.stats();
    let mut draft = Draft::default();
    let chapter_ids: BTreeSet<&str> = src.chapters.iter().map(|(c, _)| c.chapter_id.as_str()).collect();

    // Characters.
    let mut taken: BTreeSet<String> = src.bible.characters.iter().map(|c| c.id.clone()).collect();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    let mut keep_colour: BTreeSet<String> = BTreeSet::new();
    for c in list(v, "characters") {
        let name = text(c, "name");
        if name.is_empty() {
            continue;
        }
        let given = text(c, "brain_id");
        let brain = src.brain.characters.iter().find(|b| !given.is_empty() && (b.id == given || mv_core::paths::safe_name(&b.id) == given)).or_else(|| src.brain.find(&name));
        let brain_id = brain.map(|b| b.id.clone());
        // By brain id, then by name or alias, then by the id the name would get (the user
        // renamed "Kai" to "Kai Tanaka": the card is still `ch_kai`).
        let slug_id = format!("ch_{}", slug(&name));
        let existing = src
            .bible
            .characters
            .iter()
            .find(|e| (brain_id.is_some() && e.brain_id == brain_id) || e.name.eq_ignore_ascii_case(&name) || e.aliases.iter().any(|a| a.eq_ignore_ascii_case(&name)))
            .or_else(|| src.bible.characters.iter().find(|e| brain_id.is_none() && e.brain_id.is_none() && e.id == slug_id));
        let id = match (existing, &brain_id) {
            (Some(e), _) => e.id.clone(),
            (None, Some(b)) => unique(mv_core::paths::safe_name(b), &mut taken),
            (None, None) => unique(format!("ch_{}", slug(&name)), &mut taken),
        };
        if !seen_ids.insert(id.clone()) {
            issues.push(issue("bible-duplicate", Severity::Info, format!("The AI listed {name} twice; the first card was kept.")));
            continue;
        }
        let mut names = vec![name.clone()];
        if let Some(b) = brain {
            names.push(b.name.clone());
            names.extend(b.aliases.iter().cloned());
        }
        let (panels, lines) = src.counts(&stats, &names).unwrap_or((0, 0));
        if brain.is_none() && existing.is_none() && panels < 2 && lines == 0 && stats.contains_key(&name.to_lowercase()) {
            issues.push(issue("bible-extra", Severity::Info, format!("{name} appears in only one panel and never speaks, so no card was made.")));
            continue;
        }
        if existing.is_some_and(|e| !e.look.signature_colour.is_empty()) {
            keep_colour.insert(id.clone());
        }
        let look_v = c.get("look").cloned().unwrap_or(Value::Null);
        let height = num(&look_v, "height_m").filter(|h| (0.3..=12.).contains(h)).unwrap_or(1.7);
        let mut outfit_ids = BTreeSet::new();
        let outfits = list(c, "outfits")
            .iter()
            .filter_map(|o| {
                let n = text(o, "name");
                let d = text(o, "description");
                if n.is_empty() && d.is_empty() {
                    return None;
                }
                let n = if n.is_empty() { "Outfit".to_string() } else { n };
                let chapters = strings(o, "chapters").into_iter().filter(|ch| chapter_ids.contains(ch.as_str())).collect();
                Some(Outfit { id: unique(slug(&n), &mut outfit_ids), name: n, description: d, chapters })
            })
            .collect();
        let mut extra = Map::new();
        let mut best = Vec::new();
        for p in list(c, "best_panels") {
            match checked_panel(src, p) {
                Some(at) => {
                    let mut e = json!({"chapter": at.chapter_id, "page": at.page, "panel": at.panel});
                    for k in ["angle", "framing"] {
                        let w = text(p, k);
                        if !w.is_empty() {
                            e[k] = Value::String(w.to_lowercase().replace([' ', '-'], "_"));
                        }
                    }
                    if !best.contains(&e) {
                        best.push(e);
                    }
                }
                None => issues.push(issue("bible-panel", Severity::Info, format!("{name}: the AI named a panel this chapter does not have ({}); it was skipped.", short(p)))),
            }
        }
        best.truncate(8);
        extra.insert(EXTRA_BEST_PANELS.into(), Value::Array(best));
        if let Some(voice) = c.get("voice").filter(|x| x.is_object()) {
            extra.insert(EXTRA_VOICE.into(), json!({"gender": text(voice, "gender"), "age": text(voice, "age"), "type": text(voice, "type")}));
        }
        extra.insert(EXTRA_SPEAKS.into(), Value::Bool(lines > 0));
        let mut aliases: Vec<String> = brain.map(|b| b.aliases.clone()).unwrap_or_default();
        if let Some(b) = brain.filter(|b| !b.name.eq_ignore_ascii_case(&name)) {
            aliases.insert(0, b.name.clone());
        }
        let keep: Vec<String> = strings(&look_v, "keep").into_iter().take(6).collect();
        draft.characters.push(Character {
            id,
            brain_id,
            name,
            aliases,
            role: word(&text(c, "role"), Role::Unknown, Role::Neutral),
            description: text(c, "description"),
            age: text(c, "age"),
            look: Look {
                hair: text(&look_v, "hair"),
                eyes: text(&look_v, "eyes"),
                build: text(&look_v, "build"),
                default_outfit: text(&look_v, "outfit"),
                signature_colour: parse_hex(&text(&look_v, "signature_colour")).map(hex).unwrap_or_default(),
                keep,
                height_m: height,
            },
            outfits,
            extra,
            ..Default::default()
        });
    }
    let fixed: Vec<[u8; 3]> = src.bible.characters.iter().filter(|e| !draft.characters.iter().any(|d| d.id == e.id)).filter_map(|e| parse_hex(&e.look.signature_colour)).collect();
    distinct_colours(&mut draft.characters, &fixed, &keep_colour);

    // Places.
    let mut taken: BTreeSet<String> = src.bible.places.iter().map(|p| p.id.clone()).collect();
    for p in list(v, "places") {
        let name = text(p, "name");
        if name.is_empty() {
            continue;
        }
        let id = match src.bible.places.iter().find(|e| e.name.eq_ignore_ascii_case(&name)) {
            Some(e) => e.id.clone(),
            None => unique(format!("pl_{}", slug(&name)), &mut taken),
        };
        if draft.places.iter().any(|d| d.id == id) {
            issues.push(issue("bible-duplicate", Severity::Info, format!("The AI listed the place {name} twice; the first card was kept.")));
            continue;
        }
        draft.places.push(place_from(p, id, name, src, issues));
    }

    // Props.
    let mut taken: BTreeSet<String> = src.bible.props.iter().map(|p| p.id.clone()).collect();
    for p in list(v, "props") {
        let name = text(p, "name");
        if name.is_empty() {
            continue;
        }
        let id = match src.bible.props.iter().find(|e| e.name.eq_ignore_ascii_case(&name)) {
            Some(e) => e.id.clone(),
            None => unique(format!("pr_{}", slug(&name)), &mut taken),
        };
        if draft.props.iter().any(|d| d.id == id) {
            continue;
        }
        let owner_name = text(p, "owner");
        let owner = (!owner_name.is_empty())
            .then(|| {
                draft
                    .characters
                    .iter()
                    .chain(src.bible.characters.iter())
                    .find(|c| c.name.eq_ignore_ascii_case(&owner_name) || c.aliases.iter().any(|a| a.eq_ignore_ascii_case(&owner_name)))
                    .map(|c| c.id.clone())
            })
            .flatten();
        let panels: Vec<Value> = list(p, "panels").iter().filter_map(|x| checked_panel(src, x)).map(|at| json!({"chapter": at.chapter_id, "page": at.page, "panel": at.panel})).take(4).collect();
        let mut extra = Map::new();
        extra.insert(EXTRA_PANELS.into(), Value::Array(panels));
        draft.props.push(Prop {
            id,
            name,
            owner,
            description: text(p, "description"),
            held: p.get("held").and_then(Value::as_bool).unwrap_or(false),
            states: strings(p, "states"),
            extra,
            ..Default::default()
        });
    }

    // Style.
    if let Some(s) = v.get("style").filter(|s| s.is_object()) {
        let mut negative = strings(s, "negative");
        for n in DEFAULT_NEGATIVE {
            if !negative.iter().any(|x| x.eq_ignore_ascii_case(n)) {
                negative.push(n.to_string());
            }
        }
        let hints: Vec<[u8; 3]> = strings(s, "palette_hint").iter().filter_map(|h| parse_hex(h)).take(7).collect();
        let palettes = if hints.is_empty() { Vec::new() } else { vec![palette(&hints)] };
        draft.style = Some(Style { line: text(s, "line"), negative, palettes, ..Default::default() });
    }
    draft
}

/// A short form of a reply entry for messages.
fn short(v: &Value) -> String {
    format!("chapter {} page {} panel {}", text(v, "chapter"), text(v, "page"), text(v, "panel"))
}

/// The series palette from the AI's hints (roles in the prompt's order).
fn palette(hints: &[[u8; 3]]) -> Palette {
    const ROLES: [&str; 7] = ["dominant", "shadow", "light", "accent", "skin", "mid", "deep"];
    let (mut l, mut s) = (0f32, 0f32);
    for c in hints {
        let (max, min) = (c.iter().copied().max().unwrap_or(0) as f32 / 255., c.iter().copied().min().unwrap_or(0) as f32 / 255.);
        let light = (max + min) / 2.;
        l += light;
        s += if max == min { 0. } else { (max - min) / (1. - (2. * light - 1.).abs()).max(1e-3) };
    }
    let n = hints.len().max(1) as f32;
    Palette {
        id: "series".into(),
        name: "Series palette".into(),
        swatches: hints.iter().zip(ROLES).map(|(c, r)| Swatch { role: r.into(), hex: hex(*c) }).collect(),
        brightness: (l / n).clamp(0., 1.),
        saturation: (s / n).clamp(0., 1.),
    }
}

/// The colour temperature and mood a light state gets when the AI gave none.
fn state_defaults(id: &str) -> (u32, &'static str) {
    match id {
        "night" => (7500, "cold, dark, quiet"),
        "dusk" => (3200, "warm, fading, uneasy"),
        "dawn" => (4300, "pale, cool, hopeful"),
        "indoor" => (3200, "warm lamp light"),
        _ => (5600, "clear, neutral daylight"),
    }
}

/// A state id: a time-of-day word when the name holds one, else a slug.
fn state_id(s: &str) -> String {
    let l = s.to_lowercase();
    for w in ["night", "dusk", "dawn", "day", "indoor"] {
        if l.split(|c: char| !c.is_ascii_alphanumeric()).any(|p| p == w) {
            return w.into();
        }
    }
    slug(s)
}

fn time_word(t: TimeOfDay) -> &'static str {
    match t {
        TimeOfDay::Dusk => "dusk",
        TimeOfDay::Night => "night",
        TimeOfDay::Dawn => "dawn",
        TimeOfDay::Indoor => "indoor",
        _ => "day",
    }
}

fn place_from(p: &Value, id: String, name: String, src: &Sources, issues: &mut Vec<Issue>) -> Place {
    let kind = word(&text(p, "kind"), PlaceKind::Unknown, PlaceKind::Interior);
    let size: Vec<f32> = list(p, "size_m").iter().filter_map(|x| x.as_f64().map(|f| f as f32)).filter(|f| f.is_finite() && *f > 0.).collect();
    let (dw, dd) = if kind == PlaceKind::Exterior { (30., 30.) } else { (10., 8.) };
    let (w, d) = (size.first().copied().unwrap_or(dw), size.get(1).copied().unwrap_or(dd));
    let mut ids = BTreeSet::new();
    let landmarks = list(p, "landmarks")
        .iter()
        .filter_map(|l| {
            let n = text(l, "name");
            if n.is_empty() {
                return None;
            }
            Some(Landmark {
                id: unique(format!("lm_{}", slug(&n)), &mut ids),
                kind: word(&text(l, "kind"), LandmarkKind::Unknown, LandmarkKind::Feature),
                pos: [num(l, "x").unwrap_or(0.), num(l, "z").unwrap_or(0.)],
                size: [num(l, "w").unwrap_or(1.), num(l, "d").unwrap_or(1.)],
                rot_deg: num(l, "rot").or_else(|| num(l, "rot_deg")).unwrap_or(0.),
                h_m: num(l, "h").unwrap_or(1.),
                name: n,
            })
        })
        .collect();
    let walls = list(p, "walls")
        .iter()
        .filter_map(|x| {
            let pt = |k: &str| -> Option<[f32; 2]> {
                let a = x.get(k)?.as_array()?;
                Some([a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32])
            };
            Some(Wall { a: pt("a")?, b: pt("b")?, h_m: num(x, "h").unwrap_or(3.) })
        })
        .collect();
    let mut sids = BTreeSet::new();
    let stations = list(p, "stations")
        .iter()
        .map(|s| {
            let n = text(s, "name");
            let overhead = s.get("overhead").and_then(Value::as_bool).unwrap_or(false);
            let n = if n.is_empty() { if overhead { "Overhead".to_string() } else { "Camera".to_string() } } else { n };
            Station {
                id: unique(if overhead { OVERHEAD_ID.to_string() } else { format!("st_{}", slug(&n)) }, &mut sids),
                name: n,
                pos: [num(s, "x").unwrap_or(0.), num(s, "height").or_else(|| num(s, "y")).unwrap_or(1.5), num(s, "z").unwrap_or(0.)],
                yaw_deg: num(s, "yaw").or_else(|| num(s, "yaw_deg")).unwrap_or(0.),
                pitch_deg: num(s, "pitch").or_else(|| num(s, "pitch_deg")).unwrap_or(if overhead { -90. } else { 0. }),
                lens_mm: num(s, "lens").or_else(|| num(s, "lens_mm")).map(|f| f.round().clamp(1., 2000.) as u16).unwrap_or(24),
                overhead,
            }
        })
        .collect();

    // Light states: objects or plain words; the old `key_light` shape fills what they leave out.
    let key = p.get("key_light").cloned().unwrap_or(Value::Null);
    let key_dir = num(&key, "dir_deg").unwrap_or(135.);
    let mut states: Vec<PlaceState> = Vec::new();
    for s in list(p, "states") {
        let (sid, sname, dir, kelvin, mood) = match s {
            Value::String(w) => (state_id(w), flat(w), None, None, String::new()),
            Value::Object(_) => {
                let n = text(s, "name");
                let i = text(s, "id");
                let sid = state_id(if i.is_empty() { &n } else { &i });
                (sid, if n.is_empty() { i } else { n }, num(s, "key_dir_deg"), num(s, "kelvin"), text(s, "mood"))
            }
            _ => continue,
        };
        if sid.is_empty() || states.iter().any(|x| x.id == sid) {
            continue;
        }
        let (k, m) = state_defaults(&sid);
        states.push(PlaceState {
            name: if sname.is_empty() { sid.clone() } else { sname },
            key_dir_deg: dir.unwrap_or(key_dir).rem_euclid(360.),
            kelvin: kelvin.filter(|k| (1000. ..=20000.).contains(k)).map(|k| k as u32).or_else(|| num(&key, "kelvin").map(|k| k.clamp(1000., 20000.) as u32)).unwrap_or(k),
            mood: if mood.is_empty() { m.into() } else { mood },
            id: sid,
        });
    }

    // Scenes the AI set here: validated against the script; their times become states.
    let mut scenes = Vec::new();
    for s in strings(p, "scenes") {
        if let Some(scene) = src.story.scenes.iter().find(|x| x.id == s) {
            scenes.push(Value::String(scene.id.clone()));
            let w = time_word(scene.time_of_day);
            if !states.iter().any(|x| x.id == w) {
                let (k, m) = state_defaults(w);
                states.push(PlaceState { id: w.into(), name: w.into(), key_dir_deg: key_dir.rem_euclid(360.), kelvin: k, mood: m.into() });
            }
        }
    }
    if states.is_empty() {
        let w = if kind == PlaceKind::Exterior { "day" } else { "indoor" };
        let (k, m) = state_defaults(w);
        states.push(PlaceState { id: w.into(), name: w.into(), key_dir_deg: key_dir.rem_euclid(360.), kelvin: k, mood: m.into() });
    }

    let mut extra = Map::new();
    let forces = p.get("forces").cloned().unwrap_or(Value::Null);
    let wind_deg = num(&forces, "wind_deg").map(|d| d.rem_euclid(360.));
    let moving = strings(&forces, "moving");
    extra.insert(EXTRA_FORCES.into(), json!({"wind_deg": wind_deg, "wind": text(&forces, "wind"), "moving": moving}));
    let mut panels = Vec::new();
    for x in list(p, "panels") {
        match checked_panel(src, x) {
            Some(at) => panels.push(json!({"chapter": at.chapter_id, "page": at.page, "panel": at.panel})),
            None => issues.push(issue("bible-panel", Severity::Info, format!("{name}: the AI named a panel this chapter does not have ({}); it was skipped.", short(x)))),
        }
    }
    panels.truncate(6);
    extra.insert(EXTRA_PANELS.into(), Value::Array(panels));
    extra.insert(EXTRA_SCENES.into(), Value::Array(scenes));

    Place {
        id,
        name,
        kind,
        description: text(p, "description"),
        plan: FloorPlan { w_m: w, d_m: d, walls, landmarks, north_deg: 0. },
        stations,
        states,
        extra,
        ..Default::default()
    }
}

/// Takes the AI's (scene id, place id) links out of the draft places.
fn take_scene_links(draft: &mut Draft) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for p in &mut draft.places {
        if let Some(Value::Array(a)) = p.extra.remove(EXTRA_SCENES) {
            out.extend(a.iter().filter_map(Value::as_str).map(|s| (s.to_string(), p.id.clone())));
        }
    }
    out
}

// ---------------------------------------------------------------- places on the plan

fn snap_lens(f: u16) -> u16 {
    if f == 0 {
        return 24;
    }
    LENSES.iter().copied().min_by_key(|l| (*l as i32 - f as i32).abs()).unwrap_or(24)
}

fn yaw_gap(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.);
    if d > 180. {
        360. - d
    } else {
        d
    }
}

/// Two stations show the same view (≤ 1 m apart, ≤ 25° of yaw; any two overheads).
fn same_view(a: &Station, b: &Station) -> bool {
    if a.overhead || b.overhead {
        return a.overhead && b.overhead;
    }
    let d = ((a.pos[0] - b.pos[0]).powi(2) + (a.pos[2] - b.pos[2]).powi(2)).sqrt();
    d <= 1. && yaw_gap(a.yaw_deg, b.yaw_deg) <= 25. && (a.pos[1] - b.pos[1]).abs() <= 1.
}

/// The overhead station of a plan: centred, as high as the plan is large, looking straight down.
pub fn overhead_station(plan: &FloorPlan) -> Station {
    Station {
        id: OVERHEAD_ID.into(),
        name: "Overhead, looking straight down".into(),
        pos: [0., plan.w_m.max(plan.d_m).clamp(3., 300.), 0.],
        yaw_deg: 0.,
        pitch_deg: -90.,
        lens_mm: 24,
        overhead: true,
    }
}

/// Makes a plan usable: sizes, landmarks, walls and stations inside it, stations far outside
/// dropped, lenses snapped, ids unique, at most [`MAX_STATIONS`] stations with exactly one
/// overhead. `fill` (new places) also adds standard views up to [`MIN_STATIONS`] and the outer
/// walls of an interior.
fn tidy_place(p: &mut Place, fill: bool, issues: &mut Vec<Issue>) {
    let fix = |v: f32, lo: f32, hi: f32, def: f32| if v.is_finite() { v.clamp(lo, hi) } else { def };
    let (dw, dd) = if p.kind == PlaceKind::Exterior { (30., 30.) } else { (10., 8.) };
    p.plan.w_m = if p.plan.w_m.is_finite() && p.plan.w_m > 0. { p.plan.w_m.clamp(1., 1000.) } else { dw };
    p.plan.d_m = if p.plan.d_m.is_finite() && p.plan.d_m > 0. { p.plan.d_m.clamp(1., 1000.) } else { dd };
    let (pw, pd) = (p.plan.w_m, p.plan.d_m);
    let (hw, hd) = (pw / 2., pd / 2.);
    let big = pw.max(pd);

    let mut ids = BTreeSet::new();
    for l in &mut p.plan.landmarks {
        l.size = [fix(l.size[0], 0.05, pw, 1.), fix(l.size[1], 0.05, pd, 1.)];
        let (mx, mz) = ((hw - l.size[0] / 2.).max(0.), (hd - l.size[1] / 2.).max(0.));
        l.pos = [fix(l.pos[0], -mx, mx, 0.), fix(l.pos[1], -mz, mz, 0.)];
        l.h_m = fix(l.h_m, 0., 500., 1.);
        l.rot_deg = if l.rot_deg.is_finite() { l.rot_deg.rem_euclid(360.) } else { 0. };
        let base = if l.id.trim().is_empty() { format!("lm_{}", slug(&l.name)) } else { l.id.clone() };
        l.id = unique(base, &mut ids);
    }
    for w in &mut p.plan.walls {
        for pt in [&mut w.a, &mut w.b] {
            *pt = [fix(pt[0], -hw, hw, 0.), fix(pt[1], -hd, hd, 0.)];
        }
        w.h_m = fix(w.h_m, 0.1, 500., 3.);
    }
    if fill && p.kind != PlaceKind::Exterior && p.plan.walls.is_empty() {
        let h = p.plan.landmarks.iter().map(|l| l.h_m + 0.5).fold(3f32, f32::max).min(30.);
        let c = [[-hw, -hd], [hw, -hd], [hw, hd], [-hw, hd]];
        p.plan.walls = (0..4).map(|i| Wall { a: c[i], b: c[(i + 1) % 4], h_m: h }).collect();
    }
    if !p.plan.north_deg.is_finite() {
        p.plan.north_deg = 0.;
    }

    // Stations: clamp the ones near the plan inside it, drop the ones far outside.
    let (far_w, far_d) = ((0.25 * pw).max(2.), (0.25 * pd).max(2.));
    let mut kept: Vec<Station> = Vec::new();
    for mut s in std::mem::take(&mut p.stations) {
        let [x, y, z] = s.pos;
        if !(x.is_finite() && z.is_finite()) || x.abs() > hw + far_w || z.abs() > hd + far_d {
            issues.push(issue(
                "bible-station",
                Severity::Info,
                format!("{}: the camera station \"{}\" stood outside the floor plan and was left out.", p.name, s.name),
            ));
            continue;
        }
        s.overhead = s.overhead || (s.pitch_deg.is_finite() && s.pitch_deg <= -75.);
        let top = (2. * big).max(3.);
        s.pos = [x.clamp(-hw, hw), fix(y, 0.1, top, 1.5), z.clamp(-hd, hd)];
        s.yaw_deg = if s.yaw_deg.is_finite() { s.yaw_deg.rem_euclid(360.) } else { 0. };
        s.pitch_deg = fix(s.pitch_deg, -90., 90., 0.);
        if s.overhead {
            s.pitch_deg = -90.;
            if s.pos[1] < big * 0.5 {
                s.pos[1] = big.clamp(3., top);
            }
        }
        s.lens_mm = snap_lens(s.lens_mm);
        if s.name.trim().is_empty() {
            s.name = if s.overhead { "Overhead".into() } else { "Camera".into() };
        }
        if kept.iter().any(|k| same_view(k, &s)) {
            continue;
        }
        kept.push(s);
    }
    // One overhead (the first), then the others in order, at most MAX_STATIONS.
    let overhead = kept.iter().position(|s| s.overhead).map(|i| kept.remove(i)).unwrap_or_else(|| overhead_station(&p.plan));
    kept.retain(|s| !s.overhead);
    kept.truncate(MAX_STATIONS - 1);
    kept.insert(0, overhead);
    if fill {
        let h = 1.5;
        let inset = |v: f32| (v - 0.5).max(0.);
        let standard = [
            ("Wide, from the south side looking north", [0., h, inset(hd)], 0., 0.),
            ("Reverse, from the north side looking south", [0., h, -inset(hd)], 180., 0.),
            ("From the west side looking east", [-inset(hw), h, 0.], 90., 0.),
            ("From the east side looking west", [inset(hw), h, 0.], 270., 0.),
            ("Low, from the south-west corner", [-inset(hw), 0.6, inset(hd)], 45., 8.),
        ];
        for (name, pos, yaw, pitch) in standard {
            if kept.len() >= MIN_STATIONS {
                break;
            }
            let s = Station { id: String::new(), name: name.into(), pos, yaw_deg: yaw, pitch_deg: pitch, lens_mm: 24, overhead: false };
            if !kept.iter().any(|k| !k.overhead && ((k.pos[0] - s.pos[0]).powi(2) + (k.pos[2] - s.pos[2]).powi(2)).sqrt() <= 1.5 && yaw_gap(k.yaw_deg, s.yaw_deg) <= 30.) {
                kept.push(s);
            }
        }
    }
    let mut ids = BTreeSet::new();
    for s in &mut kept {
        let base = if s.overhead && s.id.trim().is_empty() {
            OVERHEAD_ID.to_string()
        } else if s.id.trim().is_empty() {
            format!("st_{}", slug(&s.name))
        } else {
            s.id.clone()
        };
        s.id = unique(base, &mut ids);
    }
    p.stations = kept;

    // States: unique ids, sane numbers, at least one.
    let mut ids = BTreeSet::new();
    p.states.retain(|s| !s.id.trim().is_empty() && ids.insert(s.id.clone()));
    for s in &mut p.states {
        s.key_dir_deg = if s.key_dir_deg.is_finite() { s.key_dir_deg.rem_euclid(360.) } else { 135. };
        if s.kelvin == 0 {
            s.kelvin = state_defaults(&s.id).0;
        }
        if s.name.trim().is_empty() {
            s.name = s.id.clone();
        }
    }
    if p.states.is_empty() {
        let w = if p.kind == PlaceKind::Exterior { "day" } else { "indoor" };
        let (k, m) = state_defaults(w);
        p.states.push(PlaceState { id: w.into(), name: w.into(), key_dir_deg: 135., kelvin: k, mood: m.into() });
    }
}

/// Renames a draft place's stations and landmarks that match the stored place's (same view,
/// same name) to the stored ids, so the merge fills them instead of adding twins; drops new
/// stations past [`MAX_STATIONS`].
fn align_place(cur: &Place, draft: &mut Place) {
    let mut room = MAX_STATIONS.saturating_sub(cur.stations.len());
    draft.stations.retain_mut(|s| {
        if let Some(c) = cur.stations.iter().find(|c| c.id == s.id || same_view(c, s)) {
            s.id = c.id.clone();
            return true;
        }
        if room == 0 {
            return false;
        }
        room -= 1;
        true
    });
    for l in &mut draft.plan.landmarks {
        let near = |c: &Landmark| c.kind == l.kind && ((c.pos[0] - l.pos[0]).powi(2) + (c.pos[1] - l.pos[1]).powi(2)).sqrt() <= 0.75;
        if let Some(c) = cur.plan.landmarks.iter().find(|c| c.name.eq_ignore_ascii_case(&l.name) || near(c)) {
            l.id = c.id.clone();
        }
    }
    for s in &mut draft.states {
        if let Some(c) = cur.states.iter().find(|c| c.id == s.id || c.name.eq_ignore_ascii_case(&s.name)) {
            s.id = c.id.clone();
        }
    }
}

/// Adds the overhead station to a stored place that lost it (unless the user locked the stations).
fn ensure_overhead(p: &mut Place) -> bool {
    if p.stations.iter().any(|s| s.overhead) || crate::merge::is_locked("/stations", &p.locked) {
        return false;
    }
    let mut s = overhead_station(&p.plan);
    let mut ids: BTreeSet<String> = p.stations.iter().map(|s| s.id.clone()).collect();
    s.id = unique(OVERHEAD_ID.into(), &mut ids);
    p.stations.insert(0, s);
    true
}

// ---------------------------------------------------------------- merge

/// Fields an AI merge never fills, at any depth (cards, the user's bookkeeping, the crops).
const NEVER_FILL: [&str; 10] = ["status", "locked", "edited", "sheets", "sheet", "plates", "frame", "crops", "schema", "takes"];
/// Numbers for which 0 means "not set yet".
const ZERO_IS_EMPTY: [&str; 6] = ["height_m", "w_m", "d_m", "h_m", "kelvin", "lens_mm"];
/// Booleans for which false means "not set yet".
const FALSE_IS_EMPTY: [&str; 1] = ["held"];

fn is_gap(key: &str, v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        Value::Number(n) => ZERO_IS_EMPTY.contains(&key) && n.as_f64().is_some_and(|f| f == 0.),
        Value::Bool(b) => FALSE_IS_EMPTY.contains(&key) && !b,
        Value::Array(a) => a.is_empty(),
        Value::Object(m) => m.is_empty(),
    }
}

/// How an array element is recognised across runs: its id, else its name, else its source panel.
fn element_key(v: &Value) -> String {
    let get = |k: &str| v.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    if let Some(id) = get("id") {
        return format!("id:{id}");
    }
    if let Some(n) = get("name").or_else(|| get("character")) {
        return format!("name:{}", n.to_lowercase());
    }
    if v.get("chapter").is_some() {
        return format!("at:{}/{}/{}", text(v, "chapter"), text(v, "page"), text(v, "panel"));
    }
    match v {
        Value::String(s) => format!("s:{}", s.trim().to_lowercase()),
        other => crate::hash::canonical(other),
    }
}

/// Items the AI offered, per list path (see [`EXTRA_OFFERED`]).
type Offered = BTreeMap<String, BTreeSet<String>>;

/// Lists whose items the user edits (not the engine's own signals kept in `extra`).
fn user_list(path: &str) -> bool {
    let first = path.trim_start_matches('/').split('/').next().unwrap_or("");
    ![EXTRA_BEST_PANELS, EXTRA_PANELS, EXTRA_VOICE, EXTRA_SPEAKS, EXTRA_FORCES, EXTRA_SCENES, EXTRA_OFFERED].contains(&first) && !NEVER_FILL.contains(&first)
}

fn is_things(x: &[Value]) -> bool {
    x.iter().all(|e| e.is_object() || e.is_string())
}

/// Records every item of every list of things in `v` (paths as [`fill_gaps`] builds them).
fn offered_in(path: &str, v: &Value, out: &mut Offered) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if NEVER_FILL.contains(&k.as_str()) || k == EXTRA_OFFERED {
                    continue;
                }
                offered_in(&format!("{path}/{k}"), x, out);
            }
        }
        Value::Array(a) if !a.is_empty() && is_things(a) && user_list(path) => {
            let keys = out.entry(path.to_string()).or_default();
            keys.extend(a.iter().map(element_key));
            for e in a {
                offered_in(&format!("{path}/*"), e, out);
            }
        }
        _ => {}
    }
}

/// What an entity's AI offered so far (empty for one stored before this was kept).
fn offered_of(v: &Value) -> Offered {
    v.get(EXTRA_OFFERED).and_then(|o| serde_json::from_value(o.clone()).ok()).unwrap_or_default()
}

/// Stores what the AI offered in a new entity's `extra`.
fn note_offered(extra: &mut Map<String, Value>, entity: &Value) {
    let mut offered = Offered::new();
    offered_in("", entity, &mut offered);
    if !offered.is_empty() {
        if let Ok(v) = serde_json::to_value(offered) {
            extra.insert(EXTRA_OFFERED.into(), v);
        }
    }
}

/// `cur` with its gaps filled from `ai`: empty leaves take the AI's value, lists of things gain the
/// AI's new items (matched items fill their own gaps), everything set stays as it is. An item the
/// AI offered before (`before`, by list path) that is no longer in the list was removed by the
/// user and is not added again.
fn fill_gaps(before: &Offered, path: &str, key: &str, cur: &Value, ai: &Value) -> Value {
    match (cur, ai) {
        (Value::Object(c), Value::Object(a)) => {
            let mut out = c.clone();
            for (k, av) in a {
                if NEVER_FILL.contains(&k.as_str()) || k == EXTRA_OFFERED {
                    continue;
                }
                let v = match c.get(k) {
                    Some(cv) => fill_gaps(before, &format!("{path}/{k}"), k, cv, av),
                    None => av.clone(),
                };
                out.insert(k.clone(), v);
            }
            Value::Object(out)
        }
        (Value::Array(c), Value::Array(a)) => {
            let removed = |e: &Value| user_list(path) && before.get(path).is_some_and(|keys| keys.contains(&element_key(e)));
            if c.is_empty() {
                if !is_things(a) {
                    return ai.clone();
                }
                return Value::Array(a.iter().filter(|e| !removed(e)).cloned().collect());
            }
            if !is_things(c) || !is_things(a) {
                // Coordinates and other number lists are one value.
                return cur.clone();
            }
            let mut out = c.clone();
            for e in a {
                let k = element_key(e);
                match out.iter().position(|x| element_key(x) == k) {
                    Some(i) => out[i] = fill_gaps(before, &format!("{path}/*"), key, &out[i], e),
                    None if removed(e) => {}
                    None => out.push(e.clone()),
                }
            }
            Value::Array(out)
        }
        (c, a) => {
            if is_gap(key, c) {
                a.clone()
            } else {
                c.clone()
            }
        }
    }
}

/// `cur` with `ai` filled into its gaps, keeping every locked field, and what the AI offered
/// noted (so a list item the user removes stays removed). Returns the entity and whether
/// anything beyond that note changed.
fn fill_entity<T: Serialize + DeserializeOwned>(cur: &T, ai: &T, locked: &BTreeSet<String>) -> Result<(T, bool)> {
    let mut cv = serde_json::to_value(cur)?;
    let av = serde_json::to_value(ai)?;
    let before = offered_of(&cv);
    let filled = fill_gaps(&before, "", "", &cv, &av);
    let snapshot = cv.clone();
    merge_unlocked(&mut cv, &filled, locked);
    let changed = cv != snapshot;
    let mut offered = before.clone();
    offered_in("", &av, &mut offered);
    if offered != before {
        if let (Some(o), Ok(v)) = (cv.as_object_mut(), serde_json::to_value(&offered)) {
            o.insert(EXTRA_OFFERED.into(), v);
        }
    }
    Ok((serde_json::from_value(cv)?, changed))
}

fn find_character<'a>(all: &'a [Character], c: &Character) -> Option<&'a Character> {
    let by_brain = c.brain_id.as_ref().and_then(|b| all.iter().find(|e| e.brain_id.as_ref() == Some(b)));
    let by_id = || all.iter().find(|e| !c.id.is_empty() && e.id == c.id);
    let named = |n: &str| all.iter().find(|e| e.name.eq_ignore_ascii_case(n) || e.aliases.iter().any(|a| a.eq_ignore_ascii_case(n)));
    // A renamed card keeps the id its first name gave it.
    let slugged = || {
        let id = format!("ch_{}", slug(&c.name));
        all.iter().find(|e| c.brain_id.is_none() && e.brain_id.is_none() && !c.name.trim().is_empty() && e.id == id)
    };
    by_brain.or_else(by_id).or_else(|| named(&c.name)).or_else(slugged)
}

/// Merges a draft (characters keyed by brain id, else name); clamps coordinates into the plan,
/// drops stations outside it, adds an overhead station when none exists.
///
/// New entities are saved as Draft. Stored ones only gain what is new and get their empty fields
/// filled: a value already set, a locked field, a card (sheets, plates, the style frame) and the
/// crops are never changed. The style line falls back to [`DEFAULT_STYLE_LINE`].
pub fn merge(store: &Store, draft: Draft) -> Result<MergeReport> {
    // The read and every save under the project's bible lock: the Bible screen's commit and a
    // sheet job's card update never land in between and get overwritten.
    store.with_bible(|| merge_locked(store, draft))
}

fn merge_locked(store: &Store, draft: Draft) -> Result<MergeReport> {
    let bible = store.bible()?;
    let mut report = MergeReport::default();

    // Characters.
    let mut taken: BTreeSet<String> = bible.characters.iter().map(|c| c.id.clone()).collect();
    let mut done: BTreeSet<String> = BTreeSet::new();
    for mut c in draft.characters {
        match find_character(&bible.characters, &c) {
            Some(cur) => {
                if !done.insert(cur.id.clone()) {
                    report.issues.push(issue("bible-duplicate", Severity::Info, format!("{} was proposed twice; the first was used.", c.name)));
                    continue;
                }
                c.id = cur.id.clone();
                if cur.brain_id.is_some() {
                    c.brain_id = cur.brain_id.clone();
                }
                let (new, changed) = fill_entity(cur, &c, &cur.locked)?;
                if new != *cur {
                    store.save_character(&new)?;
                }
                if changed {
                    report.updated.push(new.id);
                }
            }
            None => {
                if c.id.trim().is_empty() || bible.character(&c.id).is_some() {
                    let base = match &c.brain_id {
                        Some(b) => mv_core::paths::safe_name(b),
                        None => format!("ch_{}", slug(&c.name)),
                    };
                    c.id = unique(base, &mut taken);
                } else {
                    taken.insert(c.id.clone());
                }
                if !done.insert(c.id.clone()) {
                    continue;
                }
                fresh_character(&mut c);
                let v = serde_json::to_value(&c)?;
                note_offered(&mut c.extra, &v);
                store.save_character(&c)?;
                report.added.push(c.id);
            }
        }
    }

    // Places.
    let mut taken: BTreeSet<String> = bible.places.iter().map(|p| p.id.clone()).collect();
    for mut p in draft.places {
        let existing = bible.places.iter().find(|e| (!p.id.is_empty() && e.id == p.id) || e.name.eq_ignore_ascii_case(&p.name));
        match existing {
            Some(cur) => {
                p.id = cur.id.clone();
                tidy_place(&mut p, false, &mut report.issues);
                align_place(cur, &mut p);
                let (mut new, changed) = fill_entity(cur, &p, &cur.locked)?;
                let added_overhead = ensure_overhead(&mut new);
                if new != *cur || added_overhead {
                    store.save_place(&new)?;
                }
                if changed || added_overhead {
                    report.updated.push(new.id);
                }
            }
            None => {
                if p.id.trim().is_empty() {
                    p.id = format!("pl_{}", slug(&p.name));
                }
                p.id = unique(p.id.clone(), &mut taken);
                tidy_place(&mut p, true, &mut report.issues);
                p.extra.remove(EXTRA_SCENES);
                p.status = Status::Draft;
                p.locked.clear();
                p.sheet = Default::default();
                p.plates.clear();
                let v = serde_json::to_value(&p)?;
                note_offered(&mut p.extra, &v);
                store.save_place(&p)?;
                report.added.push(p.id);
            }
        }
    }

    // Props.
    let mut taken: BTreeSet<String> = bible.props.iter().map(|p| p.id.clone()).collect();
    for mut p in draft.props {
        let existing = bible.props.iter().find(|e| (!p.id.is_empty() && e.id == p.id) || e.name.eq_ignore_ascii_case(&p.name));
        match existing {
            Some(cur) => {
                p.id = cur.id.clone();
                let (new, changed) = fill_entity(cur, &p, &cur.locked)?;
                if new != *cur {
                    store.save_prop(&new)?;
                }
                if changed {
                    report.updated.push(new.id);
                }
            }
            None => {
                if p.id.trim().is_empty() {
                    p.id = format!("pr_{}", slug(&p.name));
                }
                p.id = unique(p.id.clone(), &mut taken);
                p.status = Status::Draft;
                p.locked.clear();
                p.sheets.clear();
                let v = serde_json::to_value(&p)?;
                note_offered(&mut p.extra, &v);
                store.save_prop(&p)?;
                report.added.push(p.id);
            }
        }
    }

    // Style: fill its gaps; the line and the negatives always have their defaults.
    let cur = bible.style.clone();
    let (mut style, _) = match &draft.style {
        Some(s) => fill_entity(&cur, s, &cur.locked)?,
        None => (cur.clone(), false),
    };
    if style.line.trim().is_empty() && !crate::merge::is_locked("/line", &style.locked) {
        style.line = DEFAULT_STYLE_LINE.into();
    }
    if !crate::merge::is_locked("/negative", &style.locked) {
        for n in DEFAULT_NEGATIVE {
            if !style.negative.iter().any(|x| x.eq_ignore_ascii_case(n)) {
                style.negative.push(n.into());
            }
        }
    }
    if style != cur {
        store.save_style(&style)?;
        report.updated.push("style".into());
    }
    Ok(report)
}

/// A character entering the bible: Draft, no locks, no sheets; brain-less ones keep their crops
/// empty until the crop step runs.
fn fresh_character(c: &mut Character) {
    c.status = Status::Draft;
    c.locked.clear();
    c.sheets.clear();
    if c.look.height_m <= 0. || !c.look.height_m.is_finite() {
        c.look.height_m = 1.7;
    }
}

// ---------------------------------------------------------------- gate 1

fn approved(card: &crate::model::Card) -> bool {
    card.status == Status::Approved && card.selected.is_some()
}

/// The character's sheet of `kind` is approved with a picture.
pub fn sheet_approved(c: &Character, kind: &SheetKind) -> bool {
    c.sheets.iter().any(|s| &s.kind == kind && approved(&s.card))
}

/// The characters gate 1 asks sheets of: the hero, allies and threats (or everyone when no role
/// was set), never a rejected one.
pub fn main_characters(bible: &BibleIndex) -> Vec<&Character> {
    let all: Vec<&Character> = bible.characters.iter().filter(|c| c.status != Status::Rejected).collect();
    let main: Vec<&Character> = all.iter().copied().filter(|c| matches!(c.role, Role::Hero | Role::Ally | Role::Threat)).collect();
    if main.is_empty() {
        all
    } else {
        main
    }
}

fn and_list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [a] => a.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// What still blocks "Approve the bible" (gate 1), in words; empty = it can be approved.
///
/// Gate 1 needs: the style line; at least one character and one place; every main character's
/// master design, face sheet and turnaround approved; every place's environment master approved
/// and an approved plate from its overhead station.
pub fn gate_missing(bible: &BibleIndex) -> Vec<String> {
    let mut out = Vec::new();
    if bible.style.line.trim().is_empty() {
        out.push("Write the style line (Style tab).".to_string());
    }
    let mains = main_characters(bible);
    if mains.is_empty() {
        out.push("The bible has no characters yet: build the bible first.".to_string());
    }
    for c in mains {
        let need: Vec<&str> = [(SheetKind::Master, "master design"), (SheetKind::Face, "face sheet"), (SheetKind::Turnaround, "turnaround")]
            .iter()
            .filter(|(k, _)| !sheet_approved(c, k))
            .map(|(_, n)| *n)
            .collect();
        if !need.is_empty() {
            out.push(format!("{}: approve the {}.", display(&c.name, "An unnamed character"), and_list(&need)));
        }
    }
    let places: Vec<&Place> = bible.places.iter().filter(|p| p.status != Status::Rejected).collect();
    if places.is_empty() {
        out.push("The bible has no places yet: build the bible first.".to_string());
    }
    for p in places {
        let name = display(&p.name, "An unnamed place");
        if !approved(&p.sheet) {
            out.push(format!("{name}: approve the environment master."));
        }
        let overheads: Vec<&Station> = p.stations.iter().filter(|s| s.overhead).collect();
        if overheads.is_empty() {
            out.push(format!("{name}: add an overhead camera station."));
        } else if !p.plates.iter().any(|pl| overheads.iter().any(|s| s.id == pl.station) && approved(&pl.card)) {
            out.push(format!("{name}: approve the overhead plate."));
        }
    }
    out
}

/// The name the user sees; plain words ("An unnamed character") when there is none, never an id.
fn display(name: &str, unnamed: &str) -> String {
    if name.trim().is_empty() {
        unnamed.to_string()
    } else {
        name.trim().to_string()
    }
}
