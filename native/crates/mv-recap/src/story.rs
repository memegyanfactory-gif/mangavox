//! STORY phase: turn the chapter's reading (and edited transcript) into a story-first recap voice-over.
//! The writer sees the whole chapter at once (local models with small context windows get it in a few
//! parts), keeps only what matters to the story, and anchors each passage to the panel shown while it
//! is spoken; pages it leaves out are cut from the video. The story so far is carried between parts and
//! chapters, and kept in the brain so a chapter written on its own still knows what came before.
//! Chapters play back to back in an export, so each one continues the same telling (no per-chapter
//! intro, recap or outro); subscribe reminders are planned on the manga's whole running time.

use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use mv_ai::{AiMessage, AiProviderId, CancellationToken, Registry};
use serde_json::Value;

use crate::brain::Brain;
use crate::json::{num_of, str_of};
use crate::llm::{Patient, ask_json, is_cancelled, request, truncate, Llm};
use crate::store::*;
use crate::transcript::format_line;
use crate::types::*;

/// Digest characters one story call may carry. A whole chapter (~90k) fits hosted models; local
/// ones with small context windows get it in a few parts.
fn part_budget(m: &ModelChoice) -> usize {
    match m.provider {
        AiProviderId::Ollama | AiProviderId::Lmstudio | AiProviderId::Llamacpp => 30_000,
        _ => 180_000,
    }
}

/// Splits pages (index, digest size) into as few runs as fit `budget`, of about equal size.
pub(crate) fn plan_parts(pages: &[(usize, usize)], budget: usize) -> Vec<Vec<usize>> {
    let total: usize = pages.iter().map(|p| p.1).sum();
    let n = total.div_ceil(budget.max(1)).max(1);
    let target = total.div_ceil(n);
    let mut parts: Vec<Vec<usize>> = vec![vec![]];
    let mut size = 0;
    for &(i, s) in pages {
        if size > 0 && size + s > target && parts.len() < n {
            parts.push(vec![]);
            size = 0;
        }
        parts.last_mut().unwrap().push(i);
        size += s;
    }
    parts.retain(|p| !p.is_empty());
    parts
}

pub fn style_hint(style: &str) -> &'static str {
    match style {
        "Happy" => "upbeat and bright; playful energy, a smile in the voice, light exclamations (sparingly)",
        "Warm" => "gentle, affectionate and comforting, like a close friend telling you the story by the fire",
        "Cinematic" => "movie-trailer drama: vivid visual language, strong rhythm, rising tension, punchy reveals",
        "Normal narration" => "clear, natural, engaging anime-recap narrator; neutral and easy to follow",
        "Classic" => "timeless storybook prose; polished, slightly formal, well-turned sentences",
        "Old-timer" => "folksy storyteller with warm wisdom and a 'back in the day' cadence; homespun phrases, the occasional knowing aside",
        "Charm" => "witty and charming; playful asides, light teasing, a wink at the audience without breaking the story",
        "Exotic" => "lush, vivid and unusual imagery; sensory detail, poetic metaphors, a sense of far-away places",
        "Depth" => "reflective and thoughtful; dwell on themes, motives, subtext and inner conflict, not only on events",
        "Hard" => "blunt and gritty; short, hard sentences, no sugar-coating, dry understatement",
        "Soft" => "quiet, delicate and hushed; tender word choice, slow gentle phrasing",
        _ => "",
    }
}

fn story_system(styles: &[String]) -> String {
    let styles: Vec<&str> = if styles.is_empty() { vec!["Normal narration"] } else { styles.iter().map(String::as_str).collect() };
    let style_text = styles
        .iter()
        .map(|s| match style_hint(s) {
            "" => format!("- {s}"),
            h => format!("- {s}: {h}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    let blend = if styles.len() > 1 { "Blend these styles into ONE consistent narrator voice (do not alternate between them). " } else { "" };
    format!(
        r##"You are the head writer of a top manga / manhwa recap channel. You receive a reading of a chapter (what happens on each page, what each panel shows, who is in it, every line of dialogue) and write the voice-over that tells its STORY, spoken over the most telling panels.

HOW A GREAT RECAP SOUNDS
- It is a story told by a storyteller, not a description of pages. Third person, present tense, named characters, clear cause and effect: who wants what, what stands in the way, what happens and why it matters.
- Keep the scenes whole: retell the arguments, jokes, boasts, lessons, threats and decisions beat by beat (who says what, how the other side reacts, what it changes). That back-and-forth IS the story; leaving it out turns the recap into a thin summary.
- Dialogue is retold, not read out: "Liam explains that only a swordmaster can kill a swordmaster." "She makes him promise to give up on revenge and simply survive." Quote a line word for word only when it is a killer line, and keep it short: "Eat my sword."
- Short sentences hit hard at the turns ("He was wrong." "The whole massacre took ten minutes."), then the story moves on.
- Rules of the world, powers, ranks and secrets are explained in one clear sentence the moment they matter.
- Time jumps and flashbacks are signposted: "One month passes." "Years earlier..." "Back in the present..."
- First-person narration boxes and inner monologue ("I", "me", "my") are the thoughts of the character they belong to, usually the protagonist: retell them in third person with that character's name. Never say "the narrator".
- Call characters by name: the CAST, or a name heard in the dialogue anywhere in the chapter (read ahead: a stranger introduced on page 30 is still named when they first appear). The first time someone matters, add a few words on who they are ("Liam Karavan, the legendary founder of the house"). Never call a named character by their looks ("a silver-haired man", "the blond youth"); a nameless extra gets a role ("a bandit", "the old butler"). Cast entries marked "known only by looks" are descriptions, not names: when the dialogue names that person ("Master Hyejeong!"), use the name; otherwise use their role ("the instructor"). A name may be spelled differently in earlier chapters (Mugung / Mugyeong): use the spelling this chapter's dialogue uses.
- Tell it with feeling, not as a dry summary: the narrator reacts the way a fan telling a friend would (a beat of awe at a big move, sympathy at a loss, dry amusement at an arrogant villain), then the story moves on.
- A little humour, sparingly: where it truly fits, ONE light joke or wry remark that comes from the characters and the situation (a hero's bluntness, a villain's ego, an absurd misunderstanding). One or two per chapter at most, many chapters need none, and never in serious, sad or tense scenes. It never mocks the story and never breaks it; the narrator stays the same sober, consistent voice.

ONE CONTINUOUS STORY
- The chapters play back to back as ONE long video, often hours long. Your voice-over must sound like one story told in one go by one storyteller, never like a series of episodes.
- Never mention chapters or episodes: no chapter numbers, no "chapter", "episode", "in this chapter", "previously", "last time", "this time", "end of chapter", "to be continued", "stay tuned", "see you next time".
- No greetings, introductions or sign-offs ("welcome back", "hey guys", "today we", "that's it for", "thanks for watching"), and no recap of what the viewer has already heard: pick up exactly where the story stands and keep going.
- Never wrap up or tease at the end of the pages: stop on the last beat they tell, so what comes next flows straight on.
- Never ask the viewer to like, subscribe or comment, unless the instructions for these pages ask for a CALL TO ACTION.

WHAT TO LEAVE OUT
- Never describe the artwork: no colours, lighting, clothes, hair, camera angles, backgrounds or effects ("darkness fills the screen", "blue light glows", "in this panel", "the page shows", "we see" are all forbidden). The viewer already sees the picture; you tell what it MEANS for the story.
- Skip only true filler: sound effects, establishing shots, scenery, silent reaction shots, repeated beats, small talk that changes nothing, title / credit / author pages, re-tellings of what was already covered. Pages that add nothing to the story get no segment at all and are cut from the video, so never mention them.
- Never invent events, motives or names the reading does not support.

NARRATION STYLE
{style_text}
{blend}Pacing follows the scene: fights are fast and punchy, emotional moments slow down and breathe, reveals build up and then land, comedy gets light timing.

LENGTH
Each chapter comes with a LENGTH TARGET worked out from how much it says. Land near it: far under it means scenes were skipped, and padding to reach it is just as bad. Always tell the chapter through to its last page.

SEGMENTS
The voice-over is a list of segments in story order. Each segment is 1-3 sentences (about 12-45 words) and is shown over ONE panel: pick the panel that best shows what the segment says, preferably one with the characters or the action in it; never a text-only box, an empty or nearly black panel, or a sound effect. A segment may sum up several pages; anchor it to the most telling panel among them. Move to a new panel every segment or two so the picture keeps up with the story. Plain text only: no markdown, emojis, brackets, stage directions or page numbers, because a text-to-speech voice reads it.
A segment that lands a truly big moment (a blow, a blast, a clash of blades, a shock, a reveal) may add "sfx" (one of: {sfx}) and "sfx_strength" 1-3. Leave it out everywhere else: a handful per chapter at most.

OUTPUT: STRICT JSON only, one object, no code fences, no commentary:
{{"title": "short catchy title for this chapter's recap",
  "summary_so_far": "6-10 sentences with names: the whole story so far, this chapter included, used as memory for what comes next",
  "segments": [
    {{"page": 3, "panel": 2, "text": "For one year, Arhan has hunted a single name: Karlos, the man who slaughtered his family."}},
    {{"page": 7, "panel": 1, "text": "...", "sfx": "impact", "sfx_strength": 2}}
  ]}}
"page" and "panel" use the numbers shown in the reading (both start at 1). Segments must be in story order."##,
        sfx = SFX_NAMES.join(", "),
    )
}

/// Compact text digest of the given pages. The transcript text wins over the AI lines when the user edited it.
pub(crate) fn digest(read: &ChapterRead, transcript: Option<&Transcript>, brain: &Brain, pages: &[usize]) -> String {
    let mut s = String::new();
    for &i in pages {
        let Some(p) = read.pages.iter().find(|p| p.index == i) else { continue };
        let sc = &p.scene;
        let mut head = format!("PAGE {}", i + 1);
        let tags: Vec<&str> = [sc.kind.as_str(), sc.mood.as_str()].into_iter().filter(|x| !x.is_empty()).collect();
        if !tags.is_empty() || sc.intensity > 0 {
            head += &format!(" [{}; intensity {}/10{}]", tags.join(", "), sc.intensity, if sc.tags.is_empty() { String::new() } else { format!("; {}", sc.tags.join(", ")) });
        }
        s += &head;
        s.push('\n');
        if !p.summary.is_empty() {
            s += &format!("Summary: {}\n", p.summary);
        }
        let edited = transcript.and_then(|t| t.pages.iter().find(|x| x.index == i && x.edited));
        for (n, pn) in p.panels.iter().enumerate() {
            let mut line = format!("Panel {}:", n + 1);
            if !pn.description.is_empty() {
                line += &format!(" {}", truncate(&pn.description, 240));
            }
            if !pn.characters.is_empty() {
                let names: Vec<String> = pn.characters.iter().map(|c| brain.canonical(&c.name)).collect();
                line += &format!(" (in it: {})", names.join(", "));
            }
            s += &line;
            s.push('\n');
            if edited.is_none() {
                for l in &pn.lines {
                    let mut l = l.clone();
                    l.speaker = l.speaker.map(|sp| brain.canonical(&sp));
                    s += &format!("  {}\n", format_line(&l));
                }
            }
        }
        if let Some(t) = edited {
            if !t.text.trim().is_empty() {
                s += &format!("Dialogue (edited, authoritative):\n{}\n", t.text.trim());
            }
        }
        s.push('\n');
    }
    s
}

fn has_content(p: &PageRead, tr: Option<&Transcript>) -> bool {
    !p.panels.is_empty() && !p.summary.is_empty()
        || p.panels.iter().any(|pn| !pn.lines.is_empty() || !pn.description.is_empty())
        || tr.is_some_and(|t| t.pages.iter().any(|x| x.index == p.index && !x.text.trim().is_empty()))
}

/// A cast entry that is a description ("Bald elder with glasses"), not a name: it has a word in
/// lower case ("Master Hyejeong", "Ganghyeok Choi" do not).
fn looks_only(name: &str) -> bool {
    name.split_whitespace().any(|w| w.chars().next().is_some_and(char::is_lowercase))
}

/// The cast for the writer: real names first (with their name-like aliases), then the most seen
/// characters known only by their looks, flagged as such so the writer names them from the
/// dialogue or calls them by role instead of by their looks.
pub(crate) fn cast_text(brain: &Brain) -> String {
    let mut cs: Vec<_> = brain.characters.iter().collect();
    cs.sort_by_key(|c| std::cmp::Reverse(c.appearances));
    let (named, unnamed): (Vec<_>, Vec<_>) = cs.into_iter().partition(|c| !looks_only(&c.name));
    let mut s: Vec<String> = named
        .iter()
        .take(40)
        .map(|c| {
            let aliases: Vec<&str> = c.aliases.iter().map(String::as_str).filter(|a| !looks_only(a)).collect();
            let mut line = format!("- {}", c.name);
            if !aliases.is_empty() {
                line += &format!(" (also called: {})", aliases.join(", "));
            }
            if !c.description.is_empty() {
                line += &format!(": {}", truncate(&c.description, 160));
            }
            line
        })
        .collect();
    if s.is_empty() {
        s.push("(no names known yet: take them from the dialogue)".into());
    }
    let unnamed: Vec<String> = unnamed.iter().filter(|c| c.appearances > 2).take(12).map(|c| format!("- \"{}\" (known only by looks)", c.name)).collect();
    if !unnamed.is_empty() {
        s.push("Not named yet:".into());
        s.extend(unnamed);
    }
    s.join("\n")
}

/// Lines of dialogue / narration on `pages` (sound effects not counted): how much the pages say.
fn spoken_lines(read: &ChapterRead, transcript: Option<&Transcript>, pages: &[usize]) -> usize {
    pages
        .iter()
        .filter_map(|i| read.pages.iter().find(|p| p.index == *i))
        .map(|p| match transcript.and_then(|t| t.pages.iter().find(|x| x.index == p.index && x.edited)) {
            Some(t) => t.text.lines().filter(|l| !l.trim().is_empty() && !l.starts_with("[SFX]")).count(),
            None => p.panels.iter().flat_map(|pn| &pn.lines).filter(|l| l.kind != "sfx").count(),
        })
        .sum()
}

/// Words a recap spends on pages saying `lines` lines: about three per line, as the recap channels
/// do (~650 words for a 200-line chapter), rounded to 50.
pub(crate) fn length_target(lines: usize) -> usize {
    ((lines as f64 * 3.0 / 50.).round() as usize * 50).clamp(200, 1400)
}

/// Segments of an answer on `chunk_pages`, in page order. The writer no longer makes hook
/// segments (the export makes one hook for the whole video); a `"hook"` key in an answer is ignored.
pub(crate) fn parse_segments(v: &Value, chunk_pages: &[usize], read: &ChapterRead) -> Vec<StorySegment> {
    let mut out: Vec<StorySegment> = vec![];
    for sv in v.get("segments").and_then(Value::as_array).into_iter().flatten() {
        let text = str_of(sv, "text");
        let Some(page) = num_of(sv, "page").filter(|n| *n >= 1.0).map(|n| n as usize - 1) else { continue };
        if text.is_empty() || !chunk_pages.contains(&page) {
            continue;
        }
        let panels = read.pages.iter().find(|p| p.index == page).map(|p| p.panels.len()).unwrap_or(0);
        let panel = num_of(sv, "panel").filter(|n| *n >= 1.0).map(|n| n as usize - 1).filter(|n| *n < panels);
        let sfx = SfxCue::parse(&str_of(sv, "sfx"), num_of(sv, "sfx_strength"));
        out.push(StorySegment { page, panel, text, delivery: None, sfx, hook: false });
    }
    // Stable: keeps the model's order of the panels within a page.
    out.sort_by_key(|s| s.page);
    out
}

/// Narration speed the subscribe reminders are planned at.
const WORDS_PER_MINUTE: f64 = 190.0;
/// The first subscribe reminder, in minutes into the manga's whole narration.
const FIRST_REMINDER_MIN: f64 = 27.0;
/// Then one every so many minutes.
const REMINDER_EVERY_MIN: f64 = 80.0;

/// Where a reminder mark falls in a chapter whose narration runs from minute `start` to
/// `start + len` of the manga's whole narration: the share of the chapter (0..1) at the first
/// mark in `[start, start + len)`, or `None` when none does. Marks sit at minute 27, then every 80
/// minutes (107, 187, 267 ...), so every chapter has one at most and a range played from the start
/// gets one in its first hour, about six in eight hours.
pub(crate) fn reminder_at(start: f64, len: f64) -> Option<f64> {
    if !(start.is_finite() && len.is_finite() && len > 0.0) {
        return None;
    }
    let k = ((start - FIRST_REMINDER_MIN) / REMINDER_EVERY_MIN).ceil().max(0.0);
    let mark = FIRST_REMINDER_MIN + k * REMINDER_EVERY_MIN;
    (mark < start + len).then(|| ((mark - start) / len).clamp(0.0, 1.0))
}

/// Narration words before a chapter: the sum over `earlier` chapters (saved story words, page
/// count) of their words, estimated for one without a story from its pages at the words per
/// page of `samples` (chapters with a story: words, pages), else as their average chapter. With
/// no samples, this chapter's own estimate (`this`: words, pages) stands in.
pub(crate) fn words_before(earlier: &[(Option<usize>, usize)], samples: &[(usize, usize)], this: (usize, usize)) -> f64 {
    let avg = if samples.is_empty() { this.0 as f64 } else { samples.iter().map(|s| s.0).sum::<usize>() as f64 / samples.len() as f64 };
    let paged: Vec<&(usize, usize)> = samples.iter().filter(|s| s.1 > 0).collect();
    let per_page = if !paged.is_empty() {
        Some(paged.iter().map(|s| s.0).sum::<usize>() as f64 / paged.iter().map(|s| s.1).sum::<usize>() as f64)
    } else if samples.is_empty() && this.1 > 0 {
        Some(this.0 as f64 / this.1 as f64)
    } else {
        None
    };
    earlier
        .iter()
        .map(|&(words, pages)| match (words, per_page) {
            (Some(w), _) => w as f64,
            (None, Some(r)) if pages > 0 => pages as f64 * r,
            _ => avg,
        })
        .sum()
}

fn story_words(s: &Story) -> usize {
    s.segments.iter().map(|g| g.text.split_whitespace().count()).sum()
}

/// The manga's chapters the reminders are planned on: (chapter id, output dir, saved story
/// words, page count). `ordered` when they are the manga's chapters in reading order; else they
/// are the chapter folders next to the one being written, used only for an average chapter.
struct Timeline {
    chapters: Vec<(String, std::path::PathBuf, Option<usize>, usize)>,
    ordered: bool,
}

impl Timeline {
    fn load(manga: &[ChapterRef], run: &[ChapterRef]) -> Timeline {
        if !manga.is_empty() {
            let chapters = manga.iter().map(|c| (c.chapter_id.clone(), c.output_dir.clone(), load_story(c).map(|s| story_words(&s)), list_pages(&c.pages_dir).len())).collect();
            return Timeline { chapters, ordered: true };
        }
        // No order given: the other chapter folders of the manga (`<studio>/projects/<manga>/<chapter>`).
        let mut chapters = vec![];
        let parents: std::collections::BTreeSet<std::path::PathBuf> = run.iter().filter_map(|c| c.output_dir.parent().map(|p| p.to_path_buf())).collect();
        for dir in parents.iter().flat_map(|p| std::fs::read_dir(p).into_iter().flatten().flatten()).map(|e| e.path()).filter(|p| p.is_dir()) {
            let r = ChapterRef { output_dir: dir.clone(), ..Default::default() };
            if let Some(s) = load_story(&r) {
                chapters.push((String::new(), dir, Some(story_words(&s)), 0));
            }
        }
        Timeline { chapters, ordered: false }
    }

    fn is(c: &(String, std::path::PathBuf, Option<usize>, usize), ch: &ChapterRef) -> bool {
        !c.0.is_empty() && c.0 == ch.chapter_id || c.1 == ch.output_dir
    }

    fn set_words(&mut self, ch: &ChapterRef, words: usize) {
        match self.chapters.iter_mut().find(|c| Self::is(c, ch)) {
            Some(c) => c.2 = Some(words),
            None if !self.ordered => self.chapters.push((ch.chapter_id.clone(), ch.output_dir.clone(), Some(words), 0)),
            None => {}
        }
    }

    /// Minutes of narration the manga has before `ch`, whose own estimate is `this` (words, pages).
    /// Without the manga's order, the chapters before it are counted from its number ("Chapter 12":
    /// 11), each an average chapter.
    fn minutes_before(&self, ch: &ChapterRef, this: (usize, usize)) -> f64 {
        let samples: Vec<(usize, usize)> = self.chapters.iter().filter(|c| !Self::is(c, ch)).filter_map(|c| c.2.map(|w| (w, c.3))).collect();
        let pos = self.chapters.iter().position(|c| Self::is(c, ch)).filter(|_| self.ordered);
        let earlier: Vec<(Option<usize>, usize)> = match pos {
            Some(i) => self.chapters[..i].iter().map(|c| (c.2, c.3)).collect(),
            None => {
                let n = crate::brain::chapter_number(&ch.label).unwrap_or(0.0);
                vec![(None, 0); (n.ceil() as i64 - 1).max(0) as usize]
            }
        };
        words_before(&earlier, &samples, this) / WORDS_PER_MINUTE
    }
}

/// The instruction for the subscribe reminder, at `share` (0..1) of the pages it is asked on.
fn call_to_action(share: f64) -> String {
    let at = if share < 0.34 {
        "in the first part of these pages"
    } else if share < 0.67 {
        "around the middle of these pages"
    } else {
        "in the last part of these pages"
    };
    format!(
        "CALL TO ACTION: {at}, at a natural break in the story (a scene change, or a quiet moment after a beat has landed; never in the middle of a fight, a reveal or a sad moment), add ONE short segment in which the narrator, in their own voice, asks the viewer to subscribe and turn on the bell, in the spirit of: \"If you're enjoying this, subscribe to the channel. It really helps, because we love making these. And if you love it, hit the bell for more manga like this.\" Say it in fresh words of your own (never copy that example), two or three short sentences, anchored to the panel of the beat just before it, then go straight back into the story. "
    )
}

/// The pages of `chunk` after the last one the answer narrated, when a real share of what the
/// chunk says (over 15% of its lines) is still untold: the writer stopped early.
fn untold(read: &ChapterRead, transcript: Option<&Transcript>, chunk: &[usize], segs: &[StorySegment]) -> Option<Vec<usize>> {
    let reached = segs.iter().map(|s| s.page).max()?;
    let rest: Vec<usize> = chunk.iter().copied().filter(|p| *p > reached).collect();
    let (all, left) = (spoken_lines(read, transcript, chunk), spoken_lines(read, transcript, &rest));
    (!rest.is_empty() && left * 100 > all * 15).then_some(rest)
}

pub async fn write_story(
    reg: Arc<Registry>,
    model: ModelChoice,
    chapters: Vec<ChapterRef>,
    styles: Vec<String>,
    brain: Arc<Mutex<Brain>>,
    progress: ProgressFn,
    cancel: CancellationToken,
) -> Result<Vec<(String, Story)>> {
    write_story_for_manga(reg, model, chapters, vec![], styles, brain, progress, cancel).await
}

/// [`write_story`] with `manga`: all the manga's chapters in reading order (by chapter number),
/// so the subscribe reminders are planned on the manga's whole running time. Without it they are
/// planned from the chapter numbers in the labels.
#[allow(clippy::too_many_arguments)]
pub async fn write_story_for_manga(
    reg: Arc<Registry>,
    model: ModelChoice,
    chapters: Vec<ChapterRef>,
    manga: Vec<ChapterRef>,
    styles: Vec<String>,
    brain: Arc<Mutex<Brain>>,
    progress: ProgressFn,
    cancel: CancellationToken,
) -> Result<Vec<(String, Story)>> {
    let llm = Patient::new(reg, progress.clone());
    write_story_with(&llm, &model, &chapters, &manga, &styles, &brain, &progress, &cancel).await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn write_story_with(
    llm: &dyn Llm,
    model: &ModelChoice,
    chapters: &[ChapterRef],
    manga: &[ChapterRef],
    styles: &[String],
    brain: &Mutex<Brain>,
    progress: &ProgressFn,
    cancel: &CancellationToken,
) -> Result<Vec<(String, Story)>> {
    let system = story_system(styles);
    let label = format!("{}/{}", model.provider.as_str(), model.model);
    let mut results: Vec<(String, Story)> = vec![];
    let mut so_far = String::new();
    let mut timeline = Timeline::load(manga, chapters);

    for ch in chapters {
        let Some(read) = load_read(ch).filter(|r| !r.pages.is_empty()) else {
            progress(Progress::Log(format!("{}: nothing read yet, skipped.", ch.label)));
            continue;
        };
        let transcript = load_transcript(ch);
        let snapshot = brain.lock().unwrap().clone();
        let content: Vec<usize> = read.pages.iter().filter(|p| has_content(p, transcript.as_ref())).map(|p| p.index).collect();
        let mut story = Story { styles: styles.to_vec(), model: label.clone(), ..Default::default() };
        let sizes: Vec<(usize, usize)> = content.iter().map(|&i| (i, digest(&read, transcript.as_ref(), &snapshot, &[i]).len())).collect();
        let mut parts: std::collections::VecDeque<(Vec<usize>, bool)> = plan_parts(&sizes, part_budget(model)).into_iter().map(|p| (p, false)).collect();
        let n_parts = parts.len();
        let mut done = 0;
        let mut follow_ups = 0;
        // Run on its own, the chapter starts from the story so far as of the chapter before it.
        if so_far.is_empty() {
            so_far = snapshot.recap_before(&ch.label).unwrap_or_default().to_string();
        }
        let very_start = so_far.is_empty();
        // The subscribe reminder this chapter carries, if a mark of the manga's running time falls
        // in it: at that share of its content pages, asked of the part that holds it.
        let this_words: usize = parts.iter().map(|(p, _)| length_target(spoken_lines(&read, transcript.as_ref(), p))).sum();
        let start = timeline.minutes_before(ch, (this_words, read.pages.len()));
        let mut reminder = reminder_at(start, this_words as f64 / WORDS_PER_MINUTE).map(|s| ((s * content.len() as f64) as usize).min(content.len().saturating_sub(1)));

        while let Some((chunk, carry_on)) = parts.pop_front() {
            if cancel.is_cancelled() {
                progress(Progress::Failed("Cancelled".into()));
                return Err(anyhow!("cancelled"));
            }
            let chunk = chunk.as_slice();
            let (first, last) = (story.segments.is_empty() && !carry_on, parts.is_empty());
            let span = if n_parts == 1 && !carry_on {
                format!("the whole chapter, {} pages", content.len())
            } else {
                format!("pages {}-{} of this chapter's {} pages", chunk[0] + 1, chunk[chunk.len() - 1] + 1, content.len())
            };
            let lines = spoken_lines(&read, transcript.as_ref(), chunk);
            let mut prompt = format!(
                "MANGA: {}\nCHAPTER: {} ({span}; for your reference only, never say it)\nSTYLES: {}\nLENGTH TARGET: about {} words ({lines} lines of dialogue and narration on these pages)\n",
                snapshot.manga_title,
                ch.label,
                if styles.is_empty() { "Normal narration".to_string() } else { styles.join(", ") },
                length_target(lines),
            );
            prompt += &format!("\nCAST:\n{}\n", cast_text(&snapshot));
            let notes: String = {
                let n = snapshot.notes.trim();
                n.chars().skip(n.chars().count().saturating_sub(1500)).collect()
            };
            if !notes.is_empty() {
                prompt += &format!("\nWORLD / PLOT MEMORY:\n{notes}\n");
            }
            prompt += &format!("\nSTORY SO FAR:\n{}\n", if so_far.is_empty() { "(this is the very beginning of the recap)" } else { &so_far });
            if !first {
                let said: Vec<&str> = story.segments.iter().rev().take(3).map(|s| s.text.as_str()).collect();
                prompt += &format!("\nLAST LINES ALREADY SPOKEN (do not repeat them):\n{}\n", said.into_iter().rev().collect::<Vec<_>>().join("\n"));
            }
            prompt += &format!("\nREADING OF {}:\n\n{}", if n_parts == 1 && !carry_on { "THE CHAPTER" } else { "THESE PAGES" }, digest(&read, transcript.as_ref(), &snapshot, chunk));
            prompt += match (first, very_start) {
                (true, true) => "Nothing has been told yet: these pages begin the story. Open it the way a great storyteller would, with one strong, story-driven first line (who the hero is, or what is at stake), then tell the pages in order. Do not jump ahead to later events, do not introduce the video or the manga, and keep slow setup (an ordinary life, a backstory) brisk. Include a good \"title\". ",
                (true, false) => "The viewer has just heard the story so far, with no break: continue it seamlessly from where it stands, as the same telling. No recap of earlier events, no re-introducing what the viewer already knows, no \"previously\" or \"now, in chapter ...\". Include a good \"title\". ",
                _ => "Carry straight on from the last lines already spoken. Keep the \"title\" the same as before or leave it empty. ",
            };
            if last {
                prompt += "The story goes on after these pages: tell them through to the last page and end on its last beat, so what comes next flows straight on. No wrap-up, no cliffhanger line, no \"end of chapter\", \"to be continued\" or sign-off. ";
            }
            if let Some(at) = reminder.filter(|at| !carry_on && (done..done + chunk.len()).contains(at)) {
                prompt += &call_to_action((at - done) as f64 / chunk.len() as f64);
                reminder = None;
            }
            prompt += "First list for yourself the scenes of these pages, then write the voice-over through to the last page. Reply with the JSON object only.";

            let req = request(model, vec![AiMessage::system(system.clone()), AiMessage::user(prompt)], vec![], Some(chapter_dir(ch).join("work")));
            let answer = match ask_json(llm, req, cancel).await {
                Ok(a) => a,
                Err(e) if is_cancelled(&e) => {
                    progress(Progress::Failed("Cancelled".into()));
                    return Err(e);
                }
                Err(e) => {
                    progress(Progress::Failed(format!("{}: {e}", ch.label)));
                    return Err(e);
                }
            };
            match answer {
                Ok(v) => {
                    if story.title.is_empty() {
                        story.title = str_of(&v, "title");
                    }
                    let seg = parse_segments(&v, chunk, &read);
                    if seg.is_empty() {
                        progress(Progress::Log(format!("{}: pages {}-{} came back without usable segments.", ch.label, chunk[0] + 1, chunk[chunk.len() - 1] + 1)));
                    }
                    // The writer stopped before the end of its pages: ask for the rest (twice at most).
                    if let Some(rest) = untold(&read, transcript.as_ref(), chunk, &seg).filter(|_| follow_ups < 2) {
                        follow_ups += 1;
                        progress(Progress::Log(format!("{}: the story stopped at page {}; writing the rest.", ch.label, rest[0])));
                        parts.push_front((rest, true));
                    }
                    story.segments.extend(seg);
                    let s = str_of(&v, "summary_so_far");
                    if !s.is_empty() {
                        so_far = truncate(&s, 2500);
                    }
                }
                Err(raw) => {
                    // Never leave a hole in the story: the chapter fails and shows as an issue to retry.
                    let msg = format!("{}: the AI's answer for pages {}-{} was not a story ({})", ch.label, chunk[0] + 1, chunk[chunk.len() - 1] + 1, truncate(&raw, 160));
                    progress(Progress::Failed(msg.clone()));
                    return Err(anyhow!(msg));
                }
            }
            if !carry_on {
                done += chunk.len();
            }
            progress(Progress::Page { done: done.min(content.len()), total: content.len(), page: chunk[chunk.len() - 1] });
        }

        if story.segments.is_empty() {
            progress(Progress::Log(format!("{}: no narration produced.", ch.label)));
            continue;
        }
        if story.title.is_empty() {
            story.title = ch.label.clone();
        }
        save_story(ch, &story)?;
        timeline.set_words(ch, story_words(&story));
        if !so_far.is_empty() {
            let mut b = brain.lock().unwrap();
            b.set_recap(&ch.label, &so_far);
            let _ = b.save();
        }
        results.push((ch.chapter_id.clone(), story));
    }

    if results.is_empty() {
        let msg = "No story could be written. Read the chapters first, then try again.".to_string();
        progress(Progress::Failed(msg.clone()));
        return Err(anyhow!(msg));
    }
    progress(Progress::Done);
    Ok(results)
}
