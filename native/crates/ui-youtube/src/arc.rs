//! Automation's Write step, first half: the whole story in one pass. Once every chapter is read,
//! all their transcripts go to the model together and it writes the arc: who everyone is, what each
//! chapter does for the whole, what pays off later and the ending's shape. The arc is saved next to
//! the manga's Studio files (`arc.md`) and, condensed, into the manga's brain, which every
//! chapter's narration reads, so each chapter is written knowing where the story goes rather than
//! only what came before.
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use mv_ai::{AiMessage, AiProviderId, AiStepRequest, Registry};
use ui_studio::data::ChapterItem;

/// What fits in one prompt (characters of transcript, spread over the chapters).
const BUDGET: usize = 120_000;
/// The condensed arc kept in the brain (its notes hold 4,000 characters in all).
const BRAIN_ARC: usize = 1_600;

fn transcript_text(c: &ChapterItem) -> String {
    let chref = ui_studio::pipeline::QChapter::from_item(c).chref();
    mv_recap::load_transcript(&chref).map(|t| t.pages.iter().map(|p| p.text.trim()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

/// The prompt: every chapter, each cut to its share of [`BUDGET`].
pub fn prompt(title: &str, chapters: &[(String, String)]) -> (String, String) {
    let system = "You are the head writer of a manga recap channel. You get the transcripts of every chapter of a manga at once. Write the arc that the narration of each chapter will follow, so every chapter is told knowing the whole story.\n\
Reply in plain text with exactly these parts:\n\
ARC: 4-6 sentences, the whole story from start to the last chapter given.\n\
CAST: one line per main character: name - who they are - how they change.\n\
CHAPTERS: one line per chapter: its label - what it does for the whole story (setup, turn, payoff).\n\
THREADS: mysteries, promises and foreshadowing, each with the chapter where it is set up and where it pays off.\n\
TONE: one line on how the narration should feel.\n\
Use the names exactly as the transcripts spell them. No spoilers beyond the last chapter given."
        .to_string();
    let share = (BUDGET / chapters.len().max(1)).max(800);
    let mut u = format!("Manga: {title}\nChapters: {}\n\n", chapters.len());
    for (label, text) in chapters {
        let cut: String = text.chars().take(share).collect();
        u.push_str(&format!("=== {label} ===\n{cut}\n\n"));
    }
    u.push_str("Write the arc now.");
    (system, u)
}

/// The arc's most useful lines for the brain: ARC and TONE first, then CAST, cut to `max`.
pub fn condense(arc: &str, max: usize) -> String {
    let mut parts: Vec<&str> = vec![];
    for key in ["ARC:", "TONE:", "CAST:", "THREADS:"] {
        if let Some(i) = arc.find(key) {
            let rest = &arc[i..];
            let end = ["ARC:", "CAST:", "CHAPTERS:", "THREADS:", "TONE:"].iter().filter(|k| **k != key).filter_map(|k| rest.find(k)).filter(|p| *p > 0).min().unwrap_or(rest.len());
            parts.push(rest[..end].trim());
        }
    }
    let joined = if parts.is_empty() { arc.trim().to_string() } else { parts.join(" ") };
    let flat = joined.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        format!("{}…", flat.chars().take(max - 1).collect::<String>())
    }
}

/// Writes the arc of `items` (all read) and stores it. Returns its length in characters.
pub async fn write(registry: Arc<Registry>, provider: AiProviderId, model: String, manga_id: &str, title: &str, items: &[ChapterItem]) -> Result<usize> {
    let chapters: Vec<(String, String)> = items.iter().map(|c| (c.label(), transcript_text(c))).filter(|(_, t)| !t.is_empty()).collect();
    if chapters.is_empty() {
        bail!("no transcripts to read");
    }
    let (system, user) = prompt(title, &chapters);
    let req = AiStepRequest {
        request_id: format!("automation-arc-{}", std::process::id()),
        provider_id: provider,
        model,
        messages: vec![AiMessage::system(system), AiMessage::user(user)],
        tools: None,
        effort: None,
        access: None,
        images: Vec::new(),
        workspace: None,
        lean: true,
        mcp: None,
    };
    let r = registry.run_step_cancellable(req, mv_ai::emit_noop(), mv_ai::CancellationToken::new()).await;
    if let Some(e) = r.error {
        return Err(anyhow!(e));
    }
    let arc = r.text.trim().to_string();
    if arc.len() < 200 {
        bail!("the model's arc was too short");
    }
    let dir = mv_recap::media::manga_dir(manga_id);
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(dir.join("arc.md"), format!("# {title}: the whole story\n\n{arc}\n"))?;
    let mut brain = mv_recap::Brain::load(manga_id, title);
    brain.notes = brain.notes.lines().filter(|l| !l.starts_with("- Whole story (all chapters):")).collect::<Vec<_>>().join("\n");
    brain.add_note(&format!("Whole story (all chapters): {}", condense(&arc, BRAIN_ARC)));
    brain.save()?;
    Ok(arc.chars().count())
}

#[cfg(test)]
mod tests {
    use super::{condense, prompt};

    #[test]
    fn prompt_shares_the_budget() {
        let long = "x".repeat(200_000);
        let (_, u) = prompt("Solo", &[("Chapter 1".into(), long.clone()), ("Chapter 2".into(), long)]);
        assert!(u.len() < 125_000);
        assert!(u.contains("=== Chapter 2 ==="));
    }

    #[test]
    fn condense_keeps_arc_and_tone_first() {
        let arc = "ARC: Jin rises.\nCAST:\nJin - hunter\nCHAPTERS:\nCh 1 - setup\nTHREADS: the key\nTONE: grim";
        let c = condense(arc, 200);
        assert!(c.starts_with("ARC: Jin rises. TONE: grim"), "{c}");
        assert!(!c.contains("setup"));
        assert!(condense(arc, 10).chars().count() <= 10);
    }
}
