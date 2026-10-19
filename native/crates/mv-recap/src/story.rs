//! STORY phase: turn the chapter's reading (and edited transcript) into an anime-recap voice-over script,
//! ten pages per model call with a running "story so far" carried between calls.

use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use mv_ai::{AiMessage, CancellationToken, Registry};
use serde_json::Value;

use crate::brain::Brain;
use crate::json::{num_of, str_of};
use crate::llm::{ask_json, is_cancelled, request, truncate, Llm};
use crate::read::roster_text;
use crate::store::*;
use crate::transcript::format_line;
use crate::types::*;

const CHUNK_PAGES: usize = 10;

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
    let blend = if styles.len() > 1 { "Blend these styles into ONE consistent narrator voice (do not alternate between them page by page). " } else { "" };
    format!(
        r##"You are the voice-over writer for an anime-style recap video of a manga. You receive a digest of manga pages (scene tags, what each panel shows, who is in it, the dialogue) and write the narration that will be spoken over the images, in order.

NARRATION STYLE
{style_text}
{blend}Whatever the style, scene depth drives the pacing: fight and action scenes are punchy and fast with short sentences; emotional and sad scenes slow down and turn tender, with longer breaths; comedy gets light timing; reveals build up and then land; tension stays quiet and tight; calm scenes are relaxed; flashbacks are signalled clearly ("Years earlier...").

RULES
1. Third person, present tense, engaging, like a recap channel narrator. Tell the story; do not describe drawings ("in this panel", "the page shows" are forbidden).
2. Explain who is who: use the character names from the CHARACTER BANK, and the first time an important character appears give a few words on who they are. If someone only has a descriptive placeholder name, use it naturally.
3. Do NOT read every line. Paraphrase the key dialogue and keep only a few short, punchy quotes. Skip filler, repeated sound effects and trivia. Never invent plot that the digest does not support.
4. Cover every page that has content, in order. Each segment is 1-4 sentences of spoken text (about 8-60 words). A busy page may have several segments, tied to a panel; a quiet page may have one. Plain text only: no markdown, emojis, brackets, stage directions or page numbers, because it is read aloud by a text-to-speech voice.
5. Keep continuity with the STORY SO FAR; do not repeat what was already narrated.

OUTPUT: STRICT JSON only, one object, no code fences, no commentary:
{{"title": "short catchy title for this chapter's recap",
  "summary_so_far": "3-5 sentences: everything that has happened so far (including earlier parts), used as memory for the next part",
  "segments": [
    {{"page": 3, "panel": 1, "text": "Ruko stands alone in the rain, sword trembling in his hand."}},
    {{"page": 3, "panel": null, "text": "..."}}
  ]}}
"page" and "panel" use the numbers shown in the digest (both start at 1). Use "panel": null when a segment spans the whole page. Segments must be in story order."##
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
                line += &format!(" {}", pn.description);
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
    write_story_with(&reg, &model, &chapters, &styles, &brain, &progress, &cancel).await
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
        let chunks: Vec<&[usize]> = content.chunks(CHUNK_PAGES).collect();
        let mut done = 0;

        for (ci, chunk) in chunks.iter().enumerate() {
            if cancel.is_cancelled() {
                progress(Progress::Failed("Cancelled".into()));
                return Err(anyhow!("cancelled"));
            }
            let last = ci + 1 == chunks.len();
            let mut prompt = format!(
                "MANGA: {}\nCHAPTER: {} (part {} of {}; pages {}-{} of this chapter's {} read pages)\nSTYLES: {}\n",
                snapshot.manga_title,
                ch.label,
                ci + 1,
                chunks.len(),
                chunk[0] + 1,
                chunk[chunk.len() - 1] + 1,
                content.len(),
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
            prompt += &format!("\nDIGEST OF THESE PAGES:\n\n{}", digest(&read, transcript.as_ref(), &snapshot, chunk));
            prompt += if ci == 0 { "Include a good \"title\". " } else { "Keep the \"title\" the same as before or leave it empty. " };
            if last {
                prompt += "This is the final part of the chapter: finish on a line that makes the viewer want the next chapter. ";
            }
            prompt += "Write the voice-over now. Reply with the JSON object only.";

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
                        so_far = truncate(&s, 1500);
                    }
                }
                Err(_) => progress(Progress::Log(format!("{}: part {} was not valid JSON, skipped.", ch.label, ci + 1))),
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
