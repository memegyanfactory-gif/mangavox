//! CLI agents are stateless here: each turn gets the whole (trimmed) conversation as one prompt.

use crate::types::{AiMessage, AiRole};

const BUDGET: usize = 24_000;

pub fn build_transcript(messages: &[AiMessage]) -> String {
    let system = messages.iter().filter(|m| m.role == AiRole::System).map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n\n");
    let turns: Vec<&AiMessage> = messages.iter().filter(|m| m.role != AiRole::System).collect();
    let mut lines: Vec<String> = Vec::new(); // newest first, reversed afterwards
    let mut budget = BUDGET;
    for i in (0..turns.len()).rev() {
        if budget == 0 {
            break;
        }
        let m = turns[i];
        let who = match m.role {
            AiRole::User => "User".to_string(),
            AiRole::Assistant => "Assistant".to_string(),
            _ => format!("Tool result ({})", m.tool_name.as_deref().unwrap_or("tool")),
        };
        let is_latest = i == turns.len() - 1;
        let len = m.content.chars().count();
        // Older turns that no longer fit are dropped whole; only the latest is ever shortened.
        if !is_latest && len > budget {
            break;
        }
        let body = if len > budget {
            let mut s: String = m.content.chars().take(budget).collect();
            s.push('…');
            s
        } else {
            m.content.clone()
        };
        budget = budget.saturating_sub(body.chars().count().min(len));
        lines.push(format!("{who}: {body}"));
    }
    lines.reverse();
    let last = lines.pop().unwrap_or_default();
    let history = lines;
    let last = last.strip_prefix("User: ").unwrap_or(&last).to_string();
    let mut parts: Vec<String> = Vec::new();
    if !system.is_empty() {
        parts.push(format!("<instructions>\n{system}\n</instructions>"));
    }
    if !history.is_empty() {
        parts.push(format!("<conversation>\n{}\n</conversation>", history.join("\n\n")));
    }
    parts.push(format!("Reply to this latest message:\n{last}"));
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_transcript_keeps_instructions_history_and_the_latest_message() {
        let out = build_transcript(&[
            AiMessage::system("Be brief."),
            AiMessage::user("Hi"),
            AiMessage::assistant("Hello!"),
            AiMessage::user("Recommend an anime"),
        ]);
        assert!(out.contains("<instructions>\nBe brief.\n</instructions>"));
        assert!(out.contains("User: Hi\n\nAssistant: Hello!"));
        assert!(out.ends_with("Reply to this latest message:\nRecommend an anime"));
    }

    #[test]
    fn cli_transcript_drops_the_oldest_turns_when_too_long() {
        let long = "x".repeat(20_000);
        let out = build_transcript(&[
            AiMessage::user(format!("old {long}")),
            AiMessage::assistant(long.clone()),
            AiMessage::user("newest question"),
        ]);
        assert!(!out.contains("old x"));
        assert!(out.ends_with("newest question"));
    }

    #[test]
    fn latest_message_is_shortened_not_dropped() {
        let out = build_transcript(&[AiMessage::user("y".repeat(30_000))]);
        assert!(out.ends_with('…'));
        assert!(out.chars().count() < 24_100);
    }
}
