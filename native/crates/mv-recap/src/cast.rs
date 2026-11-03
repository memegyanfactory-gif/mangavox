//! CAST check (first step of STORY): one text call per chapter works out who is who. Pages are read
//! one at a time and several at once, so the reader often calls one person by several looks
//! ("Fur-collared man", "Messy-haired man in long coat") and gives one look to different people (two
//! monsters both "Horned skull-faced creature"). Left like that the writer tells one hero as three
//! strangers and skips the scenes it cannot place. The check reads the whole chapter's text with the
//! names heard in the dialogue and the story so far, renames the labels in `read.json` (per page
//! when a label means different people), folds a looks-only placeholder into the named character
//! in the brain, and keeps a who-is-who list for the writer.

use anyhow::Result;
use mv_ai::{AiMessage, CancellationToken};
use serde_json::Value;

use crate::brain::Brain;
use crate::json::str_of;
use crate::llm::{ask_json, request, truncate, Llm};
use crate::types::*;

const CAST_SYSTEM: &str = r##"You are the continuity editor of a manga recap channel. A vision model read a chapter page by page, each page on its own, so it often calls ONE person by several looks-based labels ("Fur-collared man", "Messy-haired man in long coat", "Dark-haired boy in white coat") and sometimes gives ONE label to DIFFERENT people (two different monsters both called "Horned skull-faced creature"). Speakers can be mislabelled the same way.

Work out who every labelled person in the reading really is, from:
- names spoken or written anywhere in the chapter ("Ren!", "I'm Gorvak"), including on later pages,
- the KNOWN CHARACTERS (their names, aliases and looks) and the STORY SO FAR,
- continuity: who was on the page before, who talks to whom, who is in the scene.

Rules
- "label" is a name exactly as it appears in the reading. "name" is who that is: a KNOWN CHARACTER's exact name, else a name the chapter gives them, else (truly unnamed) a short role such as "Ren's coworker" or "the convenience store clerk", never their looks.
- Use a KNOWN CHARACTER's name only when this chapter (its dialogue, the names on its pages) or the story so far shows that character is here. Looks alone are never enough: many monsters and extras look alike, and the known characters include people from later chapters. Otherwise give a role.
- A character's transformed, monster, armoured or child form is still that character: use their name.
- "pages": [] when the label means that person on every page of the chapter, else the list of page numbers where it does (use this when one label covers different people). A label for a monster or a transformed form often covers different creatures on different pages (the villain on some, the hero's own monster form on others): check each page's dialogue and context before you give "pages": [].
- Leave out labels that are already right. Never fold two different people into one. When you are not sure, leave it out.
- "speakers": the lines (by their [L<n>] number) whose speaker is wrong or missing, with who really says them: follow the conversation (who answers whom, who is addressed by name, who is in the panel, what that character would say). Thoughts belong to the character thinking them. Leave out lines that are right.
- "cast": every person who matters in the chapter, by their resolved name, with ONE line on who they are and how they relate to the others (for the narrator).

Reply with STRICT JSON only, one object, no code fences:
{"fixes": [{"label": "Man in long coat", "name": "Ren", "pages": []}, {"label": "Horned skull-faced creature", "name": "Gorvak", "pages": [9, 10]}],
 "speakers": [{"line": 12, "speaker": "Ren"}],
 "cast": [{"name": "Ren", "who": "the hero, a monster living as a human with part-time jobs"}]}"##;

/// One rename the check asked for: `label` is `name` on `pages` (1-based; empty = everywhere).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CastFix {
    pub label: String,
    pub name: String,
    pub pages: Vec<usize>,
}

/// A label that describes looks ("Bald elder with glasses"), not a name: it has a word in lower case.
pub(crate) fn looks_only(name: &str) -> bool {
    name.split_whitespace().any(|w| w.chars().next().is_some_and(char::is_lowercase))
}

/// The people labels on the pages: characters seen and speakers.
fn labels(read: &ChapterRead) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for pn in read.pages.iter().flat_map(|p| &p.panels) {
        for n in pn.characters.iter().map(|c| c.name.as_str()).chain(pn.lines.iter().filter_map(|l| l.speaker.as_deref())) {
            if !n.trim().is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(n)) {
                out.push(n.to_string());
            }
        }
    }
    out
}

/// The spoken lines of the reading (sound effects left out), in order: the check's line `[L<n>]` is item `n - 1`.
fn spoken(read: &mut ChapterRead) -> Vec<&mut Line> {
    read.pages.iter_mut().flat_map(|p| p.panels.iter_mut()).flat_map(|pn| pn.lines.iter_mut()).filter(|l| l.kind != "sfx").collect()
}

/// Give the lines the check named their right speaker. Narration boxes keep none.
pub(crate) fn apply_speakers(read: &mut ChapterRead, v: &Value) {
    let fixes: Vec<(usize, String)> = v.get("speakers").and_then(Value::as_array).into_iter().flatten().filter_map(|f| Some((f.get("line")?.as_u64()? as usize, str_of(f, "speaker")))).filter(|(_, s)| !s.is_empty()).collect();
    let mut lines = spoken(read);
    for (n, who) in fixes {
        if let Some(l) = n.checked_sub(1).and_then(|i| lines.get_mut(i)).filter(|l| l.kind != "narration") {
            l.speaker = Some(who);
        }
    }
}

/// Compact reading for the check: per page its summary, who is in each panel and every spoken line, numbered.
fn cast_digest(read: &ChapterRead) -> String {
    let mut s = String::new();
    let mut line = 0;
    for p in &read.pages {
        if p.panels.is_empty() && p.summary.is_empty() {
            continue;
        }
        s += &format!("PAGE {}\n", p.index + 1);
        if !p.summary.is_empty() {
            s += &format!("Summary: {}\n", truncate(&p.summary, 400));
        }
        for (n, pn) in p.panels.iter().enumerate() {
            let who: Vec<&str> = pn.characters.iter().map(|c| c.name.as_str()).collect();
            s += &format!("Panel {}: {}{}\n", n + 1, truncate(&pn.description, 160), if who.is_empty() { String::new() } else { format!(" (in it: {})", who.join(", ")) });
            for l in pn.lines.iter().filter(|l| l.kind != "sfx") {
                line += 1;
                s += &format!("  [L{line}] {}\n", crate::transcript::format_line(l));
            }
        }
        s.push('\n');
    }
    s
}

/// Everything the chapter's pages say or name: lines and summaries.
fn chapter_text(read: &ChapterRead) -> String {
    let mut s = String::new();
    for p in &read.pages {
        s += &p.summary;
        s.push('\n');
        for l in p.panels.iter().flat_map(|pn| &pn.lines) {
            s += &l.text;
            s.push('\n');
        }
    }
    s
}

/// `name` is a role, or one of its words (3+ letters) appears in `said` (lower case).
pub(crate) fn mentioned(said: &str, name: &str) -> bool {
    looks_only(name)
        || name
            .split(|c: char| c.is_whitespace() || c == '-')
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
            .filter(|w| w.chars().count() >= 3)
            .any(|w| said.contains(&w))
}

fn known_text(brain: &Brain) -> String {
    let mut cs: Vec<_> = brain.characters.iter().filter(|c| !looks_only(&c.name)).collect();
    cs.sort_by_key(|c| std::cmp::Reverse(c.appearances));
    let s: Vec<String> = cs
        .iter()
        .take(40)
        .map(|c| {
            let mut line = format!("- {}", c.name);
            if !c.aliases.is_empty() {
                line += &format!(" (also labelled: {})", c.aliases.iter().take(8).cloned().collect::<Vec<_>>().join(", "));
            }
            if !c.description.is_empty() {
                line += &format!(": {}", truncate(&c.description, 160));
            }
            line
        })
        .collect();
    if s.is_empty() {
        "(none yet)".into()
    } else {
        s.join("\n")
    }
}

pub(crate) fn parse_fixes(v: &Value) -> (Vec<CastFix>, Vec<(String, String)>) {
    let fixes = v
        .get("fixes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|f| {
            let (label, name) = (str_of(f, "label"), str_of(f, "name"));
            let pages = f.get("pages").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(|n| n as usize).collect()).unwrap_or_default();
            (!label.is_empty() && !name.is_empty() && !label.eq_ignore_ascii_case(&name)).then_some(CastFix { label, name, pages })
        })
        .collect();
    let cast = v.get("cast").and_then(Value::as_array).into_iter().flatten().map(|c| (str_of(c, "name"), str_of(c, "who"))).filter(|(n, _)| !n.is_empty()).collect();
    (fixes, cast)
}

/// `text` with every whole-word `label` (any case) replaced by `name`; "the <label>" loses its
/// article when `name` needs none ("Hachiro", "Hachiro's coworker", "the clerk").
pub(crate) fn replace_label(text: &str, label: &str, name: &str) -> String {
    let (lt, ll) = (text.to_ascii_lowercase(), label.to_ascii_lowercase());
    if ll.is_empty() || !lt.contains(&ll) {
        return text.to_string();
    }
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '-');
    let nl = name.to_lowercase();
    let proper = name.chars().next().is_some_and(char::is_uppercase) || ["the ", "his ", "her ", "their ", "a ", "an "].iter().any(|d| nl.starts_with(d));
    let mut out = String::new();
    let mut at = 0;
    while let Some(off) = lt[at..].find(&ll) {
        let (s, e) = (at + off, at + off + ll.len());
        if word(text[..s].chars().next_back()) || word(text[e..].chars().next()) {
            out += &text[at..e];
            at = e;
            continue;
        }
        let mut head = &text[at..s];
        if proper {
            for art in ["the ", "The "] {
                if let Some(h) = head.strip_suffix(art) {
                    if !word(h.chars().next_back()) {
                        head = h;
                        break;
                    }
                }
            }
        }
        out += head;
        out += name;
        at = e;
    }
    out += &text[at..];
    out
}

/// Apply `fixes` to the chapter's reading (names, speakers, summaries and panel descriptions).
/// Returns how many labels were renamed on the pages.
pub(crate) fn apply_fixes(read: &mut ChapterRead, fixes: &[CastFix]) -> usize {
    let mut n = 0;
    for p in &mut read.pages {
        let here: Vec<&CastFix> = fixes.iter().filter(|f| f.pages.is_empty() || f.pages.contains(&(p.index + 1))).collect();
        if here.is_empty() {
            continue;
        }
        let fix = |name: &str| here.iter().find(|f| f.label.eq_ignore_ascii_case(name.trim())).map(|f| f.name.clone());
        for f in &here {
            p.summary = replace_label(&p.summary, &f.label, &f.name);
        }
        for pn in &mut p.panels {
            for f in &here {
                pn.description = replace_label(&pn.description, &f.label, &f.name);
            }
            for c in &mut pn.characters {
                if let Some(name) = fix(&c.name) {
                    c.name = name;
                    n += 1;
                }
            }
            // Two labels of one person in a panel are one person.
            let mut seen: Vec<String> = vec![];
            pn.characters.retain(|c| {
                let new = !seen.iter().any(|s| s.eq_ignore_ascii_case(&c.name));
                seen.push(c.name.clone());
                new
            });
            for l in &mut pn.lines {
                if let Some(name) = l.speaker.as_deref().and_then(fix) {
                    l.speaker = Some(name);
                }
            }
        }
    }
    n
}

/// Fold the chapter-wide fixes of a looks-only placeholder into the named character in the brain,
/// so later pages and chapters are read with the name. A label the check split by page, or a
/// character the user edited, is left alone. This chapter's story notes get the names too.
pub(crate) fn apply_to_brain(brain: &mut Brain, fixes: &[CastFix], label: &str) {
    for f in fixes.iter().filter(|f| f.pages.is_empty() && looks_only(&f.label) && !looks_only(&f.name)) {
        if fixes.iter().any(|o| o.label.eq_ignore_ascii_case(&f.label) && !o.name.eq_ignore_ascii_case(&f.name)) {
            continue;
        }
        let Some(from) = brain.find(&f.label).filter(|c| !c.user_locked && c.name.eq_ignore_ascii_case(&f.label)).map(|c| c.id.clone()) else { continue };
        match brain.find(&f.name).map(|c| c.id.clone()) {
            Some(to) => brain.merge(&to, &from),
            None => {
                brain.observe(&f.name, Some(&f.label), "", &[]);
            }
        }
    }
    let prefix = format!("- [{label} p");
    let notes: Vec<String> = brain
        .notes
        .lines()
        .map(|l| if l.starts_with(&prefix) { fixes.iter().filter(|f| f.pages.is_empty()).fold(l.to_string(), |t, f| replace_label(&t, &f.label, &f.name)) } else { l.to_string() })
        .collect();
    brain.notes = notes.join("\n");
}

/// Run the check on `read` (see the module docs) unless it already ran on these pages, applying
/// the fixes to it. Returns `None` when it had already run, else the fixes for [`apply_to_brain`]
/// (the reading changed: the caller saves it). An unparseable answer leaves the names as they were:
/// the story is still written, only without the fixes.
pub(crate) async fn check_cast(llm: &dyn Llm, model: &ModelChoice, ch: &ChapterRef, read: &mut ChapterRead, brain: &Brain, so_far: &str, cancel: &CancellationToken) -> Result<Option<Vec<CastFix>>> {
    if read.cast_checked {
        return Ok(None);
    }
    let all = labels(read);
    if all.is_empty() {
        read.cast_checked = true;
        return Ok(Some(vec![]));
    }
    let prompt = format!(
        "MANGA: {}\nCHAPTER: {}\n\nKNOWN CHARACTERS:\n{}\n\nSTORY SO FAR:\n{}\n\nLABELS USED FOR PEOPLE IN THIS CHAPTER:\n{}\n\nREADING OF THE CHAPTER:\n\n{}Reply with the JSON object only.",
        brain.manga_title,
        ch.label,
        known_text(brain),
        if so_far.is_empty() { "(this is the very beginning)" } else { so_far },
        all.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n"),
        cast_digest(read),
    );
    let req = request(model, vec![AiMessage::system(CAST_SYSTEM), AiMessage::user(prompt)], vec![], Some(crate::store::chapter_dir(ch).join("work")));
    let answer = ask_json(llm, req, cancel).await?;
    read.cast_checked = true;
    let Ok(v) = answer else { return Ok(Some(vec![])) };
    let (fixes, cast) = parse_fixes(&v);
    // A name the chapter never mentions is a guess from looks: dropped (roles are kept).
    let said = format!("{}\n{so_far}\n{}", chapter_text(read), all.join("\n")).to_lowercase();
    let fixes: Vec<CastFix> = fixes.into_iter().filter(|f| mentioned(&said, &f.name)).collect();
    let cast = cast.into_iter().filter(|(n, _)| mentioned(&said, n)).collect();
    apply_fixes(read, &fixes);
    apply_speakers(read, &v);
    read.cast = cast;
    Ok(Some(fixes))
}
