//! HOOK: the cold open of a full recap video, the 45-75 seconds before chapter 1 that make a
//! stranger stay, the way the top recap channels open. Either a FLASH-FORWARD (the most
//! jaw-dropping showcase of the hero from late in the story, told as a fast mini-scene: who and
//! where, the feat, the witnesses' shock, "but that's only the first stage", a bigger feat, the
//! twist) or the PREMISE (the story's own opening told as a rising montage: who the hero was, the
//! betrayal or the fall, the twist). It ends on a turn into the story, which starts right after
//! it: the question the story answers ("So how did a jobless loser get this strong?") or the line
//! that drops us into its first scene. Each line plays over one to three pictures of the scene it
//! tells, cut fast.
//!
//! The narration beats of every made chapter are narrowed to whole scenes ([`pick_candidates`]):
//! the story's opening and the strongest showcases, later ones weighing more. The model picks the
//! opening and writes it over their pictures ([`write_hook`]). Without a model the strongest late
//! scene's own words are used ([`fallback_hook`]).

use std::sync::Arc;

use anyhow::{bail, Result};
use mv_ai::{AiMessage, CancellationToken, Registry};
use serde_json::Value;

use crate::json::{num_of, str_of};
use crate::llm::{ask_json, request, truncate, Llm};
use crate::types::ModelChoice;

/// Most candidates offered to the model: about fifteen scenes of [`SCENE_BEATS`].
pub const MAX_CANDIDATES: usize = 240;
/// Beats in a row a scene is offered with (a scene of a chapter video runs about a minute).
const SCENE_BEATS: usize = 16;
/// Spoken words per second of hook (the narration voice's pace).
const WORDS_PER_SECOND: f64 = 2.6;
/// Lines a hook has, at most and at least (the outro is one more).
const MAX_LINES: usize = 16;
const MIN_LINES: usize = 2;
/// Pictures one line plays over, at most.
const MAX_SHOTS: usize = 3;
/// Most words in one line of the fallback.
const LINE_WORDS: usize = 16;
const FALLBACK_OUTRO: &str = "So how did it all come to this? Let's start from the beginning.";

/// A narration beat the hook may show and retell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookCandidate {
    /// Index of the beat among all beats of all chapters, in story order.
    pub id: usize,
    /// The chapter's label ("Chapter 3").
    pub chapter: String,
    /// What the narrator says over that beat.
    pub text: String,
}

/// One line of the hook: what is said, over the pictures of these beats in turn.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HookLine {
    pub ids: Vec<usize>,
    pub text: String,
}

impl HookLine {
    pub fn new(ids: Vec<usize>, text: impl Into<String>) -> Self {
        HookLine { ids, text: text.into() }
    }
}

/// The hook's script.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HookScript {
    /// How it opens and on what ("flash-forward: ..." / "premise: ...", empty for the fallback).
    pub idea: String,
    /// The lines, in the order they play.
    pub lines: Vec<HookLine>,
    /// The closing line, the turn into the story.
    pub outro: String,
    /// The beat whose picture is under the closing line (`None`: the manga's cover).
    pub outro_id: Option<usize>,
}

impl HookScript {
    pub fn words(&self) -> usize {
        self.lines.iter().map(|l| l.text.split_whitespace().count()).sum::<usize>() + self.outro.split_whitespace().count()
    }

    /// Drops lines from the middle (the escalation; the opening and the twist stay) while the
    /// hook runs well past [`max_words`] of `seconds`, keeping [`MIN_LINES`].
    pub fn fit(&mut self, seconds: u32) {
        let limit = max_words(seconds) + max_words(seconds) / 8;
        while self.words() > limit && self.lines.len() > MIN_LINES {
            let mid = self.lines.len() / 2;
            self.lines.remove(mid);
        }
    }
}

/// Words `seconds` of narration hold at the narrator's pace.
pub fn target_words(seconds: u32) -> usize {
    (seconds as f64 * WORDS_PER_SECOND).round() as usize
}

/// Most words a hook of `seconds` has, outro included.
pub fn max_words(seconds: u32) -> usize {
    target_words(seconds)
}

const ACTION_WORDS: &[&str] = &[
    "fight", "battle", "war", "kill", "killed", "kills", "die", "dies", "dead", "death", "blood", "bleed", "sword", "blade", "strike", "strikes", "attack",
    "attacks", "explode", "explodes", "explosion", "power", "powers", "powerful", "strongest", "rage", "fury", "scream", "screams", "roar", "roars",
    "monster", "demon", "beast", "destroy", "destroys", "crush", "crushes", "betray", "betrays", "betrayal", "betrayed", "traitor", "secret", "reveal",
    "reveals", "revealed", "truth", "curse", "cursed", "awaken", "awakens", "unleash", "unleashes", "revenge", "vengeance", "enemy", "enemies", "trap",
    "danger", "deadly", "doom", "fate", "sacrifice", "shatter", "shatters", "storm", "fire", "flames", "god", "gods", "king", "army", "duel", "clash",
    "ambush", "hunt", "hunter", "chase", "escape", "fear", "terror", "horror", "impossible", "never", "only", "last", "final", "suddenly", "shock",
];

/// Words of a character's wants, loves and quirks: what makes a hero worth watching.
const CHARM_WORDS: &[&str] = &[
    "love", "loves", "loved", "favorite", "favourite", "dream", "dreams", "wants", "wish", "wishes", "secretly", "just", "normal", "peace", "happy",
    "happily", "friend", "friends", "eat", "eats", "ate", "taste", "tasted", "food", "sweet", "sweets", "cake", "ice", "cream", "candy", "obsessed",
    "hates", "afraid", "scared", "cute", "shy", "awkward", "weird", "always", "everything", "instead", "actually", "pretends",
];

/// Words of people watching a feat they cannot believe: what makes a showcase.
const WITNESS_WORDS: &[&str] = &[
    "everyone", "everybody", "crowd", "audience", "spectators", "watching", "watches", "watch", "stunned", "shocked", "speechless", "disbelief",
    "believe", "jaw", "jaws", "gasp", "gasps", "silence", "silent", "terrified", "impressed", "awe", "level", "rank", "exam", "test", "legend",
    "legendary", "genius", "nobody", "record", "examiner", "judges", "applause", "cheer", "cheers",
];

fn words_of(text: &str) -> Vec<String> {
    text.split_whitespace().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()).filter(|w| !w.is_empty()).collect()
}

/// How strongly a beat's text would grab a viewer: action words, words of what a character loves
/// or wants, '!', and enough words to say something (very short beats are weak).
pub fn beat_score(text: &str) -> f64 {
    let words = words_of(text);
    if words.is_empty() {
        return 0.;
    }
    let hits = words.iter().filter(|w| ACTION_WORDS.contains(&w.as_str()) || CHARM_WORDS.contains(&w.as_str())).count().min(4) as f64;
    let bangs = text.matches('!').count().min(2) as f64;
    let length = (words.len().min(40) as f64) / 40.;
    let short = if words.len() < 5 { 0.5 } else { 1. };
    (1. + 0.6 * hits + 0.7 * bangs + length) * short
}

/// [`beat_score`] plus the people watching it: a feat in front of a stunned crowd is the moment a
/// cold open is made of.
pub fn showcase_score(text: &str) -> f64 {
    let witnesses = words_of(text).iter().filter(|w| WITNESS_WORDS.contains(&w.as_str())).count().min(4) as f64;
    beat_score(text) + 0.5 * witnesses
}

/// Up to `max` candidates from `chapters` (label, beat texts, in story order; an empty text is a
/// beat without a picture), as whole scenes: runs of [`SCENE_BEATS`] beats in a row. The story's
/// opening scene (the premise) always, then the scenes with the strongest showcases by
/// [`showcase_score`], later chapters weighing up to twice as much (a flash-forward comes from
/// late in the story). Ids are the beats' indices among all beats of all chapters. In story order.
pub fn pick_candidates(chapters: &[(String, Vec<String>)], max: usize) -> Vec<HookCandidate> {
    let mut flat: Vec<(usize, &str)> = vec![];
    for (c, (_, texts)) in chapters.iter().enumerate() {
        flat.extend(texts.iter().map(|t| (c, t.trim())));
    }
    let candidate = |id: usize| HookCandidate { id, chapter: chapters[flat[id].0].0.clone(), text: flat[id].1.to_string() };
    let usable: Vec<usize> = (0..flat.len()).filter(|&id| !flat[id].1.is_empty()).collect();
    if usable.len() <= max {
        return usable.into_iter().map(candidate).collect();
    }
    let n = chapters.len();
    let size = SCENE_BEATS.min(max).max(1);
    // Every scene window of every chapter: (its beats, its score).
    let mut scenes: Vec<(Vec<usize>, f64)> = vec![];
    for c in 0..n {
        let beats: Vec<usize> = usable.iter().copied().filter(|&id| flat[id].0 == c).collect();
        if beats.is_empty() {
            continue;
        }
        let w = size.min(beats.len());
        let mut starts: Vec<usize> = (0..=beats.len() - w).step_by((w / 2).max(1)).collect();
        if starts.last() != Some(&(beats.len() - w)) {
            starts.push(beats.len() - w);
        }
        let weight = if n > 1 { 1. + c as f64 / (n - 1) as f64 } else { 1. };
        for s in starts {
            let ids = beats[s..s + w].to_vec();
            let score = ids.iter().map(|&id| showcase_score(flat[id].1)).sum::<f64>() * weight;
            scenes.push((ids, score));
        }
    }
    let mut chosen: Vec<&Vec<usize>> = vec![];
    let mut count = 0;
    // The opening scene first (the premise), then the strongest that do not overlap a chosen one.
    let mut order: Vec<&(Vec<usize>, f64)> = scenes.iter().skip(1).collect();
    order.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0[0].cmp(&b.0[0])));
    for (ids, _) in scenes.first().into_iter().chain(order) {
        if count + ids.len() > max {
            continue;
        }
        if chosen.iter().any(|c| ids.iter().any(|id| c.contains(id))) {
            continue;
        }
        count += ids.len();
        chosen.push(ids);
    }
    let mut ids: Vec<usize> = chosen.into_iter().flatten().copied().collect();
    ids.sort_unstable();
    ids.into_iter().map(candidate).collect()
}

/// `candidates` (in story order) cut into scenes: runs of beats in a row of one chapter.
pub fn scenes_of(candidates: &[HookCandidate]) -> Vec<&[HookCandidate]> {
    let mut out = vec![];
    let mut start = 0;
    for i in 1..=candidates.len() {
        if i == candidates.len() || candidates[i].id != candidates[i - 1].id + 1 || candidates[i].chapter != candidates[i - 1].chapter {
            if start < i {
                out.push(&candidates[start..i]);
            }
            start = i;
        }
    }
    out
}

/// Cuts `text` to about `words` words, ending on the sentence or clause that fits.
fn trim_words(text: &str, words: usize) -> String {
    let all: Vec<&str> = text.split_whitespace().collect();
    if all.len() <= words {
        return all.join(" ");
    }
    let cut = &all[..words.max(3)];
    // Prefer to end at the last sentence or clause end inside the cut.
    let end = cut.iter().rposition(|w| w.ends_with(['.', '!', '?', ',', ';', ':'])).filter(|&i| i + 1 >= words.max(3) / 2);
    let mut s = match end {
        Some(i) => cut[..=i].join(" "),
        None => cut.join(" "),
    };
    s = s.trim_end_matches([',', ';', ':']).to_string();
    if !s.ends_with(['.', '!', '?']) {
        s.push_str("...");
    }
    s
}

/// The hook without a model: a flash-forward over the strongest scene of the later half of the
/// story (else the strongest of all), its strongest beats in story order, each cut to a short line
/// over its own picture and the next one's, fitting `seconds`; the generic turn over the story's
/// first picture.
pub fn fallback_hook(candidates: &[HookCandidate], seconds: u32) -> HookScript {
    let outro = FALLBACK_OUTRO.to_string();
    let budget = max_words(seconds).saturating_sub(outro.split_whitespace().count()).max(6);
    let lines_wanted = (budget / 12).clamp(MIN_LINES, line_range(seconds).1);
    let per_line = (budget / lines_wanted).clamp(5, LINE_WORDS);
    let scenes = scenes_of(candidates);
    let score = |s: &&[HookCandidate]| s.iter().map(|c| showcase_score(&c.text)).sum::<f64>();
    let half = candidates.get(candidates.len() / 2).map(|c| c.id).unwrap_or(0);
    let best = scenes.iter().filter(|s| s[0].id >= half).max_by(|a, b| score(a).total_cmp(&score(b))).or_else(|| scenes.iter().max_by(|a, b| score(a).total_cmp(&score(b))));
    let Some(scene) = best else { return HookScript { outro, ..Default::default() } };
    let mut ranked: Vec<usize> = (0..scene.len()).filter(|&i| !scene[i].text.trim().is_empty()).collect();
    ranked.sort_by(|&a, &b| showcase_score(&scene[b].text).total_cmp(&showcase_score(&scene[a].text)).then(a.cmp(&b)));
    let mut picked: Vec<usize> = ranked.into_iter().take(lines_wanted).collect();
    picked.sort_unstable();
    let mut left = budget;
    let mut lines = vec![];
    for (k, &i) in picked.iter().enumerate() {
        if left < 3 {
            break;
        }
        let t = trim_words(&scene[i].text, per_line.min(left));
        left = left.saturating_sub(t.split_whitespace().count());
        // The beat's own picture, then the next one of the scene while it is not a later line's.
        let next = picked.get(k + 1).copied().unwrap_or(scene.len());
        let ids = std::iter::once(scene[i].id).chain((i + 1 < next).then(|| scene[i + 1].id)).collect();
        lines.push(HookLine { ids, text: t });
    }
    HookScript { idea: String::new(), lines, outro, outro_id: candidates.first().map(|c| c.id) }
}

/// Lines a hook of `seconds` asks the model for, at least and at most (the outro comes on top):
/// about one every four seconds.
fn line_range(seconds: u32) -> (usize, usize) {
    (MIN_LINES, (seconds as usize / 4).clamp(3, MAX_LINES))
}

fn hook_system(seconds: u32) -> String {
    let words = max_words(seconds);
    let (lo, hi) = line_range(seconds);
    let lo = lo.max(hi / 2);
    format!(
        r##"You write the COLD OPEN of a YouTube manga recap: the first {seconds} seconds before the story starts, the part that decides whether a stranger stays for the whole video. The top recap channels open in one of two ways; pick the one that grabs harder for THIS story.

A. FLASH-FORWARD: retell the most jaw-dropping showcase of the hero from LATE in the story (a power reveal in front of a crowd, an exam he destroys, a duel everyone expected him to lose) as a fast mini-scene:
  1. "This guy is <Name>, and..." who he is and where we are, in one line: "This guy is Choi Junho, and the moment he shows up for the superhuman exam, he's already bored."
  2. Who is watching and why it matters: "The top hunters in the country have gathered to see what this newbie can do."
  3. The feat, plainly and fast: "He barely touches the measuring device, and the whole thing explodes."
  4. The witnesses' shock: "The strongest superhumans can't believe their eyes."
  5. The escalation: "But that's only the first stage." A bigger test, a bigger feat, a beat of suspense: "Surely that guy is dead by now..."
  6. The twist or punchline: "...but when the dust settles, it turns out Junho only punched the ground next to him."
B. PREMISE: tell the story's own opening as a rising montage, when it is the stronger hook (a legend betrayed, a villain given a second life, a hero who dies and wakes up in the past):
  1. "This guy is <Name>, <the legend he is>." Then a sharp turn: "But what did he get in return? Betrayal."
  2. The world in one or two lines, only what the viewer needs.
  3. The premise's climax, beat by beat, rising: "Rows of soldiers and mages close in on just one man. He cuts through wave after wave of them."
  4. The twist: "He is about to kill them all when suddenly, time itself stops."

THE TURN (the "outro"): the last line hands over to the story, which starts right after it from the very beginning. After a flash-forward it is the question the story answers: "So why is a jobless twenty-five-year-old this ridiculously strong? Let's start from the beginning." After a premise it is the line that drops us into the first scene: "The next thing he knows, he wakes up ten years in the past."

HOW IT SOUNDS
- A narrator telling a friend about the coolest thing they have seen: present tense, short spoken sentences, quick and confident, "our guy" or "our boy" now and then, one dry joke at most.
- Concrete events in the order they happen. No vague hype ("an epic journey", "you won't believe"), no list of unrelated moments, no summary of the whole story.
- The hero's name in the first line; everyone else by role ("the examiner", "the director") unless their name truly matters. No terms the viewer has not met without a few words on what they are.
- Plain spoken text: no markdown, emojis, brackets, stage directions or chapter numbers. Never say "in this video", never ask to like or subscribe, never describe the artwork (no colours, light, clothes, "the panel", "the screen").
- Never give away how the story ends or the answer to its big mystery.

PICTURES
The candidates come as SCENES: runs of the recap's narration beats in story order, each beat with an id and the line said over its picture. Each line of the cold open plays over the pictures of the ids it lists (1-3 ids, shown in turn while the line is spoken, about a second and a half each), so pick beats whose pictures show what the line says, in the order it happens. A flash-forward takes its pictures from ONE late scene (two neighbouring ones at most); a premise from the opening scene. Use only candidate ids, each at most once. The outro has one picture, "outro_id": the hero at his most striking in the chosen scene, or the moment the story begins.

LENGTH
{lo}-{hi} lines plus the outro, at most {words} words in all (about {seconds} seconds). Each line is one or two short sentences.

OUTPUT: STRICT JSON only, one object, no code fences, no commentary:
{{"idea": "flash-forward: Junho destroys the superhuman exam",
  "lines": [{{"ids": [812, 813], "text": "This guy is Choi Junho, and the moment he shows up for the superhuman exam, he's already bored."}},
            {{"ids": [815], "text": "The top hunters in the country have gathered to see what this newbie can do."}}],
  "outro": "So why is a jobless twenty-five-year-old this ridiculously strong? Let's start from the beginning.",
  "outro_id": 840}}"##
    )
}

/// What the hook writer is told about the story: the story so far at the end of chapter
/// `last_label` (else the latest one known), the main character (the most seen one: who they
/// are, their traits) and the most seen characters' names.
pub fn story_context(brain: &crate::Brain, last_label: &str) -> String {
    let recap = brain.recaps.iter().find(|r| r.label == last_label).or_else(|| brain.recaps.iter().max_by(|a, b| crate::brain::chapter_number(&a.label).unwrap_or(0.).total_cmp(&crate::brain::chapter_number(&b.label).unwrap_or(0.))));
    let mut cast: Vec<&crate::types::Character> = brain.characters.iter().filter(|c| !c.name.trim().is_empty()).collect();
    cast.sort_by(|a, b| b.appearances.cmp(&a.appearances));
    let names: Vec<&str> = cast.iter().take(8).map(|c| c.name.trim()).collect();
    let mut s = String::new();
    if let Some(r) = recap.filter(|r| !r.summary.trim().is_empty()) {
        s += &format!("STORY SO FAR: {}\n", truncate(r.summary.trim(), 2500));
    }
    if let Some(hero) = cast.first() {
        let mut who = hero.name.trim().to_string();
        if !hero.description.trim().is_empty() {
            who += &format!(": {}", truncate(hero.description.trim(), 300));
        }
        if !hero.traits.is_empty() {
            who += &format!(" Traits: {}.", hero.traits.iter().take(8).cloned().collect::<Vec<_>>().join(", "));
        }
        s += &format!("MAIN CHARACTER: {who}\n");
    }
    if !names.is_empty() {
        s += &format!("MAIN CHARACTERS: {}\n", names.join(", "));
    }
    s
}

/// Ids of a line: its "ids" (or a lone "id"), each a candidate not used yet, at most [`MAX_SHOTS`].
fn line_ids(l: &Value, candidates: &[HookCandidate], used: &[usize]) -> Vec<usize> {
    let listed: Vec<f64> = match l.get("ids").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.trim().parse().ok()))).collect(),
        None => num_of(l, "id").into_iter().collect(),
    };
    let mut out: Vec<usize> = vec![];
    for id in listed.into_iter().filter(|n| *n >= 0.).map(|n| n as usize) {
        if candidates.iter().any(|c| c.id == id) && !used.contains(&id) && !out.contains(&id) && out.len() < MAX_SHOTS {
            out.push(id);
        }
    }
    out
}

/// Reads the model's answer: the idea, lines whose ids are candidates (each id once, at most
/// [`MAX_LINES`] lines), the outro and its picture. Errors when fewer than [`MIN_LINES`] usable
/// lines came back.
pub(crate) fn parse_hook(v: &Value, candidates: &[HookCandidate]) -> Result<HookScript> {
    let mut lines: Vec<HookLine> = vec![];
    for l in v.get("lines").and_then(Value::as_array).into_iter().flatten() {
        let text = clean_line(&str_of(l, "text"));
        let used: Vec<usize> = lines.iter().flat_map(|l| l.ids.iter().copied()).collect();
        let ids = line_ids(l, candidates, &used);
        if text.is_empty() || ids.is_empty() {
            continue;
        }
        lines.push(HookLine { ids, text });
        if lines.len() == MAX_LINES {
            break;
        }
    }
    if lines.len() < MIN_LINES {
        bail!("the hook came back with {} usable line(s)", lines.len());
    }
    let outro = clean_line(&str_of(v, "outro"));
    let outro_id = num_of(v, "outro_id").filter(|n| *n >= 0.).map(|n| n as usize).filter(|id| candidates.iter().any(|c| c.id == *id));
    Ok(HookScript { idea: clean_line(&str_of(v, "idea")), lines, outro: if outro.is_empty() { FALLBACK_OUTRO.into() } else { outro }, outro_id })
}

/// Spoken text only: no markdown marks or bracketed directions.
fn clean_line(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in s.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = (depth - 1).max(0),
            '*' | '_' | '#' | '`' => {}
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The candidates as the model reads them: scene by scene, each beat with its id.
pub fn candidates_text(candidates: &[HookCandidate]) -> String {
    let scenes = scenes_of(candidates);
    let opening = candidates.first().map(|c| c.id);
    let mut s = String::new();
    for (k, scene) in scenes.iter().enumerate() {
        let note = if Some(scene[0].id) == opening { ", where the story begins" } else { "" };
        s += &format!("SCENE {} ({}{note}):\n", k + 1, scene[0].chapter);
        for c in scene.iter() {
            s += &format!("[{}] {}\n", c.id, truncate(&c.text, 220));
        }
        s.push('\n');
    }
    s
}

/// Asks `model` for a cold open of at most `seconds` over `candidates` (see [`pick_candidates`]).
/// Errors when the call fails or no usable script came back; [`fallback_hook`] is the way out.
/// `story` is [`story_context`] (may be empty).
pub async fn write_hook(reg: Arc<Registry>, model: ModelChoice, manga_title: &str, story: &str, candidates: &[HookCandidate], seconds: u32, cancel: CancellationToken) -> Result<HookScript> {
    write_hook_with(&reg, &model, manga_title, story, candidates, seconds, &cancel).await
}

pub(crate) async fn write_hook_with(llm: &dyn Llm, model: &ModelChoice, manga_title: &str, story: &str, candidates: &[HookCandidate], seconds: u32, cancel: &CancellationToken) -> Result<HookScript> {
    if candidates.len() < MIN_LINES {
        bail!("not enough narration to make a hook from");
    }
    let mut prompt = format!("MANGA: {manga_title}\nHOOK LENGTH: at most {seconds} seconds (about {} words at most)\n{story}\nCANDIDATES:\n\n", max_words(seconds));
    prompt += &candidates_text(candidates);
    prompt += "Pick the opening (a flash-forward or the premise) and the scene it is told over, then write the cold open. Reply with the JSON object only.";
    let ws = crate::store::studio_root().join("work").join("hook");
    let _ = std::fs::create_dir_all(&ws);
    let mut req = request(model, vec![AiMessage::system(hook_system(seconds)), AiMessage::user(prompt)], vec![], Some(ws));
    // It reads the whole story's scenes and picks one: a little more thought than a page read.
    req.effort = Some(mv_ai::AiEffort::Medium);
    match ask_json(llm, req, cancel).await? {
        Ok(v) => parse_hook(&v, candidates).map(|mut h| {
            h.fit(seconds);
            h
        }),
        Err(_) => bail!("the hook was not valid JSON"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn beats(label: &str, n: usize, strong: &[usize]) -> (String, Vec<String>) {
        let texts = (0..n)
            .map(|i| if strong.contains(&i) { format!("The demon king attacks and everyone watching is stunned by the deadly battle! Beat {i}.") } else { format!("They walk along the quiet road and talk a while, beat {i}.") })
            .collect();
        (label.to_string(), texts)
    }

    fn cand(id: usize, chapter: &str) -> HookCandidate {
        HookCandidate { id, chapter: chapter.into(), text: format!("beat {id}") }
    }

    #[test]
    fn scores_favour_action_and_exclamations_over_quiet_or_tiny_beats() {
        let quiet = beat_score("They walk along the quiet road and talk for a while.");
        let action = beat_score("The demon king attacks and the city burns in a deadly battle!");
        assert!(action > quiet + 1., "{action} vs {quiet}");
        assert!(beat_score("Hm.") < quiet);
        assert_eq!(beat_score("   "), 0.);
        // What a character loves counts too.
        assert!(beat_score("He sits on a bench and happily eats his ice cream.") > quiet + 1.);
        // A feat in front of stunned witnesses is a showcase.
        let feat = "He touches the device and it explodes.";
        assert!(showcase_score("Everyone watching is stunned: he touches the device and it explodes.") > showcase_score(feat) + 0.9);
    }

    #[test]
    fn candidates_are_whole_scenes_the_opening_first_then_the_strongest() {
        // Four chapters of 100 beats; chapter 4 has a showcase around beats 40-50.
        let mut chapters: Vec<_> = (0..3).map(|c| beats(&format!("Chapter {}", c + 1), 100, &[])).collect();
        chapters.push(beats("Chapter 4", 100, &(40..52).collect::<Vec<_>>()));
        let got = pick_candidates(&chapters, 64);
        assert!(got.len() <= 64 && got.len() >= 48, "{}", got.len());
        assert!(got.windows(2).all(|w| w[0].id < w[1].id), "story order");
        // The story's opening scene is always offered.
        assert_eq!(got[0].id, 0);
        assert!(got.iter().take(SCENE_BEATS).enumerate().all(|(i, c)| c.id == i), "the premise comes whole");
        // The showcase is offered whole, as a run of beats in a row.
        let scenes = scenes_of(&got);
        let show = scenes.iter().find(|s| s.iter().any(|c| c.id == 345)).expect("the showcase scene is offered");
        assert!(show.len() >= SCENE_BEATS && show.iter().any(|c| c.id == 340) && show.iter().any(|c| c.id == 351), "{:?}", show.iter().map(|c| c.id).collect::<Vec<_>>());
        assert_eq!(show[0].chapter, "Chapter 4");
        // The ids index all beats in story order.
        assert_eq!(chapters[3].1[345 - 300].trim(), got.iter().find(|c| c.id == 345).unwrap().text);
    }

    #[test]
    fn few_beats_are_all_offered_and_blank_ones_skipped() {
        let small = vec![("A".to_string(), vec!["One fight!".to_string(), " ".to_string()]), ("B".to_string(), vec!["Two strikes.".to_string()])];
        let got = pick_candidates(&small, 120);
        assert_eq!(got.iter().map(|c| c.id).collect::<Vec<_>>(), vec![0, 2]);
        assert!(pick_candidates(&[], 120).is_empty());
        // A small cap still offers a scene.
        let chapters = vec![beats("Chapter 1", 30, &[7, 21])];
        let got = pick_candidates(&chapters, 5);
        assert!(!got.is_empty() && got.len() <= 5, "{}", got.len());
    }

    #[test]
    fn scenes_split_where_the_beats_stop_running_on() {
        let c = vec![cand(0, "C1"), cand(1, "C1"), cand(5, "C1"), cand(6, "C2"), cand(7, "C2")];
        let s = scenes_of(&c);
        assert_eq!(s.iter().map(|s| s.iter().map(|c| c.id).collect::<Vec<_>>()).collect::<Vec<_>>(), vec![vec![0, 1], vec![5], vec![6, 7]]);
        let text = candidates_text(&c);
        assert!(text.starts_with("SCENE 1 (C1, where the story begins):\n[0] beat 0\n[1] beat 1\n"), "{text}");
        assert!(text.contains("SCENE 3 (C2):\n[6] beat 6\n[7] beat 7\n"), "{text}");
    }

    #[test]
    fn the_fallback_flashes_forward_to_the_strongest_late_scene() {
        let chapters = vec![beats("Chapter 1", 40, &[2]), beats("Chapter 2", 40, &[10, 12, 14])];
        let cands = pick_candidates(&chapters, 120);
        let h = fallback_hook(&cands, 30);
        assert!((MIN_LINES..=line_range(30).1).contains(&h.lines.len()), "{:?}", h.lines);
        let ids: Vec<usize> = h.lines.iter().flat_map(|l| l.ids.iter().copied()).collect();
        assert!(ids.contains(&50) && ids.contains(&52) && ids.contains(&54), "the showcase beats lead: {ids:?}");
        assert!(ids.iter().all(|&id| id >= 40), "one late scene: {ids:?}");
        assert!(ids.windows(2).all(|w| w[0] < w[1]), "story order: {ids:?}");
        assert!(h.lines.iter().all(|l| l.text.split_whitespace().count() <= LINE_WORDS && (1..=2).contains(&l.ids.len())), "{:?}", h.lines);
        assert!(h.words() <= max_words(30) + 3, "{} words", h.words());
        assert!(h.idea.is_empty() && h.outro == FALLBACK_OUTRO && h.outro_id == Some(0), "the turn plays over the story's first picture");
    }

    #[test]
    fn trimming_ends_on_a_clause() {
        assert_eq!(trim_words("One two three, four five six seven eight.", 5), "One two three...");
        assert_eq!(trim_words("Short line.", 10), "Short line.");
    }

    #[test]
    fn parsing_keeps_known_ids_and_cleans_text() {
        let cands = vec![cand(3, "C1"), cand(4, "C1"), cand(9, "C2")];
        let v = json!({"idea": "a *sleepy* hero", "lines": [{"ids": [3, 4, 99], "text": "**He** wakes (softly) up."}, {"id": 99, "text": "unknown"}, {"ids": ["9", 3], "text": "She falls."}], "outro": "Let's begin.", "outro_id": 4});
        let h = parse_hook(&v, &cands).unwrap();
        assert_eq!(h.lines, vec![HookLine::new(vec![3, 4], "He wakes up."), HookLine::new(vec![9], "She falls.")]);
        assert_eq!((h.outro.as_str(), h.outro_id), ("Let's begin.", Some(4)));
        assert_eq!(h.idea, "a sleepy hero");
        // A lone "id" still counts; too few usable lines is an error (the caller falls back).
        assert!(parse_hook(&json!({"lines": [{"id": 3, "text": "x"}]}), &cands).is_err());
        // No outro: a generic one, over the cover; an unknown outro picture is dropped.
        let h = parse_hook(&json!({"lines": [{"id": 3, "text": "x"}, {"id": 9, "text": "y"}], "outro_id": 77}), &cands).unwrap();
        assert_eq!((h.outro.as_str(), h.outro_id), (FALLBACK_OUTRO, None));
    }

    #[test]
    fn a_line_shows_at_most_three_pictures_and_a_hook_at_most_sixteen_lines() {
        let cands: Vec<HookCandidate> = (0..40).map(|id| cand(id, "C1")).collect();
        let v = json!({"lines": [{"ids": [0, 1, 2, 3, 4], "text": "Many."}]});
        assert!(parse_hook(&v, &cands).is_err(), "one line is too few");
        let v = json!({"lines": (0..20).map(|i| json!({"ids": [i * 2, i * 2 + 1], "text": "Line."})).collect::<Vec<_>>()});
        let h = parse_hook(&v, &cands).unwrap();
        assert_eq!(h.lines.len(), MAX_LINES);
        let v = json!({"lines": [{"ids": [0, 1, 2, 3, 4], "text": "Many."}, {"ids": [1, 5], "text": "Again."}]});
        let h = parse_hook(&v, &cands).unwrap();
        assert_eq!(h.lines, vec![HookLine::new(vec![0, 1, 2], "Many."), HookLine::new(vec![5], "Again.")], "an id plays once");
    }

    #[test]
    fn a_long_hook_drops_lines_from_the_middle() {
        let line = |id, n| HookLine::new(vec![id], vec!["word"; n].join(" "));
        let mut h = HookScript { idea: String::new(), lines: vec![line(1, 14), line(2, 14), line(3, 14), line(4, 14)], outro: "So how did he get here?".into(), outro_id: None };
        h.fit(15);
        assert_eq!(h.lines.iter().map(|l| l.ids[0]).collect::<Vec<_>>(), vec![1, 4], "the opening and the twist stay: {} words", h.words());
        h.fit(5);
        assert_eq!(h.lines.len(), MIN_LINES);
        let before = h.clone();
        h.fit(60);
        assert_eq!(h, before);
    }

    #[test]
    fn the_length_follows_the_setting() {
        assert_eq!((target_words(20), target_words(15), max_words(60)), (52, 39, 156));
        assert_eq!((line_range(15), line_range(30), line_range(60), line_range(120)), ((2, 3), (2, 7), (2, 15), (2, MAX_LINES)));
    }

    #[test]
    fn the_story_context_has_the_latest_recap_and_the_main_cast() {
        let mut b = crate::Brain::default();
        b.set_recap("Chapter 1", "Ruko joins the academy.");
        b.set_recap("Chapter 3", "Ruko defeats Kaito.");
        b.characters = vec![
            crate::types::Character { name: "Kaito".into(), appearances: 4, ..Default::default() },
            crate::types::Character { name: "Ruko".into(), appearances: 9, description: "A shy swordsman.".into(), traits: vec!["shy".into(), "kind".into()], ..Default::default() },
        ];
        let s = story_context(&b, "Chapter 9");
        assert!(s.contains("STORY SO FAR: Ruko defeats Kaito."), "{s}");
        assert!(s.contains("MAIN CHARACTER: Ruko: A shy swordsman. Traits: shy, kind."), "{s}");
        assert!(s.contains("MAIN CHARACTERS: Ruko, Kaito"), "{s}");
        assert!(story_context(&b, "Chapter 1").contains("Ruko joins the academy."));
        assert_eq!(story_context(&crate::Brain::default(), "Chapter 1"), "");
    }

    #[test]
    fn the_prompt_teaches_both_openings_and_the_turn() {
        let sys = hook_system(60);
        assert!(sys.contains("FLASH-FORWARD") && sys.contains("PREMISE") && sys.contains("THE TURN") && sys.contains("This guy is"), "{sys}");
        assert!(sys.contains("7-15 lines plus the outro, at most 156 words"), "{sys}");
    }
}
