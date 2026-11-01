//! STORY phase: turn the chapter's reading (and edited transcript) into a story-first recap voice-over.
//! The writer sees the whole chapter at once (local models with small context windows get it in a few
//! parts), keeps only what matters to the story, and anchors each passage to the panel shown while it
//! is spoken; pages it leaves out are cut from the video. The story so far is carried between parts and
//! chapters, and kept in the brain so a chapter written on its own still knows what came before.

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
"page" and "panel" use the numbers shown in the reading (both start at 1). Segments must be in story order. Only the opening of the very first chapter may mark segments "hook": true (see its instructions)."##,
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

/// Segments of an answer on `chunk_pages`. Hook segments (`allow_hook`: the very first opening)
/// come first and may show any page; the rest follow in page order.
pub(crate) fn parse_segments(v: &Value, chunk_pages: &[usize], read: &ChapterRead, allow_hook: bool) -> Vec<StorySegment> {
    let mut out: Vec<StorySegment> = vec![];
    for sv in v.get("segments").and_then(Value::as_array).into_iter().flatten() {
        let text = str_of(sv, "text");
        let Some(page) = num_of(sv, "page").filter(|n| *n >= 1.0).map(|n| n as usize - 1) else { continue };
        let hook = allow_hook && sv.get("hook").and_then(Value::as_bool).unwrap_or(false);
        if text.is_empty() || !chunk_pages.contains(&page) && !(hook && read.pages.iter().any(|p| p.index == page)) {
            continue;
        }
        let panels = read.pages.iter().find(|p| p.index == page).map(|p| p.panels.len()).unwrap_or(0);
        let panel = num_of(sv, "panel").filter(|n| *n >= 1.0).map(|n| n as usize - 1).filter(|n| *n < panels);
        let sfx = SfxCue::parse(&str_of(sv, "sfx"), num_of(sv, "sfx_strength"));
        out.push(StorySegment { page, panel, text, delivery: None, sfx, hook });
    }
    // Stable: keeps the model's order of the hook, and of the panels within a page.
    out.sort_by_key(|s| (!s.hook, if s.hook { 0 } else { s.page }));
    out
}

/// The pages of `chunk` after the last one the answer narrated, when a real share of what the
/// chunk says (over 15% of its lines) is still untold: the writer stopped early.
fn untold(read: &ChapterRead, transcript: Option<&Transcript>, chunk: &[usize], segs: &[StorySegment]) -> Option<Vec<usize>> {
    let reached = segs.iter().filter(|s| !s.hook).map(|s| s.page).max()?;
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
    let llm = Patient::new(reg, progress.clone());
    write_story_with(&llm, &model, &chapters, &styles, &brain, &progress, &cancel).await
}

pub(crate) async fn write_story_with(
    llm: &dyn Llm,
    model: &ModelChoice,
    chapters: &[ChapterRef],
    styles: &[String],
    brain: &Mutex<Brain>,
    progress: &ProgressFn,
    cancel: &CancellationToken,
) -> Result<Vec<(String, Story)>> {
    let system = story_system(styles);
    let label = format!("{}/{}", model.provider.as_str(), model.model);
    let mut results: Vec<(String, Story)> = vec![];
    let mut so_far = String::new();

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
                "MANGA: {}\nCHAPTER: {} ({span})\nSTYLES: {}\nLENGTH TARGET: about {} words ({lines} lines of dialogue and narration on these pages)\n",
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
                (true, true) => "This is the very start of the recap. Open IN MEDIAS RES, like the best recap channels: 2-4 segments marked \"hook\": true that drop the viewer straight into the chapter's most gripping moment, clash or twist (anchored to that moment's own panels, from any page), told so the viewer instantly needs to know how it came to this. Then rewind with one line (\"But to understand how ..., we have to go back.\") and tell the chapter from its start, in order, without retelling the hook word for word. Keep slow setup (an ordinary life, a backstory) brisk. Include a good \"title\". ",
                (true, false) => "Open with ONE short sentence that reminds the viewer where the story left off, then go straight on; do not re-introduce what the viewer already knows. Include a good \"title\". ",
                _ => "Carry straight on from the last lines already spoken. Keep the \"title\" the same as before or leave it empty. ",
            };
            if last {
                prompt += "End on the chapter's last turn with a line that makes the viewer need the next chapter. ";
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
                    let seg = parse_segments(&v, chunk, &read, first && very_start);
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
