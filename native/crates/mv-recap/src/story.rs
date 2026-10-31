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
use crate::read::roster_text;
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
- Dialogue is retold, not read out: "Liam explains that only a swordmaster can kill a swordmaster." "She makes him promise to give up on revenge and simply survive." Quote a line word for word only when it is a killer line, and keep it short: "Eat my sword."
- Short sentences hit hard at the turns ("He was wrong." "The whole massacre took ten minutes."), then the story moves on.
- Rules of the world, powers, ranks and secrets are explained in one clear sentence the moment they matter.
- Time jumps and flashbacks are signposted: "One month passes." "Years earlier..." "Back in the present..."
- First-person narration boxes and inner monologue ("I", "me", "my") are the thoughts of the character they belong to, usually the protagonist: retell them in third person with that character's name. Never say "the narrator".
- Call characters by name: the CHARACTER BANK, or a name heard in the dialogue anywhere in the chapter (read ahead: a stranger introduced on page 30 is still named when they first appear). The first time someone matters, add a few words on who they are ("Liam Karavan, the legendary founder of the house"). Never call a named character by their looks ("a silver-haired man", "the blond youth"); a nameless extra gets a role ("a bandit", "the old butler").

WHAT TO LEAVE OUT
- Never describe the artwork: no colours, lighting, clothes, hair, camera angles, backgrounds or effects ("darkness fills the screen", "blue light glows", "in this panel", "the page shows", "we see" are all forbidden). The viewer already sees the picture; you tell what it MEANS for the story.
- Skip filler completely: sound effects, establishing shots, scenery, reaction shots, repeated beats, small talk that changes nothing, title / credit / author pages, re-tellings of what was already covered. Pages that add nothing to the story get no segment at all and are cut from the video, so never mention them.
- Never invent events, motives or names the reading does not support.

NARRATION STYLE
{style_text}
{blend}Pacing follows the scene: fights are fast and punchy, emotional moments slow down and breathe, reveals build up and then land, comedy gets light timing.

LENGTH
Tell everything that matters to the story and nothing else. Length follows the story, not the page count: a chapter packed with events may need 800-1200 words, a quiet one 250-500. Never pad.

SEGMENTS
The voice-over is a list of segments in story order. Each segment is 1-3 sentences (about 12-45 words) and is shown over ONE panel: pick the panel that best shows what the segment says, preferably one with the characters or the action in it; never a text-only box, an empty or nearly black panel, or a sound effect. A segment may sum up several pages; anchor it to the most telling panel among them. Move to a new panel every segment or two so the picture keeps up with the story. Plain text only: no markdown, emojis, brackets, stage directions or page numbers, because a text-to-speech voice reads it.

OUTPUT: STRICT JSON only, one object, no code fences, no commentary:
{{"title": "short catchy title for this chapter's recap",
  "summary_so_far": "6-10 sentences with names: the whole story so far, this chapter included, used as memory for what comes next",
  "segments": [
    {{"page": 3, "panel": 2, "text": "For one year, Arhan has hunted a single name: Karlos, the man who slaughtered his family."}},
    {{"page": 7, "panel": 1, "text": "..."}}
  ]}}
"page" and "panel" use the numbers shown in the reading (both start at 1). Segments must be in story order."##
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
        out.push(StorySegment { page, panel, text, delivery: None });
    }
    out.sort_by_key(|s| s.page); // stable: keeps the model's panel order within a page
    out
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
        let chunks = plan_parts(&sizes, part_budget(model));
        let mut done = 0;
        // Run on its own, the chapter starts from the story so far as of the chapter before it.
        if so_far.is_empty() {
            so_far = snapshot.recap_before(&ch.label).unwrap_or_default().to_string();
        }

        for (ci, chunk) in chunks.iter().enumerate() {
            if cancel.is_cancelled() {
                progress(Progress::Failed("Cancelled".into()));
                return Err(anyhow!("cancelled"));
            }
            let last = ci + 1 == chunks.len();
            let span = if chunks.len() == 1 {
                format!("the whole chapter, {} pages", content.len())
            } else {
                format!("part {} of {}; pages {}-{} of this chapter's {} pages", ci + 1, chunks.len(), chunk[0] + 1, chunk[chunk.len() - 1] + 1, content.len())
            };
            let mut prompt = format!(
                "MANGA: {}\nCHAPTER: {} ({span})\nSTYLES: {}\n",
                snapshot.manga_title,
                ch.label,
                if styles.is_empty() { "Normal narration".to_string() } else { styles.join(", ") }
            );
            prompt += &format!("\nCHARACTER BANK (name, aliases, looks):\n{}\n", roster_text(&snapshot, 40));
            let notes: String = {
                let n = snapshot.notes.trim();
                n.chars().skip(n.chars().count().saturating_sub(1500)).collect()
            };
            if !notes.is_empty() {
                prompt += &format!("\nWORLD / PLOT MEMORY:\n{notes}\n");
            }
            prompt += &format!("\nSTORY SO FAR:\n{}\n", if so_far.is_empty() { "(this is the very beginning of the recap)" } else { &so_far });
            prompt += &format!("\nREADING OF {}:\n\n{}", if chunks.len() == 1 { "THE CHAPTER" } else { "THESE PAGES" }, digest(&read, transcript.as_ref(), &snapshot, chunk));
            prompt += match (ci, so_far.is_empty()) {
                (0, true) => "This is the very start of the recap: open with a gripping cold open of 2-4 short sentences (who the protagonist is and what is at stake), then tell the story from the beginning. Include a good \"title\". ",
                (0, false) => "Open with ONE short sentence that reminds the viewer where the story left off, then go straight on; do not re-introduce what the viewer already knows. Include a good \"title\". ",
                _ => "Carry straight on from where the previous part ended. Keep the \"title\" the same as before or leave it empty. ",
            };
            if last {
                prompt += "End on the chapter's last turn with a line that makes the viewer need the next chapter. ";
            }
            prompt += "First decide which moments carry the story, then write the voice-over. Reply with the JSON object only.";

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
                        progress(Progress::Log(format!("{}: part {} came back without usable segments.", ch.label, ci + 1)));
                    }
                    story.segments.extend(seg);
                    let s = str_of(&v, "summary_so_far");
                    if !s.is_empty() {
                        so_far = truncate(&s, 2500);
                    }
                }
                Err(raw) => {
                    // Never leave a hole in the story: the chapter fails and shows as an issue to retry.
                    let msg = format!("{}: the AI's answer for part {} was not a story ({})", ch.label, ci + 1, truncate(&raw, 160));
                    progress(Progress::Failed(msg.clone()));
                    return Err(anyhow!(msg));
                }
            }
            done += chunk.len();
            progress(Progress::Page { done, total: content.len(), page: chunk[chunk.len() - 1] });
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
