//! Titles, descriptions, hashtags and tags from the user's AI model (the same provider and model
//! the assistant uses).
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use mv_ai::{AiMessage, AiProviderId, AiStepRequest, Registry};
use serde_json::Value;

use crate::seo;
use crate::types::{MetaContext, MetaSuggestion};

fn line(u: &mut String, k: &str, v: &str) {
    if !v.trim().is_empty() {
        u.push_str(&format!("{k}: {}\n", v.trim()));
    }
}

/// The prompt: a YouTube growth editor for manga recap channels. Asks for JSON only:
/// `{"titles":[5],"description":"...","hashtags":[...],"tags":[...],"notes":"..."}`.
/// Titles: curiosity + the manga name + the chapter, under 70 characters, no clickbait lies.
/// Description: a hook line, a spoiler-light summary, "Previous episode: <link>" when known,
/// chapter-continuity line ("Watch the series in order: <playlist>"), a call to subscribe, then
/// the hashtags on the last line.
pub fn prompt(ctx: &MetaContext) -> (String, String) {
    let lang = if ctx.language.trim().is_empty() { "en" } else { ctx.language.trim() };
    let system = format!(
        "You are a YouTube growth editor for manga recap channels. You write titles, descriptions, hashtags and tags that get recaps found and clicked, without lying about the content.\n\
Rules:\n\
- Write in the language with BCP-47 code \"{lang}\".\n\
- Titles: exactly 5 options, best first. Each combines curiosity, the manga name and the chapter, stays under 70 characters, and never promises what the video does not show. No ALL CAPS, no emoji spam.\n\
- Description: a one-line hook, then a spoiler-light summary of what happens, then \"Previous episode: <link>\" when a previous episode is given, then a line telling viewers to watch the series in order (the series playlist), then a short call to subscribe, then the hashtags on the last line. Put the manga name in the first 150 characters. At least 200 characters in total.\n\
- hashtags: 3 to 5, each starting with '#', no spaces (for example \"#SoloLeveling\").\n\
- tags: 8 to 15 search tags without '#', under 500 characters together (manga name, character names, \"manga recap\", \"manhwa recap\" when it fits).\n\
- notes: one or two sentences on why these should do well.\n\
Reply with JSON only, no other text, exactly in this shape:\n\
{{\"titles\":[\"...\",\"...\",\"...\",\"...\",\"...\"],\"description\":\"...\",\"hashtags\":[\"#...\"],\"tags\":[\"...\"],\"notes\":\"...\"}}"
    );

    let mut u = String::new();
    line(&mut u, "Manga", &ctx.manga_title);
    line(&mut u, "Episode", &ctx.episode_label);
    line(&mut u, "Channel", &ctx.channel_title);
    if let Some((title, link)) = &ctx.previous {
        line(&mut u, "Previous episode", &format!("{title} - {link}"));
    }
    if !ctx.top_titles.is_empty() {
        line(&mut u, "Best videos on this channel (match what works)", &ctx.top_titles.iter().take(8).map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join("; "));
    }
    if let Some(v) = &ctx.current {
        u.push_str("\nThis video is already live and needs improving:\n");
        line(&mut u, "Current title", &v.title);
        line(&mut u, "Current description", &v.description.chars().take(1500).collect::<String>());
        line(&mut u, "Current tags", &v.tags.join(", "));
        u.push_str(&format!("Views so far: {}, likes: {}, comments: {}\n", v.views, v.likes, v.comments));
    }
    if !ctx.summary.trim().is_empty() {
        u.push_str(&format!("\nWhat happens in the recap:\n{}\n", ctx.summary.trim().chars().take(4000).collect::<String>()));
    }
    if !ctx.instruction.trim().is_empty() {
        u.push_str(&format!("\nThe user asks: {}\n", ctx.instruction.trim()));
    }
    u.push_str("\nWrite the JSON now.");
    (system, u)
}

/// First `{` to the last `}` of the text (a ```json fence or chatter around it is ignored).
fn extract_object(raw: &str) -> Option<Value> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<Value>(&raw[start..=end]).ok().filter(Value::is_object)
}

fn strings(v: &Value, key: &str) -> Vec<String> {
    match v.get(key) {
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        Some(Value::String(s)) => s.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        _ => vec![],
    }
}

/// Parses the model's answer (JSON, possibly inside a ```json fence or with text around it) and
/// cleans it with `seo::clean_*`. Hashtags missing from the description are appended to it.
pub fn parse(raw: &str) -> Option<MetaSuggestion> {
    let v = extract_object(raw)?;
    let mut titles: Vec<String> = Vec::new();
    for t in strings(&v, "titles").iter().chain(strings(&v, "title").iter()) {
        let t = seo::clean_title(t);
        if !t.is_empty() && !titles.iter().any(|x| x.to_lowercase() == t.to_lowercase()) {
            titles.push(t);
        }
    }
    titles.truncate(5);

    let mut description = v.get("description").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if titles.is_empty() && description.is_empty() {
        return None;
    }

    let mut hashtags: Vec<String> = Vec::new();
    for h in strings(&v, "hashtags") {
        let word: String = h.trim_start_matches('#').chars().filter(|c| c.is_alphanumeric() || *c == '_').collect();
        if word.is_empty() {
            continue;
        }
        let tag = format!("#{word}");
        if !hashtags.iter().any(|x| x.to_lowercase() == tag.to_lowercase()) {
            hashtags.push(tag);
        }
    }
    hashtags.truncate(5);
    let present = seo::hashtags_in(&description);
    let missing: Vec<&String> = hashtags.iter().filter(|h| !present.iter().any(|p| p.to_lowercase() == h.to_lowercase())).collect();
    if !missing.is_empty() {
        let line = missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ");
        description = if description.is_empty() { line } else { format!("{description}\n\n{line}") };
    }

    Some(MetaSuggestion {
        titles,
        description: seo::clean_description(&description),
        hashtags,
        tags: seo::clean_tags(&strings(&v, "tags")),
        notes: v.get("notes").and_then(Value::as_str).unwrap_or("").trim().to_string(),
    })
}

fn request(provider: AiProviderId, model: &str, messages: Vec<AiMessage>) -> AiStepRequest {
    static N: AtomicU64 = AtomicU64::new(0);
    AiStepRequest {
        request_id: format!("youtube-meta-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)),
        provider_id: provider,
        model: model.to_string(),
        messages,
        tools: None,
        effort: None,
        access: None,
        images: Vec::new(),
        workspace: None,
        lean: true,
        mcp: None,
    }
}

async fn step(registry: &Registry, req: AiStepRequest) -> Result<String> {
    let r = registry.run_step_cancellable(req, mv_ai::emit_noop(), mv_ai::CancellationToken::new()).await;
    if r.stopped {
        bail!("cancelled");
    }
    match r.error {
        Some(e) => Err(anyhow!(e)),
        None => Ok(r.text),
    }
}

/// One model call (lean, no tools) with the prompt above; retries once asking for JSON only when
/// the first answer does not parse.
pub async fn suggest(registry: Arc<Registry>, provider: AiProviderId, model: String, ctx: &MetaContext) -> Result<MetaSuggestion> {
    let (system, user) = prompt(ctx);
    let mut messages = vec![AiMessage::system(system), AiMessage::user(user)];
    let raw = step(&registry, request(provider, &model, messages.clone())).await?;
    if let Some(m) = parse(&raw) {
        return Ok(m);
    }
    messages.push(AiMessage::assistant(raw));
    messages.push(AiMessage::user("Your previous answer was not valid JSON. Reply with the JSON object only."));
    let raw2 = step(&registry, request(provider, &model, messages)).await?;
    parse(&raw2).ok_or_else(|| anyhow!("The AI did not return usable titles and a description. Try again."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Video;

    #[test]
    fn parses_fenced_json_and_cleans() {
        let raw = "Sure!\n```json\n{\"titles\":[\"A <b>\",\"Two\",\"two\",\"3\",\"4\",\"5\",\"6\"],\"description\":\"Hook line.\",\"hashtags\":[\"#One Piece\",\"Manga\",\"#Recap\"],\"tags\":[\"#manga\",\"Manga\",\"recap\"],\"notes\":\" why \"}\n```\nEnjoy";
        let m = parse(raw).unwrap();
        assert_eq!(m.titles, vec!["A b", "Two", "3", "4", "5"]);
        assert_eq!(m.hashtags, vec!["#OnePiece", "#Manga", "#Recap"]);
        assert!(m.description.starts_with("Hook line.") && m.description.ends_with("#OnePiece #Manga #Recap"));
        assert_eq!(m.tags, vec!["manga", "recap"]);
        assert_eq!(m.notes, "why");
    }

    #[test]
    fn hashtags_already_in_description_are_not_repeated() {
        let m = parse(r##"{"titles":["T"],"description":"Hi #Manga","hashtags":["#manga","#New"]}"##).unwrap();
        assert_eq!(m.description, "Hi #Manga\n\n#New");
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("no json here").is_none());
        assert!(parse("{\"notes\":\"x\"}").is_none());
        assert!(parse("} {").is_none());
    }

    #[test]
    fn prompt_carries_the_context() {
        let ctx = MetaContext {
            manga_title: "Solo Leveling".into(),
            episode_label: "Chapter 3".into(),
            summary: "Jinwoo enters the dungeon.".into(),
            previous: Some(("Ch 2".into(), "https://youtu.be/x".into())),
            current: Some(Video { title: "Old".into(), views: 12, ..Default::default() }),
            instruction: "more dramatic".into(),
            language: "en".into(),
            ..Default::default()
        };
        let (sys, user) = prompt(&ctx);
        assert!(sys.contains("JSON") && sys.contains("\"en\""));
        for s in ["Solo Leveling", "Chapter 3", "Jinwoo", "https://youtu.be/x", "Old", "more dramatic"] {
            assert!(user.contains(s), "missing {s}");
        }
    }
}
