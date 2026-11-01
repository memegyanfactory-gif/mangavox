//! The composer's slash commands (`/clear`, `/model`, `/make <manga>`…) and `@` mentions
//! (`@manga`, `@chapter`, `@video`, `@file`, `@page`): parsing, the autocomplete lists and how an
//! attached item becomes context the model sees. Pure logic; the dock does the IO and drawing.
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::store::{ChatItem, ItemRole};

// ── Slash commands ───────────────────────────────────────────────────────────────

/// What a command does when it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Clear,
    New,
    Model,
    Help,
    Retry,
    Stop,
    Export,
    Compact,
    Tools,
    /// Sent to the model as this prompt; `{}` is replaced by the command's arguments.
    Prompt(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Command {
    pub name: &'static str,
    /// Argument hint shown in the popup ("<manga> [chapters]"), empty when none.
    pub args: &'static str,
    pub about: &'static str,
    pub action: Action,
}

const fn cmd(name: &'static str, args: &'static str, about: &'static str, action: Action) -> Command {
    Command { name, args, about, action }
}

pub const COMMANDS: &[Command] = &[
    cmd("new", "", "Start a new chat", Action::New),
    cmd("clear", "", "Clear this conversation", Action::Clear),
    cmd("model", "[name]", "Switch the model or provider", Action::Model),
    cmd("help", "", "Commands and what the assistant can do", Action::Help),
    cmd("retry", "", "Ask the last message again", Action::Retry),
    cmd("stop", "", "Stop the reply", Action::Stop),
    cmd("export", "", "Save this chat as Markdown", Action::Export),
    cmd("compact", "", "Summarise older messages to save context", Action::Compact),
    cmd("tools", "[search]", "List the tools the assistant can use", Action::Tools),
    cmd("make", "<manga> [chapters]", "Make recap videos in Studio", Action::Prompt("In Studio, make recap videos for {}. Check studio_library first; if I did not name chapters, list the downloaded ones and ask which.")),
    cmd("download", "<manga> [chapters]", "Find a manga and download chapters", Action::Prompt("Find {} (library first, then the sources) and download the chapters I named; if I did not name any, show the chapters and ask.")),
    cmd("status", "", "Studio and download progress", Action::Prompt("How are the Studio queue and the downloads doing? Use studio_status and manga_downloads.")),
    cmd("recap", "<manga>", "Export the full recap video", Action::Prompt("Export the full recap of {} as one video (studio_export).")),
    cmd("motion", "<idea>", "Design a custom motion-graphic video", Action::Prompt("Make a custom motion-graphic video: {}. Use motion_assets for images and sounds, check it with motion_preview, then render it with motion_compose.")),
    cmd("play", "<title>", "Play an anime, movie or recap video", Action::Prompt("Play {}.")),
    cmd("guide", "[topic]", "How the app works, step by step", Action::Prompt("Explain how to use MangaVox{} (use app_guide), briefly.")),
];

pub fn find_command(name: &str) -> Option<&'static Command> {
    let n = name.trim().trim_start_matches('/').to_lowercase();
    COMMANDS.iter().find(|c| c.name == n)
}

/// `/name args` → the command and its (trimmed) arguments. `None` for ordinary text and for an
/// unknown `/word` (sent as a message, e.g. a path like `/usr/bin`).
pub fn parse(text: &str) -> Option<(&'static Command, String)> {
    let t = text.trim_start();
    let rest = t.strip_prefix('/')?;
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    find_command(name).map(|c| (c, args.trim().to_string()))
}

/// The command popup's query while the first word is still being typed (`/mo` → `mo`).
pub fn slash_query(text: &str) -> Option<&str> {
    let q = text.strip_prefix('/')?;
    (!q.contains(char::is_whitespace)).then_some(q)
}

/// Commands for the popup: name prefix matches first, then names / descriptions containing it.
pub fn filter_commands(query: &str) -> Vec<&'static Command> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return COMMANDS.iter().collect();
    }
    let mut prefix: Vec<&Command> = COMMANDS.iter().filter(|c| c.name.starts_with(&q)).collect();
    let rest = COMMANDS.iter().filter(|c| !c.name.starts_with(&q) && (c.name.contains(&q) || c.about.to_lowercase().contains(&q)));
    prefix.extend(rest);
    prefix
}

/// The prompt a [`Action::Prompt`] command sends.
pub fn expand(template: &str, args: &str) -> String {
    let args = args.trim();
    if template.contains("{}") {
        let filled = if template.starts_with("Explain how to use MangaVox") { if args.is_empty() { String::new() } else { format!(" ({args})") } } else if args.is_empty() { "the one I mean (ask me)".into() } else { args.to_string() };
        template.replace("{}", &filled)
    } else if args.is_empty() {
        template.to_string()
    } else {
        format!("{template} {args}")
    }
}

/// The `/help` text: every command, then what the assistant can do.
pub fn help_markdown() -> String {
    let mut s = String::from("**Commands**\n\n");
    for c in COMMANDS {
        s.push_str(&format!("- `/{}{}` — {}\n", c.name, if c.args.is_empty() { String::new() } else { format!(" {}", c.args) }, c.about));
    }
    s.push_str("\n**Attach context with @**\n\n- `@manga`, `@chapter`, `@page`, `@video`, `@file` — the item's ids and paths go to the assistant with your message.\n");
    s.push_str("\n**Keys**\n\n- Enter sends, Shift+Enter adds a line, ↑ in an empty box edits your last message, Esc closes a popup or stops the reply.\n");
    s.push_str("\n**The assistant can**\n\n- find manga in your library and sources, add them, download chapters and follow the downloads;\n- run Studio: generate recaps (all steps or any range), retry, pause, change voice, look, captions and music, export whole videos, parts or reels, make thumbnails;\n- design custom motion-graphic videos (scenes, text, keyframes, effects, narration, music, sound effects);\n- search and play anime, movies and series, and manage your YouTube uploads.\n");
    s
}

// ── @ mentions ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MentionKind {
    Manga,
    Chapter,
    Page,
    Video,
    File,
}

impl MentionKind {
    pub const ALL: [MentionKind; 5] = [MentionKind::Manga, MentionKind::Chapter, MentionKind::Page, MentionKind::Video, MentionKind::File];

    pub fn key(self) -> &'static str {
        match self {
            MentionKind::Manga => "manga",
            MentionKind::Chapter => "chapter",
            MentionKind::Page => "page",
            MentionKind::Video => "video",
            MentionKind::File => "file",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            MentionKind::Manga => "A manga from your library or a search",
            MentionKind::Chapter => "A chapter of a manga",
            MentionKind::Page => "A page of a downloaded chapter",
            MentionKind::Video => "A Studio video, export or motion render",
            MentionKind::File => "Pick any file on this PC",
        }
    }

    pub fn parse(s: &str) -> Option<MentionKind> {
        MentionKind::ALL.into_iter().find(|k| k.key() == s.trim().to_lowercase())
    }
}

/// An item attached with `@`: shown as a chip, sent as structured context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mention {
    pub kind: MentionKind,
    pub label: String,
    /// The app's id (manga id, chapter id), empty for files.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// Extra facts for the model ("manga id …", "source …", "page 3").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}

impl Mention {
    pub fn new(kind: MentionKind, label: impl Into<String>) -> Self {
        Mention { kind, label: label.into(), id: String::new(), detail: String::new(), path: None }
    }
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }
    pub fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = d.into();
        self
    }
    pub fn path(mut self, p: impl Into<PathBuf>) -> Self {
        self.path = Some(p.into());
        self
    }

    /// One line of the context block.
    pub fn context_line(&self) -> String {
        let mut parts = vec![format!("{}: \"{}\"", self.kind.key(), self.label)];
        if !self.id.is_empty() {
            parts.push(format!("id {}", self.id));
        }
        if !self.detail.is_empty() {
            parts.push(self.detail.clone());
        }
        if let Some(p) = &self.path {
            parts.push(format!("path {}", p.display()));
        }
        format!("- {}", parts.join(", "))
    }
}

/// What the mention popup is looking for: the token being typed at the end of the text.
#[derive(Debug, Clone, PartialEq)]
pub struct MentionQuery {
    /// Byte offset of the `@` in the text.
    pub at: usize,
    /// `@chapter:12` → Some(Chapter); `@sol` → None (kinds and manga both match).
    pub kind: Option<MentionKind>,
    pub query: String,
}

/// The `@…` token the caret is at the end of (`cursor` is a byte offset), if any. An `@` inside
/// a word (an e-mail address) is not a mention.
pub fn mention_query(text: &str, cursor: usize) -> Option<MentionQuery> {
    let cursor = cursor.min(text.len());
    if !text.is_char_boundary(cursor) {
        return None;
    }
    let before = &text[..cursor];
    let start = before.rfind(char::is_whitespace).map(|i| i + before[i..].chars().next().map_or(1, char::len_utf8)).unwrap_or(0);
    let token = &before[start..];
    let body = token.strip_prefix('@')?;
    if body.contains('@') {
        return None;
    }
    let (kind, query) = match body.split_once(':') {
        Some((k, q)) => (Some(MentionKind::parse(k)?), q.to_string()),
        None => (None, body.to_string()),
    };
    Some(MentionQuery { at: start, kind, query })
}

/// A row of the mention popup.
#[derive(Debug, Clone, PartialEq)]
pub enum Candidate {
    /// Picking it narrows the popup to that kind (`@manga:`), or opens the file picker.
    Kind(MentionKind),
    Item(Mention),
}

/// Candidates for `q`: kinds whose name matches (when no kind is chosen yet), then items of the
/// chosen kind (or manga, with no kind) ranked by how their label matches. At most `max`.
pub fn mention_candidates(q: &MentionQuery, items: &[Mention], max: usize) -> Vec<Candidate> {
    let query = q.query.trim().to_lowercase();
    let mut out: Vec<Candidate> = vec![];
    if q.kind.is_none() {
        out.extend(MentionKind::ALL.into_iter().filter(|k| query.is_empty() || k.key().starts_with(&query)).map(Candidate::Kind));
    }
    let want = q.kind.unwrap_or(MentionKind::Manga);
    if q.kind.is_none() && query.is_empty() {
        return out;
    }
    let mut scored: Vec<(u32, &Mention)> = items
        .iter()
        .filter(|m| m.kind == want)
        .filter_map(|m| {
            let l = m.label.to_lowercase();
            let score = if query.is_empty() {
                1
            } else if l == query {
                4
            } else if l.starts_with(&query) {
                3
            } else if l.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(&query)) {
                2
            } else if l.contains(&query) || m.detail.to_lowercase().contains(&query) {
                1
            } else {
                return None;
            };
            Some((score, m))
        })
        .collect();
    // Stable: equal scores keep the order given (most recent first).
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    out.extend(scored.into_iter().map(|(_, m)| Candidate::Item(m.clone())));
    out.truncate(max);
    out
}

/// The text with the `@…` token at `q.at` (up to `cursor`) removed.
pub fn remove_token(text: &str, q: &MentionQuery, cursor: usize) -> String {
    let end = cursor.min(text.len()).max(q.at);
    let mut s = String::with_capacity(text.len());
    s.push_str(text[..q.at].trim_end_matches(' '));
    let rest = &text[end..];
    if !s.is_empty() && !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        s.push(' ');
    }
    s.push_str(rest);
    s
}

/// The message the model sees: the user's text and, when items are attached, a context block
/// with their ids and paths.
pub fn with_context(text: &str, mentions: &[Mention]) -> String {
    if mentions.is_empty() {
        return text.to_string();
    }
    let lines: Vec<String> = mentions.iter().map(Mention::context_line).collect();
    format!("{text}\n\n[Attached context — use these exact ids and paths]\n{}", lines.join("\n"))
}

/// Manga mentions from the conversation's `manga_search` results, newest first (so `@` finds a
/// title the assistant just found on a source).
pub fn search_mentions(history: &[mv_ai::AiMessage]) -> Vec<Mention> {
    let mut out: Vec<Mention> = vec![];
    for m in history.iter().rev().filter(|m| m.tool_name.as_deref() == Some("manga_search")) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&m.content) else { continue };
        let rows = v.get("results").or_else(|| v.get("items")).and_then(|r| r.as_array()).cloned().or_else(|| v.as_array().cloned()).unwrap_or_default();
        for r in rows {
            let title = r.get("title").and_then(|t| t.as_str()).unwrap_or("").trim();
            let source = r.get("source").and_then(|t| t.as_str()).unwrap_or("");
            let id = r.get("id").and_then(|t| t.as_str()).unwrap_or("");
            if title.is_empty() || out.iter().any(|o| o.label == title) {
                continue;
            }
            out.push(Mention::new(MentionKind::Manga, title).id(id).detail(format!("source {source} (search result: load with manga_chapters source + id)")));
        }
    }
    out
}

// ── Export and compact ───────────────────────────────────────────────────────────

/// The chat as Markdown (`/export`): messages, tool steps and attached items.
pub fn to_markdown(title: &str, items: &[ChatItem]) -> String {
    let mut s = format!("# {}\n\n", if title.trim().is_empty() { "MangaVox chat" } else { title.trim() });
    for i in items {
        match i.role {
            ItemRole::User => {
                s.push_str("## You\n\n");
                s.push_str(i.text.trim());
                s.push('\n');
                for m in &i.mentions {
                    s.push_str(&format!("\n> @{} {}", m.kind.key(), m.label));
                }
                s.push_str("\n\n");
            }
            ItemRole::Assistant => {
                s.push_str(&format!("## Assistant{}\n\n", i.model.as_deref().map(|m| format!(" ({m})")).unwrap_or_default()));
                for st in &i.steps {
                    s.push_str(&format!("- _{}_ `{}` ({:?})\n", st.label, st.name, st.state));
                }
                if !i.steps.is_empty() {
                    s.push('\n');
                }
                s.push_str(crate::textcall::visible(&i.text).trim());
                if let Some(e) = &i.error {
                    s.push_str(&format!("\n\n> Error: {e}"));
                }
                s.push_str("\n\n");
            }
        }
    }
    s
}

/// A file name for the export ("Solo recap plan.md"), without characters Windows refuses.
pub fn export_name(title: &str) -> String {
    let clean: String = title.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { ' ' } else { c }).collect();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    let clean: String = clean.chars().take(60).collect();
    format!("{}.md", if clean.is_empty() { "MangaVox chat".into() } else { clean })
}

/// Where `/compact` cuts: the index of the first history message kept (the last `keep_turns`
/// user turns stay whole). `None` when there is too little to compact.
pub fn compact_cut(history: &[mv_ai::AiMessage], keep_turns: usize) -> Option<usize> {
    let users: Vec<usize> = history.iter().enumerate().filter(|(_, m)| m.role == mv_ai::AiRole::User).map(|(i, _)| i).collect();
    if users.len() <= keep_turns {
        return None;
    }
    let cut = users[users.len() - keep_turns.max(1)];
    (cut >= 2).then_some(cut)
}

/// The prompt that asks the model to summarise the part being compacted.
pub fn compact_prompt(old: &[mv_ai::AiMessage]) -> String {
    let mut s = String::from("Summarise this earlier part of a MangaVox assistant chat in at most 12 short bullet points. Keep every manga title, chapter number, id, file path, setting and decision, and what is still to do. No preamble.\n\n");
    for m in old {
        let who = match m.role {
            mv_ai::AiRole::User => "User",
            mv_ai::AiRole::Assistant => "Assistant",
            mv_ai::AiRole::Tool => "Tool result",
            mv_ai::AiRole::System => continue,
        };
        let mut body: String = m.content.chars().take(1200).collect();
        if let Some(calls) = &m.tool_calls {
            body.push_str(&format!(" [called {}]", calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")));
        }
        if !body.trim().is_empty() {
            s.push_str(&format!("{who}: {}\n", body.trim()));
        }
    }
    s
}

/// A summary without a model: each user request and the tools used (the fallback when the model
/// cannot summarise).
pub fn local_summary(old: &[mv_ai::AiMessage]) -> String {
    let mut lines = vec![];
    for m in old {
        match m.role {
            mv_ai::AiRole::User => lines.push(format!("- User asked: {}", m.content.lines().next().unwrap_or("").chars().take(160).collect::<String>())),
            mv_ai::AiRole::Assistant => {
                if let Some(calls) = m.tool_calls.as_ref().filter(|c| !c.is_empty()) {
                    lines.push(format!("  - used {}", calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")));
                }
            }
            _ => {}
        }
    }
    lines.join("\n")
}

/// The history after `/compact`: the summary as one exchange, then the kept messages.
pub fn compacted(history: &[mv_ai::AiMessage], cut: usize, summary: &str) -> Vec<mv_ai::AiMessage> {
    let mut out = vec![
        mv_ai::AiMessage::user(format!("[Summary of the earlier conversation]\n{}", summary.trim())),
        mv_ai::AiMessage::assistant("Noted — I'll continue from that summary."),
    ];
    out.extend(history[cut.min(history.len())..].iter().cloned());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mv_ai::{AiMessage, AiRole};

    #[test]
    fn slash_commands_parse() {
        let (c, a) = parse("/make Solo Leveling 12-14").unwrap();
        assert_eq!((c.name, a.as_str()), ("make", "Solo Leveling 12-14"));
        assert_eq!(parse("  /clear").unwrap().0.action, Action::Clear);
        assert_eq!(parse("/MODEL").unwrap().0.name, "model");
        assert!(parse("/usr/bin/env is a path").is_none(), "unknown words are messages");
        assert!(parse("hello /clear").is_none());
        assert!(parse("").is_none());
    }

    #[test]
    fn the_popup_filters_while_typing_the_name() {
        assert_eq!(slash_query("/mo"), Some("mo"));
        assert_eq!(slash_query("/"), Some(""));
        assert_eq!(slash_query("/make Solo"), None, "arguments close the popup");
        assert_eq!(slash_query("hi"), None);
        assert_eq!(filter_commands("").len(), COMMANDS.len());
        let names = |q| filter_commands(q).iter().map(|c| c.name).collect::<Vec<_>>();
        assert_eq!(names("cl"), ["clear"]);
        assert_eq!(names("m")[..2], ["model", "make"], "prefix matches first, in table order");
        // Descriptions match too, after the names.
        assert!(names("markdown").contains(&"export"));
        assert!(names("zzz").is_empty());
    }

    #[test]
    fn prompt_commands_expand() {
        let make = find_command("make").unwrap();
        let Action::Prompt(t) = make.action else { panic!() };
        assert!(expand(t, "Solo Leveling 12").contains("recap videos for Solo Leveling 12"));
        assert!(expand(t, "").contains("ask me"));
        let Action::Prompt(g) = find_command("guide").unwrap().action else { panic!() };
        assert_eq!(expand(g, ""), "Explain how to use MangaVox (use app_guide), briefly.");
        assert!(help_markdown().contains("/compact") && help_markdown().contains("@chapter"));
    }

    #[test]
    fn mention_tokens_are_found_at_the_caret() {
        let q = mention_query("look at @sol", 12).unwrap();
        assert_eq!((q.at, q.kind, q.query.as_str()), (8, None, "sol"));
        let q = mention_query("@chapter:12", 11).unwrap();
        assert_eq!((q.kind, q.query.as_str()), (Some(MentionKind::Chapter), "12"));
        assert!(mention_query("mail me@home.com", 16).is_none(), "an @ inside a word");
        assert!(mention_query("@bogus:x", 8).is_none());
        assert!(mention_query("@sol done", 9).is_none(), "the caret left the token");
        assert_eq!(mention_query("@", 1).unwrap().query, "");
        // Multi-byte text before the token.
        let t = "ça @man";
        assert_eq!(mention_query(t, t.len()).unwrap().query, "man");
    }

    #[test]
    fn mention_candidates_rank_and_resolve() {
        let items = vec![
            Mention::new(MentionKind::Manga, "Solo Leveling").id("m1"),
            Mention::new(MentionKind::Manga, "The Solo Max-Level Newbie").id("m2"),
            Mention::new(MentionKind::Manga, "Absolute Solo").id("m3"),
            Mention::new(MentionKind::Chapter, "Solo Leveling · Ch. 12").id("c12"),
        ];
        // Just "@": the kinds.
        let c = mention_candidates(&MentionQuery { at: 0, kind: None, query: String::new() }, &items, 10);
        assert_eq!(c.len(), 5);
        assert!(matches!(c[0], Candidate::Kind(MentionKind::Manga)));
        // "@sol": manga by match quality (prefix, then word, then inside).
        let c = mention_candidates(&MentionQuery { at: 0, kind: None, query: "sol".into() }, &items, 10);
        let ids: Vec<&str> = c.iter().filter_map(|c| if let Candidate::Item(m) = c { Some(m.id.as_str()) } else { None }).collect();
        assert_eq!(ids, ["m1", "m2", "m3"]);
        // "@ch": the chapter kind first.
        let c = mention_candidates(&MentionQuery { at: 0, kind: None, query: "ch".into() }, &items, 10);
        assert!(matches!(c[0], Candidate::Kind(MentionKind::Chapter)));
        // "@chapter:12": chapters only.
        let c = mention_candidates(&MentionQuery { at: 0, kind: Some(MentionKind::Chapter), query: "12".into() }, &items, 10);
        assert_eq!(c, vec![Candidate::Item(items[3].clone())]);
    }

    #[test]
    fn attached_items_become_context() {
        let m = vec![Mention::new(MentionKind::Manga, "Solo Leveling").id("m1"), Mention::new(MentionKind::File, "a.png").path("C:\\x\\a.png")];
        let s = with_context("make an intro", &m);
        assert!(s.starts_with("make an intro\n\n[Attached context"));
        assert!(s.contains("- manga: \"Solo Leveling\", id m1"));
        assert!(s.contains("path C:\\x\\a.png"));
        assert_eq!(with_context("hi", &[]), "hi");
        let q = mention_query("open @sol now", 9).unwrap();
        assert_eq!(remove_token("open @sol now", &q, 9), "open now");
    }

    #[test]
    fn search_results_become_mentions() {
        let mut t = AiMessage::new(AiRole::Tool, r#"{"results":[{"title":"Frieren","source":"mangadex","id":"abc"}]}"#);
        t.tool_name = Some("manga_search".into());
        let m = search_mentions(&[t]);
        assert_eq!((m[0].label.as_str(), m[0].id.as_str()), ("Frieren", "abc"));
        assert!(m[0].detail.contains("mangadex"));
    }

    #[test]
    fn export_and_compact() {
        let mut u = ChatItem::user("make an intro");
        u.mentions.push(Mention::new(MentionKind::Manga, "Solo"));
        let mut a = ChatItem::assistant("m");
        a.text = "Done.\n```mangavox\n{\"tool\":\"x\"}\n```".into();
        let md = to_markdown("Intro", &[u, a]);
        assert!(md.starts_with("# Intro") && md.contains("## You") && md.contains("> @manga Solo") && md.contains("Done."));
        assert!(!md.contains("mangavox"), "call blocks are not exported");
        assert_eq!(export_name("a/b: c?"), "a b c.md");
        let h: Vec<AiMessage> = (0..5).flat_map(|i| [AiMessage::user(format!("q{i}")), AiMessage::assistant(format!("a{i}"))]).collect();
        let cut = compact_cut(&h, 2).unwrap();
        assert_eq!(h[cut].content, "q3");
        assert!(compact_cut(&h[..4], 2).is_none());
        let c = compacted(&h, cut, "- did things");
        assert_eq!(c.len(), 2 + 4);
        assert!(c[0].content.contains("did things") && c[2].content == "q3");
        assert!(compact_prompt(&h[..cut]).contains("User: q0"));
        assert!(local_summary(&h[..cut]).contains("User asked: q1"));
    }
}
