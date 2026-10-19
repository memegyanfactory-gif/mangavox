//! TRANSCRIPT phase: editable per-page text derived from `read.json`, plus AI help on a single page.

use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use mv_ai::{AiMessage, CancellationToken, Registry};

use crate::brain::Brain;
use crate::llm::{request, Llm};
use crate::read::{prep_page, roster_text};
use crate::store::*;
use crate::types::*;

/// One line of transcript text for a [`Line`].
pub(crate) fn format_line(l: &Line) -> String {
    match (l.kind.as_str(), l.speaker.as_deref()) {
        ("narration", _) => format!("[Narration] {}", l.text),
        ("sfx", _) => format!("[SFX] {}", l.text),
        ("thought", Some(s)) => format!("{s} (thought): {}", l.text),
        ("thought", None) => format!("(thought): {}", l.text),
        (_, Some(s)) => format!("{s}: {}", l.text),
        (_, None) => l.text.clone(),
    }
}

/// Plain dialogue transcript, one line per balloon in reading order. Does not look at disk.
pub fn transcript_from_read(read: &ChapterRead) -> Transcript {
    Transcript {
        pages: read
            .pages
            .iter()
            .map(|p| TranscriptPage {
                index: p.index,
                text: p.panels.iter().flat_map(|pn| pn.lines.iter().map(format_line)).collect::<Vec<_>>().join("\n"),
                edited: false,
            })
            .collect(),
    }
}

/// Fresh AI transcript merged over an existing one: pages the user edited are never overwritten.
pub fn merge_transcript(existing: Option<Transcript>, fresh: Transcript) -> Transcript {
    let Some(old) = existing else { return fresh };
    let mut pages: Vec<TranscriptPage> = old.pages.iter().filter(|p| p.edited).cloned().collect();
    for p in fresh.pages {
        if !pages.iter().any(|x| x.index == p.index) {
            pages.push(p);
        }
    }
    pages.sort_by_key(|p| p.index);
    Transcript { pages }
}

/// Regenerate `transcript.json` from `read` while keeping edited pages; saves and returns the result.
pub fn update_transcript(ch: &ChapterRef, read: &ChapterRead) -> Result<Transcript> {
    let t = merge_transcript(load_transcript(ch), transcript_from_read(read));
    save_transcript(ch, &t)?;
    Ok(t)
}

const REWRITE_SYSTEM: &str = "You are a careful script editor for a manga recap video. You are given ONE manga page image and its current transcript text (dialogue the narrator will work from). Apply the user's instruction to the transcript of this page and reply with the new page text ONLY: plain text, one balloon per line in the same `Speaker: text` format, no JSON, no markdown, no explanations, no quotes around the answer. Look at the image to verify who speaks and what is said; use the character names from the roster; never invent dialogue that is not on the page unless the instruction asks for it. Do not run commands or open other files.";

/// Ask the model to fix / reword / re-attribute one page of the transcript (the page image is attached).
pub async fn rewrite_page(
    reg: Arc<Registry>,
    model: ModelChoice,
    ch: ChapterRef,
    page_index: usize,
    current_text: String,
    instruction: String,
    brain_snapshot: Brain,
    cancel: CancellationToken,
) -> Result<String> {
    rewrite_page_with(&reg, &model, &ch, page_index, &current_text, &instruction, &brain_snapshot, &cancel).await
}

pub(crate) async fn rewrite_page_with(
    llm: &dyn Llm,
    model: &ModelChoice,
    ch: &ChapterRef,
    page_index: usize,
    current_text: &str,
    instruction: &str,
    brain: &Brain,
    cancel: &CancellationToken,
) -> Result<String> {
    let pages = list_pages(&ch.pages_dir);
    let page = pages.get(page_index).cloned().ok_or_else(|| anyhow!("page {} does not exist", page_index + 1))?;
    let work = chapter_dir(ch).join("work");
    std::fs::create_dir_all(&work).context("create studio work dir")?;
    let (img, _, _) = tokio::task::spawn_blocking(move || prep_page(&page, &work, page_index)).await.map_err(|e| anyhow!(e))??;
    let summary = load_read(ch).and_then(|r| r.pages.into_iter().find(|p| p.index == page_index)).map(|p| p.summary).unwrap_or_default();
    let prompt = format!(
        "MANGA: {}\nCHAPTER: {} - page {}\n{}\nCHARACTER BANK:\n{}\n\nCURRENT TRANSCRIPT OF THIS PAGE:\n{}\n\nINSTRUCTION: {}\n\nReply with the new transcript text only.",
        brain.manga_title,
        ch.label,
        page_index + 1,
        if summary.is_empty() { String::new() } else { format!("PAGE SUMMARY: {summary}") },
        roster_text(brain, 40),
        if current_text.trim().is_empty() { "(empty)" } else { current_text },
        instruction
    );
    let req = request(model, vec![AiMessage::system(REWRITE_SYSTEM), AiMessage::user(prompt)], vec![img], Some(chapter_dir(ch).join("work")));
    let out = llm.step(req, cancel.clone()).await?;
    Ok(clean_plain(&out))
}

/// Strip code fences and wrapping quotes from a plain-text answer.
pub(crate) fn clean_plain(s: &str) -> String {
    let mut t = s.trim();
    if let Some(rest) = t.strip_prefix("```") {
        t = rest.split_once('\n').map(|x| x.1).unwrap_or(rest);
        t = t.trim_end().strip_suffix("```").unwrap_or(t);
    }
    let t = t.trim();
    let t = if t.len() > 1 && t.starts_with('"') && t.ends_with('"') && !t[1..t.len() - 1].contains('"') { &t[1..t.len() - 1] } else { t };
    t.trim().to_string()
}
