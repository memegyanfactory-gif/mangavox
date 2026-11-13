//! What Creative asks the AI: the director that chats with the user, the series bible and cast,
//! a chapter's page script, and the image prompts for character sheets, expression sheets,
//! locations, whole pages and single panels. Every image prompt carries the same art direction
//! and the character's fixed look, so the manga stays on model from page 1 to page 200.
use std::fmt::Write as _;

use serde::Deserialize;
use serde_json::Value;

use crate::layout::TEMPLATES;
use crate::project::{CastMember, Chapter, Location, Page, Panel, Project, Proposal};

/// The manga craft every writing step follows.
const CRAFT: &str = "Write like a professional weekly-manga team (mangaka plus editor):
- Pacing: each page ends on a reason to turn it; a chapter opens with a hook and ends on a cliffhanger or a strong beat.
- Panels: 3-6 per page, varied shots (establishing, wide, medium, close-up, extreme close-up, insert), with a full-page splash only for the biggest moments. Close-ups carry emotion, wides carry place and action.
- Dialogue: short balloons (at most ~18 words each, usually fewer), at most 3 balloons per panel, voice that fits each character; let pictures carry what words need not say. Use narration boxes sparingly.
- Sound effects are part of the art (ZOOM, DOKUN, GOGOGO, BAM).
- Introduce characters gradually: a cast of 30+ arrives over many chapters, two or three new faces at a time, each with a memorable entrance.
- Keep continuity: outfits, injuries, items and time of day carry from panel to panel.";

/// The director's instructions: it talks to the user and edits the project with ops.
pub fn director_system(p: &Project, library: &[String]) -> String {
    let mut s = String::from(
        "You are the head writer and editor of an original manga being made in MangaVox Creative. The user describes what they want; you shape it into a real series, then the app paints it with an image model.\n\n",
    );
    s.push_str(CRAFT);
    s.push_str("\n\nAnswer with ONE JSON object and nothing else:\n");
    s.push_str(r#"{"reply": "what you say to the user (warm, concise, concrete; markdown allowed)", "ops": [ ... ]}"#);
    s.push_str(
        "\n\nOps (use only what the user's message calls for; none is fine for a question):
- {\"op\":\"create_series\",\"brief\":\"the full idea in your words: inspiration, the user's own characters, their twist\",\"inspirations\":[\"Dr. Stone\"]} — start or restart the series; the app then writes the bible, the whole cast and the chapter outline.
- {\"op\":\"revise_series\",\"instruction\":\"what to change\"} — change the bible, the cast or the outline (add / remove / rename characters, new twist, new arc).
- {\"op\":\"write_chapter\",\"chapter\":1,\"note\":\"anything the user wants in it\"} — write (or rewrite) that chapter's page-by-page script.
- {\"op\":\"edit_page\",\"chapter\":1,\"page\":3,\"instruction\":\"what to change\"} — rewrite one page's script.
- {\"op\":\"propose\",\"kind\":\"sheets\",\"names\":[\"...\"]} — offer to paint character model sheets and expression sheets (always before a character's first painted page).
- {\"op\":\"propose\",\"kind\":\"pages\",\"chapter\":1,\"pages\":[]} — offer to paint pages (empty list = the whole chapter).
- {\"op\":\"propose\",\"kind\":\"video\",\"chapter\":1} — offer to animate the chapter into a video.
- {\"op\":\"settings\",\"color\":false,\"rtl\":true,\"pages_per_chapter\":18} — reading order, colour, chapter length when the user asks.
Proposals spend the user's image or video credits, so the app shows each as a card the user confirms, with the model to use. When the user's request is clear, act (create the series, write the chapter) instead of asking many questions; ask at most one short question when something essential is missing.\n\n",
    );
    s.push_str("## The project now\n");
    s.push_str(&state(p));
    if !library.is_empty() {
        let _ = writeln!(s, "\n## Manga the user has downloaded (their pages can serve as style references)\n{}", library.join("; "));
    }
    s
}

/// A compact picture of the project for the AI.
pub fn state(p: &Project) -> String {
    let mut s = String::new();
    if p.bible.logline.is_empty() {
        let _ = writeln!(s, "No series yet.");
        if !p.brief.is_empty() {
            let _ = writeln!(s, "Idea so far: {}", p.brief);
        }
        return s;
    }
    let b = &p.bible;
    let _ = writeln!(s, "Title: {}\nLogline: {}\nGenre: {} · Tone: {}\nTwist: {}\nArt style: {}\nWorld: {}", p.title, b.logline, b.genre, b.tone, b.twist, b.art_style, b.world);
    if !p.inspirations.is_empty() {
        let _ = writeln!(s, "Inspired by: {}", p.inspirations.join(", "));
    }
    let _ = writeln!(s, "Settings: {} pages per chapter, {}, {}.", p.settings.pages_per_chapter, if p.settings.color { "colour" } else { "black and white" }, if p.settings.rtl { "right-to-left" } else { "left-to-right" });
    let _ = writeln!(s, "\nCast ({}):", p.cast.len());
    for c in &p.cast {
        let _ = writeln!(s, "- {} ({}, from chapter {}){}", c.name, c.role, c.intro_chapter, if c.has_sheets() { " [sheets painted]" } else { "" });
    }
    let _ = writeln!(s, "\nChapters:");
    for c in &p.chapters {
        let _ = writeln!(s, "- {}. {} — {} [{} pages scripted, {} painted{}]", c.number, c.title, c.summary, c.pages.len(), c.painted(), if c.video.is_some() { ", video made" } else { "" });
    }
    s
}

/// One op from the director.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    CreateSeries { brief: String, inspirations: Vec<String> },
    ReviseSeries { instruction: String },
    WriteChapter { chapter: u32, note: String },
    EditPage { chapter: u32, page: u32, instruction: String },
    Propose(Proposal),
    Settings { color: Option<bool>, rtl: Option<bool>, pages_per_chapter: Option<u32> },
}

fn num(v: &Value, k: &str) -> u32 {
    match v.get(k) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.).max(0.) as u32,
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

fn text(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string()
}

fn strings(v: &Value, k: &str) -> Vec<String> {
    v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect()).unwrap_or_default()
}

/// The director's answer: what to say, and the ops (unknown ones are skipped).
pub fn parse_director(v: &Value) -> (String, Vec<Op>) {
    let reply = text(v, "reply");
    let mut ops = vec![];
    for o in v.get("ops").and_then(Value::as_array).into_iter().flatten() {
        let op = match o.get("op").and_then(Value::as_str).unwrap_or("") {
            "create_series" => Op::CreateSeries { brief: text(o, "brief"), inspirations: strings(o, "inspirations") },
            "revise_series" => Op::ReviseSeries { instruction: text(o, "instruction") },
            "write_chapter" => Op::WriteChapter { chapter: num(o, "chapter").max(1), note: text(o, "note") },
            "edit_page" => Op::EditPage { chapter: num(o, "chapter").max(1), page: num(o, "page").max(1), instruction: text(o, "instruction") },
            "propose" => match o.get("kind").and_then(Value::as_str).unwrap_or("") {
                "sheets" => Op::Propose(Proposal::Sheets { names: strings(o, "names"), locations: strings(o, "locations") }),
                "pages" => Op::Propose(Proposal::Pages { chapter: num(o, "chapter").max(1), pages: o.get("pages").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_u64().map(|n| n as u32)).collect()).unwrap_or_default() }),
                "video" => Op::Propose(Proposal::Video { chapter: num(o, "chapter").max(1) }),
                _ => continue,
            },
            "settings" => Op::Settings { color: o.get("color").and_then(Value::as_bool), rtl: o.get("rtl").and_then(Value::as_bool), pages_per_chapter: o.get("pages_per_chapter").map(|_| num(o, "pages_per_chapter")).filter(|n| (4..=60).contains(n)) },
            _ => continue,
        };
        ops.push(op);
    }
    (reply, ops)
}

const SERIES_SHAPE: &str = r#"{
  "title": "series title",
  "bible": {"logline": "", "genre": "", "tone": "", "themes": "", "twist": "the user's spin, stated plainly",
            "art_style": "a precise art direction for every image: line weight, inking, screentone use, how faces/eyes/hair are drawn, anatomy, backgrounds, lettering feel; name the influences (e.g. 'in the spirit of Boichi's Dr. Stone: crisp detailed linework, heavy dramatic shading')",
            "world": "setting, era, rules, the places scenes happen"},
  "cast": [{"name": "", "role": "protagonist|deuteragonist|antagonist|main|supporting|minor", "age": "", "intro_chapter": 1,
            "personality": "", "appearance": "face shape, eyes, eyebrows, hair (style, length, colour/tone), skin tone, height, build, marks — what must never change",
            "outfits": [{"name": "default", "look": "every garment, colour/tone, shoes, accessories"}],
            "signature": "props, poses, tells"}],
  "locations": [{"name": "", "look": "architecture, landmarks, light, mood"}],
  "chapters": [{"number": 1, "title": "", "summary": "what happens, who is introduced"}]
}"#;

/// The bible, the cast and the outline from the user's idea. `inspiration_notes` are what
/// MangaVox already knows of the inspiring manga (its characters and story from Studio).
pub fn series(p: &Project, brief: &str, inspiration_notes: &str) -> (String, String) {
    let system = format!(
        "You create original manga series. {CRAFT}\n\nAnswer with ONE JSON object, no other text, in this shape:\n{SERIES_SHAPE}\n\nRules: the series is ORIGINAL (no copied names, characters or plot), but it should feel like its inspirations: same genre energy, art direction and storytelling rhythm. The user's own characters are the leads, exactly as they describe them. Plan a cast that can grow past 30 named characters over the run, each with a distinct silhouette and face, and spread their intro_chapter across chapters (2-3 new faces per chapter after the first). Outline at least 8 chapters. Appearance and outfit text must be concrete enough that an artist could draw the same person a hundred times."
    );
    let mut user = format!("The user's idea:\n{brief}\n");
    let system = match &p.adapts {
        Some(title) => format!(
            "You adapt the user's own downloaded manga \"{title}\" into a live-action series for their private use. {CRAFT}\n\nAnswer with ONE JSON object, no other text, in this shape:\n{SERIES_SHAPE}\n\nRules: this is a FAITHFUL adaptation: keep the manga's own characters (their names, personalities and every trait of their look: hair, eyes, build, marks, outfits) and its story, chapter by chapter, from the notes below. Describe each appearance as a real human being would look with those traits (real hair, real skin, real clothes), concrete enough that a casting sheet could be shot of them. Sort the cast by how much of the story is theirs, using \"role\": the one or two characters the story is truly about are \"protagonist\" / \"deuteragonist\" (never more than two); up to about 19 main characters are \"antagonist\" or \"main\"; those who come back now and then are \"supporting\"; one-off faces are \"minor\" (list only the ones a scene needs). Order the cast leads first. bible.art_style describes the live-action look (lighting, lens, colour grade, production design), not a drawing style. Outline the chapters the notes cover."
        ),
        None => system,
    };
    if !inspiration_notes.trim().is_empty() {
        let _ = write!(user, "\n{}:\n{inspiration_notes}\n", if p.adapts.is_some() { "What Studio read of the manga (its characters and story): adapt this" } else { "What we know of the inspiring manga (for feel only, do not copy)" });
    }
    if !p.cast.is_empty() {
        let _ = write!(user, "\nCharacters the user already added (keep them as leads):\n");
        for c in &p.cast {
            let _ = writeln!(user, "- {}: {} {}", c.name, c.appearance, c.personality);
        }
    }
    let _ = write!(user, "\nChapters are about {} pages. {}", p.settings.pages_per_chapter, if p.settings.live_action { "Each page is a storyboard of film scenes: every panel is one live-action shot with its spoken lines and how it moves." } else if p.settings.color { "Full colour." } else { "Black and white with screentone, like a printed weekly manga." });
    (system, user)
}

/// The same shape, revised.
pub fn revise(p: &Project, instruction: &str) -> (String, String) {
    let system = format!("You edit an original manga series. {CRAFT}\n\nAnswer with ONE JSON object, no other text, the WHOLE series after the change, in this shape:\n{SERIES_SHAPE}\n\nKeep everything the instruction does not touch exactly as it is (same names, same appearance text), so painted sheets stay valid.");
    let current = serde_json::json!({ "title": p.title, "bible": p.bible, "cast": p.cast.iter().map(|c| serde_json::json!({"name": c.name, "role": c.role, "age": c.age, "intro_chapter": c.intro_chapter, "personality": c.personality, "appearance": c.appearance, "outfits": c.outfits, "signature": c.signature})).collect::<Vec<_>>(), "locations": p.locations.iter().map(|l| serde_json::json!({"name": l.name, "look": l.look})).collect::<Vec<_>>(), "chapters": p.chapters.iter().map(|c| serde_json::json!({"number": c.number, "title": c.title, "summary": c.summary})).collect::<Vec<_>>() });
    (system, format!("The series now:\n{current}\n\nChange: {instruction}"))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SeriesAnswer {
    pub title: String,
    pub bible: crate::project::Bible,
    pub cast: Vec<CastMember>,
    pub locations: Vec<Location>,
    pub chapters: Vec<Chapter>,
}

/// Applies a series answer. Characters keep their painted sheets and the user's pictures when
/// they keep their name; chapters keep their pages and pictures.
pub fn apply_series(p: &mut Project, v: Value) -> anyhow::Result<()> {
    let a: SeriesAnswer = serde_json::from_value(v)?;
    if a.cast.is_empty() || a.bible.logline.is_empty() {
        anyhow::bail!("the answer had no cast or logline");
    }
    if !a.title.trim().is_empty() {
        p.title = a.title.trim().to_string();
    }
    p.bible = a.bible;
    let old_cast = std::mem::take(&mut p.cast);
    p.cast = a
        .cast
        .into_iter()
        .filter(|c| !c.name.trim().is_empty())
        .map(|mut c| {
            // Files are only ever carried over from our own entry, never taken from the answer.
            c.id.clear();
            c.user_refs.clear();
            c.sheet = None;
            c.expressions = None;
            if let Some(o) = old_cast.iter().find(|o| o.name.eq_ignore_ascii_case(c.name.trim())) {
                c.id = o.id.clone();
                c.user_refs = o.user_refs.clone();
                // A changed look makes the old sheets wrong.
                if o.appearance == c.appearance && o.outfits == c.outfits {
                    c.sheet = o.sheet.clone();
                    c.expressions = o.expressions.clone();
                }
            }
            c
        })
        .collect();
    let old_loc = std::mem::take(&mut p.locations);
    p.locations = a
        .locations
        .into_iter()
        .map(|mut l| {
            l.id.clear();
            l.sheet = None;
            if let Some(o) = old_loc.iter().find(|o| o.name.eq_ignore_ascii_case(l.name.trim())) {
                l.id = o.id.clone();
                if o.look == l.look {
                    l.sheet = o.sheet.clone();
                }
            }
            l
        })
        .collect();
    for c in a.chapters {
        let ch = p.chapter_mut(c.number.max(1));
        ch.title = c.title;
        ch.summary = c.summary;
    }
    p.normalize();
    Ok(())
}

fn layouts_list() -> String {
    TEMPLATES.iter().map(|t| format!("- {} ({} panels): {}", t.id, t.panels.len(), t.use_for)).collect::<Vec<_>>().join("\n")
}

fn cast_for(p: &Project, chapter: u32) -> String {
    let mut s = String::new();
    for c in p.cast.iter().filter(|c| c.intro_chapter <= chapter) {
        let outfits = c.outfits.iter().map(|o| o.name.as_str()).collect::<Vec<_>>().join(", ");
        let _ = writeln!(s, "- {} ({}): {} | outfits: {}", c.name, c.role, c.personality, outfits);
    }
    let new: Vec<&str> = p.cast.iter().filter(|c| c.intro_chapter == chapter).map(|c| c.name.as_str()).collect();
    if !new.is_empty() {
        let _ = writeln!(s, "Introduced in this chapter: {}", new.join(", "));
    }
    s
}

/// The page-by-page script of a chapter, `from`..=`to` (scripts are written a few pages at a time
/// so a long chapter never truncates). `earlier` is the script so far, summarised.
pub fn chapter_script(p: &Project, chapter: u32, note: &str, from: u32, to: u32, earlier: &str) -> (String, String) {
    let system = format!(
        "You write manga page scripts. {CRAFT}\n\nAnswer with ONE JSON object, no other text:\n{{\"title\": \"chapter title\", \"summary\": \"one paragraph\", \"pages\": [{{\"number\": {from}, \"layout\": \"layout id\", \"beat\": \"what this page does\", \"panels\": [{{\"shot\": \"wide|medium|close-up|extreme close-up|establishing|insert\", \"camera\": \"eye level|low angle|high angle|bird's eye|dutch angle|over the shoulder\", \"description\": \"what we SEE: composition, action, expressions, background, lighting — concrete and visual\", \"characters\": [{{\"name\": \"exact cast name\", \"expression\": \"\", \"outfit\": \"outfit name\", \"pose\": \"\"}}], \"location\": \"location name\", \"lines\": [{{\"speaker\": \"name\", \"text\": \"\", \"kind\": \"speech|thought|shout|whisper|narration\"}}], \"sfx\": \"\", \"motion\": \"how it moves if animated: camera move and character motion in one sentence\"}}]}}]}}\n\nPage layouts (the panel count must match the layout):\n{}\n\nOnly use characters from the cast list, spelled exactly; only those introduced in this chapter or earlier. Write pages {from} to {to} only.",
        layouts_list()
    );
    let ch = p.chapter(chapter).cloned().unwrap_or_default();
    let prev = p.chapters.iter().filter(|c| c.number < chapter).map(|c| format!("{}. {}: {}", c.number, c.title, c.summary)).collect::<Vec<_>>().join("\n");
    let mut user = format!("Series: {} — {}\nTwist: {}\nWorld: {}\n\nCast available:\n{}\nLocations: {}\n", p.title, p.bible.logline, p.bible.twist, p.bible.world, cast_for(p, chapter), p.locations.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(", "));
    if !prev.is_empty() {
        let _ = write!(user, "\nStory so far:\n{prev}\n");
    }
    let _ = write!(user, "\nChapter {chapter}: {} — {}\nThe chapter has {} pages in all.", ch.title, ch.summary, p.settings.pages_per_chapter);
    if !note.trim().is_empty() {
        let _ = write!(user, "\nThe user wants: {note}");
    }
    if !earlier.is_empty() {
        let _ = write!(user, "\n\nPages already written in this chapter:\n{earlier}\nContinue from page {from}.");
    }
    if to >= p.settings.pages_per_chapter {
        user.push_str("\nThe last page ends the chapter on a strong hook.");
    }
    (system, user)
}

/// One page's script rewritten.
pub fn page_edit(p: &Project, chapter: u32, page: &Page, instruction: &str) -> (String, String) {
    let (system, _) = chapter_script(p, chapter, "", page.number, page.number, "");
    // The script only: file paths stay on this side.
    let mut script = page.clone();
    script.image = None;
    script.model.clear();
    for x in &mut script.panels {
        x.image = None;
        x.clip = None;
    }
    let user = format!("Cast available:\n{}\nThe page now:\n{}\n\nRewrite page {} with this change: {instruction}", cast_for(p, chapter), serde_json::to_string(&script).unwrap_or_default(), page.number);
    (system, user)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ScriptAnswer {
    #[serde(deserialize_with = "crate::project::lenient_text")]
    pub title: String,
    #[serde(deserialize_with = "crate::project::lenient_text")]
    pub summary: String,
    pub pages: Vec<Page>,
}

/// One line per written page, for the next batch.
pub fn page_digest(pages: &[Page]) -> String {
    pages.iter().map(|pg| format!("p{}: {}", pg.number, pg.beat)).collect::<Vec<_>>().join("\n")
}

// ------------------------------------------------------------------ image prompts

fn medium(p: &Project) -> &'static str {
    if p.settings.live_action {
        "photorealistic live-action film still shot on a cinema camera: real human actors, natural skin texture and pores, real fabric, cinematic lighting, shallow depth of field, 35mm lens; NOT a drawing, NOT anime, NOT 3D render"
    } else if p.settings.color {
        "full-colour manga art, cel shading"
    } else {
        "black-and-white Japanese manga art, clean ink lines, screentone shading, pure white paper, no colour"
    }
}

fn look(c: &CastMember, outfit: &str) -> String {
    let o = c.outfits.iter().find(|o| o.name.eq_ignore_ascii_case(outfit.trim())).or(c.outfits.first());
    let mut s = format!("{}: {}", c.name, c.appearance);
    if let Some(o) = o {
        let _ = write!(s, "; wearing {}", o.look);
    }
    if !c.signature.is_empty() {
        let _ = write!(s, "; {}", c.signature);
    }
    s
}

/// A character model sheet: the turnaround every page is drawn from.
pub fn sheet(p: &Project, c: &CastMember, has_refs: bool) -> String {
    if p.settings.live_action {
        // A casting sheet: the same real person, whole body in every view. Leads get more views.
        let extra = match c.tier() {
            0 => " Bottom row: three large face close-ups of the same person (neutral, their signature expression, three-quarter angle) and a half-body shot.",
            1 => " One large face close-up of the same person in the corner.",
            _ => " One small face close-up in the corner.",
        };
        return format!(
            "Photorealistic casting reference sheet of ONE real person playing \"{}\" in a live-action film, {}. Top row: the same person standing four times side by side on a plain light-grey studio background, FULL BODY from head to shoes in every view: front view, three-quarter view, side profile and back view, relaxed neutral standing pose, same height and proportions in each.{extra}\nThis person keeps the character's own traits exactly, turned into a believable real human: {}. Age {}; {}. Realistic hair in the character's style and colour, their real eye colour, build and marks, and the same outfit in every view. Even soft studio light, sharp focus, no scenery, no props except their signature items, no text except the name \"{}\" small at the top.{}",
            c.name,
            medium(p),
            look(c, ""),
            if c.age.is_empty() { "unknown" } else { &c.age },
            c.personality,
            c.name,
            if has_refs { "\nThe attached images show the character as drawn: keep their face shape, hair, outfit and marks, but as a real person, not a drawing." } else { "" }
        );
    }
    format!(
        "Professional manga character model sheet for \"{}\". {}. Art direction: {}.\nShow the same character four times side by side on a plain white background: front view, three-quarter view, side profile and back view, full body head to toe, neutral standing pose, consistent proportions, plus a large head close-up in the corner. Character: {}. Age {}; {}.\nClean line art, consistent design in every view, no scenery, no text except the name \"{}\" small at the top.{}",
        c.name,
        medium(p),
        p.bible.art_style,
        look(c, ""),
        if c.age.is_empty() { "unknown" } else { &c.age },
        c.personality,
        c.name,
        if has_refs { "\nThe attached images set the character's look and the art style: match them." } else { "" }
    )
}

/// A 3×3 expression sheet, drawn after the model sheet (which is attached).
pub fn expressions(p: &Project, c: &CastMember) -> String {
    if p.settings.live_action {
        return format!(
            "Photorealistic expression sheet of the real person in the attached casting sheet, playing \"{}\": the identical face, eyes, hair and outfit collar. {}.\nA 3x3 grid of head-and-shoulders portraits on a plain light-grey background: neutral, happy laugh, angry shout, sad tears, shocked, determined, smug grin, embarrassed, scared. Each expression natural and readable, acted the way this person would ({}). Thin gaps between cells, no text.",
            c.name,
            medium(p),
            c.personality
        );
    }
    format!(
        "Manga expression sheet for \"{}\", the same character as in the attached model sheet: identical face, eyes, hair and outfit collar. {}. Art direction: {}.\nA 3x3 grid of head-and-shoulders portraits on white: neutral, happy laugh, angry shout, sad tears, shocked, determined, smug grin, embarrassed blush, scared. Each expression strong and readable in that character's personality ({}). Thin grid lines between cells, no other text.",
        c.name,
        medium(p),
        p.bible.art_style,
        c.personality
    )
}

/// An establishing shot of a location, reused as the background reference.
pub fn location(p: &Project, l: &Location) -> String {
    format!(
        "Manga background art, establishing shot of \"{}\": {}. World: {}. {}. Art direction: {}. Detailed environment, no characters, no text, wide angle, strong perspective.",
        l.name,
        l.look,
        p.bible.world,
        medium(p),
        p.bible.art_style
    )
}

fn panel_text(p: &Project, panel: &Panel, letter: bool) -> String {
    let mut s = format!("{} shot, {}. {}", panel.shot, panel.camera, panel.description);
    for c in &panel.characters {
        let fixed = p.character(&c.name).map(|m| look(m, &c.outfit)).unwrap_or_else(|| c.name.clone());
        let _ = write!(s, "\n  • {fixed}. Expression: {}. Pose: {}.", c.expression, c.pose);
    }
    if let Some(l) = p.location(&panel.location) {
        let _ = write!(s, "\n  Setting: {} — {}", l.name, l.look);
    } else if !panel.location.is_empty() {
        let _ = write!(s, "\n  Setting: {}", panel.location);
    }
    if letter {
        for l in &panel.lines {
            let shape = match l.kind.as_str() {
                "narration" => "a rectangular narration box",
                "thought" => "a cloud-shaped thought balloon",
                "shout" => "a spiky shout balloon",
                _ => "a speech balloon",
            };
            let _ = write!(s, "\n  {shape} from {} with exactly this English text: \"{}\"", l.speaker, l.text.trim());
        }
        if !panel.sfx.is_empty() {
            let _ = write!(s, "\n  Hand-drawn sound effect lettering: {}", panel.sfx);
        }
    }
    s
}

/// A whole manga page in one image: the layout, every panel and its lettering.
pub fn page(p: &Project, chapter: u32, pg: &Page, refs: &[String]) -> String {
    let rtl = p.settings.rtl;
    let mut s = format!(
        "A single finished page of a published manga (chapter {chapter}, page {} of \"{}\"), portrait 2:3, {}. Art direction: {}.\nPanel layout \"{}\" with {} panels separated by clean white gutters and black panel borders, read {}. Professional composition, dynamic camera angles, speed lines and screentone where they help, readable lettering in English inside balloons.\n",
        pg.number,
        p.title,
        medium(p),
        p.bible.art_style,
        pg.layout,
        pg.panels.len(),
        if rtl { "right to left, top to bottom (Japanese order)" } else { "left to right, top to bottom" },
    );
    for (i, panel) in pg.panels.iter().enumerate() {
        let _ = write!(s, "\nPanel {} — {}\n", i + 1, panel_text(p, panel, true));
    }
    if !refs.is_empty() {
        let _ = write!(s, "\nReference images: {}. Characters must match their sheets exactly (face, hair, outfit); match the style references' linework and tone.", refs.join("; "));
    }
    s
}

/// One panel as its own image (laid out and lettered by the app).
pub fn panel(p: &Project, panel: &Panel, aspect: &str, refs: &[String]) -> String {
    if p.settings.live_action {
        // The first frame of the scene's clip: a film still, nothing drawn or written on it.
        let mut s = format!("One frame of a live-action film, {aspect} frame, {}.\n{}\nNo text, no captions, no speech balloons, no panel border, no watermark. Mouths closed or just about to speak.", medium(p), panel_text(p, panel, false));
        if !refs.is_empty() {
            let _ = write!(s, "\nReference images: {}. Every person must be the same real person as in their casting sheet (face, hair, build, outfit).", refs.join("; "));
        }
        return s;
    }
    let mut s = format!("One manga panel, {aspect} frame, {}. Art direction: {}.\n{}\nNo panel border, no speech balloons, no text (lettering is added later); leave some calm space near the top for balloons.", medium(p), p.bible.art_style, panel_text(p, panel, false));
    if !panel.sfx.is_empty() {
        let _ = write!(s, " Hand-drawn sound effect lettering is fine: {}.", panel.sfx);
    }
    if !refs.is_empty() {
        let _ = write!(s, "\nReference images: {}. Characters must match their sheets exactly.", refs.join("; "));
    }
    s
}

/// What the video model is asked for one panel.
pub fn motion(p: &Project, panel: &Panel) -> String {
    if p.settings.live_action {
        return scene(panel);
    }
    let m = if panel.motion.trim().is_empty() { "subtle camera push-in, characters breathe and blink, hair and clothes move slightly" } else { panel.motion.trim() };
    let notes = if panel.notes.trim().is_empty() { String::new() } else { format!(" Director's notes, follow them: {}", panel.notes.trim()) };
    format!("Animate this manga panel as anime: {m}. Keep the exact art style, character designs and composition of the image. {} No new characters, no text overlays.{notes}", if p.settings.color { "" } else { "Keep it black and white with screentone." })
}

/// A live-action scene for the video model: what happens over time, what is seen, and what is
/// heard (the spoken lines, so the mouths move with the voice), then the user's notes.
fn scene(panel: &Panel) -> String {
    let m = if panel.motion.trim().is_empty() { "natural small movements, the camera slowly pushes in" } else { panel.motion.trim() };
    let mut s = format!("A live-action film scene, photorealistic, real actors. {} shot, {}. {} Action: {m}.", panel.shot, panel.camera, panel.description);
    let spoken = spoken_lines(panel);
    if spoken.is_empty() {
        s.push_str(" Nobody speaks; only the natural sounds of the place.");
    } else {
        for (who, text) in &spoken {
            if who.is_empty() || who.eq_ignore_ascii_case("narrator") || who.eq_ignore_ascii_case("narration") {
                let _ = write!(s, " A narrator says off screen: \"{text}\" (nobody on screen moves their lips for it).");
            } else {
                let _ = write!(s, " {who} says, lips in sync with the voice: \"{text}\"");
            }
        }
    }
    s.push_str(" Keep every person exactly as in the first frame: same face, hair and outfit. No text, no subtitles, no new characters.");
    if !panel.notes.trim().is_empty() {
        let _ = write!(s, " Director's notes, follow them: {}", panel.notes.trim());
    }
    s
}

/// The lines of a scene that are heard, in order: who says them and the words.
pub fn spoken_lines(panel: &Panel) -> Vec<(String, String)> {
    panel.lines.iter().filter(|l| !l.text.trim().is_empty()).map(|l| (l.speaker.trim().to_string(), l.text.trim().to_string())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn project() -> Project {
        let mut p = Project { title: "Stone Spark".into(), ..Default::default() };
        p.bible.logline = "A boy restarts science after the world turns to stone.".into();
        p.bible.art_style = "crisp detailed linework".into();
        p.cast.push(CastMember { name: "Kai".into(), role: "protagonist".into(), intro_chapter: 1, appearance: "spiky white hair".into(), outfits: vec![crate::project::Outfit { name: "lab".into(), look: "torn lab coat".into() }], ..Default::default() });
        p.cast.push(CastMember { name: "Rin".into(), intro_chapter: 3, ..Default::default() });
        p.normalize();
        p
    }

    #[test]
    fn director_ops_parse_and_skip_junk() {
        let v = json!({ "reply": "On it!", "ops": [
            { "op": "create_series", "brief": "like Dr. Stone", "inspirations": ["Dr. Stone"] },
            { "op": "write_chapter", "chapter": "2" },
            { "op": "propose", "kind": "pages", "chapter": 1, "pages": [1, 2] },
            { "op": "propose", "kind": "mystery" },
            { "op": "settings", "pages_per_chapter": 500, "color": true },
            { "op": "dance" }
        ]});
        let (reply, ops) = parse_director(&v);
        assert_eq!(reply, "On it!");
        assert_eq!(ops.len(), 4);
        assert_eq!(ops[1], Op::WriteChapter { chapter: 2, note: String::new() });
        assert_eq!(ops[2], Op::Propose(Proposal::Pages { chapter: 1, pages: vec![1, 2] }));
        assert_eq!(ops[3], Op::Settings { color: Some(true), rtl: None, pages_per_chapter: None });
    }

    #[test]
    fn series_answers_keep_painted_sheets() {
        let mut p = project();
        p.cast[0].sheet = Some("sheets/kai.png".into());
        p.cast[0].expressions = Some("sheets/kai-x.png".into());
        p.cast[0].user_refs = vec!["refs/mine.png".into()];
        let v = json!({ "title": "Stone Spark", "bible": { "logline": "x", "art_style": "y" },
            "cast": [ { "name": "Kai", "appearance": "spiky white hair", "outfits": [{ "name": "lab", "look": "torn lab coat" }], "intro_chapter": 1 },
                      { "name": "Mei", "intro_chapter": "2" } ],
            "chapters": [ { "number": 1, "title": "Wake" }, { "number": 2, "title": "Fire" } ] });
        apply_series(&mut p, v).unwrap();
        assert_eq!(p.cast.len(), 2);
        assert_eq!(p.cast[0].sheet.as_deref(), Some("sheets/kai.png"));
        assert_eq!(p.cast[0].user_refs, vec!["refs/mine.png".to_string()]);
        assert_eq!(p.cast[1].intro_chapter, 2);
        assert_eq!(p.chapters.len(), 2);
        // Files named in an answer are never taken: only our own entries carry them.
        let v = json!({ "bible": { "logline": "x" }, "cast": [
            { "name": "Kai", "appearance": "spiky white hair", "outfits": [{ "name": "lab", "look": "torn lab coat" }], "sheet": "C:/elsewhere/x.png" },
            { "name": "Mei", "sheet": "C:/Users/me/private.png", "user_refs": ["../../x.png"], "expressions": "e.png" } ],
            "locations": [ { "name": "Lab", "sheet": "/etc/x.png" } ] });
        apply_series(&mut p, v).unwrap();
        assert_eq!(p.cast[0].sheet.as_deref(), Some("sheets/kai.png"));
        assert!(p.cast[1].sheet.is_none() && p.cast[1].expressions.is_none() && p.cast[1].user_refs.is_empty());
        assert!(p.locations[0].sheet.is_none());
        // A new look drops the old sheets.
        let v = json!({ "bible": { "logline": "x" }, "cast": [ { "name": "Kai", "appearance": "bald" } ] });
        apply_series(&mut p, v).unwrap();
        assert!(p.cast[0].sheet.is_none());
        assert!(apply_series(&mut p, json!({ "cast": [] })).is_err());
    }

    #[test]
    fn prompts_carry_the_fixed_look() {
        let p = project();
        let panel = Panel { shot: "close-up".into(), characters: vec![crate::project::PanelChar { name: "kai".into(), expression: "grin".into(), outfit: "lab".into(), pose: String::new() }], lines: vec![crate::project::Line { speaker: "Kai".into(), text: "Science!".into(), kind: "shout".into() }], ..Default::default() };
        let pg = Page { number: 1, layout: "splash".into(), panels: vec![panel.clone()], ..Default::default() };
        let full = page(&p, 1, &pg, &["Image 1: Kai model sheet".into()]);
        assert!(full.contains("spiky white hair") && full.contains("torn lab coat"));
        assert!(full.contains("\"Science!\"") && full.contains("right to left"));
        let one = panel_prompt_for_test(&p, &panel);
        assert!(one.contains("No panel border") && !one.contains("\"Science!\""));
        assert!(sheet(&p, &p.cast[0], false).contains("side profile"));
        // Chapter 1's cast list leaves out characters who arrive later.
        let (_, user) = chapter_script(&p, 1, "", 1, 6, "");
        assert!(user.contains("Kai") && !user.contains("- Rin"));
    }

    fn panel_prompt_for_test(p: &Project, panel: &Panel) -> String {
        super::panel(p, panel, "16:9", &[])
    }
}
